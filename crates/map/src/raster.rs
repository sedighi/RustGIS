//! Raster layers: a shared raster, where it came from, and how its pixels are coloured.
//!
//! Renderers follow the raster layer's Symbology gallery: Stretch (a colour ramp over one band),
//! RGB (three bands as red, green and blue), Classify (class breaks), Unique Values (one colour per
//! whole-number value), Colormap (the file's colour table) and Shaded Relief (hillshade tinted by
//! elevation). Stretch types, dynamic range adjustment (statistics from the current view),
//! resampling and brightness/contrast/gamma apply on top.

use std::sync::Arc;

use rustgis_data::raster::{BandStats, ColorHint, Raster};
use serde::{Deserialize, Serialize};

use crate::color::{Ramp, Rgba};
use crate::symbology::{ClassMethod, class_breaks, fmt_num};

/// A raster layer's data source and style.
#[derive(Clone, Serialize, Deserialize)]
pub struct RasterLayer {
    /// The file it was read from; reopened when the project loads.
    #[serde(default)]
    pub path: Option<String>,
    /// The file's own name (for its format) when it is embedded.
    #[serde(default)]
    pub file_name: Option<String>,
    /// The original file, kept in the project when there is no path (a browser upload).
    #[serde(default, skip_serializing_if = "Option::is_none", with = "b64")]
    pub embedded: Option<Arc<Vec<u8>>>,
    /// For a raster GDAL reads: the subdataset added (a NetCDF variable), so it is read again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subdataset: Option<String>,
    /// For a raster GDAL reads: the coordinate system given for a file that doesn't say.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub crs: Option<String>,
    pub style: RasterStyle,
    /// The pixels; `None` when the source couldn't be read.
    #[serde(skip)]
    pub data: Option<Arc<Raster>>,
}

impl std::fmt::Debug for RasterLayer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RasterLayer")
            .field("path", &self.path)
            .field("style", &self.style)
            .field("data", &self.data)
            .finish_non_exhaustive()
    }
}

impl RasterLayer {
    /// A raster read from a file on disk.
    pub fn from_path(raster: Raster, path: impl Into<String>) -> RasterLayer {
        let style = RasterStyle::default_for(&raster);
        RasterLayer {
            path: Some(path.into()),
            file_name: None,
            embedded: None,
            subdataset: None,
            crs: None,
            style,
            data: Some(Arc::new(raster)),
        }
    }

    /// A raster from a file's bytes, kept in the project.
    pub fn from_bytes(raster: Raster, file_name: impl Into<String>, bytes: Vec<u8>) -> RasterLayer {
        let style = RasterStyle::default_for(&raster);
        RasterLayer {
            path: None,
            file_name: Some(file_name.into()),
            embedded: Some(Arc::new(bytes)),
            subdataset: None,
            crs: None,
            style,
            data: Some(Arc::new(raster)),
        }
    }

    /// Reads the pixels again from the path or the embedded file (after a project opens).
    pub fn reload(&mut self) -> Result<(), String> {
        let r = if let Some(bytes) = &self.embedded {
            rustgis_data::raster::read_bytes(self.file_name.as_deref().unwrap_or("raster.tif"), bytes)
        } else if let Some(p) = &self.path {
            rustgis_data::raster::read_path_with(std::path::Path::new(p), self.subdataset.as_deref(), self.crs.as_deref())
        } else {
            return Err("the raster has no source".into());
        };
        match r {
            Ok(r) => {
                self.style.repair(&r);
                self.data = Some(Arc::new(r));
                Ok(())
            }
            Err(e) => {
                self.data = None;
                Err(e.to_string())
            }
        }
    }
}

pub(crate) mod b64 {
    use std::sync::Arc;

