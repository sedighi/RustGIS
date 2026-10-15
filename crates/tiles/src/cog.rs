//! Cloud-optimised GeoTIFFs by their web address, read in place: the file's header (its IFDs)
//! once, then only the tiles of the overview a map tile needs, by HTTP range requests. Each map
//! tile (`cog:{z}/{x}/{y}@<address>`) is drawn from them in Web Mercator, as a picture, so a COG is
//! a tile layer like a PMTiles archive: drawn as its tiles land, on the desktop and in the browser.
//!
//! Grey images are stretched between their 2nd and 98th percentiles (from the coarsest
//! overview); 8-bit colour images are shown as they are. Files are hostile input: header reads, tile
//! counts and sizes are bounded (see [`rustgis_data::raster::cog`]).

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use rustgis_data::raster::ColorHint;
use rustgis_data::raster::cog::{self, Layout, Parse, Sparse};
use rustgis_data::raster::crs::Transformer;
use rustgis_geom::Coord;
use web_time::{Duration, Instant};

use crate::cache::{Done, MAX_BYTES, RANGE};

/// The scheme of a map tile drawn from a COG.
pub const SCHEME: &str = "cog:";

/// The first read of a file: enough for a COG's IFDs, as GDAL writes them, in one request.
const HEAD: u64 = 64 << 10;
/// More of the header, when it's longer, is read in steps at least this large.
const STEP: u64 = 256 << 10;
/// The most header reads before giving up.
const MAX_ROUNDS: usize = 12;
/// The most COG tiles one map tile may need (past that, a coarser overview is read).
const MAX_PER_TILE: usize = 16;
/// The most tiles of the coarsest overview read for the stretch.
const MAX_STATS_TILES: usize = 16;
/// Decoded tiles kept, in samples.
const BUDGET: usize = if cfg!(target_arch = "wasm32") { 24 << 20 } else { 64 << 20 };
/// A failed file is tried again after this long.
const RETRY: Duration = Duration::from_secs(60);
/// A map tile's side, in pixels.
const SIZE: u32 = 256;
/// Web Mercator's half extent.
const HALF: f64 = 20_037_508.342_789_244;

/// The template of a COG's map tiles.
pub fn template(url: &str) -> String {
    format!("{SCHEME}{{z}}/{{x}}/{{y}}@{url}")
}

/// Whether a tile address is a COG's.
pub fn is_tile(url: &str) -> bool {
    url.starts_with(SCHEME)
}

/// What a COG holds, ready to draw from.
pub struct Info {
    pub layout: Layout,
    header: Sparse,
    transformer: Transformer,
    /// Longitude/latitude bounds: west, south, east, north.
    pub bounds: [f64; 4],
    /// Its Web Mercator bounds: min x, min y, max x, max y.
    merc: [f64; 4],
    pub min_zoom: u8,
    /// The zoom whose pixels are about the size of the full image's.
    pub max_zoom: u8,
    /// Each shown band's (low, high) values, mapped to 0 and 255 (set once the coarsest overview
    /// is read, before the file is ready).
    stretch: Mutex<Vec<(f32, f32)>>,
    /// Bands shown as red, green, blue (or one, grey), and alpha.
    rgb: bool,
    alpha: bool,
}

enum State {
    Ready(Arc<Info>),
    Loading(Vec<Box<dyn FnOnce(Result<Arc<Info>, String>) + Send>>),
    Failed(String, Instant),
}

fn infos() -> &'static Mutex<HashMap<String, State>> {
    static I: OnceLock<Mutex<HashMap<String, State>>> = OnceLock::new();
    I.get_or_init(Mutex::default)
}

type Bands = Arc<Vec<Vec<f32>>>;

enum TileState {
    Ready(Bands, usize),
    Loading(Vec<Box<dyn FnOnce(Result<Bands, String>) + Send>>),
}

#[derive(Default)]
struct Tiles {
    map: HashMap<(String, usize, u32), TileState>,
    samples: usize,
}

