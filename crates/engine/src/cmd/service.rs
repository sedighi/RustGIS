//! Web map and feature services: connect to a WMS, WMTS, WFS, WCS, OGC API Features or ArcGIS
//! REST service, list what it offers (from its capabilities), and add a layer — map layers as
//! tile layers, feature types and collections as data downloaded as GeoJSON, coverages as rasters
//! (a GeoTIFF of the view from GetCoverage). And search a catalogue (CSW) for records and the
//! services and files they link to.
//!
//! The app fetches the capabilities and features in the background and passes the capabilities
//! here as `"text"`; the command line, MCP and the desktop assistant let these commands fetch
//! (waiting for the answer).

use rustgis_data::ogc::{Coverage, Offer, Service};
use serde_json::{Value, json};

use super::ps;
use crate::{CmdResult, CommandSpec as C, Session};

pub fn commands() -> Vec<C> {
    vec![
        C::new("service.layers", "Service Layers", "Home › Add", layers).view().params(
            r#"{"url": "a WMS, WMTS or WFS address (GetCapabilities is added), an OGC API /collections address (features, and map and vector tiles) or landing page (its map tiles), or an ArcGIS FeatureServer, MapServer, ImageServer or VectorTileServer", "type"?: "wms"|"wmts"|"wfs"|"ogcapi"|"arcgis" (guessed from the address), "text"?: "the capabilities document, when already fetched"}"#,
        ),
        C::new("service.add", "Add Service Layer", "Home › Add", add).params(
            r#"{"url": "as service.layers", "type"?, "layer": "the layer's name (service.layers lists them)", "name"?: text, "text"?: "the capabilities document, when already fetched", "describe"?: "a WCS coverage's DescribeCoverage document, when already fetched", "tiles"?: "an OGC API layer's tile template, when already read from its tileset (the app's)", "bbox"?: [west, south, east, north] | "view" (the default) | "full" (a WCS coverage's area)}"#,
        ),
        C::new("service.coverage", "WCS Coverage Request", "Home › Add", coverage).view().params(
            r#"{"url": "a WCS address", "type"?: "wcs", "layer": "the coverage's name", "text"?: "the capabilities document, when already fetched", "describe"?: "its DescribeCoverage answer, when already fetched", "bbox"?: [west, south, east, north] | "view" (the default) | "full"} → the GetCoverage address of a GeoTIFF (layer.addUrl adds it)"#,
        ),
        C::new("catalog.search", "Search Catalogue", "Home › Add", catalog_search).view().params(
            r#"{"url": "a catalogue's CSW address", "query"?: "words anywhere in the records", "bbox"?: [west, south, east, north] | "view" (none by default), "start"?: "the first record's position (a page's next)", "text"?: "the GetRecords answer, when already fetched"} → records with their service and download links (service.add or layer.addUrl add them)"#,
        ),
    ]
}

fn url_of(p: &Value) -> Result<&str, String> {
    let url = ps(p, "url").map(str::trim).filter(|u| !u.is_empty()).ok_or("url is required")?;
    let lower = url.to_ascii_lowercase();
    if !(lower.starts_with("https://") || lower.starts_with("http://")) {
        return Err(format!("{url}: a service is given by its http:// or https:// address"));
    }
    Ok(url)
}

/// The service at `p`'s address: from its capabilities in `"text"`, or fetched (waiting for them).
fn capabilities(p: &Value) -> Result<Service, String> {
    let url = url_of(p)?;
    let caps = rustgis_data::ogc::capabilities_url(url, ps(p, "type"));
    let svc = match ps(p, "text") {
        Some(text) => rustgis_data::ogc::parse(&caps, text.as_bytes()).map_err(|e| format!("{url}: {e}"))?,
        None => fetch_capabilities(url, &caps)?,
    };
    Ok(vet(svc, &caps))
}

#[cfg(target_arch = "wasm32")]
fn fetch_capabilities(url: &str, _caps: &str) -> Result<Service, String> {
    Err(format!("{url}: in the web app, use Home › Add Data › From Service"))
}

#[cfg(not(target_arch = "wasm32"))]
fn fetch_capabilities(url: &str, caps: &str) -> Result<Service, String> {
    let bytes = crate::render::rustgis_tiles::cache::download_blocking(caps).map_err(|e| format!("{url}: {e}"))?;
    rustgis_data::ogc::parse(caps, &bytes).map_err(|e| format!("{url}: {e}"))
}