    use base64::Engine;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(v: &Option<Arc<Vec<u8>>>, s: S) -> Result<S::Ok, S::Error> {
        match v {
            Some(b) => s.serialize_some(&base64::engine::general_purpose::STANDARD.encode(b.as_slice())),
            None => s.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Arc<Vec<u8>>>, D::Error> {
        let s: Option<String> = Option::deserialize(d)?;
        // A corrupt embedded file leaves the layer without data rather than failing the project.
        Ok(s.and_then(|s| base64::engine::general_purpose::STANDARD.decode(s.as_bytes()).ok()).map(Arc::new))
    }
}

/// How pixel values map onto the colour range.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum Stretch {
    /// Values as they are (0–255 for 8-bit data, otherwise minimum to maximum).
    None,
    MinMax,
    /// Clip the lowest and highest percent of pixels.
    PercentClip {
        low: f64,
        high: f64,
    },
    /// Mean ± `n` standard deviations.
    StandardDeviation {
        n: f64,
    },
    /// Spread values evenly over the histogram.
    HistogramEqualize,
}

impl Default for Stretch {
    fn default() -> Self {
        Stretch::PercentClip { low: 0.5, high: 0.5 }
    }
}

impl Stretch {
    pub const KINDS: [(&'static str, &'static str); 5] = [
        ("none", "None"),
        ("minMax", "Minimum Maximum"),
        ("percentClip", "Percent Clip"),
        ("standardDeviation", "Standard Deviation"),
        ("histogramEqualize", "Histogram Equalize"),
    ];

    pub fn code(&self) -> &'static str {
        match self {
            Stretch::None => "none",
            Stretch::MinMax => "minMax",
            Stretch::PercentClip { .. } => "percentClip",
            Stretch::StandardDeviation { .. } => "standardDeviation",
            Stretch::HistogramEqualize => "histogramEqualize",
        }
    }

    pub fn label(&self) -> &'static str {
        Stretch::KINDS.iter().find(|(c, _)| *c == self.code()).map(|(_, l)| *l).unwrap_or("Percent Clip")
    }

    /// A stretch by code, with optional parameters (`low`/`high` percent, `n` deviations).
    pub fn from_code(code: &str, low: Option<f64>, high: Option<f64>, n: Option<f64>) -> Option<Stretch> {
        let pct = |v: Option<f64>| v.unwrap_or(0.5).clamp(0.0, 49.0);
        Some(match code.trim().to_ascii_lowercase().replace([' ', '_', '-'], "").as_str() {
            "none" => Stretch::None,
            "minmax" | "minimummaximum" => Stretch::MinMax,
            "percentclip" => Stretch::PercentClip { low: pct(low), high: pct(high) },
            "standarddeviation" | "stddev" => Stretch::StandardDeviation { n: n.unwrap_or(2.0).clamp(0.1, 10.0) },
            "histogramequalize" | "histogramequalization" => Stretch::HistogramEqualize,
            _ => return None,
        })
    }
}

/// How screen pixels pick raster values.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Resampling {
    /// The nearest pixel: crisp cells, right for categories.
    #[default]
    Nearest,
    /// Blend the four nearest pixels: smooth, for continuous data.
    Bilinear,
}

impl Resampling {
    pub const ALL: [Resampling; 2] = [Resampling::Nearest, Resampling::Bilinear];
    pub fn label(self) -> &'static str {
        match self {
            Resampling::Nearest => "Nearest Neighbor",
            Resampling::Bilinear => "Bilinear",
        }
    }
    pub fn from_name(s: &str) -> Option<Resampling> {
        let s = s.trim().to_ascii_lowercase().replace([' ', '_', '-'], "");
        Resampling::ALL.into_iter().find(|r| {
            let l = r.label().to_ascii_lowercase().replace(' ', "");
            l == s || l.starts_with(&s) || format!("{r:?}").eq_ignore_ascii_case(&s)
        })
    }
}

