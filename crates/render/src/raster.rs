//! Drawing raster layers.
//!
//! Each screen pixel is traced back to the raster's grid: Web Mercator → the raster's own
//! coordinate system → pixel position. The full transformation runs on a coarse grid of screen
//! points and is interpolated in between, which is exact for Web Mercator and lon/lat rasters
//! and well under a pixel off for projected ones. Values are then sampled (nearest or bilinear),
//! optionally re-stretched from the pixels in view (DRA), and coloured.

use rustgis_data::raster::{BandStats, ColorHint, Raster};
use rustgis_geom::{Coord, Extent, extent_to_merc};
use rustgis_map::Globe;
use rustgis_map::projection::Flat;
use rustgis_map::raster::{Colorizer, RasterRenderer, RasterStyle, Resampling, hillshade};
use tiny_skia::{FilterQuality, Pixmap, PixmapPaint, Transform};

use crate::View;

/// Screen pixels between exactly transformed points.
const STEP: usize = 8;

/// The raster pixel position under a work pixel (`k` device pixels), or `None` off the
/// projection.
/// Where a raster is drawn: on the globe, or on the flat map in a projection (with its code).
#[derive(Clone, Copy)]
pub(crate) enum Place<'a> {
    Globe(&'a Globe),
    Flat(&'a Flat, &'a str),
}

fn source(r: &Raster, view: &View, place: Place, k: f64, dx: f64, dy: f64) -> Option<(f64, f64)> {
    let ppp = view.pixels_per_point / k;
    let (sx, sy) = (dx / ppp, dy / ppp);
    let n = match place {
        Place::Globe(g) => r.transformer().from_lonlat(g.inverse(sx, sy)?)?,
        Place::Flat(Flat::Mercator, _) => r.transformer().from_merc(view.camera.to_map(sx, sy, view.width, view.height))?,
        Place::Flat(flat, key) => {
            let m = view.camera.to_map(sx, sy, view.width, view.height);
            // A raster already in the map's coordinate system needs no reprojection.
            if key.strip_prefix("EPSG:").and_then(|c| c.parse::<u16>().ok()) == Some(r.crs.epsg()) {
                m
            } else {
                r.transformer().from_lonlat(flat.inverse(m)?)?
            }
        }
    };
    let (c, row) = r.transform.to_pixel(n);
    (c.is_finite() && row.is_finite()).then_some((c, row))
}

fn sample(r: &Raster, band: usize, c: f64, row: f64, resampling: Resampling) -> f32 {
    let nearest = || r.value(band, c.floor() as i64, row.floor() as i64).unwrap_or(f32::NAN);
    if resampling == Resampling::Nearest {
        return nearest();
    }
    // Bilinear between the four pixel centres around the point.
    let (x, y) = (c - 0.5, row - 0.5);
    let (i, j) = (x.floor(), y.floor());
    let (fx, fy) = ((x - i) as f32, (y - j) as f32);
    let (i, j) = (i as i64, j as i64);
    match (r.value(band, i, j), r.value(band, i + 1, j), r.value(band, i, j + 1), r.value(band, i + 1, j + 1)) {
        (Some(a), Some(b), Some(cc), Some(d)) => {
            let top = a + (b - a) * fx;
            let bottom = cc + (d - cc) * fx;
            top + (bottom - top) * fy
        }
        _ => nearest(),
    }
}

fn place_is_flat(p: &Place) -> bool {
    matches!(p, Place::Flat(..))
}

/// Draws a raster layer onto `pm` at `opacity`.
pub(crate) fn draw(pm: &mut Pixmap, r: &Raster, style: &RasterStyle, view: &View, opacity: f32, full_res: bool, place: Place) {
    let threads = threads_for(pm.width() as usize * pm.height() as usize);
    draw_with(pm, r, style, view, opacity, full_res, place, threads);
}

