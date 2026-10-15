//! STAC: an API's collections, a search for items (with their footprints as a layer), and adding
//! an item's asset by its web address.

use serde_json::{Value, json};

use super::{b, f, ps};
use crate::render::rustgis_tiles::stac::{self, Item, Search};
use crate::{CmdResult, CommandSpec as C, Session};

pub fn commands() -> Vec<C> {
    vec![
        C::new("stac.collections", "List STAC Collections", "Home › Add › STAC Search", collections)
            .view()
            .params(r#"{"url"?: "a STAC API (default: Earth Search, https://earth-search.aws.element84.com/v1; Planetary Computer: https://planetarycomputer.microsoft.com/api/stac/v1)"}"#),
        C::new("stac.search", "STAC Search", "Home › Add › STAC Search", search).params(
            r#"{"url"?: "a STAC API (default: Earth Search)", "collections"?: id | [ids], "bbox"?: [west, south, east, north] | "view" (the map's extent), "datetime"?: "2024-06-01" | "2024-06-01/2024-06-30", "limit"?: 1–200 (default 10), "maxCloud"?: percent, "footprints"?: bool (adds the items' footprints as a layer, default true)}"#,
        ),
        C::new("stac.addFootprints", "Add STAC Footprints", "Home › Add › STAC Search", add_footprints_cmd)
            .params(r#"{"footprints": GeoJSON FeatureCollection (items' footprints, as STAC Search finds them), "name"?: text}"#),
        C::new("stac.addAsset", "Add STAC Asset", "Home › Add › STAC Search", add_asset).params(
            r#"{"item": a STAC item (as stac.search returns it) | "url": an item's address (Planetary Computer assets are signed), "asset"?: key (default: visual, else the first GeoTIFF, else the first data asset), "name"?: text}"#,
        ),
    ]
}

/// The search a command describes (`view` is the map's current extent).
pub fn search_params(s: &Session, p: &Value) -> Result<Search, String> {
    let collections = match p.get("collections") {
        Some(Value::String(c)) => c.split(',').map(|c| c.trim().to_string()).filter(|c| !c.is_empty()).collect(),
        Some(Value::Array(a)) => a.iter().filter_map(Value::as_str).map(str::to_string).collect(),
        _ => Vec::new(),
    };
    let bbox = match p.get("bbox") {
        Some(Value::String(v)) if v == "view" => Some(view(s)?),
        Some(v @ Value::Array(_)) => {
            let n: Vec<f64> = v.as_array().into_iter().flatten().filter_map(Value::as_f64).collect();
            match n.as_slice() {
                [w, south, e, n] => Some([*w, *south, *e, *n]),
                _ => return Err("bbox is [west, south, east, north], or \"view\"".into()),
            }
        }
        _ if b(p, "view").unwrap_or(false) => Some(view(s)?),
        _ => None,
    };
    Ok(Search {
        collections,
        bbox,
        datetime: ps(p, "datetime").map(str::to_string),
        limit: f(p, "limit").map(|l| l.clamp(1.0, stac::MAX_ITEMS as f64) as usize).unwrap_or(10),
        max_cloud: f(p, "maxCloud"),
    })
}

/// The map's extent in longitude/latitude, within the globe.
fn view(s: &Session) -> Result<[f64; 4], String> {
    let e = super::workspace::view_extent(s);
    if e.is_empty() {
        return Err("the map's extent isn't known; give bbox".into());
    }
    Ok([e.min_x.max(-180.0), e.min_y.max(-90.0), e.max_x.min(180.0), e.max_y.min(90.0)])
}

/// Adds items' footprints as a polygon layer; its id.
pub fn add_footprints(s: &mut Session, items: &[Item], name: &str) -> Result<Option<u32>, String> {
    add_collection(s, &stac::footprints_geojson(items), name)
}

/// `stac.addFootprints`: the footprints the app found (a GeoJSON FeatureCollection) as a layer,
/// through the command so the change is undoable like any other.
fn add_footprints_cmd(s: &mut Session, p: &Value) -> CmdResult {
    let fc = p.get("footprints").ok_or("footprints is a GeoJSON FeatureCollection")?;
    let name = ps(p, "name").map(str::trim).filter(|n| !n.is_empty()).unwrap_or("STAC footprints");
    match add_collection(s, fc, name)? {
        Some(id) => Ok(serde_json::json!({"layer": id})),
        None => Err("no footprints to add".into()),
    }
}

fn add_collection(s: &mut Session, fc: &Value, name: &str) -> Result<Option<u32>, String> {
    if fc["features"].as_array().is_none_or(Vec::is_empty) {
        return Ok(None);
    }
    let text = fc.to_string();
    if text.len() > 64 << 20 {
        return Err("the footprints are too large".into());
    }
    let (ds, _) = rustgis_data::io::read_geojson(&text, name).map_err(|e| e.to_string())?;
    let id = super::layer::add_dataset(s, ds);
    // Outlines over a faint fill, so the map (and the scenes added later) show through.
    if let Some(l) = s.map_mut().layer_mut(id)
        && let rustgis_map::Renderer::Single(sym) = &mut l.renderer
    {
        let [r, g, b, _] = sym.fill.0;
        sym.fill = rustgis_map::Rgba::new(r, g, b, 25);
        sym.stroke = rustgis_map::Rgba::new(r, g, b, 255);
        sym.stroke_width = 1.5;
    }
    Ok(Some(id))
}

fn collections(_s: &mut Session, p: &Value) -> CmdResult {
    let api = stac::api(ps(p, "url"))?;
    #[cfg(target_arch = "wasm32")]
    {
        let _ = api;
        Err("in the web app, browse STAC with Home › Add Data › STAC Search".into())
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let bytes = crate::render::rustgis_tiles::cache::download_blocking(&stac::collections_url(&api))?;
        let c = stac::parse_collections(&bytes)?;
        Ok(json!({"count": c.len(), "collections": c.iter().map(|c| json!({"id": c.id, "title": c.title})).collect::<Vec<_>>()}))
    }
}

fn search(s: &mut Session, p: &Value) -> CmdResult {
    let api = stac::api(ps(p, "url"))?;
    let url = stac::search_url(&api, &search_params(s, p)?)?;
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (s, url);
        Err("in the web app, search STAC with Home › Add Data › STAC Search".into())
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let bytes = crate::render::rustgis_tiles::cache::download_blocking(&url)?;
        let (items, next) = stac::parse_items(&bytes, &url)?;
        let mut out = json!({"count": items.len(), "items": items.iter().map(stac::item_json).collect::<Vec<_>>(), "next": next});
        if b(p, "footprints").unwrap_or(true)
            && let Some(id) = add_footprints(s, &items, "STAC footprints")?
        {
            out["layer"] = json!(id);
            if let Some(e) = s.map().layer(id).map(|l| l.extent()) {
                s.zoom_to(&e);
            }
        }
        Ok(out)
    }
}

/// The item a command names: given whole, or by its address.
fn item(p: &Value) -> Result<Item, String> {
    if let Some(v) = p.get("item").filter(|v| v.is_object()) {
        // Given whole (by the assistant or a plugin, maybe from another answer): it came from no
        // address, so its assets may only be public web addresses.
        return stac::item_from_json(v, None).ok_or_else(|| "item isn't a STAC item".to_string());
    }
    let url = ps(p, "url").map(str::trim).filter(|u| !u.is_empty()).ok_or("give item (a STAC item) or url (an item's address)")?;
    #[cfg(target_arch = "wasm32")]
    {
        let _ = url;
        Err("in the web app, add STAC assets with Home › Add Data › STAC Search".into())
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let bytes = crate::render::rustgis_tiles::cache::download_blocking(url)?;
        let mut i = stac::parse_item(&bytes, url)?;
        i.url.get_or_insert_with(|| url.to_string());
        Ok(i)
    }
}

fn add_asset(s: &mut Session, p: &Value) -> CmdResult {
    let i = item(p)?;
    let (key, href) = i.asset_href(ps(p, "asset"))?;
    let name = ps(p, "name").map(str::trim).filter(|n| !n.is_empty()).map(str::to_string).unwrap_or_else(|| format!("{} {key}", i.id));
    // A Planetary Computer asset is read with its storage container's token.
    #[cfg(not(target_arch = "wasm32"))]
    let href = stac::sign_blocking(&href)?;
    super::layer::add_url(s, &json!({"url": href, "name": name}))
}