/// What a raster layer draws.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum RasterRenderer {
    /// One band through a colour ramp.
    Stretch { band: usize, ramp: Ramp, invert: bool },
    /// Three bands as red, green and blue.
    Rgb { red: usize, green: usize, blue: usize },
    /// Class breaks over one band.
    Classify { band: usize, method: ClassMethod, ramp: Ramp, breaks: Vec<f64> },
    /// A colour per whole-number value.
    UniqueValues { band: usize, ramp: Ramp, values: Vec<(f64, Rgba)> },
    /// The file's own colour table.
    Colormap,
    /// Hillshade, optionally tinted by elevation.
    ShadedRelief { band: usize, ramp: Option<Ramp>, azimuth: f64, altitude: f64, z_factor: f64 },
}

impl RasterRenderer {
    pub const KINDS: [(&'static str, &'static str); 6] = [
        ("stretch", "Stretch"),
        ("rgb", "RGB"),
        ("classify", "Classify"),
        ("unique", "Unique Values"),
        ("colormap", "Colormap"),
        ("shadedRelief", "Shaded Relief"),
    ];

    pub fn code(&self) -> &'static str {
        match self {
            RasterRenderer::Stretch { .. } => "stretch",
            RasterRenderer::Rgb { .. } => "rgb",
            RasterRenderer::Classify { .. } => "classify",
            RasterRenderer::UniqueValues { .. } => "unique",
            RasterRenderer::Colormap => "colormap",
            RasterRenderer::ShadedRelief { .. } => "shadedRelief",
        }
    }

    pub fn label(&self) -> &'static str {
        RasterRenderer::KINDS.iter().find(|(c, _)| *c == self.code()).map(|(_, l)| *l).unwrap_or("Stretch")
    }

    /// The bands it reads.
    pub fn bands(&self) -> Vec<usize> {
        match self {
            RasterRenderer::Stretch { band, .. }
            | RasterRenderer::Classify { band, .. }
            | RasterRenderer::UniqueValues { band, .. }
            | RasterRenderer::ShadedRelief { band, .. } => vec![*band],
            RasterRenderer::Rgb { red, green, blue } => vec![*red, *green, *blue],
            RasterRenderer::Colormap => vec![0],
        }
    }

    pub fn ramp(&self) -> Option<Ramp> {
        match self {
            RasterRenderer::Stretch { ramp, .. } | RasterRenderer::Classify { ramp, .. } | RasterRenderer::UniqueValues { ramp, .. } => {
                Some(ramp.clone())
            }
            RasterRenderer::ShadedRelief { ramp, .. } => ramp.clone(),
            _ => None,
        }
    }
}

/// Most values given their own colour by Unique Values.
pub const MAX_UNIQUE_VALUES: usize = 256;

/// A raster layer's symbology.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RasterStyle {
    pub renderer: RasterRenderer,
    #[serde(default)]
    pub stretch: Stretch,
    /// Dynamic range adjustment: stretch statistics from the pixels in view.
    #[serde(default)]
    pub dra: bool,
    #[serde(default)]
    pub resampling: Resampling,
    /// −100 to 100.
    #[serde(default)]
    pub brightness: f64,
    /// −100 to 100.
    #[serde(default)]
    pub contrast: f64,
    /// 0.1 to 10; 1 leaves values unchanged.
    #[serde(default = "one")]
    pub gamma: f64,
    /// Display rotation in degrees clockwise, about the raster's centre.
    #[serde(default)]
    pub rotation: f64,
}

fn one() -> f64 {
    1.0
}

impl RasterStyle {
    /// The default look: the file's colours, RGB for colour images, unique values for a few
    /// categories, otherwise a grey stretch.
    pub fn default_for(r: &Raster) -> RasterStyle {
        let byte_rgb = r.data_type == "UInt8";
        let (renderer, stretch) = match &r.color {
            ColorHint::Palette(_) => (RasterRenderer::Colormap, Stretch::None),
            ColorHint::Rgb { .. } if r.band_count() >= 3 => {
                (RasterRenderer::Rgb { red: 0, green: 1, blue: 2 }, if byte_rgb { Stretch::None } else { Stretch::default() })
            }
            _ if r.band_count() >= 3 => (RasterRenderer::Rgb { red: 0, green: 1, blue: 2 }, Stretch::default()),
            _ => match r.bands.first().and_then(|b| b.stats.categories.as_ref()).filter(|c| c.len() <= 16 && r.data_type.contains("Int")) {
                Some(_) => (unique_values(r, 0, Ramp::Harbour), Stretch::None),
                None => (RasterRenderer::Stretch { band: 0, ramp: Ramp::Greys, invert: true }, Stretch::default()),
            },
        };
        RasterStyle {
            renderer,
            stretch,
            dra: false,
            resampling: Resampling::Nearest,
            brightness: 0.0,
            contrast: 0.0,
            gamma: 1.0,
            rotation: 0.0,
        }
    }

