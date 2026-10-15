//! PMTiles (v3): a whole tile set in one file, read a tile at a time with byte-range requests
//! (or from disk), so a vector or raster tile set needs no tile server.
//!
//! A tile's address is `pmtiles:{z}/{x}/{y}@<archive>`, the archive being an `http(s)` address
//! or, on the desktop, a file path. Fetching it reads the archive's header and root directory
//! (the first 16 KB, once), the leaf directory holding the tile if there is one (cached), then
//! the tile's bytes. Everything goes through the cache's fetcher, so it works in the browser
//! too, where the server must allow range requests (and CORS).
//!
//! Archives are hostile input: offsets, lengths and directory sizes are checked against the
//! file and bounded, and nesting is limited.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use serde_json::Value;
use web_time::{Duration, Instant};

use crate::cache::{Done, MAX_BYTES};

/// The scheme of a tile address read from a PMTiles archive.
pub const SCHEME: &str = "pmtiles:";

/// The header and root directory are within the first 16 KB of an archive.
const HEAD: u64 = 16_384;
/// The most entries a directory may hold.
const MAX_ENTRIES: usize = 1 << 20;
/// Leaf directories go no deeper than this.
const MAX_DEPTH: usize = 4;
/// Leaf directories kept.
const MAX_LEAVES: usize = 256;
/// A failed archive is tried again after this long.
const RETRY: Duration = Duration::from_secs(60);

/// What the tiles are.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TileType {
    Unknown,
    Mvt,
    Png,
    Jpeg,
    Webp,
    Avif,
}

impl TileType {
    pub fn is_vector(self) -> bool {
        self == TileType::Mvt
    }
}

/// An archive's header and metadata.
#[derive(Clone, Debug)]
pub struct Info {
    pub tile_type: TileType,
    pub min_zoom: u8,
    pub max_zoom: u8,
    /// Longitude/latitude bounds: west, south, east, north.
    pub bounds: [f64; 4],
    /// The JSON metadata (vector layers, attribution…), `Null` when there is none.
    pub metadata: Value,
    root: Arc<Vec<Entry>>,
    leaf_offset: u64,
    leaf_length: u64,
    data_offset: u64,
    data_length: u64,
    internal_gzip: bool,
    tile_gzip: bool,
}

impl Info {
    /// The names of the metadata's vector layers.
    pub fn vector_layers(&self) -> Vec<String> {
        self.metadata
            .get("vector_layers")
            .and_then(Value::as_array)
            .map(|v| v.iter().filter_map(|l| l.get("id").and_then(Value::as_str)).map(str::to_string).collect())
            .unwrap_or_default()
    }