/// Leaves out the layers whose addresses a document on the web may not send RustGIS to: the
/// capabilities are hostile input, and their endpoints and links must not reach into the local
/// network (unless the service itself is there) or a forbidden address.
fn vet(mut svc: Service, caps: &str) -> Service {
    let before = svc.layers.len();
    svc.layers.retain(|l| {
        let u = match &l.offer {
            Offer::Tiles { template } | Offer::VectorTiles { template, .. } => template,
            Offer::Features { url } | Offer::Style { url } => url,
            Offer::Coverage(Coverage { endpoint, .. }) => endpoint,
        };
        crate::render::rustgis_tiles::address::check(u, Some(caps)).is_ok()
    });
    let dropped = before - svc.layers.len();
    if dropped > 0 {
        svc.notes.push(format!("{dropped} layers point to addresses RustGIS won't fetch for a service on the web and are left out"));
    }
    svc
}

/// A service's GeoJSON answer made ready to read: latitude-first coordinates turned round, and a
/// cut-off answer pointed out.
pub fn features_answer(s: &mut Session, name: &str, bytes: Vec<u8>) -> Vec<u8> {
    if let Some(note) = rustgis_data::ogc::partial_note(&bytes) {
        s.notify(format!("{name}: {note}"), false);
    }
    rustgis_data::ogc::fix_axes(&bytes).unwrap_or(bytes)
}

/// A tile template given for a service's layer: a web address with `{z}`, `{x}` and `{y}`, that
/// the service's document may point to.
fn checked_template(service: &str, t: &str) -> Result<String, String> {
    let t = t.trim();
    if !(t.contains("{z}") && t.contains("{x}") && t.contains("{y}")) || t.len() > 2000 {
        return Err(format!("{t}: not a tile template"));
    }
    crate::render::rustgis_tiles::address::check(t, Some(service))?;
    Ok(t.to_string())
}

/// The tile template an OGC API tileset list advertises (its WebMercatorQuad tileset's), read now
/// (the list, then the tileset when the list doesn't carry it). `None` when it can't be read.
#[cfg(not(target_arch = "wasm32"))]
fn advertised_template(list: &str, vector: bool) -> Option<String> {
    use rustgis_data::ogc::{TilesStep, resolve_tiles};
    let mut doc = list.to_string();
    for _ in 0..2 {
        crate::render::rustgis_tiles::address::check(&doc, Some(list)).ok()?;
        let body = crate::render::rustgis_tiles::cache::download_blocking(&doc).ok()?;
        match resolve_tiles(list, &doc, &body, vector)? {
            TilesStep::Template(t) => return checked_template(list, &t).ok(),
            TilesStep::Next(next) => doc = next,
        }
    }
    None
}

/// In the browser nothing waits: the app reads the template itself and passes it as `"tiles"`.
#[cfg(target_arch = "wasm32")]
fn advertised_template(_: &str, _: bool) -> Option<String> {
    None
}

/// A service as the app and other callers see it.
pub fn service_json(svc: &Service) -> Value {
    let layers: Vec<Value> = svc
        .layers
        .iter()
        .map(|l| {
            let offer = match &l.offer {
                Offer::Tiles { template } => json!({"type": "tiles", "url": template}),
                Offer::Features { url } => json!({"type": "features", "url": url}),
                Offer::Style { url } => json!({"type": "style", "url": url}),
                Offer::VectorTiles { template, layers } => json!({"type": "vectorTiles", "url": template, "layers": layers}),
                Offer::Coverage(c) => json!({"type": "coverage", "url": c.endpoint, "describe": rustgis_data::ogc::describe_coverage_url(c), "version": c.version}),
            };
            json!({"name": l.name, "title": l.title, "bbox": l.bbox, "offer": offer, "tilesets": l.tilesets})
        })
        .collect();
    json!({"kind": svc.kind, "title": svc.title, "count": layers.len(), "layers": layers, "notes": svc.notes})
}

pub(crate) fn layers(_s: &mut Session, p: &Value) -> CmdResult {
    Ok(service_json(&capabilities(p)?))
}