fn tiles() -> &'static Mutex<Tiles> {
    static T: OnceLock<Mutex<Tiles>> = OnceLock::new();
    T.get_or_init(Mutex::default)
}

/// Forgets every file and tile, so they are read again (with [`crate::cache::clear`]).
pub(crate) fn forget() {
    infos().lock().unwrap_or_else(|e| e.into_inner()).retain(|_, s| matches!(s, State::Loading(_)));
    let mut t = tiles().lock().unwrap_or_else(|e| e.into_inner());
    t.map.retain(|_, s| matches!(s, TileState::Loading(_)));
    t.samples = 0;
}

/// A file's info: `Ok(None)` while it is being read (asking for it starts the read).
pub fn info(url: &str) -> Result<Option<Arc<Info>>, String> {
    {
        let i = infos().lock().unwrap_or_else(|e| e.into_inner());
        match i.get(url) {
            Some(State::Ready(info)) => return Ok(Some(info.clone())),
            Some(State::Loading(_)) => return Ok(None),
            Some(State::Failed(e, at)) if at.elapsed() < RETRY => return Err(e.clone()),
            _ => {}
        }
    }
    with_info(url, Box::new(|_| crate::cache::notify()));
    Ok(None)
}

fn with_info(url: &str, then: Box<dyn FnOnce(Result<Arc<Info>, String>) + Send>) {
    {
        let mut i = infos().lock().unwrap_or_else(|e| e.into_inner());
        match i.get_mut(url) {
            Some(State::Ready(info)) => {
                let info = info.clone();
                drop(i);
                then(Ok(info));
                return;
            }
            Some(State::Loading(waiting)) => {
                waiting.push(then);
                return;
            }
            Some(State::Failed(e, at)) if at.elapsed() < RETRY => {
                let e = e.clone();
                drop(i);
                then(Err(e));
                return;
            }
            _ => {
                i.insert(url.to_string(), State::Loading(vec![then]));
            }
        }
    }
    let key = url.to_string();
    load(
        url.to_string(),
        Box::new(move |r| {
            let waiting = {
                let mut i = infos().lock().unwrap_or_else(|e| e.into_inner());
                let next = match &r {
                    Ok(info) => State::Ready(info.clone()),
                    Err(e) => State::Failed(e.clone(), Instant::now()),
                };
                match i.insert(key, next) {
                    Some(State::Loading(w)) => w,
                    _ => Vec::new(),
                }
            };
            for w in waiting {
                w(r.clone());
            }
            crate::cache::notify();
        }),
    );
}

/// Reads `len` bytes at `at` of the file at `url`.
fn read(url: &str, at: u64, len: u64, done: Done) {
    if len == 0 {
        done(Ok(Vec::new()));
        return;
    }
    if len > MAX_BYTES as u64 {
        done(Err(format!("{len} bytes is too large to read from a GeoTIFF")));
        return;
    }
    let end = at.saturating_add(len) - 1;
    crate::cache::fetch_counted(format!("{url}{RANGE}{at}-{end}"), done);
}

/// Reads a file's header (then its stretch): the start, then whatever more the IFDs need.
fn load(url: String, done: Box<dyn FnOnce(Result<Arc<Info>, String>) + Send>) {
    if let Err(e) = crate::address::check(&url, None) {
        done(Err(e));
        return;
    }
    let u = url.clone();
    read(
        &url,
        0,
        HEAD,
        Box::new(move |r| match r {
            Ok(b) if cog::is_tiff(&b) => step(u, Sparse::default().with(0, b), 0, done),
            Ok(_) => done(Err(format!("{u}: not a GeoTIFF"))),
            Err(e) => done(Err(format!("{u}: {e}"))),
        }),
    );
}

