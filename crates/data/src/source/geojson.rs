//! Large GeoJSON FeatureCollections read in place, a view at a time.
//!
//! Opening reads the file once, a feature at a time, keeping each feature's place in the file and
//! bounding box (28 bytes a feature) and working out the schema; a read picks the features whose
//! boxes meet the request and parses just those from the file again. A file up to
//! [`io::MAX_BYTES`](crate::io::MAX_BYTES) is read into memory for that one pass (the quickest
//! way); a larger one is read through a buffer, so it's never held whole. Smaller files are loaded
//! whole by `io::read_path`.

use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use super::{Batch, FeatureSource, Request, SourceInfo, clamp_limit, stride};
use crate::io::{FeatureSink, Schema, geojson_attrs, geojson_geometry};
use crate::{DataError, Feature, GeometryType, Result};
use rustgis_geom::Extent;
use serde_json::Value as Json;

fn bad(msg: impl std::fmt::Display) -> DataError {
    DataError::Format(format!("GeoJSON: {msg}"))
}

/// GeoJSON files larger than this are read in place (smaller ones load whole faster).
pub const IN_PLACE_ABOVE: u64 = 32 << 20;

/// Where a feature's text is in the file.
#[derive(Clone, Copy)]
struct Place {
    offset: u64,
    len: u32,
}

/// A GeoJSON FeatureCollection read in place.
pub struct GeoJsonSource {
    info: SourceInfo,
    path: PathBuf,
    places: Vec<Place>,
    /// Each feature's box, rounded outward (NaN for a feature without geometry).
    boxes: Vec<[f32; 4]>,
}

/// The features of a file as it's scanned.
#[derive(Default)]
struct Scan {
    schema: Schema,
    places: Vec<Place>,
    boxes: Vec<[f32; 4]>,
    extent: Extent,
    /// Points, lines and polygons seen.
    kinds: [usize; 3],
    skipped: usize,
}

impl Scan {
    /// One element of `"features"`, read from `len` bytes at `offset` (with the comma and
    /// whitespace before it).
    fn feature(&mut self, f: &Json, offset: u64, len: u64) {
        // Other values than objects are kept as features without geometry or values, as when
        // read whole, so features have the same ids both ways.
        // Every feature's properties count in the schema, those with unreadable geometry too.
        if let Some(props) = f.get("properties").and_then(Json::as_object) {
            for (k, v) in props {
                self.schema.see(k, v);
            }
        }
        let Ok(geom) = geojson_geometry(f) else {
            self.skipped += 1;
            return;
        };
        let Ok(len) = u32::try_from(len) else {
            self.skipped += 1;
            return;
        };
        let b = match &geom {
            Some(g) => {
                let e = g.extent();
                self.extent = self.extent.union(&e);
                if let Some(slot) = self.kinds.get_mut(match g.kind() {
                    GeometryType::Point => 0,
                    GeometryType::Polyline => 1,
                    _ => 2,
                }) {
                    *slot += 1;
                }
                outward(&e)
            }
            None => [f32::NAN; 4],
        };
        self.places.push(Place { offset, len });
        self.boxes.push(b);
    }
}

/// An extent as `f32`s, rounded outward (NaN when it isn't finite, so no request meets it).
fn outward(e: &Extent) -> [f32; 4] {
    if e.is_empty() || ![e.min_x, e.min_y, e.max_x, e.max_y].iter().all(|v| v.is_finite()) {
        return [f32::NAN; 4];
    }
    let down = |v: f64| {
        let f = v as f32;
        if f64::from(f) > v { f.next_down() } else { f }
    };
    let up = |v: f64| {
        let f = v as f32;
        if f64::from(f) < v { f.next_up() } else { f }
    };
    [down(e.min_x), down(e.min_y), up(e.max_x), up(e.max_y)]
}

impl GeoJsonSource {
    /// Opens a GeoJSON FeatureCollection. Other GeoJSON documents (a single feature, a bare
    /// geometry) and TopoJSON aren't read in place: they fail here, to be read whole.
    pub fn open_path(path: &Path, name: &str) -> Result<GeoJsonSource> {
        Self::open_with(path, name, crate::io::MAX_BYTES)
    }