/// [`draw`], its rows spread over `threads` threads.
#[allow(clippy::too_many_arguments)]
fn draw_with(pm: &mut Pixmap, r: &Raster, style: &RasterStyle, view: &View, opacity: f32, full_res: bool, place: Place, threads: usize) {
    // On high-density screens, compute at logical-pixel resolution and scale up (unless asked
    // for full resolution, as exports are): a quarter of the work at 2×.
    let k = if view.pixels_per_point >= 1.5 && !full_res { view.pixels_per_point } else { 1.0 };
    let (w, h) = ((pm.width() as f64 / k).ceil(), (pm.height() as f64 / k).ceil());
    let ppp = view.pixels_per_point / k;
    // The raster's bounds on screen, clipped to the view.
    let ((ax, ay), (bx, by)) = match place {
        // On the globe: the box around the visible part of a grid of points over the extent.
        Place::Globe(g) => {
            let e = r.extent();
            let mut b = Extent::EMPTY;
            for i in 0..=24 {
                for j in 0..=24 {
                    let c = Coord::new(e.min_x + e.width() * i as f64 / 24.0, e.min_y + e.height() * j as f64 / 24.0);
                    if let Some((x, y)) = g.forward(c) {
                        b.expand(Coord::new(x, y));
                    }
                }
            }
            // A raster under the centre of view may reach past every sample point: take the disc.
            if e.contains(view.camera.center_lonlat()) {
                let (cx, cy) = g.center();
                let r = g.disc_radius();
                ((cx - r, cy - r), (cx + r, cy + r))
            } else if b.is_empty() {
                return;
            } else {
                // A margin for the parts between the sample points.
                let m = (b.width().max(b.height()) / 24.0).max(2.0);
                ((b.min_x - m, b.min_y - m), (b.max_x + m, b.max_y + m))
            }
        }
        Place::Flat(Flat::Mercator, _) => {
            let e = extent_to_merc(&r.extent());
            (view.to_screen(Coord::new(e.min_x, e.max_y)), view.to_screen(Coord::new(e.max_x, e.min_y)))
        }
        Place::Flat(flat, _) => {
            let e = flat.extent(&r.extent());
            if e.is_empty() {
                return;
            }
            (view.to_screen(Coord::new(e.min_x, e.max_y)), view.to_screen(Coord::new(e.max_x, e.min_y)))
        }
    };
    let clip = |v: f64, max: f64| if v.is_finite() { v.clamp(0.0, max) } else { 0.0 };
    // A rotated raster turns about its centre: its pixels may land anywhere in view.
    let rot = style.rotation.rem_euclid(360.0);
    let rotated = rot.abs() > 1e-6 && place_is_flat(&place);
    let centre = (((ax + bx) / 2.0) * ppp, ((ay + by) / 2.0) * ppp);
    let (sin, cos) = (-rot.to_radians()).sin_cos();
    let (x0, x1, y0, y1) = if rotated {
        (0, w as usize, 0, h as usize)
    } else {
        (
            clip((ax.min(bx) * ppp).floor(), w) as usize,
            clip((ax.max(bx) * ppp).ceil(), w) as usize,
            clip((ay.min(by) * ppp).floor(), h) as usize,
            clip((ay.max(by) * ppp).ceil(), h) as usize,
        )
    };
    if x0 >= x1 || y0 >= y1 {
        return;
    }
    let (bw, bh) = (x1 - x0, y1 - y0);

    // Exact source positions on a coarse grid.
    let (gw, gh) = (bw / STEP + 2, bh / STEP + 2);
    let mut grid = Vec::with_capacity(gw * gh);
    for gy in 0..gh {
        for gx in 0..gw {
            grid.push(source(r, view, place, k, (x0 + gx * STEP) as f64, (y0 + gy * STEP) as f64));
        }
    }

    // Pass 1: band values (and hillshade) for each screen pixel.
    let bands = style.renderer.bands();
    let nb = bands.len().max(1);
    let relief = match style.renderer {
        RasterRenderer::ShadedRelief { azimuth, altitude, z_factor, .. } => Some((azimuth, altitude, z_factor)),
        _ => None,
    };
    // Blending neighbours would invent values that aren't classes or palette entries.
    let resampling = match style.renderer {
        RasterRenderer::UniqueValues { .. } | RasterRenderer::Colormap => Resampling::Nearest,
        _ => style.resampling,
    };
    // RGBA images carry their transparency in band 4.
    let alpha_band = match (&style.renderer, &r.color) {
        (RasterRenderer::Rgb { .. }, ColorHint::Rgb { alpha: true }) if r.band_count() >= 4 => Some(3),
        (RasterRenderer::Stretch { band: 0, .. } | RasterRenderer::ShadedRelief { band: 0, .. }, ColorHint::GrayAlpha) => Some(1),
        _ => None,
    };
    let pass1 = Pass1 {
        r,
        view,
        place,
        k,
        rotated: rotated.then_some((centre, sin, cos)),
        x0,
        y0,
        bw,
        grid: &grid,
        gw,
        bands: &bands,
        data: bands.iter().map(|b| r.bands.get(*b).map(|b| b.data.as_slice())).collect(),
        relief,
        resampling,
        alpha_band,
        cell: r.cell_size_m(),
    };
    let mut vals = vec![f32::NAN; bw * bh * nb];
    let mut shade = if relief.is_some() { vec![0u8; bw * bh] } else { Vec::new() };
    let mut alpha = if alpha_band.is_some() { vec![0u8; bw * bh] } else { Vec::new() };
    let threads = threads.clamp(1, bh);
    let rows = bh.div_ceil(threads.max(1));
    each_chunk(
        threads,
        vals.chunks_mut(rows * bw * nb).zip(chunks_or_empty(&mut shade, rows * bw)).zip(chunks_or_empty(&mut alpha, rows * bw)).enumerate(),
        |(i, ((v, sh), al))| pass1.rows(y0 + i * rows, v, sh, al),
    );

    // Dynamic range adjustment: statistics of the values in view.
    let view_stats: Option<Vec<BandStats>> =
        style.dra.then(|| (0..nb).map(|k| BandStats::from_values(&vals.iter().skip(k).step_by(nb).copied().collect::<Vec<_>>())).collect());
    let colorizer = Colorizer::new(style, r, view_stats.as_deref());

    // Pass 2: colours, premultiplied, into a pixmap the size of the raster's screen box.
    let Some(mut lay) = Pixmap::new(bw as u32, bh as u32) else { return };
    let (vals, shade, alpha) = (&vals, &shade, &alpha);
    each_chunk(threads, lay.data_mut().chunks_mut(rows * bw * 4).enumerate(), |(ci, out)| {
        let first = ci * rows * bw;
        // Neighbouring pixels often share their values (zoomed in, or classes): colour once.
        let mut last: Option<(&[f32], Option<u8>, Option<u8>, Option<[u8; 4]>)> = None;
        for (j, px) in out.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            let i = first + j;
            let Some(v) = vals.get(i * nb..(i + 1) * nb) else { continue };
            let (sh, al) = (shade.get(i).copied(), alpha.get(i).copied());
            let c = match last {
                Some((lv, ls, la, c)) if ls == sh && la == al && same(lv, v) => c,
                _ => {
                    let c = colorizer.color(v, sh).map(|mut c| {
                        if let Some(a) = al {
                            c[3] = ((c[3] as u16 * a as u16 + 127) / 255) as u8;
                        }
                        let a = c[3] as u16;
                        let pm = |x: u8| ((x as u16 * a + 127) / 255) as u8;
                        [pm(c[0]), pm(c[1]), pm(c[2]), c[3]]
                    });
                    last = Some((v, sh, al, c));
                    c
                }
            };
            if let Some(c) = c {
                px.copy_from_slice(&c);
            }
        }
    });
    // Drawn at its own size and fully opaque: a plain copy over, much cheaper than the
    // rasteriser's general image drawing.
    if k == 1.0 && opacity >= 1.0 {
        crate::blend_over(pm, &lay, x0 as i64, y0 as i64);
        return;
    }
    let quality = if style.resampling == Resampling::Bilinear { FilterQuality::Bilinear } else { FilterQuality::Nearest };
    let at = Transform::from_row(k as f32, 0.0, 0.0, k as f32, (x0 as f64 * k) as f32, (y0 as f64 * k) as f32);
    pm.draw_pixmap(0, 0, lay.as_ref(), &PixmapPaint { opacity, quality, ..PixmapPaint::default() }, at, None);
}

