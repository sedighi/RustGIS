//! GeoTIFF reading and writing, including tiled and cloud-optimised files.
//!
//! Supports grey, RGB(A), palette and multiband images of any integer or floating-point pixel
//! type, stored in strips or tiles, chunky or planar, uncompressed or compressed with Deflate, LZW,
//! PackBits or JPEG. Georeferencing comes from the GeoTIFF tags (pixel scale and tie point, or a
//! transformation matrix without rotation) and the EPSG code in the GeoKey directory.

use std::io::{Read, Seek};

use tiff::ColorType;
use tiff::decoder::{Decoder, DecodingResult, Limits};
use tiff::tags::Tag;

use super::cog::Sparse;
use super::crs::CustomCrs;
use super::{ColorHint, Crs, GeoTransform, MAX_SAMPLES, Raster};
use crate::{DataError, Result};

fn err(e: impl std::fmt::Display) -> DataError {
    DataError::Format(format!("GeoTIFF: {e}"))
}

/// The GeoTIFF parts of a TIFF's first image.
struct Geo {
    transform: GeoTransform,
    crs: Option<Crs>,
    /// An EPSG code that was named but isn't known.
    unknown_epsg: Option<u32>,
    geographic_model: bool,
    nodata: Option<f64>,
    /// Why the file's own projection couldn't be read, if it has one.
    custom_error: Option<String>,
}

fn geo_tags<R: Read + Seek>(dec: &mut Decoder<R>) -> Result<Geo> {
    let doubles =
        |dec: &mut Decoder<R>, tag: u16| -> Option<Vec<f64>> { dec.find_tag(Tag::Unknown(tag)).ok().flatten()?.into_f64_vec().ok() };
    // GeoKey directory: a header of four shorts, then (key, location, count, value) entries.
    let keys: Vec<u16> = dec.find_tag(Tag::GeoKeyDirectoryTag).ok().flatten().and_then(|v| v.into_u16_vec().ok()).unwrap_or_default();
    let key = |k: u16| -> Option<u16> {
        keys.get(4..)?.as_chunks::<4>().0.iter().find(|[key, location, _, _]| *key == k && *location == 0).map(|[_, _, _, value]| *value)
    };
    let pixel_is_point = key(1025) == Some(2);
    let geographic_model = key(1024) == Some(2);
    let usable = |c: &u16| *c != 32767 && *c != 0;
    let gdoubles = doubles(dec, 34736).unwrap_or_default();
    let gascii = dec.find_tag(Tag::Unknown(34737)).ok().flatten().and_then(|v| v.into_string().ok()).unwrap_or_default();
    // A projected file with a user-defined projection (the NLCD's Albers, say) defines it in its
    // keys: read as its own system, never as the geographic system it's based on.
    let user_defined = key(1024) == Some(1) && key(3072).is_none_or(|c| !usable(&c));
    // A projection the keys don't describe well enough is left unknown (the open goes on, and fails
    // later only if the coordinates aren't degrees), its reason kept for the message.
    let mut custom_error = None;
    let (code, crs) = if user_defined {
        match custom_crs(&keys, &gdoubles, &gascii) {
            Ok(crs) => (None, Some(crs)),
            Err(e) => {
                custom_error = Some(e.to_string());
                (None, None)
            }
        }
    } else {
        let code = key(3072).filter(usable).or(key(2048).filter(usable)).map(u32::from);
        (code, code.and_then(Crs::from_epsg))
    };

    let transform = if let Some(m) = doubles(dec, 34264).filter(|m| m.len() >= 8) {
        let at = |i: usize| m.get(i).copied().unwrap_or(0.0);
        if at(1) != 0.0 || at(4) != 0.0 {
            return Err(err("rotated rasters aren't supported yet"));
        }
        GeoTransform { origin_x: at(3), origin_y: at(7), pixel_width: at(0), pixel_height: at(5) }
    } else {
        let scale = doubles(dec, 33550).filter(|s| s.len() >= 2).ok_or_else(|| err("the file isn't georeferenced"))?;
        let tie = doubles(dec, 33922).filter(|t| t.len() >= 6).ok_or_else(|| err("the file isn't georeferenced"))?;
        let s = |i: usize| scale.get(i).copied().unwrap_or(0.0);
        let t = |i: usize| tie.get(i).copied().unwrap_or(0.0);
        GeoTransform { origin_x: t(3) - t(0) * s(0), origin_y: t(4) + t(1) * s(1), pixel_width: s(0), pixel_height: -s(1) }
    };
    let transform = if pixel_is_point {
        GeoTransform {
            origin_x: transform.origin_x - transform.pixel_width / 2.0,
            origin_y: transform.origin_y - transform.pixel_height / 2.0,
            ..transform
        }
    } else {
        transform
    };
    let nodata = dec
        .find_tag(Tag::GdalNodata)
        .ok()
        .flatten()
        .and_then(|v| v.into_string().ok())
        .and_then(|s| s.trim_matches(|c: char| c == '\0' || c.is_whitespace()).parse::<f64>().ok());
    Ok(Geo { transform, crs, unknown_epsg: code.filter(|_| crs.is_none()), geographic_model, nodata, custom_error })
}

/// A cloud-optimised GeoTIFF's coordinate system, georeferencing and no-data value, from its first
/// image (as [`read`] finds them).
pub(crate) fn cog_geo<R: Read + Seek>(dec: &mut Decoder<R>) -> Result<(Crs, GeoTransform, Option<f64>)> {
    let geo = geo_tags(dec)?;
    let crs = match geo.crs {
        Some(c) => c,
        None if geo.geographic_model => Crs::LonLat,
        None => match geo.unknown_epsg {
            Some(code) => return Err(err(format!("the coordinate system EPSG:{code} isn't supported"))),
            None => {
                return Err(match geo.custom_error {
                    Some(e) => DataError::Format(e),
                    None => err("the file's coordinate system isn't recognised"),
                });
            }
        },
    };
    Ok((crs, geo.transform, geo.nodata))
}

#[cfg(test)]
pub(crate) mod tests_support {
    /// A small GeoTIFF in strips (as [`super::write`] writes them).
    pub(crate) fn strip_tiff() -> Vec<u8> {
        let r = crate::raster::Raster::new(
            "s",
            4,
            4,
            vec![vec![1.0; 16]],
            crate::raster::GeoTransform { origin_x: 0.0, origin_y: 1.0, pixel_width: 0.25, pixel_height: -0.25 },
            crate::raster::Crs::LonLat,
            None,
            crate::raster::ColorHint::Gray,
            "Float32",
        )
        .unwrap_or_else(|e| panic!("{e}"));
        super::write(&r, 0).unwrap_or_else(|e| panic!("{e}"))
    }
}

/// A GeoKey's parameter: inline, or from the double or ASCII parameter tags.
enum KeyValue {
    Short(u16),
    Double(f64),
    Ascii(String),
}

fn geokey(keys: &[u16], doubles: &[f64], ascii: &str, k: u16) -> Option<KeyValue> {
    let [_, location, count, value] = *keys.get(4..)?.as_chunks::<4>().0.iter().find(|e| e[0] == k)?;
    match location {
        0 => Some(KeyValue::Short(value)),
        34736 => doubles.get(value as usize).copied().map(KeyValue::Double),
        34737 => ascii
            .get(value as usize..(value as usize).saturating_add(count as usize))
            .map(|s| KeyValue::Ascii(s.trim_end_matches(['|', '\0']).to_string())),
        _ => None,
    }
}

