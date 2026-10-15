//! GDAL, optionally: formats the built-in readers don't know (File Geodatabase, DWG, MapInfo TAB,
//! OpenStreetMap XML…) read through GDAL's `ogr2ogr` program when it is installed.
//!
//! Each layer is converted once into a FlatGeobuf with a spatial index (in WGS 84, reprojected by
//! GDAL) in the cache folder, then read like any FlatGeobuf, so large files stream a view at a
//! time too. Nothing links to GDAL: without the program, these formats report how to get it.
//! Layers mixing geometry types (KML often does) become one converted layer per type.

use super::external::{self, TempFile, fnv1a, output, stamp, touch};
use crate::{DataError, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

/// File types read through GDAL. A File Geodatabase is a folder ending in `.gdb`.
/// (Not `.vrt`, which can point GDAL at any file on the machine, nor `.sqlite`, more often a plain
/// database than a spatial one.)
// KML, KMZ and GPX have built-in readers (`kml`, `gpx`), in the browser too.
// GML, MapInfo .mif, OSM PBF, TopoJSON and DXF have built-in readers too (`gml`, `mapinfo`, `osm`,
// `topojson`, `dxf`). DWG, AutoCAD's own format, is read by GDAL only (its CAD driver, when built
// in).
pub const EXTENSIONS: &[&str] = &["dwg", "gdb", "tab", "osm", "geojsonl", "geojsons", "gmt"];

/// Most layers converted from one file.
const MAX_LAYERS: usize = 64;

/// Whether a file name is read through GDAL.
pub fn is_gdal_name(name: &str) -> bool {
    let n = name.trim_end_matches(['/', '\\']);
    let ext = Path::new(n).extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    EXTENSIONS.contains(&ext.as_str())
}

pub use super::Family;

/// A layer of a GDAL dataset, converted.
#[derive(Clone, Debug, PartialEq)]
pub struct Converted {
    /// The FlatGeobuf in the cache.
    pub path: PathBuf,
    /// The layer's name (with its geometry kind, when the layer mixed them).
    pub name: String,
    /// The layer in the source.
    pub layer: String,
    pub family: Option<Family>,
    /// GDAL couldn't reproject it (the file doesn't say its coordinate system): its coordinates
    /// are the file's own.
    pub unprojected: bool,
}

/// A GDAL program (`ogr2ogr`, `ogrinfo`): from `RUSTGIS_GDAL_BIN` (a folder), else on the `PATH`.
pub fn program(name: &str) -> Option<PathBuf> {
    external::program("RUSTGIS_GDAL_BIN", name)
}

/// Whether GDAL's programs are installed.
pub fn available() -> bool {
    program("ogr2ogr").is_some() && program("ogrinfo").is_some()
}

fn missing() -> DataError {
    DataError::Unsupported(
        "this format needs GDAL: install it (ogr2ogr and ogrinfo on the PATH, or RUSTGIS_GDAL_BIN naming their folder) and add the file again"
            .into(),
    )
}

/// Where converted layers are kept: the platform's cache folder, or `RUSTGIS_GDAL_CACHE`.
pub fn cache_dir() -> Option<PathBuf> {
    external::cache_dir("RUSTGIS_GDAL_CACHE", "gdal")
}

/// A layer: its name, whether it holds one kind of geometry (`Some`) or mixes them (`None`), and
/// its feature count when GDAL knows it.
#[derive(Clone, Debug, PartialEq)]
struct LayerInfo {
    name: String,
    single: bool,
    count: Option<i64>,
    /// Whether the layer says its coordinate system.
    has_srs: bool,
}

/// The vector layers of a dataset, from `ogrinfo -json`.
fn list(src: &Path) -> Result<Vec<LayerInfo>> {
    let ogrinfo = program("ogrinfo").ok_or_else(missing)?;
    let src = std::path::absolute(src).unwrap_or_else(|_| src.to_path_buf());
    let mut c = Command::new(ogrinfo);
    c.arg("-json").arg("-ro").arg("-so").arg("-nomd").arg(&src);
    let out = output(c, LIST_TIMEOUT, "GDAL")?;
    if !out.status.success() {
        return Err(DataError::Format(format!("GDAL: {}", String::from_utf8_lossy(&out.stderr).trim())));
    }
    let json: serde_json::Value = serde_json::from_slice(&out.stdout)
        .map_err(|e| DataError::Format(format!("GDAL: unreadable ogrinfo output ({e}); GDAL 3.7 or later is needed")))?;
    let layers = json.get("layers").and_then(|l| l.as_array()).cloned().unwrap_or_default();
    Ok(layers
        .iter()
        .filter_map(|l| {
            let name = l.get("name")?.as_str()?.to_string();
            let geom = l.get("geometryFields").and_then(|g| g.as_array()).and_then(|g| g.first())?;
            let ty = geom.get("type").and_then(|t| t.as_str()).unwrap_or("Geometry").to_ascii_lowercase();
            let single = ["point", "linestring", "polygon", "curve", "surface"].iter().any(|k| ty.contains(k));
            let has_srs = geom.get("coordinateSystem").is_some_and(|c| !c.is_null());
            Some(LayerInfo { name, single, count: l.get("featureCount").and_then(|c| c.as_i64()), has_srs })
        })
        .take(MAX_LAYERS)
        .collect())
}

/// The vector layers of a dataset.
pub fn layers(src: &Path) -> Result<Vec<String>> {
    Ok(list(src)?.into_iter().map(|l| l.name).collect())
}

/// Converts the dataset's layers (or just `layer`) to FlatGeobufs, reusing earlier conversions of
/// the same file.
/// `crs` gives the coordinate system of a file that doesn't say (`EPSG:2263`; a DWG often doesn't).
/// Returns the converted layers, and the layers that couldn't be converted (with why), so a
/// partial result is never mistaken for the whole file.
pub fn convert(src: &Path, layer: Option<&str>, crs: Option<&str>) -> Result<(Vec<Converted>, Vec<String>)> {
    let dir = cache_dir().ok_or_else(|| DataError::Format("no cache folder for GDAL conversions".into()))?;
    convert_in(&dir, src, layer, crs)
}

fn convert_in(dir: &Path, src: &Path, layer: Option<&str>, crs: Option<&str>) -> Result<(Vec<Converted>, Vec<String>)> {
    if let Some(c) = crs
        && (c.len() > 64 || c.starts_with('-') || !c.chars().all(|ch| ch.is_ascii_alphanumeric() || ":_.".contains(ch)))
    {
        return Err(DataError::Format(format!("crs '{c}': give a code such as EPSG:2263")));
    }
    if !available() {
        return Err(missing());
    }
    let all = list(src)?;
    let chosen: Vec<LayerInfo> = match layer {
        Some(l) => all.into_iter().filter(|x| x.name.eq_ignore_ascii_case(l)).collect(),
        None => all.into_iter().filter(|x| x.count != Some(0)).collect(),
    };
    if chosen.is_empty() {
        return Err(DataError::Format(match layer {
            Some(l) => format!("no layer '{l}'"),
            None => "no layers with features".into(),
        }));
    }
    let mut out = Vec::new();
    let mut failed = Vec::new();
    let mut last = None;
    for l in chosen {
        let before = out.len();
        let mut errors = Vec::new();
        let families: Vec<Option<Family>> = if l.single { vec![None] } else { Family::ALL.iter().copied().map(Some).collect() };
        for family in families {
            match convert_one_in(dir, src, &l.name, family, crs, l.has_srs) {
                Ok(Some((path, unprojected))) => out.push(Converted {
                    path,
                    unprojected,
                    name: match family {
                        Some(f) => format!("{} ({})", l.name, f.label()),
                        None => l.name.clone(),
                    },
                    layer: l.name.clone(),
                    family,
                }),
                // A family the layer doesn't have.
                Ok(None) => {}
                // A layer (or a family filter the driver can't apply) that fails doesn't stop the
                // others; the file fails only if nothing converts.
                Err(e) => errors.push(e),
            }
        }
        // A layer none of whose parts converted is reported, with the first reason.
        if out.len() == before
            && let Some(e) = errors.into_iter().next()
        {
            failed.push(format!("{}: {e}", l.name));
            last = Some(e);
        }
    }
    if out.is_empty() {
        return Err(last.unwrap_or_else(|| DataError::Format("GDAL found no features to read".into())));
    }
    Ok((out, failed))
}

/// Converts one layer (or one geometry family of it) into the cache; `None` when it has no
/// features. The cache key covers the file's path, size and modification time.
pub fn convert_one(src: &Path, layer: &str, family: Option<Family>, crs: Option<&str>) -> Result<Option<PathBuf>> {
    let dir = cache_dir().ok_or_else(|| DataError::Format("no cache folder for GDAL conversions".into()))?;
    // Whether the layer says its coordinate system, as a fresh listing tells.
    let has_srs = list(src)?.into_iter().find(|l| l.name == layer).is_none_or(|l| l.has_srs);
    Ok(convert_one_in(&dir, src, layer, family, crs, has_srs)?.map(|(p, _)| p))
}

/// The conversion and whether it is unprojected (see [`Converted::unprojected`]).
/// `has_srs` says whether the layer gives its coordinate system: one that doesn't (and has no
/// `crs` given) is taken as it is, unprojected.
fn convert_one_in(
    dir: &Path,
    src: &Path,
    layer: &str,
    family: Option<Family>,
    crs: Option<&str>,
    has_srs: bool,
) -> Result<Option<(PathBuf, bool)>> {
    // A name GDAL would take for an option (a file can name its own layers).
    if layer.starts_with('-') {
        return Err(DataError::Format(format!("layer '{layer}': a name starting with '-' isn't read")));
    }
    let ogr2ogr = program("ogr2ogr").ok_or_else(missing)?;
    std::fs::create_dir_all(dir)?;
    external::prune(dir, ours, KEEP);
    let full = std::path::absolute(src).unwrap_or_else(|_| src.to_path_buf());
    let (len, modified) = stamp(&full)?;
    // The cache key, spelled out: kept beside the conversion and checked, so a hash collision
    // can't hand back another file's layer.
    let key = format!("{}\n{layer}\n{:?}\n{:?}\n{len}\n{modified}", full.display(), family.map(Family::label), crs);
    let tag = format!("{:016x}", fnv1a(key.as_bytes()));
    let out = dir.join(format!("{tag}.fgb"));
    let empty = dir.join(format!("{tag}.empty"));
    let keyfile = dir.join(format!("{tag}.key"));
    // The key file holds the key, then whether the conversion is unprojected: one file, so the two
    // can't disagree.
    const UNPROJECTED: &str = "\nunprojected";
    let stored = std::fs::read_to_string(&keyfile).unwrap_or_default();
    let same = stored == key || stored.strip_suffix(UNPROJECTED) == Some(key.as_str());
    let was_unprojected = stored.ends_with(UNPROJECTED);
    // A hit is touched, so pruning removes what isn't used rather than what is old.
    if same && empty.exists() {
        touch(&empty);
        touch(&keyfile);
        return Ok(None);
    }
    if same && out.metadata().is_ok_and(|m| m.len() > 0) {
        touch(&out);
        touch(&keyfile);
        return Ok(Some((out, was_unprojected)));
    }
    // Unique to this conversion, even with others of the same layer running in this process.
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let tmp = dir.join(format!("{tag}.{}{n:04}.tmp.fgb", std::process::id()));
    // Reprojected to WGS 84 from its own coordinate system, or from the one given; a layer with
    // neither is taken as it is.
    let unprojected = crs.is_none() && !has_srs;
    let mut c = Command::new(&ogr2ogr);
    c.args(["-f", "FlatGeobuf", "-skipfailures", "-nlt", "PROMOTE_TO_MULTI", "-dim", "XY", "-lco", "SPATIAL_INDEX=YES"]);
    if let Some(crs) = crs {
        c.args(["-s_srs", crs]);
    }
    if !unprojected {
        c.args(["-t_srs", "EPSG:4326"]);
    }
    if let Some(f) = family {
        c.args(["-where", f.filter()]);
    }
    c.arg(&tmp).arg(&full).arg(layer);
    // The temp file goes whatever happens, unless it becomes the conversion.
    let mut tmp_guard = TempFile(Some(tmp.clone()));
    let res = output(c, CONVERT_TIMEOUT, "GDAL")?;
    if !res.status.success() {
        let text = String::from_utf8_lossy(&res.stderr);
        // The ERROR lines say what went wrong (the rest is the usage text); else the first line.
        let mut msg: Vec<&str> = text.lines().filter(|l| l.contains("ERROR")).take(3).collect();
        if msg.is_empty() {
            msg = text.lines().filter(|l| !l.trim().is_empty()).take(1).collect();
        }
        let msg = if msg.is_empty() { format!("ogr2ogr failed ({})", res.status) } else { msg.join(" ") };
        return Err(DataError::Format(format!("GDAL: {msg}")));
    }
    // Whether it holds anything: a feature read, not the header's count (which FlatGeobuf may leave
    // out, as 0). A filter matching nothing leaves no file at all.
    let any = tmp.exists()
        && super::open_path(&tmp, None)
            .ok()
            .is_some_and(|s| s.read(&super::Request { extent: None, limit: 1 }).is_ok_and(|b| !b.features.is_empty()));
    // The entry is replaced in an order that never leaves it naming a missing file: the new state
    // first, then the key that vouches for it.
    let _ = std::fs::remove_file(&keyfile);
    if !any {
        // An empty family is remembered, so the file isn't converted again for it.
        std::fs::write(&empty, b"")?;
        let _ = std::fs::remove_file(&out);
        std::fs::write(&keyfile, &key)?;
        return Ok(None);
    }
    // A rename replaces the old conversion in one step (Windows needs it gone first).
    #[cfg(windows)]
    let _ = std::fs::remove_file(&out);
    std::fs::rename(&tmp, &out)?;
    tmp_guard.0 = None;
    let _ = std::fs::remove_file(&empty);
    std::fs::write(&keyfile, if unprojected { format!("{key}{UNPROJECTED}") } else { key.clone() })?;
    Ok(Some((out, unprojected)))
}

/// The longest a conversion may run, and the longest a layer listing may.
const CONVERT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30 * 60);
const LIST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5 * 60);