/// What pass 1 of [`draw`] reads: where each screen pixel falls in the raster, and how to
/// sample it there.
struct Pass1<'a> {
    r: &'a Raster,
    view: &'a View,
    place: Place<'a>,
    k: f64,
    /// A rotated raster's centre on screen and the sine and cosine of its back-rotation.
    rotated: Option<((f64, f64), f64, f64)>,
    x0: usize,
    y0: usize,
    bw: usize,
    /// Exact source positions every `STEP` pixels, `gw` to a row.
    grid: &'a [Option<(f64, f64)>],
    gw: usize,
    bands: &'a [usize],
    /// The renderer's bands' values.
    data: Vec<Option<&'a [f32]>>,
    relief: Option<(f64, f64, f64)>,
    resampling: Resampling,
    alpha_band: Option<usize>,
    cell: (f64, f64),
}

impl Pass1<'_> {
    /// Fills the band values (and hillshade and alpha, when kept) of the screen rows from `py0`
    /// on, as many as `vals` holds.
    fn rows(&self, py0: usize, vals: &mut [f32], shade: &mut [u8], alpha: &mut [u8]) {
        let (r, bw, nb) = (self.r, self.bw, self.bands.len().max(1));
        let lerp = |p: f64, q: f64, t: f64| p + (q - p) * t;
        // Each grid column's source position at this row, interpolated down the grid.
        let mut cols: Vec<Option<(f64, f64)>> = vec![None; self.gw];
        for (dy, row_vals) in vals.chunks_mut(bw * nb).enumerate() {
            let py = py0 + dy;
            let v = (py - self.y0) as f64 + 0.5;
            let gy = (v / STEP as f64) as usize;
            let fy = v / STEP as f64 - gy as f64;
            let g = |x: usize, y: usize| self.grid.get(y * self.gw + x).copied().flatten();
            for (gx, c) in cols.iter_mut().enumerate() {
                *c = match (g(gx, gy), g(gx, gy + 1)) {
                    (Some(a), Some(b)) => Some((lerp(a.0, b.0, fy), lerp(a.1, b.1, fy))),
                    _ => None,
                };
            }
            // Nearest sampling: the last cell read, and the pixel it was read for.
            let mut last: Option<((i64, i64), usize)> = None;
            for dx in 0..bw {
                let px = self.x0 + dx;
                let pos = match self.rotated {
                    // Back-rotate the screen pixel about the centre, then find its source exactly.
                    Some((centre, sin, cos)) => {
                        let (ddx, ddy) = (px as f64 + 0.5 - centre.0, py as f64 + 0.5 - centre.1);
                        source(r, self.view, self.place, self.k, centre.0 + ddx * cos - ddy * sin, centre.1 + ddx * sin + ddy * cos)
                    }
                    None => {
                        let u = dx as f64 + 0.5;
                        let gx = (u / STEP as f64) as usize;
                        let fx = u / STEP as f64 - gx as f64;
                        match (cols.get(gx).copied().flatten(), cols.get(gx + 1).copied().flatten()) {
                            (Some(a), Some(b)) => Some((lerp(a.0, b.0, fx), lerp(a.1, b.1, fx))),
                            // Near the edge of the projection's valid area: transform this pixel exactly.
                            _ => source(r, self.view, self.place, self.k, px as f64 + 0.5, py as f64 + 0.5),
                        }
                    }
                };
                let Some((c, row)) = pos else { continue };
                if c < 0.0 || row < 0.0 || c >= r.width as f64 || row >= r.height as f64 {
                    continue;
                }
                let i = dy * bw + dx;
                // Truncating is flooring here (both are at least 0), and much cheaper.
                let cell = (c as i64, row as i64);
                // The same cell as the last pixel, sampled nearest: the same values.
                if self.resampling == Resampling::Nearest && self.relief.is_none() {
                    if let Some((lc, ldx)) = last
                        && lc == cell
                    {
                        if let Some(&la) = alpha.get(dy * bw + ldx)
                            && let Some(a) = alpha.get_mut(i)
                        {
                            *a = la;
                        }
                        row_vals.copy_within(ldx * nb..(ldx + 1) * nb, dx * nb);
                        continue;
                    }
                    last = Some((cell, dx));
                }
                if let (Some(ab), Some(a)) = (self.alpha_band, alpha.get_mut(i)) {
                    *a = r.value(ab, cell.0, cell.1).map(|v| v.clamp(0.0, 255.0) as u8).unwrap_or(0);
                    if *a == 0 {
                        continue;
                    }
                }
                if self.resampling == Resampling::Nearest {
                    // The cell is in the grid (checked above): read each band at one index.
                    let at = cell.1 as usize * r.width as usize + cell.0 as usize;
                    for (bi, data) in self.data.iter().enumerate() {
                        if let (Some(slot), Some(v)) = (row_vals.get_mut(dx * nb + bi), data.and_then(|d| d.get(at))) {
                            *slot = if r.is_valid(*v) { *v } else { f32::NAN };
                        }
                    }
                } else {
                    for (bi, b) in self.bands.iter().enumerate() {
                        if let Some(slot) = row_vals.get_mut(dx * nb + bi) {
                            *slot = sample(r, *b, c, row, self.resampling);
                        }
                    }
                }
                if let (Some((az, alt, zf)), Some(b)) = (self.relief, self.bands.first()) {
                    let (ci, ri) = cell;
                    let centre = r.value(*b, ci, ri).unwrap_or(0.0) as f64;
                    let mut z = [centre; 9];
                    for (k, (dx, dy)) in
                        [(-1, -1), (0, -1), (1, -1), (-1, 0), (0, 0), (1, 0), (-1, 1), (0, 1), (1, 1)].into_iter().enumerate()
                    {
                        z[k] = r.value(*b, ci + dx, ri + dy).map(f64::from).unwrap_or(centre);
                    }
                    if let Some(s) = shade.get_mut(i) {
                        *s = hillshade(&z, self.cell.0, self.cell.1, az, alt, zf);
                    }
                }
            }
        }
    }
}

