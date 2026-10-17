//! OGC web services: what a service offers, from its capabilities document — WMS and WMTS map
//! layers (as tile templates in Web Mercator), WFS feature types, OGC API Features collections
//! (as GeoJSON addresses) and OGC API Tiles tilesets (map and vector tiles in WebMercatorQuad) —
//! and ArcGIS REST services (FeatureServer, MapServer, ImageServer,
//! VectorTileServer). The documents are hostile input: no DTD is read, nodes are capped, and
//! only layers RustGIS can draw are listed (the rest are counted in a note).

use roxmltree::{Node, ParsingOptions};

/// What a layer of a service becomes.
#[derive(Clone, Debug, PartialEq)]
pub enum Offer {
    /// Map tiles: an XYZ template (`{z}/{x}/{y}`, or `{bbox-epsg-3857}` for WMS).
    Tiles { template: String },
    /// Features: an address answering GeoJSON.
    Features { url: String },
    /// A MapLibre style document (an ArcGIS vector tile service's).
    Style { url: String },
    /// Vector tiles (`{z}/{x}/{y}`, Mapbox Vector Tiles) and the names of their layers, as an OGC
    /// API Tiles collection gives them.
    VectorTiles { template: String, layers: Vec<String> },
    /// A WCS coverage, added as a raster from GetCoverage (a GeoTIFF of the view, or of its
    /// bounds).
    Coverage(Coverage),
}

/// A layer a service offers.
#[derive(Clone, Debug, PartialEq)]
pub struct ServiceLayer {
    /// The name the service knows it by.
    pub name: String,
    pub title: String,
    pub offer: Offer,
    /// Its longitude/latitude bounds (west, south, east, north), when the service says.
    pub bbox: Option<[f64; 4]>,
    /// An OGC API tileset list (JSON) whose WebMercatorQuad tileset advertises the tile template:
    /// the offer's template is the standard path, used when the advertised one can't be read.
    pub tilesets: Option<String>,
}

/// A service's capabilities: its kind (`WMS`, `WMTS`, `WFS`, `OGC API Features`), title, the
/// layers RustGIS can add, and notes on what it left out.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Service {
    pub kind: String,
    pub title: String,
    pub layers: Vec<ServiceLayer>,
    pub notes: Vec<String>,
    /// An OGC API landing page's advertised JSON collections document, resolved against it.
    pub collections_url: Option<String>,
    /// Some advertised collections were left out (past the listing limit, or with an items
    /// address RustGIS doesn't fetch): the listing isn't the whole service.
    pub incomplete: bool,
}

/// Most layers listed from one service.
const MAX_LAYERS: usize = 2_000;
/// Features asked of a WFS in one request.
pub const FEATURE_LIMIT: usize = 50_000;
/// Features asked of an OGC API in one request: servers cap their pages (often at 10,000) and
/// some refuse a larger `limit` outright.
pub const OGC_API_LIMIT: usize = 10_000;

/// The address to fetch a service's capabilities from: OGC API landing pages and collection lists
/// are JSON; WMS, WMTS and WFS addresses get a GetCapabilities request unless they have one.
pub fn capabilities_url(url: &str, kind: Option<&str>) -> String {
    // ArcGIS REST: the service's JSON description.
    if kind.is_some_and(|k| k.eq_ignore_ascii_case("arcgis")) || (kind.is_none() && is_arcgis(url)) {
        return if query_value(url, "f").is_some_and(|f| f.eq_ignore_ascii_case("json") || f.eq_ignore_ascii_case("pjson")) {
            url.to_string()
        } else {
            with_query(url, "f=json")
        };
    }
    if query_value(url, "request").is_some_and(|r| r.eq_ignore_ascii_case("getcapabilities"))
        || base(url).to_ascii_lowercase().ends_with(".xml")
    {
        return url.to_string();
    }
    // The guess reads the path and query, never the host (`rapid.example.org` isn't an API).
    let rest = after_host(url).to_ascii_lowercase();
    let segments: Vec<&str> = base(&rest).split('/').filter(|s| !s.is_empty()).collect();
    let service = match kind.map(str::to_ascii_lowercase).as_deref() {
        Some("wms") => "WMS",
        Some("wmts") => "WMTS",
        Some("wfs") => "WFS",
        Some("wcs") => "WCS",
        Some("ogcapi") | Some("features") => return json_url(url),
        _ if rest.contains("wmts") => "WMTS",
        _ if rest.contains("wfs") => "WFS",
        _ if rest.contains("wcs") => "WCS",
        _ if rest.contains("wms") => "WMS",
        // An OGC API (`…/collections`, `…/ogc/features`, `…/api/…`): asked as JSON.
        _ if segments.iter().any(|s| matches!(*s, "collections" | "api" | "ogcapi" | "ogc")) => return json_url(url),
        // Anything else is taken as a WMS (the commonest service).
        _ => "WMS",
    };
    with_query(url, &format!("SERVICE={service}&REQUEST=GetCapabilities"))
}

/// Requests JSON once, retaining the address's other query parameters.
pub fn json_url(address: &str) -> String {
    let Ok(mut url) = url::Url::parse(address) else { return with_query(address, "f=json") };
    // Authentication and signed query values can depend on their original byte encoding.
    let mut format_seen = false;
    let mut pairs: Vec<&str> = url
        .query()
        .filter(|q| !q.is_empty())
        .into_iter()
        .flat_map(|q| q.split('&'))
        .filter_map(|pair| {
            if url::form_urlencoded::parse(pair.as_bytes()).next().is_some_and(|(key, _)| key.eq_ignore_ascii_case("f")) {
                let first = !format_seen;
                format_seen = true;
                first.then_some("f=json")
            } else {
                Some(pair)
            }
        })
        .collect();
    if !format_seen {
        pairs.push("f=json");
    }
    let query = pairs.join("&");
    url.set_fragment(None);
    url.set_query(Some(&query));
    url.into()
}

/// A collections link uses either the short or full OGC data relation.
fn is_data_link(link: &serde_json::Value) -> bool {
    matches!(link.get("rel").and_then(|rel| rel.as_str()), Some("data" | "http://www.opengis.net/def/rel/ogc/1.0/data"))
}

/// Chooses JSON before an untyped data link, without guessing an endpoint or copying credentials.
fn collections_link(address: &str, links: &[serde_json::Value]) -> Option<String> {
    let base = url::Url::parse(address).ok()?;
    links
        .iter()
        .filter(|l| is_data_link(l))
        .filter_map(|l| {
            let priority = match l.get("type").and_then(|v| v.as_str()) {
                Some(t) if t.split(';').next().is_some_and(|t| t.trim().eq_ignore_ascii_case("application/json")) => 0,
                None => 1,
                _ => return None,
            };
            let href = l.get("href")?.as_str()?.trim();
            if href.is_empty() || href.len() > 8192 {
                return None;
            }
            let mut next = base.join(href).ok()?;
            if !matches!(next.scheme(), "http" | "https") {
                return None;
            }
            next.set_fragment(None);
            // A typed JSON link already selects its representation; its query is authoritative.
            let next = if priority == 0 { next.into() } else { json_url(next.as_str()) };
            Some((priority, next))
        })
        .min_by_key(|(priority, _)| *priority)
        .map(|(_, url)| url)
}

/// Whether an address is an ArcGIS REST service (or its directory), not one of its OGC endpoints
/// (`…/MapServer/WMSServer`, `…/MapServer/WMTS`), which are read as WMS and WMTS.
fn is_arcgis(url: &str) -> bool {
    let b = base(url).trim_end_matches('/').to_ascii_lowercase();
    if ["/wmsserver", "/wfsserver", "/wmts"].iter().any(|k| b.contains(k)) {
        return false;
    }
    b.contains("/rest/services") || ["featureserver", "mapserver", "imageserver", "vectortileserver"].iter().any(|k| b.ends_with(k))
}

/// The part of an address after its host: the path, query and fragment.
fn after_host(url: &str) -> &str {
    let rest = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
    rest.find(['/', '?', '#']).and_then(|i| rest.get(i..)).unwrap_or("")
}

/// The scheme and host of an address (`https://x.org`).
fn origin(url: &str) -> &str {
    let host_at = url.find("://").map(|i| i + 3).unwrap_or(0);
    let end = url.get(host_at..).and_then(|r| r.find(['/', '?', '#'])).map(|i| host_at + i).unwrap_or(url.len());
    url.get(..end).unwrap_or(url)
}

/// The value of a query parameter, by its whole name (any case).
fn query_value<'a>(url: &'a str, key: &str) -> Option<&'a str> {
    let q = url.split('#').next()?.split_once('?')?.1;
    q.split('&').find_map(|kv| {
        let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
        k.eq_ignore_ascii_case(key).then_some(v)
    })
}

/// Whether an address is a web address (`http://` or `https://`), not another scheme that only
/// starts with those letters.
fn is_web(url: &str) -> bool {
    let l = url.get(..8).unwrap_or(url).to_ascii_lowercase();
    l.starts_with("http://") || l.starts_with("https://")
}

/// A link from a document at `from`, resolved: absolute, scheme-relative (`//host/…`),
/// host-relative (`/…`) or relative to the document's folder. `None` for other schemes.
fn resolve(from: &str, href: &str) -> Option<String> {
    let href = href.trim();
    if is_web(href) {
        return Some(href.to_string());
    }
    if href.contains("://") || href.starts_with("data:") || href.starts_with("javascript:") || href.is_empty() {
        return None;
    }
    if let Some(rest) = href.strip_prefix("//") {
        let scheme = from.split_once("://").map(|(s, _)| s).unwrap_or("https");
        return Some(format!("{scheme}://{rest}"));
    }
    if href.starts_with('/') {
        return Some(format!("{}{href}", origin(from)));
    }
    let b = base(from);
    let dir = if after_host(b).is_empty() {
        format!("{b}/")
    } else {
        b.get(..b.rfind('/').map(|i| i + 1).unwrap_or(b.len())).unwrap_or(b).to_string()
    };
    Some(format!("{dir}{href}"))
}

fn with_query(url: &str, q: &str) -> String {
    let (base, frag) = url.split_once('#').map(|(b, f)| (b, Some(f))).unwrap_or((url, None));
    let sep = if base.contains('?') { if base.ends_with('?') || base.ends_with('&') { "" } else { "&" } } else { "?" };
    match frag {
        Some(f) => format!("{base}{sep}{q}#{f}"),
        None => format!("{base}{sep}{q}"),
    }
}

/// A service's address for its requests when its document names none: the address without the
/// OGC request parameters, keeping the others (MapServer's `map=`, a token).
fn endpoint(url: &str) -> String {
    let b = base(url);
    let kept: Vec<&str> = url
        .split('#')
        .next()
        .and_then(|u| u.split_once('?'))
        .map(|(_, q)| q)
        .unwrap_or("")
        .split('&')
        .filter(|kv| {
            let k = kv.split_once('=').map(|(k, _)| k).unwrap_or(kv).to_ascii_lowercase();
            !kv.is_empty() && !matches!(k.as_str(), "service" | "request" | "version" | "acceptversions" | "sections" | "f")
        })
        .collect();
    if kept.is_empty() { b.to_string() } else { format!("{b}?{}", kept.join("&")) }
}

/// The part of an address before its query.
fn base(url: &str) -> &str {
    url.split(['?', '#']).next().unwrap_or(url)
}

/// Reads a capabilities document (XML or OGC API JSON) fetched from `url`.
pub fn parse(url: &str, body: &[u8]) -> Result<Service, String> {
    let text = std::str::from_utf8(body).map_err(|_| "the service's answer isn't UTF-8 text".to_string())?;
    let text = text.trim_start_matches('\u{feff}').trim_start();
    if text.starts_with('{') || text.starts_with('[') {
        let v: serde_json::Value = serde_json::from_str(text).map_err(|e| format!("the service's JSON: {e}"))?;
        if let Some(e) = v.get("error") {
            return Err(format!(
                "the service answered with an error: {}",
                e.get("message").and_then(|m| m.as_str()).unwrap_or("(no message)")
            ));
        }
        if is_arcgis(url) || v.get("currentVersion").is_some() {
            return arcgis(url, &v);
        }
        return ogc_api(url, &v);
    }
    let doc = xml(text, 5_000_000).map_err(|e| format!("the service's XML: {e}"))?;
    let root = doc.root_element();
    match local(&root) {
        "WMS_Capabilities" | "WMT_MS_Capabilities" => Ok(wms(url, &root)),
        "Capabilities" if root.descendants().any(|n| n.is_element() && local(&n) == "CoverageSummary") => Ok(wcs(url, &root)),
        "Capabilities" if child(&root, "Contents").is_some() => Ok(wmts(url, &root)),
        "WFS_Capabilities" => Ok(wfs(url, &root)),
        // WCS 1.0 (`WCS_Capabilities`), 1.1 and 2.0 (`Capabilities` with coverage summaries).
        "WCS_Capabilities" => Ok(wcs(url, &root)),
        "ServiceExceptionReport" | "ExceptionReport" => {
            Err(format!("the service answered with an error: {}", text_of(&root).chars().take(300).collect::<String>()))
        }
        other => Err(format!("not a WMS, WMTS, WFS or WCS capabilities document (<{other}>)")),
    }
}

/// Parses a service's XML document through [`crate::xml::parse`], which refuses deep nesting
/// (a stack-overflow hazard) and, with the DTD allowed here, entities holding markup.
fn xml(text: &str, nodes_limit: u32) -> Result<roxmltree::Document<'_>, String> {
    // WMS 1.1.1 documents start with a DOCTYPE, so DTDs are allowed; roxmltree still refuses
    // entity bombs and external entities, and the node count is capped.
    let opts = ParsingOptions { allow_dtd: true, nodes_limit, ..ParsingOptions::default() };
    crate::xml::parse(text.trim_start(), opts)
}

fn local<'a>(n: &Node<'a, '_>) -> &'a str {
    n.tag_name().name()
}

fn child<'a, 'i>(n: &Node<'a, 'i>, name: &str) -> Option<Node<'a, 'i>> {
    n.children().find(|c| c.is_element() && local(c) == name)
}

fn children<'a, 'i>(n: &Node<'a, 'i>, name: &'static str) -> impl Iterator<Item = Node<'a, 'i>> {
    n.children().filter(move |c| c.is_element() && local(c) == name)
}

fn text_of(n: &Node) -> String {
    n.descendants().filter(|c| c.is_text()).filter_map(|c| c.text()).collect::<String>().trim().to_string()
}

fn child_text(n: &Node, name: &str) -> Option<String> {
    child(n, name).map(|c| text_of(&c)).filter(|t| !t.is_empty())
}

fn href(n: &Node) -> Option<String> {
    n.attributes().find(|a| a.name() == "href").map(|a| a.value().to_string())
}

/// A finite longitude/latitude box.
fn lonlat_box(w: f64, s: f64, e: f64, n: f64) -> Option<[f64; 4]> {
    ([w, s, e, n].iter().all(|v| v.is_finite()) && w < e && s < n && w >= -180.5 && e <= 180.5 && s >= -90.5 && n <= 90.5)
        .then_some([w, s, e, n])
}

/// Codes a server uses for Web Mercator.
fn is_web_mercator(code: &str) -> bool {
    let c = code.to_ascii_uppercase();
    ["EPSG:3857", "EPSG:900913", "EPSG:102100", "EPSG:102113", "EPSG::3857", "OSGEO:41001"].iter().any(|k| c.contains(k))
        || c.ends_with(":3857")
}

// ---------------------------------------------------------------------------------------------
// WMS