fn step(url: String, have: Sparse, rounds: usize, done: Box<dyn FnOnce(Result<Arc<Info>, String>) + Send>) {
    match cog::layout(&have) {
        Ok(Parse::Done(layout)) => stats(url, have, *layout, done),
        Ok(Parse::NotTiled(why)) => done(Err(format!("{NOT_COG}: {why}"))),
        Ok(Parse::Need(a, b)) => {
            if rounds >= MAX_ROUNDS || have.held() >= cog::MAX_HEADER || a >= cog::MAX_HEADER.saturating_mul(64) {
                done(Err(format!("{url}: the GeoTIFF's header is too large or too scattered to read in place")));
                return;
            }
            let len = b.saturating_sub(a).max(STEP).min(MAX_BYTES as u64);
            let u = url.clone();
            read(
                &url,
                a,
                len,
                Box::new(move |r| match r {
                    // Nothing more to read: the file ends before what its header points at.
                    Ok(more) if more.is_empty() => done(Err(format!("{u}: the GeoTIFF is cut short"))),
                    Ok(more) => step(u, have.with(a, more), rounds + 1, done),
                    Err(e) => done(Err(format!("{u}: {e}"))),
                }),
            );
        }
        Err(e) => done(Err(format!("{url}: {e}"))),
    }
}

/// What a file that isn't a cloud-optimised GeoTIFF says (it is read whole instead).
pub const NOT_COG: &str = "not a cloud-optimised GeoTIFF";

/// Reads the coarsest overview (a few tiles) for the stretch, then the file is ready.
fn stats(url: String, header: Sparse, layout: Layout, done: Box<dyn FnOnce(Result<Arc<Info>, String>) + Send>) {
    let transformer = match layout.crs.transformer() {
        Ok(t) => t,
        Err(e) => {
            done(Err(format!("{url}: {e}")));
            return;
        }
    };
    let Some(full) = layout.levels.first().cloned() else {
        done(Err(format!("{url}: no image")));
        return;
    };
    // Bounds: the full image's edges, sampled (a projected square isn't square in degrees).
    let t = layout.transform;
    let (w, h) = (f64::from(full.width), f64::from(full.height));
    let mut lon = (f64::MAX, f64::MIN);
    let mut lat = (f64::MAX, f64::MIN);
    for i in 0..=8 {
        let f = f64::from(i) / 8.0;
        for (px, py) in [(f * w, 0.0), (f * w, h), (0.0, f * h), (w, f * h)] {
            let c = Coord::new(t.origin_x + px * t.pixel_width, t.origin_y + py * t.pixel_height);
            if let Some(ll) = transformer.to_lonlat(c) {
                lon = (lon.0.min(ll.x), lon.1.max(ll.x));
                lat = (lat.0.min(ll.y), lat.1.max(ll.y));
            }
        }
    }
    if lon.0 > lon.1 || lat.0 > lat.1 {
        done(Err(format!("{url}: the GeoTIFF's corners aren't on the globe")));
        return;
    }
    let bounds = [lon.0.max(-180.0), lat.0.max(-85.05), lon.1.min(180.0), lat.1.min(85.05)];
    let merc = {
        let a = rustgis_geom::lonlat_to_merc(Coord::new(bounds[0], bounds[1]));
        let b = rustgis_geom::lonlat_to_merc(Coord::new(bounds[2], bounds[3]));
        [a.x, a.y, b.x, b.y]
    };
    // The zoom whose pixels match the full image's.
    let res = (merc[2] - merc[0]).abs() / w.max(1.0);
    let max_zoom = if res > 0.0 { (2.0 * HALF / f64::from(SIZE) / res).log2().ceil().clamp(0.0, 24.0) as u8 } else { 18 };
    let rgb = layout.bands >= 3;
    let alpha = matches!(layout.color, ColorHint::Rgb { alpha: true });
    let bytes = layout.data_type == "UInt8";
    let shown = if rgb { 3 } else { 1 };
    let coarsest = layout.levels.len().saturating_sub(1);
    let n = layout.levels.get(coarsest).map(|l| (l.across() * l.down()) as usize).unwrap_or(0);
    let info = Arc::new(Info {
        layout,
        header,
        transformer,
        bounds,
        merc,
        min_zoom: 0,
        max_zoom,
        stretch: Mutex::new(vec![(0.0, 255.0); shown]),
        rgb,
        alpha,
    });
    // 8-bit colour images are shown as they are; others between the coarsest overview's
    // percentiles.
    if (bytes && rgb) || n == 0 || n > MAX_STATS_TILES {
        done(Ok(info));
        return;
    }
    let tiles: Vec<u32> = (0..n as u32).collect();
    let i = info.clone();
    fetch_all(
        info,
        url.clone(),
        coarsest,
        tiles,
        Box::new(move |r| match r {
            Ok(bands) => {
                let mut s = Vec::new();
                for b in 0..shown {
                    let mut v: Vec<f32> =
                        bands.iter().filter_map(|t| t.get(b)).flat_map(|v| v.iter().copied()).filter(|x| x.is_finite()).collect();
                    if v.is_empty() {
                        s.push((0.0, 1.0));
                        continue;
                    }
                    v.sort_by(f32::total_cmp);
                    let at = |q: f64| v.get(((v.len() - 1) as f64 * q) as usize).copied().unwrap_or(0.0);
                    let (lo, hi) = (at(0.02), at(0.98));
                    s.push(if hi > lo { (lo, hi) } else { (lo, lo + 1.0) });
                }
                *i.stretch.lock().unwrap_or_else(|e| e.into_inner()) = s;
                done(Ok(i));
            }
            Err(e) => done(Err(format!("{url}: {e}"))),
        }),
    );
}