/// Whether two pixels' band values are the same (NaN, no data, equal to itself).
fn same(a: &[f32], b: &[f32]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x.to_bits() == y.to_bits())
}

/// `v` in chunks of `n`, or endless empty chunks when it is empty (a plane not kept).
fn chunks_or_empty(v: &mut [u8], n: usize) -> Box<dyn Iterator<Item = &mut [u8]> + Send + '_> {
    if v.is_empty() { Box::new(std::iter::repeat_with(|| &mut [][..])) } else { Box::new(v.chunks_mut(n.max(1))) }
}

/// How many threads to draw `pixels` with: one for a small image, and always one in the browser.
fn threads_for(pixels: usize) -> usize {
    if cfg!(target_arch = "wasm32") || pixels < 64 * 1024 {
        return 1;
    }
    std::thread::available_parallelism().map_or(1, |n| n.get()).clamp(1, 8)
}

/// Runs `f` on each item, spread over `threads` threads (in turn on this one when it is one).
fn each_chunk<T: Send>(threads: usize, items: impl Iterator<Item = T>, f: impl Fn(T) + Sync) {
    if threads <= 1 {
        items.for_each(f);
        return;
    }
    let f = &f;
    std::thread::scope(|scope| {
        for item in items {
            scope.spawn(move || f(item));
        }
    });
}