fn wms(url: &str, root: &Node) -> Service {
    let v13 = root.attribute("version").is_none_or(|v| v.starts_with("1.3"));
    let title = child(root, "Service").and_then(|s| child_text(&s, "Title")).unwrap_or_default();
    let get_map = root
        .descendants()
        .find(|n| n.is_element() && local(n) == "GetMap")
        .and_then(|g| g.descendants().find(|n| n.is_element() && local(n) == "Get"))
        .and_then(|g| child(&g, "OnlineResource"))
        .and_then(|o| href(&o))
        .filter(|h| is_web(h))
        .unwrap_or_else(|| endpoint(url));
    let formats: Vec<String> = root
        .descendants()
        .find(|n| n.is_element() && local(n) == "GetMap")
        .map(|g| children(&g, "Format").map(|f| text_of(&f)).collect())
        .unwrap_or_default();
    let format = ["image/png", "image/png8", "image/jpeg"]
        .iter()
        .find(|f| formats.iter().any(|g| g.eq_ignore_ascii_case(f)))
        .copied()
        .unwrap_or("image/png");
    let mut service = Service { kind: "WMS".into(), title, ..Service::default() };
    let mut skipped = 0;
    // Layers inherit their parents' coordinate systems.
    let mut stack: Vec<(Node, Vec<String>)> = root
        .descendants()
        .filter(|n| n.is_element() && local(n) == "Capability")
        .flat_map(|c| children(&c, "Layer").collect::<Vec<_>>())
        .map(|l| (l, Vec::new()))
        .collect();
    while let Some((layer, inherited)) = stack.pop() {
        if service.layers.len() >= MAX_LAYERS {
            break;
        }
        let mut crs = inherited.clone();
        crs.extend(layer.children().filter(|c| c.is_element() && matches!(local(c), "CRS" | "SRS")).map(|c| text_of(&c)));
        for sub in children(&layer, "Layer") {
            stack.push((sub, crs.clone()));
        }
        let Some(name) = child_text(&layer, "Name") else { continue };
        if !crs.iter().any(|c| c.split_whitespace().any(is_web_mercator)) {
            skipped += 1;
            continue;
        }
        let bbox = child(&layer, "EX_GeographicBoundingBox")
            .and_then(|b| {
                let f = |k: &str| child_text(&b, k).and_then(|t| t.parse::<f64>().ok());
                lonlat_box(f("westBoundLongitude")?, f("southBoundLatitude")?, f("eastBoundLongitude")?, f("northBoundLatitude")?)
            })
            .or_else(|| {
                let b = child(&layer, "LatLonBoundingBox")?;
                let f = |k: &str| b.attribute(k).and_then(|t| t.parse::<f64>().ok());
                lonlat_box(f("minx")?, f("miny")?, f("maxx")?, f("maxy")?)
            });
        let crs_key = if v13 { "CRS" } else { "SRS" };
        let version = if v13 { "1.3.0" } else { "1.1.1" };
        let template = with_query(
            &get_map,
            &format!(
                "SERVICE=WMS&VERSION={version}&REQUEST=GetMap&LAYERS={}&STYLES=&{crs_key}=EPSG:3857&BBOX={{bbox-epsg-3857}}&WIDTH=256&HEIGHT=256&FORMAT={}&TRANSPARENT=TRUE",
                encode(&name),
                encode(format)
            ),
        );
        let title = child_text(&layer, "Title").unwrap_or_else(|| name.clone());
        service.layers.push(ServiceLayer { name, title, offer: Offer::Tiles { template }, bbox, tilesets: None });
    }
    service.layers.reverse();
    if skipped > 0 {
        service.notes.push(format!("{skipped} layers aren't offered in Web Mercator (EPSG:3857) and are left out"));
    }
    service
}

/// Percent-encodes a query value.
fn encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b':' | b',' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

// ---------------------------------------------------------------------------------------------
// WMTS

/// The scale denominator of Web Mercator zoom 0 for 256-pixel tiles (OGC's 0.28 mm pixels).
const Z0_SCALE: f64 = 559_082_264.028_717_8;

/// A tile matrix set's zoom levels, when it is Web Mercator's own pyramid: each matrix's
/// identifier and its zoom.
fn mercator_matrices(tms: &Node) -> Option<Vec<(String, u8)>> {
    let crs = child_text(tms, "SupportedCRS")?;
    if !is_web_mercator(&crs) {
        return None;
    }
    let mut out = Vec::new();
    for m in children(tms, "TileMatrix") {
        let id = child_text(&m, "Identifier")?;
        let sd = child_text(&m, "ScaleDenominator")?.parse::<f64>().ok()?;
        let w = child_text(&m, "TileWidth").and_then(|t| t.parse::<u32>().ok()).unwrap_or(256);
        // Zoom from the scale: a pyramid of 512 px tiles has half the scale denominator of a
        // 256 px one at the same zoom.
        let z = (Z0_SCALE / sd * 256.0 / f64::from(w)).log2();
        let zr = z.round();
        if (z - zr).abs() > 0.02 || !(0.0..=30.0).contains(&zr) {
            return None;
        }
        // The pyramid's corner must be Web Mercator's.
        let corner: Vec<f64> = child_text(&m, "TopLeftCorner")?.split_whitespace().filter_map(|t| t.parse().ok()).collect();
        let ok = |a: f64, b: f64| (a.abs() - b).abs() < 1.0;
        if corner.len() != 2 || !corner.iter().all(|c| ok(*c, 20_037_508.342_789_244)) {
            return None;
        }
        out.push((id, zr as u8));
    }
    (!out.is_empty()).then_some(out)
}

fn wmts(url: &str, root: &Node) -> Service {
    let title = child(root, "ServiceIdentification").and_then(|s| child_text(&s, "Title")).unwrap_or_default();
    let mut service = Service { kind: "WMTS".into(), title, ..Service::default() };
    let Some(contents) = child(root, "Contents") else { return service };
    // Tile matrix sets that are Web Mercator pyramids, with a zoom per matrix identifier.
    let sets: Vec<(String, Vec<(String, u8)>)> =
        children(&contents, "TileMatrixSet").filter_map(|t| Some((child_text(&t, "Identifier")?, mercator_matrices(&t)?))).collect();
    let kvp = root
        .descendants()
        .find(|n| n.is_element() && local(n) == "GetTile")
        .and_then(|g| g.descendants().find(|n| n.is_element() && local(n) == "Get"))
        .and_then(|g| href(&g))
        .filter(|h| is_web(h))
        .unwrap_or_else(|| endpoint(url));
    let mut skipped = 0;
    for layer in children(&contents, "Layer").take(MAX_LAYERS) {
        let Some(name) = child_text(&layer, "Identifier") else { continue };
        let title = child_text(&layer, "Title").unwrap_or_else(|| name.clone());
        let style = children(&layer, "Style")
            .find(|s| s.attribute("isDefault") == Some("true"))
            .or_else(|| child(&layer, "Style"))
            .and_then(|s| child_text(&s, "Identifier"))
            .unwrap_or_else(|| "default".into());
        let linked: Vec<String> = children(&layer, "TileMatrixSetLink").filter_map(|l| child_text(&l, "TileMatrixSet")).collect();
        let Some((set, matrices)) = sets.iter().find(|(id, _)| linked.contains(id)) else {
            skipped += 1;
            continue;
        };
        // A template needs the zoom in the matrix identifier: a fixed prefix then the zoom.
        let prefix = matrices.iter().find_map(|(id, z)| id.strip_suffix(&z.to_string()).map(str::to_string));
        let consistent = prefix.as_ref().is_some_and(|p| matrices.iter().all(|(id, z)| *id == format!("{p}{z}")));
        let Some(prefix) = prefix.filter(|_| consistent) else {
            skipped += 1;
            continue;
        };
        let matrix = format!("{prefix}{{z}}");
        let resource = children(&layer, "ResourceURL").find(|r| r.attribute("resourceType") == Some("tile")).and_then(|r| {
            let f = r.attribute("format").unwrap_or("");
            (!f.contains("pbf") && !f.contains("mvt")).then(|| r.attribute("template").filter(|t| is_web(t)).map(str::to_string)).flatten()
        });
        let template = match resource {
            Some(t) => t
                .replace("{TileMatrixSet}", set)
                .replace("{Style}", &style)
                .replace("{TileMatrix}", &matrix)
                .replace("{TileRow}", "{y}")
                .replace("{TileCol}", "{x}"),
            None => {
                let format =
                    children(&layer, "Format").map(|f| text_of(&f)).find(|f| f.starts_with("image/")).unwrap_or_else(|| "image/png".into());
                with_query(
                    &kvp,
                    &format!(
                        "SERVICE=WMTS&REQUEST=GetTile&VERSION=1.0.0&LAYER={}&STYLE={}&TILEMATRIXSET={}&TILEMATRIX={matrix}&TILEROW={{y}}&TILECOL={{x}}&FORMAT={}",
                        encode(&name),
                        encode(&style),
                        encode(set),
                        encode(&format)
                    ),
                )
            }
        };
        let bbox = child(&layer, "WGS84BoundingBox").and_then(|b| {
            let lo: Vec<f64> = child_text(&b, "LowerCorner")?.split_whitespace().filter_map(|t| t.parse().ok()).collect();
            let hi: Vec<f64> = child_text(&b, "UpperCorner")?.split_whitespace().filter_map(|t| t.parse().ok()).collect();
            lonlat_box(*lo.first()?, *lo.get(1)?, *hi.first()?, *hi.get(1)?)
        });
        service.layers.push(ServiceLayer { name, title, offer: Offer::Tiles { template }, bbox, tilesets: None });
    }
    if skipped > 0 {
        service.notes.push(format!("{skipped} layers have no Web Mercator tile matrix set RustGIS can draw and are left out"));
    }
    service
}

// ---------------------------------------------------------------------------------------------
// WFS

/// The output formats an `outputFormat` parameter lists among `scope`'s nodes.
fn output_formats<'a, 'i: 'a>(scope: impl Iterator<Item = Node<'a, 'i>>) -> Vec<String> {
    scope
        .filter(|n| {
            n.is_element() && local(n) == "Parameter" && n.attribute("name").is_some_and(|a| a.eq_ignore_ascii_case("outputFormat"))
        })
        .flat_map(|p| p.descendants().filter(|v| v.is_element() && local(v) == "Value").map(|v| text_of(&v)).collect::<Vec<_>>())
        .collect()
}

/// Whether a coordinate system code is OGC's CRS84 (longitude/latitude by definition).
fn is_crs84(code: &str) -> bool {
    let c = code.to_ascii_uppercase();
    c.contains("CRS84") || c.contains("CRS:84")
}

fn wfs(url: &str, root: &Node) -> Service {
    let title =
        child(root, "ServiceIdentification").or_else(|| child(root, "Service")).and_then(|s| child_text(&s, "Title")).unwrap_or_default();
    let mut service = Service { kind: "WFS".into(), title, ..Service::default() };
    let version = root.attribute("version").unwrap_or("1.1.0");
    // GetFeature's address: an `xlink:href` (WFS 1.1, 2.0) or an `onlineResource` attribute
    // (WFS 1.0); the capabilities address when the document gives no web address.
    let get = root
        .descendants()
        .find(|n| {
            n.is_element()
                && matches!(local(n), "Operation" | "GetFeature")
                && (local(n) == "GetFeature" || n.attribute("name") == Some("GetFeature"))
        })
        .and_then(|g| g.descendants().find(|n| n.is_element() && local(n) == "Get"))
        .and_then(|g| href(&g).or_else(|| g.attribute("onlineResource").map(str::to_string)))
        .filter(|h| is_web(h))
        .unwrap_or_else(|| endpoint(url));
    // The GeoJSON output format the server names (GetFeature's, else the service-wide list);
    // `application/json` when it lists none.
    let formats: Vec<String> = {
        let op = root.descendants().find(|n| n.is_element() && local(n) == "Operation" && n.attribute("name") == Some("GetFeature"));
        let in_op = output_formats(op.iter().flat_map(|o| o.descendants()));
        if in_op.is_empty() { output_formats(root.descendants()) } else { in_op }
    };
    let json = if formats.is_empty() {
        Some("application/json".to_string())
    } else {
        formats.iter().find(|f| f.to_ascii_lowercase().contains("json")).cloned()
    };
    let Some(json) = json else {
        service.notes.push("the service offers no GeoJSON output, so its feature types can't be added".into());
        return service;
    };
    let Some(list) = child(root, "FeatureTypeList") else { return service };
    for ft in children(&list, "FeatureType").take(MAX_LAYERS) {
        let Some(name) = child_text(&ft, "Name") else { continue };
        let title = child_text(&ft, "Title").unwrap_or_else(|| name.clone());
        let bbox = child(&ft, "WGS84BoundingBox")
            .and_then(|b| {
                let lo: Vec<f64> = child_text(&b, "LowerCorner")?.split_whitespace().filter_map(|t| t.parse().ok()).collect();
                let hi: Vec<f64> = child_text(&b, "UpperCorner")?.split_whitespace().filter_map(|t| t.parse().ok()).collect();
                lonlat_box(*lo.first()?, *lo.get(1)?, *hi.first()?, *hi.get(1)?)
            })
            .or_else(|| {
                let b = child(&ft, "LatLongBoundingBox")?;
                let f = |k: &str| b.attribute(k).and_then(|t| t.parse::<f64>().ok());
                lonlat_box(f("minx")?, f("miny")?, f("maxx")?, f("maxy")?)
            });
        // CRS84 when the type offers it: longitude first by definition. Otherwise EPSG:4326, which
        // WFS 1.0 answers longitude first; a 1.1 or 2.0 server answering latitude first says so
        // with a `urn:…EPSG::4326` crs in its GeoJSON, which is turned round (`fix_axes`).
        let crs84 = ft.children().any(|c| {
            c.is_element() && matches!(local(&c), "DefaultCRS" | "OtherCRS" | "DefaultSRS" | "OtherSRS" | "SRS") && is_crs84(&text_of(&c))
        });
        let srs = if crs84 { "urn:ogc:def:crs:OGC:1.3:CRS84" } else { "EPSG:4326" };
        // WFS 2.0 names the type and the count differently.
        let query = if version.starts_with('2') {
            format!(
                "SERVICE=WFS&VERSION=2.0.0&REQUEST=GetFeature&TYPENAMES={}&OUTPUTFORMAT={}&SRSNAME={srs}&COUNT={FEATURE_LIMIT}",
                encode(&name),
                encode(&json)
            )
        } else {
            let v = if version.starts_with("1.0") { "1.0.0" } else { "1.1.0" };
            format!(
                "SERVICE=WFS&VERSION={v}&REQUEST=GetFeature&TYPENAME={}&OUTPUTFORMAT={}&SRSNAME={srs}&MAXFEATURES={FEATURE_LIMIT}",
                encode(&name),
                encode(&json)
            )
        };
        let url = with_query(&get, &query);
        service.layers.push(ServiceLayer { name, title, offer: Offer::Features { url }, bbox, tilesets: None });
    }
    service
}

// ---------------------------------------------------------------------------------------------
// WCS

/// The largest side, in pixels, of a coverage asked for (GetCoverage scales it to fit).
pub const COVERAGE_MAX_PIXELS: u32 = 2048;

/// A WCS coverage: what DescribeCoverage and GetCoverage need.
#[derive(Clone, Debug, PartialEq)]
pub struct Coverage {
    /// The service's address for its requests (the capabilities' GetCoverage address).
    pub endpoint: String,
    /// The protocol asked in: `2.0.1` or `1.0.0` (a WCS 1.1 service is asked in 1.0, which the
    /// common servers answer too, rather than 1.1's multipart answers).
    pub version: String,
    pub id: String,
    /// The output formats the capabilities list (WCS 2.0's `formatSupported`).
    pub formats: Vec<String>,
}

/// What DescribeCoverage says a coverage is: the GeoTIFF format to ask for, the names of its
/// longitude-like and latitude-like axes (WCS 2.0's subsets), and its longitude/latitude bounds
/// when they can be known without reprojecting.
#[derive(Clone, Debug, PartialEq)]
pub struct CoverageInfo {
    pub format: String,
    pub axes: Option<(String, String)>,
    pub bbox: Option<[f64; 4]>,
}

