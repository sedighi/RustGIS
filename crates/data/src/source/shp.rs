//! Shapefiles read in place: a large `.shp` with its `.dbf` and `.prj`, a view at a time.
//!
//! Opening walks the `.shp` once, keeping each record's place and bounding box (24 bytes a
//! record); a read picks the records whose boxes meet the request, then reads just those shapes
//! and `.dbf` rows by their offsets. Smaller Shapefiles are still read whole by `io::read_path`,
//! so they can be edited.
//!
//! The files are hostile input: record lengths, part and point counts are checked against the
//! bytes there, and a record that doesn't parse is skipped and counted.

use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use super::{Batch, FeatureSource, Reproject, Request, SourceInfo, clamp_limit, stride};
use crate::io::group_rings;
use crate::{DataError, Feature, Field, FieldType, Geometry, GeometryType, Result, Value};
use rustgis_geom::{Coord, Extent};

fn bad(msg: impl std::fmt::Display) -> DataError {
    DataError::Format(format!("Shapefile: {msg}"))
}

/// The number of records a Shapefile holds, from the size of its `.shx` (without reading it).
pub fn record_count(path: &Path) -> Option<u64> {
    let len = std::fs::metadata(path.with_extension("shx")).ok()?.len();
    Some(len.checked_sub(100)? / 8)
}

/// Where a record's content is in the `.shp`, and how long it is.
#[derive(Clone, Copy)]
struct Place {
    offset: u32,
    len: u32,
}

/// A `.dbf` column.
struct Column {
    ty: u8,
    /// Byte offset in the row (after the deletion flag) and width.
    offset: usize,
    len: usize,
    out: FieldType,
}

/// A `.dbf` table's layout: rows of fixed width after the header.
struct Table {
    header_len: u64,
    row_len: usize,
    rows: u64,
    columns: Vec<Column>,
}

/// A Shapefile read in place.
pub struct ShpSource {
    info: SourceInfo,
    shp: PathBuf,
    dbf: PathBuf,
    places: Vec<Place>,
    /// Each record's box in the file's coordinates, rounded outward (NaN for a null shape).
    boxes: Vec<[f32; 4]>,
    table: Table,
    reproject: Reproject,
}

/// Bytes read ahead from the files: shapes and rows near each other in a read share a fill.
const BUFFER: usize = 1 << 20;