/// A user-defined projection from a file's GeoKeys, as a PROJ definition.
fn custom_crs(keys: &[u16], doubles: &[f64], ascii: &str) -> Result<Crs> {
    let short = |k| match geokey(keys, doubles, ascii, k) {
        Some(KeyValue::Short(v)) => Some(v),
        _ => None,
    };
    let num = |k| match geokey(keys, doubles, ascii, k) {
        Some(KeyValue::Double(v)) if v.is_finite() => Some(v),
        _ => None,
    };
    let text = |k| match geokey(keys, doubles, ascii, k) {
        Some(KeyValue::Ascii(s)) => Some(s),
        _ => None,
    };
    // The first of several keys a parameter may be given by.
    let any = |ks: &[u16]| ks.iter().find_map(|k| num(*k));
    // Metres per linear unit: false eastings and northings are in the file's units, PROJ's in
    // metres.
    let (to_m, units) = match short(3076) {
        Some(9002) => (0.3048, "+units=ft".to_string()),
        Some(9003) => (1200.0 / 3937.0, "+units=us-ft".to_string()),
        _ => match num(3077) {
            Some(m) if m > 0.0 && (m - 1.0).abs() > 1e-12 => (m, format!("+to_meter={m}")),
            _ => (1.0, "+units=m".to_string()),
        },
    };
    let lat0 = any(&[3081, 3085, 3089]).unwrap_or(0.0);
    let lon0 = any(&[3080, 3084, 3088, 3095]).unwrap_or(0.0);
    let x0 = any(&[3082, 3086, 3090]).unwrap_or(0.0) * to_m;
    let y0 = any(&[3083, 3087, 3091]).unwrap_or(0.0) * to_m;
    let k0 = any(&[3092, 3093]).unwrap_or(1.0);
    let (lat1, lat2) = (num(3078).unwrap_or(lat0), num(3079).unwrap_or(lat0));
    let proj = match short(3075) {
        Some(1) => format!("+proj=tmerc +lat_0={lat0} +lon_0={lon0} +k_0={k0} +x_0={x0} +y_0={y0}"),
        Some(7) => format!("+proj=merc +lon_0={lon0} +k_0={k0} +x_0={x0} +y_0={y0}"),
        Some(8) => format!("+proj=lcc +lat_1={lat1} +lat_2={lat2} +lat_0={lat0} +lon_0={lon0} +x_0={x0} +y_0={y0}"),
        Some(9) => format!("+proj=lcc +lat_1={lat0} +lat_0={lat0} +lon_0={lon0} +k_0={k0} +x_0={x0} +y_0={y0}"),
        Some(10) => format!("+proj=laea +lat_0={lat0} +lon_0={lon0} +x_0={x0} +y_0={y0}"),
        Some(11) => format!("+proj=aea +lat_1={lat1} +lat_2={lat2} +lat_0={lat0} +lon_0={lon0} +x_0={x0} +y_0={y0}"),
        Some(12) => format!("+proj=aeqd +lat_0={lat0} +lon_0={lon0} +x_0={x0} +y_0={y0}"),
        Some(14) => format!("+proj=stere +lat_0={lat0} +lon_0={lon0} +k_0={k0} +x_0={x0} +y_0={y0}"),
        Some(15) => {
            // Polar: the pole on the side of the latitude of true scale.
            let pole = if lat0 < 0.0 { -90.0 } else { 90.0 };
            format!("+proj=stere +lat_0={pole} +lat_ts={lat0} +lon_0={lon0} +k_0={k0} +x_0={x0} +y_0={y0}")
        }
        Some(16) => format!("+proj=sterea +lat_0={lat0} +lon_0={lon0} +k_0={k0} +x_0={x0} +y_0={y0}"),
        Some(17) => format!("+proj=eqc +lat_ts={lat1} +lon_0={lon0} +x_0={x0} +y_0={y0}"),
        Some(28) => format!("+proj=cea +lat_ts={lat1} +lon_0={lon0} +x_0={x0} +y_0={y0}"),
        Some(c) => return Err(err(format!("the file's own projection (coordinate transformation {c}) isn't supported"))),
        None => return Err(err("the file's own projection isn't described in its keys")),
    };
    // The datum or ellipsoid, by code or by its axes.
    let earth = match (short(2048), short(2050), short(2056)) {
        (Some(4326), _, _) | (_, Some(6326), _) | (_, _, Some(7030)) => "+datum=WGS84".to_string(),
        (Some(4269), _, _) | (_, Some(6269), _) | (_, _, Some(7019)) => "+ellps=GRS80 +towgs84=0,0,0".to_string(),
        (Some(4267), _, _) | (_, Some(6267), _) | (_, _, Some(7008)) => "+ellps=clrk66".to_string(),
        _ => match (num(2057), num(2059), num(2058)) {
            (Some(a), Some(rf), _) if a > 0.0 && rf > 0.0 => format!("+a={a} +rf={rf}"),
            (Some(a), _, Some(b)) if a > 0.0 && b > 0.0 => format!("+a={a} +b={b}"),
            _ => "+datum=WGS84".to_string(),
        },
    };
    let label = text(3073).or_else(|| text(1026)).filter(|t| !t.trim().is_empty()).unwrap_or_else(|| "Custom projection".into());
    Crs::custom(CustomCrs {
        proj4: format!("{proj} {earth} {units} +no_defs"),
        label,
        geokeys: keys.to_vec(),
        doubles: doubles.to_vec(),
        ascii: ascii.to_string(),
    })
    .map_err(err)
}

fn to_f32(buf: DecodingResult) -> (Vec<f32>, &'static str) {
    match buf {
        DecodingResult::U8(v) => (v.into_iter().map(f32::from).collect(), "UInt8"),
        DecodingResult::I8(v) => (v.into_iter().map(f32::from).collect(), "Int8"),
        DecodingResult::U16(v) => (v.into_iter().map(f32::from).collect(), "UInt16"),
        DecodingResult::I16(v) => (v.into_iter().map(f32::from).collect(), "Int16"),
        DecodingResult::U32(v) => (v.into_iter().map(|x| x as f32).collect(), "UInt32"),
        DecodingResult::I32(v) => (v.into_iter().map(|x| x as f32).collect(), "Int32"),
        DecodingResult::U64(v) => (v.into_iter().map(|x| x as f32).collect(), "UInt64"),
        DecodingResult::I64(v) => (v.into_iter().map(|x| x as f32).collect(), "Int64"),
        DecodingResult::F16(v) => (v.into_iter().map(f32::from).collect(), "Float16"),
        DecodingResult::F32(v) => (v, "Float32"),
        DecodingResult::F64(v) => (v.into_iter().map(|x| x as f32).collect(), "Float64"),
    }
}

/// Bits per sample of the current image (the most of any sample).
fn bits<R: Read + Seek>(dec: &mut Decoder<R>) -> u16 {
    dec.find_tag_unsigned_vec::<u16>(Tag::BitsPerSample).ok().flatten().and_then(|v| v.into_iter().max()).unwrap_or(8)
}

/// Bytes per sample of the current image (from its bits per sample).
fn sample_bytes<R: Read + Seek>(dec: &mut Decoder<R>) -> usize {
    (bits(dec) as usize).div_ceil(8).max(1)
}

fn samples<R: Read + Seek>(dec: &mut Decoder<R>) -> usize {
    dec.find_tag_unsigned::<u16>(Tag::SamplesPerPixel).ok().flatten().unwrap_or(1).max(1) as usize
}

/// A reader that shows palette images as grey ones: the `tiff` crate decodes no palette images, but
/// decodes their colour indices as grey samples, and the colour table is read from the ColorMap tag
/// as it is. The only change is the PhotometricInterpretation value (3 → 1) in each image's IFD.
pub(crate) struct Unpaletted<R> {
    inner: R,
    /// Where each palette image's PhotometricInterpretation value is, and what to show there.
    patches: Vec<(u64, [u8; 2])>,
    pos: u64,
}

/// Most images and IFD entries looked at for palette images.
const MAX_IFDS: usize = 64;
const MAX_ENTRIES: u64 = 4096;

/// Where the palette images' PhotometricInterpretation values are, walking the IFD chain
/// (classic and BigTIFF). A file it can't walk has none: the decoder reports what's wrong.
fn palette_patches<R: Read + Seek>(r: &mut R) -> Vec<(u64, [u8; 2])> {
    use std::io::SeekFrom;
    let mut patches = Vec::new();
    let mut walk = || -> Option<()> {
        let mut head = [0u8; 16];
        r.seek(SeekFrom::Start(0)).ok()?;
        r.read_exact(&mut head[..8]).ok()?;
        let little = match &head[..2] {
            b"II" => true,
            b"MM" => false,
            _ => return None,
        };
        let u16_ = |b: &[u8]| -> Option<u16> {
            let a: [u8; 2] = b.get(..2)?.try_into().ok()?;
            Some(if little { u16::from_le_bytes(a) } else { u16::from_be_bytes(a) })
        };
        let u32_ = |b: &[u8]| -> Option<u32> {
            let a: [u8; 4] = b.get(..4)?.try_into().ok()?;
            Some(if little { u32::from_le_bytes(a) } else { u32::from_be_bytes(a) })
        };
        let u64_ = |b: &[u8]| -> Option<u64> {
            let a: [u8; 8] = b.get(..8)?.try_into().ok()?;
            Some(if little { u64::from_le_bytes(a) } else { u64::from_be_bytes(a) })
        };
        let big = match u16_(head.get(2..)?)? {
            42 => false,
            43 => true,
            _ => return None,
        };
        let mut ifd = if big {
            r.read_exact(&mut head[8..16]).ok()?;
            u64_(head.get(8..)?)?
        } else {
            u64::from(u32_(head.get(4..)?)?)
        };
        let (count_len, entry_len, next_len) = if big { (8u64, 20u64, 8usize) } else { (2, 12, 4) };
        for _ in 0..MAX_IFDS {
            if ifd == 0 {
                break;
            }
            r.seek(SeekFrom::Start(ifd)).ok()?;
            let mut c = [0u8; 8];
            r.read_exact(&mut c[..count_len as usize]).ok()?;
            let n = if big { u64_(&c)? } else { u64::from(u16_(&c)?) };
            if n > MAX_ENTRIES {
                return None;
            }
            let mut entries = vec![0u8; usize::try_from(n * entry_len).ok()?];
            r.read_exact(&mut entries).ok()?;
            for (i, e) in entries.chunks(entry_len as usize).enumerate() {
                // PhotometricInterpretation (262), a SHORT, its value inline.
                if u16_(e)? == 262 && u16_(e.get(2..)?)? == 3 {
                    let value_at = if big { 12 } else { 8 };
                    if u16_(e.get(value_at..)?)? == 3 {
                        let at = ifd + count_len + i as u64 * entry_len + value_at as u64;
                        patches.push((at, if little { [1, 0] } else { [0, 1] }));
                    }
                }
            }
            let mut next = [0u8; 8];
            r.read_exact(&mut next[..next_len]).ok()?;
            ifd = if big { u64_(&next)? } else { u64::from(u32_(&next)?) };
        }
        Some(())
    };
    let _ = walk();
    patches
}

