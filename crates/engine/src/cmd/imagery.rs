//! Analyze tab, imagery groups: georeferencing, change detection, spectral profiles, classification (k-means and
//! minimum distance), indices, processing (clip, resample), the pixel editor, mensuration, and
//! ortho and reality mapping workspaces (mosaics of georeferenced images, DSM meshes).

use std::sync::Arc;

use rustgis_analysis::rasterops::{self, NODATA};
use rustgis_data::raster::{ColorHint, Crs, GeoTransform, Raster};
use rustgis_data::{Geometry, GeometryType};
use rustgis_geom::{Coord, Extent};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{b, f, ps, uint};
use crate::{CmdResult, CommandSpec as C, Session};

/// An imagery workspace: images mosaicked into one product.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Workspace {
    pub name: String,
    /// "ortho" (an orthomosaic of images) or "reality" (a surface model from elevation rasters).
    pub kind: String,
    pub images: Vec<String>,
    /// Output cell size in degrees (the finest image's when unset).
    #[serde(default)]
    pub cell_size: Option<f64>,
    /// Where images overlap: "first" (the first image wins) or "mean".
    #[serde(default = "first")]
    pub method: String,
}

fn first() -> String {
    "first".into()
}

pub fn commands() -> Vec<C> {
    vec![
        C::new("raster.georeference", "Georeference", "Analyze › Alignment", georeference).params(
            r#"{"layer": raster, "points"?: [[column, row, lon, lat], …] (two or more), "extent"?: [minLon, minLat, maxLon, maxLat]}"#,
        ),
        C::new("raster.change", "Change Detection", "Analyze › Image Analysis", change)
            .params(r#"{"before": raster, "after": raster, "band"?: 1-based, "method"?: "difference"|"ratio"|"percent", "name"?}"#),
        C::new("raster.spectral", "Spectral Analysis", "Analyze › Image Analysis", spectral)
            .view()
            .params(r#"{"layer": raster, "lon"?, "lat"?, "zone"?: polygon layer (the selected or first polygon)}"#),
        C::new("raster.classify", "Classification Wizard", "Analyze › Image Classification", |s, p| {
            let r = raster(s, p)?;
            let bands = bands_param(p, &r)?;
            if bands.is_empty() {
                return Err("give at least one band".into());
            }
            let k = match p.get("classes") {
                None => 5,
                Some(_) => uint(p, "classes").filter(|k| (2..=50).contains(k)).ok_or("classes is 2 to 50")? as usize,
            };
            let classes = rasterops::kmeans(&r, &bands, k, 20)?;
            add(s, rasterops::derive(&r, &name(p, format!("{}_classes", r.name)), vec![classes])?, true)
        })
        .params(r#"{"layer": raster, "classes"?: 2..50, "bands"?: [1-based], "name"?: output}"#),
        C::new("raster.classifySupervised", "Classification Tools", "Analyze › Image Classification", supervised)
            .params(r#"{"layer": raster, "training": polygon layer, "classField": field, "bands"?: [1-based], "name"?: output}"#),
        C::new("raster.index", "Indices", "Analyze › Imagery Tools", index).params(
            r#"{"layer": raster, "index": "ndvi"|"ndwi"|"savi"|"nbr"|"ndbi"|"evi", "red"?, "green"?, "blue"?, "nir"?, "swir"?: band numbers, "name"?: output}"#,
        ),
        C::new("raster.process", "Process", "Analyze › Imagery Tools", process)
            .params(r#"{"layer": raster, "operation": "clip"|"resample", "extent"?: [minLon, minLat, maxLon, maxLat] (clip; the view when omitted), "factor"?: n (resample: n×n pixels become one), "name"?: output}"#),
        C::new("raster.editPixels", "Pixel Editor", "Analyze › Imagery Tools", edit_pixels)
            .params(r#"{"layer": raster, "polygon": [[lon, lat], …], "value"?: number (no-data when omitted), "band"?: 1-based (all when omitted)}"#),
        // Measurements are session results, not project changes: no undo step.
        C::new("raster.measure", "Mensuration Tools", "Analyze › Mensuration", measure).view().params(
            r#"{"layer"?: elevation raster, "kind": "distance"|"area"|"height"|"volume", "points": [[lon, lat], …], "base"?: metres (volume)}"#,
        ),
        C::new("raster.measurements", "Results", "Analyze › Mensuration", |s, p| {
            if b(p, "clear").unwrap_or(false) {
                s.measurements.clear();
            }
            Ok(Value::Array(s.measurements.clone()))
        })
        .view()
        .params(r#"{"clear"?: bool}"#),
        C::new("imagery.workspace", "New Workspace", "Analyze › Ortho Mapping", new_workspace).params(
            r#"{"kind": "ortho"|"reality", "name"?: string, "images": [raster layers], "cellSize"?: degrees, "method"?: "first"|"mean"}"#,
        ),
        C::new("imagery.workspaceOptions", "Workspace Options", "Analyze › Ortho Mapping", |s, p| {
            let n = ps(p, "name").ok_or("name is required")?;
            let w = s.project.workspaces.iter_mut().find(|w| w.name.eq_ignore_ascii_case(n)).ok_or_else(|| format!("no workspace '{n}'"))?;
            if p.get("cellSize").is_some() {
                w.cell_size = cell_size(p)?;
            }
            if ps(p, "method").is_some() {
                w.method = method(p, "first")?;
            }
            serde_json::to_value(&*w).map_err(|e| e.to_string())
        })
        .params(r#"{"name": workspace, "cellSize"?: degrees|null, "method"?: "first"|"mean"}"#),
        C::new("imagery.mosaic", "Build Mosaic", "Analyze › Ortho Mapping", mosaic).params(r#"{"name": workspace}"#),
        C::new("imagery.mesh", "Export Mesh", "Analyze › Reality Mapping", mesh)
            .view()
            .params(r#"{"layer": elevation raster, "path"?: "*.obj" (the text is returned without one), "step"?: pixels between vertices}"#),
    ]
}

fn raster_named(s: &Session, n: &str) -> Result<Arc<Raster>, String> {
    s.map().layer_by_name(n).and_then(|l| l.raster.as_ref()).and_then(|r| r.data.clone()).ok_or_else(|| format!("no raster layer '{n}'"))
}

pub(crate) fn raster(s: &Session, p: &Value) -> Result<Arc<Raster>, String> {
    raster_named(s, ps(p, "layer").ok_or("layer is required")?)
}

pub(crate) fn name(p: &Value, d: String) -> String {
    ps(p, "name").map(str::trim).filter(|n| !n.is_empty()).map(|n| n.chars().take(200).collect()).unwrap_or(d)
}

pub(crate) fn bands_param(p: &Value, r: &Raster) -> Result<Vec<usize>, String> {
    match p.get("bands").and_then(Value::as_array) {
        Some(a) => a
            .iter()
            .map(|v| {
                v.as_u64()
                    .map(|b| b as usize)
                    .filter(|b| *b >= 1 && *b <= r.band_count())
                    .map(|b| b - 1)
                    .ok_or_else(|| format!("{} has bands 1 to {}", r.name, r.band_count()))
            })
            .collect(),
        None => Ok((0..r.band_count().min(4)).collect()),
    }
}

/// Embeds a raster as GeoTIFF bytes (colour rasters keep their bands; others band 1).
fn layer_of(r: Raster) -> Result<rustgis_map::RasterLayer, String> {
    let bytes = rustgis_data::raster::geotiff::write(&r, 0).map_err(|e| e.to_string())?;
    let fname = format!("{}.tif", r.name);
    Ok(rustgis_map::RasterLayer::from_bytes(r, fname, bytes))
}

/// Adds a raster layer; classes get unique-value colours.
pub(crate) fn add(s: &mut Session, r: Raster, classes: bool) -> CmdResult {
    let n = r.name.clone();
    let mut layer = layer_of(r)?;
    if classes && let Some(d) = layer.data.clone() {
        layer.style.renderer = rustgis_map::raster::unique_values(&d, 0, rustgis_map::Ramp::Harbour);
    }
    super::raster::add_raster(s, &n, layer)
}

/// Replaces a raster layer's pixels, keeping its look.
fn replace(s: &mut Session, layer: &str, r: Raster) -> CmdResult {
    let id = s.map().layer_by_name(layer).map(|l| l.id).ok_or_else(|| format!("no raster layer '{layer}'"))?;
    let mut new = layer_of(r)?;
    let l = s.map_mut().layer_mut(id).ok_or("no such layer")?;
    if let Some(old) = &l.raster {
        new.style = old.style.clone();
    }
    l.raster = Some(new);
    Ok(json!({"layer": id}))
}

fn rebuild(r: &Raster, transform: GeoTransform, crs: Crs, bands: Vec<Vec<f32>>) -> Result<Raster, String> {
    let mut out = Raster::new(r.name.clone(), r.width, r.height, bands, transform, crs, r.nodata, r.color.clone(), r.data_type.clone())
        .map_err(|e| e.to_string())?;
    for (o, i) in out.bands.iter_mut().zip(&r.bands) {
        o.name = i.name.clone();
    }
    Ok(out)
}

/// Fits a north-up longitude/latitude grid to control points (least squares, separately in x
/// and y), or stretches the raster over an extent.
fn georeference(s: &mut Session, p: &Value) -> CmdResult {
    let layer = ps(p, "layer").ok_or("layer is required")?.to_string();
    let r = raster(s, p)?;
    let gt = if let Some(e) = p.get("extent").and_then(Value::as_array) {
        let n: Vec<f64> = e.iter().filter_map(Value::as_f64).collect();
        let [x0, y0, x1, y1] = n.as_slice() else { return Err("extent is [minLon, minLat, maxLon, maxLat]".into()) };
        if !(x0 < x1 && y0 < y1) {
            return Err("extent is [minLon, minLat, maxLon, maxLat]".into());
        }
        GeoTransform { origin_x: *x0, origin_y: *y1, pixel_width: (x1 - x0) / r.width as f64, pixel_height: -(y1 - y0) / r.height as f64 }
    } else {
        let pts: Vec<[f64; 4]> = p
            .get("points")
            .and_then(Value::as_array)
            .ok_or("give control points or an extent")?
            .iter()
            .take(1000)
            .map(|v| {
                let a: Vec<f64> =
                    v.as_array().map(|a| a.iter().filter_map(Value::as_f64).filter(|x| x.is_finite()).collect()).unwrap_or_default();
                if let [c, rr, x, y] = a.as_slice() {
                    Ok([*c, *rr, *x, *y])
                } else {
                    Err("each control point is [column, row, lon, lat]".to_string())
                }
            })
            .collect::<Result<_, _>>()?;
        if pts.len() < 2 {
            return Err("give at least two control points [column, row, lon, lat]".into());
        }
        // y = a·t + b by least squares.
        let fit = |ts: &[f64], ys: &[f64]| -> Option<(f64, f64)> {
            let n = ts.len() as f64;
            let (mt, my) = (ts.iter().sum::<f64>() / n, ys.iter().sum::<f64>() / n);
            let stt: f64 = ts.iter().map(|t| (t - mt).powi(2)).sum();
            if stt < 1e-12 {
                return None;
            }
            let a = ts.iter().zip(ys).map(|(t, y)| (t - mt) * (y - my)).sum::<f64>() / stt;
            Some((a, my - a * mt))
        };
        let (cols, lons): (Vec<f64>, Vec<f64>) = pts.iter().map(|p| (p[0], p[2])).unzip();
        let (rows, lats): (Vec<f64>, Vec<f64>) = pts.iter().map(|p| (p[1], p[3])).unzip();
        let (ax, bx) = fit(&cols, &lons).ok_or("the control points need different columns")?;
        let (ay, by) = fit(&rows, &lats).ok_or("the control points need different rows")?;
        if ax <= 0.0 || ay >= 0.0 {
            return Err("the points must keep north up (longitude rising with columns, latitude falling with rows)".into());
        }
        GeoTransform { origin_x: bx, origin_y: by, pixel_width: ax, pixel_height: ay }
    };
    let out = rebuild(&r, gt, Crs::LonLat, r.bands.iter().map(|b| b.data.clone()).collect())?;
    let e = out.extent();
    let v = replace(s, &layer, out)?;
    Ok(json!({"layer": v["layer"], "extent": [e.min_x, e.min_y, e.max_x, e.max_y]}))
}

fn change(s: &mut Session, p: &Value) -> CmdResult {
    let a = raster_named(s, ps(p, "before").ok_or("before is required")?)?;
    let bfr = raster_named(s, ps(p, "after").ok_or("after is required")?)?;
    // The method is checked before any pixels are resampled.
    let method = ps(p, "method").unwrap_or("difference");
    if !matches!(method, "difference" | "ratio" | "percent") {
        return Err(format!("unknown method '{method}'"));
    }
    let band = uint(p, "band").unwrap_or(1).saturating_sub(1) as usize;
    let av: Vec<f32> = a.bands.get(band).ok_or("no such band")?.data.clone();
    let bv = rasterops::resample_to(&bfr, band, &a)?;
    let out: Vec<f32> = av
        .iter()
        .zip(&bv)
        .map(|(x, y)| {
            if !a.is_valid(*x) || *y == NODATA {
                return NODATA;
            }
            let v = match method {
                "ratio" if *x != 0.0 => y / x,
                "percent" if *x != 0.0 => (y - x) / x * 100.0,
                "difference" => y - x,
                _ => f32::NAN,
            };
            if v.is_finite() { v } else { NODATA }
        })
        .collect();
    add(s, rasterops::derive(&a, &name(p, format!("{}_change", a.name)), vec![out])?, false)
}

fn spectral(s: &mut Session, p: &Value) -> CmdResult {
    let r = raster(s, p)?;
    let all: Vec<usize> = (0..r.band_count()).collect();
    let names: Vec<String> = r.bands.iter().map(|b| b.name.clone()).collect();
    if let Some(z) = ps(p, "zone") {
        let l = s
            .map()
            .layer_by_name(z)
            .filter(|l| l.geometry_type() == GeometryType::Polygon)
            .ok_or_else(|| format!("no polygon layer '{z}'"))?;
        let row = l.selection.first().copied().unwrap_or(0);
        let Some(Geometry::Polygon(polys)) = l.dataset.features.get(row).and_then(|f| f.geometry.as_ref()) else {
            return Err("the zone has no polygon".into());
        };
        let rings = polys.first().ok_or("the zone has no polygon")?;
        let (means, n) = rasterops::zonal_means(&r, &all, rings).ok_or("no pixels fall in the zone")?;
        return Ok(json!({"bands": names, "values": means, "pixels": n}));
    }
    let c = Coord::new(f(p, "lon").ok_or("give lon and lat, or a zone")?, f(p, "lat").ok_or("lat is required")?);
    let (col, row) = r.pixel_at(c).ok_or("the point is outside the raster")?;
    let vals: Vec<Option<f32>> = all.iter().map(|b| r.value(*b, col, row)).collect();
    Ok(json!({"bands": names, "values": vals, "pixel": [col, row]}))
}

fn supervised(s: &mut Session, p: &Value) -> CmdResult {
    let r = raster(s, p)?;
    let bands = bands_param(p, &r)?;
    let tl = ps(p, "training").ok_or("training (a polygon layer) is required")?;
    let l = s
        .map()
        .layer_by_name(tl)
        .filter(|l| l.geometry_type() == GeometryType::Polygon)
        .ok_or_else(|| format!("no polygon layer '{tl}'"))?;
    let fi = l.dataset.field_index(ps(p, "classField").ok_or("classField is required")?).ok_or("no such class field")?;
    // Mean of each class's training pixels.
    let mut classes: Vec<(String, Vec<f64>, usize)> = Vec::new();
    for (row, ft) in l.dataset.features.iter().enumerate() {
        let Some(Geometry::Polygon(polys)) = &ft.geometry else { continue };
        let key = l.dataset.value(row, fi).to_string();
        for rings in polys {
            if let Some((m, n)) = rasterops::zonal_means(&r, &bands, rings) {
                match classes.iter_mut().find(|c| c.0 == key) {
                    Some(c) => {
                        let total = c.2 + n;
                        c.1 = c.1.iter().zip(&m).map(|(a, b)| (a * c.2 as f64 + b * n as f64) / total as f64).collect();
                        c.2 = total;
                    }
                    None => classes.push((key.clone(), m, n)),
                }
            }
        }
    }
    if classes.len() < 2 {
        return Err("the training polygons need at least two classes with pixels under them".into());
    }
    let means: Vec<(f32, Vec<f64>)> = classes.iter().enumerate().map(|(i, c)| ((i + 1) as f32, c.1.clone())).collect();
    let out = rasterops::classify_min_distance(&r, &bands, &means)?;
    let v = add(s, rasterops::derive(&r, &name(p, format!("{}_classified", r.name)), vec![out])?, true)?;
    let legend: Vec<Value> = classes.iter().enumerate().map(|(i, c)| json!({"value": i + 1, "class": c.0, "pixels": c.2})).collect();
    Ok(json!({"layer": v["layer"], "classes": legend}))
}

fn index(s: &mut Session, p: &Value) -> CmdResult {
    let r = raster(s, p)?;
    let bn = |k: &str, d: u64| uint(p, k).unwrap_or(d);
    let (blue, green, red, nir, swir) = (bn("blue", 1), bn("green", 2), bn("red", 3), bn("nir", 4), bn("swir", 5));
    let idx = ps(p, "index").ok_or("index is required")?;
    let expr = match idx {
        "ndvi" => format!("(b{nir} - b{red}) / (b{nir} + b{red})"),
        "ndwi" => format!("(b{green} - b{nir}) / (b{green} + b{nir})"),
        "savi" => format!("(b{nir} - b{red}) * 1.5 / (b{nir} + b{red} + 0.5)"),
        "nbr" => format!("(b{nir} - b{swir}) / (b{nir} + b{swir})"),
        "ndbi" => format!("(b{swir} - b{nir}) / (b{swir} + b{nir})"),
        "evi" => format!("2.5 * (b{nir} - b{red}) / (b{nir} + 6 * b{red} - 7.5 * b{blue} + 1)"),
        o => return Err(format!("unknown index '{o}'")),
    };
    let out = rasterops::band_math(&r, &expr)?;
    add(s, rasterops::derive(&r, &name(p, format!("{}_{idx}", r.name)), vec![out])?, false)
}

fn process(s: &mut Session, p: &Value) -> CmdResult {
    let r = raster(s, p)?;
    match ps(p, "operation").ok_or("operation is required")? {
        "clip" => {
            let e = match p.get("extent").and_then(Value::as_array) {
                Some(a) => {
                    let n: Vec<f64> = a.iter().filter_map(Value::as_f64).collect();
                    let [x0, y0, x1, y1] = n.as_slice() else { return Err("extent is [minLon, minLat, maxLon, maxLat]".into()) };
                    Extent::new(*x0, *y0, *x1, *y1)
                }
                None => {
                    let m = s.map();
                    let (w, h) = s.view_size;
                    let ce = m.camera.extent(w, h);
                    let mut e = Extent::EMPTY;
                    for c in [Coord::new(ce.min_x, ce.min_y), Coord::new(ce.max_x, ce.max_y)] {
                        if let Some(ll) = m.to_lonlat(c) {
                            e.expand(ll);
                        }
                    }
                    e
                }
            };
            // The pixel window covering the extent's corners.
            let corners =
                [Coord::new(e.min_x, e.min_y), Coord::new(e.max_x, e.max_y), Coord::new(e.min_x, e.max_y), Coord::new(e.max_x, e.min_y)];
            let px: Vec<(f64, f64)> =
                corners.iter().filter_map(|c| r.transformer().from_lonlat(*c)).map(|w| r.transform.to_pixel(w)).collect();
            if px.is_empty() {
                return Err("the extent can't be placed on the raster".into());
            }
            let c0 = px.iter().map(|p| p.0).fold(f64::INFINITY, f64::min).floor().max(0.0) as u32;
            let c1 = px.iter().map(|p| p.0).fold(f64::NEG_INFINITY, f64::max).ceil().min(r.width as f64) as u32;
            let r0 = px.iter().map(|p| p.1).fold(f64::INFINITY, f64::min).floor().max(0.0) as u32;
            let r1 = px.iter().map(|p| p.1).fold(f64::NEG_INFINITY, f64::max).ceil().min(r.height as f64) as u32;
            if c1 <= c0 || r1 <= r0 {
                return Err("the extent doesn't overlap the raster".into());
            }
            let (w, h) = (c1 - c0, r1 - r0);
            let bands: Vec<Vec<f32>> = r
                .bands
                .iter()
                .map(|b| {
                    (r0..r1)
                        .flat_map(|row| (c0..c1).map(move |col| (row, col)))
                        .map(|(row, col)| b.data.get((row * r.width + col) as usize).copied().unwrap_or(NODATA))
                        .collect()
                })
                .collect();
            let o = r.transform.to_world(c0 as f64, r0 as f64);
            let gt = GeoTransform { origin_x: o.x, origin_y: o.y, ..r.transform };
            let mut out =
                Raster::new(name(p, format!("{}_clip", r.name)), w, h, bands, gt, r.crs, r.nodata, r.color.clone(), r.data_type.clone())
                    .map_err(|e| e.to_string())?;
            out.bands.iter_mut().zip(&r.bands).for_each(|(o, i)| o.name = i.name.clone());
            add(s, out, false)
        }
        "resample" => {
            // Never coarser than the raster itself; trailing columns and rows short of a full
            // block are left out.
            let k = (uint(p, "factor").unwrap_or(2).clamp(2, 64) as u32).min(r.width.min(r.height)).max(1);
            let (w, h) = ((r.width / k).max(1), (r.height / k).max(1));
            // Each output pixel is the mean of the valid pixels it covers.
            let bands: Vec<Vec<f32>> = r
                .bands
                .iter()
                .map(|b| {
                    (0..h)
                        .flat_map(|row| (0..w).map(move |col| (row, col)))
                        .map(|(row, col)| {
                            let (mut sum, mut n) = (0.0f64, 0u32);
                            for dy in 0..k {
                                for dx in 0..k {
                                    let (x, y) = (col * k + dx, row * k + dy);
                                    if x >= r.width || y >= r.height {
                                        continue;
                                    }
                                    if let Some(v) = b.data.get((y * r.width + x) as usize).copied().filter(|v| r.is_valid(*v)) {
                                        sum += v as f64;
                                        n += 1;
                                    }
                                }
                            }
                            if n > 0 { (sum / n as f64) as f32 } else { NODATA }
                        })
                        .collect()
                })
                .collect();
            let gt = GeoTransform {
                pixel_width: r.transform.pixel_width * k as f64,
                pixel_height: r.transform.pixel_height * k as f64,
                ..r.transform
            };
            let nodata = r.nodata.or(Some(NODATA as f64));
            let color = if matches!(r.color, ColorHint::Palette(_)) { ColorHint::Gray } else { r.color.clone() };
            let out = Raster::new(name(p, format!("{}_resampled", r.name)), w, h, bands, gt, r.crs, nodata, color, r.data_type.clone())
                .map_err(|e| e.to_string())?;
            add(s, out, false)
        }
        o => Err(format!("unknown operation '{o}'")),
    }
}

/// A point given as [lon, lat]; anything else is an error (a dropped point would give a wrong
/// answer that looks right).
fn point(v: &Value) -> Result<Coord, String> {
    match (v.get(0).and_then(Value::as_f64), v.get(1).and_then(Value::as_f64)) {
        (Some(x), Some(y)) if x.is_finite() && y.is_finite() => Ok(Coord::new(x, y)),
        _ => Err(format!("each point is [lon, lat], not {v}")),
    }
}

/// The columns and rows of a raster under a longitude/latitude box, or `None` when it misses.
fn pixel_window(r: &Raster, e: &Extent) -> Option<(std::ops::Range<u32>, std::ops::Range<u32>)> {
    let tr = r.transformer();
    let (mut c0, mut r0, mut c1, mut r1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for corner in [Coord::new(e.min_x, e.min_y), Coord::new(e.max_x, e.min_y), Coord::new(e.max_x, e.max_y), Coord::new(e.min_x, e.max_y)] {
        if let Some(w) = tr.from_lonlat(corner) {
            let (c, rr) = r.transform.to_pixel(w);
            (c0, r0, c1, r1) = (c0.min(c), r0.min(rr), c1.max(c), r1.max(rr));
        }
    }
    if c0 > c1 || c1 < 0.0 || r1 < 0.0 || c0 > f64::from(r.width) || r0 > f64::from(r.height) {
        return None;
    }
    let cols = (c0.floor().max(0.0) as u32).min(r.width)..(c1.ceil().max(0.0) as u32 + 1).min(r.width);
    let rows = (r0.floor().max(0.0) as u32).min(r.height)..(r1.ceil().max(0.0) as u32 + 1).min(r.height);
    Some((cols, rows))
}

fn edit_pixels(s: &mut Session, p: &Value) -> CmdResult {
    let layer = ps(p, "layer").ok_or("layer is required")?.to_string();
    let r = raster(s, p)?;
    let ring: Vec<Coord> = p
        .get("polygon")
        .and_then(Value::as_array)
        .ok_or("polygon is required")?
        .iter()
        .take(10_000)
        .map(point)
        .collect::<Result<_, _>>()?;
    if ring.len() < 3 {
        return Err("the polygon needs at least three points".into());
    }
    let mut e = Extent::EMPTY;
    ring.iter().for_each(|c| e.expand(*c));
    let rings = vec![ring];
    let value = match f(p, "value") {
        Some(v) => v as f32,
        None => r.nodata.map(|v| v as f32).unwrap_or(NODATA),
    };
    let only = match p.get("band") {
        None => None,
        Some(_) => Some(
            uint(p, "band")
                .filter(|b| *b >= 1 && *b <= r.band_count() as u64)
                .ok_or_else(|| format!("{} has bands 1 to {}", r.name, r.band_count()))? as usize
                - 1,
        ),
    };
    let mut bands: Vec<Vec<f32>> = r.bands.iter().map(|b| b.data.clone()).collect();
    let mut n = 0usize;
    // Only the pixels under the polygon's box are visited.
    let (cols, rows) = pixel_window(&r, &e).ok_or("the polygon is outside the raster")?;
    let tr = r.transformer();
    for row in rows {
        for col in cols.clone() {
            let w = r.transform.to_world(col as f64 + 0.5, row as f64 + 0.5);
            let Some(ll) = tr.to_lonlat(w) else { continue };
            if !e.contains(ll) || !rustgis_data::geometry::polygon_contains(&rings, ll) {
                continue;
            }
            for (bi, b) in bands.iter_mut().enumerate() {
                if only.is_none_or(|o| o == bi)
                    && let Some(v) = b.get_mut((row * r.width + col) as usize)
                {
                    *v = value;
                }
            }
            n += 1;
        }
    }
    if n == 0 {
        return Err("no pixel centres fall in the polygon".into());
    }
    let nodata = if f(p, "value").is_none() && r.nodata.is_none() { Some(NODATA as f64) } else { r.nodata };
    let mut out = Raster::new(r.name.clone(), r.width, r.height, bands, r.transform, r.crs, nodata, r.color.clone(), r.data_type.clone())
        .map_err(|e| e.to_string())?;
    out.bands.iter_mut().zip(&r.bands).for_each(|(o, i)| o.name = i.name.clone());
    replace(s, &layer, out)?;
    Ok(json!({"pixels": n}))
}

fn measure(s: &mut Session, p: &Value) -> CmdResult {
    let pts: Vec<Coord> =
        p.get("points").and_then(Value::as_array).ok_or("points is required")?.iter().take(10_000).map(point).collect::<Result<_, _>>()?;
    let kind = ps(p, "kind").ok_or("kind is required")?;
    let dem = ps(p, "layer").map(|n| raster_named(s, n)).transpose()?;
    let z = |c: Coord| dem.as_ref().and_then(|d| rasterops::elevation_at(d, 0, c));
    let out = match kind {
        "distance" => {
            if pts.len() < 2 {
                return Err("a distance needs two or more points".into());
            }
            let flat = rustgis_geom::geodesic_length_m(&pts);
            // Along the surface when there's an elevation raster.
            let surface = dem.as_ref().map(|d| {
                let prof = rasterops::profile(d, 0, &pts, 500);
                prof.windows(2)
                    .map(|w| match w {
                        [a, c] => {
                            let dh = match (a.2, c.2) {
                                (Some(x), Some(y)) => y - x,
                                _ => 0.0,
                            };
                            ((c.0 - a.0).powi(2) + dh * dh).sqrt()
                        }
                        _ => 0.0,
                    })
                    .sum::<f64>()
            });
            json!({"kind": "distance", "metres": flat, "surfaceMetres": surface})
        }
        "area" => {
            if pts.len() < 3 {
                return Err("an area needs three or more points".into());
            }
            let mut ring = pts.clone();
            if let Some(first) = pts.first() {
                ring.push(*first);
            }
            json!({"kind": "area", "squareMetres": rustgis_geom::geodesic_ring_area_m2(&ring), "perimeterMetres": rustgis_geom::geodesic_length_m(&ring)})
        }
        "height" => {
            let (Some(a), Some(c)) = (pts.first(), pts.get(1)) else { return Err("a height needs two points (base, top)".into()) };
            let (za, zc) = (z(*a).ok_or("the base is off the elevation raster")?, z(*c).ok_or("the top is off the elevation raster")?);
            json!({"kind": "height", "metres": zc - za, "base": za, "top": zc})
        }
        "volume" => {
            let d = dem.as_ref().ok_or("a volume needs an elevation raster (layer)")?;
            if pts.len() < 3 {
                return Err("a volume needs a polygon of three or more points".into());
            }
            let base = f(p, "base");
            let rings = vec![pts.clone()];
            let (cx, cy) = d.cell_size_m();
            let (mut above, mut below, mut n, mut lowest) = (0.0, 0.0, 0usize, f64::INFINITY);
            let mut cells = Vec::new();
            let mut e = Extent::EMPTY;
            pts.iter().for_each(|c| e.expand(*c));
            let (cols, rows) = pixel_window(d, &e).ok_or("the polygon is outside the raster")?;
            let tr = d.transformer();
            for row in rows.map(i64::from) {
                for col in cols.clone().map(i64::from) {
                    let w = d.transform.to_world(col as f64 + 0.5, row as f64 + 0.5);
                    let Some(ll) = tr.to_lonlat(w) else { continue };
                    if !e.contains(ll) || !rustgis_data::geometry::polygon_contains(&rings, ll) {
                        continue;
                    }
                    if let Some(v) = d.value(0, col, row) {
                        cells.push(v as f64);
                        lowest = lowest.min(v as f64);
                    }
                }
            }
            let base = base.unwrap_or(lowest);
            for v in &cells {
                let dz = v - base;
                if dz > 0.0 {
                    above += dz * cx * cy;
                } else {
                    below -= dz * cx * cy;
                }
                n += 1;
            }
            if n == 0 {
                return Err("no cells fall in the polygon".into());
            }
            json!({"kind": "volume", "cubicMetresAbove": above, "cubicMetresBelow": below, "base": base, "cells": n})
        }
        k => return Err(format!("unknown measurement '{k}'")),
    };
    s.measurements.push(out.clone());
    if s.measurements.len() > 500 {
        s.measurements.remove(0);
    }
    Ok(out)
}

/// A workspace's cell size (degrees, or null for the finest image's), checked the same way when
/// a workspace is made and when its options change.
fn cell_size(p: &Value) -> Result<Option<f64>, String> {
    match p.get("cellSize") {
        None | Some(Value::Null) => Ok(None),
        Some(_) => f(p, "cellSize")
            .filter(|c| *c > 0.0 && *c < 10.0)
            .map(Some)
            .ok_or_else(|| "cellSize is degrees, above 0 and below 10".to_string()),
    }
}

/// A workspace's mosaic method: first or mean.
fn method(p: &Value, default: &str) -> Result<String, String> {
    match ps(p, "method").unwrap_or(default) {
        m @ ("first" | "mean") => Ok(m.to_string()),
        m => Err(format!("method is first or mean, not '{m}'")),
    }
}

fn new_workspace(s: &mut Session, p: &Value) -> CmdResult {
    let kind = ps(p, "kind").ok_or("kind is required")?;
    if !matches!(kind, "ortho" | "reality") {
        return Err("kind is ortho or reality".into());
    }
    let images: Vec<String> = p
        .get("images")
        .and_then(Value::as_array)
        .ok_or("images is required")?
        .iter()
        .map(|v| {
            let n = v.as_str().ok_or("images are raster layer names")?;
            s.map().layer_by_name(n).filter(|l| l.is_raster()).map(|l| l.name.clone()).ok_or_else(|| format!("no raster layer '{n}'"))
        })
        .collect::<Result<_, String>>()?;
    if images.is_empty() {
        return Err("give at least one image".into());
    }
    let base = ps(p, "name")
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .unwrap_or(if kind == "ortho" { "Ortho Mapping" } else { "Reality Mapping" })
        .to_string();
    let mut nm = base.clone();
    let mut i = 1;
    while s.project.workspaces.iter().any(|w| w.name.eq_ignore_ascii_case(&nm)) {
        i += 1;
        nm = format!("{base} {i}");
    }
    let method = method(p, if kind == "reality" { "mean" } else { "first" })?;
    let cell_size = cell_size(p)?;
    s.project.workspaces.push(Workspace { name: nm.clone(), kind: kind.into(), images, cell_size, method });
    Ok(json!({"workspace": nm}))
}

/// Mosaics a workspace's images onto one longitude/latitude grid.
fn mosaic(s: &mut Session, p: &Value) -> CmdResult {
    let n = ps(p, "name").ok_or("name is required")?;
    let w = s.project.workspaces.iter().find(|w| w.name.eq_ignore_ascii_case(n)).cloned().ok_or_else(|| format!("no workspace '{n}'"))?;
    let imgs: Vec<Arc<Raster>> = w.images.iter().map(|i| raster_named(s, i)).collect::<Result<_, _>>()?;
    let mut e = Extent::EMPTY;
    for r in &imgs {
        e = e.union(&r.extent());
    }
    let finest = imgs
        .iter()
        .map(|r| {
            let (cx, _) = r.cell_size_m();
            cx / (111_320.0 * r.extent().center().y.to_radians().cos().abs().max(0.01))
        })
        .fold(f64::INFINITY, f64::min);
    let cell = w.cell_size.unwrap_or(finest).max(1e-7);
    let (cols, rows) = ((e.width() / cell).ceil().max(1.0) as usize, (e.height() / cell).ceil().max(1.0) as usize);
    if cols.saturating_mul(rows) > rasterops::MAX_PIXELS / 2 {
        return Err(format!("the mosaic would be {cols} × {rows} cells; set a larger cell size in the workspace options"));
    }
    let nb = imgs.iter().map(|r| r.band_count()).min().unwrap_or(1).min(if w.kind == "reality" { 1 } else { 4 });
    let mut bands = vec![vec![NODATA; cols * rows]; nb];
    for row in 0..rows {
        for col in 0..cols {
            let c = Coord::new(e.min_x + (col as f64 + 0.5) * cell, e.max_y - (row as f64 + 0.5) * cell);
            // Where the cell falls in each image, once for all bands.
            let at: Vec<Option<(i64, i64)>> = imgs.iter().map(|r| r.pixel_at(c)).collect();
            for (bi, band) in bands.iter_mut().enumerate() {
                let vals = imgs.iter().zip(&at).filter_map(|(r, a)| a.and_then(|(cc, rr)| r.value(bi, cc, rr)));
                let v = if w.method == "mean" {
                    let (sum, k) = vals.fold((0.0f64, 0usize), |(s, k), v| (s + v as f64, k + 1));
                    (k > 0).then(|| (sum / k as f64) as f32)
                } else {
                    vals.into_iter().next()
                };
                if let (Some(v), Some(slot)) = (v, band.get_mut(row * cols + col)) {
                    *slot = v;
                }
            }
        }
    }
    // Colour only when every image is colour. A mosaic of 8-bit images stays 8-bit (means
    // rounded), so it displays as they do; anything else is Float32.
    let all_rgb = imgs.iter().all(|r| matches!(r.color, ColorHint::Rgb { .. }));
    let color = if matches!(nb, 3 | 4) && all_rgb { ColorHint::Rgb { alpha: nb == 4 } } else { ColorHint::Gray };
    let all_u8 = imgs.iter().all(|r| r.data_type == "UInt8");
    let dt = if matches!(color, ColorHint::Rgb { .. }) && all_u8 { "UInt8" } else { "Float32" };
    if dt == "UInt8" {
        for v in bands.iter_mut().flatten().filter(|v| **v != NODATA) {
            *v = v.round().clamp(0.0, 255.0);
        }
    }
    let gt = GeoTransform { origin_x: e.min_x, origin_y: e.max_y, pixel_width: cell, pixel_height: -cell };
    let suffix = if w.kind == "reality" { "DSM" } else { "Orthomosaic" };
    let out = Raster::new(format!("{} {suffix}", w.name), cols as u32, rows as u32, bands, gt, Crs::LonLat, Some(NODATA as f64), color, dt)
        .map_err(|e| e.to_string())?;
    add(s, out, false)
}

/// A surface model as a triangle mesh (Wavefront OBJ), in metres from its south-west corner.
fn mesh(s: &mut Session, p: &Value) -> CmdResult {
    if ps(p, "path").is_some_and(|path| !path.to_ascii_lowercase().ends_with(".obj")) {
        return Err("the mesh is written to an .obj file".into());
    }
    let r = raster(s, p)?;
    let step = uint(p, "step").unwrap_or(((r.width.max(r.height) / 300).max(1)) as u64).clamp(1, 1000) as i64;
    let (cx, cy) = r.cell_size_m();
    let (nc, nr) = ((r.width as i64 - 1) / step + 1, (r.height as i64 - 1) / step + 1);
    if nc * nr > 1_000_000 {
        return Err("the mesh would be too large; use a larger step".into());
    }
    let mut obj = format!("# {} surface mesh exported by RustGIS\no {}\n", r.name, r.name.replace(char::is_whitespace, "_"));
    let mut ids = vec![0usize; (nc * nr) as usize];
    let mut v = 0usize;
    for j in 0..nr {
        for i in 0..nc {
            if let Some(z) = r.value(0, i * step, j * step) {
                v += 1;
                if let Some(slot) = ids.get_mut((j * nc + i) as usize) {
                    *slot = v;
                }
                obj.push_str(&format!("v {:.2} {:.2} {:.2}\n", i as f64 * step as f64 * cx, z, j as f64 * step as f64 * cy));
            }
        }
    }
    let id = |i: i64, j: i64| ids.get((j * nc + i) as usize).copied().unwrap_or(0);
    let mut faces = 0;
    for j in 0..nr - 1 {
        for i in 0..nc - 1 {
            let (a, b2, c, d) = (id(i, j), id(i + 1, j), id(i + 1, j + 1), id(i, j + 1));
            if a > 0 && b2 > 0 && c > 0 {
                obj.push_str(&format!("f {a} {c} {b2}\n"));
                faces += 1;
            }
            if a > 0 && c > 0 && d > 0 {
                obj.push_str(&format!("f {a} {d} {c}\n"));
                faces += 1;
            }
        }
    }
    if faces == 0 {
        return Err("the raster has no surface to mesh".into());
    }
    match ps(p, "path") {
        Some(path) => {
            rustgis_data::io::write_atomic(std::path::Path::new(path), obj.as_bytes()).map_err(|e| format!("{path}: {e}"))?;
            Ok(json!({"path": path, "vertices": v, "faces": faces}))
        }
        None => Ok(json!({"text": obj, "vertices": v, "faces": faces})),
    }
}