    /// [`open_path`](Self::open_path), reading a file of at most `in_memory` bytes into memory to
    /// scan it, and a larger one through a buffer.
    pub(crate) fn open_with(path: &Path, name: &str, in_memory: u64) -> Result<GeoJsonSource> {
        use serde::de::DeserializeSeed;
        let mut scan = Scan::default();
        let rest = if std::fs::metadata(path)?.len() <= in_memory {
            let text = std::fs::read_to_string(path)?;
            crate::io::stream_features(&text, &mut InText { scan: &mut scan, start: text.as_ptr() as usize }).map_err(bad)?
        } else {
            let read = std::rc::Rc::new(std::cell::Cell::new(0u64));
            let file = Counting { inner: BufReader::with_capacity(BUFFER, File::open(path)?), read: read.clone() };
            let mut de = serde_json::Deserializer::from_reader(file);
            let rest = Top { scan: &mut scan, read }.deserialize(&mut de).map_err(bad)?;
            // Only whitespace may follow, as when read whole.
            de.end().map_err(bad)?;
            rest
        };
        if rest.get("type").and_then(Json::as_str) != Some("FeatureCollection") {
            return Err(DataError::Unsupported("only GeoJSON FeatureCollections are read in place".into()));
        }
        let mut notes = Vec::new();
        if scan.skipped > 0 {
            notes.push(format!("{} features had unreadable geometry", scan.skipped));
        }
        // The kind most features have, as a layer read whole takes.
        let geometry_type = match scan.kinds.iter().enumerate().max_by_key(|(_, n)| **n) {
            Some((_, 0)) | None => GeometryType::None,
            Some((0, _)) => GeometryType::Point,
            Some((1, _)) => GeometryType::Polyline,
            Some(_) => GeometryType::Polygon,
        };
        let info = SourceInfo {
            name: name.to_string(),
            format: "GeoJSON",
            geometry_type,
            fields: scan.schema.fields(),
            extent: scan.extent,
            count: Some(scan.places.len() as u64),
            crs: "EPSG:4326".into(),
            indexed: false,
            notes,
        };
        Ok(GeoJsonSource { info, path: path.to_path_buf(), places: scan.places, boxes: scan.boxes })
    }

    /// The features a request matches, in file order: every one without an extent; otherwise
    /// those whose boxes meet it (features without geometry never do).
    fn matches(&self, req: &Request) -> impl Iterator<Item = u32> + '_ {
        // Rounded to the nearest f32: never past a box rounded outward that meets the extent.
        let e = req.extent.map(|e| (e.is_empty(), [e.min_x as f32, e.min_y as f32, e.max_x as f32, e.max_y as f32]));
        self.boxes.iter().enumerate().filter_map(move |(i, b)| match e {
            None => Some(i as u32),
            Some((true, _)) => None,
            Some((false, [x0, y0, x1, y1])) => (b[0] <= x1 && b[2] >= x0 && b[1] <= y1 && b[3] >= y0).then_some(i as u32),
        })
    }
}

/// Bytes read ahead from the file: features near each other in a read share a fill.
const BUFFER: usize = 1 << 20;

impl FeatureSource for GeoJsonSource {
    fn info(&self) -> &SourceInfo {
        &self.info
    }

    fn read(&self, req: &Request) -> Result<Batch> {
        let limit = clamp_limit(req.limit);
        // Counted first, then the sample taken: no list of every match.
        let matched = self.matches(req).count();
        let step = stride(matched as u64, limit).max(1) as usize;
        let mut f = BufReader::with_capacity(BUFFER, File::open(&self.path)?);
        let mut pos = 0u64;
        let mut buf = Vec::new();
        let mut features = Vec::with_capacity(matched.div_ceil(step).min(limit));
        let mut skipped = 0;
        for i in self.matches(req).step_by(step).take(limit) {
            let Some(place) = self.places.get(i as usize) else { continue };
            // Ahead and near: through the buffer; otherwise a seek.
            let read = (|| -> std::io::Result<()> {
                match i64::try_from(place.offset.wrapping_sub(pos)) {
                    Ok(0) => {}
                    Ok(d) if place.offset > pos && d < BUFFER as i64 => f.seek_relative(d)?,
                    _ => {
                        f.seek(SeekFrom::Start(place.offset))?;
                    }
                }
                buf.resize(place.len as usize, 0);
                f.read_exact(&mut buf)
            })();
            if read.is_err() {
                // The file changed since it was opened: where it is now is unknown.
                pos = u64::MAX;
                skipped += 1;
                continue;
            }
            pos = place.offset + u64::from(place.len);
            // Past the comma and whitespace before the feature.
            let text = buf.iter().position(|b| !b.is_ascii_whitespace() && *b != b',').and_then(|at| buf.get(at..)).unwrap_or_default();
            let Ok(json) = serde_json::from_slice::<Json>(text) else {
                skipped += 1;
                continue;
            };
            let Ok(geometry) = geojson_geometry(&json) else {
                skipped += 1;
                continue;
            };
            features.push(Feature { id: u64::from(i) + 1, geometry, attrs: geojson_attrs(&json, &self.info.fields) });
        }
        Ok(Batch { features, matched: matched as u64, complete: step == 1, skipped })
    }
}