/// Adds one of a service's layers: map layers as tiles (an overlay), a style as a style, features
/// downloaded.
pub(crate) fn add(s: &mut Session, p: &Value) -> CmdResult {
    let url = url_of(p)?.to_string();
    let want = ps(p, "layer").map(str::trim).filter(|l| !l.is_empty()).ok_or("layer is required")?;
    let svc = capabilities(p)?;
    let layer = svc
        .layers
        .iter()
        .find(|l| l.name == want)
        .or_else(|| svc.layers.iter().find(|l| l.name.eq_ignore_ascii_case(want) || l.title.eq_ignore_ascii_case(want)))
        .ok_or_else(|| format!("{url}: no layer '{want}' (service.layers lists them)"))?;
    let name = ps(p, "name").map(str::trim).filter(|n| !n.is_empty()).unwrap_or(&layer.title).to_string();
    let zoom = |s: &mut Session| {
        if let Some([w, so, e, n]) = layer.bbox {
            s.zoom_to(&rustgis_geom::Extent::new(w, so, e, n));
        }
    };
    // An OGC API layer's tiles: the template its tileset advertises (given, or read now), else
    // the usual path.
    let advertised = match (&layer.tilesets, ps(p, "tiles")) {
        (Some(_), Some(t)) => Some(checked_template(&url, t)?),
        (Some(list), None) => advertised_template(list, matches!(layer.offer, Offer::VectorTiles { .. })),
        _ => None,
    };
    match &layer.offer {
        Offer::Tiles { template } => {
            let template = advertised.as_deref().unwrap_or(template);
            let r = super::tiles::add(s, &json!({"url": template, "kind": "raster", "name": name, "overlay": true}))?;
            zoom(s);
            Ok(r)
        }
        Offer::VectorTiles { template, layers } => {
            let template = advertised.as_deref().unwrap_or(template);
            let r = super::tiles::add(s, &json!({"url": template, "kind": "vector", "name": name, "layers": layers, "overlay": true}))?;
            zoom(s);
            Ok(r)
        }
        Offer::Style { url: style } => {
            let r = super::tiles::add(s, &json!({"url": style, "kind": "style", "name": name}))?;
            zoom(s);
            Ok(r)
        }
        Offer::Coverage(c) => {
            let (get, _) = coverage_request(s, p, c)?;
            #[cfg(target_arch = "wasm32")]
            {
                let _ = get;
                Err(format!("{url}: in the web app, Home › Add Data › From Service downloads the coverage"))
            }
            #[cfg(not(target_arch = "wasm32"))]
            {
                let bytes = crate::render::rustgis_tiles::cache::download_blocking(&get).map_err(|e| format!("{name}: {e}"))?;
                coverage_check(&name, &bytes)?;
                super::raster::add_named_bytes(s, &json!({"name": name}), format!("{name}.tif"), bytes)
            }
        }
        Offer::Features { url: items } => {
            #[cfg(target_arch = "wasm32")]
            {
                let _ = items;
                Err(format!("{url}: in the web app, Home › Add Data › From Service downloads the features"))
            }
            #[cfg(not(target_arch = "wasm32"))]
            {
                // `download_blocking` is bounded: MAX_DOWNLOAD bytes and BLOCKING_TIMEOUT.
                let bytes = crate::render::rustgis_tiles::cache::download_blocking(items).map_err(|e| format!("{items}: {e}"))?;
                let bytes = features_answer(s, &name, bytes);
                let r = super::raster::add_named_bytes(s, &json!({"name": name}), format!("{name}.geojson"), bytes)?;
                zoom(s);
                Ok(r)
            }
        }
    }
}

/// The area a coverage is asked for over: `bbox` given, `"full"` (its bounds), or the map's view
/// (the default); within its bounds when they are known.
fn coverage_area(s: &Session, p: &Value, info: &rustgis_data::ogc::CoverageInfo, id: &str) -> Result<[f64; 4], String> {
    let area = match p.get("bbox") {
        Some(Value::String(f)) if f == "full" => {
            info.bbox.ok_or_else(|| format!("{id}: its bounds aren't known; give bbox or use the view"))?
        }
        _ => super::catalogs::area(s, p)?,
    };
    if area[0] > area[2] {
        return Err(format!("{id}: an area across the antimeridian; give it one side at a time"));
    }
    rustgis_data::ogc::coverage_area(info, area).ok_or_else(|| format!("{id}: the coverage has no data in the area"))
}

/// A coverage's GetCoverage address and the area it covers: its description from `"describe"`, or
/// fetched (waiting for it).
fn coverage_request(s: &Session, p: &Value, c: &Coverage) -> Result<(String, [f64; 4]), String> {
    let describe = rustgis_data::ogc::describe_coverage_url(c);
    let info = match ps(p, "describe") {
        Some(text) => rustgis_data::ogc::parse_describe(c, text.as_bytes())?,
        None => fetch_describe(c, &describe)?,
    };
    let area = coverage_area(s, p, &info, &c.id)?;
    let get = rustgis_data::ogc::get_coverage_url(c, &info, area);
    crate::render::rustgis_tiles::address::check(&get, Some(&c.endpoint))?;
    Ok((get, area))
}

#[cfg(not(target_arch = "wasm32"))]
fn fetch_describe(c: &Coverage, describe: &str) -> Result<rustgis_data::ogc::CoverageInfo, String> {
    let bytes = crate::render::rustgis_tiles::cache::download_blocking(describe).map_err(|e| format!("{}: {e}", c.id))?;
    rustgis_data::ogc::parse_describe(c, &bytes)
}

