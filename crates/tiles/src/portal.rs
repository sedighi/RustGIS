//! Open-data portals searched by text: CKAN catalogues (data.gov's own catalogue, and any CKAN
//! site such as open.canada.ca), Socrata's discovery API (every Socrata site, or one) and ArcGIS
//! Hub. A search names datasets and the geospatial resources each offers: files RustGIS reads by
//! address (GeoJSON, KML, zipped Shapefiles, GeoParquet…) and services it adds (WMS, WFS, ArcGIS
//! REST). Requests are built and answers read here; the fetching is the caller's.

use serde_json::Value;

/// The portals searched when none is named.
pub const DATA_GOV: &str = "https://catalog.data.gov";
pub const SOCRATA: &str = "https://api.us.socrata.com";
pub const HUB: &str = "https://hub.arcgis.com";

/// How many datasets a search asks for at most.
pub const MAX_RESULTS: usize = 50;
/// How many resources a dataset lists at most.
const MAX_RESOURCES: usize = 20;
/// The longest query sent.
const MAX_QUERY: usize = 200;

/// The kind of portal: its search request and its answer's form.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A CKAN site's `package_search`.
    Ckan,
    /// data.gov's catalogue (its own search, DCAT records).
    DataGov,
    /// Socrata's discovery API (every site, or one site's datasets).
    Socrata,
    /// ArcGIS Hub's search (OGC API Records).
    Hub,
}

/// A portal: its kind, its address, and (Socrata) the site searched.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Portal {
    pub kind: Kind,
    /// The address searched (no path for Socrata's discovery and Hub; a CKAN site's root).
    pub base: String,
    /// A Socrata site the search keeps to (`data.cityofnewyork.us`).
    pub domain: Option<String>,
}

/// What a resource is to RustGIS: a file read by address, or a service of a type.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Use {
    File,
    /// `wms`, `wfs` or `arcgis`, and the layer an address names (an ArcGIS layer's number).
    Service {
        kind: &'static str,
        layer: Option<String>,
    },
}

/// A dataset's resource RustGIS can add.
#[derive(Clone, Debug, PartialEq)]
pub struct Resource {
    /// What it is, as people say it (`GeoJSON`, `Shapefile`, `ArcGIS REST`…).
    pub format: String,
    pub url: String,
    pub using: Use,
}

/// A dataset found.
#[derive(Clone, Debug, PartialEq)]
pub struct Dataset {
    pub id: String,
    pub title: String,
    /// Its description's start.
    pub description: String,
    pub organization: String,
    /// Its page on the portal.
    pub page: Option<String>,
    /// Its extent (west, south, east, north), when the portal says.
    pub bbox: Option<[f64; 4]>,
    pub resources: Vec<Resource>,
}

/// A host name: letters, digits, dots and dashes.
fn host_ok(h: &str) -> bool {
    !h.is_empty() && h.len() <= 253 && h.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')
}

/// The host of an address (or a bare host).
fn host_of(url: &str) -> &str {
    let rest = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
    rest.split(['/', '?', '#']).next().unwrap_or(rest)
}