/// A reader counting the bytes read through it, for features' places in the file. The JSON reader
/// takes a byte at a time and looks at most one ahead, which it doesn't after a feature's last
/// byte, so the count after one is where it ends.
struct Counting<R> {
    inner: R,
    read: std::rc::Rc<std::cell::Cell<u64>>,
}

impl<R: Read> Read for Counting<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.read.set(self.read.get() + n as u64);
        Ok(n)
    }
}

/// The top level of the file: its members, but for `"features"`, which are scanned as they're
/// read.
struct Top<'a> {
    scan: &'a mut Scan,
    read: std::rc::Rc<std::cell::Cell<u64>>,
}

impl<'de> serde::de::DeserializeSeed<'de> for Top<'_> {
    type Value = serde_json::Map<String, Json>;

    fn deserialize<D: serde::Deserializer<'de>>(self, d: D) -> std::result::Result<Self::Value, D::Error> {
        d.deserialize_map(self)
    }
}

impl<'de> serde::de::Visitor<'de> for Top<'_> {
    type Value = serde_json::Map<String, Json>;

    fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str("a GeoJSON FeatureCollection")
    }

    fn visit_map<A: serde::de::MapAccess<'de>>(self, mut map: A) -> std::result::Result<Self::Value, A::Error> {
        let mut rest = serde_json::Map::new();
        while let Some(key) = map.next_key::<String>()? {
            if key == "features" {
                // The last of repeated members wins, as when read whole.
                *self.scan = Scan::default();
                map.next_value_seed(Elements { scan: &mut *self.scan, read: self.read.clone() })?;
            } else {
                rest.insert(key, map.next_value()?);
            }
        }
        Ok(rest)
    }
}

/// The `"features"` array, a feature at a time.
struct Elements<'a> {
    scan: &'a mut Scan,
    read: std::rc::Rc<std::cell::Cell<u64>>,
}

impl<'de> serde::de::DeserializeSeed<'de> for Elements<'_> {
    type Value = ();

    fn deserialize<D: serde::Deserializer<'de>>(self, d: D) -> std::result::Result<(), D::Error> {
        d.deserialize_seq(self)
    }
}

impl<'de> serde::de::Visitor<'de> for Elements<'_> {
    type Value = ();

    fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str("an array of features")
    }

    fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut seq: A) -> std::result::Result<(), A::Error> {
        loop {
            let start = self.read.get();
            let Some(f) = seq.next_element::<Json>()? else { break };
            self.scan.feature(&f, start, self.read.get().saturating_sub(start));
        }
        Ok(())
    }
}

/// The scan of a file read into memory: each feature's place is where its text is in the file's.
struct InText<'a> {
    scan: &'a mut Scan,
    /// Where the file's text starts in memory.
    start: usize,
}

impl FeatureSink for InText<'_> {
    fn restart(&mut self) {
        *self.scan = Scan::default();
    }

    fn feature(&mut self, raw: &str) {
        let offset = (raw.as_ptr() as usize).saturating_sub(self.start) as u64;
        match serde_json::from_str::<Json>(raw) {
            Ok(f) => self.scan.feature(&f, offset, raw.len() as u64),
            // Valid JSON, but with a number that doesn't fit.
            Err(_) => self.scan.skipped += 1,
        }
    }
}