type Gathered = Box<dyn FnOnce(Result<Vec<Bands>, String>) + Send>;

/// Reads tiles of a level (each once, shared by the map tiles that need it), then calls `then`
/// with their bands in the order asked.
fn fetch_all(info: Arc<Info>, url: String, level: usize, wanted: Vec<u32>, then: Gathered) {
    if wanted.is_empty() {
        then(Ok(Vec::new()));
        return;
    }
    struct Gather {
        got: Vec<Option<Bands>>,
        left: usize,
        failed: Option<String>,
        then: Option<Gathered>,
    }
    let g = Arc::new(Mutex::new(Gather { got: vec![None; wanted.len()], left: wanted.len(), failed: None, then: Some(then) }));
    for (k, idx) in wanted.into_iter().enumerate() {
        let g = g.clone();
        get_tile(
            info.clone(),
            url.clone(),
            level,
            idx,
            Box::new(move |r| {
                let mut s = g.lock().unwrap_or_else(|e| e.into_inner());
                match r {
                    Ok(b) => {
                        if let Some(slot) = s.got.get_mut(k) {
                            *slot = Some(b);
                        }
                    }
                    Err(e) => s.failed = Some(e),
                }
                s.left = s.left.saturating_sub(1);
                if s.left == 0
                    && let Some(then) = s.then.take()
                {
                    let r = match s.failed.take() {
                        Some(e) => Err(e),
                        None => Ok(s.got.iter_mut().filter_map(Option::take).collect()),
                    };
                    drop(s);
                    then(r);
                }
            }),
        );
    }
}