impl Portal {
    /// A portal by kind (`ckan`, the default, `socrata` or `arcgis-hub`) and an optional address:
    /// a CKAN site (data.gov's catalogue by default), a Socrata site (`data.cityofnewyork.us`) or
    /// discovery API, an ArcGIS Hub site.
    pub fn new(kind: Option<&str>, url: Option<&str>) -> Result<Portal, String> {
        let url = url.map(str::trim).filter(|u| !u.is_empty());
        let web = |u: &str| -> Result<String, String> {
            let u = if u.contains("://") { u.to_string() } else { format!("https://{u}") };
            crate::address::check(&u, None)?;
            Ok(u)
        };
        match kind.map(|k| k.trim().to_ascii_lowercase()).as_deref() {
            None | Some("") | Some("ckan") | Some("datagov") | Some("data.gov") => {
                let base = match url {
                    Some(u) => web(u)?,
                    None => DATA_GOV.to_string(),
                };
                // The site's root: a pasted search address keeps only it.
                let base = base.split(['?', '#']).next().unwrap_or(&base).trim_end_matches('/').to_string();
                let base = base.split("/api/").next().unwrap_or(&base).to_string();
                let kind = if host_of(&base).eq_ignore_ascii_case("catalog.data.gov") { Kind::DataGov } else { Kind::Ckan };
                Ok(Portal { kind, base, domain: None })
            }
            Some("socrata") => {
                let (base, domain) = match url {
                    None => (SOCRATA.to_string(), None),
                    Some(u) => {
                        let host = host_of(u).to_ascii_lowercase();
                        if !host_ok(&host) {
                            return Err(format!("'{u}': a Socrata site is its domain, such as data.cityofnewyork.us"));
                        }
                        web(&host)?;
                        // A discovery API itself searches every site; a site keeps the search to it.
                        if host.starts_with("api.") && host.ends_with(".socrata.com") {
                            (format!("https://{host}"), None)
                        } else {
                            (SOCRATA.to_string(), Some(host))
                        }
                    }
                };
                Ok(Portal { kind: Kind::Socrata, base, domain })
            }
            Some("arcgis-hub") | Some("hub") | Some("arcgis") => {
                let base = match url {
                    Some(u) => {
                        let host = host_of(u).to_ascii_lowercase();
                        if !host_ok(&host) {
                            return Err(format!("'{u}': an ArcGIS Hub site is its address, such as hub.arcgis.com"));
                        }
                        web(&host)?
                    }
                    None => HUB.to_string(),
                };
                Ok(Portal { kind: Kind::Hub, base, domain: None })
            }
            Some(other) => Err(format!("unknown portal '{other}' (ckan, socrata or arcgis-hub)")),
        }
    }

    /// What the portal is called in messages.
    pub fn label(&self) -> String {
        match (&self.kind, &self.domain) {
            (Kind::Socrata, Some(d)) => d.clone(),
            _ => host_of(&self.base).to_string(),
        }
    }

    /// The search for `query`, up to `limit` datasets, in `bbox` (west, south, east, north) when
    /// given and the portal filters by place (CKAN with its spatial extension, Hub; the others'
    /// answers are kept to it after).
    pub fn search_url(&self, query: &str, bbox: Option<[f64; 4]>, limit: usize) -> String {
        let q: String = query.trim().chars().take(MAX_QUERY).collect();
        let q = crate::s3::encode(&q, true);
        let limit = limit.clamp(1, MAX_RESULTS);
        let bb = |b: [f64; 4]| format!("{},{},{},{}", b[0], b[1], b[2], b[3]);
        match self.kind {
            Kind::Ckan => {
                let mut u = format!("{}/api/3/action/package_search?q={q}&rows={limit}", self.base);
                if let Some(b) = bbox {
                    u.push_str(&format!("&ext_bbox={}", bb(b)));
                }
                u
            }
            Kind::DataGov => format!("{}/search?q={q}&per_page={limit}", self.base),
            Kind::Socrata => {
                let mut u = format!("{}/api/catalog/v1?q={q}&only=dataset&limit={limit}", self.base);
                if let Some(d) = &self.domain {
                    u.push_str(&format!("&domains={d}"));
                }
                u
            }
            Kind::Hub => {
                let mut u = format!("{}/api/search/v1/collections/dataset/items?q={q}&type=Feature%20Service&limit={limit}", self.base);
                if let Some(b) = bbox {
                    u.push_str(&format!("&bbox={}", bb(b)));
                }
                u
            }
        }
    }

