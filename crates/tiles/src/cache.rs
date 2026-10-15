//! The tile cache: every tile and document fetched, decoded, by address.
//!
//! Drawing never waits. [`get`] answers what the cache holds and, for an address it doesn't hold
//! yet, starts fetching it; when the bytes arrive they are decoded, [`generation`] goes up and
//! the waker (the app's repaint) is called, so the map draws again with the new tile. Exports
//! and the command line, which can't draw a half-loaded map, [`wait`] for the fetches first.
//!
//! Natively, tiles are fetched on a few worker threads, the newest request first (the view the
//! user is looking at now, not the one they panned away from). The browser app installs its own
//! fetcher with [`set_fetcher`].

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use tiny_skia::Pixmap;
use web_time::{Duration, Instant};

use crate::mvt;

/// What a fetched address is decoded as.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Want {
    /// A PNG, JPEG or WebP tile.
    Image,
    /// A Mapbox Vector Tile.
    Vector,
    /// A JSON document (a style or TileJSON).
    Json,
}

/// A decoded fetch.
pub enum Payload {
    Image(Pixmap),
    Vector(mvt::Tile),
    Json(serde_json::Value),
    /// No content (a 204, or an empty body): nothing to draw.
    Empty,
}

/// What the cache knows about an address.
#[derive(Clone)]
pub enum State {
    Ready(Arc<Payload>),
    Pending,
    Failed(String),
}

/// Called once with a fetch's bytes or error.
pub type Done = Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>;
type Fetcher = Box<dyn Fn(String, Done) + Send + Sync>;

/// Bytes the cache keeps before dropping the least recently used entries.
const BUDGET: usize = if cfg!(target_arch = "wasm32") { 160 << 20 } else { 384 << 20 };
/// A failed address is tried again after this long.
const RETRY: Duration = Duration::from_secs(60);
/// The largest tile or document fetched.
pub const MAX_BYTES: usize = 16 << 20;
/// The error a fetcher gives a request it dropped unfetched (the view moved on): the address is
/// forgotten rather than failed, so it is fetched again when it is next wanted.
pub const DROPPED: &str = "dropped: the view moved on";
/// The end of a fetcher's error when a range was asked for and the server answered with the
/// whole file: a file read in place by range is downloaded whole instead.
pub const NO_RANGES: &str = "the server doesn't answer byte-range requests";

enum Entry {
    Ready(Arc<Payload>, usize),
    Pending,
    Failed(String, Instant),
}

struct Slot {
    entry: Entry,
    used: u64,
}

#[derive(Default)]
struct Cache {
    slots: HashMap<String, Slot>,
    bytes: usize,
    tick: u64,
}

fn cache() -> &'static Mutex<Cache> {
    static C: OnceLock<Mutex<Cache>> = OnceLock::new();
    C.get_or_init(Mutex::default)
}

fn lock() -> std::sync::MutexGuard<'static, Cache> {
    cache().lock().unwrap_or_else(|e| e.into_inner())
}

static GENERATION: AtomicU64 = AtomicU64::new(0);
static IN_FLIGHT: AtomicUsize = AtomicUsize::new(0);

fn fetcher() -> &'static Mutex<Option<Arc<Fetcher>>> {
    static F: OnceLock<Mutex<Option<Arc<Fetcher>>>> = OnceLock::new();
    F.get_or_init(Mutex::default)
}

fn waker() -> &'static Mutex<Option<Arc<dyn Fn() + Send + Sync>>> {
    static W: OnceLock<Mutex<Option<Arc<dyn Fn() + Send + Sync>>>> = OnceLock::new();
    W.get_or_init(Mutex::default)
}

/// Replaces how addresses are fetched (the browser's `fetch`). `done` must be called once.
pub fn set_fetcher(f: impl Fn(String, Done) + Send + Sync + 'static) {
    *fetcher().lock().unwrap_or_else(|e| e.into_inner()) = Some(Arc::new(Box::new(f)));
}