/// Conversions not used for this long are removed from the cache.
const KEEP: std::time::Duration = std::time::Duration::from_secs(30 * 24 * 3600);

/// Whether a file name is one this module writes: `<16 hex>.fgb|.key|.empty`, or a
/// `<16 hex>.<pid>.tmp.fgb` left by a conversion that didn't finish.
fn ours(name: &str) -> bool {
    let Some((tag, rest)) = name.split_once('.') else { return false };
    let tmp = rest.strip_suffix(".tmp.fgb").is_some_and(|pid| !pid.is_empty() && pid.bytes().all(|b| b.is_ascii_digit()));
    tag.len() == 16 && tag.bytes().all(|b| b.is_ascii_hexdigit()) && (matches!(rest, "fgb" | "key" | "empty") || tmp)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert!(
            is_gdal_name("a.DWG")
                && !is_gdal_name("a.dxf")
                && is_gdal_name("/x/roads.gdb/")
                && !is_gdal_name("t.gpx")
                && !is_gdal_name("a.gml")
        );
        assert!(!is_gdal_name("a.geojson") && !is_gdal_name("a.fgb") && !is_gdal_name("noext"));
        // A layer named like an option never reaches GDAL.
        let e = convert_one_in(&std::env::temp_dir(), Path::new("x.kml"), "-overwrite", None, None, true).unwrap_err();
        assert!(e.to_string().contains("starting with '-'"), "{e}");
        // Pruning touches only the cache's own files.
        assert!(ours("0123456789abcdef.fgb") && ours("0123456789abcdef.key") && ours("0123456789abcdef.empty"));
        assert!(ours("0123456789abcdef.4242.tmp.fgb"));
        assert!(!ours("notes.fgb") && !ours("0123456789abcdef.txt") && !ours("photo.jpg") && !ours("0123456789abcdef.x.tmp.fgb"));
        // The cache key's hash is stable.
        assert_eq!(fnv1a(b"rustgis"), fnv1a(b"rustgis"));
        assert_ne!(fnv1a(b"a"), fnv1a(b"b"));
    }

    /// A KML mixing points and polygons, through GDAL when it is installed (skipped otherwise).
    #[test]
    fn converts_a_mixed_kml_when_gdal_is_installed() {
        if !available() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("rustgis-gdal-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let kml = dir.join("mixed.kml");
        std::fs::write(
            &kml,
            r#"<?xml version="1.0" encoding="UTF-8"?><kml xmlns="http://www.opengis.net/kml/2.2"><Document><name>mixed</name>
<Placemark><name>a</name><Point><coordinates>10,20</coordinates></Point></Placemark>
<Placemark><name>b</name><Point><coordinates>11,21</coordinates></Point></Placemark>
<Placemark><name>c</name><Polygon><outerBoundaryIs><LinearRing><coordinates>0,0 1,0 1,1 0,0</coordinates></LinearRing></outerBoundaryIs></Polygon></Placemark>
</Document></kml>"#,
        )
        .unwrap();
        let cache = dir.join("cache");
        let (out, failed) = convert_in(&cache, &kml, None, None).unwrap();
        assert!(failed.is_empty(), "{failed:?}");
        let kinds: Vec<(String, u64)> = out
            .iter()
            .map(|c| {
                let s = crate::source::open_path(&c.path, None).unwrap();
                (c.name.clone(), s.info().count.unwrap_or(0))
            })
            .collect();
        assert_eq!(kinds, vec![("mixed (points)".to_string(), 2), ("mixed (polygons)".to_string(), 1)], "{kinds:?}");
        // Converted once: the second call reuses the cache.
        let (again, _) = convert_in(&cache, &kml, Some("mixed"), None).unwrap();
        assert_eq!(again.iter().map(|c| &c.path).collect::<Vec<_>>(), out.iter().map(|c| &c.path).collect::<Vec<_>>());
        let _ = std::fs::remove_dir_all(dir);
    }
}