    /// The datasets in a search's answer (`from`: the search's address, which the resources' own
    /// addresses are judged against), with the resources RustGIS can add; datasets without one are
    /// left out, as are those outside `bbox` when they say where they are.
    pub fn parse(&self, from: &str, bytes: &[u8], bbox: Option<[f64; 4]>) -> Result<Vec<Dataset>, String> {
        let v: Value = serde_json::from_slice(bytes).map_err(|_| format!("{}: the answer isn't JSON", self.label()))?;
        let mut out = match self.kind {
            Kind::Ckan => {
                if v.get("success").and_then(Value::as_bool) == Some(false) {
                    return Err(format!("{}: {}", self.label(), error_text(&v)));
                }
                let r = v.pointer("/result/results").and_then(Value::as_array).ok_or_else(|| self.not_portal())?;
                r.iter().take(MAX_RESULTS).filter_map(|d| ckan_dataset(&self.base, d)).collect::<Vec<_>>()
            }
            Kind::DataGov => {
                let r = v.get("results").and_then(Value::as_array).ok_or_else(|| self.not_portal())?;
                r.iter().take(MAX_RESULTS).filter_map(|d| datagov_dataset(&self.base, d)).collect()
            }
            Kind::Socrata => {
                let r = v.get("results").and_then(Value::as_array).ok_or_else(|| self.not_portal())?;
                r.iter().take(MAX_RESULTS).filter_map(socrata_dataset).collect()
            }
            Kind::Hub => {
                let r = v.get("features").and_then(Value::as_array).ok_or_else(|| self.not_portal())?;
                r.iter().take(MAX_RESULTS).filter_map(|d| hub_dataset(&self.base, d)).collect()
            }
        };
        for d in &mut out {
            // The portal's answer is hostile input: an address it gives may not reach into the
            // local network (unless the portal is there) or a forbidden address.
            d.resources.retain(|r| crate::address::check(&r.url, Some(from)).is_ok());
            d.resources.sort_by_key(|r| rank(&r.format));
            d.resources.truncate(MAX_RESOURCES);
        }
        out.retain(|d| !d.resources.is_empty() && bbox.is_none_or(|b| d.bbox.is_none_or(|e| overlaps(b, e))));
        Ok(out)
    }

    fn not_portal(&self) -> String {
        match self.kind {
            Kind::Ckan => format!("{}: not a CKAN catalogue (its package_search answered something else)", self.label()),
            Kind::DataGov => format!("{}: the catalogue's answer has no results", self.label()),
            Kind::Socrata => format!("{}: not a Socrata discovery answer", self.label()),
            Kind::Hub => format!("{}: not an ArcGIS Hub search answer", self.label()),
        }
    }
}

/// A CKAN error's message.
fn error_text(v: &Value) -> String {
    v.pointer("/error/message").and_then(Value::as_str).unwrap_or("the search failed").chars().take(200).collect()
}

fn text(v: &Value, k: &str, max: usize) -> String {
    v.get(k).and_then(Value::as_str).map(|s| s.trim().chars().take(max).collect()).unwrap_or_default()
}

/// A description's start, on one line, without its HTML tags and common entities.
fn snippet(s: &str) -> String {
    let mut plain = String::with_capacity(s.len());
    let mut tag = false;
    for c in s.chars() {
        match c {
            '<' => tag = true,
            '>' if tag => {
                tag = false;
                plain.push(' ');
            }
            _ if !tag => plain.push(c),
            _ => {}
        }
    }
    let plain = plain
        .replace("&nbsp;", " ")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&");
    let one: String = plain.split_whitespace().collect::<Vec<_>>().join(" ");
    if one.chars().count() > 240 { format!("{}…", one.chars().take(240).collect::<String>()) } else { one }
}

/// The order resources are offered in: what reads best first.
fn rank(format: &str) -> usize {
    ["GeoJSON", "FlatGeobuf", "GeoParquet", "GeoPackage", "KML", "KMZ", "Shapefile", "GeoTIFF", "ArcGIS REST", "WMS", "WFS", "CSV"]
        .iter()
        .position(|f| *f == format)
        .unwrap_or(usize::MAX)
}

