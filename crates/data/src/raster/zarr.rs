//! Zarr stores (format 2 and 3): a variable's 2D slice — latitude/longitude or y/x, its other
//! dimensions (time, level) picked by index — read as a raster.
//!
//! The store is read in steps so a caller can fetch its files however it reaches them (a folder,
//! the web): [`METADATA_KEYS`] and [`parse_store`], then [`plan`] names the chunk files a slice
//! needs and [`assemble`] decodes them. Chunks are decoded in pure Rust: blosc (LZ4, zlib, zstd,
//! with byte shuffle), zlib, gzip, zstd and LZ4. Every count, size and length in a store is
//! hostile input and bounded.

use std::collections::HashMap;

use serde_json::{Map, Value};

use super::{ColorHint, Crs, GeoTransform, MAX_SAMPLES, Raster};
use crate::{DataError, Result};

/// The metadata files a store may have at its root: Zarr 3's `zarr.json`, Zarr 2's consolidated
/// `.zmetadata`, and a lone Zarr 2 array's or group's own files.
pub const METADATA_KEYS: [&str; 5] = ["zarr.json", ".zmetadata", ".zarray", ".zattrs", ".zgroup"];

/// Largest metadata document read.
pub const MAX_METADATA: usize = 32 << 20;
/// Most arrays a store may hold.
const MAX_ARRAYS: usize = 10_000;
/// Most dimensions of an array.
const MAX_DIMS: usize = 32;
/// Most chunk files a read fetches.
pub const MAX_CHUNKS: usize = 4096;
/// Most bytes a chunk may hold, decoded (a slice reads only its values out of them).
#[cfg(not(target_arch = "wasm32"))]
const MAX_CHUNK_BYTES: u64 = 1 << 30;
#[cfg(target_arch = "wasm32")]
const MAX_CHUNK_BYTES: u64 = 256 << 20;
/// Longest coordinate array read (the labels of a time dimension, say).
const MAX_COORDS: u64 = 4_000_000;
/// Largest chunk file read from a folder (stored, before decoding).
pub const MAX_CHUNK_FILE: u64 = MAX_CHUNK_BYTES;
/// Most bytes of chunk files one read takes from a folder.
pub const MAX_READ: u64 = 4 << 30;

fn bad(m: impl Into<String>) -> DataError {
    DataError::Format(m.into())
}

/// How a value is stored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Int,
    UInt,
    Float,
    Bool,
}

/// A data type: its kind, size in bytes and byte order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DType {
    pub kind: Kind,
    pub size: usize,
    pub little: bool,
}

impl DType {
    /// Zarr 2's `<f4`, `|u1`, `>i2`…
    fn parse_v2(s: &str) -> Option<DType> {
        let b = s.as_bytes();
        let (order, kind, size) = (*b.first()?, *b.get(1)?, s.get(2..)?.parse::<usize>().ok()?);
        let little = order != b'>';
        let kind = match kind {
            b'i' => Kind::Int,
            b'u' => Kind::UInt,
            b'f' => Kind::Float,
            b'b' => Kind::Bool,
            _ => return None,
        };
        DType { kind, size, little }.valid()
    }

    /// Zarr 3's `float32`, `uint8`…
    fn parse_v3(s: &str) -> Option<DType> {
        let (kind, bits) = if let Some(b) = s.strip_prefix("uint") {
            (Kind::UInt, b)
        } else if let Some(b) = s.strip_prefix("int") {
            (Kind::Int, b)
        } else if let Some(b) = s.strip_prefix("float") {
            (Kind::Float, b)
        } else if s == "bool" {
            (Kind::Bool, "8")
        } else {
            return None;
        };
        DType { kind, size: bits.parse::<usize>().ok()? / 8, little: true }.valid()
    }

    /// `v` stored as this type, in the byte order given (a fill value).
    fn encode(self, v: f64, little: bool) -> Vec<u8> {
        let mut b = match (self.kind, self.size) {
            (Kind::Float, 8) => v.to_le_bytes().to_vec(),
            (Kind::Float, 4) => (v as f32).to_le_bytes().to_vec(),
            (Kind::Float, _) => to_f16(v).to_le_bytes().to_vec(),
            (Kind::Int, n) => (v as i64).to_le_bytes().get(..n).unwrap_or_default().to_vec(),
            (Kind::UInt | Kind::Bool, n) => (v as u64).to_le_bytes().get(..n).unwrap_or_default().to_vec(),
        };
        if !little {
            b.reverse();
        }
        b
    }

    fn valid(self) -> Option<DType> {
        let ok = match self.kind {
            Kind::Int | Kind::UInt => matches!(self.size, 1 | 2 | 4 | 8),
            Kind::Float => matches!(self.size, 2 | 4 | 8),
            Kind::Bool => self.size == 1,
        };
        ok.then_some(self)
    }

    /// A value from its bytes (exactly `size` of them).
    fn value(self, b: &[u8]) -> f64 {
        let mut a = [0u8; 8];
        let n = self.size.min(8);
        if let Some(src) = b.get(..n) {
            if self.little {
                a[..n].copy_from_slice(src);
            } else {
                for (i, v) in src.iter().rev().enumerate() {
                    a[i] = *v;
                }
            }
        }
        // Sign extension for the signed kinds, from the little-endian bytes.
        let sign = |bits: u32| {
            let u = u64::from_le_bytes(a);
            let shift = 64 - bits;
            ((u << shift) as i64 >> shift) as f64
        };
        match (self.kind, self.size) {
            (Kind::Bool, _) | (Kind::UInt, 1) => f64::from(a[0]),
            (Kind::UInt, 2) => f64::from(u16::from_le_bytes([a[0], a[1]])),
            (Kind::UInt, 4) => f64::from(u32::from_le_bytes([a[0], a[1], a[2], a[3]])),
            (Kind::UInt, _) => u64::from_le_bytes(a) as f64,
            (Kind::Int, n) => sign((n * 8) as u32),
            (Kind::Float, 2) => f16(u16::from_le_bytes([a[0], a[1]])),
            (Kind::Float, 4) => f64::from(f32::from_le_bytes([a[0], a[1], a[2], a[3]])),
            (Kind::Float, _) => f64::from_le_bytes(a),
        }
    }
}

/// A half-precision float's value.
fn f16(h: u16) -> f64 {
    let sign = if h & 0x8000 != 0 { -1.0 } else { 1.0 };
    let exp = i32::from((h >> 10) & 0x1f);
    let frac = f64::from(h & 0x3ff);
    sign * match exp {
        0 => frac * 2f64.powi(-24),
        31 if frac == 0.0 => f64::INFINITY,
        31 => f64::NAN,
        e => (1.0 + frac / 1024.0) * 2f64.powi(e - 15),
    }
}

/// A half-precision float's bits (rounded towards zero).
fn to_f16(v: f64) -> u16 {
    let x = v as f32;
    let b = x.to_bits();
    let sign = ((b >> 16) & 0x8000) as u16;
    if x.is_nan() {
        return sign | 0x7e00;
    }
    let e = ((b >> 23) & 0xff) as i32 - 127 + 15;
    let m = b & 0x7f_ffff;
    if e >= 31 {
        sign | 0x7c00
    } else if e <= 0 {
        if e < -10 { sign } else { sign | ((m | 0x80_0000) >> (14 - e)) as u16 }
    } else {
        sign | ((e as u16) << 10) | (m >> 13) as u16
    }
}

/// A step of a chunk's encoding, undone in reverse order.
#[derive(Clone, Debug, PartialEq)]
pub enum Codec {
    Blosc,
    Zlib,
    Gzip,
    Zstd,
    /// numcodecs' LZ4: a little-endian size, then an LZ4 block.
    Lz4,
    /// numcodecs' byte shuffle of `n`-byte elements.
    Shuffle(usize),
    /// Zarr 3's checksum after the data, dropped.
    Crc32c,
    /// Zarr 3's `bytes` codec: the values' byte order.
    Bytes {
        little: bool,
    },
    /// Zarr 3's sharding: the chunk (a shard) holds smaller chunks of `inner` values, each
    /// encoded with `codecs`, and an index of where they are (`index` its encoding, at the end or
    /// the start).
    Sharding {
        inner: Vec<u64>,
        codecs: Vec<Codec>,
        index: Vec<Codec>,
        index_at_end: bool,
    },
    /// A codec this reader doesn't know: the array can't be read, and this says why.
    Unsupported(String),
}

/// An array of a store.
#[derive(Clone, Debug)]
pub struct Array {
    /// Its path in the store (`2m_temperature`, `group/var`; empty for a store that is one array).
    pub path: String,
    pub zarr_format: u8,
    pub shape: Vec<u64>,
    pub chunks: Vec<u64>,
    pub dtype: DType,
    pub fill: Option<f64>,
    /// Fortran order (the first dimension varies fastest).
    pub fortran: bool,
    /// The encoding, in the order it was applied.
    pub codecs: Vec<Codec>,
    /// Its dimensions' names (`_ARRAY_DIMENSIONS`, `dimension_names`), when it says.
    pub dims: Vec<String>,
    pub attrs: Map<String, Value>,
    /// Chunk keys: a prefix (`c/` in Zarr 3) and the separator between indexes.
    key_prefix: String,
    separator: char,
}

impl Array {
    /// Its name: the last part of its path.
    pub fn name(&self) -> &str {
        self.path.rsplit('/').next().unwrap_or(&self.path)
    }

    /// Its dimension names, made up (`dim_0`…) where it doesn't say.
    pub fn dim_names(&self) -> Vec<String> {
        (0..self.shape.len()).map(|i| self.dims.get(i).filter(|d| !d.is_empty()).cloned().unwrap_or_else(|| format!("dim_{i}"))).collect()
    }

    /// Why it can't be read, if it can't.
    pub fn unsupported(&self) -> Option<String> {
        unsupported(&self.codecs)
    }

    /// The key of a chunk, relative to the store's root.
    pub fn chunk_key(&self, idx: &[u64]) -> String {
        let parts: Vec<String> = idx.iter().map(u64::to_string).collect();
        // A 0-dimensional array's one chunk is `0` in Zarr 2 and `c` in Zarr 3.
        let body = if parts.is_empty() && self.zarr_format == 2 { "0".to_string() } else { parts.join(&self.separator.to_string()) };
        let key = format!("{}{}", self.key_prefix, body).trim_end_matches('/').to_string();
        if self.path.is_empty() { key } else { format!("{}/{key}", self.path) }
    }

    /// How many chunks lie along dimension `d`.
    fn grid(&self, d: usize) -> u64 {
        let (s, c) = (self.shape.get(d).copied().unwrap_or(0), self.chunks.get(d).copied().unwrap_or(1).max(1));
        s.div_ceil(c)
    }

    fn chunk_values(&self) -> Result<u64> {
        let n = self.chunks.iter().try_fold(1u64, |a, &c| a.checked_mul(c)).ok_or_else(|| bad("a chunk too large"))?;
        if n.saturating_mul(self.dtype.size as u64) > MAX_CHUNK_BYTES {
            return Err(bad(format!("{}: chunks of {n} values are too large to read", self.path)));
        }
        Ok(n)
    }

    /// Decodes a chunk's file into its values (row-major or column-major, as stored).
    pub fn decode(&self, bytes: &[u8]) -> Result<Vec<f64>> {
        let (data, dt) = self.decode_bytes(bytes)?;
        Ok(data.chunks_exact(dt.size).map(|b| dt.value(b)).collect())
    }

    /// Decodes a chunk's file into its values' bytes and their type (byte order included):
    /// value `i` is at `i * size`.
    pub fn decode_bytes(&self, bytes: &[u8]) -> Result<(Vec<u8>, DType)> {
        self.chunk_values()?;
        let (data, little) = decode_chunk(&self.codecs, bytes, &self.chunks, self.dtype, self.fill).map_err(|e| match e {
            e if self.path.is_empty() => e,
            DataError::Format(m) => bad(format!("{}: {m}", self.path)),
            e => bad(format!("{}: {e}", self.path)),
        })?;
        Ok((data, DType { little, ..self.dtype }))
    }
}