fn wcs(url: &str, root: &Node) -> Service {
    let title =
        child(root, "ServiceIdentification").or_else(|| child(root, "Service")).and_then(|s| child_text(&s, "Title")).unwrap_or_default();
    let doc_version = root.attribute("version").unwrap_or("1.0.0");
    let v2 = doc_version.starts_with('2');
    let version = if v2 { "2.0.1" } else { "1.0.0" };
    let mut service = Service { kind: "WCS".into(), title, ..Service::default() };
    if doc_version.starts_with("1.1") {
        service.notes.push("a WCS 1.1 service: its coverages are asked for in WCS 1.0".into());
    }
    // GetCoverage's address: an `ows:Operation` (1.1, 2.0) or a `GetCoverage` request's
    // `OnlineResource` (1.0); the capabilities address when the document gives no web address.
    let get = root
        .descendants()
        .find(|n| n.is_element() && (local(n) == "GetCoverage" || (local(n) == "Operation" && n.attribute("name") == Some("GetCoverage"))))
        .and_then(|g| g.descendants().find(|n| n.is_element() && local(n) == "Get"))
        .and_then(|g| href(&g).or_else(|| g.descendants().find(|n| n.is_element() && local(n) == "OnlineResource").and_then(|o| href(&o))))
        // MapServer escapes its `&` twice: `&amp;` is still there after the XML is read.
        .map(|h| h.replace("&amp;", "&"))
        .filter(|h| is_web(h))
        .unwrap_or_else(|| endpoint(url));
    let formats: Vec<String> = root
        .descendants()
        .filter(|n| n.is_element() && matches!(local(n), "formatSupported" | "SupportedFormat"))
        .map(|n| text_of(&n))
        .filter(|f| !f.is_empty() && f.len() < 200)
        .take(64)
        .collect();
    let corners = |b: &Node| -> Option<[f64; 4]> {
        let lo: Vec<f64> = child_text(b, "LowerCorner")?.split_whitespace().filter_map(|t| t.parse().ok()).collect();
        let hi: Vec<f64> = child_text(b, "UpperCorner")?.split_whitespace().filter_map(|t| t.parse().ok()).collect();
        lonlat_box(*lo.first()?, *lo.get(1)?, *hi.first()?, *hi.get(1)?)
    };
    let mut add = |id: String, title: Option<String>, bbox: Option<[f64; 4]>| {
        let coverage = Coverage { endpoint: get.clone(), version: version.into(), id: id.clone(), formats: formats.clone() };
        service.layers.push(ServiceLayer {
            title: title.filter(|t| !t.is_empty()).unwrap_or_else(|| id.clone()),
            name: id,
            offer: Offer::Coverage(coverage),
            bbox,
            tilesets: None,
        });
    };
    let mut n = 0;
    // WCS 1.1 and 2.0: `CoverageSummary` (nested ones too), named by `CoverageId` or `Identifier`.
    for s in root.descendants().filter(|n| n.is_element() && local(n) == "CoverageSummary") {
        let Some(id) = child_text(&s, "CoverageId").or_else(|| child_text(&s, "Identifier")) else { continue };
        if id.len() > 500 || n >= MAX_LAYERS {
            continue;
        }
        n += 1;
        let bbox = child(&s, "WGS84BoundingBox").and_then(|b| corners(&b));
        add(id, child_text(&s, "Title"), bbox);
    }
    // WCS 1.0: `CoverageOfferingBrief`, named by `name`, its bounds a `lonLatEnvelope`.
    for b in root.descendants().filter(|n| n.is_element() && local(n) == "CoverageOfferingBrief") {
        let Some(id) = child_text(&b, "name") else { continue };
        if id.len() > 500 || n >= MAX_LAYERS {
            continue;
        }
        n += 1;
        let bbox = child(&b, "lonLatEnvelope").and_then(|e| {
            let pos: Vec<f64> = children(&e, "pos")
                .flat_map(|p| text_of(&p).split_whitespace().filter_map(|t| t.parse().ok()).collect::<Vec<f64>>())
                .collect();
            lonlat_box(*pos.first()?, *pos.get(1)?, *pos.get(2)?, *pos.get(3)?)
        });
        add(id, child_text(&b, "label"), bbox);
    }
    service
}

/// The DescribeCoverage request of a coverage.
pub fn describe_coverage_url(c: &Coverage) -> String {
    let q = if c.version.starts_with('2') {
        format!("SERVICE=WCS&VERSION={}&REQUEST=DescribeCoverage&COVERAGEID={}", c.version, encode(&c.id))
    } else {
        format!("SERVICE=WCS&VERSION=1.0.0&REQUEST=DescribeCoverage&COVERAGE={}", encode(&c.id))
    };
    with_query(&c.endpoint, &q)
}

/// The GeoTIFF format among those offered: one naming TIFF (`image/tiff`, `GeoTIFF`,
/// `GEOTIFF_INT16`), else `image/tiff`.
fn tiff_format<'a>(offered: impl IntoIterator<Item = &'a str>) -> String {
    offered.into_iter().find(|f| f.to_ascii_lowercase().contains("tif")).unwrap_or("image/tiff").to_string()
}

/// Which of a coverage's axes, by their labels, runs east-west and which north-south: by name
/// (`Long`/`Lat`, `x`/`y`, `E`/`N`), else in the order given (latitude first for EPSG:4326).
fn lonlat_axes(labels: &[&str], srs: &str) -> Option<(String, String)> {
    let (a, b) = (*labels.first()?, *labels.get(1)?);
    let ok = |s: &str| !s.is_empty() && s.len() <= 32 && s.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-');
    if !ok(a) || !ok(b) {
        return None;
    }
    let is_lat = |s: &str| matches!(s.to_ascii_lowercase().as_str(), "lat" | "latitude" | "y" | "n" | "north" | "northing");
    let is_lon = |s: &str| matches!(s.to_ascii_lowercase().as_str(), "long" | "lon" | "longitude" | "x" | "e" | "east" | "easting");
    Some(if is_lon(a) || is_lat(b) {
        (a.to_string(), b.to_string())
    } else if is_lat(a) || is_lon(b) || srs.ends_with("/4326") || srs.ends_with(":4326") {
        (b.to_string(), a.to_string())
    } else {
        (a.to_string(), b.to_string())
    })
}

/// Reads a coverage's DescribeCoverage answer.
pub fn parse_describe(c: &Coverage, body: &[u8]) -> Result<CoverageInfo, String> {
    let text = std::str::from_utf8(body).map_err(|_| "the coverage's description isn't UTF-8 text".to_string())?;
    let doc = xml(text, 1_000_000).map_err(|e| format!("the coverage's description: {e}"))?;
    let root = doc.root_element();
    if matches!(local(&root), "ServiceExceptionReport" | "ExceptionReport") {
        return Err(format!("the service answered with an error: {}", text_of(&root).chars().take(300).collect::<String>()));
    }
    if c.version.starts_with('2') {
        let Some(env) = root.descendants().find(|n| n.is_element() && local(n) == "Envelope") else {
            return Err(format!("{}: the description gives no envelope", c.id));
        };
        let srs = env.attribute("srsName").unwrap_or("").to_string();
        let labels_text = env.attribute("axisLabels").unwrap_or("").to_string();
        let labels: Vec<&str> = labels_text.split_whitespace().collect();
        let axes = lonlat_axes(&labels, &srs);
        let corner = |k: &str| -> Option<Vec<f64>> {
            let t = child_text(&env, k)?;
            Some(t.split_whitespace().filter_map(|v| v.parse().ok()).collect())
        };
        let s = srs.to_ascii_lowercase();
        let bbox = (|| {
            let (lo, hi) = (corner("lowerCorner")?, corner("upperCorner")?);
            let (lo0, lo1, hi0, hi1) = (*lo.first()?, *lo.get(1)?, *hi.first()?, *hi.get(1)?);
            // Which corner value is the longitude: by the axes' order in the labels; without
            // labels, EPSG:4326's own order (latitude first), CRS84's and others' longitude first.
            let lon_first = match &axes {
                Some((lon, _)) => labels.first() == Some(&lon.as_str()),
                None => !(s.ends_with("/4326") || s.ends_with(":4326")),
            };
            if s.ends_with("/4326") || s.ends_with(":4326") || s.contains("crs84") {
                if lon_first { lonlat_box(lo0, lo1, hi0, hi1) } else { lonlat_box(lo1, lo0, hi1, hi0) }
            } else if is_web_mercator(&srs) {
                let (w, so) = merc_to_lonlat(lo0, lo1);
                let (e, n) = merc_to_lonlat(hi0, hi1);
                lonlat_box(w, so, e, n)
            } else {
                None
            }
        })();
        let native = root.descendants().find(|n| n.is_element() && local(n) == "nativeFormat").map(|n| text_of(&n));
        let offered: Vec<&str> =
            native.iter().map(String::as_str).filter(|f| !f.is_empty()).chain(c.formats.iter().map(String::as_str)).collect();
        return Ok(CoverageInfo { format: tiff_format(offered), axes, bbox });
    }
    // WCS 1.0: the formats it answers in, and its longitude/latitude envelope.
    let formats: Vec<String> = root
        .descendants()
        .filter(|n| n.is_element() && local(n) == "formats")
        .map(|n| text_of(&n))
        .filter(|f| !f.is_empty() && f.len() < 200)
        .take(64)
        .collect();
    if root.descendants().all(|n| !(n.is_element() && local(&n) == "CoverageOffering")) {
        return Err(format!("{}: not a coverage description", c.id));
    }
    let bbox = root.descendants().find(|n| n.is_element() && local(n) == "lonLatEnvelope").and_then(|e| {
        let pos: Vec<f64> =
            children(&e, "pos").flat_map(|p| text_of(&p).split_whitespace().filter_map(|t| t.parse().ok()).collect::<Vec<f64>>()).collect();
        lonlat_box(*pos.first()?, *pos.get(1)?, *pos.get(2)?, *pos.get(3)?)
    });
    Ok(CoverageInfo { format: tiff_format(formats.iter().map(String::as_str)), axes: None, bbox })
}

/// The pixel size of a coverage asked for over a longitude/latitude `area`: its longer side
/// [`COVERAGE_MAX_PIXELS`], square degrees.
pub fn coverage_size(area: [f64; 4]) -> (u32, u32) {
    let (dx, dy) = ((area[2] - area[0]).abs().max(1e-9), (area[3] - area[1]).abs().max(1e-9));
    let m = f64::from(COVERAGE_MAX_PIXELS);
    let (w, h) = if dx >= dy { (m, (m * dy / dx).round()) } else { ((m * dx / dy).round(), m) };
    (w.clamp(1.0, m) as u32, h.clamp(1.0, m) as u32)
}

/// The GetCoverage request of a coverage over a longitude/latitude `area`, as a GeoTIFF in
/// EPSG:4326 (the server reprojects; WCS 2.0 through its CRS extension), scaled to fit
/// [`COVERAGE_MAX_PIXELS`].
pub fn get_coverage_url(c: &Coverage, info: &CoverageInfo, area: [f64; 4]) -> String {
    let [w, s, e, n] = area;
    let (pw, ph) = coverage_size(area);
    let q = if c.version.starts_with('2') {
        let (lon, lat) = info.axes.clone().unwrap_or_else(|| ("x".into(), "y".into()));
        let crs = "http://www.opengis.net/def/crs/EPSG/0/4326";
        format!(
            "SERVICE=WCS&VERSION={}&REQUEST=GetCoverage&COVERAGEID={}&FORMAT={}&SUBSETTINGCRS={crs}&OUTPUTCRS={crs}&SUBSET={lon}({w},{e})&SUBSET={lat}({s},{n})&SCALESIZE={lon}({pw}),{lat}({ph})",
            c.version,
            encode(&c.id),
            encode(&info.format)
        )
    } else {
        format!(
            "SERVICE=WCS&VERSION=1.0.0&REQUEST=GetCoverage&COVERAGE={}&CRS=EPSG:4326&BBOX={w},{s},{e},{n}&WIDTH={pw}&HEIGHT={ph}&FORMAT={}",
            encode(&c.id),
            encode(&info.format)
        )
    };
    with_query(&c.endpoint, &q)
}

/// The part of `area` a coverage covers (all of `area` when its bounds aren't known); `None` when
/// they don't meet.
pub fn coverage_area(info: &CoverageInfo, area: [f64; 4]) -> Option<[f64; 4]> {
    let Some([w, s, e, n]) = info.bbox else { return lonlat_box(area[0], area[1], area[2], area[3]) };
    lonlat_box(area[0].max(w), area[1].max(s), area[2].min(e), area[3].min(n))
}

// ---------------------------------------------------------------------------------------------
// CSW

/// Records asked of a catalogue in one request.
pub const RECORDS_PAGE: u32 = 20;

/// A catalogue record's link: a service (`wms`, `wmts`, `wfs`, `wcs`, `ogcapi`, `arcgis`) RustGIS
/// can open, a file to download, or another web page.
#[derive(Clone, Debug, PartialEq)]
pub struct RecordLink {
    pub url: String,
    pub kind: &'static str,
    /// The layer the link names, when it names one (a WMS link's layer).
    pub name: Option<String>,
    pub protocol: String,
}

/// A catalogue record (Dublin Core): its identifier, title, abstract, bounds and links.
#[derive(Clone, Debug, PartialEq)]
pub struct Record {
    pub id: String,
    pub title: String,
    pub summary: String,
    pub bbox: Option<[f64; 4]>,
    pub links: Vec<RecordLink>,
}

/// A page of a catalogue search: how many records match, the next page's first position, and the
/// records.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RecordPage {
    pub matched: u64,
    pub next: Option<u64>,
    pub records: Vec<Record>,
}

/// The GetRecords request (CSW 2.0.2, Dublin Core records) of a search: words anywhere in the
/// records, within a longitude/latitude box, from position `start` (1-based).
pub fn records_url(endpoint_url: &str, words: &str, bbox: Option<[f64; 4]>, start: u64) -> String {
    let words: String = words.chars().filter(|c| !c.is_control()).take(200).collect::<String>().trim().replace('\'', "''");
    let mut cql = Vec::new();
    if !words.is_empty() {
        cql.push(format!("AnyText LIKE '%{words}%'"));
    }
    if let Some([w, s, e, n]) = bbox {
        cql.push(format!("BBOX(ows:BoundingBox,{w},{s},{e},{n})"));
    }
    let mut q = format!(
        "SERVICE=CSW&VERSION=2.0.2&REQUEST=GetRecords&typeNames=csw:Record&resultType=results&elementSetName=full&outputSchema={}&maxRecords={RECORDS_PAGE}&startPosition={}",
        encode("http://www.opengis.net/cat/csw/2.0.2"),
        start.max(1)
    );
    if !cql.is_empty() {
        q.push_str(&format!("&constraintLanguage=CQL_TEXT&constraint_language_version=1.1.0&constraint={}", encode(&cql.join(" AND "))));
    }
    with_query(&endpoint(endpoint_url), &q)
}

/// What a link's protocol (or, when it says nothing useful, its address) makes it.
fn link_kind(protocol: &str, url: &str) -> &'static str {
    let p = protocol.to_ascii_lowercase();
    let u = url.to_ascii_lowercase();
    let says = |k: &str| p.contains(k);
    if says("wmts") {
        "wmts"
    } else if says("wms") {
        "wms"
    } else if says("wfs") {
        "wfs"
    } else if says("wcs") {
        "wcs"
    } else if says("ogc:api") || says("ogcapi") || says("ogc api") {
        "ogcapi"
    } else if says("esri:rest") || says("arcgis") {
        "arcgis"
    } else if says("download") {
        "download"
    } else if u.contains("service=wmts") || u.contains("/wmts") {
        "wmts"
    } else if u.contains("service=wms") || u.contains("/wms") {
        "wms"
    } else if u.contains("service=wfs") || u.contains("/wfs") {
        "wfs"
    } else if u.contains("service=wcs") || u.contains("/wcs") {
        "wcs"
    } else if u.contains("/rest/services/") {
        "arcgis"
    } else {
        "web"
    }
}

/// The first line of an attribute or text that repeats itself across languages (geocat's).
fn first_line(s: &str) -> String {
    s.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("").chars().take(500).collect()
}