/// One tile of a level, decoded: from the tiles kept, or read.
fn get_tile(info: Arc<Info>, url: String, level: usize, idx: u32, then: Box<dyn FnOnce(Result<Bands, String>) + Send>) {
    let Some(l) = info.layout.levels.get(level) else {
        then(Err("no such overview".into()));
        return;
    };
    let (Some(&at), Some(&len)) = (l.offsets.get(idx as usize), l.counts.get(idx as usize)) else {
        then(Err("no such tile".into()));
        return;
    };
    let nb = info.layout.bands.max(1);
    let per_band = (l.tile_width as usize) * (l.tile_height as usize);
    let samples = per_band * nb;
    // A tile the file leaves out (GDAL's sparse files: no bytes) is empty.
    if len == 0 {
        then(Ok(Arc::new(vec![vec![f32::NAN; per_band]; nb])));
        return;
    }
    // Bytes at offset 0 would be the file's header: a broken index, said rather than drawn blank.
    if at == 0 {
        then(Err(format!("tile {idx} of level {level} has bytes but no offset")));
        return;
    }
    let key = (url.clone(), level, idx);
    {
        let mut t = tiles().lock().unwrap_or_else(|e| e.into_inner());
        match t.map.get_mut(&key) {
            Some(TileState::Ready(b, _)) => {
                let b = b.clone();
                drop(t);
                then(Ok(b));
                return;
            }
            Some(TileState::Loading(w)) => {
                w.push(then);
                return;
            }
            None => {
                t.map.insert(key.clone(), TileState::Loading(vec![then]));
            }
        }
    }
    read(
        &url,
        at,
        len,
        Box::new(move |r| {
            let r = r.and_then(|b| cog::decode_tile(&info.header, &info.layout, level, idx, b).map_err(|e| e.to_string())).map(Arc::new);
            let waiting = {
                let mut t = tiles().lock().unwrap_or_else(|e| e.into_inner());
                let old = match &r {
                    Ok(b) => {
                        // Over the budget: tiles ready make room (the cache is a working set).
                        while t.samples + samples > BUDGET {
                            let Some(k) = t.map.iter().find(|(_, s)| matches!(s, TileState::Ready(..))).map(|(k, _)| k.clone()) else {
                                break;
                            };
                            if let Some(TileState::Ready(_, n)) = t.map.remove(&k) {
                                t.samples = t.samples.saturating_sub(n);
                            }
                        }
                        t.samples += samples;
                        t.map.insert(key, TileState::Ready(b.clone(), samples))
                    }
                    // A failed tile is read again by the next map tile that needs it.
                    Err(_) => t.map.remove(&key),
                };
                match old {
                    Some(TileState::Loading(w)) => w,
                    _ => Vec::new(),
                }
            };
            for w in waiting {
                w(r.clone());
            }
        }),
    );
}

/// `cog:z/x/y@address` → its parts.
fn parse_url(url: &str) -> Option<(u8, u32, u32, String)> {
    let rest = url.strip_prefix(SCHEME)?;
    let (zxy, archive) = rest.split_once('@')?;
    let mut p = zxy.split('/');
    let z: u8 = p.next()?.parse().ok()?;
    let x: u32 = p.next()?.parse().ok()?;
    let y: u32 = p.next()?.parse().ok()?;
    (z <= 30 && !archive.is_empty()).then(|| (z, x, y, archive.to_string()))
}

/// Draws a map tile (`cog:z/x/y@address`): empty where the file has nothing.
pub(crate) fn fetch(url: &str, done: Done) {
    let Some((z, x, y, archive)) = parse_url(url) else {
        done(Err(format!("{url}: not a COG tile address")));
        return;
    };
    let a = archive.clone();
    with_info(
        &archive,
        Box::new(move |info| match info {
            Ok(info) => draw(info, a, z, x, y, done),
            Err(e) => done(Err(e)),
        }),
    );
}

/// Where each pixel of a map tile falls in the file, at a level: (tile, sample) or none.
fn locate(info: &Info, level: usize, native: &[Option<Coord>]) -> Vec<Option<(u32, usize)>> {
    let Some(l) = info.layout.levels.get(level) else { return vec![None; native.len()] };
    let t = info.layout.transform_of(l);
    let (tw, th) = (l.tile_width.max(1), l.tile_height.max(1));
    native
        .iter()
        .map(|c| {
            let c = (*c)?;
            let px = (c.x - t.origin_x) / t.pixel_width;
            let py = (c.y - t.origin_y) / t.pixel_height;
            if !(px >= 0.0 && py >= 0.0 && px < f64::from(l.width) && py < f64::from(l.height)) {
                return None;
            }
            let (px, py) = (px as u32, py as u32);
            let idx = (py / th) * l.across() + px / tw;
            Some((idx, ((py % th) * tw + px % tw) as usize))
        })
        .collect()
}