    /// The metadata's attribution, if any.
    pub fn attribution(&self) -> String {
        self.metadata.get("attribution").and_then(Value::as_str).unwrap_or_default().to_string()
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Entry {
    tile_id: u64,
    offset: u64,
    length: u64,
    run_length: u64,
}

/// The template for an archive's tiles.
pub fn template(archive: &str) -> String {
    format!("{SCHEME}{{z}}/{{x}}/{{y}}@{archive}")
}

/// Whether an address is a tile read from a PMTiles archive.
pub fn is_tile(url: &str) -> bool {
    url.starts_with(SCHEME)
}

/// Whether an archive address is a file on this computer rather than a web address.
/// (As `rustgis_map::tiles::is_local_archive` decides it, which a layer's address went through.)
pub fn is_local(archive: &str) -> bool {
    let lower = archive.get(..8).unwrap_or(archive).to_ascii_lowercase();
    !(lower.starts_with("http://") || lower.starts_with("https://") || lower.starts_with("s3://"))
}

/// Whether a local archive may be read: an absolute path to a `.pmtiles` file (checked again
/// here, whatever route the address came by).
pub fn check_local(archive: &str) -> Result<(), String> {
    let p = archive.strip_prefix("file://").unwrap_or(archive);
    let drive = p.as_bytes().get(1) == Some(&b':') && p.as_bytes().first().is_some_and(u8::is_ascii_alphabetic);
    // A path on this computer: not a network share (\\server\share, or file://server/…), which a
    // project could use to make Windows connect to another machine.
    let share = p.starts_with("\\\\") || p.starts_with("//") || (archive.starts_with("file://") && !p.starts_with('/'));
    if share || !(p.starts_with('/') || drive) || p.chars().any(char::is_control) || p.len() > 4096 {
        return Err(format!("'{p}': a PMTiles file is given by its full path on this computer"));
    }
    if !p.to_ascii_lowercase().ends_with(".pmtiles") {
        return Err(format!("'{p}' isn't a .pmtiles file"));
    }
    Ok(())
}

/// The tile id of z/x/y: tiles of lower zooms first, then along a Hilbert curve.
pub fn tile_id(z: u8, x: u32, y: u32) -> u64 {
    let z = u32::from(z.min(31));
    let base = ((1u64 << (2 * z)) - 1) / 3;
    let n = 1u64 << z;
    let (mut x, mut y) = (u64::from(x), u64::from(y));
    let mut d = 0u64;
    let mut s = n / 2;
    while s > 0 {
        let rx = u64::from(x & s > 0);
        let ry = u64::from(y & s > 0);
        d += s * s * ((3 * rx) ^ ry);
        // Only the bits below `s` matter from here; rotate the quadrant they're in.
        x &= s - 1;
        y &= s - 1;
        if ry == 0 {
            if rx == 1 {
                x = s - 1 - x;
                y = s - 1 - y;
            }
            std::mem::swap(&mut x, &mut y);
        }
        s /= 2;
    }
    base + d
}

enum State {
    Loading(Vec<Box<dyn FnOnce(Result<Arc<Info>, String>) + Send>>),
    Ready(Arc<Info>),
    Failed(String, Instant),
}

fn archives() -> &'static Mutex<HashMap<String, State>> {
    static A: OnceLock<Mutex<HashMap<String, State>>> = OnceLock::new();
    A.get_or_init(Mutex::default)
}

/// A leaf directory: read, or being read with tiles waiting for it.
enum Leaf {
    Ready(Arc<Vec<Entry>>),
    Loading(Vec<Box<dyn FnOnce(Result<Arc<Vec<Entry>>, String>) + Send>>),
}

fn leaves() -> &'static Mutex<HashMap<(String, u64), Leaf>> {
    static L: OnceLock<Mutex<HashMap<(String, u64), Leaf>>> = OnceLock::new();
    L.get_or_init(Mutex::default)
}

/// Forgets the archives read (with the tile cache).
pub(crate) fn forget() {
    archives().lock().unwrap_or_else(|e| e.into_inner()).retain(|_, s| matches!(s, State::Loading(_)));
    // Reads under way are kept: their waiting tiles are answered when they land.
    leaves().lock().unwrap_or_else(|e| e.into_inner()).retain(|_, l| matches!(l, Leaf::Loading(_)));
}

/// An archive's header and metadata: `Ok(None)` while they are read (the map draws again once
/// they arrive).
pub fn info(archive: &str) -> Result<Option<Arc<Info>>, String> {
    let mut all = archives().lock().unwrap_or_else(|e| e.into_inner());
    match all.get(archive) {
        Some(State::Ready(i)) => return Ok(Some(i.clone())),
        Some(State::Loading(_)) => return Ok(None),
        Some(State::Failed(e, at)) if at.elapsed() < RETRY => return Err(e.clone()),
        _ => {}
    }
    all.insert(archive.to_string(), State::Loading(Vec::new()));
    drop(all);
    load(archive.to_string());
    Ok(None)
}