/// Reads a GetRecords answer fetched from `url`.
pub fn parse_records(url: &str, body: &[u8]) -> Result<RecordPage, String> {
    let text = std::str::from_utf8(body).map_err(|_| "the catalogue's answer isn't UTF-8 text".to_string())?;
    let doc = xml(text, 2_000_000).map_err(|e| format!("the catalogue's XML: {e}"))?;
    let root = doc.root_element();
    match local(&root) {
        "GetRecordsResponse" => {}
        "ServiceExceptionReport" | "ExceptionReport" => {
            return Err(format!("the catalogue answered with an error: {}", text_of(&root).chars().take(300).collect::<String>()));
        }
        other => return Err(format!("not a catalogue's (CSW) answer (<{other}>)")),
    }
    let mut page = RecordPage::default();
    if let Some(r) = root.descendants().find(|n| n.is_element() && local(n) == "SearchResults") {
        let num = |k: &str| r.attribute(k).and_then(|v| v.trim().parse::<u64>().ok());
        page.matched = num("numberOfRecordsMatched").unwrap_or(0);
        page.next = num("nextRecord").filter(|n| *n > 0 && *n <= page.matched);
    }
    for rec in root
        .descendants()
        .filter(|n| n.is_element() && matches!(local(n), "Record" | "SummaryRecord" | "BriefRecord"))
        .take(RECORDS_PAGE as usize * 5)
    {
        let field = |k: &str| child(&rec, k).map(|c| first_line(&text_of(&c))).filter(|t| !t.is_empty());
        let id = field("identifier").unwrap_or_default();
        let title = field("title").unwrap_or_else(|| id.clone());
        let summary: String =
            child(&rec, "abstract").or_else(|| child(&rec, "description")).map(|c| first_line(&text_of(&c))).unwrap_or_default();
        // Bounds: a WGS84 box (longitude first), else a box in EPSG:4326 (latitude first when
        // given by URN, as the standard has it), else CRS84.
        let bbox = child(&rec, "WGS84BoundingBox")
            .and_then(|b| {
                let lo: Vec<f64> = child_text(&b, "LowerCorner")?.split_whitespace().filter_map(|t| t.parse().ok()).collect();
                let hi: Vec<f64> = child_text(&b, "UpperCorner")?.split_whitespace().filter_map(|t| t.parse().ok()).collect();
                lonlat_box(*lo.first()?, *lo.get(1)?, *hi.first()?, *hi.get(1)?)
            })
            .or_else(|| {
                let b = child(&rec, "BoundingBox")?;
                let crs = b.attribute("crs").unwrap_or("").to_ascii_lowercase();
                let lo: Vec<f64> = child_text(&b, "LowerCorner")?.split_whitespace().filter_map(|t| t.parse().ok()).collect();
                let hi: Vec<f64> = child_text(&b, "UpperCorner")?.split_whitespace().filter_map(|t| t.parse().ok()).collect();
                let (a, b0, c, d) = (*lo.first()?, *lo.get(1)?, *hi.first()?, *hi.get(1)?);
                if crs.contains("urn:") && crs.contains("4326") { lonlat_box(b0, a, d, c) } else { lonlat_box(a, b0, c, d) }
            });
        let mut links: Vec<RecordLink> = Vec::new();
        for l in rec.children().filter(|c| c.is_element() && matches!(local(c), "URI" | "references")).take(50) {
            let Some(href) = text_of(&l).split_whitespace().next().and_then(|u| resolve(url, u)) else { continue };
            if href.len() > 2000 || links.iter().any(|k| k.url == href) {
                continue;
            }
            let protocol = l.attribute("protocol").or_else(|| l.attribute("scheme")).map(first_line).unwrap_or_default();
            let name = l.attribute("name").map(first_line).filter(|n| !n.is_empty());
            links.push(RecordLink { kind: link_kind(&protocol, &href), url: href, name, protocol });
        }
        page.records.push(Record { id, title, summary, bbox, links });
    }
    Ok(page)
}

// ---------------------------------------------------------------------------------------------
// ArcGIS REST

/// Web Mercator metres to longitude/latitude.
fn merc_to_lonlat(x: f64, y: f64) -> (f64, f64) {
    const R: f64 = 6_378_137.0;
    ((x / R).to_degrees(), (2.0 * (y / R).exp().atan() - std::f64::consts::FRAC_PI_2).to_degrees())
}

/// An ArcGIS extent (`{xmin, ymin, xmax, ymax, spatialReference}`) in longitude/latitude, for
/// WGS 84 and Web Mercator extents.
fn arcgis_extent(e: &serde_json::Value) -> Option<[f64; 4]> {
    let f = |k: &str| e.get(k).and_then(|v| v.as_f64());
    let (x0, y0, x1, y1) = (f("xmin")?, f("ymin")?, f("xmax")?, f("ymax")?);
    let wkid = e.pointer("/spatialReference/latestWkid").or_else(|| e.pointer("/spatialReference/wkid")).and_then(|w| w.as_i64())?;
    match wkid {
        4326 => lonlat_box(x0, y0, x1, y1),
        3857 | 102100 | 102113 | 900913 => {
            let (w, s) = merc_to_lonlat(x0, y0);
            let (e, n) = merc_to_lonlat(x1, y1);
            lonlat_box(w.max(-180.0), s.max(-85.06), e.min(180.0), n.min(85.06))
        }
        _ => None,
    }
}

/// Whether a cached map service's tiles are Web Mercator's own pyramid (zoom 0 at 156543 m/px).
fn arcgis_mercator_cache(v: &serde_json::Value) -> bool {
    let Some(ti) = v.get("tileInfo") else { return false };
    let wkid = ti.pointer("/spatialReference/latestWkid").or_else(|| ti.pointer("/spatialReference/wkid")).and_then(|w| w.as_i64());
    let first = ti.pointer("/lods/0/resolution").and_then(|r| r.as_f64());
    let level0 = ti.pointer("/lods/0/level").and_then(|l| l.as_i64());
    let rows = ti.get("rows").and_then(|r| r.as_i64()).unwrap_or(256);
    matches!(wkid, Some(3857 | 102100 | 102113))
        && rows == 256
        && level0 == Some(0)
        && first.is_some_and(|r| (r - 156_543.033_928_041).abs() < 0.01)
}

fn arcgis(url: &str, v: &serde_json::Value) -> Result<Service, String> {
    let b = base(url).trim_end_matches('/').to_string();
    let lower = b.to_ascii_lowercase();
    // A name, never the service's description (often a paragraph of HTML). "Layers" and "Map"
    // are ArcGIS's defaults, which say nothing: the service's own name in the address does.
    let title = ["/mapName", "/name", "/documentInfo/Title"]
        .iter()
        .filter_map(|k| v.pointer(k).and_then(|t| t.as_str()).map(str::trim))
        .find(|t| !t.is_empty() && !["Layers", "Map"].contains(t) && !t.contains(['<', '\n']) && t.chars().count() <= 200)
        .unwrap_or_else(|| b.rsplit('/').nth(1).filter(|n| !n.is_empty()).unwrap_or("ArcGIS"))
        .to_string();
    let full = v.get("fullExtent").or_else(|| v.get("extent")).and_then(arcgis_extent);
    let mut service = Service { kind: "ArcGIS".into(), title: title.clone(), ..Service::default() };
    // A directory of services: say which to open.
    if v.get("services").is_some() && !lower.ends_with("server") {
        let names: Vec<String> = v
            .get("services")
            .and_then(|s| s.as_array())
            .map(|a| a.iter().filter_map(|s| Some(format!("{}/{}", s.get("name")?.as_str()?, s.get("type")?.as_str()?))).take(30).collect())
            .unwrap_or_default();
        return Err(format!("an ArcGIS services directory: open one of its services ({})", names.join(", ")));
    }
    // Feature layers (FeatureServer, and a MapServer's layers): queried as GeoJSON, so only from a
    // service that allows queries: "Query", or "Data" on a MapServer (a cached basemap's
    // capabilities are often just "Map,Tilemap").
    let features = |service: &mut Service| {
        let queries = v
            .get("capabilities")
            .and_then(|c| c.as_str())
            .is_none_or(|c| c.split(',').any(|k| ["query", "data"].iter().any(|q| k.trim().eq_ignore_ascii_case(q))));
        if !queries {
            if v.get("layers").and_then(|l| l.as_array()).is_some_and(|a| !a.is_empty()) {
                service.notes.push("the service doesn't allow queries, so its layers' features can't be read".into());
            }
            return;
        }
        for l in v.get("layers").and_then(|l| l.as_array()).into_iter().flatten().take(MAX_LAYERS) {
            let (Some(id), Some(name)) = (l.get("id").and_then(|i| i.as_i64()), l.get("name").and_then(|n| n.as_str())) else { continue };
            // Group layers have sub-layers and no features of their own.
            if l.get("subLayerIds").is_some_and(|s| s.as_array().is_some_and(|a| !a.is_empty())) {
                continue;
            }
            let url = format!("{b}/{id}/query?where=1%3D1&outFields=*&returnGeometry=true&outSR=4326&f=geojson");
            service.layers.push(ServiceLayer {
                name: id.to_string(),
                title: name.to_string(),
                offer: Offer::Features { url },
                bbox: full,
                tilesets: None,
            });
        }
    };
    if lower.ends_with("/featureserver") {
        service.kind = "ArcGIS FeatureServer".into();
        features(&mut service);
    } else if lower.ends_with("/mapserver") {
        service.kind = "ArcGIS MapServer".into();
        // The whole map: its cache when it is Web Mercator's pyramid, else exported images.
        let template = if v.get("singleFusedMapCache").and_then(|c| c.as_bool()) == Some(true) && arcgis_mercator_cache(v) {
            format!("{b}/tile/{{z}}/{{y}}/{{x}}")
        } else {
            format!("{b}/export?bbox={{bbox-epsg-3857}}&bboxSR=3857&imageSR=3857&size=256,256&format=png32&transparent=true&f=image")
        };
        service.layers.push(ServiceLayer {
            name: "map".into(),
            title: format!("{title} (map)"),
            offer: Offer::Tiles { template },
            bbox: full,
            tilesets: None,
        });
        features(&mut service);
    } else if lower.ends_with("/imageserver") {
        service.kind = "ArcGIS ImageServer".into();
        let template = format!("{b}/exportImage?bbox={{bbox-epsg-3857}}&bboxSR=3857&imageSR=3857&size=256,256&format=png&f=image");
        service.layers.push(ServiceLayer {
            name: "image".into(),
            title: title.clone(),
            offer: Offer::Tiles { template },
            bbox: full,
            tilesets: None,
        });
    } else if lower.ends_with("/vectortileserver") {
        service.kind = "ArcGIS VectorTileServer".into();
        let style = v
            .get("defaultStyles")
            .and_then(|s| s.as_str())
            .map(|s| format!("{b}/{}/root.json", s.trim_matches('/')))
            .unwrap_or_else(|| format!("{b}/resources/styles/root.json"));
        service.layers.push(ServiceLayer {
            name: "style".into(),
            title: title.clone(),
            offer: Offer::Style { url: style },
            bbox: full,
            tilesets: None,
        });
    } else {
        return Err("an ArcGIS address, but not a FeatureServer, MapServer, ImageServer or VectorTileServer".into());
    }
    // A feature layer is one request: the server's record limit bounds it (a cut-off answer is
    // pointed out when it lands).
    if let Some(max) = v.get("maxRecordCount").and_then(|m| m.as_i64())
        && service.layers.iter().any(|l| matches!(l.offer, Offer::Features { .. }))
    {
        service.notes.push(format!("a feature layer is read up to the server's {max} features per request"));
    }
    // A secured service's token goes with every request.
    if let Some(token) = query_value(url, "token").filter(|t| !t.is_empty()) {
        for l in &mut service.layers {
            match &mut l.offer {
                Offer::Tiles { template: u }
                | Offer::Features { url: u }
                | Offer::Style { url: u }
                | Offer::VectorTiles { template: u, .. }
                | Offer::Coverage(Coverage { endpoint: u, .. }) => *u = with_query(u, &format!("token={token}")),
            }
        }
    }
    Ok(service)
}

// ---------------------------------------------------------------------------------------------
// OGC API Features

fn ogc_api(url: &str, v: &serde_json::Value) -> Result<Service, String> {
    let collections = v.get("collections").and_then(|c| c.as_array());
    let Some(collections) = collections else {
        // A landing page can offer both dataset tiles and a collections document to discover.
        let title = v.get("title").and_then(|t| t.as_str()).unwrap_or_default().to_string();
        let mut service = Service { kind: "OGC API Tiles".into(), title: title.clone(), ..Service::default() };
        let links = v.get("links").and_then(|l| l.as_array()).cloned().unwrap_or_default();
        service.collections_url = collections_link(url, &links);
        let name = if title.is_empty() { "tiles".to_string() } else { title };
        // The dataset's layers aren't listed here, so its vector tiles can't be styled: map tiles only.
        if let Some((t, list)) = tileset(url, &links, TileRel::Map) {
            service.layers.push(ServiceLayer {
                name: "map".into(),
                title: format!("{name} (map tiles)"),
                offer: Offer::Tiles { template: t },
                bbox: None,
                tilesets: Some(list),
            });
        }
        if !service.layers.is_empty() || service.collections_url.is_some() {
            if service.layers.is_empty() {
                service.kind = "OGC API Features".into();
            }
            return Ok(service);
        }
        return Err(if links.iter().any(is_data_link) {
            "the OGC API landing page has no usable JSON collections link".into()
        } else {
            "not an OGC API collections document (no \"collections\")".into()
        });
    };
    let title = v.get("title").and_then(|t| t.as_str()).unwrap_or_default().to_string();
    let mut service = Service { kind: "OGC API Features".into(), title, ..Service::default() };
    let root = base(url).trim_end_matches('/').to_string();
    let mut unreadable = 0;
    let mut tiled = 0;
    for c in collections.iter().take(MAX_LAYERS) {
        let Some(id) = c.get("id").and_then(|i| i.as_str()) else { continue };
        let links = c.get("links").and_then(|l| l.as_array()).cloned().unwrap_or_default();
        let title = c.get("title").and_then(|t| t.as_str()).unwrap_or(id).to_string();
        let bbox = c.pointer("/extent/spatial/bbox/0").and_then(|b| b.as_array()).and_then(|b| {
            let f = |i: usize| b.get(i).and_then(|v| v.as_f64());
            lonlat_box(f(0)?, f(1)?, f(2)?, f(3)?)
        });
        // OGC API Tiles: the collection's map and vector tiles in WebMercatorQuad. Its vector
        // tiles' layer is named after the collection, as the standard has it.
        let map = tileset(url, &links, TileRel::Map);
        let vector = tileset(url, &links, TileRel::Vector);
        if let Some((t, list)) = &map {
            service.layers.push(ServiceLayer {
                name: format!("{id} (map tiles)"),
                title: format!("{title} (map tiles)"),
                offer: Offer::Tiles { template: t.clone() },
                bbox,
                tilesets: Some(list.clone()),
            });
        }
        if let Some((t, list)) = &vector {
            service.layers.push(ServiceLayer {
                name: format!("{id} (vector tiles)"),
                title: format!("{title} (vector tiles)"),
                offer: Offer::VectorTiles { template: t.clone(), layers: vec![id.chars().take(200).collect()] },
                bbox,
                tilesets: Some(list.clone()),
            });
        }
        if map.is_some() || vector.is_some() {
            tiled += 1;
        }
        let link = links
            .iter()
            .filter(|l| l.get("rel").and_then(|r| r.as_str()) == Some("items"))
            .find(|l| l.get("type").and_then(|t| t.as_str()).is_none_or(|t| t.contains("json")));
        // A collection with tiles and no items link has no features to offer.
        if link.is_none() && (map.is_some() || vector.is_some()) {
            continue;
        }
        let declared_geojson = link.and_then(|l| l.get("type").and_then(|t| t.as_str())).is_some_and(|t| t.contains("geo+json"));
        // Without an items link: the standard path under the collections address. A relative
        // link is resolved against the document's address.
        let items = match link.and_then(|l| l.get("href").and_then(|h| h.as_str())) {
            Some(h) => match resolve(url, h) {
                Some(u) => u,
                None => {
                    unreadable += 1;
                    continue;
                }
            },
            None => format!("{root}/{}/items", encode(id)),
        };
        let items = if query_value(&items, "limit").is_some() { items } else { with_query(&items, &format!("limit={OGC_API_LIMIT}")) };
        // GeoJSON asked for unless the link says it is already.
        let items = if declared_geojson || query_value(&items, "f").is_some() { items } else { with_query(&items, "f=json") };
        service.layers.push(ServiceLayer { name: id.to_string(), title, offer: Offer::Features { url: items }, bbox, tilesets: None });
    }
    if unreadable > 0 {
        service.notes.push(format!("{unreadable} collections link to their items with an address RustGIS doesn't fetch and are left out"));
    }
    if collections.len() > MAX_LAYERS {
        service.notes.push(format!("only the first {MAX_LAYERS} collections are listed"));
    }
    service.incomplete = unreadable > 0 || collections.len() > MAX_LAYERS;
    if service.layers.iter().any(|l| matches!(l.offer, Offer::Features { .. })) {
        service.notes.push(format!("a collection is read up to {OGC_API_LIMIT} features (one page of the server's)"));
    }
    if tiled > 0 {
        service.kind = if service.layers.iter().any(|l| matches!(l.offer, Offer::Features { .. })) {
            "OGC API Features and Tiles".into()
        } else {
            "OGC API Tiles".into()
        };
    }
    Ok(service)
}