fn draw(info: Arc<Info>, url: String, z: u8, x: u32, y: u32, done: Done) {
    let n = (1u64 << z) as f64;
    let size = 2.0 * HALF / n;
    let (x0, y1) = (-HALF + f64::from(x) * size, HALF - f64::from(y) * size);
    let [mx0, my0, mx1, my1] = info.merc;
    if x0 > mx1 || x0 + size < mx0 || y1 < my0 || y1 - size > my1 {
        done(Ok(Vec::new()));
        return;
    }
    // Native coordinates of every pixel's centre: exact on a grid every 16 pixels, between
    // them interpolated (projections are smooth at a tile's scale).
    const STEP_PX: usize = 16;
    let px = size / f64::from(SIZE);
    let s = SIZE as usize;
    let g = s / STEP_PX + 1;
    let grid: Vec<Option<Coord>> = (0..g * g)
        .map(|k| {
            let (i, j) = ((k % g) * STEP_PX, (k / g) * STEP_PX);
            info.transformer.from_merc(Coord::new(x0 + i as f64 * px, y1 - j as f64 * px))
        })
        .collect();
    let native: Vec<Option<Coord>> = (0..s * s)
        .map(|k| {
            let (i, j) = (k % s, k / s);
            let (fx, fy) = ((i as f64 + 0.5) / STEP_PX as f64, (j as f64 + 0.5) / STEP_PX as f64);
            let (gi, gj) = ((fx as usize).min(g - 2), (fy as usize).min(g - 2));
            let (tx, ty) = (fx - gi as f64, fy - gj as f64);
            let at = |a: usize, b: usize| grid.get(b * g + a).copied().flatten();
            match (at(gi, gj), at(gi + 1, gj), at(gi, gj + 1), at(gi + 1, gj + 1)) {
                (Some(a), Some(b), Some(c), Some(d)) => {
                    let top = (a.x + (b.x - a.x) * tx, a.y + (b.y - a.y) * tx);
                    let bottom = (c.x + (d.x - c.x) * tx, c.y + (d.y - c.y) * tx);
                    Some(Coord::new(top.0 + (bottom.0 - top.0) * ty, top.1 + (bottom.1 - top.1) * ty))
                }
                // Near a projection's edge: exactly.
                _ => info.transformer.from_merc(Coord::new(x0 + (i as f64 + 0.5) * px, y1 - (j as f64 + 0.5) * px)),
            }
        })
        .collect();
    // The output pixel's size in the file's units, from the grid across the tile.
    let target = match (grid.first().copied().flatten(), grid.get(g - 1).copied().flatten()) {
        (Some(a), Some(b)) => ((b.x - a.x).powi(2) + (b.y - a.y).powi(2)).sqrt() / s as f64,
        _ => f64::MAX,
    };
    // The coarsest overview fine enough for the tile; a coarser one when too many tiles of it
    // are needed.
    let levels = &info.layout.levels;
    let mut level = levels
        .iter()
        .enumerate()
        .rev()
        .find(|(_, l)| info.layout.transform_of(l).pixel_width.abs() <= target * 1.25)
        .map(|(i, _)| i)
        .unwrap_or(0);
    let (spots, wanted) = loop {
        let spots = locate(&info, level, &native);
        let mut wanted: Vec<u32> = spots.iter().flatten().map(|(t, _)| *t).collect();
        wanted.sort_unstable();
        wanted.dedup();
        if wanted.len() <= MAX_PER_TILE || level + 1 >= levels.len() {
            break (spots, wanted);
        }
        level += 1;
    };
    if wanted.is_empty() {
        done(Ok(Vec::new()));
        return;
    }
    if wanted.len() > MAX_PER_TILE * 4 {
        done(Err("too many GeoTIFF tiles for one map tile".into()));
        return;
    }
    let i = info.clone();
    let w2 = wanted.clone();
    fetch_all(
        info,
        url,
        level,
        wanted,
        Box::new(move |r| match r {
            Ok(bands) => done(paint(&i, &spots, &w2, &bands)),
            Err(e) => done(Err(e)),
        }),
    );
}