/// Why chunks encoded with `codecs` can't be read, if they can't.
fn unsupported(codecs: &[Codec]) -> Option<String> {
    codecs.iter().find_map(|c| match c {
        Codec::Unsupported(why) => Some(why.clone()),
        Codec::Sharding { codecs, index, .. } => unsupported(codecs).or_else(|| unsupported(index)),
        _ => None,
    })
}

/// Undoes `codecs` on a chunk of `shape` values of type `dtype`: its values' bytes (as many as
/// the shape holds, never more) and their byte order. `fill` stands for a shard's missing chunks.
fn decode_chunk(codecs: &[Codec], bytes: &[u8], shape: &[u64], dtype: DType, fill: Option<f64>) -> Result<(Vec<u8>, bool)> {
    let n = shape.iter().try_fold(1u64, |a, &c| a.checked_mul(c)).ok_or_else(|| bad("a chunk too large"))?;
    let want = usize::try_from(n).ok().and_then(|n| n.checked_mul(dtype.size)).ok_or_else(|| bad("a chunk too large"))?;
    let mut data = bytes.to_vec();
    let mut little = dtype.little;
    for c in codecs.iter().rev() {
        data = match c {
            Codec::Blosc => blosc(&data, want)?,
            Codec::Zlib => inflate(&data, true, want)?,
            Codec::Gzip => gunzip(&data, want)?,
            Codec::Zstd => unzstd(&data, want)?,
            Codec::Lz4 => numcodecs_lz4(&data, want)?,
            Codec::Shuffle(e) => unshuffle(&data, *e),
            Codec::Crc32c => data.get(..data.len().saturating_sub(4)).unwrap_or_default().to_vec(),
            Codec::Bytes { little: l } => {
                little = *l;
                data
            }
            Codec::Sharding { inner, codecs, index, index_at_end } => {
                let (d, l) = unshard(&data, shape, inner, codecs, index, *index_at_end, dtype, fill)?;
                little = l;
                d
            }
            Codec::Unsupported(why) => return Err(bad(why.clone())),
        };
    }
    if data.len() < want {
        return Err(bad(format!("a chunk holds {} bytes, not {want}", data.len())));
    }
    data.truncate(want);
    Ok((data, little))
}

/// A store's arrays.
#[derive(Clone, Debug, Default)]
pub struct Store {
    pub arrays: Vec<Array>,
}

impl Store {
    pub fn array(&self, path: &str) -> Option<&Array> {
        self.arrays.iter().find(|a| a.path == path).or_else(|| self.arrays.iter().find(|a| a.name() == path))
    }

    /// The arrays that are dimensions' coordinates (`lat`, `time`): 1D arrays named like a
    /// dimension of another array.
    fn is_coordinate(&self, a: &Array) -> bool {
        a.shape.len() == 1 && self.arrays.iter().any(|o| o.shape.len() > 1 && o.dim_names().iter().any(|d| d == a.name()))
    }

    /// Cell bounds (`lat_bnds`, CF's `bounds` attribute): coordinates' edges, not maps.
    fn is_bounds(&self, a: &Array) -> bool {
        let name = a.name().to_ascii_lowercase();
        name.ends_with("_bnds")
            || name.ends_with("_bounds")
            || a.dim_names()
                .iter()
                .any(|d| matches!(d.to_ascii_lowercase().as_str(), "bnds" | "nbnd" | "nbnds" | "bounds" | "nv" | "vertices"))
            || self.arrays.iter().any(|o| o.attrs.get("bounds").and_then(Value::as_str) == Some(a.name()))
    }

    /// The arrays that can be shown: two or more dimensions, not coordinates or their bounds.
    pub fn variables(&self) -> Vec<&Array> {
        self.arrays.iter().filter(|a| a.shape.len() >= 2 && !self.is_coordinate(a) && !self.is_bounds(a)).collect()
    }

    /// A dimension's coordinate array, for `of` (in its group first).
    fn coordinate(&self, of: &Array, dim: &str) -> Option<&Array> {
        let group = of.path.rsplit_once('/').map(|(g, _)| g.to_string());
        let in_group = group.as_ref().map(|g| format!("{g}/{dim}"));
        self.arrays
            .iter()
            .find(|a| a.shape.len() == 1 && Some(&a.path) == in_group.as_ref())
            .or_else(|| self.arrays.iter().find(|a| a.shape.len() == 1 && a.path == dim))
    }
}

/// Reads a store's metadata: `docs` are the metadata files found, by key from the root
/// (`zarr.json`, `.zmetadata`, or each array's `.zarray` and `.zattrs`, or `zarr.json`).
/// Whether a path from a store's metadata stays inside the store: names joined by `/`, none of
/// them `.`, `..` or empty, with no root, backslash or drive (`C:`). Paths name files under a
/// folder and addresses under a web store, so one that climbs out is refused.
fn safe_path(p: &str) -> bool {
    p.is_empty() || (!p.starts_with('/') && !p.contains(['\\', ':', '\0']) && p.split('/').all(|s| !s.is_empty() && s != "." && s != ".."))
}

pub fn parse_store(docs: &[(String, Vec<u8>)]) -> Result<Store> {
    let mut json: HashMap<String, Value> = HashMap::new();
    for (k, b) in docs.iter().take(4 * MAX_ARRAYS) {
        if b.len() > MAX_METADATA {
            return Err(bad(format!("{k} is too large")));
        }
        if let Ok(v) = serde_json::from_slice::<Value>(b) {
            json.insert(k.trim_start_matches('/').to_string(), v);
        }
    }
    // Zarr 2's consolidated metadata holds every array's files.
    if let Some(Value::Object(m)) = json.get(".zmetadata").and_then(|z| z.get("metadata")).cloned() {
        for (k, v) in m.into_iter().take(4 * MAX_ARRAYS) {
            json.entry(k).or_insert(v);
        }
    }
    // Zarr 3's consolidated metadata, in the root group's zarr.json.
    if let Some(Value::Object(m)) = json.get("zarr.json").and_then(|z| z.pointer("/consolidated_metadata/metadata")).cloned() {
        for (k, v) in m.into_iter().take(MAX_ARRAYS) {
            json.entry(format!("{}/zarr.json", k.trim_matches('/'))).or_insert(v);
        }
    }
    let mut arrays = Vec::new();
    let mut keys: Vec<&String> = json.keys().collect();
    keys.sort();
    for k in keys {
        if arrays.len() >= MAX_ARRAYS {
            break;
        }
        let Some(v) = json.get(k) else { continue };
        // A key that would lead out of the store (`../x/.zarray`, `/etc/x/.zarray`) is skipped.
        let dir = k.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
        if !safe_path(dir) {
            continue;
        }
        let a = if let Some(path) = k.strip_suffix(".zarray") {
            let path = path.trim_end_matches('/');
            let attrs = json.get(&format!("{}.zattrs", if path.is_empty() { String::new() } else { format!("{path}/") }));
            array_v2(path, v, attrs)
        } else if let Some(path) = k.strip_suffix("zarr.json") {
            if v.get("node_type").and_then(Value::as_str) != Some("array") {
                continue;
            }
            array_v3(path.trim_end_matches('/'), v)
        } else {
            continue;
        };
        match a {
            Some(a) => arrays.push(a),
            None => continue,
        }
    }
    if arrays.is_empty() {
        return Err(bad(
            "no Zarr arrays found: give a store with consolidated metadata (.zmetadata, or zarr.json at its root), a folder on this computer, or the address of one array",
        ));
    }
    Ok(Store { arrays })
}

fn u64s(v: Option<&Value>) -> Option<Vec<u64>> {
    let a = v?.as_array()?;
    if a.len() > MAX_DIMS {
        return None;
    }
    a.iter().map(Value::as_u64).collect()
}

fn fill_of(v: Option<&Value>) -> Option<f64> {
    match v? {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => match s.as_str() {
            "NaN" => Some(f64::NAN),
            "Infinity" => Some(f64::INFINITY),
            "-Infinity" => Some(f64::NEG_INFINITY),
            _ => None,
        },
        Value::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
        _ => None,
    }
}

fn dims_of(v: Option<&Value>) -> Vec<String> {
    v.and_then(Value::as_array).map(|a| a.iter().take(MAX_DIMS).map(|d| d.as_str().unwrap_or("").to_string()).collect()).unwrap_or_default()
}

fn array_v2(path: &str, z: &Value, attrs: Option<&Value>) -> Option<Array> {
    let shape = u64s(z.get("shape"))?;
    let chunks = u64s(z.get("chunks"))?;
    if chunks.len() != shape.len() || chunks.contains(&0) {
        return None;
    }
    let dtype = DType::parse_v2(z.get("dtype")?.as_str()?);
    let mut codecs = Vec::new();
    for f in z.get("filters").and_then(Value::as_array).into_iter().flatten().take(8) {
        codecs.push(numcodec(f));
    }
    if let Some(c) = z.get("compressor").filter(|c| !c.is_null()) {
        codecs.push(numcodec(c));
    }
    let dtype = match dtype {
        Some(d) => d,
        None => {
            codecs.push(Codec::Unsupported(format!("{path}: its data type {} isn't read", z.get("dtype").unwrap_or(&Value::Null))));
            DType { kind: Kind::UInt, size: 1, little: true }
        }
    };
    let attrs = attrs.and_then(Value::as_object).cloned().unwrap_or_default();
    Some(Array {
        path: path.to_string(),
        zarr_format: 2,
        dims: dims_of(attrs.get("_ARRAY_DIMENSIONS")),
        shape,
        chunks,
        dtype,
        fill: fill_of(z.get("fill_value")),
        fortran: z.get("order").and_then(Value::as_str) == Some("F"),
        codecs,
        attrs,
        key_prefix: String::new(),
        separator: if z.get("dimension_separator").and_then(Value::as_str) == Some("/") { '/' } else { '.' },
    })
}

/// A Zarr 3 array's codecs, for chunks of `chunks` values; a shard's (not `outer`) can't be
/// sharded again.
fn codecs_v3(path: &str, list: Option<&Value>, chunks: &[u64], outer: bool) -> Vec<Codec> {
    let mut codecs = Vec::new();
    for c in list.and_then(Value::as_array).into_iter().flatten().take(8) {
        let name = c.get("name").and_then(Value::as_str).unwrap_or("");
        codecs.push(match name {
            "bytes" => Codec::Bytes { little: c.pointer("/configuration/endian").and_then(Value::as_str) != Some("big") },
            "gzip" => Codec::Gzip,
            "zstd" => Codec::Zstd,
            "blosc" => Codec::Blosc,
            "crc32c" => Codec::Crc32c,
            "sharding_indexed" if outer => sharding(path, c, chunks),
            "sharding_indexed" => Codec::Unsupported(format!("{path}: shards inside shards aren't read")),
            "transpose" => Codec::Unsupported(format!("{path}: transposed chunks aren't read")),
            other => Codec::Unsupported(format!("{path}: the {other} codec isn't read")),
        });
    }
    // Sharding turns the values into bytes, so only it can come first; anything after it would
    // encode the whole shard, which isn't read.
    if codecs.len() > 1 && codecs.iter().any(|c| matches!(c, Codec::Sharding { .. })) {
        codecs.push(Codec::Unsupported(format!("{path}: only shards with nothing encoded after them are read")));
    }
    codecs
}

