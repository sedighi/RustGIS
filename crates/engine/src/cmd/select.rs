//! Home › Selection.

use rustgis_geom::{Coord, Extent};
use rustgis_map::PopupMode;
use serde_json::{Value, json};

use super::{f, need_layer, need_layers, need_selection, ps};
use crate::{CmdResult, CommandSpec as C, Session};

pub fn commands() -> Vec<C> {
    vec![
        C::new("select.byAttributes", "Select By Attributes", "Home › Selection", by_attributes)
            .when(need_layers)
            .params(r#"{"layer"?, "where": "SQL clause", "mode"?: "new"|"add"|"remove"|"subset"}"#),
        C::new("select.byRect", "Select (rectangle)", "Home › Selection", by_rect)
            .params(r#"{"minLon", "minLat", "maxLon", "maxLat", "mode"?: "new"|"add"|"remove"}"#),
        C::new("select.atPoint", "Select (click)", "Home › Selection", at_point).params(r#"{"lon", "lat", "tolerancePx"?, "mode"?}"#),
        C::new("select.rows", "Select Rows", "Attribute Table", |s, p| {
            let id = s.layer_id(p)?;
            let rows: Vec<usize> = p
                .get("rows")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(|v| v.as_u64()).map(|v| v as usize).collect())
                .unwrap_or_default();
            set(s, id, rows, mode(p));
            Ok(Value::Null)
        })
        .params(r#"{"layer"?, "rows": [number], "mode"?}"#),
        C::new("select.clear", "Clear", "Home › Selection", |s, _| {
            s.map_mut().clear_selection();
            Ok(Value::Null)
        })
        .when(need_selection),
        C::new("select.all", "Select All", "Data › Selection", |s, p| {
            let id = s.layer_id(p)?;
            let n = s.map().layer(id).map(|l| l.dataset.features.len()).unwrap_or(0);
            set(s, id, (0..n).collect(), Mode::New);
            Ok(Value::Null)
        }).params(r#"{"layer"?: name|id (default: the active layer)}"#)
        .when(need_layer),
        C::new("select.switch", "Switch Selection", "Data › Selection", |s, p| {
            let id = s.layer_id(p)?;
            let l = s.map().layer(id).ok_or("no layer")?;
            let rows: Vec<usize> = (0..l.dataset.features.len()).filter(|r| !l.selection.contains(r)).collect();
            set(s, id, rows, Mode::New);
            Ok(Value::Null)
        }).params(r#"{"layer"?: name|id (default: the active layer)}"#)
        .when(need_layer),
        C::new("select.zoomTo", "Zoom To Selection", "Home › Selection", |s, _| {
            let e = s.map().selection_extent();
            if e.is_empty() {
                return Err("nothing with a shape is selected".into());
            }
            s.zoom_to(&e);
            Ok(Value::Null)
        })
        .view()
        .when(need_selection),
        C::new("select.layerFromSelection", "Make Layer From Selected Features", "Data › Selection", |s, p| {
            let id = s.layer_id(p)?;
            let l = s.map().layer(id).ok_or("no layer")?;
            if l.selection.is_empty() {
                return Err("nothing is selected in this layer".into());
            }
            let mut ds = rustgis_data::Dataset::new(format!("{} selection", l.name), l.geometry_type(), l.dataset.fields.clone());
            for r in &l.selection {
                if let Some(f) = l.dataset.features.get(*r) {
                    ds.push(f.geometry.clone(), f.attrs.clone());
                }
            }
            let nid = super::layer::add_dataset(s, ds);
            Ok(json!({"layer": nid}))
        }).params(r#"{"layer"?: name|id (default: the active layer, which need not be the layer last selected in)}"#)
        .when(need_selection),
        C::new("identify", "Identify", "Home › Inspect", identify).view().params(
            r#"{"lon", "lat", "tolerancePx"?, "mode"?: "topmost"|"visible"|"selectable"|"selectedInContents"|"none" (the map's pop-up mode when omitted; the mode picks the layers, and every feature under the point in them, up to 100, is in "results")}"#,
        ),
        C::new("map.popupMode", "Pop-up Mode", "Home › Navigate", popup_mode)
            .params(r#"{"mode": "topmost"|"visible"|"selectable"|"selectedInContents"|"none"}"#),
    ]
}

#[derive(Clone, Copy, PartialEq)]
pub enum Mode {
    New,
    Add,
    Remove,
    Subset,
}

fn mode(p: &Value) -> Mode {
    match ps(p, "mode").unwrap_or("new").to_ascii_lowercase().as_str() {
        "add" => Mode::Add,
        "remove" => Mode::Remove,
        "subset" => Mode::Subset,
        _ => Mode::New,
    }
}

/// Applies a selection to one layer (New clears the others too, like desktop GIS).
pub fn set(s: &mut Session, id: u32, rows: Vec<usize>, m: Mode) {
    if m == Mode::New {
        s.map_mut().clear_selection();
    }
    let Some(l) = s.map_mut().layer_mut(id) else { return };
    let n = l.dataset.features.len();
    let rows: Vec<usize> = rows.into_iter().filter(|r| *r < n).collect();
    match m {
        Mode::New => l.selection = rows,
        Mode::Add => {
            for r in rows {
                if !l.selection.contains(&r) {
                    l.selection.push(r);
                }
            }
        }
        Mode::Remove => l.selection.retain(|r| !rows.contains(r)),
        Mode::Subset => l.selection.retain(|r| rows.contains(r)),
    }
    l.selection.sort_unstable();
    l.selection.dedup();
}

fn by_attributes(s: &mut Session, p: &Value) -> CmdResult {
    let id = s.layer_id(p)?;
    let w = ps(p, "where").ok_or("where is required")?;
    let rows = rustgis_data::query::select(w, &s.map().layer(id).ok_or("no layer")?.dataset)?;
    let n = rows.len();
    set(s, id, rows, mode(p));
    s.notify(format!("{n} features selected"), false);
    Ok(json!({"selected": n}))
}

fn by_rect(s: &mut Session, p: &Value) -> CmdResult {
    let (a, b, c, d) = (f(p, "minLon"), f(p, "minLat"), f(p, "maxLon"), f(p, "maxLat"));
    let (Some(a), Some(b), Some(c), Some(d)) = (a, b, c, d) else { return Err("minLon, minLat, maxLon and maxLat are required".into()) };
    let ll = Extent::from_points(Coord::new(a, b), Coord::new(c, d));
    let e = s.map().flat().extent(&ll);
    let scale = s.scale();
    let m = mode(p);
    let mut hits = s.map_mut().hits_in(&e, scale);
    // In curved projections the map-space box is larger than the lon/lat one: keep only features
    // that reach into the requested rectangle.
    if !matches!(s.map().flat(), rustgis_map::projection::Flat::Mercator) {
        for (id, rows) in &mut hits {
            if let Some(l) = s.map().layer(*id) {
                rows.retain(|r| l.dataset.features.get(*r).and_then(|f| f.geometry.as_ref()).is_some_and(|g| g.extent().intersects(&ll)));
            }
        }
        hits.retain(|(_, rows)| !rows.is_empty());
    }
    if m == Mode::New {
        s.map_mut().clear_selection();
    }
    let mut total = 0;
    for (id, rows) in hits {
        total += rows.len();
        set(s, id, rows, if m == Mode::New { Mode::Add } else { m });
    }
    Ok(json!({"selected": total}))
}

fn hit(s: &mut Session, p: &Value) -> Result<Option<(u32, usize)>, String> {
    let lon = f(p, "lon").ok_or("lon is required")?;
    let lat = f(p, "lat").ok_or("lat is required")?;
    let tol = f(p, "tolerancePx").unwrap_or(4.0).clamp(0.0, 50.0) * s.map().camera.res;
    let scale = s.scale();
    let Some(p) = s.map().to_map(Coord::new(lon, lat)) else { return Ok(None) };
    // The feature drawn on top among the selectable layers: one in a layer that isn't selectable
    // doesn't hide those below it.
    Ok(s.map_mut().hit(p, tol, scale, |l| l.selectable))
}

fn at_point(s: &mut Session, p: &Value) -> CmdResult {
    let m = mode(p);
    match hit(s, p)? {
        Some((id, row)) => {
            set(s, id, vec![row], m);
            Ok(json!({"layer": id, "row": row}))
        }
        None => {
            if m == Mode::New {
                s.map_mut().clear_selection();
            }
            Ok(Value::Null)
        }
    }
}

/// The most features one identify reports, so a click on a dense spot stays quick.
const MAX_IDENTIFIED: usize = 100;

fn identify(s: &mut Session, p: &Value) -> CmdResult {
    let mode = match ps(p, "mode") {
        Some(m) => PopupMode::from_code(m).ok_or_else(|| format!("unknown pop-up mode '{m}'"))?,
        None => s.map().popup_mode,
    };
    if mode == PopupMode::None {
        return Ok(Value::Null);
    }
    let lon = f(p, "lon").ok_or("lon is required")?;
    let lat = f(p, "lat").ok_or("lat is required")?;
    let tol = f(p, "tolerancePx").unwrap_or(4.0).clamp(0.0, 50.0) * s.map().camera.res;
    let scale = s.scale();
    let active = s.active_layer;
    let keep = |l: &rustgis_map::Layer| match mode {
        PopupMode::Selectable => l.selectable,
        PopupMode::SelectedInContents => Some(l.id) == active,
        _ => true,
    };
    // Every feature under the click in the layers the mode takes; topmost stops at the first layer.
    let first_layer = mode == PopupMode::Topmost;
    let hits = match s.map().to_map(Coord::new(lon, lat)) {
        Some(pt) => s.map_mut().hits_at(pt, tol, scale, keep, first_layer, MAX_IDENTIFIED),
        None => Vec::new(),
    };
    let map = s.map();
    let results: Vec<Value> = hits.iter().filter_map(|(id, row)| feature_json(map, *id, *row)).collect();
    let Some(first) = results.first().cloned() else {
        // No feature: report the pixel of the topmost raster instead.
        if mode == PopupMode::SelectedInContents && active.and_then(|id| map.layer(id)).is_some_and(|l| !l.is_raster()) {
            return Ok(Value::Null);
        }
        return Ok(super::raster::identify(s, lon, lat).unwrap_or(Value::Null));
    };
    let mut out = first;
    if let Value::Object(o) = &mut out {
        o.insert("results".into(), Value::Array(results));
    }
    Ok(out)
}

fn popup_mode(s: &mut Session, p: &Value) -> CmdResult {
    let m = ps(p, "mode").ok_or("mode is required")?;
    let mode = PopupMode::from_code(m).ok_or_else(|| format!("unknown pop-up mode '{m}'"))?;
    s.map_mut().popup_mode = mode;
    Ok(json!({"mode": mode.code()}))
}

fn feature_json(map: &rustgis_map::Map, id: u32, row: usize) -> Option<Value> {
    let l = map.layer(id)?;
    let mut attrs = serde_json::Map::new();
    for (i, fd) in l.dataset.fields.iter().enumerate() {
        attrs.insert(fd.name.clone(), json!(l.dataset.value(row, i).to_string()));
    }
    // The feature id finds the feature again after a streamed layer reads another window.
    let fid = l.dataset.features.get(row).map(|f| f.id);
    Some(json!({"layer": l.name, "layerId": id, "row": row, "fid": fid, "attributes": attrs}))
}