    /// Recolours with a colour scheme.
    pub fn set_ramp(&mut self, ramp: Ramp) {
        match &mut self.renderer {
            RasterRenderer::Stretch { ramp: r, .. } | RasterRenderer::Classify { ramp: r, .. } => *r = ramp,
            RasterRenderer::ShadedRelief { ramp: r, .. } => *r = Some(ramp),
            RasterRenderer::UniqueValues { ramp: r, values, .. } => {
                let n = values.len();
                for (i, (_, c)) in values.iter_mut().enumerate() {
                    *c = ramp.pick(i, n);
                }
                *r = ramp;
            }
            RasterRenderer::Rgb { .. } | RasterRenderer::Colormap => {}
        }
    }

    /// Keeps band numbers inside the raster (after it changes on disk).
    pub fn repair(&mut self, r: &Raster) {
        let last = r.band_count().saturating_sub(1);
        match &mut self.renderer {
            RasterRenderer::Stretch { band, .. }
            | RasterRenderer::Classify { band, .. }
            | RasterRenderer::UniqueValues { band, .. }
            | RasterRenderer::ShadedRelief { band, .. } => *band = (*band).min(last),
            RasterRenderer::Rgb { red, green, blue } => {
                for b in [red, green, blue] {
                    *b = (*b).min(last);
                }
            }
            RasterRenderer::Colormap => {
                if !matches!(r.color, ColorHint::Palette(_)) {
                    *self = RasterStyle::default_for(r);
                }
            }
        }
    }
}

/// Valid values of a band, sorted, sampled down to about `max` of them.
pub fn sorted_sample(r: &Raster, band: usize, max: usize) -> Vec<f64> {
    let Some(b) = r.bands.get(band) else { return Vec::new() };
    let step = (b.data.len() / max.max(1)).max(1);
    let mut v: Vec<f64> = b.data.iter().step_by(step).filter(|v| r.is_valid(**v)).map(|v| *v as f64).collect();
    v.sort_by(|a, b| a.total_cmp(b));
    v
}

/// A Classify renderer with `classes` breaks.
pub fn classify(r: &Raster, band: usize, classes: usize, method: ClassMethod, ramp: Ramp) -> RasterRenderer {
    let sorted = sorted_sample(r, band, 50_000);
    RasterRenderer::Classify { band, method, ramp, breaks: class_breaks(&sorted, classes.clamp(1, 32), method) }
}

/// A Unique Values renderer over a band's distinct whole numbers (at most [`MAX_UNIQUE_VALUES`]).
pub fn unique_values(r: &Raster, band: usize, ramp: Ramp) -> RasterRenderer {
    let cats = r.bands.get(band).and_then(|b| b.stats.categories.clone()).unwrap_or_default();
    let n = cats.len().min(MAX_UNIQUE_VALUES);
    let values = cats.into_iter().take(n).enumerate().map(|(i, v)| (v, ramp.pick(i, n))).collect();
    RasterRenderer::UniqueValues { band, ramp, values }
}

/// Maps a band's values to 0–1 for a stretch.
#[derive(Clone, Debug)]
pub struct Normalizer {
    pub lo: f64,
    pub hi: f64,
    /// Cumulative histogram (0–1) for histogram equalisation, over `lo`..`hi`.
    cdf: Option<Vec<f64>>,
}