/// The `sharding_indexed` codec of an array whose chunks (shards) hold `shard` values.
fn sharding(path: &str, c: &Value, shard: &[u64]) -> Codec {
    let cfg = c.get("configuration");
    let inner = u64s(cfg.and_then(|c| c.get("chunk_shape"))).unwrap_or_default();
    // The inner chunks tile the shard exactly.
    if inner.len() != shard.len() || inner.iter().zip(shard).any(|(&i, &s)| i == 0 || !s.is_multiple_of(i)) {
        return Codec::Unsupported(format!("{path}: its shards' chunks don't tile its shards"));
    }
    let codecs = codecs_v3(path, cfg.and_then(|c| c.get("codecs")), &inner, false);
    let index = codecs_v3(path, cfg.and_then(|c| c.get("index_codecs")), &[], false);
    // The index's size is known only when it is stored as it is (with a checksum, or not).
    if !index.iter().all(|c| matches!(c, Codec::Bytes { .. } | Codec::Crc32c)) {
        return Codec::Unsupported(format!("{path}: its shards' index is encoded in a way that isn't read"));
    }
    let index_at_end = cfg.and_then(|c| c.get("index_location")).and_then(Value::as_str) != Some("start");
    Codec::Sharding { inner, codecs, index, index_at_end }
}

/// A Zarr 2 (numcodecs) codec.
fn numcodec(c: &Value) -> Codec {
    match c.get("id").and_then(Value::as_str).unwrap_or("") {
        "blosc" => Codec::Blosc,
        "zlib" => Codec::Zlib,
        "gzip" => Codec::Gzip,
        "zstd" => Codec::Zstd,
        "lz4" => Codec::Lz4,
        "shuffle" => {
            Codec::Shuffle(c.get("elementsize").and_then(Value::as_u64).and_then(|e| usize::try_from(e).ok()).unwrap_or(4).clamp(1, 16))
        }
        other => Codec::Unsupported(format!("the {other} codec isn't read")),
    }
}

fn array_v3(path: &str, z: &Value) -> Option<Array> {
    let shape = u64s(z.get("shape"))?;
    let chunks = u64s(z.pointer("/chunk_grid/configuration/chunk_shape"))?;
    if chunks.len() != shape.len() || chunks.contains(&0) {
        return None;
    }
    let mut codecs = Vec::new();
    let dtype = match z.get("data_type").and_then(Value::as_str).and_then(DType::parse_v3) {
        Some(d) => d,
        None => {
            codecs.push(Codec::Unsupported(format!("{path}: its data type {} isn't read", z.get("data_type").unwrap_or(&Value::Null))));
            DType { kind: Kind::UInt, size: 1, little: true }
        }
    };
    if z.pointer("/chunk_grid/name").and_then(Value::as_str) != Some("regular") {
        codecs.push(Codec::Unsupported(format!("{path}: only regular chunk grids are read")));
    }
    codecs.extend(codecs_v3(path, z.get("codecs"), &chunks, true));
    let encoding = z.pointer("/chunk_key_encoding/name").and_then(Value::as_str).unwrap_or("default");
    let sep = z.pointer("/chunk_key_encoding/configuration/separator").and_then(Value::as_str);
    let (key_prefix, separator) = if encoding == "v2" {
        (String::new(), if sep == Some("/") { '/' } else { '.' })
    } else {
        ("c/".to_string(), if sep == Some(".") { '.' } else { '/' })
    };
    Some(Array {
        path: path.to_string(),
        zarr_format: 3,
        dims: dims_of(z.get("dimension_names")),
        shape,
        chunks,
        dtype,
        fill: fill_of(z.get("fill_value")),
        fortran: false,
        codecs,
        attrs: z.get("attributes").and_then(Value::as_object).cloned().unwrap_or_default(),
        key_prefix,
        separator,
    })
}

// --- Decompression -------------------------------------------------------------------------------

fn inflate(data: &[u8], zlib: bool, want: usize) -> Result<Vec<u8>> {
    let r = if zlib {
        miniz_oxide::inflate::decompress_to_vec_zlib_with_limit(data, want.max(1))
    } else {
        miniz_oxide::inflate::decompress_to_vec_with_limit(data, want.max(1))
    };
    r.map_err(|_| bad("a chunk doesn't inflate"))
}

/// gzip: its header (with the optional parts), then deflate.
fn gunzip(d: &[u8], want: usize) -> Result<Vec<u8>> {
    if d.get(..3) != Some(&[0x1f, 0x8b, 8]) {
        return Err(bad("a chunk isn't gzip"));
    }
    let flags = d.get(3).copied().unwrap_or(0);
    let mut at = 10usize;
    if flags & 4 != 0 {
        let n = d.get(at..at + 2).map(|b| usize::from(u16::from_le_bytes([b[0], b[1]]))).ok_or_else(|| bad("a cut gzip header"))?;
        at += 2 + n;
    }
    for bit in [8u8, 16] {
        if flags & bit != 0 {
            at += d.get(at..).and_then(|r| r.iter().position(|&b| b == 0)).ok_or_else(|| bad("a cut gzip header"))? + 1;
        }
    }
    if flags & 2 != 0 {
        at += 2;
    }
    inflate(d.get(at..).ok_or_else(|| bad("a cut gzip header"))?, false, want)
}

fn unzstd(d: &[u8], want: usize) -> Result<Vec<u8>> {
    use std::io::Read;
    let dec = ruzstd::decoding::StreamingDecoder::new(d).map_err(|e| bad(format!("a zstd chunk: {e}")))?;
    let mut out = Vec::with_capacity(want.min(64 << 20));
    dec.take(want as u64 + 1).read_to_end(&mut out).map_err(|e| bad(format!("a zstd chunk: {e}")))?;
    Ok(out)
}

fn numcodecs_lz4(d: &[u8], want: usize) -> Result<Vec<u8>> {
    let n = d.get(..4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize).ok_or_else(|| bad("a cut LZ4 chunk"))?;
    if n > want.max(1) {
        return Err(bad("an LZ4 chunk larger than its array's chunks"));
    }
    let mut out = vec![0u8; n];
    let got = lz4_flex::block::decompress_into(d.get(4..).unwrap_or_default(), &mut out).map_err(|e| bad(format!("an LZ4 chunk: {e}")))?;
    out.truncate(got);
    Ok(out)
}

/// Byte unshuffle: element `j`'s byte `i` was stored at `i * n + j`.
fn unshuffle(d: &[u8], size: usize) -> Vec<u8> {
    if size <= 1 {
        return d.to_vec();
    }
    let n = d.len() / size;
    let mut out = vec![0u8; d.len()];
    for i in 0..size {
        for j in 0..n {
            if let (Some(o), Some(v)) = (out.get_mut(j * size + i), d.get(i * n + j)) {
                *o = *v;
            }
        }
    }
    // Bytes past the last whole element are as they were.
    let rest = n * size;
    if let (Some(o), Some(v)) = (out.get_mut(rest..), d.get(rest..)) {
        o.copy_from_slice(v);
    }
    out
}

/// A blosc (version 1) frame: a 16-byte header, then blocks compressed with LZ4, zlib or zstd,
/// each byte-shuffled and possibly split into a stream per byte of the type.
fn blosc(d: &[u8], want: usize) -> Result<Vec<u8>> {
    let h = d.get(..16).ok_or_else(|| bad("a cut blosc chunk"))?;
    let flags = h[2];
    let typesize = usize::from(h[3]).max(1);
    let le = |o: usize| u32::from_le_bytes([h[o], h[o + 1], h[o + 2], h[o + 3]]) as usize;
    let (nbytes, blocksize) = (le(4), le(8));
    if nbytes > want.max(1) {
        return Err(bad("a blosc chunk larger than its array's chunks"));
    }
    // Stored as it is.
    if flags & 0x2 != 0 {
        return Ok(d.get(16..16 + nbytes).ok_or_else(|| bad("a cut blosc chunk"))?.to_vec());
    }
    if blocksize == 0 || nbytes == 0 {
        return Ok(Vec::new());
    }
    let codec = flags >> 5;
    let nblocks = nbytes.div_ceil(blocksize);
    let mut out = Vec::with_capacity(nbytes);
    for b in 0..nblocks {
        let start = d
            .get(16 + 4 * b..20 + 4 * b)
            .map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]) as usize)
            .ok_or_else(|| bad("a cut blosc chunk"))?;
        let bsize = if b + 1 == nblocks { nbytes - b * blocksize } else { blocksize };
        // A stream per byte of the type, unless the block wasn't split; the last, shorter block
        // never is.
        let leftover = bsize < blocksize;
        let split = !leftover && flags & 0x10 == 0 && typesize <= 16 && blocksize / typesize >= 128;
        let streams = if split { typesize } else { 1 };
        let ssize = bsize / streams;
        let mut block = Vec::with_capacity(bsize);
        let mut at = start;
        for s in 0..streams {
            let this = if s + 1 == streams { bsize - ssize * (streams - 1) } else { ssize };
            let cbytes = d.get(at..at + 4).map(|s| i32::from_le_bytes([s[0], s[1], s[2], s[3]])).ok_or_else(|| bad("a cut blosc chunk"))?;
            at += 4;
            let cb = usize::try_from(cbytes).map_err(|_| bad("a blosc stream of special values isn't read"))?;
            let src = d.get(at..at + cb).ok_or_else(|| bad("a cut blosc chunk"))?;
            at += cb;
            if cb == this {
                block.extend_from_slice(src);
                continue;
            }
            let part = match codec {
                1 => {
                    let mut o = vec![0u8; this];
                    let n = lz4_flex::block::decompress_into(src, &mut o).map_err(|e| bad(format!("a blosc LZ4 block: {e}")))?;
                    o.truncate(n);
                    o
                }
                3 => inflate(src, true, this)?,
                4 => unzstd(src, this)?,
                0 => return Err(bad("blosc's own codec (blosclz) isn't read: use LZ4, zlib or zstd")),
                2 => return Err(bad("snappy blosc chunks aren't read")),
                c => return Err(bad(format!("blosc codec {c} isn't read"))),
            };
            block.extend_from_slice(part.get(..this.min(part.len())).unwrap_or_default());
        }
        if flags & 0x4 != 0 {
            block = bitunshuffle(&block, typesize, h[0]);
        } else if flags & 0x1 != 0 && typesize > 1 {
            block = unshuffle(&block, typesize);
        }
        out.extend_from_slice(&block);
    }
    Ok(out)
}

/// Bit unshuffle of a blosc block of `size`-byte elements: bit `k` of byte `j` of element `i` was
/// stored in bit-plane `8j + k`, at bit `i % 8` of its byte `i / 8`. Blosc format 2 shuffles only
/// blocks of a multiple of 8 elements; later formats shuffle the first multiple of 8 and leave the
/// rest as it was.
fn bitunshuffle(d: &[u8], size: usize, format: u8) -> Vec<u8> {
    let n = d.len() / size.max(1);
    let shuffled = if format <= 2 { if n.is_multiple_of(8) { n } else { 0 } } else { n - n % 8 };
    let mut out = d.to_vec();
    if shuffled == 0 {
        return out;
    }
    let plane = shuffled / 8;
    for (i, o) in out.iter_mut().take(shuffled * size).enumerate() {
        let (e, j) = (i / size, i % size);
        let mut byte = 0u8;
        for k in 0..8 {
            let bit = d.get((j * 8 + k) * plane + e / 8).map_or(0, |b| (b >> (e % 8)) & 1);
            byte |= bit << k;
        }
        *o = byte;
    }
    out
}