/// Calls `f` with the archive's info, once it is read.
fn with_info(archive: &str, f: Box<dyn FnOnce(Result<Arc<Info>, String>) + Send>) {
    let mut all = archives().lock().unwrap_or_else(|e| e.into_inner());
    match all.get_mut(archive) {
        Some(State::Ready(i)) => {
            let i = i.clone();
            drop(all);
            f(Ok(i));
        }
        Some(State::Loading(waiting)) => waiting.push(f),
        Some(State::Failed(e, at)) if at.elapsed() < RETRY => {
            let e = e.clone();
            drop(all);
            f(Err(e));
        }
        _ => {
            all.insert(archive.to_string(), State::Loading(vec![f]));
            drop(all);
            load(archive.to_string());
        }
    }
}

/// Reads an archive's header, root directory and metadata, then answers whoever waits.
fn load(archive: String) {
    let a = archive.clone();
    read(
        &archive,
        0,
        HEAD,
        Box::new(move |head| {
            let parsed = head.and_then(|b| parse_head(&b));
            match parsed {
                Err(e) => finish(&a, Err(e)),
                Ok((mut info, meta_at, meta_len)) => {
                    if meta_len == 0 {
                        finish(&a, Ok(info));
                        return;
                    }
                    let a2 = a.clone();
                    read(
                        &a,
                        meta_at,
                        meta_len,
                        Box::new(move |m| {
                            // Unreadable metadata only loses the layer names and credits.
                            if let Ok(m) = m {
                                let m = if info.internal_gzip { gunzip(&m).unwrap_or_default() } else { m };
                                info.metadata = serde_json::from_slice(&m).unwrap_or(Value::Null);
                            }
                            finish(&a2, Ok(info));
                        }),
                    );
                }
            }
        }),
    );
}

fn finish(archive: &str, r: Result<Info, String>) {
    let r = r.map(Arc::new);
    let waiting = {
        let mut all = archives().lock().unwrap_or_else(|e| e.into_inner());
        let old = match &r {
            Ok(i) => all.insert(archive.to_string(), State::Ready(i.clone())),
            // A read the busy queue dropped isn't the archive's fault: forgotten, so the next
            // draw reads it again, rather than kept as a failure.
            Err(e) if e == crate::cache::DROPPED => all.remove(archive),
            Err(e) => all.insert(archive.to_string(), State::Failed(e.clone(), Instant::now())),
        };
        match old {
            Some(State::Loading(w)) => w,
            _ => Vec::new(),
        }
    };
    for w in waiting {
        w(r.clone());
    }
    crate::cache::notify();
}

/// Fetches a tile address (`pmtiles:z/x/y@archive`): its bytes, or none for a tile the archive
/// doesn't have.
pub(crate) fn fetch(url: &str, done: Done) {
    let Some((z, x, y, archive)) = parse_url(url) else {
        done(Err(format!("{url}: not a PMTiles tile address")));
        return;
    };
    let a = archive.clone();
    with_info(
        &archive,
        Box::new(move |info| match info {
            Err(e) => done(Err(e)),
            Ok(info) => {
                // Outside the archive's zooms, or not a tile of its zoom: none.
                let n = 1u64 << z;
                if z < info.min_zoom || z > info.max_zoom || u64::from(x) >= n || u64::from(y) >= n {
                    done(Ok(Vec::new()));
                    return;
                }
                let id = tile_id(z, x, y);
                let root = info.root.clone();
                find(a, info, id, root, 0, done);
            }
        }),
    );
}

