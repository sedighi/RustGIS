//! Cloud-optimised GeoTIFFs read in place: the layout of a tiled GeoTIFF (its images, their tile
//! grids and where each tile is) from the bytes read so far, and one tile at a time decoded from
//! its own bytes. Nothing here fetches: [`Sparse`] holds the byte ranges the caller has read, and a
//! read past them says which range is needed next (the tile cache fetches it, by HTTP range).
//!
//! Files are hostile input: image counts, tile counts and sizes are bounded, and a tile is decoded
//! with the same limits as a whole GeoTIFF.

use std::collections::BTreeMap;
use std::io::{Read, Seek, SeekFrom};
use std::sync::{Arc, Mutex};

use tiff::ColorType;
use tiff::decoder::{Decoder, DecodingResult, Limits};
use tiff::tags::Tag;

use super::{ColorHint, Crs, GeoTransform};
use crate::{DataError, Result};

/// The most images (the full one and its overviews) looked at.
const MAX_LEVELS: usize = 32;
/// The most tiles an image may have.
const MAX_TILES: usize = 4 << 20;
/// How many tiles all the levels hold together (their offsets and lengths are kept, 16 bytes a
/// tile): about 128 MB at most.
const MAX_TOTAL_TILES: usize = 8 << 20;
/// The largest tile, in samples.
const MAX_TILE_SAMPLES: usize = 4 << 20;
/// The most bytes held from a file's header (its IFDs and their tag arrays).
pub const MAX_HEADER: u64 = 32 << 20;

/// Byte ranges read from a file, as a reader: a read outside them fails and says what to read.
#[derive(Clone, Default)]
pub struct Sparse {
    chunks: Arc<BTreeMap<u64, Arc<Vec<u8>>>>,
    pos: u64,
    /// The first range a read needed and didn't have.
    missing: Arc<Mutex<Option<(u64, u64)>>>,
    /// The file's length, when known: reads past it find its end, not a gap.
    len: Option<u64>,
}

/// What a container of a file's parts ([`Sparse::container`]) starts with.
const CONTAINER_MAGIC: &[u8; 16] = b"RustGIS parts 1\n";
/// The most parts a container may hold.
const MAX_PARTS: u64 = 1 << 20;

impl Sparse {
    /// The bytes read so far, starting at `at`.
    pub fn with(mut self, at: u64, bytes: Vec<u8>) -> Sparse {
        Arc::make_mut(&mut self.chunks).insert(at, Arc::new(bytes));
        self
    }

    /// The same bytes, of a file `len` bytes long.
    pub fn with_len(mut self, len: u64) -> Sparse {
        self.len = Some(len);
        self
    }

    /// Where the next part held after `at` starts, if any.
    pub fn next_start(&self, at: u64) -> Option<u64> {
        self.chunks.range(at.saturating_add(1)..).next().map(|(a, _)| *a)
    }

    /// The parts held, as one file: the file's length, then each part's start, length and bytes
    /// (read by [`Sparse::from_container`]). A browser keeps the parts of a large GeoTIFF it read
    /// so, as a raster layer's file and in a saved project, rather than the whole file.
    pub fn container(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(32 + self.chunks.values().map(|c| c.len() + 16).sum::<usize>());
        out.extend_from_slice(CONTAINER_MAGIC);
        out.extend_from_slice(&self.len.unwrap_or(u64::MAX).to_le_bytes());
        out.extend_from_slice(&(self.chunks.len() as u64).to_le_bytes());
        for (at, c) in self.chunks.iter() {
            out.extend_from_slice(&at.to_le_bytes());
            out.extend_from_slice(&(c.len() as u64).to_le_bytes());
            out.extend_from_slice(c);
        }
        out
    }

    /// Whether `bytes` are a container of a file's parts.
    pub fn is_container(bytes: &[u8]) -> bool {
        bytes.starts_with(CONTAINER_MAGIC)
    }