/// Which tiles of an OGC API: map (pictures) or vector (Mapbox Vector Tiles).
#[derive(Clone, Copy, PartialEq)]
enum TileRel {
    Map,
    Vector,
}

/// The tiles of a dataset or collection in WebMercatorQuad, from its `tilesets-map` or
/// `tilesets-vector` link: the template at the usual path under the tileset list
/// (`…/WebMercatorQuad/{tileMatrix}/{tileRow}/{tileCol}`), and the list's address (JSON), whose
/// WebMercatorQuad tileset advertises the template [`resolve_tiles`] reads (the usual path is what
/// is used when that can't be read). Both keep the link's query (a token, a collections choice),
/// all but `f`.
fn tileset(from: &str, links: &[serde_json::Value], which: TileRel) -> Option<(String, String)> {
    let suffix = match which {
        TileRel::Map => "tilesets-map",
        TileRel::Vector => "tilesets-vector",
    };
    let link = links
        .iter()
        .take(200)
        .filter(|l| l.get("rel").and_then(|r| r.as_str()).is_some_and(|r| r == suffix || r.ends_with(&format!("/{suffix}"))));
    // The JSON link first (or one that names no type), else any.
    let chosen: Vec<&serde_json::Value> = link.collect();
    let l = chosen.iter().find(|l| l.get("type").and_then(|t| t.as_str()).is_none_or(|t| t.contains("json"))).or_else(|| chosen.first())?;
    let href = resolve(from, l.get("href")?.as_str()?)?;
    let at = base(&href).trim_end_matches('/');
    if at.contains(['{', '}']) || at.len() > 2000 {
        return None;
    }
    // Map tiles come in the server's own image format (PNG or JPEG, which it picks); vector
    // tiles are asked as MVT.
    let template = match which {
        TileRel::Map => format!("{at}/WebMercatorQuad/{{z}}/{{y}}/{{x}}"),
        TileRel::Vector => format!("{at}/WebMercatorQuad/{{z}}/{{y}}/{{x}}?f=mvt"),
    };
    let list = if query_value(&href, "f").is_some() { href.clone() } else { with_query(&href, "f=json") };
    Some((carry_query(&template, &href), list))
}

/// `template` with the query parameters of `link` it doesn't have, but `f` (the representation
/// the link was for).
fn carry_query(template: &str, link: &str) -> String {
    let q = link.split('#').next().and_then(|u| u.split_once('?')).map(|(_, q)| q).unwrap_or("");
    let mut out = template.to_string();
    for kv in q.split('&').take(32) {
        let k = kv.split_once('=').map(|(k, _)| k).unwrap_or(kv);
        if kv.is_empty() || k.eq_ignore_ascii_case("f") || query_value(&out, k).is_some() || kv.contains(['{', '}']) {
            continue;
        }
        out = with_query(&out, kv);
    }
    out
}

/// What an OGC API tiles document says on the way to a tile template: the template, or the
/// address of the document to read next (a tileset list's WebMercatorQuad tileset).
#[derive(Clone, Debug, PartialEq)]
pub enum TilesStep {
    Template(String),
    Next(String),
}

/// One step of reading an OGC API tile template: `body` is the tileset list (or a tileset)
/// fetched from `doc`; `list` is the list's address, whose query the template keeps. `None` when
/// the document names no WebMercatorQuad tileset or tile template.
pub fn resolve_tiles(list: &str, doc: &str, body: &[u8], vector: bool) -> Option<TilesStep> {
    let v: serde_json::Value = serde_json::from_slice(body).ok()?;
    let links = |v: &serde_json::Value| v.get("links").and_then(|l| l.as_array()).cloned().unwrap_or_default();
    let Some(sets) = v.get("tilesets").and_then(|t| t.as_array()) else {
        // A tileset: its tile template.
        return item_template(doc, &links(&v), vector).map(|t| TilesStep::Template(carry_query(&t, list)));
    };
    let mercator = |t: &serde_json::Value| {
        let tiling = links(t);
        let named: Vec<String> = ["tileMatrixSetURI", "tileMatrixSetId", "tileMatrixSet", "id"]
            .iter()
            .filter_map(|k| t.get(*k).and_then(|x| x.as_str()))
            .chain(
                tiling
                    .iter()
                    .filter(|l| l.get("rel").and_then(|r| r.as_str()).is_some_and(|r| r.contains("tiling-scheme")))
                    .filter_map(|l| l.get("href").and_then(|h| h.as_str())),
            )
            .map(|n| base(n).trim_end_matches('/').to_ascii_lowercase())
            .collect();
        named.iter().any(|n| n.ends_with("webmercatorquad") || n.ends_with("googlemapscompatible"))
    };
    let kind_fits = |t: &serde_json::Value| match t.get("dataType").and_then(|d| d.as_str()) {
        Some(d) => (d == "vector") == vector,
        None => true,
    };
    let t = sets.iter().take(200).find(|t| mercator(t) && kind_fits(t))?;
    let ls = links(t);
    if let Some(template) = item_template(doc, &ls, vector) {
        return Some(TilesStep::Template(carry_query(&template, list)));
    }
    let next = ls
        .iter()
        .find(|l| l.get("rel").and_then(|r| r.as_str()).is_some_and(|r| r == "self" || r.ends_with("/tileset")))
        .and_then(|l| resolve(doc, l.get("href")?.as_str()?))?;
    let next = carry_query(&next, list);
    Some(TilesStep::Next(if query_value(&next, "f").is_some() { next } else { with_query(&next, "f=json") }))
}

/// A tileset's advertised tile template (its `item` link, `{tileMatrix}/{tileRow}/{tileCol}`) as
/// `{z}/{y}/{x}`, in the tiles' format: MVT for vector tiles, an image (PNG first) for map tiles.
fn item_template(from: &str, links: &[serde_json::Value], vector: bool) -> Option<String> {
    let items: Vec<&serde_json::Value> = links
        .iter()
        .take(200)
        .filter(|l| l.get("rel").and_then(|r| r.as_str()).is_some_and(|r| r == "item" || r.ends_with("/item")))
        .filter(|l| l.get("href").and_then(|h| h.as_str()).is_some_and(|h| h.contains("{tileMatrix}")))
        .collect();
    let ty = |l: &&&serde_json::Value| l.get("type").and_then(|t| t.as_str()).unwrap_or("").to_ascii_lowercase();
    let pick = if vector {
        items.iter().find(|l| {
            let t = ty(l);
            t.contains("mapbox-vector-tile") || t.contains("mvt") || t.contains("protobuf")
        })
    } else {
        items.iter().find(|l| ty(l) == "image/png").or_else(|| items.iter().find(|l| ty(l).starts_with("image/")))
    }
    .or_else(|| items.first())?;
    let href = pick.get("href")?.as_str()?;
    if href.len() > 2000 {
        return None;
    }
    let t = href.replace("{tileMatrix}", "{z}").replace("{tileRow}", "{y}").replace("{tileCol}", "{x}");
    // Only the three coordinates are filled in.
    let rest = t.replace("{z}", "").replace("{y}", "").replace("{x}", "");
    if rest.contains(['{', '}']) {
        return None;
    }
    let t = resolve(from, &t)?;
    is_web(&t).then_some(t)
}

// ---------------------------------------------------------------------------------------------
// Feature answers

/// How much of an answer's head and tail is searched for its paging members (they sit at the
/// start or the end of a feature collection).
const EDGE: usize = 64 * 1024;

/// The head and tail of an answer, as text.
fn edges(bytes: &[u8]) -> (String, String) {
    let head = bytes.get(..bytes.len().min(EDGE)).unwrap_or(bytes);
    let tail = bytes.get(bytes.len().saturating_sub(EDGE)..).unwrap_or(bytes);
    (String::from_utf8_lossy(head).into_owned(), String::from_utf8_lossy(tail).into_owned())
}