/// The map tile's pixels, from the tiles read, as a raw picture (none when nothing is there).
fn paint(info: &Info, spots: &[Option<(u32, usize)>], wanted: &[u32], tiles: &[Bands]) -> Result<Vec<u8>, String> {
    let stretch = info.stretch.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let scale = |b: usize, v: f32| -> u8 {
        let (lo, hi) = stretch.get(b).copied().unwrap_or((0.0, 255.0));
        (((v - lo) / (hi - lo)).clamp(0.0, 1.0) * 255.0).round() as u8
    };
    let mut rgba = vec![0u8; spots.len() * 4];
    let mut any = false;
    for (k, spot) in spots.iter().enumerate() {
        let Some((t, at)) = spot else { continue };
        let Some(bands) = wanted.binary_search(t).ok().and_then(|i| tiles.get(i)) else { continue };
        let v = |b: usize| bands.get(b).and_then(|band| band.get(*at)).copied().unwrap_or(f32::NAN);
        let px = if info.rgb {
            let (r, g, b) = (v(0), v(1), v(2));
            if !(r.is_finite() && g.is_finite() && b.is_finite()) {
                continue;
            }
            let a = if info.alpha { v(3) } else { 255.0 };
            if info.alpha && (a.is_nan() || a <= 0.0) {
                continue;
            }
            [scale(0, r), scale(1, g), scale(2, b), a.clamp(0.0, 255.0) as u8]
        } else {
            let g = v(0);
            if !g.is_finite() {
                continue;
            }
            let g = scale(0, g);
            [g, g, g, 255]
        };
        if let Some(out) = rgba.get_mut(k * 4..k * 4 + 4) {
            out.copy_from_slice(&px);
            any = true;
        }
    }
    if !any {
        return Ok(Vec::new());
    }
    let mut out = Vec::with_capacity(crate::cache::RAW.len() + 8 + rgba.len());
    out.extend_from_slice(crate::cache::RAW);
    out.extend_from_slice(&SIZE.to_le_bytes());
    out.extend_from_slice(&SIZE.to_le_bytes());
    out.extend_from_slice(&rgba);
    Ok(out)
}

/// Whether a failure says the file is a GeoTIFF to read whole rather than in place.
fn read_whole(e: &str) -> bool {
    e.contains(NOT_COG) || e.contains("not a GeoTIFF")
}

/// Whether the file at `url` is a cloud-optimised GeoTIFF, waiting for the answer (the command
/// line): `Ok(false)` for any other file, read whole instead.
#[cfg(not(target_arch = "wasm32"))]
pub fn probe_blocking(url: &str) -> Result<bool, String> {
    let (tx, rx) = std::sync::mpsc::channel();
    with_info(url, Box::new(move |r| tx.send(r.map(|_| ())).unwrap_or_default()));
    match rx.recv_timeout(std::time::Duration::from_secs(60)) {
        Ok(Ok(())) => Ok(true),
        Ok(Err(e)) if read_whole(&e) => Ok(false),
        Ok(Err(e)) => Err(e),
        Err(_) => Err(format!("{url}: no answer in a minute")),
    }
}

/// Whether the file at `url` is a cloud-optimised GeoTIFF, off the frame (the app): `then` gets
/// `Ok(false)` for any other file, read whole instead.
pub fn probe(url: &str, then: Box<dyn FnOnce(Result<bool, String>) + Send>) {
    with_info(
        url,
        Box::new(move |r| {
            then(match r {
                Ok(_) => Ok(true),
                Err(e) if read_whole(&e) => Ok(false),
                Err(e) => Err(e),
            })
        }),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tile_addresses() {
        assert_eq!(template("https://x.org/a.tif"), "cog:{z}/{x}/{y}@https://x.org/a.tif");
        assert_eq!(parse_url("cog:3/2/1@https://x.org/a.tif"), Some((3, 2, 1, "https://x.org/a.tif".into())));
        assert!(parse_url("cog:99/0/0@x").is_none() && parse_url("cog:1/0/0@").is_none() && !is_tile("pmtiles:1/0/0@x"));
    }
}