/// What a resource is, from its format label, media type and address; `None` for what RustGIS
/// doesn't add (web pages, PDFs, plain JSON…).
pub fn classify(format: &str, media: &str, url: &str) -> Option<Resource> {
    let url = url.trim();
    let lower_url = url.to_ascii_lowercase();
    if !(lower_url.starts_with("https://") || lower_url.starts_with("http://")) || url.len() > 2048 {
        return None;
    }
    let f = format.trim().to_ascii_lowercase();
    let m = media.trim().to_ascii_lowercase();
    let path = lower_url.split(['?', '#']).next().unwrap_or(&lower_url);
    let ext = path.rsplit('/').next().and_then(|n| n.rsplit_once('.')).map(|(_, e)| e).unwrap_or("");
    let file = |name: &str| Some(Resource { format: name.into(), url: url.to_string(), using: Use::File });
    let service = |name: &str, kind: &'static str, layer: Option<String>| {
        Some(Resource { format: name.into(), url: url.to_string(), using: Use::Service { kind, layer } })
    };
    // Services first: their addresses often end in no extension, and their labels are many.
    if f.contains("arcgis") || f.contains("esri rest") || path.contains("/rest/services/") {
        // `…/FeatureServer` or `…/MapServer`, and a layer's number after it.
        let parts: Vec<&str> = url.split(['?', '#']).next().unwrap_or(url).trim_end_matches('/').rsplitn(2, '/').collect();
        let (layer, root) = match parts.as_slice() {
            [last, root] if last.bytes().all(|b| b.is_ascii_digit()) && !last.is_empty() => (Some(last.to_string()), *root),
            _ => (None, url.split(['?', '#']).next().unwrap_or(url).trim_end_matches('/')),
        };
        let r = root.to_ascii_lowercase();
        if r.ends_with("/featureserver") || r.ends_with("/mapserver") {
            return Some(Resource { format: "ArcGIS REST".into(), url: root.to_string(), using: Use::Service { kind: "arcgis", layer } });
        }
        return None;
    }
    if f == "wms" || f.contains("web map service") || lower_url.contains("service=wms") {
        return service("WMS", "wms", None);
    }
    if f == "wfs" || f.contains("web feature service") || lower_url.contains("service=wfs") {
        return service("WFS", "wfs", None);
    }
    match () {
        _ if f == "geojson" || m.contains("geo+json") || m.contains("vnd.geo+json") || ext == "geojson" => file("GeoJSON"),
        _ if f == "kmz" || m.contains("kmz") || ext == "kmz" => file("KMZ"),
        _ if f == "kml" || m.contains("google-earth.kml") || ext == "kml" => file("KML"),
        _ if f.contains("shapefile")
            || f == "shp"
            || (ext == "zip" && (f.contains("shp") || lower_url.contains("shapefile")))
            || (f == "zip" && lower_url.contains("shapefile")) =>
        {
            file("Shapefile")
        }
        _ if f.contains("geoparquet") || f == "parquet" || ext == "parquet" => file("GeoParquet"),
        _ if f.contains("flatgeobuf") || ext == "fgb" => file("FlatGeobuf"),
        _ if f.contains("geopackage") || f == "gpkg" || ext == "gpkg" => file("GeoPackage"),
        _ if f == "geotiff" || f == "tiff" || f == "tif" || m.contains("image/tiff") || ext == "tif" || ext == "tiff" => file("GeoTIFF"),
        // A zip of unknown content: often a Shapefile, read as one (the reader says if it isn't).
        _ if f == "zip" || m.contains("application/zip") || ext == "zip" => file("Shapefile"),
        _ if f == "csv" || m.contains("text/csv") || ext == "csv" => file("CSV"),
        _ => None,
    }
}

/// A box of a GeoJSON geometry's coordinates (bounded walk), in longitude/latitude.
fn bbox_of(g: &Value) -> Option<[f64; 4]> {
    fn walk(v: &Value, depth: usize, b: &mut [f64; 4], n: &mut usize) {
        if depth > 6 || *n > 100_000 {
            return;
        }
        if let Some(a) = v.as_array() {
            if let (Some(x), Some(y)) = (a.first().and_then(Value::as_f64), a.get(1).and_then(Value::as_f64)) {
                if (-180.0..=180.0).contains(&x) && (-90.0..=90.0).contains(&y) {
                    *b = [b[0].min(x), b[1].min(y), b[2].max(x), b[3].max(y)];
                }
                *n += 1;
                return;
            }
            for c in a {
                walk(c, depth + 1, b, n);
            }
        }
    }
    let mut b = [f64::MAX, f64::MAX, f64::MIN, f64::MIN];
    let mut n = 0;
    walk(g.get("coordinates")?, 0, &mut b, &mut n);
    (b[0] <= b[2] && b[1] <= b[3]).then_some(b)
}