/// Sets what is called whenever a fetch finishes (the app asks for a repaint).
pub fn set_waker(f: impl Fn() + Send + Sync + 'static) {
    *waker().lock().unwrap_or_else(|e| e.into_inner()) = Some(Arc::new(f));
}

/// Goes up each time a fetch finishes: a map drawn at an older generation may now draw more.
pub fn generation() -> u64 {
    GENERATION.load(Ordering::Relaxed)
}

/// How many fetches are under way.
pub fn in_flight() -> usize {
    IN_FLIGHT.load(Ordering::Relaxed)
}

/// What the cache holds for `url`, fetching it when it holds nothing (or a failure old enough
/// to try again).
pub fn get(url: &str, want: Want) -> State {
    let mut c = lock();
    c.tick += 1;
    let tick = c.tick;
    if let Some(slot) = c.slots.get_mut(url) {
        slot.used = tick;
        match &slot.entry {
            Entry::Ready(p, _) => return State::Ready(p.clone()),
            Entry::Pending => return State::Pending,
            Entry::Failed(e, at) if at.elapsed() < RETRY => return State::Failed(e.clone()),
            Entry::Failed(..) => {}
        }
    }
    c.slots.insert(url.to_string(), Slot { entry: Entry::Pending, used: tick });
    drop(c);
    start(url.to_string(), want);
    State::Pending
}

/// What the cache holds for `url`, without fetching it.
pub fn peek(url: &str) -> Option<Arc<Payload>> {
    let mut c = lock();
    c.tick += 1;
    let tick = c.tick;
    let slot = c.slots.get_mut(url)?;
    match &slot.entry {
        Entry::Ready(p, _) => {
            slot.used = tick;
            Some(p.clone())
        }
        _ => None,
    }
}

/// Forgets everything (and failures, so they are tried again).
pub fn clear() {
    let mut c = lock();
    c.slots.retain(|_, s| matches!(s.entry, Entry::Pending));
    c.bytes = 0;
    drop(c);
    // Styles, TileJSON documents and PMTiles archives are read again too.
    crate::source::forget();
    crate::pmtiles::forget();
    crate::mbtiles::forget();
    crate::cog::forget();
    rustgis_data::source::remote::forget();
    GENERATION.fetch_add(1, Ordering::Relaxed);
}

/// Waits (up to `timeout`) for the fetches under way to finish. Native only: in the browser
/// fetches finish between frames, so this returns at once.
pub fn wait(timeout: Duration) {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let start = Instant::now();
        while in_flight() > 0 && start.elapsed() < timeout {
            std::thread::sleep(std::time::Duration::from_millis(15));
        }
    }
    #[cfg(target_arch = "wasm32")]
    let _ = timeout;
}

fn start(url: String, want: Want) {
    IN_FLIGHT.fetch_add(1, Ordering::Relaxed);
    let key = url.clone();
    let done: Done = Box::new(move |bytes| {
        if matches!(&bytes, Err(e) if e == DROPPED) {
            lock().slots.remove(&key);
            finished();
            return;
        }
        let entry = match bytes.and_then(|b| decode(&b, want)) {
            Ok(p) => {
                let size = payload_bytes(&p);
                Entry::Ready(Arc::new(p), size)
            }
            Err(e) => {
                log::debug!("{key}: {e}");
                Entry::Failed(e, Instant::now())
            }
        };
        store(&key, entry);
        finished();
    });
    // A tile from a PMTiles archive is read from the archive, by range.
    if crate::pmtiles::is_tile(&url) {
        crate::pmtiles::fetch(&url, done);
        return;
    }
    // A tile of a cloud-optimised GeoTIFF is drawn from the file's tiles, read by range.
    if crate::cog::is_tile(&url) {
        crate::cog::fetch(&url, done);
        return;
    }
    // A tile from an MBTiles archive is read from the file, or the upload in memory.
    if crate::mbtiles::is_tile(&url) {
        crate::mbtiles::fetch(&url, done);
        return;
    }
    fetch_bytes(url, done);
}

