//! Web tile layers: basemaps and overlays drawn from tile services, under the map's own layers.
//!
//! A map keeps a stack of them ([`crate::Map::tiles`], index 0 on top) over its built-in basemap.
//! Three kinds of service are read: XYZ/TMS raster tiles (PNG, JPEG or WebP, and WMS through a
//! `{bbox-epsg-3857}` template), Mapbox Vector Tiles drawn with a built-in style, and MapLibre
//! style documents (vector and raster sources together). The tiles themselves are fetched and
//! drawn by `rustgis-tiles` and the renderer; this is only what the project saves.

use serde::{Deserialize, Serialize};

/// The most tile layers a map holds.
pub const MAX_TILE_LAYERS: usize = 32;

/// What a tile layer's address points at.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TileKind {
    /// Raster tiles from a URL template (`{z}/{x}/{y}`, `{-y}`, `{s}`, `{q}`, `{bbox-epsg-3857}`).
    #[default]
    Raster,
    /// Mapbox Vector Tiles from a URL template, or a TileJSON document, in a built-in style.
    Vector,
    /// A MapLibre style document (`style.json`).
    Style,
    /// A PMTiles archive of raster or vector tiles: a web address (read by byte ranges) or, on the
    /// desktop, a file.
    #[serde(rename = "pmtiles")]
    PmTiles,
    /// An MBTiles archive of raster or vector tiles: a file on the desktop, or `upload:<name>`
    /// for one added to the web app (kept in memory, not in the project).
    #[serde(rename = "mbtiles")]
    MbTiles,
    /// A cloud-optimised GeoTIFF by its web address: drawn from the overview and tiles the view
    /// needs, read by byte range.
    #[serde(rename = "cog")]
    Cog,
}

impl TileKind {
    pub const ALL: [TileKind; 6] =
        [TileKind::Raster, TileKind::Vector, TileKind::Style, TileKind::PmTiles, TileKind::MbTiles, TileKind::Cog];
    pub fn code(self) -> &'static str {
        match self {
            TileKind::Raster => "raster",
            TileKind::Vector => "vector",
            TileKind::Style => "style",
            TileKind::PmTiles => "pmtiles",
            TileKind::MbTiles => "mbtiles",
            TileKind::Cog => "cog",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            TileKind::Raster => "Raster tiles (XYZ)",
            TileKind::Vector => "Vector tiles (MVT)",
            TileKind::Style => "Map style (JSON)",
            TileKind::PmTiles => "PMTiles archive",
            TileKind::MbTiles => "MBTiles archive",
            TileKind::Cog => "Cloud-optimised GeoTIFF",
        }
    }
    pub fn from_code(s: &str) -> Option<TileKind> {
        let s = s.trim();
        match s.to_ascii_lowercase().as_str() {
            "raster" | "xyz" | "tms" | "wms" => Some(TileKind::Raster),
            "vector" | "mvt" | "pbf" => Some(TileKind::Vector),
            "style" | "maplibre" | "mapbox" => Some(TileKind::Style),
            "pmtiles" => Some(TileKind::PmTiles),
            "mbtiles" => Some(TileKind::MbTiles),
            "cog" | "geotiff" => Some(TileKind::Cog),
            _ => None,
        }
    }
    /// The kind an address most likely is, from how it looks.
    pub fn guess(url: &str) -> TileKind {
        let path = url.split(['?', '#']).next().unwrap_or(url).to_ascii_lowercase();
        if path.ends_with(".pmtiles") {
            TileKind::PmTiles
        } else if path.ends_with(".mbtiles") {
            TileKind::MbTiles
        } else if path.ends_with(".pbf") || path.ends_with(".mvt") || path.contains("/{z}/{x}/{y}.vector") {
            TileKind::Vector
        } else if path.ends_with("style.json") || (path.contains("/styles/") && !path.contains("{z}")) {
            TileKind::Style
        } else {
            TileKind::Raster
        }
    }
}