impl Normalizer {
    pub fn new(stretch: Stretch, s: &BandStats, data_type: &str) -> Normalizer {
        let (lo, hi) = match stretch {
            Stretch::None if data_type == "UInt8" => (0.0, 255.0),
            Stretch::None | Stretch::MinMax | Stretch::HistogramEqualize => (s.min, s.max),
            Stretch::PercentClip { low, high } => (s.percentile(low), s.percentile(100.0 - high)),
            Stretch::StandardDeviation { n } => ((s.mean - n * s.std_dev).max(s.min), (s.mean + n * s.std_dev).min(s.max)),
        };
        let cdf = (stretch == Stretch::HistogramEqualize && !s.histogram.is_empty()).then(|| {
            let total: u64 = s.histogram.iter().sum::<u64>().max(1);
            let mut acc = 0u64;
            s.histogram
                .iter()
                .map(|c| {
                    acc += c;
                    acc as f64 / total as f64
                })
                .collect()
        });
        let (lo, hi) = if lo.is_finite() && hi.is_finite() { (lo, hi) } else { (0.0, 1.0) };
        Normalizer { lo, hi, cdf }
    }

    /// `v` scaled to 0–1 (clamped).
    pub fn t(&self, v: f64) -> f64 {
        let span = self.hi - self.lo;
        let x = if span > 0.0 { (v - self.lo) / span } else { 0.5 };
        let x = x.clamp(0.0, 1.0);
        match &self.cdf {
            Some(cdf) if !cdf.is_empty() => {
                let i = (x * (cdf.len() - 1) as f64).round() as usize;
                cdf.get(i.min(cdf.len() - 1)).copied().unwrap_or(x)
            }
            _ => x,
        }
    }
}

/// Brightness, contrast and gamma as a lookup table on 0–255 channel values.
pub fn enhancement_lut(brightness: f64, contrast: f64, gamma: f64) -> [u8; 256] {
    let mut lut = [0u8; 256];
    let b = brightness.clamp(-100.0, 100.0) / 100.0 * 255.0;
    let c = contrast.clamp(-100.0, 100.0) / 100.0;
    let k = if c >= 0.0 { 1.0 + c * 3.0 } else { 1.0 + c };
    let g = if gamma.is_finite() { gamma.clamp(0.1, 10.0) } else { 1.0 };
    for (i, out) in lut.iter_mut().enumerate() {
        let x = ((i as f64 - 127.5) * k + 127.5 + b).clamp(0.0, 255.0) / 255.0;
        *out = (x.powf(1.0 / g) * 255.0).round().clamp(0.0, 255.0) as u8;
    }
    lut
}

/// Colours pixel values for one draw of a raster layer.
pub struct Colorizer<'a> {
    style: &'a RasterStyle,
    norms: Vec<Normalizer>,
    lut: [u8; 256],
    plain: bool,
    ramp_lut: Vec<[u8; 4]>,
    palette: &'a [[u8; 4]],
}

