//! The map's search box: one query matched against a coordinate, the map's layers, its
//! bookmarks and the values in its layers' attribute tables. Places (geocoding) are the UI's,
//! asked for on Enter: the public servers don't allow searching as one types.

use rustgis_data::Value as Attr;
use rustgis_geom::{Coord, Extent};
use serde_json::{Value, json};

use super::{f, ps};
use crate::{CmdResult, CommandSpec as C, Session};

pub fn commands() -> Vec<C> {
    vec![
        // Not a view command: going to a feature selects it.
        C::new("map.search", "Search", "Map", search_cmd).params(
            r#"{"query": "a latitude and longitude (40.7, -74.0; 40°42'N 74°0'W), a layer, a bookmark or a value in a layer's table", "limit"?: 1–50, "go"?: true (the first hit) | index}"#,
        ),
    ]
}

/// The fewest characters a query needs to search attribute values.
pub const MIN_FEATURE_QUERY: usize = 2;
/// Most matches listed for one layer's values.
const PER_LAYER: usize = 5;
/// Most rows of a layer whose values are searched; a larger layer's later rows aren't. The
/// search box searches as one types, on the frame, so this stays small enough to be quick.
const MAX_ROWS: usize = 50_000;
/// The most hits `map.search` lists unless told otherwise; the search box lists as many, so an
/// index it picks is one the command finds.
pub const LIMIT: usize = 30;
/// Most characters of a query, and of a value shown.
const MAX_CHARS: usize = 200;

/// What a search found.
#[derive(Clone, Debug, PartialEq)]
pub enum Hit {
    /// A point typed as a latitude and longitude.
    Coordinate { lon: f64, lat: f64 },
    /// A layer whose name matches.
    Layer { id: u32, name: String },
    /// A bookmark whose name matches.
    Bookmark { name: String },
    /// A feature with a matching value.
    /// `name` is the feature's name, when the layer has a name field and it isn't the one matched.
    Feature { layer: u32, layer_name: String, row: usize, field: String, value: String, name: Option<String> },
}

impl Hit {
    /// The hit as `map.search` returns it.
    pub fn to_json(&self) -> Value {
        match self {
            Hit::Coordinate { lon, lat } => json!({"kind": "coordinate", "lon": lon, "lat": lat, "label": format_lat_lon(*lat, *lon)}),
            Hit::Layer { id, name } => json!({"kind": "layer", "layer": id, "label": name}),
            Hit::Bookmark { name } => json!({"kind": "bookmark", "label": name}),
            Hit::Feature { layer, layer_name, row, field, value, name } => {
                json!({"kind": "feature", "layer": layer, "layerName": layer_name, "row": row, "field": field, "label": value, "name": name})
            }
        }
    }
}

/// The results of a search, and the layers whose values were searched only in part.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Results {
    pub hits: Vec<Hit>,
    pub partial: Vec<u32>,
}