/// A tile layer.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TileLayer {
    pub name: String,
    pub kind: TileKind,
    /// The URL template, TileJSON or style address.
    pub url: String,
    /// Subdomains that `{s}` takes in turn (`a`, `b`, `c` when empty and the URL has `{s}`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub subdomains: Vec<String>,
    #[serde(default)]
    pub min_zoom: u8,
    #[serde(default = "max_zoom")]
    pub max_zoom: u8,
    /// Pixels per tile side (256, or 512 for most vector and high-density raster tiles).
    #[serde(default = "tile_size")]
    pub tile_size: u32,
    /// `y` counts from the south (TMS).
    #[serde(default, skip_serializing_if = "is_false")]
    pub tms: bool,
    /// The credit the provider asks for, shown in the corner of the map.
    #[serde(default)]
    pub attribution: String,
    #[serde(default = "yes")]
    pub visible: bool,
    /// 0 = invisible, 1 = opaque.
    #[serde(default = "one")]
    pub opacity: f32,
    /// A transparent layer meant to sit over a basemap (labels, roads, traffic).
    #[serde(default, skip_serializing_if = "is_false")]
    pub overlay: bool,
    /// The catalog entry it came from (`osm`, `esri-imagery`), when it came from one.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub id: String,
    /// Vector tiles' layer names, when the service gave them (each drawn in a colour of its own).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub layers: Vec<String>,
}

fn max_zoom() -> u8 {
    19
}
fn tile_size() -> u32 {
    256
}
fn yes() -> bool {
    true
}
fn one() -> f32 {
    1.0
}
fn is_false(b: &bool) -> bool {
    !*b
}

/// The deepest zoom level tiles are requested at.
pub const MAX_ZOOM: u8 = 24;

impl TileLayer {
    pub fn new(name: impl Into<String>, kind: TileKind, url: impl Into<String>) -> TileLayer {
        TileLayer {
            name: name.into(),
            kind,
            url: url.into(),
            subdomains: Vec::new(),
            min_zoom: 0,
            max_zoom: max_zoom(),
            tile_size: tile_size(),
            tms: false,
            attribution: String::new(),
            visible: true,
            opacity: 1.0,
            overlay: false,
            id: String::new(),
            layers: Vec::new(),
        }
    }

    /// Checks the address and brings the numbers into range, after reading one from a project
    /// file or a command.
    pub fn repair(&mut self) -> Result<(), String> {
        self.url = self.url.trim().to_string();
        let local = matches!(self.kind, TileKind::PmTiles | TileKind::MbTiles) && is_local_archive(&self.url);
        if let Some(name) = self.url.strip_prefix(UPLOAD).filter(|_| self.kind == TileKind::MbTiles) {
            if name.is_empty() || name.len() > 512 || name.chars().any(char::is_control) {
                return Err("an uploaded MBTiles file needs a name".into());
            }
        } else if local {
            check_path(&self.url, if self.kind == TileKind::MbTiles { "mbtiles" } else { "pmtiles" })?;
        } else {
            check_url(&self.url)?;
        }
        self.name = self.name.trim().chars().take(120).collect();
        if self.name.is_empty() {
            let file = || {
                std::path::Path::new(self.url.trim_start_matches(UPLOAD).trim_start_matches("file://"))
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .map(str::to_string)
            };
            self.name = if local { file() } else { host_of(&self.url).map(str::to_string) }.unwrap_or_else(|| "Tiles".into());
        }
        // Layer names (a project file is hostile input): at most 64, each short and printable.
        self.layers.retain(|l| !l.trim().is_empty() && l.len() <= 200 && !l.chars().any(char::is_control));
        self.layers.truncate(64);
        self.max_zoom = self.max_zoom.min(MAX_ZOOM);
        self.min_zoom = self.min_zoom.min(self.max_zoom);
        self.tile_size = if self.tile_size >= 384 { 512 } else { 256 };
        if !self.opacity.is_finite() {
            self.opacity = 1.0;
        }
        self.opacity = self.opacity.clamp(0.0, 1.0);
        self.subdomains.retain(|s| !s.is_empty() && s.len() <= 32 && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'));
        self.subdomains.truncate(16);
        self.attribution = self.attribution.chars().take(500).collect();
        self.id = self.id.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')).take(80).collect();
        Ok(())
    }
}

/// The prefix of an MBTiles archive added to the web app, held in memory.
pub const UPLOAD: &str = "upload:";

/// Whether a PMTiles or MBTiles archive address is a file rather than a web address.
pub fn is_local_archive(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    !(lower.starts_with("https://") || lower.starts_with("http://") || lower.starts_with("s3://"))
}

/// Whether an archive's file path may be read: absolute, a `.<ext>` file (`pmtiles`,
/// `mbtiles`), no longer than 4096 bytes.
pub fn check_path(path: &str, ext: &str) -> Result<(), String> {
    let p = path.strip_prefix("file://").unwrap_or(path);
    if p.len() > 4096 || p.chars().any(char::is_control) {
        return Err("the path is too long or has control characters".into());
    }
    let windows_drive = p.as_bytes().get(1) == Some(&b':') && p.as_bytes().first().is_some_and(u8::is_ascii_alphabetic);
    // Not a network share (\\server\share, or file://server/…), which a project could use to
    // make Windows connect to another machine.
    let share = p.starts_with("\\\\") || p.starts_with("//") || (path.starts_with("file://") && !p.starts_with('/'));
    if share || !(p.starts_with('/') || windows_drive) {
        return Err(format!("'{p}': give the full path of the archive on this computer"));
    }
    if !p.to_ascii_lowercase().ends_with(&format!(".{ext}")) {
        return Err(format!("'{p}' isn't a .{ext} file"));
    }
    Ok(())
}

/// Whether `url` may be fetched: HTTP or HTTPS, with a host, and no longer than 4096 bytes.
pub fn check_url(url: &str) -> Result<(), String> {
    if url.len() > 4096 {
        return Err("the address is too long".into());
    }
    let lower = url.to_ascii_lowercase();
    if !(lower.starts_with("https://") || lower.starts_with("http://") || lower.starts_with("s3://")) {
        return Err(format!("'{url}': tile addresses start with https://, http:// or s3://"));
    }
    if host_of(url).is_none_or(str::is_empty) {
        return Err(format!("'{url}' has no host"));
    }
    if url.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(format!("'{url}' has spaces in it"));
    }
    Ok(())
}