/// A shard of `shard` values: its chunks of `inner` values, each decoded with `codecs`, put in
/// place (row-major), with its index of (offset, size) pairs (`index` its encoding) at the end
/// or the start. Chunks it doesn't hold are `fill`. Returns the values' bytes and byte order.
#[allow(clippy::too_many_arguments)]
fn unshard(
    d: &[u8],
    shard: &[u64],
    inner: &[u64],
    codecs: &[Codec],
    index: &[Codec],
    index_at_end: bool,
    dtype: DType,
    fill: Option<f64>,
) -> Result<(Vec<u8>, bool)> {
    let nd = shard.len();
    if inner.len() != nd || inner.contains(&0) {
        return Err(bad("a shard's chunks don't fit it"));
    }
    let grid: Vec<u64> = shard.iter().zip(inner).map(|(&s, &i)| s / i).collect();
    let count = grid
        .iter()
        .try_fold(1u64, |a, &g| a.checked_mul(g))
        .and_then(|n| usize::try_from(n).ok())
        .ok_or_else(|| bad("a shard too large"))?;
    // The index: 16 bytes a chunk, and a checksum after them. It must lie in the file, so its size
    // is bounded by the file's before anything is made for it.
    let checksum = if index.contains(&Codec::Crc32c) { 4 } else { 0 };
    let ilen = count
        .checked_mul(16)
        .and_then(|n| n.checked_add(checksum))
        .filter(|&n| n <= d.len())
        .ok_or_else(|| bad("a shard's index is cut"))?;
    let raw = if index_at_end { d.get(d.len() - ilen..) } else { d.get(..ilen) }.ok_or_else(|| bad("a shard's index is cut"))?;
    let (idx, il) = decode_chunk(index, raw, &[count as u64, 2], DType { kind: Kind::UInt, size: 8, little: true }, None)?;
    let word = |i: usize| -> u64 {
        let b: [u8; 8] = idx.get(i * 8..i * 8 + 8).and_then(|b| b.try_into().ok()).unwrap_or([0xff; 8]);
        if il { u64::from_le_bytes(b) } else { u64::from_be_bytes(b) }
    };
    let little = codecs.iter().find_map(|c| if let Codec::Bytes { little } = c { Some(*little) } else { None }).unwrap_or(dtype.little);
    let size = dtype.size;
    let total = shard
        .iter()
        .try_fold(1u64, |a, &c| a.checked_mul(c))
        .and_then(|n| usize::try_from(n).ok())
        .and_then(|n| n.checked_mul(size))
        .ok_or_else(|| bad("a shard too large"))?;
    let fill_bytes = dtype.encode(fill.unwrap_or(0.0), little);
    let mut out: Vec<u8> = fill_bytes.iter().copied().cycle().take(total).collect();
    // Row-major strides of the shard, in values.
    let mut strides = vec![1u64; nd];
    for k in (0..nd.saturating_sub(1)).rev() {
        strides[k] = strides[k + 1].saturating_mul(shard[k + 1]);
    }
    let row = inner.last().copied().unwrap_or(1) as usize * size;
    let rows = inner.iter().take(nd.saturating_sub(1)).product::<u64>() as usize;
    for c in 0..count {
        let (off, len) = (word(2 * c), word(2 * c + 1));
        if off == u64::MAX && len == u64::MAX {
            continue;
        }
        let src = usize::try_from(off)
            .ok()
            .zip(usize::try_from(len).ok())
            .and_then(|(o, l)| d.get(o..o.checked_add(l)?))
            .ok_or_else(|| bad("a shard's chunk lies outside it"))?;
        let (vals, _) = decode_chunk(codecs, src, inner, dtype, fill)?;
        // The chunk's corner in the shard, then each of its rows (all but the last dimension)
        // copied to where it goes.
        let mut at = vec![0u64; nd];
        let mut rest = c as u64;
        for k in (0..nd).rev() {
            at[k] = rest % grid[k] * inner[k];
            rest /= grid[k];
        }
        for r in 0..rows {
            let mut rest = r as u64;
            let mut start = at.last().copied().unwrap_or(0);
            for k in (0..nd.saturating_sub(1)).rev() {
                start += (at[k] + rest % inner[k]) * strides[k];
                rest /= inner[k];
            }
            let to = start as usize * size;
            if let (Some(o), Some(v)) = (out.get_mut(to..to + row), vals.get(r * row..r * row + row)) {
                o.copy_from_slice(v);
            }
        }
    }
    Ok((out, little))
}

// --- Slices --------------------------------------------------------------------------------------

/// Which slice of which variable: its other dimensions' indexes (0 when not given).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Selection {
    pub variable: String,
    pub index: Vec<(String, u64)>,
}

impl Selection {
    /// `t2m[time=3,level=0]`, as projects save it.
    pub fn label(&self) -> String {
        if self.index.is_empty() {
            return self.variable.clone();
        }
        let parts: Vec<String> = self.index.iter().map(|(d, i)| format!("{d}={i}")).collect();
        format!("{}[{}]", self.variable, parts.join(","))
    }

    pub fn parse(s: &str) -> Selection {
        let s = s.trim();
        match s.split_once('[') {
            Some((v, rest)) => Selection {
                variable: v.to_string(),
                index: rest
                    .trim_end_matches(']')
                    .split(',')
                    .filter_map(|kv| {
                        let (k, v) = kv.split_once('=')?;
                        Some((k.trim().to_string(), v.trim().parse().ok()?))
                    })
                    .take(MAX_DIMS)
                    .collect(),
            },
            None => Selection { variable: s.to_string(), index: Vec::new() },
        }
    }
}

/// What a slice needs: the chunk files to fetch, and how they make the raster.
#[derive(Clone, Debug)]
pub struct Plan {
    pub keys: Vec<String>,
    var: String,
    y: usize,
    x: usize,
    fixed: Vec<u64>,
    ycoord: Option<String>,
    xcoord: Option<String>,
}

fn is_y(d: &str) -> bool {
    matches!(d.to_ascii_lowercase().as_str(), "lat" | "latitude" | "y" | "rlat" | "nav_lat" | "south_north" | "j" | "yc" | "northing")
}

fn is_x(d: &str) -> bool {
    matches!(d.to_ascii_lowercase().as_str(), "lon" | "longitude" | "x" | "rlon" | "nav_lon" | "west_east" | "i" | "xc" | "easting")
}

/// The dimensions that make the map: those named like y and x, else the last two.
fn map_dims(a: &Array) -> (usize, usize) {
    let names = a.dim_names();
    let y = names.iter().rposition(|d| is_y(d));
    let x = names.iter().rposition(|d| is_x(d));
    match (y, x) {
        (Some(y), Some(x)) if y != x => (y, x),
        _ => (a.shape.len().saturating_sub(2), a.shape.len().saturating_sub(1)),
    }
}

/// The chunk keys of all of a 1D array.
fn all_keys(a: &Array) -> Vec<String> {
    (0..a.grid(0).min(MAX_CHUNKS as u64)).map(|i| a.chunk_key(&[i])).collect()
}

/// The chunks a slice needs.
pub fn plan(store: &Store, sel: &Selection) -> Result<Plan> {
    let a = store.array(&sel.variable).ok_or_else(|| bad(format!("no variable named {}", sel.variable)))?;
    if let Some(why) = a.unsupported() {
        return Err(bad(why));
    }
    if a.shape.len() < 2 {
        return Err(bad(format!("{} has fewer than two dimensions", a.path)));
    }
    let names = a.dim_names();
    let (y, x) = map_dims(a);
    let (h, w) = (a.shape.get(y).copied().unwrap_or(0), a.shape.get(x).copied().unwrap_or(0));
    let cells = h.checked_mul(w).ok_or_else(|| bad("too large"))?;
    if cells == 0 || cells > MAX_SAMPLES as u64 {
        return Err(bad(format!("{} is {w} × {h}, more than a raster holds", a.path)));
    }
    a.chunk_values()?;
    let mut fixed = vec![0u64; a.shape.len()];
    for (d, i) in &sel.index {
        let Some(k) = names.iter().position(|n| n == d) else { return Err(bad(format!("{} has no dimension {d}", a.path))) };
        if k != y && k != x {
            if *i >= a.shape.get(k).copied().unwrap_or(0) {
                return Err(bad(format!("{d} has {} steps; {i} is past them", a.shape.get(k).copied().unwrap_or(0))));
            }
            if let Some(f) = fixed.get_mut(k) {
                *f = *i;
            }
        }
    }
    let (gy, gx) = (a.grid(y), a.grid(x));
    if gy.saturating_mul(gx) > MAX_CHUNKS as u64 {
        return Err(bad(format!("{} needs {} chunks for one slice, more than {MAX_CHUNKS}", a.path, gy.saturating_mul(gx))));
    }
    let mut keys = Vec::new();
    for cy in 0..gy {
        for cx in 0..gx {
            let idx: Vec<u64> = (0..a.shape.len())
                .map(|d| {
                    if d == y {
                        cy
                    } else if d == x {
                        cx
                    } else {
                        fixed.get(d).copied().unwrap_or(0) / a.chunks.get(d).copied().unwrap_or(1).max(1)
                    }
                })
                .collect();
            keys.push(a.chunk_key(&idx));
        }
    }
    let coord = |d: usize| names.get(d).and_then(|n| store.coordinate(a, n)).filter(|c| c.shape.first() == a.shape.get(d));
    let (yc, xc) = (coord(y), coord(x));
    for c in [yc, xc].into_iter().flatten() {
        keys.extend(all_keys(c));
    }
    Ok(Plan { keys, var: a.path.clone(), y, x, fixed, ycoord: yc.map(|c| c.path.clone()), xcoord: xc.map(|c| c.path.clone()) })
}

/// A 1D array's values, from its chunks (missing ones are its fill value).
fn values_1d(a: &Array, chunks: &HashMap<String, Vec<u8>>) -> Result<Vec<f64>> {
    let n = a.shape.first().copied().unwrap_or(0);
    if n > MAX_COORDS {
        return Err(bad(format!("{} is too long", a.path)));
    }
    let c = a.chunks.first().copied().unwrap_or(1).max(1);
    let mut out = Vec::with_capacity(n as usize);
    for i in 0..a.grid(0) {
        let take = (n - i * c).min(c) as usize;
        match chunks.get(&a.chunk_key(&[i])) {
            Some(b) => out.extend(a.decode(b)?.into_iter().take(take)),
            None => out.extend(std::iter::repeat_n(a.fill.unwrap_or(f64::NAN), take)),
        }
    }
    Ok(out)
}

/// A number attribute (`scale_factor`, `_FillValue`).
fn attr_f64(a: &Array, k: &str) -> Option<f64> {
    match a.attrs.get(k)? {
        Value::Number(n) => n.as_f64(),
        Value::Array(v) => v.first().and_then(Value::as_f64),
        Value::String(s) => s.parse().ok(),
        _ => None,
    }
}

fn attr_str<'a>(a: &'a Array, k: &str) -> Option<&'a str> {
    a.attrs.get(k).and_then(Value::as_str)
}

/// Whether a coordinate array is longitude/latitude, by its name, units or standard name.
fn geographic(c: &Array) -> bool {
    let units = attr_str(c, "units").unwrap_or("").to_ascii_lowercase();
    let std = attr_str(c, "standard_name").unwrap_or("").to_ascii_lowercase();
    units.starts_with("degree")
        || matches!(std.as_str(), "latitude" | "longitude")
        || matches!(c.name().to_ascii_lowercase().as_str(), "lat" | "lon" | "latitude" | "longitude")
}

/// The coordinate system a variable's attributes (or its grid mapping's) say.
fn crs_of(store: &Store, a: &Array) -> Option<Crs> {
    let from = |x: &Array| {
        for k in ["crs_wkt", "spatial_ref", "crs", "proj4", "proj4text", "proj4_params"] {
            if let Some(t) = attr_str(x, k).and_then(|t| Crs::parse(t).ok()) {
                return Some(t);
            }
        }
        for k in ["epsg_code", "epsg"] {
            let code = match x.attrs.get(k) {
                Some(Value::Number(n)) => n.as_u64(),
                Some(Value::String(s)) => s.trim_start_matches("EPSG:").parse().ok(),
                _ => None,
            };
            if let Some(c) = code.and_then(|c| u32::try_from(c).ok()).and_then(Crs::from_epsg) {
                return Some(c);
            }
        }
        None
    };
    from(a).or_else(|| attr_str(a, "grid_mapping").and_then(|g| store.coordinate(a, g).or_else(|| store.array(g))).and_then(from))
}