/// Searches the active map for `query`: a coordinate first, then layers, bookmarks and features.
pub fn search(s: &Session, query: &str, limit: usize) -> Results {
    let q: String = query.trim().chars().take(MAX_CHARS).collect();
    let mut out = Results::default();
    if q.is_empty() {
        return out;
    }
    if let Some(c) = parse_lat_lon(&q) {
        out.hits.push(Hit::Coordinate { lon: c.x, lat: c.y });
    }
    let needle = q.to_lowercase();
    let map = s.map();
    for l in &map.layers {
        if contains_ci(&l.name, &needle) {
            out.hits.push(Hit::Layer { id: l.id, name: l.name.clone() });
        }
    }
    for b in &map.bookmarks {
        if contains_ci(&b.name, &needle) {
            out.hits.push(Hit::Bookmark { name: b.name.clone() });
        }
    }
    // A query that is plainly a coordinate (a decimal point, a degree mark, a hemisphere) isn't
    // looked for in the tables; two whole numbers (an id, years, a range) may be either.
    let plainly_coordinate = matches!(out.hits.first(), Some(Hit::Coordinate { .. }))
        && q.chars().any(|c| matches!(c, '.' | '°' | 'º' | '\'' | '′' | '"' | '″') || c.is_alphabetic());
    if q.chars().count() >= MIN_FEATURE_QUERY && !plainly_coordinate {
        let number = q.parse::<f64>().ok().filter(|n| n.is_finite());
        for l in map.layers.iter().filter(|l| !l.is_raster() && !l.graphics) {
            // A field called name (or the like) ranks ahead of the others when they match as well.
            let name_fi = name_field(&l.dataset.fields);
            // The best few: an exact value, then one that starts with the query, then the rest;
            // each feature by its best-matching field.
            let mut best: Vec<(u8, u8, usize, usize)> = Vec::new();
            for (row, feat) in l.dataset.features.iter().enumerate().take(MAX_ROWS) {
                let mut mine: Option<(u8, u8, usize, usize)> = None;
                for (fi, v) in feat.attrs.iter().enumerate() {
                    let rank = match v {
                        Attr::Text(t) => rank_text(t, &needle),
                        Attr::Int(i) => number.filter(|n| *n == *i as f64).map(|_| 0),
                        Attr::Float(x) => number.filter(|n| n == x).map(|_| 0),
                        _ => None,
                    };
                    if let Some(rank) = rank {
                        let key = (rank, u8::from(Some(fi) != name_fi), row, fi);
                        if mine.is_none_or(|m| key < m) {
                            mine = Some(key);
                        }
                    }
                }
                best.extend(mine);
                if best.len() >= PER_LAYER * 8 {
                    best.sort();
                    best.truncate(PER_LAYER);
                    if best.iter().all(|b| b.0 == 0 && b.1 == 0) {
                        break;
                    }
                }
            }
            best.sort();
            best.truncate(PER_LAYER);
            if l.dataset.features.len() > MAX_ROWS {
                out.partial.push(l.id);
            }
            for (_, _, row, fi) in best {
                let Some(feat) = l.dataset.features.get(row) else { continue };
                let Some(v) = feat.attrs.get(fi) else { continue };
                let field = l.dataset.fields.get(fi).map(|f| f.display().to_string()).unwrap_or_default();
                let value: String = v.to_string().chars().take(MAX_CHARS).collect();
                let name = name_fi
                    .filter(|n| *n != fi)
                    .and_then(|n| feat.attrs.get(n))
                    .filter(|v| !matches!(v, Attr::Null))
                    .map(|v| v.to_string().chars().take(MAX_CHARS).collect());
                out.hits.push(Hit::Feature { layer: l.id, layer_name: l.name.clone(), row, field, value, name });
            }
        }
    }
    out.hits.truncate(limit.max(1));
    out
}

/// The field that names a layer's features: `name`, or one starting with it (`NAME_EN`,
/// `name_long`), or a title or label.
fn name_field(fields: &[rustgis_data::Field]) -> Option<usize> {
    let low: Vec<String> = fields.iter().map(|f| f.name.to_ascii_lowercase()).collect();
    low.iter()
        .position(|n| n == "name")
        .or_else(|| low.iter().position(|n| n.starts_with("name")))
        .or_else(|| low.iter().position(|n| matches!(n.as_str(), "title" | "label")))
}

/// 0 for the same text, 1 for one starting with the query, 2 for one containing it.
fn rank_text(t: &str, needle: &str) -> Option<u8> {
    if t.is_ascii() && needle.is_ascii() {
        // Lowercasing other text can change its length in bytes, so only here is this a bound.
        if t.len() < needle.len() {
            return None;
        }
        let (h, n) = (t.as_bytes(), needle.as_bytes());
        if h.eq_ignore_ascii_case(n) {
            return Some(0);
        }
        if h.get(..n.len()).is_some_and(|p| p.eq_ignore_ascii_case(n)) {
            return Some(1);
        }
        return h.windows(n.len()).any(|w| w.eq_ignore_ascii_case(n)).then_some(2);
    }
    let low = t.to_lowercase();
    if low == needle {
        Some(0)
    } else if low.starts_with(needle) {
        Some(1)
    } else if low.contains(needle) {
        Some(2)
    } else {
        None
    }
}

/// Whether `t` contains `needle` (already lowercase), ignoring case.
fn contains_ci(t: &str, needle: &str) -> bool {
    rank_text(t, needle).is_some()
}