    /// The parts a container holds; `None` for anything else or a damaged one.
    pub fn from_container(bytes: &[u8]) -> Option<Sparse> {
        fn u64_(rest: &mut &[u8]) -> Option<u64> {
            let (head, tail) = rest.split_first_chunk::<8>()?;
            *rest = tail;
            Some(u64::from_le_bytes(*head))
        }
        let mut rest = bytes.strip_prefix(CONTAINER_MAGIC.as_slice())?;
        let len = u64_(&mut rest)?;
        let n = u64_(&mut rest)?;
        if n > MAX_PARTS {
            return None;
        }
        let mut s = Sparse::default();
        if len != u64::MAX {
            s.len = Some(len);
        }
        for _ in 0..n {
            let (at, size) = (u64_(&mut rest)?, u64_(&mut rest)?);
            // A part past the file's end (or the largest offset) is a damaged container.
            let end = at.checked_add(size)?;
            if s.len.is_some_and(|len| end > len) {
                return None;
            }
            let size = usize::try_from(size).ok()?;
            if size > rest.len() {
                return None;
            }
            let (part, tail) = rest.split_at(size);
            rest = tail;
            s = s.with(at, part.to_vec());
        }
        rest.is_empty().then_some(s)
    }

    /// How many bytes it holds.
    pub fn held(&self) -> u64 {
        self.chunks.values().map(|c| c.len() as u64).sum()
    }

    /// A reader of the same bytes from the start, with its own record of what was missing.
    pub(crate) fn reader(&self) -> Sparse {
        Sparse { chunks: self.chunks.clone(), pos: 0, missing: Arc::new(Mutex::new(None)), len: self.len }
    }