/// One axis from its coordinates: the first cell's outer edge and the cell size, if the
/// coordinates are evenly spaced (to within a fifth of a cell).
fn axis(v: &[f64]) -> Option<(f64, f64)> {
    let (first, last) = (*v.first()?, *v.last()?);
    if v.len() < 2 {
        return None;
    }
    let step = (last - first) / (v.len() - 1) as f64;
    if !step.is_finite() || step == 0.0 {
        return None;
    }
    let even = v.iter().enumerate().all(|(i, c)| c.is_finite() && (c - (first + step * i as f64)).abs() <= step.abs() * 0.2);
    even.then_some((first - step / 2.0, step))
}

/// Decodes the chunks a [`plan`] named (missing ones are the fill value) into a raster.
/// `crs` is used when the store doesn't say its coordinate system.
pub fn assemble(store: &Store, plan: &Plan, chunks: &HashMap<String, Vec<u8>>, name: &str, crs: Option<Crs>) -> Result<Raster> {
    let a = store.array(&plan.var).ok_or_else(|| bad("the variable is gone"))?;
    let (h, w) = (a.shape.get(plan.y).copied().unwrap_or(0) as usize, a.shape.get(plan.x).copied().unwrap_or(0) as usize);
    let mut grid = vec![f32::NAN; h * w];
    let (cy, cx) = (a.chunks.get(plan.y).copied().unwrap_or(1).max(1), a.chunks.get(plan.x).copied().unwrap_or(1).max(1));
    let nd = a.shape.len();
    // Values meaning "no data", before any scaling, as xarray reads them: the CF attributes, and a
    // Zarr 2 array's fill_value (xarray's _FillValue encoding). A Zarr 3 array's fill_value (0 by
    // default in zarr-python) only stands for chunks that were never written, and those cells stay
    // empty here; real zeros are kept.
    let v2_fill = if a.zarr_format == 2 { a.fill } else { None };
    let mut missing: Vec<f64> = [v2_fill, attr_f64(a, "_FillValue"), attr_f64(a, "missing_value")].into_iter().flatten().collect();
    missing.dedup();
    let scale = attr_f64(a, "scale_factor").unwrap_or(1.0);
    let offset = attr_f64(a, "add_offset").unwrap_or(0.0);
    let mut found = 0usize;
    for gy in 0..a.grid(plan.y) {
        for gx in 0..a.grid(plan.x) {
            let idx: Vec<u64> = (0..nd)
                .map(|d| match d {
                    d if d == plan.y => gy,
                    d if d == plan.x => gx,
                    d => plan.fixed.get(d).copied().unwrap_or(0) / a.chunks.get(d).copied().unwrap_or(1).max(1),
                })
                .collect();
            let Some(bytes) = chunks.get(&a.chunk_key(&idx)) else { continue };
            found += 1;
            let (vals, dt) = a.decode_bytes(bytes)?;
            // Each value's position in the chunk, row-major (or column-major).
            let local: Vec<u64> =
                (0..nd).map(|d| plan.fixed.get(d).copied().unwrap_or(0) % a.chunks.get(d).copied().unwrap_or(1).max(1)).collect();
            let strides: Vec<u64> = {
                let mut s = vec![1u64; nd];
                if a.fortran {
                    for d in 1..nd {
                        s[d] = s[d - 1] * a.chunks[d - 1];
                    }
                } else {
                    for d in (0..nd.saturating_sub(1)).rev() {
                        s[d] = s[d + 1] * a.chunks[d + 1];
                    }
                }
                s
            };
            let base: u64 = (0..nd).filter(|d| *d != plan.y && *d != plan.x).map(|d| local[d] * strides[d]).sum();
            for ly in 0..cy {
                let row = gy * cy + ly;
                if row as usize >= h {
                    break;
                }
                for lx in 0..cx {
                    let col = gx * cx + lx;
                    if col as usize >= w {
                        break;
                    }
                    let i = base + ly * strides[plan.y] + lx * strides[plan.x];
                    let at = usize::try_from(i).ok().and_then(|i| i.checked_mul(dt.size));
                    let Some(v) = at.and_then(|at| vals.get(at..at + dt.size)).map(|b| dt.value(b)) else { continue };
                    let out = if v.is_nan() || missing.contains(&v) { f32::NAN } else { (v * scale + offset) as f32 };
                    if let Some(g) = grid.get_mut(row as usize * w + col as usize) {
                        *g = out;
                    }
                }
            }
        }
    }
    if found == 0 {
        return Err(bad(format!("{}: none of the slice's chunks could be read", a.path)));
    }
    // Georeferencing from the coordinate arrays.
    let coords = |p: &Option<String>| -> Result<Option<(Vec<f64>, &Array)>> {
        match p.as_ref().and_then(|p| store.array(p)) {
            Some(c) => Ok(Some((values_1d(c, chunks)?, c))),
            None => Ok(None),
        }
    };
    let (ys, xs) = (coords(&plan.ycoord)?, coords(&plan.xcoord)?);
    let (Some((ys, yc)), Some((xs, xc))) = (ys, xs) else {
        return Err(bad(format!("{}: its y and x dimensions have no coordinates, so it can't be placed on the map", a.path)));
    };
    let ((mut y0, mut dy), (mut x0, mut dx)) = match (axis(&ys), axis(&xs)) {
        (Some(y), Some(x)) => (y, x),
        _ => return Err(bad(format!("{}: its coordinates aren't evenly spaced (a curvilinear or irregular grid isn't read)", a.path))),
    };
    // North up and west to east: flip rows (or columns) as needed.
    if dy > 0.0 {
        let mut flipped = Vec::with_capacity(grid.len());
        for r in (0..h).rev() {
            flipped.extend_from_slice(grid.get(r * w..(r + 1) * w).unwrap_or_default());
        }
        grid = flipped;
        y0 += dy * h as f64;
        dy = -dy;
    }
    if dx < 0.0 {
        for r in 0..h {
            if let Some(row) = grid.get_mut(r * w..(r + 1) * w) {
                row.reverse();
            }
        }
        x0 += dx * w as f64;
        dx = -dx;
    }
    let lonlat = geographic(yc) && geographic(xc);
    let crs = if lonlat { Some(Crs::LonLat) } else { crs_of(store, a).or(crs) };
    let Some(crs) = crs else {
        return Err(bad(format!("{}: the store doesn't say its coordinate system; give \"crs\" (such as EPSG:3035)", a.path)));
    };
    // Longitudes of 0–360: a global grid is turned to start at -180; one wholly east of 180 is
    // moved by 360.
    if crs == Crs::LonLat && x0 + dx * w as f64 > 180.5 {
        if x0 >= 180.0 {
            x0 -= 360.0;
        } else if dx * w as f64 >= 359.0 && x0 > -1.0 - dx {
            let split = (((180.0 - x0) / dx).round().max(0.0) as usize).min(w);
            for r in 0..h {
                if let Some(row) = grid.get_mut(r * w..(r + 1) * w) {
                    row.rotate_left(split);
                }
            }
            x0 += dx * split as f64 - 360.0;
        }
    }
    let transform = GeoTransform { origin_x: x0, origin_y: y0, pixel_width: dx, pixel_height: dy };
    let data_type = match (a.dtype.kind, a.dtype.size) {
        (Kind::Float, 8) => "Float64",
        (Kind::Float, _) => "Float32",
        (Kind::Int, 1) => "Int8",
        (Kind::Int, 2) => "Int16",
        (Kind::Int, _) => "Int32",
        (Kind::UInt, 1) | (Kind::Bool, _) => "UInt8",
        (Kind::UInt, 2) => "UInt16",
        (Kind::UInt, _) => "UInt32",
    };
    let data_type = if scale != 1.0 || offset != 0.0 { "Float32" } else { data_type };
    Raster::new(name, w as u32, h as u32, vec![grid], transform, crs, None, ColorHint::Gray, data_type)
}

// --- What a store holds --------------------------------------------------------------------------

/// The coordinate arrays [`info`] labels dimensions with (time, level): their chunk keys.
pub fn info_keys(store: &Store) -> Vec<String> {
    let mut keys = Vec::new();
    for v in store.variables() {
        let (y, x) = map_dims(v);
        for (d, name) in v.dim_names().iter().enumerate() {
            if d == y || d == x {
                continue;
            }
            if let Some(c) =
                store.coordinate(v, name).filter(|c| c.shape.first().is_some_and(|n| *n <= MAX_COORDS) && c.unsupported().is_none())
            {
                for k in all_keys(c) {
                    if !keys.contains(&k) {
                        keys.push(k);
                    }
                }
            }
        }
    }
    keys
}

/// A coordinate value as a label: a CF time (`days since 1850-01-01`) as a date, else the
/// number and units.
fn label(c: &Array, v: f64) -> String {
    let units = attr_str(c, "units").unwrap_or("");
    let calendar = attr_str(c, "calendar").unwrap_or("standard").to_ascii_lowercase();
    if let Some((unit, since)) = units.split_once(" since ") {
        let secs = match unit.trim().to_ascii_lowercase().as_str() {
            "seconds" | "second" | "s" => Some(1.0),
            "minutes" | "minute" | "min" => Some(60.0),
            "hours" | "hour" | "h" => Some(3600.0),
            "days" | "day" | "d" => Some(86_400.0),
            _ => None,
        };
        let since = since.trim();
        if let Some(s) = secs.filter(|_| v.is_finite() && v.abs() < 1e13) {
            match calendar.as_str() {
                "standard" | "gregorian" | "proleptic_gregorian" => {
                    if let Some(base) = epoch_seconds(since) {
                        // `as` saturates rather than wraps.
                        return date_label((base + v * s) as i64);
                    }
                }
                // Model calendars: years of fixed length.
                "noleap" | "365_day" | "all_leap" | "366_day" | "360_day" => {
                    if let Some(l) = fixed_calendar(&calendar, since, v * s) {
                        return l;
                    }
                }
                _ => {}
            }
        }
    }
    let n = if v.fract() == 0.0 && v.abs() < 1e15 { format!("{}", v as i64) } else { format!("{v}") };
    if units.is_empty() { n } else { format!("{n} {units}") }
}

/// A date in a model calendar of fixed-length years (365, 366 or 360 days): `secs` after `since`.
fn fixed_calendar(calendar: &str, since: &str, secs: f64) -> Option<String> {
    let mut d = since.split([' ', 'T']).next()?.splitn(3, '-').map(|p| p.trim().parse::<i64>().ok());
    // The parts of a hostile `units` string are kept to sane ranges, so nothing below overflows.
    let (y0, m0, d0) =
        (d.next()??.clamp(-MAX_YEAR, MAX_YEAR), d.next().flatten().unwrap_or(1).clamp(1, 12), d.next().flatten().unwrap_or(1).clamp(1, 31));
    let months: [i64; 12] = match calendar {
        "360_day" => [30; 12],
        "all_leap" | "366_day" => [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31],
        _ => [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31],
    };
    let year: i64 = months.iter().sum();
    let start = months.iter().take((m0 - 1) as usize).sum::<i64>() + d0 - 1;
    // `as` saturates a float past i64's range; the sums saturate too.
    let total = ((secs / 86_400.0).floor() as i64).saturating_add(start);
    let (y, mut doy) = (y0.saturating_add(total.div_euclid(year)), total.rem_euclid(year));
    let mut m = 0usize;
    while m < 11 && doy >= months[m] {
        doy -= months[m];
        m += 1;
    }
    Some(format!("{y:04}-{:02}-{:02}", m + 1, doy + 1))
}