/// Any reader, boxed: the decoder is compiled once for every kind of source (a file, bytes in
/// memory, a COG's ranges), which keeps the browser build small.
pub(crate) trait ReadSeek: Read + Seek {}
impl<T: Read + Seek> ReadSeek for T {}

/// What the decoder reads from.
pub(crate) type Reader<'a> = Unpaletted<Box<dyn ReadSeek + 'a>>;

/// A reader for the decoder.
pub(crate) fn reader<'a>(r: impl Read + Seek + 'a) -> std::io::Result<Reader<'a>> {
    Unpaletted::new(Box::new(r))
}

impl<R: Read + Seek> Unpaletted<R> {
    fn new(mut inner: R) -> std::io::Result<Self> {
        let patches = palette_patches(&mut inner);
        inner.seek(std::io::SeekFrom::Start(0))?;
        Ok(Unpaletted { inner, patches, pos: 0 })
    }
}

impl<R: Read> Read for Unpaletted<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        let (start, end) = (self.pos, self.pos + n as u64);
        for (at, bytes) in &self.patches {
            for (k, b) in bytes.iter().enumerate() {
                let p = at + k as u64;
                if p >= start
                    && p < end
                    && let Some(slot) = buf.get_mut((p - start) as usize)
                {
                    *slot = *b;
                }
            }
        }
        self.pos = end;
        Ok(n)
    }
}

impl<R: Seek> Seek for Unpaletted<R> {
    fn seek(&mut self, to: std::io::SeekFrom) -> std::io::Result<u64> {
        self.pos = self.inner.seek(to)?;
        Ok(self.pos)
    }
}