/// Looks `id` up in a directory, reading leaf directories as needed, then reads the tile.
fn find(archive: String, info: Arc<Info>, id: u64, dir: Arc<Vec<Entry>>, depth: usize, done: Done) {
    let Some(e) = lookup(&dir, id) else {
        done(Ok(Vec::new()));
        return;
    };
    if e.run_length > 0 {
        let gz = info.tile_gzip;
        // Within the tile data section (when the header gives its length).
        let inside = info.data_length == 0 || e.offset.saturating_add(e.length) <= info.data_length;
        let Some(at) = info.data_offset.checked_add(e.offset).filter(|_| inside) else {
            done(Err("a tile outside the archive".into()));
            return;
        };
        read(&archive, at, e.length, Box::new(move |b| done(b.and_then(|b| if gz { gunzip(&b) } else { Ok(b) }))));
        return;
    }
    // A leaf directory.
    if depth >= MAX_DEPTH {
        done(Err("PMTiles directories nested too deep".into()));
        return;
    }
    if e.offset.saturating_add(e.length) > info.leaf_length {
        done(Err("a leaf directory outside the archive".into()));
        return;
    }
    let at = info.leaf_offset.saturating_add(e.offset);
    let key = (archive.clone(), at);
    // Continues the lookup once the leaf is there.
    let next: Box<dyn FnOnce(Result<Arc<Vec<Entry>>, String>) + Send> = {
        let (archive, info) = (archive.clone(), info.clone());
        Box::new(move |leaf| match leaf {
            Ok(leaf) => find(archive, info, id, leaf, depth + 1, done),
            Err(e) => done(Err(e)),
        })
    };
    // One read per leaf: tiles needing a leaf already on its way wait for it.
    {
        let mut l = leaves().lock().unwrap_or_else(|e| e.into_inner());
        match l.get_mut(&key) {
            Some(Leaf::Ready(leaf)) => {
                let leaf = leaf.clone();
                drop(l);
                next(Ok(leaf));
                return;
            }
            Some(Leaf::Loading(waiting)) => {
                waiting.push(next);
                return;
            }
            None => {
                // Full: one ready leaf makes room (the cache is a working set, not a history).
                if l.len() >= MAX_LEAVES
                    && let Some(old) = l.iter().find(|(_, v)| matches!(v, Leaf::Ready(_))).map(|(k, _)| k.clone())
                {
                    l.remove(&old);
                }
                l.insert(key.clone(), Leaf::Loading(vec![next]));
            }
        }
    }
    read(
        &archive,
        at,
        e.length,
        Box::new(move |b| {
            let leaf = b.and_then(|b| if info.internal_gzip { gunzip(&b) } else { Ok(b) }).and_then(|b| parse_dir(&b)).map(Arc::new);
            let waiting = {
                let mut l = leaves().lock().unwrap_or_else(|e| e.into_inner());
                let w = match &leaf {
                    Ok(leaf) => l.insert(key.clone(), Leaf::Ready(leaf.clone())),
                    // A failed leaf is read again by the next tile that needs it.
                    Err(_) => l.remove(&key),
                };
                match w {
                    Some(Leaf::Loading(w)) => w,
                    _ => Vec::new(),
                }
            };
            for w in waiting {
                w(leaf.clone());
            }
        }),
    );
}

/// The entry holding `id`: the last at or before it, when it is a leaf directory or its run
/// covers `id`.
fn lookup(dir: &[Entry], id: u64) -> Option<Entry> {
    let i = dir.partition_point(|e| e.tile_id <= id);
    let e = *dir.get(i.checked_sub(1)?)?;
    if e.run_length == 0 || id < e.tile_id.saturating_add(e.run_length) { Some(e) } else { None }
}

/// `pmtiles:z/x/y@archive` → its parts.
fn parse_url(url: &str) -> Option<(u8, u32, u32, String)> {
    let rest = url.strip_prefix(SCHEME)?;
    let (zxy, archive) = rest.split_once('@')?;
    let mut p = zxy.split('/');
    let z: u8 = p.next()?.parse().ok()?;
    let x: u32 = p.next()?.parse().ok()?;
    let y: u32 = p.next()?.parse().ok()?;
    (z <= 30 && !archive.is_empty()).then(|| (z, x, y, archive.to_string()))
}

/// Reads `len` bytes at `at` of an archive.
fn read(archive: &str, at: u64, len: u64, done: Done) {
    if len == 0 {
        done(Ok(Vec::new()));
        return;
    }
    if len > MAX_BYTES as u64 {
        done(Err(format!("{len} bytes is too large to read from an archive")));
        return;
    }
    let end = at.saturating_add(len) - 1;
    let url = format!("{archive}{}{at}-{end}", crate::cache::RANGE);
    if is_local(archive) {
        if let Err(e) = check_local(archive) {
            done(Err(e));
            return;
        }
        // Read on the fetch workers, as web archives are, never on the frame.
        crate::cache::fetch_local(url, done);
        return;
    }
    crate::cache::fetch_counted(url, done);
}