/// Seconds since 1970 of `YYYY-MM-DD[ T]HH:MM:SS`.
fn epoch_seconds(s: &str) -> Option<f64> {
    let (date, time) = s.split_once([' ', 'T']).unwrap_or((s, "0:0:0"));
    let mut d = date.splitn(3, '-').map(|p| p.trim().parse::<i64>().ok());
    let (y, m, day) =
        (d.next()??.clamp(-MAX_YEAR, MAX_YEAR), d.next().flatten().unwrap_or(1).clamp(1, 12), d.next().flatten().unwrap_or(1).clamp(1, 31));
    let mut t = time.trim().trim_end_matches('Z').split(':').map(|p| p.trim().parse::<f64>().ok());
    let (hh, mm, ss) = (t.next().flatten().unwrap_or(0.0), t.next().flatten().unwrap_or(0.0), t.next().flatten().unwrap_or(0.0));
    Some(days_from_civil(y, m, day) as f64 * 86_400.0 + hh * 3600.0 + mm * 60.0 + ss)
}

/// The years a `since` date may name: far past any data, small enough that day counts can't
/// overflow.
const MAX_YEAR: i64 = 1_000_000;

fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn date_label(t: i64) -> String {
    let (days, secs) = (t.div_euclid(86_400), t.rem_euclid(86_400));
    let z = days.saturating_add(719_468);
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    if secs == 0 { format!("{y:04}-{m:02}-{d:02}") } else { format!("{y:04}-{m:02}-{d:02} {:02}:{:02}", secs / 3600, secs % 3600 / 60) }
}

/// Labels for many values: at most this many are listed (the dialog shows one at a time); longer
/// dimensions give their first and last.
const MAX_LABELS: usize = 100_000;

/// What a store holds: its variables with their dimensions (sizes, and labels where the
/// coordinates were fetched: `chunks` from [`info_keys`]).
pub fn info(store: &Store, chunks: &HashMap<String, Vec<u8>>) -> Value {
    let vars: Vec<Value> = store
        .variables()
        .iter()
        .map(|v| {
            let (y, x) = map_dims(v);
            let dims: Vec<Value> = v
                .dim_names()
                .iter()
                .enumerate()
                .map(|(d, name)| {
                    let size = v.shape.get(d).copied().unwrap_or(0);
                    let mut o = serde_json::json!({"name": name, "size": size, "map": d == y || d == x});
                    if d != y
                        && d != x
                        && let Some(c) = store.coordinate(v, name)
                        && let Ok(vals) = values_1d(c, chunks)
                    {
                        // Long ones (hourly over decades) give their ends; dim_label gives any step.
                        if vals.len() <= MAX_LABELS {
                            let labels: Vec<String> = vals.iter().map(|x| label(c, *x)).collect();
                            o["labels"] = serde_json::json!(labels);
                        } else if let (Some(a), Some(b)) = (vals.first(), vals.last()) {
                            o["first"] = serde_json::json!(label(c, *a));
                            o["last"] = serde_json::json!(label(c, *b));
                        }
                    }
                    o
                })
                .collect();
            serde_json::json!({
                "name": v.path,
                "longName": attr_str(v, "long_name").or_else(|| attr_str(v, "standard_name")),
                "units": attr_str(v, "units"),
                "dtype": format!("{:?}{}", v.dtype.kind, v.dtype.size * 8),
                "dims": dims,
                "readable": v.unsupported().is_none(),
                "why": v.unsupported(),
            })
        })
        .collect();
    serde_json::json!({"count": vars.len(), "variables": vars})
}

// --- A folder on this computer -------------------------------------------------------------------

/// Whether a folder is a Zarr store (or one of its arrays).
#[cfg(not(target_arch = "wasm32"))]
pub fn is_store(dir: &std::path::Path) -> bool {
    dir.is_dir() && ["zarr.json", ".zmetadata", ".zarray", ".zgroup"].iter().any(|k| dir.join(k).is_file())
}

/// A folder's metadata: consolidated, else each array's files (up to four levels deep).
#[cfg(not(target_arch = "wasm32"))]
pub fn read_metadata(dir: &std::path::Path) -> Result<Store> {
    fn read(docs: &mut Vec<(String, Vec<u8>)>, rel: String, p: std::path::PathBuf) -> Result<()> {
        if p.metadata().map(|m| m.len()).unwrap_or(0) as usize > MAX_METADATA {
            return Err(bad(format!("{} is too large", p.display())));
        }
        if let Ok(b) = std::fs::read(&p) {
            docs.push((rel, b));
        }
        Ok(())
    }
    let mut docs = Vec::new();
    for k in METADATA_KEYS {
        if dir.join(k).is_file() {
            read(&mut docs, k.to_string(), dir.join(k))?;
        }
    }
    // Consolidated: a .zmetadata, or a root zarr.json whose consolidated_metadata is an object
    // (zarr-python 3 writes `"consolidated_metadata": null` for a store that isn't).
    let consolidated = docs.iter().any(|(k, b)| {
        k == ".zmetadata"
            || (k == "zarr.json"
                && serde_json::from_slice::<Value>(b).ok().is_some_and(|v| v.get("consolidated_metadata").is_some_and(Value::is_object)))
    });
    if !consolidated {
        let mut stack = vec![(dir.to_path_buf(), String::new(), 0usize)];
        let mut seen = 0usize;
        while let Some((d, rel, depth)) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&d) else { continue };
            for e in entries.flatten().take(MAX_ARRAYS) {
                seen += 1;
                if seen > 4 * MAX_ARRAYS {
                    break;
                }
                let p = e.path();
                if !p.is_dir() || depth >= 4 {
                    continue;
                }
                let name = e.file_name().to_string_lossy().to_string();
                let r = if rel.is_empty() { name } else { format!("{rel}/{name}") };
                for k in [".zarray", ".zattrs", "zarr.json"] {
                    if p.join(k).is_file() {
                        read(&mut docs, format!("{r}/{k}"), p.join(k))?;
                    }
                }
                if p.join(".zgroup").is_file() || p.join("zarr.json").is_file() {
                    stack.push((p, r, depth + 1));
                }
            }
        }
    }
    parse_store(&docs)
}