/// Goes to a hit: centres a coordinate, zooms to a layer or a bookmark, selects a feature and
/// zooms to it.
pub fn go_to(s: &mut Session, hit: &Hit) -> CmdResult {
    match hit {
        Hit::Coordinate { lon, lat } => {
            s.zoom_to(&around(Coord::new(*lon, *lat)));
            Ok(Value::Null)
        }
        Hit::Layer { id, name } => {
            let e = s.map().layer(*id).map(|l| l.shown_extent()).ok_or_else(|| format!("no layer '{name}'"))?;
            s.active_layer = Some(*id);
            if !e.is_empty() {
                s.zoom_to(&e);
            }
            Ok(Value::Null)
        }
        Hit::Bookmark { name } => {
            let cam =
                s.map().bookmarks.iter().find(|b| b.name == *name).map(|b| b.camera).ok_or_else(|| format!("no bookmark '{name}'"))?;
            s.map_mut().camera = cam;
            s.record_extent();
            Ok(Value::Null)
        }
        Hit::Feature { layer, layer_name, row, .. } => {
            if s.map().layer(*layer).is_none_or(|l| *row >= l.dataset.features.len()) {
                return Err(format!("{layer_name} has changed; search again"));
            }
            s.active_layer = Some(*layer);
            super::select::set(s, *layer, vec![*row], super::select::Mode::New);
            let e = s
                .map()
                .layer(*layer)
                .and_then(|l| l.dataset.features.get(*row))
                .and_then(|f| f.geometry.as_ref())
                .map(|g| g.extent())
                .unwrap_or_default();
            // A feature without a shape (a table row) is still selected.
            if !e.is_empty() {
                s.zoom_to(&if e.width() == 0.0 && e.height() == 0.0 { around(e.center()) } else { e });
            }
            Ok(Value::Null)
        }
    }
}

/// A town-sized view around a point (about 1:150,000 on a screen).
fn around(c: Coord) -> Extent {
    Extent::new(c.x - 0.05, c.y - 0.04, c.x + 0.05, c.y + 0.04)
}

fn search_cmd(s: &mut Session, p: &Value) -> CmdResult {
    let query = ps(p, "query").map(str::trim).filter(|q| !q.is_empty()).ok_or("query is required")?;
    let limit = f(p, "limit").filter(|l| l.is_finite()).map(|l| l.clamp(1.0, 50.0) as usize).unwrap_or(LIMIT);
    let r = search(s, query, limit);
    let pick = match p.get("go") {
        None | Some(Value::Null | Value::Bool(false)) => None,
        Some(Value::Bool(true)) => Some(0),
        Some(v) => Some(
            v.as_u64()
                .or_else(|| v.as_f64().filter(|x| x.fract() == 0.0 && *x >= 0.0 && *x <= 1e6).map(|x| x as u64))
                .and_then(|n| usize::try_from(n).ok())
                .ok_or("go is true or the index of a hit (0 for the first)")?,
        ),
    };
    if let Some(i) = pick {
        let hit =
            r.hits.get(i).ok_or_else(|| if r.hits.is_empty() { format!("nothing matches '{query}'") } else { format!("no hit {i}") })?;
        go_to(s, hit)?;
    }
    Ok(json!({"hits": r.hits.iter().map(Hit::to_json).collect::<Vec<_>>(), "partial": r.partial}))
}