impl ShpSource {
    /// Opens `path` (the `.shp`) with the `.dbf` and `.prj` beside it.
    pub fn open_path(path: &Path, name: &str) -> Result<ShpSource> {
        let dbf = path.with_extension("dbf");
        if !dbf.is_file() {
            return Err(bad(format!("{} is missing", dbf.display())));
        }
        let mut notes = Vec::new();
        let table = read_table(&dbf)?;
        let shp_len = std::fs::metadata(path)?.len();
        if shp_len > u64::from(u32::MAX) {
            return Err(bad("the .shp is larger than the format's 4 GB"));
        }
        let mut f = BufReader::with_capacity(BUFFER, File::open(path)?);
        let mut header = [0u8; 100];
        f.read_exact(&mut header).map_err(|_| bad("the .shp header is cut short"))?;
        if be_i32(&header, 0) != Some(9994) {
            return Err(bad("not a .shp file"));
        }
        let shape_type = le_i32(&header, 32).unwrap_or(0);
        let geometry_type = match shape_type {
            1 | 11 | 21 | 8 | 18 | 28 => GeometryType::Point,
            3 | 13 | 23 => GeometryType::Polyline,
            5 | 15 | 25 => GeometryType::Polygon,
            _ => GeometryType::None,
        };
        let native = Extent::new(
            le_f64(&header, 36).unwrap_or(0.0),
            le_f64(&header, 44).unwrap_or(0.0),
            le_f64(&header, 52).unwrap_or(0.0),
            le_f64(&header, 60).unwrap_or(0.0),
        );
        // Walk the records by their headers: each one's place and box.
        let (mut places, mut boxes) = (Vec::new(), Vec::new());
        let mut pos = 100u64;
        let mut head = [0u8; 8 + 36];
        while pos + 8 <= shp_len {
            let want = (shp_len - pos).min(head.len() as u64) as usize;
            let Some(buf) = head.get_mut(..want) else { break };
            if f.read_exact(buf).is_err() {
                break;
            }
            let len = u64::try_from(be_i32(&head, 4).unwrap_or(-1)).unwrap_or(u64::MAX).saturating_mul(2);
            let content = pos + 8;
            if len < 4 || content.saturating_add(len) > shp_len {
                notes.push(format!("the .shp ends partway through record {}; the rest are left out", places.len() + 1));
                break;
            }
            // Only this record's bytes: a short one (a null shape, or a damaged one) mustn't read
            // its box from the next record's header.
            let c = head.get(8..(want as u64).min(8 + len) as usize).unwrap_or(&[]);
            boxes.push(record_box(c));
            places.push(Place { offset: content as u32, len: len as u32 });
            pos = content + len;
            // On to the next record's header, past what was read of this one.
            let read = (want as u64).saturating_sub(8);
            if f.seek_relative(len as i64 - read as i64).is_err() {
                break;
            }
        }
        if table.rows != places.len() as u64 {
            notes.push(format!("the .shp holds {} shapes and the .dbf {} rows; rows are matched by position", places.len(), table.rows));
        }
        let reproject = shp_reproject(crate::raster::read_prj(path).as_deref(), &mut notes);
        let extent = if native.is_empty() || !native.min_x.is_finite() { Extent::EMPTY } else { reproject.extent_to_lonlat(&native) };
        if reproject.is_lonlat()
            && !extent.is_empty()
            && (extent.min_x < -180.5 || extent.max_x > 180.5 || extent.min_y < -90.5 || extent.max_y > 90.5)
        {
            notes.push("coordinates are not longitude/latitude; RustGIS reads them as WGS 84 and they may draw out of place".into());
        }
        let fields = table.columns.iter().zip(read_names(&dbf, table.columns.len())?).map(|(c, n)| Field::new(n, c.out)).collect();
        let info = SourceInfo {
            name: name.to_string(),
            format: "Shapefile",
            geometry_type,
            fields,
            extent,
            count: Some(places.len() as u64),
            crs: reproject.label(),
            indexed: false,
            notes,
        };
        Ok(ShpSource { info, shp: path.to_path_buf(), dbf, places, boxes, table, reproject })
    }

    /// The records a request matches, in file order: every one without an extent; otherwise
    /// those whose boxes meet it (null shapes never do).
    fn matches(&self, req: &Request) -> impl Iterator<Item = u32> + '_ {
        // Rounded to the nearest f32: never past a box rounded outward that meets the extent.
        let e = req
            .extent
            .map(|e| self.reproject.extent_from_lonlat(&e))
            .map(|e| (e.is_empty(), [e.min_x as f32, e.min_y as f32, e.max_x as f32, e.max_y as f32]));
        self.boxes.iter().enumerate().filter_map(move |(i, b)| match e {
            None => Some(i as u32),
            Some((true, _)) => None,
            Some((false, [x0, y0, x1, y1])) => (b[0] <= x1 && b[2] >= x0 && b[1] <= y1 && b[3] >= y0).then_some(i as u32),
        })
    }
}

impl FeatureSource for ShpSource {
    fn info(&self) -> &SourceInfo {
        &self.info
    }

    fn read(&self, req: &Request) -> Result<Batch> {
        let limit = clamp_limit(req.limit);
        // Counted first, then the sample taken: no list of every match.
        let matched = self.matches(req).count();
        let step = stride(matched as u64, limit).max(1) as usize;
        let wanted: Vec<u32> = self.matches(req).step_by(step).take(limit).collect();
        let mut shp = Cursor::open(&self.shp)?;
        let mut dbf = Cursor::open(&self.dbf)?;
        let mut buf = Vec::new();
        let mut row = vec![0u8; self.table.row_len];
        let mut features = Vec::with_capacity(wanted.len());
        let mut skipped = 0;
        for i in wanted {
            let Some(place) = self.places.get(i as usize) else { continue };
            buf.resize(place.len as usize, 0);
            if shp.read_at(u64::from(place.offset), &mut buf).is_err() {
                skipped += 1;
                continue;
            }
            let geometry = match parse_shape(&buf) {
                Ok(Some(g)) => match self.reproject.geometry(g) {
                    Some(g) => Some(g),
                    None => {
                        skipped += 1;
                        continue;
                    }
                },
                Ok(None) => None,
                Err(()) => {
                    skipped += 1;
                    continue;
                }
            };
            let attrs = if u64::from(i) < self.table.rows {
                let at = self.table.header_len + u64::from(i) * self.table.row_len as u64;
                if dbf.read_at(at, &mut row).is_err() {
                    skipped += 1;
                    continue;
                }
                // A deleted row: its shape is left out too.
                if row.first() == Some(&b'*') {
                    continue;
                }
                self.table.columns.iter().map(|c| value(c, row.get(1 + c.offset..1 + c.offset + c.len).unwrap_or(&[]))).collect()
            } else {
                vec![Value::Null; self.table.columns.len()]
            };
            features.push(Feature { id: u64::from(i) + 1, geometry, attrs });
        }
        Ok(Batch { features, matched: matched as u64, complete: step == 1, skipped })
    }
}