#[cfg(target_arch = "wasm32")]
fn fetch_describe(c: &Coverage, _: &str) -> Result<rustgis_data::ogc::CoverageInfo, String> {
    Err(format!("{}: in the web app, the description is read first (give it as \"describe\")", c.id))
}

/// `service.coverage`: the GetCoverage address of a WCS coverage over an area, for the app (which
/// downloads it in the background) and for scripts.
fn coverage(s: &mut Session, p: &Value) -> CmdResult {
    let url = url_of(p)?.to_string();
    let want = ps(p, "layer").map(str::trim).filter(|l| !l.is_empty()).ok_or("layer is required")?;
    let svc = capabilities(p)?;
    let layer = svc
        .layers
        .iter()
        .find(|l| l.name == want || l.name.eq_ignore_ascii_case(want))
        .ok_or_else(|| format!("{url}: no coverage '{want}' (service.layers lists them)"))?;
    let Offer::Coverage(c) = &layer.offer else { return Err(format!("{url}: '{want}' isn't a WCS coverage")) };
    let (get, area) = coverage_request(s, p, c)?;
    Ok(json!({"url": get, "name": layer.title, "bbox": area, "describe": rustgis_data::ogc::describe_coverage_url(c)}))
}

/// The GetRecords request `catalog.search` makes of its parameters (the app fetches it first).
pub fn records_request(s: &Session, p: &Value) -> Result<String, String> {
    let url = url_of(p)?;
    let words = ps(p, "query").unwrap_or("");
    let bbox = match p.get("bbox") {
        None | Some(Value::Null) => None,
        // A box across the antimeridian isn't asked for: the whole catalogue is searched.
        Some(_) => Some(super::catalogs::area(s, p)?).filter(|a| a[0] <= a[2]),
    };
    let start = p.get("start").and_then(Value::as_u64).unwrap_or(1).clamp(1, 1_000_000);
    Ok(rustgis_data::ogc::records_url(url, words, bbox, start))
}

/// A WCS coverage's answer is a GeoTIFF; anything else (an XML exception, an HTML page, plain
/// text) is the service's error, its start reported.
pub(crate) fn coverage_check(name: &str, bytes: &[u8]) -> Result<(), String> {
    // TIFF's and BigTIFF's byte-order marks with their magic numbers.
    if [&b"II*\0"[..], b"MM\0*", b"II+\0", b"MM\0+"].iter().any(|m| bytes.starts_with(m)) {
        return Ok(());
    }
    let text = String::from_utf8_lossy(bytes.get(..bytes.len().min(400)).unwrap_or_default()).into_owned();
    Err(format!("{name}: the service answered with an error: {text}"))
}

/// `catalog.search`: a page of a catalogue's (CSW) records matching words and an area, with the
/// links RustGIS can follow (those pointing where a document on the web may not send it left out).
fn catalog_search(s: &mut Session, p: &Value) -> CmdResult {
    let url = url_of(p)?;
    let request = records_request(s, p)?;
    let page = match ps(p, "text") {
        Some(text) => rustgis_data::ogc::parse_records(&request, text.as_bytes()).map_err(|e| format!("{url}: {e}"))?,
        None => fetch_records(url, &request)?,
    };
    let mut dropped = 0usize;
    let records: Vec<Value> = page
        .records
        .iter()
        .map(|r| {
            let links: Vec<Value> = r
                .links
                .iter()
                .filter(|l| {
                    let ok = crate::render::rustgis_tiles::address::check(&l.url, Some(&request)).is_ok();
                    dropped += usize::from(!ok);
                    ok
                })
                .map(|l| json!({"url": l.url, "kind": l.kind, "name": l.name, "protocol": l.protocol}))
                .collect();
            json!({"id": r.id, "title": r.title, "abstract": r.summary, "bbox": r.bbox, "links": links})
        })
        .collect();
    let mut out = json!({"matched": page.matched, "count": records.len(), "next": page.next, "records": records, "request": request});
    if dropped > 0 {
        out["notes"] =
            json!([format!("{dropped} links point to addresses RustGIS won't fetch for a catalogue on the web and are left out")]);
    }
    Ok(out)
}

#[cfg(not(target_arch = "wasm32"))]
fn fetch_records(url: &str, request: &str) -> Result<rustgis_data::ogc::RecordPage, String> {
    let bytes = crate::render::rustgis_tiles::cache::download_blocking(request).map_err(|e| format!("{url}: {e}"))?;
    rustgis_data::ogc::parse_records(request, &bytes).map_err(|e| format!("{url}: {e}"))
}

#[cfg(target_arch = "wasm32")]
fn fetch_records(url: &str, _: &str) -> Result<rustgis_data::ogc::RecordPage, String> {
    Err(format!("{url}: in the web app, use Home › Add Data › From Service › Search Catalogue"))
}