/// Reads a range of a local archive (on a fetch worker).
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn read_file(archive: &str, at: u64, len: u64) -> Result<Vec<u8>, String> {
    use std::io::{Read, Seek, SeekFrom};
    let path = archive.strip_prefix("file://").unwrap_or(archive);
    let mut f = std::fs::File::open(path).map_err(|e| format!("{path}: {e}"))?;
    f.seek(SeekFrom::Start(at)).map_err(|e| e.to_string())?;
    let mut buf = Vec::with_capacity(usize::try_from(len).unwrap_or(0).min(MAX_BYTES));
    f.take(len).read_to_end(&mut buf).map_err(|e| e.to_string())?;
    Ok(buf)
}

fn u64_at(b: &[u8], i: usize) -> Option<u64> {
    Some(u64::from_le_bytes(b.get(i..i + 8)?.try_into().ok()?))
}

fn i32_at(b: &[u8], i: usize) -> Option<f64> {
    Some(f64::from(i32::from_le_bytes(b.get(i..i + 4)?.try_into().ok()?)) / 1e7)
}

/// The header and root directory, and where the metadata is.
fn parse_head(b: &[u8]) -> Result<(Info, u64, u64), String> {
    if b.get(..7) != Some(b"PMTiles".as_slice()) {
        return Err("not a PMTiles archive".into());
    }
    if b.get(7) != Some(&3) {
        return Err(format!("PMTiles version {} isn't read (only 3)", b.get(7).copied().unwrap_or(0)));
    }
    let g = |i| u64_at(b, i).ok_or("a truncated PMTiles header");
    let (root_at, root_len, meta_at, meta_len) = (g(8)?, g(16)?, g(24)?, g(32)?);
    let (leaf_offset, leaf_length, data_offset, data_length) = (g(40)?, g(48)?, g(56)?, g(64)?);
    let byte = |i: usize| b.get(i).copied().ok_or("a truncated PMTiles header");
    let compression = |c: u8, what: &str| match c {
        0 | 1 => Ok(false),
        2 => Ok(true),
        3 => Err(format!("{what} compressed with Brotli isn't read")),
        4 => Err(format!("{what} compressed with Zstandard isn't read")),
        c => Err(format!("unknown {what} compression {c}")),
    };
    let internal_gzip = compression(byte(97)?, "directories")?;
    let tile_gzip = compression(byte(98)?, "tiles")?;
    let tile_type = match byte(99)? {
        1 => TileType::Mvt,
        2 => TileType::Png,
        3 => TileType::Jpeg,
        4 => TileType::Webp,
        5 => TileType::Avif,
        _ => TileType::Unknown,
    };
    let (min_zoom, max_zoom) = (byte(100)?.min(30), byte(101)?.min(30));
    let c = |i| i32_at(b, i).ok_or("a truncated PMTiles header");
    let bounds = [c(102)?, c(106)?, c(110)?, c(114)?];
    // The root directory is within the first 16 KB.
    let root_end = root_at.checked_add(root_len).filter(|e| *e <= b.len() as u64).ok_or("the root directory isn't where it should be")?;
    let raw = b
        .get(usize::try_from(root_at).map_err(|_| "bad offset")?..usize::try_from(root_end).map_err(|_| "bad offset")?)
        .ok_or("bad offset")?;
    let raw = if internal_gzip { gunzip(raw)? } else { raw.to_vec() };
    let root = parse_dir(&raw)?;
    let info = Info {
        tile_type,
        min_zoom: min_zoom.min(max_zoom),
        max_zoom,
        bounds,
        metadata: Value::Null,
        root: Arc::new(root),
        leaf_offset,
        leaf_length,
        data_offset,
        data_length,
        internal_gzip,
        tile_gzip,
    };
    Ok((info, meta_at, meta_len.min(MAX_BYTES as u64)))
}