impl<'a> Colorizer<'a> {
    /// `view_stats` are statistics of the pixels in view, one per renderer band, for DRA.
    pub fn new(style: &'a RasterStyle, r: &'a Raster, view_stats: Option<&[BandStats]>) -> Colorizer<'a> {
        let bands = style.renderer.bands();
        let norms = bands
            .iter()
            .enumerate()
            .map(|(i, b)| {
                let s = view_stats.and_then(|v| v.get(i)).or_else(|| r.bands.get(*b).map(|b| &b.stats));
                s.map(|s| Normalizer::new(style.stretch, s, &r.data_type)).unwrap_or(Normalizer { lo: 0.0, hi: 1.0, cdf: None })
            })
            .collect();
        let ramp_lut = match &style.renderer {
            RasterRenderer::Stretch { ramp, invert, .. } => {
                (0..256).map(|i| ramp.at(if *invert { 1.0 - i as f64 / 255.0 } else { i as f64 / 255.0 }).0).collect()
            }
            RasterRenderer::ShadedRelief { ramp: Some(ramp), .. } => (0..256).map(|i| ramp.at(i as f64 / 255.0).0).collect(),
            _ => Vec::new(),
        };
        let palette: &[[u8; 4]] = match &r.color {
            ColorHint::Palette(p) => p,
            _ => &[],
        };
        let plain = style.brightness == 0.0 && style.contrast == 0.0 && (style.gamma - 1.0).abs() < 1e-9;
        Colorizer { style, norms, lut: enhancement_lut(style.brightness, style.contrast, style.gamma), plain, ramp_lut, palette }
    }

    fn byte(&self, band: usize, v: f32) -> u8 {
        self.norms.get(band).map(|n| (n.t(v as f64) * 255.0).round() as u8).unwrap_or(0)
    }

    fn enhance(&self, c: [u8; 4]) -> [u8; 4] {
        if self.plain {
            return c;
        }
        let l = |x: u8| self.lut[x as usize];
        [l(c[0]), l(c[1]), l(c[2]), c[3]]
    }

    /// The colour of a pixel from its renderer band values (`NaN` = no data), or `None` for
    /// transparent. `hillshade` (0–255) is used by Shaded Relief.
    pub fn color(&self, vals: &[f32], hillshade: Option<u8>) -> Option<[u8; 4]> {
        let v0 = *vals.first()?;
        let c = match &self.style.renderer {
            RasterRenderer::Rgb { .. } => {
                if vals.iter().take(3).all(|v| v.is_nan()) {
                    return None;
                }
                let g = |i: usize| vals.get(i).filter(|v| !v.is_nan()).map(|v| self.byte(i, *v)).unwrap_or(0);
                [g(0), g(1), g(2), 255]
            }
            _ if v0.is_nan() => return None,
            RasterRenderer::Stretch { .. } => *self.ramp_lut.get(self.byte(0, v0) as usize)?,
            RasterRenderer::Classify { ramp, breaks, .. } => {
                let k = breaks.iter().position(|b| v0 as f64 <= *b).unwrap_or(breaks.len().saturating_sub(1));
                ramp.pick(k, breaks.len()).0
            }
            RasterRenderer::UniqueValues { values, .. } => {
                let i = values.binary_search_by(|(v, _)| v.total_cmp(&(v0 as f64))).ok()?;
                values.get(i)?.1.0
            }
            RasterRenderer::Colormap => {
                let i = v0 as i64;
                if i < 0 {
                    return None;
                }
                *self.palette.get(i as usize)?
            }
            RasterRenderer::ShadedRelief { ramp, .. } => {
                let h = hillshade.unwrap_or(180) as f64 / 255.0;
                let base = if ramp.is_some() { *self.ramp_lut.get(self.byte(0, v0) as usize)? } else { [255, 255, 255, 255] };
                // Multiply the tint by the hillshade, keeping some ambient light.
                let m = |c: u8| (c as f64 * (0.25 + 0.75 * h)).round().clamp(0.0, 255.0) as u8;
                [m(base[0]), m(base[1]), m(base[2]), 255]
            }
        };
        (c[3] > 0).then(|| self.enhance(c))
    }
}

/// Hillshade (0–255) from a 3×3 window of elevations (row by row), Horn's method.
pub fn hillshade(z: &[f64; 9], cell_x: f64, cell_y: f64, azimuth: f64, altitude: f64, z_factor: f64) -> u8 {
    let [a, b, c, d, _, f, g, h, i] = *z;
    let dzdx = ((c + 2.0 * f + i) - (a + 2.0 * d + g)) / (8.0 * cell_x.max(1e-9));
    let dzdy = ((g + 2.0 * h + i) - (a + 2.0 * b + c)) / (8.0 * cell_y.max(1e-9));
    let slope = (z_factor * (dzdx * dzdx + dzdy * dzdy).sqrt()).atan();
    let aspect = if dzdx != 0.0 || dzdy != 0.0 { dzdy.atan2(-dzdx) } else { 0.0 };
    let zenith = (90.0 - altitude.clamp(0.0, 90.0)).to_radians();
    let az = (360.0 - azimuth + 90.0).rem_euclid(360.0).to_radians();
    let v = zenith.cos() * slope.cos() + zenith.sin() * slope.sin() * (az - aspect).cos();
    (v.max(0.0) * 255.0).round().clamp(0.0, 255.0) as u8
}