/// The number after a JSON key (`"numberMatched": 1234`), in `text`.
fn number_after(text: &str, key: &str) -> Option<u64> {
    let at = text.find(&format!("\"{key}\""))? + key.len() + 2;
    let rest = text.get(at..)?.trim_start().strip_prefix(':')?.trim_start();
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

/// Whether `text` holds `"key": true`.
fn is_true(text: &str, key: &str) -> bool {
    text.match_indices(&format!("\"{key}\"")).any(|(i, m)| {
        text.get(i + m.len()..).and_then(|r| r.trim_start().strip_prefix(':')).is_some_and(|r| r.trim_start().starts_with("true"))
    })
}

/// A note when a service's GeoJSON answer is only part of what it holds: an ArcGIS layer past its
/// record limit (`exceededTransferLimit`), or an OGC API or WFS 2.0 page (`numberMatched` above
/// `numberReturned`, or a `next` link).
pub fn partial_note(bytes: &[u8]) -> Option<String> {
    let (head, tail) = edges(bytes);
    if is_true(&head, "exceededTransferLimit") || is_true(&tail, "exceededTransferLimit") {
        return Some("the server sent its first features only (its record limit); the rest aren't read".into());
    }
    let n = |k: &str| number_after(&head, k).or_else(|| number_after(&tail, k));
    if let (Some(matched), Some(returned)) = (n("numberMatched"), n("numberReturned"))
        && matched > returned
    {
        return Some(format!("the server sent {returned} of its {matched} features (one page); the rest aren't read"));
    }
    let next = |t: &str| t.contains("\"rel\":\"next\"") || t.contains("\"rel\": \"next\"");
    (next(&head) || next(&tail)).then(|| "the server sent one page of its features; the rest aren't read".into())
}

/// Whether a GeoJSON answer names, as its `crs`, EPSG:4326 in the URN form that puts latitude
/// first (as GeoServer does for WFS 1.1 and 2.0).
fn lat_first(crs: &str) -> bool {
    let c = crs.to_ascii_lowercase();
    c.starts_with("urn:") && (c.ends_with("epsg::4326") || c.ends_with("epsg:6.6:4326") || c.ends_with("epsg:4326"))
}

fn swap_positions(v: &mut serde_json::Value, depth: usize) {
    if depth > 8 {
        return;
    }
    let Some(a) = v.as_array_mut() else { return };
    if a.len() >= 2 && a.iter().take(2).all(serde_json::Value::is_number) {
        a.swap(0, 1);
        return;
    }
    for c in a {
        swap_positions(c, depth + 1);
    }
}

fn swap_geometry(g: &mut serde_json::Value, depth: usize) {
    if depth > 8 {
        return;
    }
    if let Some(c) = g.get_mut("coordinates") {
        swap_positions(c, 0);
    }
    if let Some(gs) = g.get_mut("geometries").and_then(|g| g.as_array_mut()) {
        for g in gs {
            swap_geometry(g, depth + 1);
        }
    }
}

/// A service's GeoJSON answer with latitude-first coordinates (see [`lat_first`]) turned round to
/// longitude first, and its `crs` dropped; `None` when it is longitude first already.
pub fn fix_axes(bytes: &[u8]) -> Option<Vec<u8>> {
    let (head, tail) = edges(bytes);
    let mentions =
        |t: &str| t.to_ascii_lowercase().contains("urn:ogc:def:crs:epsg:") || t.to_ascii_lowercase().contains("urn:x-ogc:def:crs:epsg:");
    if !(mentions(&head) || mentions(&tail)) {
        return None;
    }
    let mut v: serde_json::Value = serde_json::from_slice(bytes).ok()?;
    let crs = v.pointer("/crs/properties/name").and_then(|n| n.as_str())?;
    if !lat_first(crs) {
        return None;
    }
    if let Some(o) = v.as_object_mut() {
        o.remove("crs");
    }
    match v.get("type").and_then(|t| t.as_str()) {
        Some("FeatureCollection") => {
            for f in v.get_mut("features").and_then(|f| f.as_array_mut()).into_iter().flatten() {
                if let Some(g) = f.get_mut("geometry") {
                    swap_geometry(g, 0);
                }
            }
        }
        Some("Feature") => {
            if let Some(g) = v.get_mut("geometry") {
                swap_geometry(g, 0);
            }
        }
        _ => swap_geometry(&mut v, 0),
    }
    serde_json::to_vec(&v).ok()
}

#[cfg(test)]
#[path = "../../../tests/support/ogc.rs"]
mod fixtures;

#[cfg(test)]
mod tests {
    use super::*;

    /// #167: World_Street_Map's map is named "Layers" and titled "Map", and its description is
    /// HTML: the service is named from its address, never from the description.
    #[test]
    fn arcgis_title_is_a_name_not_the_description() {
        let street = r#"{"currentVersion": 11.5, "mapName": "Layers",
            "serviceDescription": "<div style='text-align:Left'><p><span>This worldwide street map presents highway-level data.</span></p></div>",
            "documentInfo": {"Title": "Map", "Comments": "<DIV>This worldwide street map</DIV>"},
            "layers": [{"id": 0, "name": "World Street Map"}]}"#;
        let s = parse("https://services.arcgisonline.com/arcgis/rest/services/World_Street_Map/MapServer", street.as_bytes()).unwrap();
        assert_eq!(s.title, "World_Street_Map");
        assert_eq!(s.layers[0].title, "World_Street_Map (map)");
        assert_eq!(s.layers[1].title, "World Street Map");
        // Its capabilities are "Map,Tilemap": no queries, so only the map is offered.
        let cached = street.replace(r#""mapName""#, r#""capabilities": "Map,Tilemap", "mapName""#);
        let s = parse("https://services.arcgisonline.com/arcgis/rest/services/World_Street_Map/MapServer", cached.as_bytes()).unwrap();
        assert_eq!(s.layers.len(), 1);
        assert!(matches!(&s.layers[0].offer, Offer::Tiles { .. }));
        assert!(s.notes.iter().any(|n| n.contains("doesn't allow queries")), "{:?}", s.notes);
        let queried = street.replace(r#""mapName""#, r#""capabilities": "Map,Query,Data", "mapName""#);
        assert_eq!(parse("https://x.org/arcgis/rest/services/S/MapServer", queried.as_bytes()).unwrap().layers.len(), 2);
        let data = street.replace(r#""mapName""#, r#""capabilities": "Map,Data", "mapName""#);
        assert_eq!(parse("https://x.org/arcgis/rest/services/S/MapServer", data.as_bytes()).unwrap().layers.len(), 2);
        // A title of its own is kept; one that is HTML is not.
        let titled = r#"{"mapName": "Layers", "documentInfo": {"Title": "Street Map"}, "layers": []}"#;
        assert_eq!(parse("https://x.org/arcgis/rest/services/S/MapServer", titled.as_bytes()).unwrap().title, "Street Map");
        let html = r#"{"name": "<b>Parcels</b>", "serviceDescription": "Parcels", "layers": []}"#;
        assert_eq!(parse("https://x.org/arcgis/rest/services/Folder/P/FeatureServer", html.as_bytes()).unwrap().title, "P");
    }

    #[test]
    fn arcgis_services() {
        assert_eq!(
            capabilities_url("https://x.org/arcgis/rest/services/A/FeatureServer", None),
            "https://x.org/arcgis/rest/services/A/FeatureServer?f=json"
        );
        let fs = r#"{"currentVersion": 11.1, "serviceDescription": "Parcels", "maxRecordCount": 2000,
            "fullExtent": {"xmin": -1000000, "ymin": 0, "xmax": 1000000, "ymax": 1000000, "spatialReference": {"wkid": 102100, "latestWkid": 3857}},
            "layers": [{"id": 0, "name": "Parcels"}, {"id": 1, "name": "Group", "subLayerIds": [2]}, {"id": 2, "name": "Roads"}]}"#;
        let s = parse("https://x.org/arcgis/rest/services/A/FeatureServer", fs.as_bytes()).unwrap();
        assert_eq!(s.kind, "ArcGIS FeatureServer");
        assert_eq!(s.layers.iter().map(|l| l.title.as_str()).collect::<Vec<_>>(), ["Parcels", "Roads"]);
        assert!(
            matches!(&s.layers[0].offer, Offer::Features { url } if url == "https://x.org/arcgis/rest/services/A/FeatureServer/0/query?where=1%3D1&outFields=*&returnGeometry=true&outSR=4326&f=geojson")
        );
        let b = s.layers[0].bbox.unwrap();
        assert!((b[0] + 8.983).abs() < 0.01 && b[1].abs() < 1e-9, "{b:?}");
        let tiled = r#"{"currentVersion": 10.9, "mapName": "World", "singleFusedMapCache": true,
            "tileInfo": {"rows": 256, "spatialReference": {"wkid": 102100}, "lods": [{"level": 0, "resolution": 156543.03392800014}]}, "layers": []}"#;
        let s = parse("https://x.org/arcgis/rest/services/World/MapServer?f=json", tiled.as_bytes()).unwrap();
        assert_eq!(s.title, "World");
        assert!(
            matches!(&s.layers[0].offer, Offer::Tiles { template } if template == "https://x.org/arcgis/rest/services/World/MapServer/tile/{z}/{y}/{x}")
        );
        let dynamic = r#"{"currentVersion": 10.9, "mapName": "Dyn", "layers": [{"id": 3, "name": "Wells"}]}"#;
        let s = parse("https://x.org/arcgis/rest/services/Dyn/MapServer", dynamic.as_bytes()).unwrap();
        assert!(matches!(&s.layers[0].offer, Offer::Tiles { template } if template.contains("/export?bbox={bbox-epsg-3857}")));
        assert!(matches!(&s.layers[1].offer, Offer::Features { url } if url.contains("/MapServer/3/query?")));
        let vts = r#"{"currentVersion": 11, "name": "Basemap", "defaultStyles": "resources/styles"}"#;
        let s = parse("https://x.org/arcgis/rest/services/B/VectorTileServer", vts.as_bytes()).unwrap();
        assert!(
            matches!(&s.layers[0].offer, Offer::Style { url } if url == "https://x.org/arcgis/rest/services/B/VectorTileServer/resources/styles/root.json")
        );
        let img = r#"{"currentVersion": 11, "name": "Elev"}"#;
        let s = parse("https://x.org/arcgis/rest/services/E/ImageServer", img.as_bytes()).unwrap();
        assert!(matches!(&s.layers[0].offer, Offer::Tiles { template } if template.contains("/exportImage?")));
        let dir = r#"{"currentVersion": 11, "folders": [], "services": [{"name": "A", "type": "FeatureServer"}]}"#;
        assert!(parse("https://x.org/arcgis/rest/services", dir.as_bytes()).unwrap_err().contains("A/FeatureServer"));
        assert!(
            parse("https://x.org/arcgis/rest/services/A/FeatureServer", br#"{"error": {"code": 499, "message": "Token Required"}}"#)
                .unwrap_err()
                .contains("Token Required")
        );
    }

    #[test]
    fn capabilities_addresses() {
        assert_eq!(capabilities_url("https://x.org/wms", None), "https://x.org/wms?SERVICE=WMS&REQUEST=GetCapabilities");
        assert_eq!(
            capabilities_url("https://x.org/geoserver/ows?", Some("wfs")),
            "https://x.org/geoserver/ows?SERVICE=WFS&REQUEST=GetCapabilities"
        );
        assert_eq!(
            capabilities_url("https://x.org/a?map=b&request=GetCapabilities&service=WMS", None),
            "https://x.org/a?map=b&request=GetCapabilities&service=WMS"
        );
        assert_eq!(capabilities_url("https://x.org/ogc/features/collections", None), "https://x.org/ogc/features/collections?f=json");
        assert_eq!(
            capabilities_url("https://ows.example.org/osm/service", None),
            "https://ows.example.org/osm/service?SERVICE=WMS&REQUEST=GetCapabilities"
        );
        // The host doesn't decide the type: only the path and query do.
        assert_eq!(
            capabilities_url("https://rapid.example.org/geoserver/ows", None),
            "https://rapid.example.org/geoserver/ows?SERVICE=WMS&REQUEST=GetCapabilities"
        );
        assert_eq!(
            capabilities_url("https://wfs.example.org/maps", None),
            "https://wfs.example.org/maps?SERVICE=WMS&REQUEST=GetCapabilities"
        );
        assert_eq!(capabilities_url("https://x.org/geo/api/v1", None), "https://x.org/geo/api/v1?f=json");
        assert_eq!(capabilities_url("https://x.org/rapids/v1", None), "https://x.org/rapids/v1?SERVICE=WMS&REQUEST=GetCapabilities");
        // Whole query keys: `prof=` isn't `f=`.
        assert_eq!(
            capabilities_url("https://x.org/arcgis/rest/services/A/MapServer?prof=json", None),
            "https://x.org/arcgis/rest/services/A/MapServer?prof=json&f=json"
        );
        // ArcGIS's own OGC endpoints are read as WMS and WMTS.
        assert_eq!(
            capabilities_url("https://x.org/arcgis/rest/services/A/MapServer/WMSServer", None),
            "https://x.org/arcgis/rest/services/A/MapServer/WMSServer?SERVICE=WMS&REQUEST=GetCapabilities"
        );
        assert_eq!(query_value("https://x.org/a?F=json&limit=5#x", "f"), Some("json"));
        assert_eq!(query_value("https://x.org/a?shelf=1", "f"), None);
    }

    #[test]
    fn links_resolved_and_schemes_checked() {
        assert_eq!(
            resolve("https://x.org/api/collections?f=json", "collections/a/items").as_deref(),
            Some("https://x.org/api/collections/a/items")
        );
        assert_eq!(resolve("https://x.org/api/collections", "/ogc/a/items").as_deref(), Some("https://x.org/ogc/a/items"));
        assert_eq!(resolve("https://x.org", "a/items").as_deref(), Some("https://x.org/a/items"));
        assert_eq!(resolve("http://x.org/c", "//y.org/a").as_deref(), Some("http://y.org/a"));
        assert_eq!(resolve("https://x.org/c", "https://y.org/a").as_deref(), Some("https://y.org/a"));
        for bad in ["file:///etc/passwd", "httpfoo://x/a", "javascript:alert(1)", ""] {
            assert_eq!(resolve("https://x.org/c", bad), None, "{bad}");
        }
        assert!(is_web("HTTPS://x.org") && !is_web("httpfoo:x") && !is_web("http:/x"));
        // A capabilities document's endpoints in other schemes are not used.
        let xml = r#"<WMS_Capabilities version="1.3.0" xmlns:xlink="http://www.w3.org/1999/xlink"><Capability><Request><GetMap><DCPType><HTTP><Get><OnlineResource xlink:href="httpfoo://evil/wms"/></Get></HTTP></DCPType></GetMap></Request>
<Layer><Name>a</Name><CRS>EPSG:3857</CRS></Layer></Capability></WMS_Capabilities>"#;
        let s = parse("https://maps.example.org/wms", xml.as_bytes()).unwrap();
        assert!(matches!(&s.layers[0].offer, Offer::Tiles { template } if template.starts_with("https://maps.example.org/wms?")));
        // OGC API items links that are relative, or not web addresses.
        let json = r#"{"collections": [{"id": "a", "links": [{"rel": "items", "href": "collections/a/items"}]}, {"id": "b", "links": [{"rel": "items", "href": "file:///x"}]}]}"#;
        let s = parse("https://api.example.org/collections", json.as_bytes()).unwrap();
        assert_eq!(s.layers.len(), 1);
        assert!(
            matches!(&s.layers[0].offer, Offer::Features { url } if url == "https://api.example.org/collections/a/items?limit=10000&f=json"),
            "{:?}",
            s.layers[0].offer
        );
        assert!(s.notes.iter().any(|n| n.contains("1 collections")));
    }

    #[test]
    fn wfs_versions_and_axes() {
        // WFS 1.0: onlineResource, TYPENAME and MAXFEATURES.
        let v10 = r#"<WFS_Capabilities version="1.0.0"><Capability><Request><GetFeature><DCPType><HTTP><Get onlineResource="https://old.example.org/wfs?"/></HTTP></DCPType></GetFeature></Request></Capability>
<FeatureTypeList><FeatureType><Name>roads</Name><SRS>EPSG:4326</SRS></FeatureType></FeatureTypeList></WFS_Capabilities>"#;
        let s = parse("https://old.example.org/wfs", v10.as_bytes()).unwrap();
        assert!(
            matches!(&s.layers[0].offer, Offer::Features { url } if url.starts_with("https://old.example.org/wfs?SERVICE=WFS&VERSION=1.0.0&REQUEST=GetFeature&TYPENAME=roads&") && url.contains("SRSNAME=EPSG:4326&MAXFEATURES=")),
            "{:?}",
            s.layers[0].offer
        );
        // WFS 2.0 offering CRS84: asked for it, longitude first by definition.
        let v20 = r#"<WFS_Capabilities version="2.0.0"><FeatureTypeList><FeatureType><Name>a</Name><DefaultCRS>urn:ogc:def:crs:EPSG::25832</DefaultCRS><OtherCRS>urn:ogc:def:crs:OGC:1.3:CRS84</OtherCRS></FeatureType></FeatureTypeList></WFS_Capabilities>"#;
        let s = parse("https://x.org/wfs", v20.as_bytes()).unwrap();
        assert!(
            matches!(&s.layers[0].offer, Offer::Features { url } if url.contains("VERSION=2.0.0") && url.contains("TYPENAMES=a") && url.contains("SRSNAME=urn:ogc:def:crs:OGC:1.3:CRS84&COUNT=")),
            "{:?}",
            s.layers[0].offer
        );
        // An answer that says latitude comes first is turned round; others are left alone.
        let lat_lon = br#"{"type":"FeatureCollection","features":[{"type":"Feature","properties":{},"geometry":{"type":"LineString","coordinates":[[46.5,7.1],[47.0,8.2]]}},{"type":"Feature","properties":{},"geometry":{"type":"GeometryCollection","geometries":[{"type":"Point","coordinates":[45,6]}]}}],"crs":{"type":"name","properties":{"name":"urn:ogc:def:crs:EPSG::4326"}}}"#;
        let fixed: serde_json::Value = serde_json::from_slice(&fix_axes(lat_lon).unwrap()).unwrap();
        assert_eq!(fixed.pointer("/features/0/geometry/coordinates/0").unwrap(), &serde_json::json!([7.1, 46.5]));
        assert_eq!(fixed.pointer("/features/1/geometry/geometries/0/coordinates").unwrap(), &serde_json::json!([6, 45]));
        assert!(fixed.get("crs").is_none());
        assert!(
            fix_axes(br#"{"type":"FeatureCollection","features":[],"crs":{"type":"name","properties":{"name":"EPSG:4326"}}}"#).is_none()
        );
        assert!(fix_axes(br#"{"type":"FeatureCollection","features":[]}"#).is_none());
        assert!(fix_axes(b"urn:ogc:def:crs:EPSG::4326 not json").is_none());
    }

    #[test]
    fn partial_answers_pointed_out() {
        assert!(
            partial_note(br#"{"type":"FeatureCollection","features":[],"properties":{"exceededTransferLimit":true}}"#)
                .is_some_and(|n| n.contains("record limit"))
        );
        assert!(
            partial_note(br#"{"type":"FeatureCollection","numberMatched": 25000, "numberReturned":10000,"features":[]}"#)
                .is_some_and(|n| n.contains("10000 of its 25000"))
        );
        assert!(partial_note(br#"{"type":"FeatureCollection","features":[],"links":[{"rel":"next","href":"x"}]}"#).is_some());
        assert!(
            partial_note(
                br#"{"type":"FeatureCollection","numberMatched":3,"numberReturned":3,"features":[],"exceededTransferLimit":false}"#
            )
            .is_none()
        );
        assert!(partial_note(b"").is_none());
    }

    #[test]
    fn wms_layers_in_web_mercator() {
        let xml = r#"<?xml version="1.0"?><WMS_Capabilities version="1.3.0" xmlns="http://www.opengis.net/wms" xmlns:xlink="http://www.w3.org/1999/xlink">
<Service><Title>Demo WMS</Title></Service>
<Capability><Request><GetMap><Format>image/jpeg</Format><Format>image/png</Format><DCPType><HTTP><Get><OnlineResource xlink:href="https://maps.example.org/wms?map=demo"/></Get></HTTP></DCPType></GetMap></Request>
<Layer><Title>Root</Title><CRS>EPSG:4326</CRS><CRS>EPSG:3857</CRS>
  <Layer><Name>roads</Name><Title>Roads</Title><EX_GeographicBoundingBox><westBoundLongitude>-10</westBoundLongitude><eastBoundLongitude>20</eastBoundLongitude><southBoundLatitude>35</southBoundLatitude><northBoundLatitude>60</northBoundLatitude></EX_GeographicBoundingBox></Layer>
  <Layer><Name>water bodies</Name></Layer>
</Layer>
<Layer><Name>utm only</Name><CRS>EPSG:32633</CRS></Layer></Capability></WMS_Capabilities>"#;
        let s = parse("https://maps.example.org/wms", xml.as_bytes()).unwrap();
        assert_eq!((s.kind.as_str(), s.title.as_str()), ("WMS", "Demo WMS"));
        let names: Vec<&str> = s.layers.iter().map(|l| l.name.as_str()).collect();
        assert!(names.contains(&"roads") && names.contains(&"water bodies") && !names.contains(&"utm only"), "{names:?}");
        let roads = s.layers.iter().find(|l| l.name == "roads").unwrap();
        assert_eq!(roads.bbox, Some([-10.0, 35.0, 20.0, 60.0]));
        let Offer::Tiles { template } = &roads.offer else { panic!() };
        assert_eq!(
            template,
            "https://maps.example.org/wms?map=demo&SERVICE=WMS&VERSION=1.3.0&REQUEST=GetMap&LAYERS=roads&STYLES=&CRS=EPSG:3857&BBOX={bbox-epsg-3857}&WIDTH=256&HEIGHT=256&FORMAT=image%2Fpng&TRANSPARENT=TRUE"
        );
        let water = s.layers.iter().find(|l| l.name == "water bodies").unwrap();
        assert!(matches!(&water.offer, Offer::Tiles { template } if template.contains("LAYERS=water%20bodies")));
        assert!(s.notes.iter().any(|n| n.contains("1 layers")));
    }

    #[test]
    fn wmts_web_mercator_pyramids() {
        let mut matrices = String::new();
        for z in 0..=3 {
            let sd = Z0_SCALE / f64::from(1u32 << z);
            matrices.push_str(&format!("<TileMatrix><ows:Identifier>EPSG:3857:{z}</ows:Identifier><ScaleDenominator>{sd}</ScaleDenominator><TopLeftCorner>-20037508.3427892 20037508.3427892</TopLeftCorner><TileWidth>256</TileWidth><TileHeight>256</TileHeight></TileMatrix>"));
        }
        let xml = format!(
            r#"<Capabilities xmlns="http://www.opengis.net/wmts/1.0" xmlns:ows="http://www.opengis.net/ows/1.1" version="1.0.0">
<ows:ServiceIdentification><ows:Title>Tiles</ows:Title></ows:ServiceIdentification>
<Contents>
<Layer><ows:Title>Ortho</ows:Title><ows:Identifier>ortho</ows:Identifier><ows:WGS84BoundingBox><ows:LowerCorner>5 45</ows:LowerCorner><ows:UpperCorner>11 48</ows:UpperCorner></ows:WGS84BoundingBox>
 <Style isDefault="true"><ows:Identifier>normal</ows:Identifier></Style><Format>image/jpeg</Format><TileMatrixSetLink><TileMatrixSet>gm</TileMatrixSet></TileMatrixSetLink>
 <ResourceURL format="image/jpeg" resourceType="tile" template="https://tiles.example.org/{{Style}}/{{TileMatrixSet}}/{{TileMatrix}}/{{TileRow}}/{{TileCol}}.jpg"/></Layer>
<Layer><ows:Title>Swiss grid</ows:Title><ows:Identifier>lv95</ows:Identifier><TileMatrixSetLink><TileMatrixSet>2056</TileMatrixSet></TileMatrixSetLink></Layer>
<TileMatrixSet><ows:Identifier>gm</ows:Identifier><ows:SupportedCRS>urn:ogc:def:crs:EPSG::3857</ows:SupportedCRS>{matrices}</TileMatrixSet>
<TileMatrixSet><ows:Identifier>2056</ows:Identifier><ows:SupportedCRS>urn:ogc:def:crs:EPSG::2056</ows:SupportedCRS></TileMatrixSet>
</Contents></Capabilities>"#
        );
        let s = parse("https://tiles.example.org/WMTSCapabilities.xml", xml.as_bytes()).unwrap();
        assert_eq!(s.kind, "WMTS");
        assert_eq!(s.layers.len(), 1);
        let Offer::Tiles { template } = &s.layers[0].offer else { panic!() };
        assert_eq!(template, "https://tiles.example.org/normal/gm/EPSG:3857:{z}/{y}/{x}.jpg");
        assert_eq!(s.layers[0].bbox, Some([5.0, 45.0, 11.0, 48.0]));
        assert!(s.notes.iter().any(|n| n.contains("1 layers")));
    }

    #[test]
    fn wmts_512_pyramids_and_kept_queries() {
        // A 512 px Web Mercator pyramid: half the scale denominator of a 256 px one at each zoom.
        let mut matrices = String::new();
        for z in 0..=3 {
            let sd = Z0_SCALE / f64::from(1u32 << (z + 1));
            matrices.push_str(&format!("<TileMatrix><ows:Identifier>{z}</ows:Identifier><ScaleDenominator>{sd}</ScaleDenominator><TopLeftCorner>-20037508.3427892 20037508.3427892</TopLeftCorner><TileWidth>512</TileWidth><TileHeight>512</TileHeight></TileMatrix>"));
        }
        let xml = format!(
            r#"<Capabilities xmlns="http://www.opengis.net/wmts/1.0" xmlns:ows="http://www.opengis.net/ows/1.1" version="1.0.0">
<Contents><Layer><ows:Title>Big</ows:Title><ows:Identifier>big</ows:Identifier><Style isDefault="true"><ows:Identifier>s</ows:Identifier></Style><Format>image/png</Format><TileMatrixSetLink><TileMatrixSet>m512</TileMatrixSet></TileMatrixSetLink>
 <ResourceURL format="image/png" resourceType="tile" template="https://t.example.org/{{TileMatrix}}/{{TileRow}}/{{TileCol}}.png"/></Layer>
<TileMatrixSet><ows:Identifier>m512</ows:Identifier><ows:SupportedCRS>EPSG:3857</ows:SupportedCRS>{matrices}</TileMatrixSet></Contents></Capabilities>"#
        );
        let s = parse("https://t.example.org/WMTSCapabilities.xml", xml.as_bytes()).unwrap();
        assert_eq!(s.layers.len(), 1, "{:?}", s.notes);
        // MapServer's map= (and a token) stay; the request's own parameters go.
        assert_eq!(
            endpoint("https://m.example.org/cgi-bin/mapserv?map=/maps/demo.map&SERVICE=WMS&REQUEST=GetCapabilities&token=t"),
            "https://m.example.org/cgi-bin/mapserv?map=/maps/demo.map&token=t"
        );
        assert_eq!(endpoint("https://m.example.org/wms?service=WMS"), "https://m.example.org/wms");
        // An ArcGIS service's token goes with its requests.
        let fs = br#"{"layers": [{"id": 0, "name": "Parcels"}]}"#;
        let s = parse("https://gis.example.org/arcgis/rest/services/P/FeatureServer?token=abc", fs).unwrap();
        let Offer::Features { url } = &s.layers[0].offer else { panic!() };
        assert!(url.ends_with("&token=abc"), "{url}");
    }

    #[test]
    fn ogc_landing_fixture_discovers_json_and_resolves_items() {
        let address = "https://example.org/ogc/user/test-token?f=json";
        let service = parse(address, super::fixtures::fixture_landing().as_bytes()).unwrap();
        let catalog = service.collections_url.unwrap();
        assert_eq!(catalog, "https://example.org/ogc/user/test-token/catalog?f=json&key=fixture-key");
        let collections = parse(&catalog, super::fixtures::fixture_collections().as_bytes()).unwrap();
        assert_eq!(collections.layers.len(), 1);
        assert_eq!(collections.layers[0].title, "Places");
        assert_eq!(
            collections.layers[0].offer,
            Offer::Features { url: "https://example.org/ogc/user/test-token/collections/places/items?key=fixture-key&limit=10000".into() }
        );
        // The existing parser omits a point's zero-area bounds as a map-view extent.
        assert_eq!(collections.layers[0].bbox, None);
    }

    #[test]
    fn ogc_landing_links_are_resolved_without_inventing_paths_or_credentials() {
        let data_uri = "http://www.opengis.net/def/rel/ogc/1.0/data";
        for (href, expected) in [
            ("https://other.example/catalog?key=other", "https://other.example/catalog?key=other&f=json"),
            ("/catalog", "https://example.org/catalog?f=json"),
            ("catalog", "https://example.org/api/catalog?f=json"),
            ("./catalog", "https://example.org/api/catalog?f=json"),
            ("../catalog", "https://example.org/catalog?f=json"),
            ("?catalog=1", "https://example.org/api/root?catalog=1&f=json"),
        ] {
            let body = serde_json::json!({"links": [{"rel": "data", "href": href}]}).to_string();
            let result = parse("https://example.org/api/root?token=private&f=json", body.as_bytes()).unwrap();
            assert_eq!(result.collections_url.as_deref(), Some(expected));
        }
        let body = serde_json::json!({"links": [{"rel": data_uri, "href": "/catalog"}]}).to_string();
        let result = parse("https://example.org/api", body.as_bytes()).unwrap();
        assert_eq!(result.collections_url.as_deref(), Some("https://example.org/catalog?f=json"));
        for link in [
            serde_json::json!({"rel":"data", "href":"javascript:alert(1)"}),
            serde_json::json!({"rel":"data", "href":"file:///etc/passwd"}),
            serde_json::json!({"rel":"data", "href":"/catalog", "type":"text/html"}),
            serde_json::json!({"rel":"data", "href":""}),
            serde_json::json!({"rel":data_uri, "href":"/catalog", "type":"text/html"}),
        ] {
            let doc = serde_json::json!({"links":[link]}).to_string();
            assert!(parse("https://example.org/api", doc.as_bytes()).unwrap_err().contains("no usable JSON collections link"));
        }
        assert!(parse("https://example.org/api", br#"{"links":[]}"#).is_err());
        assert!(parse("https://example.org/api", br#"{"collections":[]}"#).unwrap().layers.is_empty());
        let chosen = parse(
            "https://example.org/api",
            br#"{"links":[
            {"rel":"data","href":"/untyped"},
            {"rel":"data","href":"/json","type":"application/json; charset=utf-8"}
        ]}"#,
        )
        .unwrap();
        assert_eq!(chosen.collections_url.as_deref(), Some("https://example.org/json"));
    }

    #[test]
    fn ogc_json_format_is_idempotent() {
        let url = capabilities_url("https://example.org/api?f=html&key=a%2Fb&f=json#fragment", Some("ogcapi"));
        assert_eq!(url, "https://example.org/api?f=json&key=a%2Fb");
        assert_eq!(capabilities_url(&url, Some("ogcapi")), url);
    }

    #[test]
    fn ogc_format_preserves_opaque_query_bytes_and_typed_links() {
        let raw = "https://example.org/api?token=%FF%2f%20&f=json&signature=a~b&flag";
        assert_eq!(json_url(raw), raw);
        assert_eq!(json_url("https://example.org/api?token=%FF%2f%20&%66=html&signature=a~b&F=json&flag"), raw);
        for href in ["/catalog?token=%FF%2f%20&signature=a~b&flag", "/catalog?f=application%2Fjson&token=%FF%2f%20"] {
            let body = serde_json::json!({"links":[{"rel":"data","type":"application/json","href":href}]}).to_string();
            let service = parse("https://example.org/api", body.as_bytes()).unwrap();
            assert_eq!(service.collections_url, Some(format!("https://example.org{href}")));
        }
    }

    #[test]
    fn ogc_api_tiles() {
        // A collection with features and vector tiles, one with map tiles only, and one whose
        // tiles link is no web address.
        let doc = br#"{"title": "Wine", "collections": [
            {"id": "vineyards", "title": "Vineyards", "extent": {"spatial": {"bbox": [[6, 49, 8, 50]]}}, "links": [
                {"rel": "http://www.opengis.net/def/rel/ogc/1.0/tilesets-vector", "href": "https://x.org/wine/collections/vineyards/tiles"},
                {"rel": "http://www.opengis.net/def/rel/ogc/1.0/tilesets-vector", "type": "text/html", "href": "https://x.org/wine/collections/vineyards/tiles?f=html"},
                {"rel": "items", "type": "application/geo+json", "href": "https://x.org/wine/collections/vineyards/items"}]},
            {"id": "relief", "links": [{"rel": "tilesets-map", "type": "application/json", "href": "collections/relief/map/tiles?f=json"}]},
            {"id": "odd", "links": [{"rel": "tilesets-map", "href": "javascript:alert(1)"}]}]}"#;
        let s = parse("https://x.org/wine/collections", doc).unwrap();
        assert_eq!(s.kind, "OGC API Features and Tiles");
        let vt = s.layers.iter().find(|l| l.name == "vineyards (vector tiles)").unwrap();
        assert_eq!(
            vt.offer,
            Offer::VectorTiles {
                template: "https://x.org/wine/collections/vineyards/tiles/WebMercatorQuad/{z}/{y}/{x}?f=mvt".into(),
                layers: vec!["vineyards".into()]
            }
        );
        assert_eq!(vt.bbox, Some([6.0, 49.0, 8.0, 50.0]));
        assert!(s.layers.iter().any(|l| l.name == "vineyards" && matches!(l.offer, Offer::Features { .. })));
        // Map tiles only: no features offered; the relative link is resolved.
        let relief: Vec<&ServiceLayer> = s.layers.iter().filter(|l| l.name.starts_with("relief")).collect();
        assert_eq!(relief.len(), 1);
        assert_eq!(
            relief[0].offer,
            Offer::Tiles { template: "https://x.org/wine/collections/relief/map/tiles/WebMercatorQuad/{z}/{y}/{x}".into() }
        );
        assert!(!s.layers.iter().any(|l| l.name.starts_with("odd (")), "a link that isn't a web address is left out");
        // A landing page with the dataset's map tiles.
        let landing = br#"{"title": "Earth at Night", "links": [
            {"rel": "http://www.opengis.net/def/rel/ogc/1.0/tilesets-map", "href": "https://x.org/night/map/tiles"}]}"#;
        let s = parse("https://x.org/night/", landing).unwrap();
        assert_eq!(s.kind, "OGC API Tiles");
        assert_eq!(s.layers[0].offer, Offer::Tiles { template: "https://x.org/night/map/tiles/WebMercatorQuad/{z}/{y}/{x}".into() });
        // A landing page advertises the collections document instead of rejecting it.
        assert_eq!(
            parse("https://x.org/a/", br#"{"links": [{"rel": "data", "href": "https://x.org/a/collections"}]}"#)
                .unwrap()
                .collections_url
                .as_deref(),
            Some("https://x.org/a/collections?f=json")
        );
        // The tileset list to read the advertised template from, with the link's query kept (but
        // its `f`), on the list and the usual-path template alike.
        assert_eq!(vt.tilesets.as_deref(), Some("https://x.org/wine/collections/vineyards/tiles?f=json"));
        let tokened = br#"{"collections": [{"id": "roads", "links": [
            {"rel": "tilesets-map", "href": "https://x.org/c/roads/map/tiles?token=abc&f=json"}]}]}"#;
        let s = parse("https://x.org/c", tokened).unwrap();
        assert_eq!(
            s.layers[0].offer,
            Offer::Tiles { template: "https://x.org/c/roads/map/tiles/WebMercatorQuad/{z}/{y}/{x}?token=abc".into() }
        );
        assert_eq!(s.layers[0].tilesets.as_deref(), Some("https://x.org/c/roads/map/tiles?token=abc&f=json"));
    }

    #[test]
    fn ogc_api_advertised_tile_templates() {
        let list = "https://x.org/c/roads/tiles?token=abc&f=json";
        // A list whose WebMercatorQuad tileset (named otherwise, as CubeWerx's `smerc`) points to
        // the tileset document; a tileset of another grid is passed over.
        let sets = br#"{"tilesets": [
            {"tileMatrixSetURI": "http://www.opengis.net/def/tilematrixset/OGC/1.0/WorldCRS84Quad", "links": [{"rel": "self", "href": "crs84"}]},
            {"dataType": "vector", "tileMatrixSetURI": "http://www.opengis.net/def/tilematrixset/OGC/1.0/WebMercatorQuad", "links": [{"rel": "self", "href": "https://x.org/c/roads/tiles/smerc"}]}]}"#;
        assert_eq!(
            resolve_tiles(list, list, sets, true),
            Some(TilesStep::Next("https://x.org/c/roads/tiles/smerc?token=abc&f=json".into()))
        );
        assert_eq!(resolve_tiles(list, list, sets, false), None, "no map tileset in WebMercatorQuad");
        // The tileset's item links: MVT for vector tiles, PNG first for map tiles, the token kept.
        let tileset = br#"{"links": [
            {"rel": "item", "type": "image/jpeg", "templated": true, "href": "https://x.org/c/roads/tiles/smerc/{tileMatrix}/{tileRow}/{tileCol}.jpg"},
            {"rel": "item", "type": "image/png", "templated": true, "href": "https://x.org/c/roads/tiles/smerc/{tileMatrix}/{tileRow}/{tileCol}.png"},
            {"rel": "item", "type": "application/vnd.mapbox-vector-tile", "templated": true, "href": "smerc/{tileMatrix}/{tileRow}/{tileCol}.mvt"}]}"#;
        let doc = "https://x.org/c/roads/tiles/smerc?token=abc&f=json";
        assert_eq!(
            resolve_tiles(list, doc, tileset, true),
            Some(TilesStep::Template("https://x.org/c/roads/tiles/smerc/{z}/{y}/{x}.mvt?token=abc".into()))
        );
        assert_eq!(
            resolve_tiles(list, doc, tileset, false),
            Some(TilesStep::Template("https://x.org/c/roads/tiles/smerc/{z}/{y}/{x}.png?token=abc".into()))
        );
        // A list that carries its tilesets' item links needs no second document.
        let inline = br#"{"tilesets": [{"tileMatrixSetId": "WebMercatorQuad", "links": [
            {"rel": "item", "type": "image/png", "href": "https://x.org/t/{tileMatrix}/{tileRow}/{tileCol}"}]}]}"#;
        assert_eq!(resolve_tiles(list, list, inline, false), Some(TilesStep::Template("https://x.org/t/{z}/{y}/{x}?token=abc".into())));
        // A template with other variables, or no web address, isn't taken.
        for bad in [
            br#"{"links": [{"rel": "item", "href": "https://x.org/{style}/{tileMatrix}/{tileRow}/{tileCol}"}]}"#.as_slice(),
            br#"{"links": [{"rel": "item", "href": "javascript:{tileMatrix}"}]}"#.as_slice(),
            b"not json",
        ] {
            assert_eq!(resolve_tiles(list, doc, bad, false), None);
        }
    }

    #[test]
    fn wfs_and_ogc_api_features() {
        let xml = r#"<wfs:WFS_Capabilities version="2.0.0" xmlns:wfs="http://www.opengis.net/wfs/2.0" xmlns:ows="http://www.opengis.net/ows/1.1" xmlns:xlink="http://www.w3.org/1999/xlink">
<ows:ServiceIdentification><ows:Title>Parcels</ows:Title></ows:ServiceIdentification>
<ows:OperationsMetadata><ows:Operation name="GetFeature"><ows:DCP><ows:HTTP><ows:Get xlink:href="https://gis.example.org/wfs?"/></ows:HTTP></ows:DCP></ows:Operation></ows:OperationsMetadata>
<wfs:FeatureTypeList><wfs:FeatureType><wfs:Name>ns:parcels</wfs:Name><wfs:Title>Parcels</wfs:Title><ows:WGS84BoundingBox><ows:LowerCorner>-1 2</ows:LowerCorner><ows:UpperCorner>3 4</ows:UpperCorner></ows:WGS84BoundingBox></wfs:FeatureType></wfs:FeatureTypeList></wfs:WFS_Capabilities>"#;
        let s = parse("https://gis.example.org/wfs", xml.as_bytes()).unwrap();
        assert_eq!(s.kind, "WFS");
        let Offer::Features { url } = &s.layers[0].offer else { panic!() };
        assert!(
            url.starts_with("https://gis.example.org/wfs?SERVICE=WFS&VERSION=2.0.0&REQUEST=GetFeature&TYPENAMES=ns:parcels&OUTPUTFORMAT=application%2Fjson&SRSNAME=EPSG:4326&COUNT="),
            "{url}"
        );
        assert_eq!(s.layers[0].bbox, Some([-1.0, 2.0, 3.0, 4.0]));
        // The JSON format the server names, or a note when it has none.
        let named = xml.replace("<ows:DCP>", r#"<ows:Parameter name="outputFormat"><ows:Value>text/xml; subtype=gml/3.1.1</ows:Value><ows:Value>application/json; subtype=geojson</ows:Value></ows:Parameter><ows:DCP>"#);
        let s = parse("https://gis.example.org/wfs", named.as_bytes()).unwrap();
        assert!(
            matches!(&s.layers[0].offer, Offer::Features { url } if url.contains("OUTPUTFORMAT=application%2Fjson%3B%20subtype%3Dgeojson"))
        );
        let gml_only = xml.replace(
            "<ows:DCP>",
            r#"<ows:Parameter name="outputFormat"><ows:Value>text/xml; subtype=gml/3.1.1</ows:Value></ows:Parameter><ows:DCP>"#,
        );
        let s = parse("https://gis.example.org/wfs", gml_only.as_bytes()).unwrap();
        assert!(s.layers.is_empty() && s.notes.iter().any(|n| n.contains("no GeoJSON")));

        let json = r#"{"title": "API", "collections": [
            {"id": "buildings", "title": "Buildings", "extent": {"spatial": {"bbox": [[7, 46, 8, 47]]}},
             "links": [{"rel": "items", "type": "application/geo+json", "href": "https://api.example.org/collections/buildings/items"}]},
            {"id": "roads"}]}"#;
        let s = parse("https://api.example.org/collections?f=json", json.as_bytes()).unwrap();
        assert_eq!(s.kind, "OGC API Features");
        assert_eq!(s.layers.len(), 2);
        assert!(
            matches!(&s.layers[0].offer, Offer::Features { url } if url == "https://api.example.org/collections/buildings/items?limit=10000")
        );
        assert!(
            matches!(&s.layers[1].offer, Offer::Features { url } if url == "https://api.example.org/collections/roads/items?limit=10000&f=json")
        );
        assert_eq!(s.layers[0].bbox, Some([7.0, 46.0, 8.0, 47.0]));
        assert!(parse("x", br#"{"links": [{"rel": "data", "href": "c"}]}"#).unwrap_err().contains("landing page"));
        assert!(parse("x", b"<ServiceExceptionReport>boom</ServiceExceptionReport>").unwrap_err().contains("boom"));
        assert!(parse("x", b"<html/>").is_err());
        assert!(parse("x", &[0xff]).is_err());
    }

    #[test]
    fn wcs_coverages() {
        let v2 = r#"<wcs:Capabilities version="2.0.1" xmlns:wcs="http://www.opengis.net/wcs/2.0" xmlns:ows="http://www.opengis.net/ows/2.0" xmlns:xlink="http://www.w3.org/1999/xlink">
            <ows:ServiceIdentification><ows:Title>Soils</ows:Title></ows:ServiceIdentification>
            <ows:OperationsMetadata><ows:Operation name="GetCoverage"><ows:DCP><ows:HTTP>
              <ows:Get xlink:href="https://maps.example.org/mapserv?map=/m/soc.map&amp;amp;"/></ows:HTTP></ows:DCP></ows:Operation></ows:OperationsMetadata>
            <wcs:ServiceMetadata><wcs:formatSupported>image/png</wcs:formatSupported><wcs:formatSupported>image/tiff</wcs:formatSupported></wcs:ServiceMetadata>
            <wcs:Contents><wcs:CoverageSummary><wcs:CoverageId>soc_mean</wcs:CoverageId>
              <ows:WGS84BoundingBox><ows:LowerCorner>-180 -56</ows:LowerCorner><ows:UpperCorner>180 84</ows:UpperCorner></ows:WGS84BoundingBox>
            </wcs:CoverageSummary></wcs:Contents></wcs:Capabilities>"#;
        let s = parse("https://maps.example.org/mapserv?map=/m/soc.map&SERVICE=WCS&REQUEST=GetCapabilities", v2.as_bytes()).unwrap();
        assert_eq!((s.kind.as_str(), s.title.as_str()), ("WCS", "Soils"));
        assert_eq!(s.layers[0].bbox, Some([-180.0, -56.0, 180.0, 84.0]));
        let Offer::Coverage(c) = &s.layers[0].offer else { panic!("{:?}", s.layers[0].offer) };
        assert_eq!(c.endpoint, "https://maps.example.org/mapserv?map=/m/soc.map&");
        assert_eq!((c.version.as_str(), c.id.as_str()), ("2.0.1", "soc_mean"));
        assert_eq!(
            describe_coverage_url(c),
            "https://maps.example.org/mapserv?map=/m/soc.map&SERVICE=WCS&VERSION=2.0.1&REQUEST=DescribeCoverage&COVERAGEID=soc_mean"
        );
        let desc = r#"<wcs:CoverageDescriptions xmlns:wcs="http://www.opengis.net/wcs/2.0" xmlns:gml="http://www.opengis.net/gml/3.2">
            <wcs:CoverageDescription><gml:boundedBy><gml:Envelope srsName="http://www.opengis.net/def/crs/EPSG/0/4326" axisLabels="lat long">
              <gml:lowerCorner>-56 -180</gml:lowerCorner><gml:upperCorner>84 180</gml:upperCorner></gml:Envelope></gml:boundedBy>
            <wcs:ServiceParameters><wcs:nativeFormat>image/tiff</wcs:nativeFormat></wcs:ServiceParameters></wcs:CoverageDescription></wcs:CoverageDescriptions>"#;
        let info = parse_describe(c, desc.as_bytes()).unwrap();
        assert_eq!(info.axes, Some(("long".into(), "lat".into())));
        assert_eq!(info.bbox, Some([-180.0, -56.0, 180.0, 84.0]));
        assert_eq!(info.format, "image/tiff");
        let area = coverage_area(&info, [170.0, 80.0, 190.0, 90.0]).unwrap();
        assert_eq!(area, [170.0, 80.0, 180.0, 84.0]);
        assert_eq!(coverage_size(area), (2048, 819));
        assert_eq!(coverage_size([0.0, 0.0, 1.0, 4.0]), (512, 2048));
        assert!(coverage_area(&info, [0.0, 85.0, 1.0, 89.0]).is_none());
        let get = get_coverage_url(c, &info, [7.0, 46.0, 8.0, 47.0]);
        assert!(
            get.contains("COVERAGEID=soc_mean&FORMAT=image%2Ftiff&")
                && get.contains("&SUBSET=long(7,8)&SUBSET=lat(46,47)&SCALESIZE=long(2048),lat(2048)"),
            "{get}"
        );
        assert!(
            parse_describe(c, b"<ows:ExceptionReport xmlns:ows='o'>no such coverage</ows:ExceptionReport>")
                .unwrap_err()
                .contains("no such coverage")
        );
        assert!(parse_describe(c, b"<CoverageDescriptions/>").is_err());
        // No axis labels: EPSG:4326's corners are latitude first.
        let unlabelled = desc.replace(r#" axisLabels="lat long""#, "");
        assert_eq!(parse_describe(c, unlabelled.as_bytes()).unwrap().bbox, Some([-180.0, -56.0, 180.0, 84.0]));

        let v1 = r#"<WCS_Capabilities version="1.0.0" xmlns="http://www.opengis.net/wcs" xmlns:gml="http://www.opengis.net/gml" xmlns:xlink="http://www.w3.org/1999/xlink">
            <Service><name>MapServer WCS</name><label>Elevation</label></Service>
            <Capability><Request><GetCoverage><DCPType><HTTP><Get><OnlineResource xlink:href="https://dem.example.org/wcs?"/></Get></HTTP></DCPType></GetCoverage></Request></Capability>
            <ContentMetadata><CoverageOfferingBrief><name>dem</name><label>Terrain</label>
              <lonLatEnvelope srsName="urn:ogc:def:crs:OGC:1.3:CRS84"><gml:pos>5 45</gml:pos><gml:pos>11 48</gml:pos></lonLatEnvelope>
            </CoverageOfferingBrief></ContentMetadata></WCS_Capabilities>"#;
        let s = parse("https://dem.example.org/wcs", v1.as_bytes()).unwrap();
        assert_eq!((s.layers[0].title.as_str(), s.layers[0].bbox), ("Terrain", Some([5.0, 45.0, 11.0, 48.0])));
        let Offer::Coverage(c) = &s.layers[0].offer else { panic!() };
        assert_eq!(c.version, "1.0.0");
        let desc = r#"<CoverageDescription xmlns="http://www.opengis.net/wcs" xmlns:gml="http://www.opengis.net/gml"><CoverageOffering><name>dem</name>
            <lonLatEnvelope><gml:pos>5 45</gml:pos><gml:pos>11 48</gml:pos></lonLatEnvelope>
            <supportedFormats><formats>PNG</formats><formats>GEOTIFF_INT16</formats></supportedFormats></CoverageOffering></CoverageDescription>"#;
        let info = parse_describe(c, desc.as_bytes()).unwrap();
        assert_eq!((info.format.as_str(), info.bbox), ("GEOTIFF_INT16", Some([5.0, 45.0, 11.0, 48.0])));
        assert_eq!(
            get_coverage_url(c, &info, [6.0, 46.0, 7.0, 46.5]),
            "https://dem.example.org/wcs?SERVICE=WCS&VERSION=1.0.0&REQUEST=GetCoverage&COVERAGE=dem&CRS=EPSG:4326&BBOX=6,46,7,46.5&WIDTH=2048&HEIGHT=1024&FORMAT=GEOTIFF_INT16"
        );
        assert!(parse_describe(c, b"<CoverageDescription/>").is_err());

        let v11 = r#"<Capabilities version="1.1.1" xmlns="http://www.opengis.net/wcs/1.1.1" xmlns:ows="http://www.opengis.net/ows/1.1">
            <Contents><CoverageSummary><Identifier>ndvi</Identifier></CoverageSummary></Contents></Capabilities>"#;
        let s = parse("https://x.org/wcs", v11.as_bytes()).unwrap();
        assert_eq!(s.kind, "WCS");
        assert!(matches!(&s.layers[0].offer, Offer::Coverage(c) if c.version == "1.0.0" && c.endpoint == "https://x.org/wcs"));
        assert!(!s.notes.is_empty());
        assert_eq!(
            capabilities_url("https://x.org/geoserver/wcs", None),
            "https://x.org/geoserver/wcs?SERVICE=WCS&REQUEST=GetCapabilities"
        );

        assert_eq!(lonlat_axes(&["Lat", "Long"], ""), Some(("Long".into(), "Lat".into())));
        assert_eq!(lonlat_axes(&["x", "y"], "EPSG:4326"), Some(("x".into(), "y".into())));
        assert_eq!(lonlat_axes(&["a", "b"], "http://www.opengis.net/def/crs/EPSG/0/4326"), Some(("b".into(), "a".into())));
        assert_eq!(lonlat_axes(&["x(1)", "y"], ""), None);
        assert_eq!(lonlat_axes(&["x"], ""), None);
    }

    #[test]
    fn csw_records() {
        let u = records_url(
            "https://cat.example.org/csw?SERVICE=CSW&REQUEST=GetCapabilities",
            "O'Brien\u{7}",
            Some([7.0, 46.0, 8.0, 47.0]),
            21,
        );
        assert!(u.starts_with("https://cat.example.org/csw?SERVICE=CSW&VERSION=2.0.2&REQUEST=GetRecords&"), "{u}");
        assert!(u.contains("&startPosition=21&") && u.contains("O%27%27Brien") && u.contains("BBOX%28ows:BoundingBox,7,46,8,47%29"), "{u}");
        assert!(!records_url("https://cat.example.org/csw", " ", None, 0).contains("constraint="));

        let xml = r#"<csw:GetRecordsResponse xmlns:csw="http://www.opengis.net/cat/csw/2.0.2" xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:dct="http://purl.org/dc/terms/" xmlns:ows="http://www.opengis.net/ows">
            <csw:SearchResults numberOfRecordsMatched="45" numberOfRecordsReturned="2" nextRecord="3">
              <csw:Record><dc:identifier>a1</dc:identifier><dc:title>Forests
                Wälder</dc:title><dct:abstract>Forest cover.</dct:abstract>
                <dc:URI protocol="OGC:WMS" name="forests">https://geo.example.org/wms?SERVICE=WMS&amp;REQUEST=GetCapabilities</dc:URI>
                <dc:URI protocol="WWW:DOWNLOAD-1.0-http--download">https://geo.example.org/forests.zip</dc:URI>
                <dc:URI protocol="WWW:DOWNLOAD-1.0-http--download">https://geo.example.org/forests.zip</dc:URI>
                <dct:references scheme="ESRI:REST">https://x.org/arcgis/rest/services/F/MapServer</dct:references>
                <dc:URI>/files/doc.pdf</dc:URI>
                <dc:URI>https://geo.example.org/geoserver/wfs</dc:URI>
                <ows:BoundingBox crs="urn:ogc:def:crs:EPSG:6.6:4326"><ows:LowerCorner>45.8 5.9</ows:LowerCorner><ows:UpperCorner>47.8 10.5</ows:UpperCorner></ows:BoundingBox>
              </csw:Record>
              <csw:Record><dc:title>Lakes</dc:title>
                <ows:WGS84BoundingBox><ows:LowerCorner>6 46</ows:LowerCorner><ows:UpperCorner>7 47</ows:UpperCorner></ows:WGS84BoundingBox></csw:Record>
            </csw:SearchResults></csw:GetRecordsResponse>"#;
        let page = parse_records("https://cat.example.org/csw?x=1", xml.as_bytes()).unwrap();
        assert_eq!((page.matched, page.next, page.records.len()), (45, Some(3), 2));
        let r = &page.records[0];
        assert_eq!((r.id.as_str(), r.title.as_str(), r.summary.as_str()), ("a1", "Forests", "Forest cover."));
        assert_eq!(r.bbox, Some([5.9, 45.8, 10.5, 47.8]));
        let kinds: Vec<&str> = r.links.iter().map(|l| l.kind).collect();
        assert_eq!(kinds, ["wms", "download", "arcgis", "web", "wfs"]);
        assert_eq!(r.links[0].name.as_deref(), Some("forests"));
        assert_eq!(r.links[3].url, "https://cat.example.org/files/doc.pdf");
        assert_eq!((page.records[1].title.as_str(), page.records[1].bbox), ("Lakes", Some([6.0, 46.0, 7.0, 47.0])));

        let last = xml.replace(r#"nextRecord="3""#, r#"nextRecord="0""#);
        assert_eq!(parse_records("https://c.org/csw", last.as_bytes()).unwrap().next, None);
        assert!(
            parse_records("https://c.org/csw", b"<ows:ExceptionReport xmlns:ows='o'>bad CQL</ows:ExceptionReport>")
                .unwrap_err()
                .contains("bad CQL")
        );
        assert!(parse_records("https://c.org/csw", b"<WMS_Capabilities/>").is_err());
        assert!(parse_records("https://c.org/csw", &[0xff, 0xfe]).is_err());
        let deep = format!("<GetRecordsResponse>{}{}</GetRecordsResponse>", "<a>".repeat(5000), "</a>".repeat(5000));
        assert!(
            parse("https://c.org/wms", deep.replace("GetRecordsResponse", "WMS_Capabilities").as_bytes()).unwrap_err().contains("deep")
        );
        assert!(parse_records("https://c.org/csw", deep.as_bytes()).unwrap_err().contains("deep"));
        let many = format!("<GetRecordsResponse>{}</GetRecordsResponse>", "<Record><title>t</title></Record>".repeat(1000));
        assert_eq!(parse_records("https://c.org/csw", many.as_bytes()).unwrap().records.len(), RECORDS_PAGE as usize * 5);
    }
}