/// Whether two boxes meet (a box crossing the antimeridian, west past east, as two).
fn overlaps(a: [f64; 4], b: [f64; 4]) -> bool {
    let spans = |w: f64, e: f64| if w <= e { vec![(w, e)] } else { vec![(w, 180.0), (-180.0, e)] };
    a[1] <= b[3] && b[1] <= a[3] && spans(a[0], a[2]).iter().any(|(aw, ae)| spans(b[0], b[2]).iter().any(|(bw, be)| aw <= be && bw <= ae))
}

fn ckan_dataset(base: &str, d: &Value) -> Option<Dataset> {
    let resources: Vec<Resource> = d
        .get("resources")?
        .as_array()?
        .iter()
        .take(200)
        .filter_map(|r| classify(&text(r, "format", 80), &text(r, "mimetype", 120), &text(r, "url", 2048)))
        .collect();
    let name = text(d, "name", 200);
    let bbox = d
        .get("extras")
        .and_then(Value::as_array)
        .and_then(|e| e.iter().take(100).find(|x| x.get("key").and_then(Value::as_str) == Some("spatial")))
        .and_then(|x| x.get("value")?.as_str())
        .filter(|s| s.len() < 1 << 20)
        .and_then(|s| serde_json::from_str::<Value>(s).ok())
        .and_then(|g| bbox_of(&g));
    Some(Dataset {
        id: if name.is_empty() { text(d, "id", 200) } else { name.clone() },
        title: text(d, "title", 300),
        description: snippet(&text(d, "notes", 4000)),
        organization: d.get("organization").map(|o| text(o, "title", 200)).unwrap_or_default(),
        page: (!name.is_empty()).then(|| format!("{base}/dataset/{name}")),
        bbox,
        resources,
    })
}

fn datagov_dataset(base: &str, d: &Value) -> Option<Dataset> {
    let dcat = d.get("dcat")?;
    let resources: Vec<Resource> = dcat
        .get("distribution")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .take(200)
        .filter_map(|r| {
            let url = r.get("downloadURL").or_else(|| r.get("accessURL")).and_then(Value::as_str).unwrap_or("");
            classify(&text(r, "format", 80), &text(r, "mediaType", 120), url)
        })
        .collect();
    let slug = text(d, "slug", 200);
    let title = Some(text(d, "title", 300)).filter(|t| !t.is_empty()).unwrap_or_else(|| text(dcat, "title", 300));
    let description = Some(text(d, "description", 4000)).filter(|t| !t.is_empty()).unwrap_or_else(|| text(dcat, "description", 4000));
    let organization =
        d.get("organization").map(|o| text(o, "name", 200)).filter(|o| !o.is_empty()).unwrap_or_else(|| text(d, "publisher", 200));
    Some(Dataset {
        id: if slug.is_empty() { text(d, "identifier", 300) } else { slug.clone() },
        title,
        description: snippet(&description),
        organization,
        page: (!slug.is_empty()).then(|| format!("{base}/dataset/{slug}")),
        bbox: d.get("spatial_shape").and_then(bbox_of),
        resources,
    })
}

/// The geometry column types Socrata names.
const SOCRATA_GEO: [&str; 7] = ["point", "multipoint", "line", "multiline", "polygon", "multipolygon", "location"];