/// One legend row for a raster layer.
#[derive(Clone, Debug, PartialEq)]
pub enum RasterLegend {
    /// A continuous ramp from `low` to `high`.
    Ramp { ramp: Ramp, invert: bool, high: String, low: String },
    /// A swatch and label.
    Class { color: Rgba, label: String },
    /// A band shown as a primary colour.
    Channel { color: Rgba, label: String },
}

impl RasterLayer {
    /// Legend rows for the Layers pane.
    pub fn legend(&self) -> Vec<RasterLegend> {
        let Some(r) = &self.data else { return Vec::new() };
        let band_name = |b: usize| r.bands.get(b).map(|b| b.name.clone()).unwrap_or_default();
        match &self.style.renderer {
            RasterRenderer::Stretch { band, ramp, invert } => {
                let n = r.bands.get(*band).map(|b| Normalizer::new(self.style.stretch, &b.stats, &r.data_type));
                let (lo, hi) = n.map(|n| (n.lo, n.hi)).unwrap_or((0.0, 1.0));
                vec![RasterLegend::Ramp { ramp: ramp.clone(), invert: *invert, high: fmt_num(hi), low: fmt_num(lo) }]
            }
            RasterRenderer::ShadedRelief { ramp: Some(ramp), band, .. } => {
                let s = r.bands.get(*band).map(|b| (b.stats.min, b.stats.max)).unwrap_or((0.0, 1.0));
                vec![RasterLegend::Ramp { ramp: ramp.clone(), invert: false, high: fmt_num(s.1), low: fmt_num(s.0) }]
            }
            RasterRenderer::ShadedRelief { ramp: None, .. } => {
                vec![RasterLegend::Ramp { ramp: Ramp::Greys, invert: true, high: "Lit".into(), low: "Shaded".into() }]
            }
            RasterRenderer::Rgb { red, green, blue } => vec![
                RasterLegend::Channel { color: Rgba::rgb(0xE5, 0x39, 0x35), label: band_name(*red) },
                RasterLegend::Channel { color: Rgba::rgb(0x43, 0xA0, 0x47), label: band_name(*green) },
                RasterLegend::Channel { color: Rgba::rgb(0x1E, 0x88, 0xE5), label: band_name(*blue) },
            ],
            RasterRenderer::Classify { ramp, breaks, band, .. } => {
                let mut lo = r.bands.get(*band).map(|b| b.stats.min).unwrap_or(0.0);
                breaks
                    .iter()
                    .enumerate()
                    .map(|(i, b)| {
                        let label = format!("{} – {}", fmt_num(lo), fmt_num(*b));
                        lo = *b;
                        RasterLegend::Class { color: ramp.pick(i, breaks.len()), label }
                    })
                    .collect()
            }
            RasterRenderer::UniqueValues { values, .. } => {
                values.iter().map(|(v, c)| RasterLegend::Class { color: *c, label: fmt_num(*v) }).collect()
            }
            RasterRenderer::Colormap => {
                let ColorHint::Palette(p) = &r.color else { return Vec::new() };
                let cats = r.bands.first().and_then(|b| b.stats.categories.clone()).unwrap_or_default();
                cats.iter()
                    .filter_map(|v| p.get(*v as usize).map(|c| RasterLegend::Class { color: Rgba(*c), label: fmt_num(*v) }))
                    .collect()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use rustgis_data::raster::{Crs, GeoTransform};

    use super::*;

    fn dem() -> Raster {
        let gt = GeoTransform { origin_x: 0.0, origin_y: 4.0, pixel_width: 1.0, pixel_height: -1.0 };
        let data: Vec<f32> = (0..16).map(|i| (i % 4 + i / 4) as f32).collect();
        Raster::new("dem", 4, 4, vec![data], gt, Crs::LonLat, None, ColorHint::Gray, "Float32").unwrap()
    }

    #[test]
    fn defaults_and_colours() {
        let r = dem();
        let s = RasterStyle::default_for(&r);
        assert_eq!(s.renderer.code(), "stretch");
        let c = Colorizer::new(&s, &r, None);
        let lo = c.color(&[0.0], None).unwrap();
        let hi = c.color(&[6.0], None).unwrap();
        assert!(lo[0] < hi[0], "inverted greys: low is dark ({lo:?} vs {hi:?})");
        assert_eq!(c.color(&[f32::NAN], None), None);
        assert_eq!(c.color(&[], None), None);

        let u = RasterStyle { renderer: unique_values(&r, 0, Ramp::Harbour), ..s.clone() };
        let c = Colorizer::new(&u, &r, None);
        assert!(c.color(&[3.0], None).is_some());
        assert_eq!(c.color(&[3.5], None), None);
        let k = RasterStyle { renderer: classify(&r, 0, 3, ClassMethod::EqualInterval, Ramp::Ember), ..s };
        assert!(Colorizer::new(&k, &r, None).color(&[5.0], None).is_some());
    }

    #[test]
    fn stretches_and_enhancement() {
        let r = dem();
        let st = &r.bands[0].stats;
        for code in ["none", "minMax", "percentClip", "standardDeviation", "histogramEqualize"] {
            let n = Normalizer::new(Stretch::from_code(code, None, None, None).unwrap(), st, "Float32");
            assert!(n.t(-1e9) <= n.t(3.0) && n.t(3.0) <= n.t(1e9), "{code}");
            assert!((0.0..=1.0).contains(&n.t(f64::NAN).max(0.0)));
        }
        assert_eq!(Stretch::from_code("bogus", None, None, None), None);
        let flat = enhancement_lut(0.0, 0.0, 1.0);
        assert!(flat.iter().enumerate().all(|(i, v)| (*v as i32 - i as i32).abs() <= 1));
        let bright = enhancement_lut(50.0, 0.0, 1.0);
        assert!(bright[100] > flat[100]);
        let _ = enhancement_lut(f64::NAN, f64::INFINITY, f64::NAN);
    }

    #[test]
    fn hillshade_faces_the_light() {
        // Ground rising to the east faces west: light from the west brightens it. Flat ground is
        // lit evenly at cos(45°).
        let east = [0.0, 1.0, 2.0, 0.0, 1.0, 2.0, 0.0, 1.0, 2.0];
        let lit_w = hillshade(&east, 1.0, 1.0, 270.0, 45.0, 1.0);
        let lit_e = hillshade(&east, 1.0, 1.0, 90.0, 45.0, 1.0);
        assert!(lit_w > lit_e, "{lit_w} vs {lit_e}");
        let flat = hillshade(&[5.0; 9], 1.0, 1.0, 315.0, 45.0, 1.0);
        assert!((175..=185).contains(&flat), "{flat}");
    }

    #[test]
    fn project_json_round_trip_embeds_uploads() {
        let r = dem();
        let bytes = rustgis_data::raster::geotiff::write(&r, 0).unwrap();
        let rl = RasterLayer::from_bytes(r, "dem.tif", bytes);
        let json = serde_json::to_string(&rl).unwrap();
        let mut back: RasterLayer = serde_json::from_str(&json).unwrap();
        assert!(back.data.is_none());
        back.reload().unwrap();
        assert_eq!(back.data.as_ref().map(|d| d.width), Some(4));
        let mut missing = RasterLayer { path: Some("/nonexistent/x.tif".into()), embedded: None, ..back };
        assert!(missing.reload().is_err());
        assert!(missing.data.is_none());
    }
}
