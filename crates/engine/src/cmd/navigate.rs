//! Home › Navigate, Add (basemap) and Data › Project (new map), bookmarks.

use rustgis_geom::{Coord, Extent};
use rustgis_map::{Basemap, Bookmark, Map, Projection};
use serde_json::{Value, json};

use super::{f, ps};
use crate::{CmdResult, CommandSpec as C, Session};

pub fn commands() -> Vec<C> {
    vec![
        C::new("map.zoomFull", "Full Extent", "Home › Navigate", zoom_full).view(),
        C::new("map.zoomIn", "Fixed Zoom In", "Home › Navigate", |s, _| zoom(s, 2.0)).view().key("Mod+="),
        C::new("map.zoomOut", "Fixed Zoom Out", "Home › Navigate", |s, _| zoom(s, 0.5)).view().key("Mod+-"),
        C::new("map.prevExtent", "Previous Extent", "Home › Navigate", |s, _| {
            if s.extent_back() { Ok(Value::Null) } else { Err("no previous extent".into()) }
        })
        .view()
        .when(|s| if s.can_extent_back() { None } else { Some("no previous extent") }),
        C::new("map.nextExtent", "Next Extent", "Home › Navigate", |s, _| {
            if s.extent_forward() { Ok(Value::Null) } else { Err("no next extent".into()) }
        })
        .view()
        .when(|s| if s.can_extent_forward() { None } else { Some("no next extent") }),
        C::new("map.zoomWorld", "Zoom To World", "Home › Navigate", |s, _| {
            s.zoom_to(&Extent::new(-170.0, -58.0, 180.0, 78.0));
            Ok(Value::Null)
        })
        .view(),
        C::new("map.extent", "View Extent", "Home › Navigate", |s, _| {
            let e = if s.map().projection.is_globe() { Extent::new(-180.0, -90.0, 180.0, 90.0) } else { super::workspace::view_extent(s) };
            if e.is_empty() {
                return Err("the view shows no part of the Earth".into());
            }
            Ok(json!({"extent": [e.min_x, e.min_y, e.max_x, e.max_y]}))
        })
        .view(),
        C::new("map.zoomToExtent", "Zoom To Extent", "Home › Navigate", zoom_to_extent)
            .view()
            .params(r#"{"extent": [minLon, minLat, maxLon, maxLat]}"#),
        C::new("map.goToXY", "Go To XY", "Home › Navigate", go_to_xy)
            .view()
            .params(r#"{"lon" | "x": number, "lat" | "y": number, "scale"?: number}"#),
        C::new("map.setScale", "Set Scale", "Status Bar", |s, p| {
            let sc = f(p, "scale").ok_or("scale is required")?;
            s.map_mut().set_scale(sc);
            s.record_extent();
            Ok(Value::Null)
        })
        .view()
        .params(r#"{"scale": number}"#),
        C::new("map.basemap", "Basemap", "Home › Add", basemap)
            .params(r#"{"name": "paper"|"midnight"|"ocean"|"atlas"|"blank", "keepTiles"?: bool (keeps the web tile basemaps over it)}"#),
        C::new("map.graticule", "Graticule", "View › Display", |s, p| {
            let m = s.map_mut();
            m.graticule = super::b(p, "on").unwrap_or(!m.graticule);
            Ok(Value::Null)
        })
        .params(r#"{"on"?: bool (toggles when omitted)}"#),
        C::new("map.projection", "Globe", "View › View", |s, p| {
            let m = s.map_mut();
            let next = match ps(p, "type") {
                Some(t) => Projection::from_code(t)?,
                None if m.projection.is_globe() => Projection::WebMercator,
                None => Projection::Globe,
            };
            m.set_projection(next);
            let out = json!({"projection": m.projection.code(), "label": m.projection.label()});
            if s.auto_cache {
                super::workspace::fill_cache(s);
            }
            Ok(out)
        })
        .params(r#"{"type"?: "mercator"|"globe"|preset id|"EPSG:<code>" (toggles the globe when omitted)}"#),
        C::new("map.projections", "Projections", "View › View", |_, _| {
            let mut v = vec![json!({"id": "mercator", "label": "Web Mercator"}), json!({"id": "globe", "label": "Globe"})];
            v.extend(rustgis_map::projection::PRESETS.iter().map(|(id, label, _)| json!({"id": id, "label": label})));
            Ok(Value::Array(v))
        })
        .view(),
        C::new("bookmark.add", "New Bookmark", "Home › Navigate › Bookmarks", add_bookmark).params(r#"{"name"?: string}"#),
        C::new("bookmark.go", "Go To Bookmark", "Home › Navigate › Bookmarks", go_bookmark).view().params(r#"{"name": string}"#),
        C::new("bookmark.remove", "Remove Bookmark", "Home › Navigate › Bookmarks", |s, p| {
            let n = s_name(p)?;
            let m = s.map_mut();
            let before = m.bookmarks.len();
            m.bookmarks.retain(|b| !b.name.eq_ignore_ascii_case(n));
            if m.bookmarks.len() == before { Err(format!("no bookmark '{n}'")) } else { Ok(Value::Null) }
        })
        .params(r#"{"name": string}"#),
        C::new("map.new", "New Map", "Data › Project", new_map).params(r#"{"name"?: string}"#),
        C::new("map.activate", "Activate Map", "Browser", |s, p| {
            let n = s_name(p)?;
            let i = s.project.maps.iter().position(|m| m.name.eq_ignore_ascii_case(n)).ok_or_else(|| format!("no map '{n}'"))?;
            s.project.active_map = i;
            s.active_layer = s.map().layers.first().map(|l| l.id);
            s.table_layer = None;
            Ok(Value::Null)
        })
        .params(r#"{"name": string}"#)
        .view(),
        C::new("map.rename", "Rename Map", "Layers", |s, p| {
            let n = s_name(p)?.trim();
            if n.is_empty() {
                return Err("name is required".into());
            }
            s.map_mut().name = n.chars().take(200).collect();
            Ok(Value::Null)
        })
        .params(r#"{"name": string (the active map's new name)}"#),
        C::new("map.remove", "Delete Map", "Browser", |s, p| {
            if s.project.maps.len() < 2 {
                return Err("a project keeps at least one map".into());
            }
            let n = s_name(p)?;
            let i = s.project.maps.iter().position(|m| m.name.eq_ignore_ascii_case(n)).ok_or_else(|| format!("no map '{n}'"))?;
            s.project.maps.remove(i);
            s.project.active_map = 0;
            s.active_layer = s.map().layers.first().map(|l| l.id);
            Ok(Value::Null)
        })
        .params(r#"{"name": string}"#),
    ]
}

fn s_name(p: &Value) -> Result<&str, String> {
    ps(p, "name").ok_or_else(|| "name is required".to_string())
}

fn zoom_full(s: &mut Session, _: &Value) -> CmdResult {
    let e = s.map().full_extent();
    let e = if e.is_empty() { Extent::new(-170.0, -58.0, 180.0, 78.0) } else { e };
    s.zoom_to(&e);
    Ok(Value::Null)
}

fn zoom_to_extent(s: &mut Session, p: &Value) -> CmdResult {
    let e = lonlat_extent(p.get("extent")).ok_or("extent is [minLon, minLat, maxLon, maxLat], west before east and south before north")?;
    s.zoom_to(&e);
    Ok(Value::Null)
}

/// A longitude/latitude extent `[minLon, minLat, maxLon, maxLat]` within ±180/±90, not empty.
pub fn lonlat_extent(v: Option<&Value>) -> Option<Extent> {
    let n: Vec<f64> = v?.as_array()?.iter().map(Value::as_f64).collect::<Option<_>>()?;
    let [x0, y0, x1, y1] = n.as_slice() else { return None };
    let ok = |x: f64, lim: f64| x.is_finite() && (-lim..=lim).contains(&x);
    (ok(*x0, 180.0) && ok(*x1, 180.0) && ok(*y0, 90.0) && ok(*y1, 90.0) && x0 < x1 && y0 < y1).then(|| Extent::new(*x0, *y0, *x1, *y1))
}

fn zoom(s: &mut Session, factor: f64) -> CmdResult {
    let (w, h) = s.view_size;
    s.map_mut().camera.zoom_at(factor, w / 2.0, h / 2.0, w, h);
    s.record_extent();
    Ok(Value::Null)
}

pub(crate) fn go_to_xy(s: &mut Session, p: &Value) -> CmdResult {
    let lon = f(p, "lon").or_else(|| f(p, "x")).ok_or("lon is required")?;
    let lat = f(p, "lat").or_else(|| f(p, "y")).ok_or("lat is required")?;
    if !(-180.0..=180.0).contains(&lon) || !(-90.0..=90.0).contains(&lat) {
        return Err("longitude must be within ±180 and latitude within ±90".into());
    }
    let m = s.map_mut();
    m.camera.center = m.to_map(Coord::new(lon, lat)).ok_or("that point is outside the map's projection")?;
    if let Some(sc) = f(p, "scale") {
        m.set_scale(sc);
    }
    m.camera.clamp();
    s.record_extent();
    Ok(Value::Null)
}

fn basemap(s: &mut Session, p: &Value) -> CmdResult {
    let n = s_name(p)?;
    let m = s.map_mut();
    m.basemap = Basemap::from_code(n).ok_or_else(|| format!("unknown basemap '{n}'"))?;
    // A built-in basemap takes the place of the web basemaps (tile overlays stay).
    if !super::b(p, "keepTiles").unwrap_or(false) {
        m.tiles.retain(|t| t.overlay);
    }
    Ok(Value::Null)
}

fn add_bookmark(s: &mut Session, p: &Value) -> CmdResult {
    let m = s.map_mut();
    let name = match super::ps(p, "name").map(str::trim).filter(|n| !n.is_empty()) {
        Some(n) => n.chars().take(120).collect(),
        None => format!("Bookmark {}", m.bookmarks.len() + 1),
    };
    m.bookmarks.retain(|b| !b.name.eq_ignore_ascii_case(&name));
    m.bookmarks.push(Bookmark { name: name.clone(), camera: m.camera });
    Ok(json!({"name": name}))
}

fn go_bookmark(s: &mut Session, p: &Value) -> CmdResult {
    let n = s_name(p)?;
    let cam =
        s.map().bookmarks.iter().find(|b| b.name.eq_ignore_ascii_case(n)).map(|b| b.camera).ok_or_else(|| format!("no bookmark '{n}'"))?;
    s.map_mut().camera = cam;
    s.record_extent();
    Ok(Value::Null)
}

fn new_map(s: &mut Session, p: &Value) -> CmdResult {
    let base = super::ps(p, "name").map(str::trim).filter(|n| !n.is_empty()).unwrap_or("Map").to_string();
    let mut name = base.clone();
    let mut i = 1;
    while s.project.maps.iter().any(|m| m.name.eq_ignore_ascii_case(&name)) {
        i += 1;
        name = format!("{base}{i}");
    }
    let mut m = Map::new(name.clone());
    m.basemap = s.map().basemap;
    s.project.maps.push(m);
    s.project.active_map = s.project.maps.len() - 1;
    s.active_layer = None;
    s.table_layer = None;
    Ok(json!({"name": name}))
}