/// Reads a slice of a store in a folder (`selection` as [`Selection::label`] writes it).
#[cfg(not(target_arch = "wasm32"))]
pub fn read_dir(dir: &std::path::Path, selection: &str, crs: Option<&str>) -> Result<Raster> {
    let store = read_metadata(dir)?;
    let mut sel = Selection::parse(selection);
    if sel.variable.is_empty() {
        sel.variable = store
            .variables()
            .iter()
            .find(|v| v.unsupported().is_none())
            .map(|v| v.path.clone())
            .ok_or_else(|| bad("the store has no variable that can be read"))?;
    }
    let p = plan(&store, &sel)?;
    let mut chunks = HashMap::new();
    let mut total = 0u64;
    for k in &p.keys {
        let path = dir.join(k);
        let Ok(meta) = std::fs::metadata(&path) else { continue };
        total = total.saturating_add(meta.len());
        if !meta.is_file() || meta.len() > MAX_CHUNK_FILE || total > MAX_READ {
            return Err(bad(format!("{}: the slice's chunks are too large to read", dir.display())));
        }
        chunks.insert(k.clone(), std::fs::read(&path)?);
    }
    let crs = crs.map(Crs::parse).transpose().map_err(DataError::Format)?;
    let stem = dir.file_stem().and_then(|s| s.to_str()).unwrap_or("zarr");
    let mut r = assemble(&store, &p, &chunks, &format!("{stem} {}", sel.label()), crs)?;
    r.source = Some(dir.display().to_string());
    Ok(r)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lz4_block(raw: &[u8]) -> Vec<u8> {
        lz4_flex::block::compress(raw)
    }

    /// A blosc frame of one block, LZ4, byte-shuffled.
    fn blosc_frame(raw: &[u8], typesize: u8) -> Vec<u8> {
        let shuffled = {
            let n = raw.len() / typesize as usize;
            let mut o = vec![0u8; raw.len()];
            for j in 0..n {
                for i in 0..typesize as usize {
                    o[i * n + j] = raw[j * typesize as usize + i];
                }
            }
            o
        };
        // Not split (flag 0x10), so one stream.
        let c = lz4_block(&shuffled);
        let mut f = vec![2u8, 1, 0x1 | 0x10 | (1 << 5), typesize];
        f.extend((raw.len() as u32).to_le_bytes());
        f.extend((raw.len() as u32).to_le_bytes());
        f.extend(((16 + 4 + 4 + c.len()) as u32).to_le_bytes());
        f.extend(20u32.to_le_bytes());
        f.extend((c.len() as i32).to_le_bytes());
        f.extend(c);
        f
    }

    /// A blosc frame as c-blosc writes LZ4 with byte shuffle: full blocks split into a stream per
    /// byte of the type (no 0x10 flag), the last, shorter block in one stream.
    fn blosc_blocks(raw: &[u8], typesize: usize, blocksize: usize) -> Vec<u8> {
        let shuffle = |b: &[u8]| {
            let n = b.len() / typesize;
            let mut o = b.to_vec();
            for j in 0..n {
                for i in 0..typesize {
                    o[i * n + j] = b[j * typesize + i];
                }
            }
            o
        };
        let blocks: Vec<&[u8]> = raw.chunks(blocksize).collect();
        let mut body = Vec::new();
        let mut starts = Vec::new();
        let header = 16 + 4 * blocks.len();
        for b in &blocks {
            starts.push((header + body.len()) as u32);
            let s = shuffle(b);
            let streams = if b.len() == blocksize { typesize } else { 1 };
            for part in s.chunks(s.len() / streams) {
                let c = lz4_block(part);
                body.extend((c.len() as i32).to_le_bytes());
                body.extend(c);
            }
        }
        let mut f = vec![2u8, 1, 0x1 | (1 << 5), typesize as u8];
        f.extend((raw.len() as u32).to_le_bytes());
        f.extend((blocksize as u32).to_le_bytes());
        f.extend(((header + body.len()) as u32).to_le_bytes());
        for s in starts {
            f.extend(s.to_le_bytes());
        }
        f.extend(body);
        f
    }

    #[test]
    fn blosc_split_blocks_and_a_leftover() {
        // Two full blocks split in four streams, then a shorter one that isn't split.
        let raw: Vec<u8> = (0..2700u32).flat_map(|v| ((v % 97) as f32 * 1.5).to_le_bytes()).collect();
        let f = blosc_blocks(&raw, 4, 4096);
        assert_eq!(blosc(&f, raw.len()).unwrap(), raw);
    }

    fn f32s(v: &[f32]) -> Vec<u8> {
        v.iter().flat_map(|x| x.to_le_bytes()).collect()
    }

    /// A consolidated Zarr 2 store: `t(time=2, lat=2, lon=4)` in chunks of (1, 2, 2), with lat, lon
    /// and time coordinates, blosc-LZ4 compressed.
    fn store_v2() -> (Store, HashMap<String, Vec<u8>>) {
        let arr = |shape: &str, chunks: &str, dtype: &str, comp: &str| {
            serde_json::from_str::<Value>(&format!(
                r#"{{"shape":{shape},"chunks":{chunks},"dtype":"{dtype}","compressor":{comp},"fill_value":"NaN","filters":null,"order":"C","zarr_format":2}}"#
            ))
            .unwrap()
        };
        let blosc = r#"{"id":"blosc","cname":"lz4","clevel":5,"shuffle":1,"blocksize":0}"#;
        let meta = serde_json::json!({"metadata": {
            ".zgroup": {"zarr_format": 2},
            "t/.zarray": arr("[2,2,4]", "[1,2,2]", "<f4", blosc),
            "t/.zattrs": {"_ARRAY_DIMENSIONS": ["time","lat","lon"], "units": "K", "long_name": "temperature"},
            "lat/.zarray": arr("[2]", "[2]", "<f8", "null"),
            "lat/.zattrs": {"_ARRAY_DIMENSIONS": ["lat"], "units": "degrees_north"},
            "lon/.zarray": arr("[4]", "[4]", "<f8", r#"{"id":"zlib","level":1}"#),
            "lon/.zattrs": {"_ARRAY_DIMENSIONS": ["lon"], "units": "degrees_east"},
            "time/.zarray": arr("[2]", "[2]", "<i8", "null"),
            "time/.zattrs": {"_ARRAY_DIMENSIONS": ["time"], "units": "hours since 2000-01-01", "calendar": "proleptic_gregorian"},
        }, "zarr_consolidated_format": 1});
        let store = parse_store(&[(".zmetadata".into(), meta.to_string().into_bytes())]).unwrap();
        let mut chunks = HashMap::new();
        // Latitude ascending (south to north), longitude 0–360 (a global grid of 90° cells).
        chunks.insert("lat/0".to_string(), [-45.0f64, 45.0].iter().flat_map(|x| x.to_le_bytes()).collect());
        let lon: Vec<u8> = [45.0f64, 135.0, 225.0, 315.0].iter().flat_map(|x| x.to_le_bytes()).collect();
        chunks.insert("lon/0".to_string(), miniz_oxide::deflate::compress_to_vec_zlib(&lon, 6));
        chunks.insert("time/0".to_string(), [0i64, 30].iter().flat_map(|x| x.to_le_bytes()).collect());
        // time 1: values 100 + 10·row + col over the whole (lat, lon) grid.
        for cx in 0..2u32 {
            let mut v = Vec::new();
            for r in 0..2u32 {
                for c in 0..2u32 {
                    v.push((100 + 10 * r + cx * 2 + c) as f32);
                }
            }
            chunks.insert(format!("t/1.0.{cx}"), blosc_frame(&f32s(&v), 4));
        }
        (store, chunks)
    }

    #[test]
    fn reads_a_slice_of_a_v2_store() {
        let (store, chunks) = store_v2();
        assert_eq!(store.variables().len(), 1);
        let sel = Selection { variable: "t".into(), index: vec![("time".into(), 1)] };
        let p = plan(&store, &sel).unwrap();
        assert!(
            p.keys.contains(&"t/1.0.0".to_string()) && p.keys.contains(&"t/1.0.1".to_string()) && p.keys.contains(&"lat/0".to_string())
        );
        let r = assemble(&store, &p, &chunks, "t", None).unwrap();
        assert_eq!((r.width, r.height, r.crs), (4, 2, Crs::LonLat));
        // North up (rows flipped) and turned to start at -180 (columns from 180° first).
        assert_eq!(r.transform, GeoTransform { origin_x: -180.0, origin_y: 90.0, pixel_width: 90.0, pixel_height: -90.0 });
        let row0: Vec<f32> = r.bands[0].data[..4].to_vec();
        assert_eq!(row0, vec![112.0, 113.0, 110.0, 111.0]);
        // A missing chunk is no data; none at all is an error.
        let mut fewer = chunks.clone();
        fewer.remove("t/1.0.1");
        let r = assemble(&store, &p, &fewer, "t", None).unwrap();
        assert!(r.bands[0].data.iter().filter(|v| v.is_nan()).count() == 4);
        let p0 = plan(&store, &Selection { variable: "t".into(), index: vec![] }).unwrap();
        assert!(assemble(&store, &p0, &chunks, "t", None).is_err());
        // Out of range, unknown names.
        assert!(plan(&store, &Selection { variable: "t".into(), index: vec![("time".into(), 2)] }).is_err());
        assert!(plan(&store, &Selection { variable: "nope".into(), index: vec![] }).is_err());
        assert!(plan(&store, &Selection { variable: "t".into(), index: vec![("depth".into(), 0)] }).is_err());
        // Time labels.
        let info = info(&store, &chunks);
        assert_eq!(info["variables"][0]["dims"][0]["labels"], serde_json::json!(["2000-01-01", "2000-01-02 06:00"]));
        assert_eq!(info["variables"][0]["longName"], "temperature");
    }

    #[test]
    fn v3_metadata_codecs_and_keys() {
        let z = serde_json::json!({
            "zarr_format": 3, "node_type": "array", "shape": [3, 4], "data_type": "int16",
            "chunk_grid": {"name": "regular", "configuration": {"chunk_shape": [3, 4]}},
            "chunk_key_encoding": {"name": "default", "configuration": {"separator": "/"}},
            "codecs": [{"name": "bytes", "configuration": {"endian": "big"}}, {"name": "zstd", "configuration": {"level": 1}}],
            "fill_value": -9999, "dimension_names": ["y", "x"], "attributes": {"epsg_code": 3857, "_FillValue": -9999}
        });
        let root = serde_json::json!({"zarr_format": 3, "node_type": "group", "consolidated_metadata": {"kind": "inline", "metadata": {
            "elev": z,
            "y": {"zarr_format": 3, "node_type": "array", "shape": [3], "data_type": "float64", "chunk_grid": {"name": "regular", "configuration": {"chunk_shape": [3]}}, "codecs": [{"name": "bytes", "configuration": {"endian": "little"}}], "fill_value": 0, "dimension_names": ["y"]},
            "x": {"zarr_format": 3, "node_type": "array", "shape": [4], "data_type": "float64", "chunk_grid": {"name": "regular", "configuration": {"chunk_shape": [4]}}, "codecs": [{"name": "bytes", "configuration": {"endian": "little"}}, {"name": "crc32c"}], "fill_value": 0, "dimension_names": ["x"]},
        }}});
        let store = parse_store(&[("zarr.json".into(), root.to_string().into_bytes())]).unwrap();
        let a = store.array("elev").unwrap();
        assert_eq!(a.chunk_key(&[0, 0]), "elev/c/0/0");
        let p = plan(&store, &Selection { variable: "elev".into(), index: vec![] }).unwrap();
        assert_eq!(p.keys, vec!["elev/c/0/0", "y/c/0", "x/c/0"]);
        // A zstd frame isn't made here (ruzstd decodes only): the chunk is checked raw instead.
        let raw: Vec<u8> = (0..12i16).map(|v| if v == 5 { -9999 } else { v * 10 }).flat_map(|v| v.to_be_bytes()).collect();
        let mut plain = a.clone();
        plain.codecs.retain(|c| *c != Codec::Zstd);
        let vals = plain.decode(&raw).unwrap();
        assert_eq!(vals[..3], [0.0, 10.0, 20.0]);
        let mut st2 = store.clone();
        if let Some(e) = st2.arrays.iter_mut().find(|x| x.path == "elev") {
            *e = plain;
        }
        let mut chunks = HashMap::new();
        chunks.insert("elev/c/0/0".to_string(), raw);
        chunks.insert("y/c/0".to_string(), [300.0f64, 200.0, 100.0].iter().flat_map(|x| x.to_le_bytes()).collect());
        let mut x: Vec<u8> = [0.0f64, 10.0, 20.0, 30.0].iter().flat_map(|x| x.to_le_bytes()).collect();
        x.extend([0u8; 4]);
        chunks.insert("x/c/0".to_string(), x);
        let r = assemble(&st2, &p, &chunks, "elev", None).unwrap();
        assert_eq!(r.crs, Crs::WebMercator);
        assert_eq!(r.transform, GeoTransform { origin_x: -5.0, origin_y: 350.0, pixel_width: 10.0, pixel_height: -100.0 });
        assert!(r.bands[0].data[5].is_nan());
        // A Zarr 3 fill_value (0, zarr-python's default) isn't no-data: real zeros stay, and only
        // the CF _FillValue masks.
        if let Some(e) = st2.arrays.iter_mut().find(|x| x.path == "elev") {
            e.fill = Some(0.0);
            e.attrs.remove("_FillValue");
        }
        let r = assemble(&st2, &p, &chunks, "elev", None).unwrap();
        assert_eq!(r.bands[0].data[0], 0.0);
        assert_eq!(r.bands[0].data[5], -9999.0);
        // Unknown codecs, and shards that can't be read, are said, not misread.
        let refused = |codecs: Value, why: &str| {
            let mut r = root.clone();
            r["consolidated_metadata"]["metadata"]["elev"]["codecs"] = codecs;
            let s = parse_store(&[("zarr.json".into(), r.to_string().into_bytes())]).unwrap();
            let e = plan(&s, &Selection { variable: "elev".into(), index: vec![] }).unwrap_err().to_string();
            assert!(e.contains(why), "{e}");
        };
        let shard = |inner: Value, codecs: Value| serde_json::json!({"name": "sharding_indexed", "configuration": {"chunk_shape": inner, "codecs": codecs, "index_codecs": [{"name": "bytes"}]}});
        refused(serde_json::json!([{"name": "transpose"}, {"name": "bytes"}]), "transposed");
        refused(serde_json::json!([shard(serde_json::json!([2, 4]), serde_json::json!([{"name": "bytes"}]))]), "don't tile");
        refused(
            serde_json::json!([shard(
                serde_json::json!([3, 2]),
                serde_json::json!([shard(serde_json::json!([3, 1]), serde_json::json!([]))])
            )]),
            "inside shards",
        );
        refused(
            serde_json::json!([shard(serde_json::json!([3, 2]), serde_json::json!([{"name": "bytes"}])), {"name": "zstd"}]),
            "after them",
        );
    }

    /// A shard of a (4, 6) int16 array in chunks of (2, 3), raw: chunk (0, 1) isn't held, and
    /// the index (little- or big-endian, with a checksum) is at the end or the start.
    fn shard_file(index_at_end: bool, index_little: bool) -> Vec<u8> {
        let value = |r: u64, c: u64| (r * 10 + c) as i16;
        let mut body = Vec::new();
        let mut index = Vec::new();
        let head = if index_at_end { 0 } else { 4 * 16 + 4 };
        for (gy, gx) in [(0u64, 0u64), (0, 1), (1, 0), (1, 1)] {
            if (gy, gx) == (0, 1) {
                index.extend([u64::MAX, u64::MAX]);
                continue;
            }
            let start = head + body.len() as u64;
            for r in 0..2 {
                for c in 0..3 {
                    body.extend(value(gy * 2 + r, gx * 3 + c).to_le_bytes());
                }
            }
            index.extend([start, head + body.len() as u64 - start]);
        }
        let mut idx: Vec<u8> = index.iter().flat_map(|v| if index_little { v.to_le_bytes() } else { v.to_be_bytes() }).collect();
        idx.extend([0u8; 4]);
        if index_at_end { [body, idx].concat() } else { [idx, body].concat() }
    }

    #[test]
    fn sharded_chunks() {
        for (at_end, little) in [(true, true), (false, true), (true, false)] {
            let z = serde_json::json!({
                "zarr_format": 3, "node_type": "array", "shape": [4, 6], "data_type": "int16",
                "chunk_grid": {"name": "regular", "configuration": {"chunk_shape": [4, 6]}},
                "codecs": [{"name": "sharding_indexed", "configuration": {
                    "chunk_shape": [2, 3],
                    "codecs": [{"name": "bytes", "configuration": {"endian": "little"}}],
                    "index_codecs": [{"name": "bytes", "configuration": {"endian": if little { "little" } else { "big" }}}, {"name": "crc32c"}],
                    "index_location": if at_end { "end" } else { "start" }}}],
                "fill_value": -1, "dimension_names": ["y", "x"]
            });
            let s = parse_store(&[("zarr.json".into(), z.to_string().into_bytes())]).unwrap();
            let a = s.array("").unwrap();
            assert_eq!(a.unsupported(), None);
            assert_eq!(a.chunk_key(&[0, 0]), "c/0/0");
            let vals = a.decode(&shard_file(at_end, little)).unwrap();
            // Row-major over the shard; the chunk it doesn't hold is the fill value.
            let want: Vec<f64> =
                (0..4).flat_map(|r| (0..6).map(move |c| if r < 2 && c >= 3 { -1.0 } else { (r * 10 + c) as f64 })).collect();
            assert_eq!(vals, want, "index at end {at_end}, little {little}");
        }
    }

    /// Blosc's bit shuffle of `size`-byte elements (the first multiple of 8 of them).
    fn bitshuffle(raw: &[u8], size: usize) -> Vec<u8> {
        let n = raw.len() / size / 8 * 8;
        let mut o = raw.to_vec();
        o[..n * size].fill(0);
        for e in 0..n {
            for j in 0..size {
                for k in 0..8 {
                    let bit = (raw[e * size + j] >> k) & 1;
                    o[(j * 8 + k) * (n / 8) + e / 8] |= bit << (e % 8);
                }
            }
        }
        o
    }

    #[test]
    fn blosc_bit_shuffle() {
        // 1001 values: format 3 shuffles the first 1000 and leaves the last; format 2 shuffles a
        // block only when it is a whole number of 8 values, so this one is stored as it is.
        let raw: Vec<u8> = (0..1001u32).flat_map(|v| ((v * 37 % 1013) as f32 - 500.0).to_le_bytes()).collect();
        for (format, body) in [(3u8, bitshuffle(&raw, 4)), (2, raw.clone())] {
            let c = lz4_block(&body);
            let mut f = vec![format, 1, 0x4 | 0x10 | (1 << 5), 4];
            f.extend((raw.len() as u32).to_le_bytes());
            f.extend((raw.len() as u32).to_le_bytes());
            f.extend(((16 + 4 + 4 + c.len()) as u32).to_le_bytes());
            f.extend(20u32.to_le_bytes());
            f.extend((c.len() as i32).to_le_bytes());
            f.extend(c);
            assert_eq!(blosc(&f, raw.len()).unwrap(), raw, "format {format}");
        }
        assert_eq!(bitunshuffle(&bitshuffle(&raw[..64], 2), 2, 2), raw[..64]);
        // As c-blosc 1.21 writes it (numcodecs `Blosc(cname="zstd", clevel=9, shuffle=BITSHUFFLE)`
        // of `arange(3000, dtype="<i4") // 7`): one 12,000-byte block, its bits transposed whole
        // (bitshuffle's 8 KiB sub-blocks are its own framing, which blosc doesn't use).
        let hex = concat!(
            "02019404e02e0000e02e000078000000140000006000000028b52ffd60e02db502005242070ef01903d27a8f44ed7297",
            "b9b16c0af7f6f6feff6ded8b47e9848b920914a0c05e661df0300cb701ad016f12858b2928e35426fd34dd31af0e33dd",
            "19cfae1e4667aba7aa554766810ab06cab9a5bab6c93aa5b",
        );
        let real: Vec<u8> = (0..hex.len() / 2).map(|i| u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).unwrap()).collect();
        assert_eq!((real[0], real[2] & 0x4, u32::from_le_bytes(real[8..12].try_into().unwrap())), (2, 0x4, 12_000));
        let want: Vec<u8> = (0..3000i32).flat_map(|v| (v / 7).to_le_bytes()).collect();
        assert_eq!(blosc(&real, want.len()).unwrap(), want);
        assert_eq!(to_f16(1.0), 0x3c00);
        assert_eq!(f16(to_f16(-2.5)), -2.5);
        assert_eq!(DType::parse_v2(">i2").unwrap().encode(-2.0, false), vec![0xff, 0xfe]);
    }

    #[test]
    fn codecs_round_trip() {
        let raw: Vec<u8> = (0..4000u32).flat_map(|v| ((v * 7) as f32).to_le_bytes()).collect();
        assert_eq!(blosc(&blosc_frame(&raw, 4), raw.len()).unwrap(), raw);
        let z = miniz_oxide::deflate::compress_to_vec_zlib(&raw, 6);
        assert_eq!(inflate(&z, true, raw.len()).unwrap(), raw);
        let mut gz = vec![0x1f, 0x8b, 8, 8, 0, 0, 0, 0, 0, 3];
        gz.extend(b"name\0");
        gz.extend(miniz_oxide::deflate::compress_to_vec(&raw, 6));
        assert_eq!(gunzip(&gz, raw.len()).unwrap(), raw);
        let mut l = (raw.len() as u32).to_le_bytes().to_vec();
        l.extend(lz4_block(&raw));
        assert_eq!(numcodecs_lz4(&l, raw.len()).unwrap(), raw);
        assert_eq!(unshuffle(&[1, 3, 2, 4, 9], 2), vec![1, 2, 3, 4, 9]);
        assert_eq!(f16(0x3c00), 1.0);
        assert_eq!(DType::parse_v2(">i2").unwrap().value(&[0xff, 0xfe]), -2.0);
        assert_eq!(DType::parse_v3("uint16").unwrap().value(&[1, 1]), 257.0);
    }

    /// Stores zarr-python writes (`v2.zarr`: blosc LZ4 + shuffle, consolidated; `v3.zarr`: zstd),
    /// in the folder RUSTGIS_TEST_ZARR names; skipped without it.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn stores_zarr_python_writes() {
        let Ok(dir) = std::env::var("RUSTGIS_TEST_ZARR") else { return };
        for store in ["v2.zarr", "v3.zarr"] {
            let path = std::path::Path::new(&dir).join(store);
            assert!(is_store(&path), "{store}");
            let r = read_dir(&path, "t2m[time=2]", None).unwrap();
            assert_eq!((r.width, r.height, r.crs), (360, 180, Crs::LonLat), "{store}");
            assert_eq!(r.transform, GeoTransform { origin_x: -180.0, origin_y: 90.0, pixel_width: 1.0, pixel_height: -1.0 }, "{store}");
            // The north-west corner (lon 0.5–19.5 at the top) is no data; it sits right of -180 after the turn.
            assert!(r.bands[0].data[180].is_nan() && !r.bands[0].data[0].is_nan(), "{store}");
            let s = &r.bands[0].stats;
            assert!(s.min > 230.0 && s.max < 320.0, "{store}: {s:?}");
            let store_m = read_metadata(&path).unwrap();
            let labels = info(&store_m, &HashMap::new());
            assert_eq!(labels["variables"][0]["name"], "t2m");
        }
    }

    #[test]
    fn store_paths_stay_inside_and_dates_stay_in_range() {
        // Keys that would lead out of the store are skipped, never read.
        let arr = serde_json::json!({"shape": [2, 2], "chunks": [2, 2], "dtype": "<f4", "compressor": null, "fill_value": null, "filters": null, "order": "C", "zarr_format": 2});
        for bad in ["../up/.zarray", "a/../../x/.zarray", "a\\b/.zarray", "C:x/.zarray", "a//b/.zarray", "./a/.zarray"] {
            let meta = serde_json::json!({"metadata": {bad: arr.clone()}});
            assert!(parse_store(&[(".zmetadata".into(), meta.to_string().into_bytes())]).is_err(), "{bad}");
        }
        let meta = serde_json::json!({"metadata": {"grp/v/.zarray": arr.clone()}});
        let st = parse_store(&[(".zmetadata".into(), meta.to_string().into_bytes())]).unwrap();
        assert_eq!(st.arrays[0].path, "grp/v");
        assert!(safe_path("") && safe_path("a/b") && !safe_path("/etc") && !safe_path("a/.."));
        // Hostile `since` dates: kept in range, no overflow (debug builds would panic).
        let mut t = st.arrays[0].clone();
        for units in
            ["days since 9223372036854775807-99-01", "days since -9223372036854775807-01-01", "seconds since 2000-01-01T99999999999:00"]
        {
            t.attrs.insert("units".into(), Value::String(units.into()));
            for cal in ["standard", "noleap", "360_day"] {
                t.attrs.insert("calendar".into(), Value::String(cal.into()));
                for v in [0.0, 9.9e12, -9.9e12, 1e300] {
                    let _ = label(&t, v);
                }
            }
        }
        assert!(fixed_calendar("noleap", "9223372036854775807-01-01", 1e300).is_some());
    }

    #[test]
    fn consolidated_null_is_not_consolidated() {
        let dir = std::env::temp_dir().join(format!("rustgis-zarr-null-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("v")).unwrap();
        // zarr-python 3 writes `"consolidated_metadata": null`: the arrays' own files are read.
        std::fs::write(dir.join("zarr.json"), r#"{"zarr_format": 3, "node_type": "group", "consolidated_metadata": null}"#).unwrap();
        std::fs::write(
            dir.join("v/zarr.json"),
            r#"{"zarr_format": 3, "node_type": "array", "shape": [2, 2], "data_type": "float32", "chunk_grid": {"name": "regular", "configuration": {"chunk_shape": [2, 2]}}, "codecs": [{"name": "bytes", "configuration": {"endian": "little"}}], "fill_value": 0, "dimension_names": ["y", "x"]}"#,
        )
        .unwrap();
        let st = read_metadata(&dir).unwrap();
        assert!(st.array("v").is_some());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn hostile_input_never_panics() {
        let raw: Vec<u8> = (0..4000u32).flat_map(|v| (v as f32).to_le_bytes()).collect();
        let frame = blosc_frame(&raw, 4);
        for n in 0..frame.len() {
            let _ = blosc(&frame[..n], raw.len());
        }
        let mut lied = frame.clone();
        lied[4..8].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(blosc(&lied, raw.len()).is_err());
        for d in [&b""[..], b"\x1f\x8b\x08\xff", b"\x00\x00\x00\x10abc", b"\x28\xb5\x2f\xfd"] {
            let _ = gunzip(d, 100);
            let _ = numcodecs_lz4(d, 100);
            let _ = unzstd(d, 100);
        }
        // Shards cut anywhere, with an index lying about where its chunks are or how many there
        // are, fail without panicking or making room for what they claim.
        let shard = |inner: u64| {
            serde_json::json!({"zarr_format": 3, "node_type": "array", "shape": [4, 6], "data_type": "int16",
            "chunk_grid": {"name": "regular", "configuration": {"chunk_shape": [4, 6]}},
            "codecs": [{"name": "sharding_indexed", "configuration": {"chunk_shape": [2, inner],
                "codecs": [{"name": "bytes"}, {"name": "zstd"}], "index_codecs": [{"name": "bytes"}, {"name": "crc32c"}]}}],
            "fill_value": 0})
        };
        let s = parse_store(&[("zarr.json".into(), shard(3).to_string().into_bytes())]).unwrap();
        let a = s.array("").unwrap();
        let file = shard_file(true, true);
        for n in 0..file.len() {
            let _ = a.decode(&file[..n]);
        }
        let mut lied = file.clone();
        let at = lied.len() - 68;
        lied[at..at + 8].copy_from_slice(&(u64::MAX - 3).to_le_bytes());
        assert!(a.decode(&lied).is_err());
        let s1 = parse_store(&[("zarr.json".into(), shard(1).to_string().into_bytes())]).unwrap();
        assert!(s1.array("").unwrap().decode(&[0u8; 40]).is_err());
        let huge = serde_json::json!({"metadata": {"a/.zarray": {"shape": [1u64 << 40, 1u64 << 40], "chunks": [1u64 << 30, 1u64 << 30], "dtype": "<f8", "compressor": null}}});
        let s = parse_store(&[(".zmetadata".into(), huge.to_string().into_bytes())]).unwrap();
        assert!(plan(&s, &Selection { variable: "a".into(), index: vec![] }).is_err());
        assert!(parse_store(&[(".zmetadata".into(), b"{}".to_vec())]).is_err());
        assert!(parse_store(&[("zarr.json".into(), b"[1,2".to_vec())]).is_err());
        assert_eq!(
            Selection::parse("t2m[time=3, level=1]"),
            Selection { variable: "t2m".into(), index: vec![("time".into(), 3), ("level".into(), 1)] }
        );
        assert_eq!(Selection::parse("t2m[time=3,level=1]").label(), "t2m[time=3,level=1]");
    }
}