/// Marks the part of an address that asks for a byte range, `#rustgis-range=<first>-<last>`:
/// a fragment, so it never reaches the server, which gets a `Range` header instead.
pub(crate) const RANGE: &str = "#rustgis-range=";

/// Splits an address into the address to fetch and the byte range it asks for, if any.
pub fn split_range(url: &str) -> (&str, Option<(u64, u64)>) {
    match url.rsplit_once(RANGE) {
        Some((base, r)) => match r.split_once('-').and_then(|(a, b)| Some((a.parse().ok()?, b.parse().ok()?))) {
            Some((a, b)) if a <= b => (base, Some((a, b))),
            _ => (url, None),
        },
        None => (url, None),
    }
}

/// Marks an address asking for the file's last bytes, `#rustgis-tail=<n>` (`Range: bytes=-n`),
/// where its length isn't known (a GeoParquet's footer). A shorter answer is the whole file.
pub const TAIL: &str = "#rustgis-tail=";

/// Splits an address into the address to fetch and how many of its last bytes it asks for, if any.
pub fn split_tail(url: &str) -> (&str, Option<u64>) {
    match url.rsplit_once(TAIL) {
        Some((base, n)) => match n.parse::<u64>() {
            Ok(n) if n > 0 => (base, Some(n)),
            _ => (url, None),
        },
        None => (url, None),
    }
}

/// Starts an address that [`download`] made: the fetchers take up to [`MAX_DOWNLOAD`] bytes for
/// it, and [`MAX_BYTES`] (a tile) for any other. No tile, style or archive address can start with
/// it: those pass `address::check`, which takes only `http(s)://` ones.
pub const DOWNLOAD: &str = "rustgis-download:";

/// The most bytes a data download may have.
pub const MAX_DOWNLOAD: usize = if cfg!(target_arch = "wasm32") { 256 << 20 } else { 512 << 20 };

/// Splits an address into the address to fetch and whether it is a download ([`DOWNLOAD`]).
pub fn split_download(url: &str) -> (&str, bool) {
    match url.strip_prefix(DOWNLOAD) {
        Some(u) => (u, true),
        None => (url, false),
    }
}

/// The HTTP status a fetch's error is, if it is one: the whole message, as the native fetcher
/// (`http status: 404`) and the browser's (`HTTP 404`) give it — never a number found inside
/// another message (an address can hold "404").
pub fn http_status(error: &str) -> Option<u16> {
    let e = error.trim();
    let code = e.strip_prefix("http status: ").or_else(|| e.strip_prefix("HTTP "))?;
    (code.len() == 3 && code.bytes().all(|b| b.is_ascii_digit())).then(|| code.parse().ok()).flatten()
}

/// Downloads a file the user named (data added by its web address): up to [`MAX_DOWNLOAD`]
/// bytes, uncached, off the frame (a thread natively, the browser's fetch on the web), counted as
/// a fetch under way.
pub fn download(url: &str, done: Done) {
    if let Err(e) = crate::address::check(url, None) {
        done(Err(e));
        return;
    }
    let marked = format!("{DOWNLOAD}{url}");
    IN_FLIGHT.fetch_add(1, Ordering::Relaxed);
    let done: Done = Box::new(move |r| {
        done(r);
        finished();
    });
    let f = fetcher().lock().unwrap_or_else(|e| e.into_inner()).clone();
    match f {
        Some(f) => f(marked, done),
        None => native::download(marked, done),
    }
}

/// How long a download the caller waits for may take: commands run where the app waits too (the
/// AI Assistant's), so a large file comes through Add Data › From URL, which downloads off the
/// frame.
#[cfg(not(target_arch = "wasm32"))]
pub const BLOCKING_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