fn socrata_dataset(r: &Value) -> Option<Dataset> {
    let res = r.get("resource")?;
    let id = text(res, "id", 20);
    // A dataset (four-by-four id) with a geometry column: its rows as GeoJSON.
    let is_id = id.len() == 9 && id.as_bytes().get(4) == Some(&b'-') && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-');
    let geo = res
        .get("columns_datatype")
        .and_then(Value::as_array)
        .is_some_and(|c| c.iter().take(500).filter_map(Value::as_str).any(|t| SOCRATA_GEO.contains(&t.to_ascii_lowercase().as_str())));
    let domain = r.get("metadata").map(|m| text(m, "domain", 253)).unwrap_or_default();
    if !is_id || !geo || !host_ok(&domain) || text(res, "type", 40) != "dataset" {
        return None;
    }
    let url = format!("https://{domain}/resource/{id}.geojson?%24limit={}", SOCRATA_ROWS);
    Some(Dataset {
        title: text(res, "name", 300),
        description: snippet(&text(res, "description", 4000)),
        organization: Some(text(res, "attribution", 200)).filter(|a| !a.is_empty()).unwrap_or_else(|| domain.clone()),
        page: Some(text(r, "permalink", 500)).filter(|p| p.starts_with("https://")),
        bbox: None,
        resources: vec![Resource { format: "GeoJSON".into(), url, using: Use::File }],
        id,
    })
}

/// How many rows a Socrata dataset's GeoJSON asks for (its API answers 1000 when not asked).
pub const SOCRATA_ROWS: usize = 50_000;