/// A directory: delta-coded tile ids, then run lengths, lengths and offsets, all varints.
fn parse_dir(b: &[u8]) -> Result<Vec<Entry>, String> {
    let mut r = Varints { b, at: 0 };
    let n = usize::try_from(r.next()?).map_err(|_| "a directory too large")?;
    // Each entry takes at least four bytes.
    if n > MAX_ENTRIES || n.saturating_mul(4) > b.len() {
        return Err("a directory with more entries than it can hold".into());
    }
    let mut entries = vec![Entry { tile_id: 0, offset: 0, length: 0, run_length: 0 }; n];
    let mut last = 0u64;
    for e in entries.iter_mut() {
        last = last.checked_add(r.next()?).ok_or("tile ids overflow")?;
        e.tile_id = last;
    }
    for e in entries.iter_mut() {
        e.run_length = r.next()?;
    }
    for e in entries.iter_mut() {
        e.length = r.next()?;
    }
    for i in 0..n {
        let v = r.next()?;
        let prev = if i > 0 { entries.get(i - 1).copied() } else { None };
        let e = entries.get_mut(i).ok_or("a short directory")?;
        e.offset = match (v, prev) {
            // Right after the previous entry.
            (0, Some(p)) => p.offset.saturating_add(p.length),
            (v, _) => v.saturating_sub(1),
        };
    }
    Ok(entries)
}

struct Varints<'a> {
    b: &'a [u8],
    at: usize,
}

impl Varints<'_> {
    fn next(&mut self) -> Result<u64, String> {
        let mut v = 0u64;
        for shift in (0..64).step_by(7) {
            let byte = *self.b.get(self.at).ok_or("a truncated directory")?;
            self.at += 1;
            v |= u64::from(byte & 0x7f) << shift;
            if byte & 0x80 == 0 {
                return Ok(v);
            }
        }
        Err("a varint too long".into())
    }
}