#[cfg(test)]
mod tests {
    use rustgis_data::raster::{ColorHint, Crs, GeoTransform};
    use rustgis_geom::lonlat_to_merc;
    use rustgis_map::Camera;
    use rustgis_map::raster::RasterStyle;

    use super::*;

    #[test]
    fn draws_a_utm_raster_where_it_belongs() {
        // 100 × 100 cells of 1 km in UTM 17N around 81°W, 42°N, valued by column.
        let gt = GeoTransform { origin_x: 450_000.0, origin_y: 4_700_000.0, pixel_width: 1000.0, pixel_height: -1000.0 };
        let data: Vec<f32> = (0..10_000).map(|i| (i % 100) as f32).collect();
        let r = Raster::new("utm", 100, 100, vec![data], gt, Crs::from_epsg(32617).unwrap(), None, ColorHint::Gray, "Float32").unwrap();
        let style = RasterStyle::default_for(&r);
        let camera = Camera { center: lonlat_to_merc(Coord::new(-81.0, 42.0)), res: 2000.0 };
        let view = View { width: 200.0, height: 200.0, pixels_per_point: 1.0, camera };
        let mut pm = Pixmap::new(200, 200).unwrap();
        draw(&mut pm, &r, &style, &view, 1.0, false, Place::Flat(&Flat::Mercator, "mercator"));
        let alpha = |x: u32, y: u32| pm.pixel(x, y).map(|p| p.alpha()).unwrap_or(0);
        assert_eq!(alpha(100, 100), 255, "the centre is covered");
        assert_eq!(alpha(2, 2), 0, "the corner of the view is outside the raster");
        // West is low (dark), east high (light).
        let lum = |x: u32| pm.pixel(x, 100).map(|p| p.red()).unwrap_or(0);
        assert!(lum(70) < lum(130), "{} vs {}", lum(70), lum(130));
        // Every renderer draws without panicking, at odd view sizes too.
        for kind in ["rgb", "classify", "unique", "shadedRelief"] {
            let mut s = style.clone();
            s.renderer = match kind {
                "rgb" => RasterRenderer::Rgb { red: 0, green: 0, blue: 0 },
                "classify" => rustgis_map::raster::classify(&r, 0, 5, Default::default(), Default::default()),
                "unique" => rustgis_map::raster::unique_values(&r, 0, Default::default()),
                _ => RasterRenderer::ShadedRelief { band: 0, ramp: None, azimuth: 315.0, altitude: 45.0, z_factor: 1.0 },
            };
            s.dra = true;
            s.resampling = Resampling::Bilinear;
            draw(&mut pm, &r, &s, &view, 0.5, false, Place::Flat(&Flat::Mercator, "mercator"));
        }
        // High-density screens draw at logical resolution, scaled up.
        let hi = View { pixels_per_point: 2.0, ..view };
        let mut pm2 = Pixmap::new(400, 400).unwrap();
        draw(&mut pm2, &r, &style, &hi, 1.0, false, Place::Flat(&Flat::Mercator, "mercator"));
        assert_eq!(pm2.pixel(200, 200).map(|p| p.alpha()), Some(255));
        assert_eq!(pm2.pixel(4, 4).map(|p| p.alpha()), Some(0));
        // Spread over threads, every pixel comes out as drawn on one, for each renderer.
        let wide = View { width: 420.0, height: 300.0, pixels_per_point: 1.0, camera };
        for kind in ["stretch", "rgb", "shadedRelief"] {
            let mut s = style.clone();
            s.renderer = match kind {
                "rgb" => RasterRenderer::Rgb { red: 0, green: 0, blue: 0 },
                "shadedRelief" => RasterRenderer::ShadedRelief { band: 0, ramp: None, azimuth: 315.0, altitude: 45.0, z_factor: 1.0 },
                _ => s.renderer,
            };
            let mut one = Pixmap::new(420, 300).unwrap();
            let mut many = Pixmap::new(420, 300).unwrap();
            draw_with(&mut one, &r, &s, &wide, 1.0, false, Place::Flat(&Flat::Mercator, "mercator"), 1);
            draw_with(&mut many, &r, &s, &wide, 1.0, false, Place::Flat(&Flat::Mercator, "mercator"), 7);
            assert!(one.data() == many.data(), "{kind}");
            assert!(one.pixels().iter().any(|p| p.alpha() == 255), "{kind} draws");
        }
        let far = View { width: 3.0, height: 1.0, pixels_per_point: 2.0, camera: Camera::world() };
        let mut tiny = Pixmap::new(6, 2).unwrap();
        draw(&mut tiny, &r, &style, &far, 1.0, true, Place::Flat(&Flat::Mercator, "mercator"));
    }
}