/// Downloads a file the user named, waiting for it (at most [`BLOCKING_TIMEOUT`]).
#[cfg(not(target_arch = "wasm32"))]
pub fn download_blocking(url: &str) -> Result<Vec<u8>, String> {
    crate::address::check(url, None)?;
    native::get(&native::download_agent(BLOCKING_TIMEOUT), &format!("{DOWNLOAD}{url}")).map_err(|e| {
        if e.contains("timeout") || e.contains("timed out") {
            format!("{e} (a large file: add it with Add Data › From URL, which downloads in the background)")
        } else {
            e
        }
    })
}

/// Fetches an address's bytes, uncached (through the browser's fetcher, or the worker threads).
pub(crate) fn fetch_bytes(url: String, done: Done) {
    let f = fetcher().lock().unwrap_or_else(|e| e.into_inner()).clone();
    match f {
        Some(f) => f(url, done),
        None => native::fetch(url, done),
    }
}

/// Like [`fetch_bytes`], counted as a fetch under way (so exports [`wait`] for it) and drawing
/// again when it lands: for what the map needs before it can ask for tiles (a PMTiles header).
pub(crate) fn fetch_counted(url: String, done: Done) {
    IN_FLIGHT.fetch_add(1, Ordering::Relaxed);
    fetch_bytes(
        url,
        Box::new(move |r| {
            done(r);
            finished();
        }),
    );
}

/// Reads a tile of a local MBTiles file on the fetch workers (counted already, as a tile).
pub(crate) fn fetch_worker(url: String, done: Done) {
    native::fetch(url, done);
}

/// Reads a range of a local file (`<path>#rustgis-range=a-b`) on the fetch workers, counted like
/// [`fetch_counted`]; never through an installed fetcher, which only knows web addresses.
pub(crate) fn fetch_local(url: String, done: Done) {
    IN_FLIGHT.fetch_add(1, Ordering::Relaxed);
    native::fetch_local(
        url,
        Box::new(move |r| {
            done(r);
            finished();
        }),
    );
}

/// Something the map draws from arrived outside the cache (a PMTiles archive's header): draw
/// again.
pub(crate) fn notify() {
    GENERATION.fetch_add(1, Ordering::Relaxed);
    let w = waker().lock().unwrap_or_else(|e| e.into_inner()).clone();
    if let Some(w) = w {
        w();
    }
}

/// A fetch is over: the map can draw again.
fn finished() {
    IN_FLIGHT.fetch_sub(1, Ordering::Relaxed);
    GENERATION.fetch_add(1, Ordering::Relaxed);
    let w = waker().lock().unwrap_or_else(|e| e.into_inner()).clone();
    if let Some(w) = w {
        w();
    }
}

fn store(url: &str, entry: Entry) {
    let mut c = lock();
    let added = if let Entry::Ready(_, n) = &entry { *n } else { 0 };
    // Dropped while it was fetched (cleared, or evicted): kept anyway, it was asked for.
    let tick = c.tick;
    let old = c.slots.insert(url.to_string(), Slot { entry, used: tick });
    if let Some(Slot { entry: Entry::Ready(_, n), .. }) = old {
        c.bytes = c.bytes.saturating_sub(n);
    }
    c.bytes += added;
    if c.bytes > BUDGET {
        evict(&mut c);
    }
}

/// Drops the least recently used entries until the cache is at three quarters of its budget.
fn evict(c: &mut Cache) {
    let mut ready: Vec<(u64, String, usize)> = c
        .slots
        .iter()
        .filter_map(|(k, s)| match s.entry {
            Entry::Ready(_, n) => Some((s.used, k.clone(), n)),
            Entry::Failed(..) => Some((s.used, k.clone(), 0)),
            Entry::Pending => None,
        })
        .collect();
    ready.sort_unstable_by_key(|r| r.0);
    for (_, k, n) in ready {
        if c.bytes <= BUDGET / 4 * 3 {
            break;
        }
        c.slots.remove(&k);
        c.bytes = c.bytes.saturating_sub(n);
    }
}