/// The host part of an HTTP(S) address.
pub fn host_of(url: &str) -> Option<&str> {
    let rest = url.split_once("://")?.1;
    let host = rest.split(['/', '?', '#']).next()?;
    let host = host.rsplit_once('@').map_or(host, |(_, h)| h);
    Some(host.split(':').next().unwrap_or(host))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guesses_kinds() {
        assert_eq!(TileKind::guess("https://tile.openstreetmap.org/{z}/{x}/{y}.png"), TileKind::Raster);
        assert_eq!(TileKind::guess("https://example.com/tiles/{z}/{x}/{y}.pbf?key=1"), TileKind::Vector);
        assert_eq!(TileKind::guess("https://tiles.openfreemap.org/styles/liberty"), TileKind::Style);
        assert_eq!(TileKind::guess("https://api.maptiler.com/maps/streets/style.json?key=k"), TileKind::Style);
    }

    #[test]
    fn repairs() {
        let mut t = TileLayer::new("  ", TileKind::Raster, " https://a.tile.example.com/{z}/{x}/{y}.png ");
        t.max_zoom = 200;
        t.min_zoom = 250;
        t.opacity = f32::NAN;
        t.tile_size = 7;
        assert!(t.repair().is_ok());
        assert_eq!(t.name, "a.tile.example.com");
        assert_eq!((t.min_zoom, t.max_zoom, t.tile_size, t.opacity), (MAX_ZOOM, MAX_ZOOM, 256, 1.0));
        for bad in ["file:///etc/passwd", "ftp://x/{z}", "https://", "https:// x.com/{z}", "javascript:alert(1)"] {
            assert!(TileLayer::new("x", TileKind::Raster, bad).repair().is_err(), "{bad}");
        }
    }

    #[test]
    fn reads_old_and_short_json() {
        let t: TileLayer =
            serde_json::from_str(r#"{"name": "OSM", "kind": "raster", "url": "https://tile.openstreetmap.org/{z}/{x}/{y}.png"}"#)
                .unwrap_or_else(|e| panic!("{e}"));
        assert!(t.visible && t.opacity == 1.0 && t.max_zoom == 19 && t.tile_size == 256);
    }
}