/// A file read at offsets, through a buffer that a nearby offset reuses.
struct Cursor {
    f: BufReader<File>,
    pos: u64,
}

impl Cursor {
    fn open(path: &Path) -> Result<Cursor> {
        Ok(Cursor { f: BufReader::with_capacity(BUFFER, File::open(path)?), pos: 0 })
    }

    fn read_at(&mut self, at: u64, buf: &mut [u8]) -> std::io::Result<()> {
        if at != self.pos {
            // Ahead and near: within the buffer, or worth reading through.
            match i64::try_from(at.wrapping_sub(self.pos)) {
                Ok(d) if at > self.pos && d < BUFFER as i64 => self.f.seek_relative(d)?,
                _ => {
                    self.f.seek(SeekFrom::Start(at))?;
                }
            }
        }
        let r = self.f.read_exact(buf);
        // After a failure, where the file is is unknown: seek next time.
        self.pos = if r.is_ok() { at + buf.len() as u64 } else { u64::MAX };
        r
    }
}

fn be_i32(b: &[u8], at: usize) -> Option<i32> {
    Some(i32::from_be_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

fn le_i32(b: &[u8], at: usize) -> Option<i32> {
    Some(i32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

fn le_f64(b: &[u8], at: usize) -> Option<f64> {
    Some(f64::from_le_bytes(b.get(at..at + 8)?.try_into().ok()?))
}

/// A record's box from the start of its content (shape type, then a point or a box), rounded
/// outward to `f32`; NaN for a null or unreadable shape, so no extent meets it.
fn record_box(c: &[u8]) -> [f32; 4] {
    const NONE: [f32; 4] = [f32::NAN; 4];
    let b = match le_i32(c, 0) {
        Some(1 | 11 | 21) => match (le_f64(c, 4), le_f64(c, 12)) {
            (Some(x), Some(y)) => [x, y, x, y],
            _ => return NONE,
        },
        Some(3 | 5 | 8 | 13 | 15 | 18 | 23 | 25 | 28 | 31) => match (le_f64(c, 4), le_f64(c, 12), le_f64(c, 20), le_f64(c, 28)) {
            (Some(a), Some(b), Some(x), Some(y)) => [a, b, x, y],
            _ => return NONE,
        },
        _ => return NONE,
    };
    if b.iter().any(|v| !v.is_finite()) {
        return NONE;
    }
    let down = |v: f64| {
        let f = v as f32;
        if f64::from(f) > v { f.next_down() } else { f }
    };
    let up = |v: f64| {
        let f = v as f32;
        if f64::from(f) < v { f.next_up() } else { f }
    };
    [down(b[0]), down(b[1]), up(b[2]), up(b[3])]
}

/// Most parts and points one shape may have (a record's own length bounds them first).
const MAX_PARTS: usize = 1 << 24;

/// A record's shape: `Ok(None)` for a null shape, `Err` for one that doesn't parse or isn't
/// supported (a multipatch). Z and M values are dropped.
fn parse_shape(c: &[u8]) -> std::result::Result<Option<Geometry>, ()> {
    let pt = |at: usize| -> std::result::Result<Coord, ()> { Ok(Coord::new(le_f64(c, at).ok_or(())?, le_f64(c, at + 8).ok_or(())?)) };
    match le_i32(c, 0).ok_or(())? {
        0 => Ok(None),
        1 | 11 | 21 => Ok(Some(Geometry::Point(pt(4)?))),
        8 | 18 | 28 => {
            let n = usize::try_from(le_i32(c, 36).ok_or(())?).map_err(|_| ())?;
            if n > MAX_PARTS || 40 + n * 16 > c.len() {
                return Err(());
            }
            Ok(Some(Geometry::MultiPoint((0..n).map(|i| pt(40 + i * 16)).collect::<std::result::Result<_, _>>()?)))
        }
        t @ (3 | 5 | 13 | 15 | 23 | 25) => {
            let parts = usize::try_from(le_i32(c, 36).ok_or(())?).map_err(|_| ())?;
            let points = usize::try_from(le_i32(c, 40).ok_or(())?).map_err(|_| ())?;
            if parts > MAX_PARTS || points > MAX_PARTS || 44 + parts * 4 + points * 16 > c.len() {
                return Err(());
            }
            let at = 44 + parts * 4;
            let mut lines: Vec<Vec<Coord>> = Vec::with_capacity(parts);
            for p in 0..parts {
                let start = usize::try_from(le_i32(c, 44 + p * 4).ok_or(())?).map_err(|_| ())?;
                let end = if p + 1 < parts { usize::try_from(le_i32(c, 44 + (p + 1) * 4).ok_or(())?).map_err(|_| ())? } else { points };
                if start > end || end > points {
                    return Err(());
                }
                lines.push((start..end).map(|i| pt(at + i * 16)).collect::<std::result::Result<_, _>>()?);
            }
            if matches!(t, 3 | 13 | 23) {
                return Ok(Some(Geometry::Line(lines)));
            }
            // A ring wound clockwise is an outer one (the shapefile crate's rule, for the same
            // shapes as a Shapefile read whole).
            let outer = |r: &[Coord]| r.windows(2).map(|w| (w[1].x - w[0].x) * (w[1].y + w[0].y)).sum::<f64>() >= 0.0;
            Ok(Some(group_rings(
                lines
                    .into_iter()
                    .map(|r| {
                        let o = outer(&r);
                        (r, o)
                    })
                    .collect(),
            )))
        }
        _ => Err(()),
    }
}

/// Reads a `.dbf` header: its row layout and columns.
fn read_table(path: &Path) -> Result<Table> {
    let mut f = BufReader::new(File::open(path)?);
    let mut head = [0u8; 32];
    f.read_exact(&mut head).map_err(|_| bad("the .dbf header is cut short"))?;
    let rows = u64::from(u32::from_le_bytes(head.get(4..8).and_then(|b| b.try_into().ok()).unwrap_or([0; 4])));
    let header_len = u64::from(u16::from_le_bytes([head[8], head[9]]));
    let row_len = usize::from(u16::from_le_bytes([head[10], head[11]]));
    if header_len < 33 || row_len == 0 {
        return Err(bad("the .dbf header is damaged"));
    }
    let mut columns = Vec::new();
    let mut offset = 0usize;
    // Field descriptors, 32 bytes each, until the 0x0D terminator.
    for _ in 0..((header_len - 32) / 32).min(2048) {
        let mut d = [0u8; 32];
        if f.read_exact(&mut d[..1]).is_err() || d[0] == 0x0D {
            break;
        }
        f.read_exact(&mut d[1..]).map_err(|_| bad("the .dbf header is cut short"))?;
        let (ty, len, decimals) = (d[11].to_ascii_uppercase(), usize::from(d[16]), d[17]);
        let out = match ty {
            b'N' if decimals == 0 && len <= 18 => FieldType::Integer,
            b'I' => FieldType::Integer,
            b'N' | b'F' | b'O' | b'Y' => FieldType::Double,
            b'L' => FieldType::Boolean,
            _ => FieldType::Text,
        };
        columns.push(Column { ty, offset, len, out });
        offset += len;
    }
    if offset + 1 > row_len {
        return Err(bad("the .dbf's fields are wider than its rows"));
    }
    Ok(Table { header_len, row_len, rows, columns })
}

/// The `.dbf`'s column names, in order (read again: the layout above keeps only offsets).
fn read_names(path: &Path, n: usize) -> Result<Vec<String>> {
    let mut f = BufReader::new(File::open(path)?);
    f.seek(SeekFrom::Start(32))?;
    let mut out = Vec::with_capacity(n);
    let mut d = [0u8; 32];
    for _ in 0..n {
        f.read_exact(&mut d)?;
        let name = d.get(..11).unwrap_or(&[]);
        let end = name.iter().position(|b| *b == 0).unwrap_or(name.len());
        out.push(String::from_utf8_lossy(name.get(..end).unwrap_or(&[])).trim().to_string());
    }
    Ok(out)
}

/// A field's bytes up to the first NUL, without the spaces around them.
fn trimmed(b: &[u8]) -> &[u8] {
    let b = b.get(..b.iter().position(|c| *c == 0).unwrap_or(b.len())).unwrap_or(&[]);
    let start = b.iter().position(|c| *c != b' ').unwrap_or(b.len());
    let end = b.iter().rposition(|c| *c != b' ').map_or(start, |e| e + 1);
    b.get(start..end).unwrap_or(&[])
}

/// One `.dbf` value, as a Shapefile read whole gives it (blank and unreadable values are null).
fn value(c: &Column, b: &[u8]) -> Value {
    let text =
        || std::str::from_utf8(trimmed(b)).ok().map(str::to_string).unwrap_or_else(|| String::from_utf8_lossy(trimmed(b)).into_owned());
    let number = || {
        let t = trimmed(b);
        if t.is_empty() || t.iter().all(|c| *c == b'*') {
            return None;
        }
        std::str::from_utf8(t).ok()?.parse::<f64>().ok().filter(|v| v.is_finite())
    };
    match c.ty {
        b'C' => {
            let t = text();
            if t.is_empty() { Value::Null } else { Value::Text(t) }
        }
        // A whole-number column's values as integers, exactly (its at most 18 digits fit an i64);
        // one written with a point or exponent (`12.0`, `1e3`) too, and one that isn't whole is
        // kept as a number rather than lost.
        b'N' | b'F' if c.out == FieldType::Integer => match std::str::from_utf8(trimmed(b)).ok().and_then(|t| t.parse::<i64>().ok()) {
            Some(i) => Value::Int(i),
            None => {
                number().map_or(Value::Null, |v| if v.fract() == 0.0 && v.abs() < 9.0e15 { Value::Int(v as i64) } else { Value::Float(v) })
            }
        },
        b'N' | b'F' => number().map_or(Value::Null, Value::Float),
        b'I' => b.get(..4).and_then(|v| v.try_into().ok()).map_or(Value::Null, |v| Value::Int(i64::from(i32::from_le_bytes(v)))),
        // Currency as the `dbase` crate reads it (a Shapefile read whole): a little-endian f64.
        b'O' | b'Y' => {
            b.get(..8).and_then(|v| v.try_into().ok()).map(f64::from_le_bytes).filter(|v| v.is_finite()).map_or(Value::Null, Value::Float)
        }
        b'L' => match trimmed(b).first() {
            Some(b'1' | b'T' | b't' | b'Y' | b'y') => Value::Bool(true),
            Some(b'0' | b'F' | b'f' | b'N' | b'n') => Value::Bool(false),
            _ => Value::Null,
        },
        b'D' => {
            let t = trimmed(b);
            match t.get(..8) {
                Some(d) if d.iter().all(u8::is_ascii_digit) => {
                    let part = |r: std::ops::Range<usize>| String::from_utf8_lossy(d.get(r).unwrap_or(&[])).into_owned();
                    Value::Text(format!("{}-{}-{}", part(0..4), part(4..6), part(6..8)))
                }
                _ => Value::Null,
            }
        }
        // Memos (in a `.dbt` beside the file), date-times and binary fields aren't read.
        b'M' | b'T' | b'@' | b'B' | b'G' | b'P' => Value::Null,
        _ => {
            let t = text();
            if t.is_empty() { Value::Null } else { Value::Text(t) }
        }
    }
}

/// The coordinate system in a `.prj`, with what couldn't be understood said in the notes.
fn shp_reproject(prj: Option<&str>, notes: &mut Vec<String>) -> Reproject {
    let crs = match prj.map(crate::raster::wkt::read_wkt) {
        None => crate::raster::Crs::LonLat,
        Some(Ok((c, note))) => {
            if let Some(n) = note {
                notes.push(format!("the .prj file: {n}"));
            }
            c
        }
        Some(Err(e)) => {
            notes.push(format!("the .prj file: {e}; coordinates are read as longitude/latitude"));
            crate::raster::Crs::LonLat
        }
    };
    Reproject::from_crs(crs, notes)
}