    pub(crate) fn missing(&self) -> Option<(u64, u64)> {
        *self.missing.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl Read for Sparse {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if buf.is_empty() || self.len.is_some_and(|len| self.pos >= len) {
            return Ok(0);
        }
        let hit = self.chunks.range(..=self.pos).next_back().and_then(|(at, c)| {
            let off = usize::try_from(self.pos - at).ok()?;
            (off < c.len()).then(|| c.get(off..)).flatten()
        });
        match hit {
            Some(src) => {
                // Never past the file's end, whatever a part holds.
                let left = self.len.map_or(usize::MAX, |len| usize::try_from(len - self.pos).unwrap_or(usize::MAX));
                let n = src.len().min(buf.len()).min(left);
                buf[..n].copy_from_slice(&src[..n]);
                self.pos = self.pos.saturating_add(n as u64);
                Ok(n)
            }
            None => {
                let mut m = self.missing.lock().unwrap_or_else(|e| e.into_inner());
                if m.is_none() {
                    let end = self.pos.saturating_add(buf.len() as u64);
                    *m = Some((self.pos, self.len.map_or(end, |len| end.min(len))));
                }
                Err(std::io::Error::other("rustgis: bytes not read yet"))
            }
        }
    }
}

impl Seek for Sparse {
    fn seek(&mut self, to: SeekFrom) -> std::io::Result<u64> {
        self.pos = match to {
            SeekFrom::Start(p) => p,
            SeekFrom::Current(d) => self.pos.checked_add_signed(d).ok_or_else(|| std::io::Error::other("seek before the start"))?,
            // The file's length isn't known: the end of what was read stands for it.
            SeekFrom::End(d) => {
                let end = self
                    .len
                    .unwrap_or_else(|| self.chunks.iter().next_back().map(|(at, c)| at.saturating_add(c.len() as u64)).unwrap_or(0));
                end.checked_add_signed(d).ok_or_else(|| std::io::Error::other("seek before the start"))?
            }
        };
        Ok(self.pos)
    }
}

/// One image of the file: the full one or an overview.
#[derive(Clone, Debug, PartialEq)]
pub struct Level {
    /// Its place in the file's chain of images.
    pub ifd: usize,
    pub width: u32,
    pub height: u32,
    pub tile_width: u32,
    pub tile_height: u32,
    /// Where each tile is, and how many bytes it has (row by row).
    pub offsets: Vec<u64>,
    pub counts: Vec<u64>,
}

impl Level {
    pub fn across(&self) -> u32 {
        self.width.div_ceil(self.tile_width.max(1))
    }
    pub fn down(&self) -> u32 {
        self.height.div_ceil(self.tile_height.max(1))
    }
}

/// What a tiled GeoTIFF holds and where.
#[derive(Clone, Debug)]
pub struct Layout {
    /// The full image first, then its overviews from finest to coarsest.
    pub levels: Vec<Level>,
    pub crs: Crs,
    /// The full image's georeferencing.
    pub transform: GeoTransform,
    pub nodata: Option<f64>,
    pub bands: usize,
    pub color: ColorHint,
    pub data_type: &'static str,
}

impl Layout {
    /// A level's georeferencing: the full image's, with pixels as much larger as the level is
    /// smaller.
    pub fn transform_of(&self, level: &Level) -> GeoTransform {
        let full = self.levels.first();
        let (fw, fh) = full.map(|l| (l.width, l.height)).unwrap_or((level.width, level.height));
        GeoTransform {
            pixel_width: self.transform.pixel_width * f64::from(fw) / f64::from(level.width.max(1)),
            pixel_height: self.transform.pixel_height * f64::from(fh) / f64::from(level.height.max(1)),
            ..self.transform
        }
    }
}

/// What reading the layout came to.
#[derive(Debug)]
pub enum Parse {
    Done(Box<Layout>),
    /// These bytes (first, end) must be read first.
    Need(u64, u64),
    /// Not a tiled GeoTIFF RustGIS can read a tile at a time (strips, separate band planes…): read
    /// it whole instead.
    NotTiled(String),
}

fn err(e: impl std::fmt::Display) -> DataError {
    DataError::Format(format!("GeoTIFF: {e}"))
}

fn limits() -> Limits {
    let mut l = Limits::default();
    l.decoding_buffer_size = MAX_TILE_SAMPLES * 8;
    l.intermediate_buffer_size = 64 << 20;
    l.ifd_value_size = 16 << 20;
    l
}

/// Whether `bytes` start a TIFF (classic or BigTIFF).
pub fn is_tiff(bytes: &[u8]) -> bool {
    matches!(bytes.get(..4), Some([b'I', b'I', 42 | 43, 0] | [b'M', b'M', 0, 42 | 43]))
}

/// The layout of a tiled GeoTIFF, from the bytes read so far.
/// Takes a level's `n` tiles from what all the levels may hold together.
fn take_tiles(allowance: &mut usize, n: usize) -> Result<()> {
    if n > MAX_TILES || n > *allowance {
        return Err(err(format!("{n} tiles is too many")));
    }
    *allowance -= n;
    Ok(())
}

pub fn layout(bytes: &Sparse) -> Result<Parse> {
    let r = bytes.reader();
    let watch = r.clone();
    match read_layout(r) {
        Ok(p) => Ok(p),
        Err(e) => match watch.missing() {
            Some((a, b)) => Ok(Parse::Need(a, b)),
            None => Err(e),
        },
    }
}

fn read_layout(r: Sparse) -> Result<Parse> {
    let mut dec = Decoder::new(super::geotiff::reader(r).map_err(err)?).map_err(err)?.with_limits(limits());
    let geo = super::geotiff::cog_geo(&mut dec)?;
    let ct = dec.colortype().map_err(err)?;
    let (bands, color) = match ct {
        ColorType::Gray(_) => (1, ColorHint::Gray),
        ColorType::GrayA(_) => (2, ColorHint::GrayAlpha),
        ColorType::RGB(_) => (3, ColorHint::Rgb { alpha: false }),
        ColorType::YCbCr(_) if dec.find_tag_unsigned::<u16>(Tag::Compression).ok().flatten() == Some(7) => {
            (3, ColorHint::Rgb { alpha: false })
        }
        ColorType::RGBA(_) => (4, ColorHint::Rgb { alpha: true }),
        ColorType::Multiband { num_samples, .. } => (num_samples.max(1) as usize, ColorHint::Gray),
        other => return Ok(Parse::NotTiled(format!("{other:?} images are read whole"))),
    };
    let planar = dec.find_tag_unsigned::<u16>(Tag::PlanarConfiguration).ok().flatten() == Some(2);
    if planar && bands > 1 {
        return Ok(Parse::NotTiled("bands stored apart are read whole".into()));
    }
    let data_type = match dec.find_tag_unsigned::<u16>(Tag::SampleFormat).ok().flatten().unwrap_or(1) {
        3 => "Float32",
        2 => "Int16",
        _ => {
            if dec.find_tag_unsigned_vec::<u16>(Tag::BitsPerSample).ok().flatten().and_then(|v| v.first().copied()).unwrap_or(8) > 8 {
                "UInt16"
            } else {
                "UInt8"
            }
        }
    };
    let mut levels = Vec::new();
    let mut index = 0usize;
    let mut allowance = MAX_TOTAL_TILES;
    loop {
        let subfile = dec.find_tag_unsigned::<u32>(Tag::NewSubfileType).ok().flatten().unwrap_or(0);
        // Transparency masks are skipped.
        if subfile & 4 == 0 {
            let Some(tw) = dec.find_tag_unsigned::<u32>(Tag::TileWidth).ok().flatten() else {
                if index == 0 {
                    return Ok(Parse::NotTiled("a GeoTIFF in strips is read whole".into()));
                }
                break;
            };
            let th = dec.find_tag_unsigned::<u32>(Tag::TileLength).ok().flatten().unwrap_or(tw);
            let (w, h) = dec.dimensions().map_err(err)?;
            if tw == 0 || th == 0 || (tw as usize).saturating_mul(th as usize).saturating_mul(bands) > MAX_TILE_SAMPLES {
                return Err(err(format!("tiles of {tw} × {th} pixels")));
            }
            let n = (w.div_ceil(tw) as usize).saturating_mul(h.div_ceil(th) as usize);
            take_tiles(&mut allowance, n)?;
            let mut offsets = dec.find_tag_unsigned_vec::<u64>(Tag::TileOffsets).map_err(err)?.unwrap_or_default();
            let mut counts = dec.find_tag_unsigned_vec::<u64>(Tag::TileByteCounts).map_err(err)?.unwrap_or_default();
            if offsets.len() < n || counts.len() < n {
                return Err(err("the tile index is shorter than the image"));
            }
            // Entries past the image's tiles aren't kept.
            offsets.truncate(n);
            counts.truncate(n);
            // An overview a little larger than the one before isn't an overview of it.
            if levels.last().is_none_or(|l: &Level| w < l.width) {
                levels.push(Level { ifd: index, width: w, height: h, tile_width: tw, tile_height: th, offsets, counts });
            }
        }
        if levels.len() >= MAX_LEVELS || !dec.more_images() {
            break;
        }
        dec.next_image().map_err(err)?;
        index += 1;
    }
    if levels.is_empty() {
        return Ok(Parse::NotTiled("no image".into()));
    }
    Ok(Parse::Done(Box::new(Layout { levels, crs: geo.0, transform: geo.1, nodata: geo.2, bands, color, data_type })))
}

/// Decodes one tile: its bands, each `tile_width × tile_height` samples (row by row; NaN past the
/// image's edge). `header` holds the file's IFDs, `bytes` the tile's own bytes.
pub fn decode_tile(header: &Sparse, layout: &Layout, level: usize, index: u32, bytes: Vec<u8>) -> Result<Vec<Vec<f32>>> {
    let l = layout.levels.get(level).ok_or_else(|| err("no such level"))?;
    let at = *l.offsets.get(index as usize).ok_or_else(|| err("no such tile"))?;
    let r = header.clone().with(at, bytes).reader();
    let mut dec = Decoder::new(super::geotiff::reader(r).map_err(err)?).map_err(err)?.with_limits(limits());
    dec.seek_to_image(l.ifd).map_err(err)?;
    let values = match dec.read_chunk(index).map_err(err)? {
        DecodingResult::U8(v) => v.into_iter().map(f32::from).collect::<Vec<f32>>(),
        DecodingResult::I8(v) => v.into_iter().map(f32::from).collect(),
        DecodingResult::U16(v) => v.into_iter().map(f32::from).collect(),
        DecodingResult::I16(v) => v.into_iter().map(f32::from).collect(),
        DecodingResult::U32(v) => v.into_iter().map(|x| x as f32).collect(),
        DecodingResult::I32(v) => v.into_iter().map(|x| x as f32).collect(),
        DecodingResult::U64(v) => v.into_iter().map(|x| x as f32).collect(),
        DecodingResult::I64(v) => v.into_iter().map(|x| x as f32).collect(),
        DecodingResult::F16(v) => v.into_iter().map(f32::from).collect(),
        DecodingResult::F32(v) => v,
        DecodingResult::F64(v) => v.into_iter().map(|x| x as f32).collect(),
    };
    let (tw, th) = (l.tile_width as usize, l.tile_height as usize);
    let nb = layout.bands.max(1);
    // An edge tile may come back cropped to the image: its rows are as wide as it says.
    let (col, row) = (index % l.across(), index / l.across());
    let rows = (l.height - (row * l.tile_height).min(l.height)).min(l.tile_height) as usize;
    let cols_cropped = (l.width - (col * l.tile_width).min(l.width)).min(l.tile_width) as usize;
    let stride = if values.len() >= tw * th * nb { tw } else { cols_cropped.max(1) };
    let mut out = vec![vec![f32::NAN; tw * th]; nb];
    for y in 0..rows.min(th) {
        for x in 0..stride.min(tw) {
            let src = (y * stride + x) * nb;
            for (b, band) in out.iter_mut().enumerate() {
                if let (Some(v), Some(slot)) = (values.get(src + b), band.get_mut(y * tw + x)) {
                    *slot = *v;
                }
            }
        }
    }
    if let Some(nd) = layout.nodata {
        let nd = nd as f32;
        for band in &mut out {
            for v in band.iter_mut().filter(|v| **v == nd) {
                *v = f32::NAN;
            }
        }
    }
    Ok(out)
}

/// A small cloud-optimised GeoTIFF, for tests and examples: tiled, uncompressed, single-band
/// 8-bit, in longitude/latitude from 10° E, 46° N over one degree; `size` pixels a side in tiles
/// of `tile`, with overviews halving down to one tile; pixel (x, y) of the full image is
/// `(x + y) % 256`. Its IFDs come first, as a cloud-optimised file's do.
#[doc(hidden)]
pub fn example(size: u32, tile: u32) -> Vec<u8> {
    let (size, tile) = (size.clamp(1, 4096), tile.clamp(16, 1024));
    let mut sizes = vec![size];
    while *sizes.last().unwrap_or(&tile) > tile {
        sizes.push(sizes.last().unwrap_or(&tile).div_ceil(2));
    }
    // Each image's pixels, tile by tile.
    let images: Vec<Vec<Vec<u8>>> = sizes
        .iter()
        .map(|&s| {
            let k = size / s;
            let n = s.div_ceil(tile);
            (0..n * n)
                .map(|i| {
                    let (tx, ty) = (i % n, i / n);
                    (0..tile * tile)
                        .map(|p| {
                            let (x, y) = (tx * tile + p % tile, ty * tile + p / tile);
                            if x < s && y < s { ((x * k + y * k) % 256) as u8 } else { 0 }
                        })
                        .collect()
                })
                .collect()
        })
        .collect();
    // IFD entries: (tag, type, values); type 3 short, 4 long, 12 double.
    enum V {
        S(Vec<u16>),
        L(Vec<u32>),
        D(Vec<f64>),
    }
    let header_len = 8u32;
    let mut ifds: Vec<Vec<(u16, V)>> = Vec::new();
    for (i, &s) in sizes.iter().enumerate() {
        let n = (s.div_ceil(tile) * s.div_ceil(tile)) as usize;
        let mut e = vec![
            (254u16, V::L(vec![if i == 0 { 0 } else { 1 }])),
            (256, V::L(vec![s])),
            (257, V::L(vec![s])),
            (258, V::S(vec![8])),
            (259, V::S(vec![1])),
            (262, V::S(vec![1])),
            (277, V::S(vec![1])),
            (322, V::L(vec![tile])),
            (323, V::L(vec![tile])),
            (324, V::L(vec![0; n])),
            (325, V::L(vec![tile * tile; n])),
        ];
        if i == 0 {
            e.push((33550, V::D(vec![1.0 / f64::from(size), 1.0 / f64::from(size), 0.0])));
            e.push((33922, V::D(vec![0.0, 0.0, 0.0, 10.0, 46.0, 0.0])));
            e.push((34735, V::S(vec![1, 1, 0, 2, 1024, 0, 1, 2, 2048, 0, 1, 4326])));
        }
        ifds.push(e);
    }
    // Sizes: each IFD is 2 + 12·entries + 4, its out-of-line values after it.
    let size_of = |v: &V| match v {
        V::S(x) => x.len() * 2,
        V::L(x) => x.len() * 4,
        V::D(x) => x.len() * 8,
    };
    let mut ifd_at = Vec::new();
    let mut at = header_len as usize;
    for e in &ifds {
        ifd_at.push(at);
        at += 2 + 12 * e.len() + 4;
        at += e.iter().map(|(_, v)| size_of(v)).filter(|s| *s > 4).map(|s| s + s % 2).sum::<usize>();
    }
    // Tile data after every IFD.
    let mut data_at = at;
    let mut out = vec![b'I', b'I', 42, 0];
    out.extend_from_slice(&header_len.to_le_bytes());
    for e in ifds.iter_mut() {
        for (tag, v) in e.iter_mut() {
            if *tag == 324
                && let V::L(offs) = v
            {
                for o in offs.iter_mut() {
                    *o = data_at as u32;
                    data_at += (tile * tile) as usize;
                }
            }
        }
    }
    for (i, e) in ifds.iter().enumerate() {
        // Written in order, each IFD where `ifd_at` placed it.
        let mut extra = out.len() + 2 + 12 * e.len() + 4;
        let mut body = Vec::new();
        let mut tail = Vec::new();
        body.extend_from_slice(&(e.len() as u16).to_le_bytes());
        for (tag, v) in e {
            let (ty, count, bytes): (u16, usize, Vec<u8>) = match v {
                V::S(x) => (3, x.len(), x.iter().flat_map(|a| a.to_le_bytes()).collect()),
                V::L(x) => (4, x.len(), x.iter().flat_map(|a| a.to_le_bytes()).collect()),
                V::D(x) => (12, x.len(), x.iter().flat_map(|a| a.to_le_bytes()).collect()),
            };
            body.extend_from_slice(&tag.to_le_bytes());
            body.extend_from_slice(&ty.to_le_bytes());
            body.extend_from_slice(&(count as u32).to_le_bytes());
            if bytes.len() <= 4 {
                let mut b = bytes.clone();
                b.resize(4, 0);
                body.extend_from_slice(&b);
            } else {
                body.extend_from_slice(&(extra as u32).to_le_bytes());
                let mut b = bytes.clone();
                if b.len() % 2 == 1 {
                    b.push(0);
                }
                extra += b.len();
                tail.extend_from_slice(&b);
            }
        }
        let next = ifd_at.get(i + 1).copied().unwrap_or(0) as u32;
        body.extend_from_slice(&next.to_le_bytes());
        out.extend_from_slice(&body);
        out.extend_from_slice(&tail);
    }
    for image in &images {
        for t in image {
            out.extend_from_slice(t);
        }
    }
    out
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[test]
    fn tile_budget_across_levels() {
        let mut allowance = MAX_TOTAL_TILES;
        assert!(take_tiles(&mut allowance, MAX_TILES + 1).is_err());
        assert!(take_tiles(&mut allowance, MAX_TILES).is_ok() && take_tiles(&mut allowance, MAX_TILES).is_ok());
        // All the levels together past the budget: refused.
        assert!(take_tiles(&mut allowance, 1).is_err());
    }

    #[test]
    fn layout_from_the_header_then_tiles_one_by_one() {
        let file = example(1024, 256);
        // The first 64 bytes aren't enough: it says what to read next, until it has the IFDs.
        let mut s = Sparse::default().with(0, file[..64].to_vec());
        let mut rounds = 0;
        let l = loop {
            match layout(&s).unwrap() {
                Parse::Done(l) => break l,
                Parse::Need(a, b) => {
                    rounds += 1;
                    assert!(rounds < 20);
                    let end = (b.max(a + 256) as usize).min(file.len());
                    s = s.with(a, file[a as usize..end].to_vec());
                }
                Parse::NotTiled(why) => panic!("{why}"),
            }
        };
        assert_eq!(l.levels.iter().map(|l| l.width).collect::<Vec<_>>(), vec![1024, 512, 256]);
        assert_eq!(l.crs, Crs::LonLat);
        assert_eq!((l.transform.origin_x, l.transform.origin_y), (10.0, 46.0));
        let t = l.transform_of(&l.levels[2]);
        assert!((t.pixel_width - 1.0 / 256.0).abs() < 1e-12);
        // Tile 5 of the full image (row 1, column 1): pixel (256 + x) + (256 + y).
        let lv = &l.levels[0];
        let (at, n) = (lv.offsets[5] as usize, lv.counts[5] as usize);
        let bands = decode_tile(&s, &l, 0, 5, file[at..at + n].to_vec()).unwrap();
        assert_eq!(bands.len(), 1);
        assert_eq!(bands[0][0], 0.0); // (256 + 256) % 256
        assert_eq!(bands[0][3 * 256 + 2], 5.0);
        // The coarsest overview: every fourth pixel.
        let lv = &l.levels[2];
        let (at, n) = (lv.offsets[0] as usize, lv.counts[0] as usize);
        let bands = decode_tile(&s, &l, 2, 0, file[at..at + n].to_vec()).unwrap();
        assert_eq!(bands[0][256 + 1], 8.0);
        // A wrong tile index, junk tile bytes: errors, never a panic.
        assert!(decode_tile(&s, &l, 0, 99, Vec::new()).is_err());
        let _ = decode_tile(&s, &l, 0, 0, vec![1, 2, 3]);
    }

    #[test]
    fn strips_and_junk_are_refused() {
        assert!(is_tiff(b"II*\0") && is_tiff(b"MM\0+") && !is_tiff(b"PK\x03\x04"));
        let strips = crate::raster::geotiff::tests_support::strip_tiff();
        assert!(matches!(layout(&Sparse::default().with(0, strips)).unwrap(), Parse::NotTiled(_)));
        // Every truncation of a COG: a range asked for, an error, never a panic.
        let file = example(512, 256);
        for n in 0..300 {
            let _ = layout(&Sparse::default().with(0, file[..n].to_vec()));
        }
        let _ = layout(&Sparse::default().with(0, b"II*\0\xff\xff\xff\x7f".to_vec()));
    }
}