/// `lat, lon` rounded to six decimals (about 0.1 m), without trailing zeros.
pub fn format_lat_lon(lat: f64, lon: f64) -> String {
    let r = |v: f64| {
        let t = format!("{v:.6}");
        let t = t.trim_end_matches('0').trim_end_matches('.');
        if t == "-0" { "0".to_string() } else { t.to_string() }
    };
    format!("{}, {}", r(lat), r(lon))
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Tok {
    Num(f64),
    /// A mark after a number: ° (true), or ' or " (false).
    Unit(bool),
    Hemi(char),
    Comma,
}

fn tokens(text: &str) -> Option<Vec<Tok>> {
    let mut out = Vec::new();
    let mut chars = text.chars().peekable();
    // A sign starts a number only after a space, a comma or a mark, not straight after a digit.
    let mut prev = ' ';
    while let Some(&c) = chars.peek() {
        if c.is_ascii_digit() || c == '.' || (matches!(c, '-' | '+' | '−') && !prev.is_ascii_digit() && prev != '.') {
            let mut s = String::new();
            if matches!(c, '-' | '−') {
                s.push('-');
                chars.next();
            } else if c == '+' {
                chars.next();
            }
            while let Some(&d) = chars.peek() {
                if d.is_ascii_digit() || d == '.' {
                    s.push(d);
                    chars.next();
                } else {
                    break;
                }
            }
            out.push(Tok::Num(s.parse::<f64>().ok().filter(|v| v.is_finite())?));
            prev = '0';
        } else if c.is_alphabetic() {
            let mut word = String::new();
            while let Some(&d) = chars.peek() {
                if d.is_alphabetic() {
                    word.push(d);
                    chars.next();
                } else {
                    break;
                }
            }
            let up = word.to_ascii_uppercase();
            match up.as_str() {
                "N" | "S" | "E" | "W" => out.push(Tok::Hemi(up.chars().next()?)),
                _ => return None,
            }
            prev = 'a';
        } else {
            chars.next();
            prev = c;
            match c {
                '°' | 'º' => out.push(Tok::Unit(true)),
                '\'' | '′' | '’' | '"' | '″' | '”' => out.push(Tok::Unit(false)),
                ',' | ';' => out.push(Tok::Comma),
                c if c.is_whitespace() => {}
                _ => return None,
            }
        }
        if out.len() > 32 {
            return None;
        }
    }
    Some(out)
}

/// One axis: degrees, and maybe minutes and seconds; `None` for anything else.
fn axis(nums: &[f64]) -> Option<f64> {
    let (d, rest) = nums.split_first()?;
    if rest.len() > 2 || rest.iter().any(|v| !(0.0..60.0).contains(v)) {
        return None;
    }
    let m = rest.first().copied().unwrap_or(0.0);
    let sec = rest.get(1).copied().unwrap_or(0.0);
    let v = d.abs() + m / 60.0 + sec / 3600.0;
    Some(if d.is_sign_negative() { -v } else { v })
}

/// A latitude and longitude typed in decimal degrees (`40.7, -74.0`), degrees and minutes or
/// degrees, minutes and seconds (`40°42'46"N 74°0'21"W`), with N/S/E/W before or after each
/// number. Latitude comes first unless the hemispheres say otherwise, or the first number can
/// only be a longitude.
pub fn parse_lat_lon(text: &str) -> Option<Coord> {
    let toks = tokens(text)?;
    let hemis: Vec<char> = toks.iter().filter_map(|t| if let Tok::Hemi(c) = t { Some(*c) } else { None }).collect();
    let mut groups: Vec<(Vec<f64>, Option<char>)> = Vec::new();
    if !hemis.is_empty() {
        if hemis.len() != 2 {
            return None;
        }
        // A letter before the first number marks each axis where it starts; otherwise where it ends.
        let prefix = matches!(toks.iter().find(|t| !matches!(t, Tok::Comma)), Some(Tok::Hemi(_)));
        let mut cur: (Vec<f64>, Option<char>) = (Vec::new(), None);
        for t in &toks {
            match *t {
                Tok::Num(v) => cur.0.push(v),
                Tok::Hemi(h) if prefix => {
                    if !cur.0.is_empty() {
                        groups.push(std::mem::take(&mut cur));
                    }
                    cur.1 = Some(h);
                }
                Tok::Hemi(h) => {
                    cur.1 = Some(h);
                    groups.push(std::mem::take(&mut cur));
                }
                Tok::Unit(_) | Tok::Comma => {}
            }
        }
        if !cur.0.is_empty() || cur.1.is_some() {
            groups.push(cur);
        }
    } else if toks.contains(&Tok::Comma) {
        let mut cur = Vec::new();
        for t in &toks {
            match *t {
                Tok::Num(v) => cur.push(v),
                Tok::Comma => groups.push((std::mem::take(&mut cur), None)),
                _ => {}
            }
        }
        groups.push((cur, None));
    } else if toks.contains(&Tok::Unit(true)) {
        // Degrees, minutes and seconds with their marks but no comma: an axis starts at each
        // number marked as degrees.
        let mut cur: Vec<f64> = Vec::new();
        for (i, t) in toks.iter().enumerate() {
            if let Tok::Num(v) = *t {
                if toks.get(i + 1) == Some(&Tok::Unit(true)) && !cur.is_empty() {
                    groups.push((std::mem::take(&mut cur), None));
                }
                cur.push(v);
            }
        }
        groups.push((cur, None));
    } else {
        let nums: Vec<f64> = toks.iter().filter_map(|t| if let Tok::Num(v) = t { Some(*v) } else { None }).collect();
        if nums.len() != 2 {
            return None;
        }
        groups = nums.into_iter().map(|v| (vec![v], None)).collect();
    }
    let [(a, ha), (b, hb)] = <[_; 2]>::try_from(groups).ok()?;
    let (mut a, mut b) = (axis(&a)?, axis(&b)?);
    let sign = |v: f64, h: char| if matches!(h, 'S' | 'W') { -v.abs() } else { v };
    let (lat, lon) = match (ha, hb) {
        (Some(x), Some(y)) => {
            a = sign(a, x);
            b = sign(b, y);
            match (matches!(x, 'N' | 'S'), matches!(y, 'N' | 'S')) {
                (true, false) => (a, b),
                (false, true) => (b, a),
                _ => return None,
            }
        }
        (None, None) => {
            if a.abs() > 90.0 && b.abs() <= 90.0 {
                (b, a)
            } else {
                (a, b)
            }
        }
        _ => return None,
    };
    if !(-90.0..=90.0).contains(&lat) || !(-180.0..=180.0).contains(&lon) {
        return None;
    }
    Some(Coord::new(lon, lat))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ll(t: &str) -> Option<(f64, f64)> {
        parse_lat_lon(t).map(|c| ((c.y * 1e4).round() / 1e4, (c.x * 1e4).round() / 1e4))
    }

    #[test]
    fn coordinates_parse() {
        assert_eq!(ll("40.7, -74.0"), Some((40.7, -74.0)));
        assert_eq!(ll("40.7 -74.0"), Some((40.7, -74.0)));
        assert_eq!(ll("-122.4, 37.8"), Some((37.8, -122.4)));
        assert_eq!(ll("40.7N 74.0W"), Some((40.7, -74.0)));
        assert_eq!(ll("74.0 W, 40.7 N"), Some((40.7, -74.0)));
        assert_eq!(ll("N 40.7 W 74"), Some((40.7, -74.0)));
        assert_eq!(ll("40°42'46\"N 74°0'21\"W"), Some((40.7128, -74.0058)));
        assert_eq!(ll("40°42.767' -74°0.35'"), Some((40.7128, -74.0058)));
        assert_eq!(ll("40 42 46 N, 74 0 21 W"), Some((40.7128, -74.0058)));
        assert_eq!(ll("51.5074,-0.1278"), Some((51.5074, -0.1278)));
    }

    #[test]
    fn non_coordinates_are_refused() {
        for t in
            ["Paris", "New York", "95, 200", "40.7", "40.7N 74W 3E", "1 2 3", "40N 50S", "", "12-5", "40° 61' N 74 W", "Seattle 47 -122"]
        {
            assert_eq!(parse_lat_lon(t), None, "{t}");
        }
    }

    #[test]
    fn formats_round() {
        assert_eq!(format_lat_lon(40.7, -74.00000001), "40.7, -74");
        assert_eq!(format_lat_lon(-0.0000001, 2.5), "0, 2.5");
    }

    #[test]
    fn finds_layers_and_values() {
        let s = Session::new(crate::sample::world_project());
        let r = search(&s, "cities", 50);
        assert!(r.hits.iter().any(|h| matches!(h, Hit::Layer { name, .. } if name == "Cities")), "{r:?}");
        let r = search(&s, "Tokyo", 50);
        assert!(r.hits.iter().any(|h| matches!(h, Hit::Feature { value, .. } if value == "Tokyo")), "{r:?}");
        // A city in Togo is found by its country, and named.
        let r = search(&s, "togo", 50);
        assert!(r.hits.iter().any(|h| matches!(h, Hit::Feature { field, name: Some(_), .. } if field != "name")), "{r:?}");
        let r = search(&s, "48.85, 2.35", 50);
        assert_eq!(r.hits, vec![Hit::Coordinate { lon: 2.35, lat: 48.85 }]);
        // Two whole numbers may be a coordinate or values: both are listed.
        let r = search(&s, "10 20", 50);
        assert_eq!(r.hits.first(), Some(&Hit::Coordinate { lon: 20.0, lat: 10.0 }));
        // Lowercasing İ makes it longer, so a match isn't refused by its length.
        assert_eq!(rank_text("İstanbul", "i̇stanbul"), Some(0));
        assert!(search(&s, "   ", 50).hits.is_empty());
    }

    #[test]
    fn going_to_a_feature_selects_it() {
        let mut s = Session::new(crate::sample::world_project());
        let v = search_cmd(&mut s, &json!({"query": "Tokyo", "go": true})).unwrap_or_default();
        assert!(v["hits"].as_array().is_some_and(|h| !h.is_empty()));
        assert!(s.map().layers.iter().any(|l| !l.selection.is_empty()));
        assert!(search_cmd(&mut s, &json!({"query": "zzqqxx-none", "go": true})).is_err());
        assert!(search_cmd(&mut s, &json!({"query": "Tokyo", "go": 999})).is_err());
        assert!(search_cmd(&mut s, &json!({"query": "Tokyo", "go": 0.0})).is_ok());
        let e = search_cmd(&mut s, &json!({"query": "Tokyo", "go": -1.5})).err().unwrap_or_default();
        assert!(e.starts_with("go is"), "{e}");
    }
}