fn payload_bytes(p: &Payload) -> usize {
    match p {
        Payload::Image(pm) => pm.data().len(),
        Payload::Vector(t) => t
            .layers
            .iter()
            .flat_map(|l| &l.features)
            .map(|f| 64 + f.geometry.iter().map(|g| g.len() * 8 + 24).sum::<usize>() + f.props.len() * 48)
            .sum(),
        Payload::Json(_) => 64 << 10,
        Payload::Empty => 64,
    }
}

/// Decodes fetched bytes.
pub fn decode(bytes: &[u8], want: Want) -> Result<Payload, String> {
    if bytes.is_empty() {
        return Ok(Payload::Empty);
    }
    if bytes.len() > MAX_BYTES {
        return Err(format!("{} bytes is too large for a tile", bytes.len()));
    }
    match want {
        Want::Image => decode_image(bytes).map(Payload::Image),
        Want::Vector => mvt::decode(bytes).map(Payload::Vector),
        Want::Json => serde_json::from_slice(bytes).map(Payload::Json).map_err(|e| format!("not JSON: {e}")),
    }
}

/// Starts a picture drawn by RustGIS itself (a COG's map tile): its width and height (u32, little
/// end first), then straight RGBA pixels. No encoding, so no PNG encoder in the browser build.
pub(crate) const RAW: &[u8] = b"RUSTGIS-RGBA";

/// Decodes a PNG, JPEG or WebP picture into a premultiplied pixmap (at most 4096 pixels a side).
pub fn decode_image(bytes: &[u8]) -> Result<Pixmap, String> {
    if let Some(rest) = bytes.strip_prefix(RAW) {
        let dim = |at: usize| rest.get(at..at + 4).and_then(|b| b.try_into().ok()).map(u32::from_le_bytes).unwrap_or(0);
        let (w, h) = (dim(0), dim(4));
        let mut data = rest.get(8..).unwrap_or_default().to_vec();
        if w == 0 || h == 0 || w > 4096 || h > 4096 || data.len() != (w as usize) * (h as usize) * 4 {
            return Err("a picture of the wrong size".into());
        }
        premultiply(&mut data);
        return Pixmap::from_vec(data, tiny_skia::IntSize::from_wh(w, h).ok_or("an empty picture")?)
            .ok_or_else(|| "a picture of the wrong size".into());
    }
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format().map_err(|e| e.to_string())?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(4096);
    limits.max_image_height = Some(4096);
    limits.max_alloc = Some(96 << 20);
    reader.limits(limits);
    let img = reader.decode().map_err(|e| e.to_string())?.into_rgba8();
    let (w, h) = img.dimensions();
    let mut data = img.into_raw();
    premultiply(&mut data);
    Pixmap::from_vec(data, tiny_skia::IntSize::from_wh(w, h).ok_or("an empty picture")?).ok_or_else(|| "a picture of the wrong size".into())
}

