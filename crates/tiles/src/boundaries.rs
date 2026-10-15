//! geoBoundaries: open administrative boundaries for every country (ADM0 to ADM5), from the
//! geoBoundaries API (https://www.geoboundaries.org). The API's answer names the GeoJSON file
//! (on GitHub's large-file storage); requests are built and answers read here, the fetching is the
//! caller's.

use serde_json::Value;

/// The geoBoundaries API (its open release).
pub const API: &str = "https://www.geoboundaries.org/api/current/gbOpen";

/// A country's ISO 3166-1 alpha-3 code, upper case.
pub fn country(code: &str) -> Result<String, String> {
    let c = code.trim().to_ascii_uppercase();
    if c.len() == 3 && c.chars().all(|ch| ch.is_ascii_uppercase()) {
        Ok(c)
    } else {
        Err(format!("'{}': a country is its three-letter ISO code (such as CHE or KEN)", code.trim()))
    }
}

/// A level, `ADM0` to `ADM5` (a number 0–5 too).
pub fn level(l: &str) -> Result<String, String> {
    let t = l.trim().to_ascii_uppercase();
    let n = t.strip_prefix("ADM").unwrap_or(&t);
    match n.parse::<u8>() {
        Ok(n) if n <= 5 => Ok(format!("ADM{n}")),
        _ => Err(format!("'{}': a level is ADM0 (the country) to ADM5", l.trim())),
    }
}

/// The API's address for a country's level.
pub fn meta_url(country: &str, level: &str) -> String {
    format!("{API}/{country}/{level}/")
}

/// The address of every country's ADM0 entry (a list of countries).
pub fn countries_url() -> String {
    format!("{API}/ALL/ADM0/")
}

/// A country's level, as the API describes it.
#[derive(Clone, Debug, PartialEq)]
pub struct Boundary {
    pub iso: String,
    pub name: String,
    pub level: String,
    /// The simplified GeoJSON's address.
    pub geojson: String,
}

/// Reads the API's answer for one country's level.
pub fn parse_meta(bytes: &[u8]) -> Result<Boundary, String> {
    let v: Value = serde_json::from_slice(bytes).map_err(|_| "geoBoundaries has no such country or level".to_string())?;
    // An unknown country or level is answered as an error text or an empty list.
    let v = match &v {
        Value::Array(a) => a.first().ok_or("geoBoundaries has no such country or level")?,
        v => v,
    };
    let text = |k: &str| v.get(k).and_then(Value::as_str).map(|s| s.chars().take(300).collect::<String>());
    let geojson =
        text("simplifiedGeometryGeoJSON").or_else(|| text("gjDownloadURL")).ok_or("geoBoundaries has no such country or level")?;
    Ok(Boundary {
        iso: text("boundaryISO").unwrap_or_default(),
        name: text("boundaryName").unwrap_or_default(),
        level: text("boundaryType").unwrap_or_default(),
        geojson: direct(&geojson),
    })
}

/// The countries the API lists: code and name, sorted by name.
pub fn parse_countries(bytes: &[u8]) -> Result<Vec<(String, String)>, String> {
    let v: Value = serde_json::from_slice(bytes).map_err(|_| "the geoBoundaries list isn't JSON".to_string())?;
    let mut out: Vec<(String, String)> = v
        .as_array()
        .ok_or("the geoBoundaries list isn't a list")?
        .iter()
        .take(1000)
        .filter_map(|c| {
            let iso = country(c.get("boundaryISO")?.as_str()?).ok()?;
            Some((iso, c.get("boundaryName")?.as_str()?.chars().take(100).collect()))
        })
        .collect();
    out.sort_by(|a, b| a.1.cmp(&b.1));
    out.dedup();
    Ok(out)
}

/// GitHub's `…/raw/…` addresses redirect to the large-file host; the downloads follow no
/// redirects, so the file is asked for there directly.
fn direct(url: &str) -> String {
    if let Some(rest) = url.strip_prefix("https://github.com/")
        && let [owner, repo, "raw", reference, path @ ..] = rest.split('/').collect::<Vec<_>>().as_slice()
        && !path.is_empty()
    {
        return format!("https://media.githubusercontent.com/media/{owner}/{repo}/{reference}/{}", path.join("/"));
    }
    url.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_and_answers() {
        assert_eq!(country(" che ").unwrap(), "CHE");
        assert!(country("CH").is_err() && country("C1E").is_err());
        assert_eq!(level("1").unwrap(), "ADM1");
        assert_eq!(level("adm0").unwrap(), "ADM0");
        assert!(level("ADM6").is_err() && level("x").is_err());
        assert_eq!(meta_url("CHE", "ADM1"), "https://www.geoboundaries.org/api/current/gbOpen/CHE/ADM1/");
        let meta = br#"{"boundaryISO": "CHE", "boundaryName": "Switzerland", "boundaryType": "ADM1", "simplifiedGeometryGeoJSON": "https://github.com/wmgeolab/geoBoundaries/raw/9469f09/releaseData/gbOpen/CHE/ADM1/x_simplified.geojson"}"#;
        let b = parse_meta(meta).unwrap();
        assert_eq!((b.iso.as_str(), b.name.as_str(), b.level.as_str()), ("CHE", "Switzerland", "ADM1"));
        assert_eq!(
            b.geojson,
            "https://media.githubusercontent.com/media/wmgeolab/geoBoundaries/9469f09/releaseData/gbOpen/CHE/ADM1/x_simplified.geojson"
        );
        assert!(parse_meta(b"Not found").is_err() && parse_meta(b"[]").is_err() && parse_meta(b"{}").is_err());
        let list = br#"[{"boundaryISO": "KEN", "boundaryName": "Kenya"}, {"boundaryISO": "ABW", "boundaryName": "Aruba"}, {"boundaryISO": "bad!"}]"#;
        assert_eq!(parse_countries(list).unwrap(), vec![("ABW".into(), "Aruba".into()), ("KEN".into(), "Kenya".into())]);
        assert_eq!(direct("https://example.org/a.geojson"), "https://example.org/a.geojson");
    }
}