/// Inflates gzip (or zlib) bytes, bounded.
fn gunzip(b: &[u8]) -> Result<Vec<u8>, String> {
    crate::mvt::inflate(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tile_ids_follow_the_spec() {
        assert_eq!(tile_id(0, 0, 0), 0);
        // Zoom 1: (0,0), (0,1), (1,1), (1,0).
        assert_eq!([tile_id(1, 0, 0), tile_id(1, 0, 1), tile_id(1, 1, 1), tile_id(1, 1, 0)], [1, 2, 3, 4]);
        assert_eq!(tile_id(2, 0, 0), 5);
        // Every id of zoom 3 once.
        let mut ids: Vec<u64> = (0..8).flat_map(|x| (0..8).map(move |y| tile_id(3, x, y))).collect();
        ids.sort_unstable();
        assert_eq!(ids, (21..85).collect::<Vec<_>>());
    }

    #[test]
    fn directories_round_trip_and_hostile_ones_fail() {
        // Three entries: ids 1, 2, 5; runs 1, 1, 0 (a leaf); lengths 10, 20, 30; offsets 0, next, 100.
        let mut b = vec![3u8];
        b.extend([1, 1, 3]);
        b.extend([1, 1, 0]);
        b.extend([10, 20, 30]);
        b.extend([1, 0, 101]);
        let d = parse_dir(&b).unwrap();
        assert_eq!(
            d.iter().map(|e| (e.tile_id, e.offset, e.length, e.run_length)).collect::<Vec<_>>(),
            vec![(1, 0, 10, 1), (2, 10, 20, 1), (5, 100, 30, 0)]
        );
        assert_eq!(lookup(&d, 2).map(|e| e.offset), Some(10));
        assert_eq!(lookup(&d, 3), None);
        assert_eq!(lookup(&d, 9).map(|e| e.run_length), Some(0));
        assert!(parse_dir(&[0xff, 0xff, 0xff, 0xff, 0x7f]).is_err());
        assert!(parse_dir(&[]).is_err());
        for n in 0..b.len() {
            let _ = parse_dir(&b[..n]);
        }
        assert!(parse_head(b"PMTiles\x02").is_err());
        assert!(parse_head(b"nope").is_err());
        assert!(parse_url("pmtiles:3/1/2@https://x/a.pmtiles").is_some());
        assert!(parse_url("pmtiles:99/1/2@x").is_none() && parse_url("pmtiles:1/2@x").is_none());
    }

    fn varint(out: &mut Vec<u8>, mut v: u64) {
        loop {
            let b = (v & 0x7f) as u8;
            v >>= 7;
            if v == 0 {
                out.push(b);
                return;
            }
            out.push(b | 0x80);
        }
    }

    fn dir(entries: &[Entry]) -> Vec<u8> {
        let mut b = Vec::new();
        varint(&mut b, entries.len() as u64);
        let mut last = 0;
        for e in entries {
            varint(&mut b, e.tile_id - last);
            last = e.tile_id;
        }
        for e in entries {
            varint(&mut b, e.run_length);
        }
        for e in entries {
            varint(&mut b, e.length);
        }
        for e in entries {
            varint(&mut b, e.offset + 1);
        }
        b
    }

    /// A PMTiles archive of `tiles` (uncompressed), its directory in the root or in a leaf.
    pub(crate) fn archive(tiles: &[(u8, u32, u32, Vec<u8>)], tile_type: u8, leaf: bool, metadata: &str) -> Vec<u8> {
        let mut tiles: Vec<(u64, &Vec<u8>)> = tiles.iter().map(|(z, x, y, b)| (tile_id(*z, *x, *y), b)).collect();
        tiles.sort_by_key(|t| t.0);
        let mut data = Vec::new();
        let mut entries = Vec::new();
        for (id, b) in &tiles {
            entries.push(Entry { tile_id: *id, offset: data.len() as u64, length: b.len() as u64, run_length: 1 });
            data.extend_from_slice(b);
        }
        let (root, leaves) = if leaf {
            let l = dir(&entries);
            (dir(&[Entry { tile_id: entries[0].tile_id, offset: 0, length: l.len() as u64, run_length: 0 }]), l)
        } else {
            (dir(&entries), Vec::new())
        };
        let meta = metadata.as_bytes();
        let root_at = 127u64;
        let meta_at = root_at + root.len() as u64;
        let leaf_at = meta_at + meta.len() as u64;
        let data_at = leaf_at + leaves.len() as u64;
        let mut h = b"PMTiles".to_vec();
        h.push(3);
        for v in [root_at, root.len() as u64, meta_at, meta.len() as u64, leaf_at, leaves.len() as u64, data_at, data.len() as u64] {
            h.extend_from_slice(&v.to_le_bytes());
        }
        h.extend_from_slice(&[0u8; 24]);
        h.push(1); // clustered
        h.extend_from_slice(&[1, 1, tile_type, 0, 2]); // no compression; type; zooms 0–2
        for v in [-1_800_000_000i32, -850_000_000, 1_800_000_000, 850_000_000] {
            h.extend_from_slice(&v.to_le_bytes());
        }
        h.push(0);
        h.extend_from_slice(&[0u8; 8]);
        assert_eq!(h.len(), 127);
        [h, root, meta.to_vec(), leaves, data].concat()
    }

    fn get(url: &str) -> Result<Vec<u8>, String> {
        let (tx, rx) = std::sync::mpsc::channel();
        fetch(url, Box::new(move |r| tx.send(r).unwrap_or_default()));
        rx.recv_timeout(std::time::Duration::from_secs(10)).map_err(|e| e.to_string())?
    }

    #[test]
    fn reads_tiles_from_root_and_leaf_directories() {
        let dir = std::env::temp_dir().join(format!("rustgis-pmtiles-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let tiles = vec![(0u8, 0u32, 0u32, b"zero".to_vec()), (1, 1, 0, b"one-one-zero".to_vec()), (2, 3, 3, b"two".to_vec())];
        for leaf in [false, true] {
            let p = dir.join(format!("t{leaf}.pmtiles"));
            std::fs::write(&p, archive(&tiles, 1, leaf, r#"{"vector_layers":[{"id":"roads"}],"attribution":"Test"}"#)).unwrap();
            let a = p.display().to_string();
            let mut i = info(&a).unwrap();
            for _ in 0..200 {
                if i.is_some() {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
                i = info(&a).unwrap();
            }
            let i = i.unwrap();
            assert_eq!(i.tile_type, TileType::Mvt);
            assert_eq!((i.min_zoom, i.max_zoom), (0, 2));
            assert_eq!(i.vector_layers(), vec!["roads".to_string()]);
            assert_eq!(i.attribution(), "Test");
            for (z, x, y, b) in &tiles {
                assert_eq!(&get(&format!("pmtiles:{z}/{x}/{y}@{a}")).unwrap(), b, "{z}/{x}/{y} leaf={leaf}");
            }
            // Not in the archive, outside its zooms, or not a tile of its zoom: no tile.
            assert!(get(&format!("pmtiles:1/0/0@{a}")).unwrap().is_empty());
            assert!(get(&format!("pmtiles:1/2/0@{a}")).unwrap().is_empty());
            assert!(get(&format!("pmtiles:9/0/0@{a}")).unwrap().is_empty());
        }
        // Not an archive.
        let bad = dir.join("bad.pmtiles");
        std::fs::write(&bad, b"not one").unwrap();
        assert!(get(&format!("pmtiles:0/0/0@{}", bad.display())).is_err());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_raster_archive_resolves_and_decodes_and_truncations_fail_cleanly() {
        let dir = std::env::temp_dir().join(format!("rustgis-pmtiles-raster-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut png = Vec::new();
        image::RgbaImage::from_pixel(256, 256, image::Rgba([200, 30, 30, 255]))
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let bytes = archive(&[(0, 0, 0, png)], 2, false, "{}");
        let p = dir.join("red.pmtiles");
        std::fs::write(&p, &bytes).unwrap();
        let a = p.display().to_string();
        let spec = crate::source::Spec {
            kind: crate::source::Kind::PmTiles,
            url: a.clone(),
            subdomains: Vec::new(),
            min_zoom: 0,
            max_zoom: 19,
            tile_size: 256,
            tms: false,
            attribution: String::new(),
            layers: Vec::new(),
        };
        let mut resolved = crate::source::resolve(&spec);
        for _ in 0..200 {
            if !matches!(resolved, crate::source::Resolve::Pending) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
            resolved = crate::source::resolve(&spec);
        }
        let crate::source::Resolve::Ready(r) = resolved else { panic!("not resolved") };
        assert_eq!(r.style.sources.len(), 1);
        // Through the cache, decoded as a picture.
        let url = format!("pmtiles:0/0/0@{a}");
        let mut state = crate::cache::get(&url, crate::cache::Want::Image);
        for _ in 0..100 {
            if !matches!(state, crate::cache::State::Pending) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
            state = crate::cache::get(&url, crate::cache::Want::Image);
        }
        match state {
            crate::cache::State::Ready(p) => match &*p {
                crate::cache::Payload::Image(pm) => assert_eq!(pm.pixel(10, 10).map(|c| c.red()), Some(200)),
                _ => panic!("not a picture"),
            },
            _ => panic!("tile not read"),
        }
        // Every truncation of an archive is an error, never a panic.
        for n in (0..bytes.len()).step_by(7) {
            let t = dir.join(format!("cut{n}.pmtiles"));
            std::fs::write(&t, &bytes[..n]).unwrap();
            let _ = get(&format!("pmtiles:0/0/0@{}", t.display()));
        }
        let _ = std::fs::remove_dir_all(dir);
    }
}