/// Straight RGBA to premultiplied, in place.
fn premultiply(data: &mut [u8]) {
    for px in data.as_chunks_mut::<4>().0 {
        let a = u16::from(px[3]);
        if a < 255 {
            for c in &mut px[..3] {
                *c = ((u16::from(*c) * a + 127) / 255) as u8;
            }
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
mod native {
    //! Worker threads fetching with `ureq`, newest request first.

    use std::sync::{Condvar, Mutex, OnceLock};

    use super::{DROPPED, Done, MAX_BYTES};

    const WORKERS: usize = 8;
    /// Requests waiting beyond this many are dropped, oldest first: the view moved on.
    const QUEUE: usize = 512;

    struct Queue {
        jobs: Mutex<Vec<(String, Done)>>,
        ready: Condvar,
    }

    fn queue() -> &'static Queue {
        static Q: OnceLock<Queue> = OnceLock::new();
        Q.get_or_init(|| {
            for i in 0..WORKERS {
                let spawned = std::thread::Builder::new().name(format!("rustgis-tiles-{i}")).spawn(work);
                if let Err(e) = spawned {
                    log::error!("could not start a tile fetcher: {e}");
                }
            }
            Queue { jobs: Mutex::new(Vec::new()), ready: Condvar::new() }
        })
    }

    /// A local file's range, read on a worker.
    pub(super) fn fetch_local(url: String, done: Done) {
        fetch(format!("{LOCAL}{url}"), done);
    }

    /// Marks a queued job as a local file read.
    const LOCAL: &str = "local-file:";

    pub(super) fn fetch(url: String, done: Done) {
        let q = queue();
        let dropped = {
            let mut jobs = q.jobs.lock().unwrap_or_else(|e| e.into_inner());
            jobs.push((url, done));
            (jobs.len() > QUEUE).then(|| jobs.remove(0))
        };
        q.ready.notify_one();
        // Answered once the queue is unlocked: `done` takes the cache's lock and wakes the app.
        if let Some((_, done)) = dropped {
            done(Err(DROPPED.into()));
        }
    }

    pub(super) fn agent() -> ureq::Agent {
        ureq::Agent::config_builder()
            .timeout_global(Some(std::time::Duration::from_secs(30)))
            .user_agent(concat!("RustGIS/", env!("CARGO_PKG_VERSION"), " (+https://rustgis.app)"))
            // Redirects are followed by `get`, which checks where each one leads.
            .max_redirects(0)
            .build()
            .into()
    }

    /// An agent for downloads, which may take minutes.
    pub(super) fn download_agent(timeout: std::time::Duration) -> ureq::Agent {
        ureq::Agent::config_builder()
            .timeout_global(Some(timeout))
            .user_agent(concat!("RustGIS/", env!("CARGO_PKG_VERSION"), " (+https://rustgis.app)"))
            .max_redirects(0)
            .build()
            .into()
    }

    /// Downloads on a thread of its own, so a large file doesn't hold up the tiles.
    pub(super) fn download(url: String, done: Done) {
        std::thread::spawn(move || done(get(&download_agent(std::time::Duration::from_secs(900)), &url)));
    }

    fn work() {
        let agent = agent();
        let q = queue();
        loop {
            let job = {
                let mut jobs = q.jobs.lock().unwrap_or_else(|e| e.into_inner());
                loop {
                    if let Some(j) = jobs.pop() {
                        break j;
                    }
                    jobs = q.ready.wait(jobs).unwrap_or_else(|e| e.into_inner());
                }
            };
            let (url, done) = job;
            done(get(&agent, &url));
        }
    }

    /// Fetches `url`, following up to 5 redirects. `url` itself passed `address::check` when
    /// it was made (the layer's address, or one its style or TileJSON chose, checked in
    /// `source::resolve`); each redirect is checked here.
    pub(super) fn get(agent: &ureq::Agent, url: &str) -> Result<Vec<u8>, String> {
        use std::io::Read;
        if crate::mbtiles::is_tile(url) {
            return crate::mbtiles::read_local(url);
        }
        if let Some(local) = url.strip_prefix(LOCAL) {
            let (path, range) = super::split_range(local);
            let (a, b) = range.ok_or("a local read without a range")?;
            return crate::pmtiles::read_file(path, a, b - a + 1);
        }
        let (url, whole) = super::split_download(url);
        let max = if whole { super::MAX_DOWNLOAD } else { MAX_BYTES };
        let (url, tail) = super::split_tail(url);
        let (url, range) = super::split_range(url);
        // An `s3://` address becomes its web address, and a request to an S3 connection's
        // endpoint is signed.
        let prepared = crate::s3::prepare(url)?;
        let mut at = prepared.url.clone();
        let mut redirects = 0;
        let mut resp = loop {
            let mut req = agent.get(&at);
            if let Some((a, b)) = range {
                req = req.header("Range", &format!("bytes={a}-{b}"));
            } else if let Some(n) = tail {
                req = req.header("Range", &format!("bytes=-{n}"));
            }
            // A range of the file's own bytes: a server compressing its answer would count the
            // range in the compressed ones.
            if range.is_some() || tail.is_some() {
                req = req.header("Accept-Encoding", "identity");
            }
            if redirects == 0 {
                for (k, v) in &prepared.headers {
                    req = req.header(k.as_str(), v.as_str());
                }
            }
            let resp = req.call().map_err(|e| e.to_string())?;
            if !resp.status().is_redirection() {
                break resp;
            }
            // A signed request goes only where its credentials are for.
            if prepared.signed {
                return Err(format!(
                    "{url}: the S3 endpoint answered with a redirect (HTTP {}), which a signed request doesn't follow: check the connection's region and endpoint",
                    resp.status().as_u16()
                ));
            }
            let location = resp.headers().get("location").and_then(|v| v.to_str().ok()).ok_or("a redirect with no Location")?;
            let next = crate::template::join(&at, location);
            // Where a redirect leads is the server's choice, not the user's.
            crate::address::check(&next, Some(&at))?;
            // Never from HTTPS down to plain HTTP.
            if at.starts_with("https://") && !next.starts_with("https://") {
                return Err(format!("{next}: a redirect from https to http"));
            }
            redirects += 1;
            if redirects > 5 {
                return Err("too many redirects".into());
            }
            at = next;
        };
        if resp.status().as_u16() == 204 {
            return Ok(Vec::new());
        }
        // A range asked for and the whole file answered (200): fine for a range from the start
        // that holds the whole file (a small archive's header read), cut to the range; otherwise
        // the server doesn't do ranges.
        let whole = range.is_some() && resp.status().as_u16() != 206;
        if let Some((a, _)) = range.filter(|_| whole)
            && a > 0
        {
            return Err(format!("{url}: {}", super::NO_RANGES));
        }
        let mut buf = Vec::new();
        if !resp.status().is_success() {
            return Err(format!("HTTP {}", resp.status().as_u16()));
        }
        // The last bytes asked for and the whole file answered: fine when it is no longer.
        if let Some(n) = tail.filter(|_| resp.status().as_u16() != 206) {
            // Read one byte past `n` (no more, and no error at the limit): a longer file is
            // reported as a server without ranges, so the file is downloaded whole instead.
            let limit = n.saturating_add(1);
            resp.body_mut()
                .with_config()
                .limit(limit.saturating_add(1))
                .reader()
                .take(limit)
                .read_to_end(&mut buf)
                .map_err(|e| e.to_string())?;
            if buf.len() as u64 > n {
                return Err(format!("{url}: {}", super::NO_RANGES));
            }
            return Ok(buf);
        }
        resp.body_mut().with_config().limit(max as u64 + 1).reader().read_to_end(&mut buf).map_err(|e| e.to_string())?;
        if buf.len() > max {
            return Err(format!("larger than {} MB", max >> 20));
        }
        if let Some((a, b)) = range.filter(|_| whole) {
            if buf.len() as u64 > b - a + 1 {
                return Err(format!("{url}: {}", super::NO_RANGES));
            }
            buf.truncate(usize::try_from(b - a + 1).unwrap_or(usize::MAX));
        }
        Ok(buf)
    }
}

#[cfg(target_arch = "wasm32")]
mod native {
    use super::Done;

    pub(super) fn fetch(url: String, done: Done) {
        done(Err(format!("{url}: this build has no way to fetch tiles")));
    }

    pub(super) fn download(url: String, done: Done) {
        fetch(url, done);
    }

    pub(super) fn fetch_local(url: String, done: Done) {
        let (path, _) = super::split_range(&url);
        done(Err(format!("{path}: the web app reads PMTiles archives from web addresses only")));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn http_statuses_from_whole_messages() {
        assert_eq!(http_status("http status: 404"), Some(404));
        assert_eq!(http_status("HTTP 403"), Some(403));
        assert_eq!(http_status("HTTP 500"), Some(500));
        // A number inside another message is not a status.
        assert_eq!(http_status("https://x.org/404/a.zarr: timed out"), None);
        assert_eq!(http_status("HTTP 4040"), None);
        assert_eq!(http_status("io: connection reset (read 404 bytes)"), None);
    }

    #[test]
    fn decodes_pictures_and_documents() {
        let mut pm = Pixmap::new(2, 2).unwrap_or_else(|| panic!("pixmap"));
        pm.fill(tiny_skia::Color::from_rgba8(200, 100, 50, 128));
        let png = pm.encode_png().unwrap_or_default();
        let Payload::Image(back) = decode(&png, Want::Image).unwrap_or(Payload::Empty) else { panic!("not an image") };
        assert_eq!((back.width(), back.height()), (2, 2));
        assert!(matches!(decode(b"{\"a\": 1}", Want::Json), Ok(Payload::Json(_))));
        assert!(matches!(decode(b"", Want::Image), Ok(Payload::Empty)));
        assert!(decode(b"not a picture", Want::Image).is_err());
        assert!(decode(b"{", Want::Json).is_err());
    }

    /// A server on this computer answering each request with the next of `answers`.
    #[cfg(not(target_arch = "wasm32"))]
    fn serve(answers: Vec<String>) -> String {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            for answer in answers {
                let Ok((mut stream, _)) = listener.accept() else { return };
                let mut buf = [0u8; 2048];
                let _ = stream.read(&mut buf);
                let _ = stream.write_all(answer.as_bytes());
            }
        });
        format!("http://{addr}")
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn follows_only_allowed_redirects() {
        let redirect = |to: &str| format!("HTTP/1.1 302 Found\r\nLocation: {to}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
        let ok = "HTTP/1.1 200 OK\r\nContent-Length: 4\r\nConnection: close\r\n\r\ntile".to_string();
        let agent = native::agent();
        // Within the same server: followed.
        let base = serve(vec![redirect("/tile.png"), ok]);
        assert_eq!(native::get(&agent, &format!("{base}/old.png")).unwrap(), b"tile");
        // To a metadata address, or from HTTPS down to HTTP: refused.
        let base = serve(vec![redirect("http://169.254.169.254/latest/meta-data/")]);
        assert!(native::get(&agent, &format!("{base}/x")).unwrap_err().contains("never fetched"));
        // Too many.
        let base = serve((0..7).map(|_| redirect("/again")).collect());
        assert!(native::get(&agent, &format!("{base}/x")).unwrap_err().contains("too many"));
    }

    #[test]
    fn fetches_through_the_fetcher_once() {
        static CALLS: AtomicUsize = AtomicUsize::new(0);
        set_fetcher(|url, done| {
            CALLS.fetch_add(1, Ordering::Relaxed);
            if url.ends_with("ok.json") { done(Ok(b"[1, 2]".to_vec())) } else { done(Err("404".into())) }
        });
        let g = generation();
        assert!(matches!(get("test://cache/ok.json", Want::Json), State::Pending));
        assert!(generation() > g);
        assert!(matches!(get("test://cache/ok.json", Want::Json), State::Ready(_)));
        assert!(peek("test://cache/ok.json").is_some());
        assert!(matches!(get("test://cache/missing.json", Want::Json), State::Pending));
        assert!(matches!(get("test://cache/missing.json", Want::Json), State::Failed(_)));
        assert!(peek("test://cache/never.json").is_none());
        // A request the fetcher dropped is forgotten, not failed: asked for again, it is fetched.
        set_fetcher(|url, done| {
            CALLS.fetch_add(1, Ordering::Relaxed);
            if url.ends_with("first.json") { done(Err(DROPPED.into())) } else { done(Ok(b"1".to_vec())) }
        });
        assert!(matches!(get("test://cache/first.json", Want::Json), State::Pending));
        assert!(matches!(get("test://cache/first.json", Want::Json), State::Pending), "fetched again");
        assert_eq!(CALLS.load(Ordering::Relaxed), 4);
    }
}