/// An ArcGIS Hub item, its page on the Hub site searched (`base`: hub.arcgis.com or an
/// organisation's own).
fn hub_dataset(base: &str, f: &Value) -> Option<Dataset> {
    let p = f.get("properties")?;
    let url = text(p, "url", 2048);
    let resource = classify("ArcGIS REST", "", &url)?;
    let id = Some(text(p, "id", 100)).filter(|i| !i.is_empty()).unwrap_or_else(|| text(f, "id", 100));
    Some(Dataset {
        page: (!id.is_empty()).then(|| format!("{}/datasets/{id}", base.trim_end_matches('/'))),
        title: text(p, "title", 300),
        description: snippet(&Some(text(p, "snippet", 2000)).filter(|s| !s.is_empty()).unwrap_or_else(|| text(p, "description", 2000))),
        organization: text(p, "owner", 200),
        bbox: p.get("extent").and_then(bbox_of),
        resources: vec![resource],
        id,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn portals_and_searches() {
        let ckan = Portal::new(None, None).unwrap();
        assert_eq!((ckan.kind, ckan.base.as_str()), (Kind::DataGov, DATA_GOV));
        assert_eq!(ckan.search_url(" city parks ", None, 99), "https://catalog.data.gov/search?q=city%20parks&per_page=50");
        let ca = Portal::new(Some("ckan"), Some("https://open.canada.ca/data/api/3/action/package_search?q=x")).unwrap();
        assert_eq!((ca.kind, ca.base.as_str()), (Kind::Ckan, "https://open.canada.ca/data"));
        assert_eq!(
            ca.search_url("lakes", Some([-80.0, 43.0, -79.0, 44.0]), 5),
            "https://open.canada.ca/data/api/3/action/package_search?q=lakes&rows=5&ext_bbox=-80,43,-79,44"
        );
        let nyc = Portal::new(Some("socrata"), Some("https://data.cityofnewyork.us/browse")).unwrap();
        assert_eq!((nyc.domain.as_deref(), nyc.label().as_str()), (Some("data.cityofnewyork.us"), "data.cityofnewyork.us"));
        assert_eq!(
            nyc.search_url("parks", None, 3),
            "https://api.us.socrata.com/api/catalog/v1?q=parks&only=dataset&limit=3&domains=data.cityofnewyork.us"
        );
        assert_eq!(Portal::new(Some("socrata"), None).unwrap().domain, None);
        let hub = Portal::new(Some("arcgis-hub"), None).unwrap();
        assert_eq!(
            hub.search_url("parks", Some([-74.3, 40.5, -73.7, 40.9]), 4),
            "https://hub.arcgis.com/api/search/v1/collections/dataset/items?q=parks&type=Feature%20Service&limit=4&bbox=-74.3,40.5,-73.7,40.9"
        );
        assert!(Portal::new(Some("google"), None).is_err());
        assert!(Portal::new(Some("socrata"), Some("not a host!")).is_err());
        assert!(Portal::new(Some("ckan"), Some("file:///etc")).is_err());
        assert!(Portal::new(Some("ckan"), Some("http://169.254.169.254/")).is_err());
    }

    #[test]
    fn resources_by_format() {
        let r = |f: &str, m: &str, u: &str| classify(f, m, u).map(|r| (r.format, r.using));
        assert_eq!(r("GeoJSON", "", "https://x.org/a?x=1"), Some(("GeoJSON".into(), Use::File)));
        assert_eq!(r("", "application/geo+json", "https://x.org/q"), Some(("GeoJSON".into(), Use::File)));
        assert_eq!(r("ZIP", "application/zip", "https://x.org/a/shapefile"), Some(("Shapefile".into(), Use::File)));
        assert_eq!(r("KML", "", "https://x.org/a.kml").map(|x| x.0), Some("KML".into()));
        assert_eq!(
            r("ArcGIS GeoServices REST API", "", "https://s.arcgis.com/a/arcgis/rest/services/p/FeatureServer/0"),
            Some(("ArcGIS REST".into(), Use::Service { kind: "arcgis", layer: Some("0".into()) }))
        );
        assert_eq!(
            classify("Esri REST", "", "https://s.arcgis.com/a/arcgis/rest/services/p/MapServer/").map(|x| x.url),
            Some("https://s.arcgis.com/a/arcgis/rest/services/p/MapServer".into())
        );
        assert_eq!(
            r("WMS", "", "https://x.org/wms?service=WMS&request=GetCapabilities").map(|x| x.1),
            Some(Use::Service { kind: "wms", layer: None })
        );
        for none in [
            ("HTML", "text/html", "https://x.org/"),
            ("PDF", "", "https://x.org/a.pdf"),
            ("GeoJSON", "", "ftp://x.org/a.geojson"),
            ("ArcGIS REST", "", "https://x.org/rest/services/a/GPServer"),
        ] {
            assert!(classify(none.0, none.1, none.2).is_none(), "{none:?}");
        }
    }

    #[test]
    fn answers() {
        let from = "https://catalog.data.gov/search?q=parks";
        let dg = br#"{"results":[{"slug":"parks","title":"Parks","description":"Parks of a city","organization":{"name":"City"},"spatial_shape":{"type":"Polygon","coordinates":[[[-122.5,47.5],[-122.2,47.5],[-122.2,47.7],[-122.5,47.5]]]},"dcat":{"distribution":[{"downloadURL":"https://x.org/p.csv","mediaType":"text/csv"},{"downloadURL":"https://x.org/p.geojson","mediaType":"application/geo+json"},{"accessURL":"http://127.0.0.1/p.geojson","format":"GeoJSON"},{"downloadURL":"https://x.org/p.html","format":"HTML"}]}},{"slug":"report","title":"A report","dcat":{"distribution":[{"downloadURL":"https://x.org/r.pdf"}]}}],"sort":"relevance"}"#;
        let p = Portal::new(None, None).unwrap();
        let ds = p.parse(from, dg, None).unwrap();
        assert_eq!(ds.len(), 1, "a dataset with nothing to add is left out");
        let d = &ds[0];
        assert_eq!((d.title.as_str(), d.organization.as_str()), ("Parks", "City"));
        assert_eq!(d.page.as_deref(), Some("https://catalog.data.gov/dataset/parks"));
        assert_eq!(d.bbox, Some([-122.5, 47.5, -122.2, 47.7]));
        // GeoJSON first, the local-network address left out, the page not a resource.
        assert_eq!(d.resources.iter().map(|r| r.format.as_str()).collect::<Vec<_>>(), vec!["GeoJSON", "CSV"]);
        // Kept to a box: a dataset elsewhere goes.
        assert!(p.parse(from, dg, Some([0.0, 0.0, 1.0, 1.0])).unwrap().is_empty());
        assert_eq!(p.parse(from, dg, Some([-123.0, 47.0, -122.0, 48.0])).unwrap().len(), 1);
        assert!(p.parse(from, b"<html>", None).is_err() && p.parse(from, b"{}", None).is_err());

        let soc = br#"{"results":[{"resource":{"id":"enfh-gkve","name":"Parks Properties","type":"dataset","columns_datatype":["Text","MultiPolygon"]},"metadata":{"domain":"data.cityofnewyork.us"},"permalink":"https://data.cityofnewyork.us/d/enfh-gkve"},{"resource":{"id":"nc67-uf89","name":"Violations","type":"dataset","columns_datatype":["Text"]},"metadata":{"domain":"data.cityofnewyork.us"}},{"resource":{"id":"bad","name":"x","type":"dataset","columns_datatype":["Point"]},"metadata":{"domain":"evil host"}}]}"#;
        let s = Portal::new(Some("socrata"), None).unwrap();
        let ds = s.parse("https://api.us.socrata.com/api/catalog/v1?q=parks", soc, None).unwrap();
        assert_eq!(ds.len(), 1);
        assert_eq!(ds[0].resources[0].url, "https://data.cityofnewyork.us/resource/enfh-gkve.geojson?%24limit=50000");

        let hub = br#"{"type":"FeatureCollection","features":[{"id":"c5ef","properties":{"id":"c5ef","title":"parks","type":"Feature Service","owner":"nyc","url":"https://services1.arcgis.com/S/arcgis/rest/services/parks/FeatureServer","extent":{"type":"Polygon","coordinates":[[[-73.9,40.76],[-73.87,40.76],[-73.87,40.74],[-73.9,40.76]]]}}},{"properties":{"title":"no url"}}]}"#;
        let h = Portal::new(Some("arcgis-hub"), None).unwrap();
        let ds = h.parse("https://hub.arcgis.com/api/search/v1/collections/dataset/items?q=parks", hub, None).unwrap();
        assert_eq!(ds.len(), 1);
        assert_eq!(ds[0].resources[0].using, Use::Service { kind: "arcgis", layer: None });
        assert_eq!(ds[0].bbox, Some([-73.9, 40.74, -73.87, 40.76]));

        let ckan = br#"{"success":true,"result":{"count":1,"results":[{"name":"lakes","title":"Lakes","notes":"All lakes","organization":{"title":"NRCan"},"extras":[{"key":"spatial","value":"{\"type\":\"Polygon\",\"coordinates\":[[[-80,43],[-79,43],[-79,44],[-80,43]]]}"}],"resources":[{"format":"WMS","url":"https://maps.example.ca/wms?service=WMS"},{"format":"SHP","url":"https://x.ca/lakes.zip"}]}]}}"#;
        let c = Portal::new(Some("ckan"), Some("https://open.canada.ca/data")).unwrap();
        let ds = c.parse("https://open.canada.ca/data/api/3/action/package_search?q=lakes", ckan, None).unwrap();
        assert_eq!(ds[0].page.as_deref(), Some("https://open.canada.ca/data/dataset/lakes"));
        assert_eq!(ds[0].resources.iter().map(|r| r.format.as_str()).collect::<Vec<_>>(), vec!["Shapefile", "WMS"]);
        assert!(c.parse("x", br#"{"success":false,"error":{"message":"Bad request"}}"#, None).unwrap_err().contains("Bad request"));
    }

    #[test]
    fn boxes_meet() {
        assert!(overlaps([0.0, 0.0, 10.0, 10.0], [5.0, 5.0, 15.0, 15.0]));
        assert!(!overlaps([0.0, 0.0, 10.0, 10.0], [11.0, 0.0, 12.0, 1.0]));
        // Across the antimeridian.
        assert!(overlaps([170.0, -20.0, -170.0, -10.0], [178.0, -18.0, 179.0, -17.0]));
        assert!(overlaps([170.0, -20.0, -170.0, -10.0], [-179.0, -18.0, -178.0, -17.0]));
        assert!(!overlaps([170.0, -20.0, -170.0, -10.0], [0.0, -18.0, 1.0, -17.0]));
    }
}