/// Opens another reader on the same file, for decoding its chunks on more than one thread.
pub(crate) type Opener<'a> = &'a (dyn Fn() -> std::io::Result<Reader<'static>> + Sync);

/// How an image is cut into chunks (strips or tiles), and how they're laid out.
#[derive(Clone, Copy)]
struct Chunks {
    w: u32,
    h: u32,
    /// Chunk width and height.
    cw: u32,
    ch: u32,
    /// Chunks across and down one plane.
    across: u32,
    per_plane: u32,
    planes: usize,
    /// Samples in each chunk's pixels.
    samples: usize,
    planar: bool,
}

impl Chunks {
    fn new<R: Read + Seek>(dec: &mut Decoder<R>, w: u32, h: u32, nbands: usize, planar: bool) -> Self {
        let tag = |dec: &mut Decoder<R>, t: Tag| dec.find_tag_unsigned::<u32>(t).ok().flatten();
        // Chunk sizes from the tags (the decoder's own helpers assume a sound file).
        let tiled = tag(dec, Tag::TileWidth).is_some();
        let (cw, ch) = if tiled {
            (tag(dec, Tag::TileWidth).unwrap_or(256).max(1), tag(dec, Tag::TileLength).unwrap_or(256).max(1))
        } else {
            (w.max(1), tag(dec, Tag::RowsPerStrip).unwrap_or(h).clamp(1, h.max(1)))
        };
        let (across, down) = (w.div_ceil(cw), h.div_ceil(ch));
        Chunks {
            w,
            h,
            cw,
            ch,
            across,
            per_plane: across.saturating_mul(down),
            planes: if planar { nbands } else { 1 },
            samples: if planar { 1 } else { nbands },
            planar,
        }
    }

    fn count(&self) -> u64 {
        u64::from(self.per_plane) * self.planes as u64
    }

    /// Chunk `i` (counting through every plane) as (plane, chunk index in the file), when it holds
    /// a kept row and column; a chunk holding none is skipped unread.
    fn wanted(&self, i: u64, k: u32) -> Option<(usize, u32)> {
        let plane = usize::try_from(i / u64::from(self.per_plane.max(1))).ok()?;
        let c = u32::try_from(i % u64::from(self.per_plane.max(1))).ok()?;
        let (x0, y0) = ((c % self.across) * self.cw, (c / self.across) * self.ch);
        let rows = self.ch.min(self.h.saturating_sub(y0));
        if y0.div_ceil(k) * k >= y0 + rows || x0.div_ceil(k) * k >= x0 + self.cw.min(self.w - x0) {
            return None;
        }
        let index = u32::try_from(plane).ok()?.checked_mul(self.per_plane)?.checked_add(c)?;
        Some((plane, index))
    }

    /// Copies every `k`-th pixel each way of chunk `c` of `plane` (decoded as `values`) into
    /// `bands`, each `ow` pixels wide.
    fn place(&self, bands: &mut [Vec<f32>], values: &[f32], plane: usize, c: u32, k: u32, ow: usize) {
        let c = c % self.per_plane.max(1);
        let (x0, y0) = ((c % self.across) * self.cw, (c / self.across) * self.ch);
        let rows = self.ch.min(self.h.saturating_sub(y0));
        // The chunk's row length: the full tile width when padded, else the data width.
        let stride = (values.len() / (rows as usize).max(1) / self.samples.max(1)).max(1);
        let mut y = y0.div_ceil(k) * k;
        while y < y0 + rows {
            let mut x = x0.div_ceil(k) * k;
            while x < (x0 + self.cw).min(self.w) {
                let src = ((y - y0) as usize * stride + (x - x0) as usize) * self.samples;
                let dst = (y / k) as usize * ow + (x / k) as usize;
                for b in 0..self.samples {
                    let band = if self.planar { plane } else { b };
                    if let (Some(v), Some(slot)) = (values.get(src + b), bands.get_mut(band).and_then(|v| v.get_mut(dst))) {
                        *slot = *v;
                    }
                }
                x += k;
            }
            y += k;
        }
    }
}

/// Fewest chunks worth decoding on more than one thread.
#[cfg(not(target_arch = "wasm32"))]
const PARALLEL_CHUNKS: u64 = 16;

/// Reads an image a strip or tile at a time, keeping every `k`-th pixel each way: an overview
/// made as it is read, for a file too large to hold that has none. Returns the bands. With `k`
/// 1, the whole image a chunk at a time. With `open`, chunks are decoded on several threads,
/// each reading image `image` of its own copy of the file.
#[allow(clippy::too_many_arguments)]
fn decimate<R: Read + Seek>(
    dec: &mut Decoder<R>,
    w: u32,
    h: u32,
    nbands: usize,
    planar: bool,
    k: u32,
    open: Option<(Opener<'_>, usize, Limits)>,
) -> Result<(Vec<Vec<f32>>, &'static str)> {
    let k = k.max(1);
    let (ow, oh) = (w.div_ceil(k) as usize, h.div_ceil(k) as usize);
    let mut bands = vec![vec![f32::NAN; ow * oh]; nbands];
    let chunks = Chunks::new(dec, w, h, nbands, planar);
    #[cfg(not(target_arch = "wasm32"))]
    if let Some((open, image, limits)) = open
        && chunks.count() >= PARALLEL_CHUNKS
    {
        let threads = std::thread::available_parallelism().map_or(1, |n| n.get()).min(16);
        if threads > 1 {
            return decimate_parallel(chunks, bands, k, ow, threads, open, image, limits);
        }
    }
    let _ = open;
    let mut data_type = "Float32";
    for i in 0..chunks.count() {
        let Some((plane, index)) = chunks.wanted(i, k) else { continue };
        let (values, dt) = to_f32(dec.read_chunk(index).map_err(err)?);
        data_type = dt;
        chunks.place(&mut bands, &values, plane, index, k, ow);
    }
    Ok((bands, data_type))
}

/// [`decimate`] on `threads` threads, each with its own decoder taking the next chunk in turn.
#[cfg(not(target_arch = "wasm32"))]
#[allow(clippy::too_many_arguments)]
fn decimate_parallel(
    chunks: Chunks,
    bands: Vec<Vec<f32>>,
    k: u32,
    ow: usize,
    threads: usize,
    open: Opener<'_>,
    image: usize,
    limits: Limits,
) -> Result<(Vec<Vec<f32>>, &'static str)> {
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    let next = AtomicU64::new(0);
    let failed = AtomicBool::new(false);
    let failure: Mutex<Option<DataError>> = Mutex::new(None);
    let out = Mutex::new((bands, "Float32"));
    let fail = |e: DataError| {
        failed.store(true, Ordering::Relaxed);
        if let Ok(mut f) = failure.lock() {
            f.get_or_insert(e);
        }
    };
    let work = || {
        let dec = open().map_err(err).and_then(|r| Decoder::new(r).map_err(err)).map(|d| d.with_limits(limits.clone()));
        let mut dec = match dec.and_then(|mut d| d.seek_to_image(image).map_err(err).map(|()| d)) {
            Ok(d) => d,
            Err(e) => return fail(e),
        };
        while !failed.load(Ordering::Relaxed) {
            let i = next.fetch_add(1, Ordering::Relaxed);
            if i >= chunks.count() {
                break;
            }
            let Some((plane, index)) = chunks.wanted(i, k) else { continue };
            match dec.read_chunk(index) {
                Ok(buf) => {
                    let (values, dt) = to_f32(buf);
                    if let Ok(mut o) = out.lock() {
                        o.1 = dt;
                        chunks.place(&mut o.0, &values, plane, index, k, ow);
                    }
                }
                Err(e) => return fail(err(e)),
            }
        }
    };
    std::thread::scope(|scope| {
        let handles: Vec<_> = (0..threads).map(|_| scope.spawn(work)).collect();
        for h in handles {
            if h.join().is_err() {
                fail(err("a chunk couldn't be decoded"));
            }
        }
    });
    if let Some(e) = failure.into_inner().ok().flatten() {
        return Err(e);
    }
    out.into_inner().map_err(|_| err("a chunk couldn't be decoded"))
}

/// Reads a GeoTIFF. Images too large for [`MAX_SAMPLES`] are read from their largest overview that
/// fits, when the file has overviews; otherwise an overview is made as the image is read, a strip
/// or tile at a time (a regular, not cloud-optimised, file).
pub fn read<R: Read + Seek>(r: R, name: &str) -> Result<Raster> {
    read_limited(reader(r).map_err(err)?, name, MAX_SAMPLES, None)
}

/// What reading a GeoTIFF from the parts of it read so far came to.
#[derive(Debug)]
pub enum Partial {
    Done(Box<Raster>),
    /// These bytes (first, end) must be read first.
    Need(u64, u64),
}

/// Reads a GeoTIFF from the parts of it read so far, as [`read`] does: the raster, or the range
/// to read next. The web app reads a large file so, a part at a time, keeping only its header and
/// the overview it opens from (or the strips or tiles an overview made as it is read needs).
pub fn read_sparse(parts: &Sparse, name: &str) -> Result<Partial> {
    read_sparse_limited(parts, name, MAX_SAMPLES)
}

fn read_sparse_limited(parts: &Sparse, name: &str, max: usize) -> Result<Partial> {
    let r = parts.reader();
    let watch = r.clone();
    let out = reader(r).map_err(err).and_then(|r| read_limited(r, name, max, None));
    // A part missing anywhere (a palette walk that gave up, say) is read before trusting the rest.
    if let Some((a, b)) = watch.missing() {
        return Ok(Partial::Need(a, b));
    }
    out.map(|r| Partial::Done(Box::new(r)))
}

/// [`read`] from a file, decoding its strips or tiles on several threads.
#[cfg(not(target_arch = "wasm32"))]
pub fn read_file(path: &std::path::Path, name: &str) -> Result<Raster> {
    let path = path.to_path_buf();
    let open = move || -> std::io::Result<Reader<'static>> { reader(std::io::BufReader::new(std::fs::File::open(&path)?)) };
    read_limited(open().map_err(err)?, name, MAX_SAMPLES, Some(&open))
}

/// [`read`], holding at most `max` samples (the tests' small limit exercises the overviews), with
/// `open` making more readers of the same file for decoding it on several threads.
fn read_limited(r: Reader<'_>, name: &str, max: usize, open: Option<Opener<'_>>) -> Result<Raster> {
    let palette = !r.patches.is_empty();
    let limits = {
        let mut l = Limits::default();
        l.decoding_buffer_size = MAX_SAMPLES.saturating_mul(4);
        l.intermediate_buffer_size = 256 << 20;
        l.ifd_value_size = 64 << 20;
        l
    };
    let mut dec = Decoder::new(r).map_err(err)?.with_limits(limits.clone());
    let (full_w, full_h) = dec.dimensions().map_err(err)?;
    let geo = geo_tags(&mut dec)?;
    let spp = samples(&mut dec);
    let bytes = sample_bytes(&mut dec);
    // The decoded image is held at its own pixel size before conversion to f32, so bound bytes
    // as well as samples.
    let fits = |w: u32, h: u32, spp: usize, bytes: usize| {
        let n = (w as usize).saturating_mul(h as usize).saturating_mul(spp);
        n <= max && n.saturating_mul(bytes) <= max.saturating_mul(4)
    };
    let (mut w, mut h) = (full_w, full_h);
    let mut overview = None;
    // Every k-th pixel of the full image, for a large one without overviews (1: read whole).
    let mut decimate_by = 1u32;
    // The image read: 0, the full one, or an overview.
    let mut image = 0;
    if !fits(w, h, spp, bytes) {
        // The largest overview that fits, whatever order the images are in; a damaged image
        // ends the search rather than the open.
        let mut best: Option<(usize, u32, u32)> = None;
        let mut index = 0;
        while dec.more_images() {
            if dec.next_image().is_err() {
                break;
            }
            index += 1;
            let subfile = dec.find_tag_unsigned::<u32>(Tag::NewSubfileType).ok().flatten().unwrap_or(0);
            if subfile & 4 != 0 {
                continue; // a transparency mask
            }
            let Ok((ow, oh)) = dec.dimensions() else { continue };
            let (ospp, obytes) = (samples(&mut dec), sample_bytes(&mut dec));
            if fits(ow, oh, ospp, obytes) && best.is_none_or(|(_, bw, bh)| ow as u64 * oh as u64 > bw as u64 * bh as u64) {
                best = Some((index, ow, oh));
            }
        }
        // Every k-th pixel of the full image, the fewest left out that fit.
        let mut k = 2u32;
        while !fits(full_w.div_ceil(k), full_h.div_ceil(k), spp, 4) {
            k = k.saturating_mul(2);
            if k > 1 << 20 {
                break;
            }
        }
        let (dw, dh) = (full_w.div_ceil(k), full_h.div_ceil(k));
        let decimated = (k <= 1 << 20).then_some(u64::from(dw) * u64::from(dh));
        match best {
            // An overview at least as detailed as the full image read sparsely: only it is read.
            Some((index, ow, oh)) if decimated.is_none_or(|d| u64::from(ow) * u64::from(oh) >= d) => {
                dec.seek_to_image(index).map_err(err)?;
                image = index;
                (w, h) = (ow, oh);
            }
            // No overview, or only coarse ones (a 5000-pixel orthophoto with one of 157 pixels,
            // issue #162): one is made as the full image is read.
            _ => {
                if decimated.is_none() {
                    return Err(err(format!("{full_w} × {full_h} pixels is too large to open")));
                }
                dec.seek_to_image(0).map_err(err)?;
                decimate_by = k;
                (w, h) = (dw, dh);
            }
        }
        overview = Some((full_w, full_h));
    }
    // From here on, tags (colour type, planar layout) are those of the chosen image.
    let ct = dec.colortype().map_err(err)?;
    // A palette image (shown to the decoder as grey) is coloured by its table.
    let ct = match ct {
        ColorType::Gray(bits) if palette && dec.find_tag(Tag::ColorMap).ok().flatten().is_some() => ColorType::Palette(bits),
        other => other,
    };
    let (nbands, color) = match ct {
        ColorType::Gray(_) => (1, ColorHint::Gray),
        ColorType::GrayA(_) => (2, ColorHint::GrayAlpha),
        ColorType::RGB(_) => (3, ColorHint::Rgb { alpha: false }),
        // The decoder turns JPEG-compressed YCbCr into RGB; other YCbCr data would stay YCbCr.
        ColorType::YCbCr(_) if dec.find_tag_unsigned::<u16>(Tag::Compression).ok().flatten() == Some(7) => {
            (3, ColorHint::Rgb { alpha: false })
        }
        ColorType::RGBA(_) => (4, ColorHint::Rgb { alpha: true }),
        ColorType::Palette(bits) => {
            let map: Vec<u16> = dec.find_tag(Tag::ColorMap).ok().flatten().and_then(|v| v.into_u16_vec().ok()).unwrap_or_default();
            let n = 1usize << bits.min(16);
            let c = |i: usize| (map.get(i).copied().unwrap_or(0) / 257) as u8;
            let table = if map.len() >= 3 * n { (0..n).map(|i| [c(i), c(n + i), c(2 * n + i), 255]).collect() } else { Vec::new() };
            (1, if table.is_empty() { ColorHint::Gray } else { ColorHint::Palette(table) })
        }
        ColorType::Multiband { num_samples, .. } => (num_samples.max(1) as usize, ColorHint::Gray),
        other => return Err(err(format!("{other:?} images aren't supported"))),
    };
    let planar = dec.find_tag_unsigned::<u16>(Tag::PlanarConfiguration).ok().flatten() == Some(2);
    let n = (w as usize) * (h as usize);
    // The decoder leaves out samples past the colour ones (RGB with an undefined fourth band, as
    // in RGBI orthophotos) a tile row at a time, and writes a padded edge tile's whole width into
    // the image: past its edge, over the start of the next row (issue #162). Such tiled images
    // are read a tile at a time, as their chunks come back only as wide as the image.
    let extra_samples = !planar && samples(&mut dec) > nbands && dec.find_tag_unsigned::<u32>(Tag::TileWidth).ok().flatten().is_some();
    // A large image, from a file, is read a chunk at a time on several threads (not one of
    // fewer than 8 bits a sample, whose chunks pack several pixels to a byte).
    let chunked = open.is_some() && n >= 1 << 20 && bits(&mut dec) >= 8;
    let (bands, data_type) = if decimate_by > 1 || extra_samples || chunked {
        let (iw, ih) = if decimate_by > 1 { (full_w, full_h) } else { (w, h) };
        let open = open.map(|o| (o, image, limits));
        // Already one vector per band.
        decimate(&mut dec, iw, ih, nbands, planar, decimate_by, open)?
    } else {
        let mut buf = DecodingResult::U8(Vec::new());
        dec.read_image_to_buffer(&mut buf).map_err(err)?;
        let (values, dt) = to_f32(buf);
        if values.len() < n.saturating_mul(nbands) {
            return Err(err("couldn't read every band"));
        }
        let bands: Vec<Vec<f32>> = if nbands == 1 {
            let mut v = values;
            v.truncate(n);
            vec![v]
        } else if planar {
            values.chunks(n).take(nbands).map(<[f32]>::to_vec).collect()
        } else {
            // One pass over the interleaved pixels.
            let mut bands: Vec<Vec<f32>> = (0..nbands).map(|_| Vec::with_capacity(n)).collect();
            for px in values.chunks(nbands).take(n) {
                for (band, v) in bands.iter_mut().zip(px) {
                    band.push(*v);
                }
            }
            drop(values);
            bands
        };
        (bands, dt)
    };

    let mut transform = geo.transform;
    if decimate_by > 1 {
        // Every k-th pixel: pixels k times the size, from the same corner.
        transform.pixel_width *= f64::from(decimate_by);
        transform.pixel_height *= f64::from(decimate_by);
    } else if overview.is_some() {
        transform.pixel_width *= full_w as f64 / w.max(1) as f64;
        transform.pixel_height *= full_h as f64 / h.max(1) as f64;
    }
    let crs = match geo.crs {
        Some(c) => c,
        None => {
            let right = transform.origin_x + transform.pixel_width * w as f64;
            let bottom = transform.origin_y + transform.pixel_height * h as f64;
            let degrees = [transform.origin_x, right].iter().all(|x| (-180.0..=180.0).contains(x))
                && [transform.origin_y, bottom].iter().all(|y| (-90.0..=90.0).contains(y));
            if let Some(code) = geo.unknown_epsg {
                return Err(err(format!("the coordinate system EPSG:{code} isn't supported")));
            }
            if let Some(e) = &geo.custom_error
                && !degrees
            {
                return Err(DataError::Format(e.clone()));
            }
            if geo.geographic_model || degrees {
                Crs::LonLat
            } else {
                return Err(err("the file's coordinate system isn't recognised"));
            }
        }
    };
    let mut r = Raster::new(name, w, h, bands, transform, crs, geo.nodata, color, data_type)?;
    r.overview = overview;
    Ok(r)
}

/// Writes a raster as a Deflate-compressed GeoTIFF: RGB(A) bytes for 8-bit colour rasters,
/// otherwise `band` (0-based) as 32-bit floats.
pub fn write(r: &Raster, band: usize) -> Result<Vec<u8>> {
    use tiff::encoder::{Compression, DeflateLevel, TiffEncoder, colortype};
    let mut out = std::io::Cursor::new(Vec::new());
    let mut enc = TiffEncoder::new(&mut out).map_err(err)?.with_compression(Compression::Deflate(DeflateLevel::Balanced));
    let gt = r.transform;
    if gt.pixel_height > 0.0 || gt.pixel_width < 0.0 {
        return Err(err("only north-up rasters can be exported for now"));
    }
    let scale = [gt.pixel_width.abs(), gt.pixel_height.abs(), 0.0];
    let tie = [0.0, 0.0, 0.0, gt.origin_x, gt.origin_y, 0.0];
    let geographic = r.crs.is_geographic();
    // A file's own projection is written back as the keys it came in.
    let custom = r.crs.custom_def();
    let keys: Vec<u16> = match &custom {
        // A system read from a saved project has no keys to write.
        Some(d) if d.geokeys.is_empty() => {
            return Err(err(format!("{}: a coordinate system without GeoTIFF keys can't be written", d.label)));
        }
        Some(d) => d.geokeys.clone(),
        None => vec![
            1,
            1,
            0,
            3,
            1024,
            0,
            1,
            if geographic { 2 } else { 1 },
            1025,
            0,
            1,
            1,
            if geographic { 2048 } else { 3072 },
            0,
            1,
            r.crs.epsg(),
        ],
    };
    let (w, h) = (r.width, r.height);
    let bytes = |b: usize| -> Option<Vec<u8>> {
        let band = r.bands.get(b)?;
        let s = &band.stats;
        // Whole numbers within 0–255 (or no valid pixels at all) fit in a byte, and so must the
        // no-data value, which no-data pixels keep so the written GDAL_NODATA tag still matches.
        let whole_bytes = s.min >= 0.0 && s.max <= 255.0 && (s.categories.is_some() || r.data_type == "UInt8");
        let nodata = match r.nodata {
            Some(d) if (0.0..=255.0).contains(&d) && d.fract() == 0.0 => Some(d as u8),
            Some(_) => return None,
            None => None,
        };
        (whole_bytes || s.count == 0)
            .then(|| band.data.iter().map(|v| if r.is_valid(*v) { v.clamp(0.0, 255.0) as u8 } else { nodata.unwrap_or(0) }).collect())
    };
    let rgb = matches!(r.color, ColorHint::Rgb { .. }) && r.bands.len() >= 3;
    macro_rules! tags {
        ($img:expr) => {{
            let e = $img.encoder();
            e.write_tag(Tag::ModelPixelScaleTag, &scale[..]).map_err(err)?;
            e.write_tag(Tag::ModelTiepointTag, &tie[..]).map_err(err)?;
            e.write_tag(Tag::GeoKeyDirectoryTag, &keys[..]).map_err(err)?;
            if let Some(d) = &custom {
                if !d.doubles.is_empty() {
                    e.write_tag(Tag::Unknown(34736), &d.doubles[..]).map_err(err)?;
                }
                if !d.ascii.is_empty() {
                    e.write_tag(Tag::Unknown(34737), d.ascii.as_str()).map_err(err)?;
                }
            }
            if let Some(nd) = r.nodata {
                e.write_tag(Tag::GdalNodata, nd.to_string().as_str()).map_err(err)?;
            }
        }};
    }
    if rgb && let (Some(red), Some(green), Some(blue)) = (bytes(0), bytes(1), bytes(2)) {
        let alpha = if r.bands.len() >= 4 { bytes(3) } else { None };
        let n = (w as usize) * (h as usize);
        if let Some(a) = alpha {
            let mut data = Vec::with_capacity(n * 4);
            for i in 0..n {
                data.extend([red.get(i), green.get(i), blue.get(i), a.get(i)].map(|v| v.copied().unwrap_or(0)));
            }
            let mut img = enc.new_image::<colortype::RGBA8>(w, h).map_err(err)?;
            tags!(img);
            img.write_data(&data).map_err(err)?;
        } else {
            let mut data = Vec::with_capacity(n * 3);
            for i in 0..n {
                data.extend([red.get(i), green.get(i), blue.get(i)].map(|v| v.copied().unwrap_or(0)));
            }
            let mut img = enc.new_image::<colortype::RGB8>(w, h).map_err(err)?;
            tags!(img);
            img.write_data(&data).map_err(err)?;
        }
    } else {
        let b = r.bands.get(band).ok_or_else(|| err(format!("the raster has no band {}", band + 1)))?;
        let mut img = enc.new_image::<colortype::Gray32Float>(w, h).map_err(err)?;
        tags!(img);
        img.write_data(&b.data).map_err(err)?;
    }
    Ok(out.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::raster::tests::small;

    #[test]
    fn float_round_trip() {
        let r = small();
        let bytes = write(&r, 0).unwrap();
        let back = read(std::io::Cursor::new(&bytes), "back").unwrap();
        assert_eq!((back.width, back.height), (4, 3));
        assert_eq!(back.crs, Crs::LonLat);
        assert_eq!(back.nodata, Some(-9999.0));
        assert_eq!(back.transform, r.transform);
        assert_eq!(back.value(0, 2, 1), Some(7.0));
        assert_eq!(back.value(0, 1, 1), None);
        assert_eq!(back.data_type, "Float32");
    }

    #[test]
    fn rgb_round_trip_in_utm() {
        let gt = GeoTransform { origin_x: 500_000.0, origin_y: 4_000_000.0, pixel_width: 30.0, pixel_height: -30.0 };
        let bands = vec![vec![10.0, 20.0], vec![30.0, 40.0], vec![50.0, 60.0]];
        let r =
            Raster::new("rgb", 2, 1, bands, gt, Crs::from_epsg(32617).unwrap(), None, ColorHint::Rgb { alpha: false }, "UInt8").unwrap();
        let back = read(std::io::Cursor::new(write(&r, 0).unwrap()), "back").unwrap();
        assert_eq!(back.band_count(), 3);
        assert_eq!(back.crs.epsg(), 32617);
        assert_eq!(back.color, ColorHint::Rgb { alpha: false });
        assert_eq!(back.value(2, 1, 0), Some(60.0));
        assert_eq!(back.data_type, "UInt8");
    }

    #[test]
    fn rgb_export_keeps_no_data() {
        let gt = GeoTransform { origin_x: 0.0, origin_y: 2.0, pixel_width: 1.0, pixel_height: -1.0 };
        let mk = |nodata: f64| {
            let bands = vec![vec![10.0, nodata as f32], vec![20.0, nodata as f32], vec![30.0, nodata as f32]];
            Raster::new("rgb", 2, 1, bands, gt, Crs::LonLat, Some(nodata), ColorHint::Rgb { alpha: false }, "UInt8").unwrap()
        };
        // A byte no-data value survives on the byte path.
        let back = read(std::io::Cursor::new(write(&mk(255.0), 0).unwrap()), "b").unwrap();
        assert_eq!((back.band_count(), back.value(0, 1, 0), back.value(0, 0, 0)), (3, None, Some(10.0)));
        // One that doesn't fit a byte keeps the file honest by writing floats (band 1 only).
        let back = read(std::io::Cursor::new(write(&mk(-9999.0), 0).unwrap()), "f").unwrap();
        assert_eq!((back.band_count(), back.value(0, 1, 0), back.nodata), (1, None, Some(-9999.0)));
    }

    #[test]
    fn south_up_export_is_refused() {
        let gt = GeoTransform { origin_x: 0.0, origin_y: 0.0, pixel_width: 1.0, pixel_height: 1.0 };
        let r = Raster::new("s", 1, 1, vec![vec![1.0]], gt, Crs::LonLat, None, ColorHint::Gray, "Float32").unwrap();
        assert!(write(&r, 0).is_err());
    }

    #[test]
    fn plain_tiff_is_not_georeferenced() {
        use tiff::encoder::{TiffEncoder, colortype};
        let mut out = std::io::Cursor::new(Vec::new());
        TiffEncoder::new(&mut out).unwrap().write_image::<colortype::Gray8>(2, 2, &[1, 2, 3, 4]).unwrap();
        let e = read(std::io::Cursor::new(out.into_inner()), "x").unwrap_err();
        assert!(e.to_string().contains("georeferenced"), "{e}");
    }

    #[test]
    fn junk_never_panics() {
        let r = small();
        let good = write(&r, 0).unwrap();
        for cut in [0, 8, 16, 100, good.len() / 2, good.len() - 1] {
            let _ = read(std::io::Cursor::new(&good[..cut]), "x");
        }
        let mut flipped = good.clone();
        for i in (8..flipped.len()).step_by(7) {
            flipped[i] ^= 0x5a;
        }
        let _ = read(std::io::Cursor::new(&flipped), "x");
    }

    /// A grey 8-bit GeoTIFF in longitude/latitude, from the encoder, with `extra` tags.
    fn grey8(w: u32, h: u32, data: &[u8], colormap: Option<&[u16]>) -> Vec<u8> {
        use tiff::encoder::{TiffEncoder, colortype};
        let mut out = std::io::Cursor::new(Vec::new());
        {
            let mut enc = TiffEncoder::new(&mut out).unwrap();
            let mut img = enc.new_image::<colortype::Gray8>(w, h).unwrap();
            let e = img.encoder();
            e.write_tag(Tag::ModelPixelScaleTag, &[0.5f64, 0.5, 0.0][..]).unwrap();
            e.write_tag(Tag::ModelTiepointTag, &[0.0f64, 0.0, 0.0, 10.0, 50.0, 0.0][..]).unwrap();
            e.write_tag(Tag::GeoKeyDirectoryTag, &[1u16, 1, 0, 2, 1024, 0, 1, 2, 2048, 0, 1, 4326][..]).unwrap();
            if let Some(m) = colormap {
                e.write_tag(Tag::ColorMap, m).unwrap();
            }
            img.rows_per_strip(4).unwrap();
            img.write_data(data).unwrap();
        }
        out.into_inner()
    }

    /// Sets a classic little-endian TIFF's PhotometricInterpretation (as a palette writer would).
    fn set_photometric(b: &mut [u8], value: u16) {
        let ifd = u32::from_le_bytes(b[4..8].try_into().unwrap()) as usize;
        let n = u16::from_le_bytes(b[ifd..ifd + 2].try_into().unwrap()) as usize;
        for i in 0..n {
            let e = ifd + 2 + i * 12;
            if u16::from_le_bytes(b[e..e + 2].try_into().unwrap()) == 262 {
                b[e + 8..e + 10].copy_from_slice(&value.to_le_bytes());
            }
        }
    }

    /// Recompresses a classic little-endian TIFF's strips with Zstandard (as GDAL's
    /// `COMPRESS=ZSTD` does), appending them and repointing the strip offsets.
    #[cfg(not(target_arch = "wasm32"))]
    fn zstd_strips(b: &mut Vec<u8>) {
        let u16_at = |b: &[u8], i: usize| u16::from_le_bytes(b[i..i + 2].try_into().unwrap());
        let u32_at = |b: &[u8], i: usize| u32::from_le_bytes(b[i..i + 4].try_into().unwrap()) as usize;
        let ifd = u32_at(b, 4);
        // A tag's directory entry.
        let entry = |b: &[u8], tag: u16| (0..u16_at(b, ifd) as usize).map(|i| ifd + 2 + i * 12).find(|&e| u16_at(b, e) == tag).unwrap();
        let (offsets, counts) = (entry(b, 273), entry(b, 279));
        assert_eq!((u16_at(b, offsets + 2), u16_at(b, counts + 2)), (4, 4), "LONG strip tags");
        let n = u32_at(b, offsets + 4);
        let at = |b: &[u8], e: usize| if n == 1 { e + 8 } else { u32_at(b, e + 8) };
        let (offsets, counts) = (at(b, offsets), at(b, counts));
        for i in 0..n {
            let (start, len) = (u32_at(b, offsets + i * 4), u32_at(b, counts + i * 4));
            let packed = zstd::encode_all(&b[start..start + len], 3).unwrap();
            let new_start = u32::try_from(b.len()).unwrap();
            let new_len = u32::try_from(packed.len()).unwrap();
            b.extend(packed);
            b[offsets + i * 4..offsets + i * 4 + 4].copy_from_slice(&new_start.to_le_bytes());
            b[counts + i * 4..counts + i * 4 + 4].copy_from_slice(&new_len.to_le_bytes());
        }
        let compression = entry(b, 259);
        b[compression + 8..compression + 10].copy_from_slice(&50000u16.to_le_bytes());
    }

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn zstd_compressed_files_are_read() {
        // Issue #134: Zstandard is a usual compression for COGs (EnMAP's, among others).
        let (w, h) = (8u32, 8u32);
        let data: Vec<u8> = (0..w * h).map(|i| (i * 3 % 251) as u8).collect();
        let mut bytes = grey8(w, h, &data, None);
        zstd_strips(&mut bytes);
        let r = read(std::io::Cursor::new(&bytes), "zstd").unwrap();
        assert_eq!((r.width, r.height), (8, 8));
        for (x, y) in [(0i64, 0i64), (7, 0), (3, 5), (7, 7)] {
            assert_eq!(r.value(0, x, y), Some(f32::from(data[(y * 8 + x) as usize])), "{x},{y}");
        }
    }

    /// Reads `file` as the web app does: parts of `chunk` bytes, as [`read_sparse_limited`] asks
    /// for them. Returns the raster and the parts read.
    fn read_in_parts(file: &[u8], chunk: u64, max: usize) -> (Raster, Sparse) {
        let len = file.len() as u64;
        let mut parts = Sparse::default().with_len(len).with(0, file[..chunk.min(len) as usize].to_vec());
        for _ in 0..1000 {
            match read_sparse_limited(&parts, "parts", max).unwrap() {
                Partial::Done(r) => return (*r, parts),
                Partial::Need(a, b) => {
                    assert!(a < len, "a read past the end: {a}..{b}");
                    let end = b.max(a + chunk).min(len).min(parts.next_start(a).unwrap_or(len));
                    parts = parts.with(a, file[a as usize..end as usize].to_vec());
                }
            }
        }
        panic!("too many rounds");
    }

    #[test]
    fn a_cog_read_in_parts_reads_only_its_overview() {
        // 2048 pixels a side in 256-pixel tiles, with overviews of 1024, 512 and 256; at most a
        // million samples, so the 1024 overview is read, and nothing of the full image's tiles.
        let file = super::super::cog::example(2048, 256);
        let whole = read_limited(reader(std::io::Cursor::new(&file)).unwrap(), "whole", 1 << 20, None).unwrap();
        assert_eq!((whole.width, whole.height), (1024, 1024));
        let (r, parts) = read_in_parts(&file, 64 << 10, 1 << 20);
        assert_eq!((r.width, r.height, r.overview), (whole.width, whole.height, whole.overview));
        assert_eq!(r.transform, whole.transform);
        for (x, y) in [(0i64, 0i64), (1023, 0), (517, 300), (1023, 1023)] {
            assert_eq!(r.value(0, x, y), whole.value(0, x, y), "{x},{y}");
        }
        // The full image is 4 MB of the file's 5.6 MB; well under that is read.
        assert!(parts.held() < file.len() as u64 / 3, "{} of {}", parts.held(), file.len());
        // Kept as a container and read back (with the same limit), the same raster.
        let kept = Sparse::from_container(&parts.container()).unwrap();
        let Partial::Done(back) = read_sparse_limited(&kept, "back", 1 << 20).unwrap() else { panic!("parts missing") };
        assert_eq!((back.width, back.height), (r.width, r.height));
        assert_eq!(back.value(0, 517, 300), r.value(0, 517, 300));
    }

    #[test]
    fn a_striped_file_read_in_parts_is_read_to_its_end() {
        // No overviews and in strips: every strip is needed, and reads stop at the file's end.
        let (w, h) = (301u32, 203u32);
        let data: Vec<u8> = (0..w * h).map(|i| ((i % w + i / w) % 251) as u8).collect();
        let bytes = grey8(w, h, &data, None);
        let whole = read_limited(reader(std::io::Cursor::new(&bytes)).unwrap(), "big", 10_000, None).unwrap();
        let (r, _) = read_in_parts(&bytes, 1000, 10_000);
        assert_eq!((r.width, r.height), (whole.width, whole.height));
        assert_eq!(r.value(0, 30, 7), whole.value(0, 30, 7));
    }

    #[test]
    fn a_part_container_opens_as_a_tif() {
        let file = super::super::cog::example(512, 256);
        let (r, parts) = read_in_parts(&file, 4096, MAX_SAMPLES);
        let back = super::super::read_bytes("parts.tif", &parts.container()).unwrap();
        assert_eq!((back.width, back.height, back.value(0, 300, 200)), (r.width, r.height, r.value(0, 300, 200)));
    }

    #[test]
    fn damaged_part_containers_never_panic() {
        let file = super::super::cog::example(512, 256);
        let (_, parts) = read_in_parts(&file, 4096, MAX_SAMPLES);
        let good = parts.container();
        for cut in (0..good.len()).step_by(97) {
            let _ = super::super::read_bytes("x.tif", &good[..cut]);
        }
        let mut flipped = good.clone();
        for i in (16..flipped.len()).step_by(13) {
            flipped[i] ^= 0x5a;
        }
        let _ = super::super::read_bytes("x.tif", &flipped);
        // A container that says it holds more parts than it does is refused.
        let mut lying = good[..32].to_vec();
        lying[24..32].copy_from_slice(&u64::MAX.to_le_bytes());
        assert!(Sparse::from_container(&lying).is_none());
        // Parts at the largest offsets, or past the file's end, are refused or read safely.
        let container = |len: u64, at: u64, part: &[u8]| {
            let mut c = good[..16].to_vec();
            for v in [len, 1, at, part.len() as u64] {
                c.extend_from_slice(&v.to_le_bytes());
            }
            c.extend_from_slice(part);
            c
        };
        assert!(Sparse::from_container(&container(10, 8, b"abcd")).is_none());
        assert!(Sparse::from_container(&container(u64::MAX, u64::MAX - 1, b"abcd")).is_none());
        let near_end = Sparse::from_container(&container(u64::MAX, u64::MAX - 8, b"abcd")).unwrap();
        let mut r = near_end.reader();
        assert!(std::io::Seek::seek(&mut r, std::io::SeekFrom::End(0)).is_ok());
        let _ = super::super::read_bytes("x.tif", &container(u64::MAX, u64::MAX - 8, b"II*\0"));
    }

    #[test]
    fn palette_images_read_their_colour_table() {
        let data: Vec<u8> = (0..64u32).map(|i| (i % 3) as u8).collect();
        // A 256-entry map: index 1 red, 2 green.
        let mut map = vec![0u16; 768];
        map[1] = 65535;
        map[256 + 2] = 65535;
        let mut bytes = grey8(8, 8, &data, Some(&map));
        set_photometric(&mut bytes, 3);
        let r = read(std::io::Cursor::new(&bytes), "pal").unwrap();
        match &r.color {
            ColorHint::Palette(t) => {
                assert_eq!(t[1], [255, 0, 0, 255]);
                assert_eq!(t[2], [0, 255, 0, 255]);
            }
            other => panic!("not a palette: {other:?}"),
        }
        assert_eq!(r.value(0, 1, 0), Some(1.0));
        assert_eq!(r.value(0, 2, 0), Some(2.0));
        // Every truncation of a palette file is an error, not a panic.
        for n in (0..bytes.len()).step_by(11) {
            let _ = read(std::io::Cursor::new(&bytes[..n]), "x");
        }
    }

    #[test]
    fn large_files_without_overviews_are_read_as_one() {
        // Over a 10,000-sample limit, so every 4th pixel each way is kept.
        // 301 × 203 in 4-row strips: the last strip is short, the width odd.
        let (w, h) = (301u32, 203u32);
        let data: Vec<u8> = (0..w * h).map(|i| ((i % w + i / w) % 251) as u8).collect();
        let bytes = grey8(w, h, &data, None);
        let full = read(std::io::Cursor::new(&bytes), "full").unwrap();
        let r = read_limited(reader(std::io::Cursor::new(&bytes)).unwrap(), "big", 10_000, None).unwrap();
        assert_eq!((r.width, r.height), (76, 51));
        assert_eq!(r.overview, Some((301, 203)));
        assert_eq!(r.transform.pixel_width, full.transform.pixel_width * 4.0);
        assert_eq!((r.transform.origin_x, r.transform.origin_y), (full.transform.origin_x, full.transform.origin_y));
        for (x, y) in [(0i64, 0i64), (1, 1), (75, 50), (74, 49), (30, 7)] {
            assert_eq!(r.value(0, x, y), full.value(0, x * 4, y * 4), "{x},{y}");
        }
    }

    /// Reads `bytes` as from a file, decoding its chunks on several threads.
    fn read_threaded(bytes: &[u8], max: usize) -> Result<Raster> {
        let bytes = std::sync::Arc::new(bytes.to_vec());
        let open = move || reader(std::io::Cursor::new(bytes.as_slice().to_vec()));
        read_limited(open().unwrap(), "threaded", max, Some(&open))
    }

    #[test]
    fn chunks_decoded_on_several_threads_match_one() {
        // 1031 × 1029 in 4-row strips: 258 strips, over a million pixels, so read a chunk at a
        // time; the last strip short, the width odd.
        let (w, h) = (1031u32, 1029u32);
        let data: Vec<u8> = (0..w * h).map(|i| ((i % w * 7 + i / w * 3) % 253) as u8).collect();
        let bytes = grey8(w, h, &data, None);
        let one = read(std::io::Cursor::new(&bytes), "one").unwrap();
        let many = read_threaded(&bytes, MAX_SAMPLES).unwrap();
        assert_eq!((many.width, many.height), (w, h));
        assert!(many.bands.iter().zip(&one.bands).all(|(a, b)| a.data == b.data) && many.bands.len() == one.bands.len());
        assert_eq!(many.data_type, one.data_type);
        // Made smaller as it's read: every 4th pixel, as on one thread.
        let one = read_limited(reader(std::io::Cursor::new(&bytes)).unwrap(), "one", 100_000, None).unwrap();
        let many = read_threaded(&bytes, 100_000).unwrap();
        assert_eq!((many.width, many.height, many.overview), (one.width, one.height, Some((w, h))));
        assert!(many.bands.iter().zip(&one.bands).all(|(a, b)| a.data == b.data) && many.bands.len() == one.bands.len());
        assert_eq!(many.transform, one.transform);
    }

    #[test]
    fn a_damaged_chunk_fails_the_threaded_read() {
        let (w, h) = (1031u32, 1029u32);
        let data = vec![7u8; (w * h) as usize];
        // Cut off partway: the strips past the end can't be read.
        let bytes = grey8(w, h, &data, None);
        let cut = &bytes[..bytes.len() / 2];
        assert!(read_threaded(cut, MAX_SAMPLES).is_err());
        assert!(read_threaded(cut, 100_000).is_err());
    }

    /// A tiled, uncompressed RGB GeoTIFF with a fourth, undefined sample (an RGBI orthophoto's
    /// layout), each image `(size, tile)` in turn: the first the full one, the rest overviews of
    /// it. Sample `b` of pixel (x, y) at a `k`-times overview is `pixel(b, x·k, y·k)`; edge tiles
    /// are padded with zeros.
    fn rgbi_tiled(images: &[(u32, u32)], pixel: impl Fn(u32, u32, u32) -> u8) -> Vec<u8> {
        let full = images.first().map_or(1, |i| i.0);
        let mut out = vec![b'I', b'I', 42, 0, 0, 0, 0, 0];
        let mut next_link = 4;
        for (i, &(size, tile)) in images.iter().enumerate() {
            let k = full / size;
            let n = size.div_ceil(tile);
            // The tiles' data first, then the IFD pointing at it.
            let mut offsets = Vec::new();
            for t in 0..n * n {
                offsets.push(out.len() as u32);
                for p in 0..tile * tile {
                    let (x, y) = ((t % n) * tile + p % tile, (t / n) * tile + p / tile);
                    for b in 0..4 {
                        out.push(if x < size && y < size { pixel(b, x * k, y * k) } else { 0 });
                    }
                }
            }
            // Out-of-line values: (tag, type, count, bytes); type 3 short, 4 long, 12 double.
            let shorts = |v: &[u16]| v.iter().flat_map(|x| x.to_le_bytes()).collect::<Vec<u8>>();
            let longs = |v: &[u32]| v.iter().flat_map(|x| x.to_le_bytes()).collect::<Vec<u8>>();
            let doubles = |v: &[f64]| v.iter().flat_map(|x| x.to_le_bytes()).collect::<Vec<u8>>();
            let counts = vec![tile * tile * 4; offsets.len()];
            let mut e: Vec<(u16, u16, u32, Vec<u8>)> = vec![
                (254, 4, 1, longs(&[u32::from(i > 0)])),
                (256, 4, 1, longs(&[size])),
                (257, 4, 1, longs(&[size])),
                (258, 3, 4, shorts(&[8; 4])),
                (259, 3, 1, shorts(&[1])),
                (262, 3, 1, shorts(&[2])),
                (277, 3, 1, shorts(&[4])),
                (284, 3, 1, shorts(&[1])),
                (322, 4, 1, longs(&[tile])),
                (323, 4, 1, longs(&[tile])),
                (324, 4, offsets.len() as u32, longs(&offsets)),
                (325, 4, counts.len() as u32, longs(&counts)),
                (338, 3, 1, shorts(&[0])),
            ];
            if i == 0 {
                e.push((33550, 12, 3, doubles(&[0.2, 0.2, 0.0])));
                e.push((33922, 12, 6, doubles(&[0.0, 0.0, 0.0, 641_000.0, 5_650_000.0, 0.0])));
                e.push((34735, 3, 8, shorts(&[1, 1, 0, 1, 3072, 0, 1, 25832])));
            }
            let mut values: Vec<u8> = Vec::new();
            let ifd_at = out.len() + out.len() % 2;
            out.resize(ifd_at, 0);
            let values_at = ifd_at + 2 + 12 * e.len() + 4;
            let mut dir = (e.len() as u16).to_le_bytes().to_vec();
            for (tag, ty, count, bytes) in &mut e {
                dir.extend(tag.to_le_bytes());
                dir.extend(ty.to_le_bytes());
                dir.extend(count.to_le_bytes());
                if bytes.len() <= 4 {
                    bytes.resize(4, 0);
                    dir.extend(bytes.iter());
                } else {
                    dir.extend(((values_at + values.len()) as u32).to_le_bytes());
                    values.extend(bytes.iter());
                }
            }
            out[next_link..next_link + 4].copy_from_slice(&(ifd_at as u32).to_le_bytes());
            next_link = ifd_at + dir.len();
            out.extend(dir);
            out.extend([0; 4]);
            out.extend(values);
        }
        out
    }

    #[test]
    fn padded_tiles_with_an_extra_sample_stay_in_their_rows() {
        // Issue #162: an RGBI orthophoto (5000 pixels in 128-pixel tiles) opened with black
        // columns down its west edge, the padding of each row's last tile spilt into the next row.
        let pixel = |b: u32, x: u32, y: u32| ((x + 3 * y + 60 * b) % 250 + 1) as u8;
        let bytes = rgbi_tiled(&[(40, 16), (20, 16)], pixel);
        let check = |r: &Raster, k: u32| {
            assert_eq!((r.band_count(), r.color.clone()), (3, ColorHint::Rgb { alpha: false }));
            for y in 0..r.height {
                for x in 0..r.width {
                    for b in 0..3 {
                        let want = f32::from(pixel(b, x * k, y * k));
                        assert_eq!(r.value(b as usize, i64::from(x), i64::from(y)), Some(want), "band {b} at {x},{y} (k {k})");
                    }
                }
            }
        };
        // The full image, and the overview a smaller limit (the browser's) opens instead.
        let r = read(std::io::Cursor::new(&bytes), "rgbi").unwrap();
        assert_eq!((r.width, r.height, r.overview), (40, 40, None));
        check(&r, 1);
        let r = read_limited(reader(std::io::Cursor::new(&bytes)).unwrap(), "rgbi", 2_000, None).unwrap();
        assert_eq!((r.width, r.height, r.overview), (20, 20, Some((40, 40))));
        check(&r, 2);
        assert_eq!(r.crs.epsg(), 25832);
        // With only a coarse overview (the orthophoto's is 157 pixels), every 2nd pixel of the
        // full image is read instead: four times the detail of the overview.
        let coarse = rgbi_tiled(&[(40, 16), (10, 16)], pixel);
        let r = read_limited(reader(std::io::Cursor::new(&coarse)).unwrap(), "rgbi", 2_000, None).unwrap();
        assert_eq!((r.width, r.height, r.overview), (20, 20, Some((40, 40))));
        check(&r, 2);
    }

    #[test]
    fn a_files_own_projection_is_read_and_written_back() {
        // The NLCD's Albers, as its GeoKeys give it.
        let keys = [
            1u16, 1, 0, 11, 1024, 0, 1, 1, 1025, 0, 1, 1, 2048, 0, 1, 4326, 3072, 0, 1, 32767, 3074, 0, 1, 32767, 3075, 0, 1, 11, 3076, 0,
            1, 9001, 3078, 34736, 1, 0, 3079, 34736, 1, 1, 3080, 34736, 1, 2, 3081, 34736, 1, 3, 3082, 34736, 1, 4,
        ];
        let doubles = [29.5f64, 45.5, -96.0, 23.0, 0.0];
        let mut tiff = Vec::new();
        {
            use tiff::encoder::{TiffEncoder, colortype};
            let mut enc = TiffEncoder::new(std::io::Cursor::new(&mut tiff)).unwrap();
            let mut img = enc.new_image::<colortype::Gray32Float>(2, 2).unwrap();
            let e = img.encoder();
            e.write_tag(Tag::ModelPixelScaleTag, &[30.0f64, 30.0, 0.0][..]).unwrap();
            e.write_tag(Tag::ModelTiepointTag, &[0.0f64, 0.0, 0.0, 0.0, 60.0, 0.0][..]).unwrap();
            e.write_tag(Tag::GeoKeyDirectoryTag, &keys[..]).unwrap();
            e.write_tag(Tag::Unknown(34736), &doubles[..]).unwrap();
            img.write_data(&[1.0f32, 2.0, 3.0, 4.0]).unwrap();
        }
        let r = read(std::io::Cursor::new(&tiff), "albers").unwrap();
        assert!(matches!(r.crs, Crs::Custom(_)), "{:?}", r.crs);
        assert!(r.crs.custom_def().unwrap().proj4.contains("+proj=aea"));
        // The projection's origin is at 96° W, 23° N.
        let ll = r.crs.transformer().unwrap().to_lonlat(rustgis_geom::Coord::new(0.0, 0.0)).unwrap();
        assert!((ll.x + 96.0).abs() < 1e-6 && (ll.y - 23.0).abs() < 1e-6, "{ll:?}");
        // Written and read again, the same system.
        let back = read(std::io::Cursor::new(write(&r, 0).unwrap()), "back").unwrap();
        assert_eq!(back.crs.custom_def().unwrap().proj4, r.crs.custom_def().unwrap().proj4);
        assert_eq!(back.value(0, 1, 1), Some(4.0));
        // Saved, it is its definition, not this run's number for it.
        let json = serde_json::to_string(&r.crs).unwrap();
        assert!(json.contains("+proj=aea"), "{json}");
        let again: Crs = serde_json::from_str(&json).unwrap();
        assert_eq!(again.custom_def().unwrap().proj4, r.crs.custom_def().unwrap().proj4);
        // False eastings in US feet become metres for PROJ.
        let mut feet = keys.to_vec();
        if let Some(i) = feet.chunks(4).position(|e| e[0] == 3076) {
            feet[i * 4 + 3] = 9003;
        }
        let ft = custom_crs(&feet, &[29.5, 45.5, -96.0, 23.0, 1000.0], "").unwrap();
        let p = ft.custom_def().unwrap().proj4.clone();
        assert!(p.contains("+x_0=304.80060960") && p.contains("+units=us-ft"), "{p}");
        // A transformation it doesn't know: unknown, not a failed open, when the coordinates are degrees.
        let mut odd = keys;
        odd[27] = 99; // ProjCoordTransGeoKey
        assert!(custom_crs(&odd, &doubles, "").is_err());
    }
}
