//! TopoJSON: geometries sharing arcs, quantized and delta-encoded when the topology has a
//! transform. Each object of the topology becomes a layer (its geometries rebuilt from their arcs
//! as GeoJSON features, then read as GeoJSON).
//!
//! The file is hostile: arc indices are checked, nesting is fixed by the geometry types, and the
//! number of coordinates rebuilt is bounded.

use serde_json::{Map, Value as Json, json};

use crate::{DataError, Dataset, ReadReport, Result};

/// The most coordinates rebuilt from arcs.
const MAX_COORDS: usize = 20_000_000;
/// The most objects (layers) read.
const MAX_OBJECTS: usize = 64;

pub fn is_topojson_name(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n.ends_with(".topojson") || n.ends_with(".topo.json")
}

/// Whether JSON text is a TopoJSON topology.
pub fn looks_like_topojson(text: &str) -> bool {
    // The first 512 bytes, cut at a character boundary.
    let end = (0..=text.len().min(512)).rev().find(|&i| text.is_char_boundary(i)).unwrap_or(0);
    let head = text.get(..end).unwrap_or("");
    // `"type": "Topology"`, whatever the spacing (a property value "Topology" doesn't count).
    let squeezed: String = head.chars().filter(|c| !c.is_whitespace()).collect();
    squeezed.contains("\"type\":\"Topology\"")
}

struct Topology {
    /// The arcs, decoded to coordinates.
    arcs: Vec<Vec<[f64; 2]>>,
    transform: Option<([f64; 2], [f64; 2])>,
    budget: usize,
}

impl Topology {
    fn point(&self, p: &Json) -> Option<[f64; 2]> {
        let a = p.as_array()?;
        let (x, y) = (a.first()?.as_f64()?, a.get(1)?.as_f64()?);
        Some(match self.transform {
            Some(([sx, sy], [tx, ty])) => [x * sx + tx, y * sy + ty],
            None => [x, y],
        })
    }

    /// A line from arc indices (`~i` is arc `i` reversed), shared ends written once.
    fn line(&mut self, ids: &Json) -> Option<Vec<[f64; 2]>> {
        let mut out: Vec<[f64; 2]> = Vec::new();
        for id in ids.as_array()? {
            let i = id.as_i64()?;
            let (k, rev) = if i < 0 { (!i, true) } else { (i, false) };
            let arc = self.arcs.get(usize::try_from(k).ok()?)?;
            self.budget = self.budget.checked_sub(arc.len())?;
            let pts: Vec<[f64; 2]> = if rev { arc.iter().rev().copied().collect() } else { arc.clone() };
            let skip = usize::from(!out.is_empty());
            out.extend(pts.into_iter().skip(skip));
        }
        Some(out)
    }

    fn rings(&mut self, v: &Json) -> Option<Vec<Vec<[f64; 2]>>> {
        v.as_array()?.iter().map(|r| self.line(r)).collect()
    }

    /// A TopoJSON geometry as GeoJSON (`None` when it can't be read).
    fn geometry(&mut self, g: &Json, depth: usize) -> Option<Json> {
        let ty = g.get("type")?.as_str()?;
        let arcs = g.get("arcs");
        let coords = g.get("coordinates");
        Some(match ty {
            "Point" => json!({"type": ty, "coordinates": self.point(coords?)?}),
            "MultiPoint" => {
                let pts: Option<Vec<[f64; 2]>> = coords?.as_array()?.iter().map(|p| self.point(p)).collect();
                json!({"type": ty, "coordinates": pts?})
            }
            "LineString" => json!({"type": ty, "coordinates": self.line(arcs?)?}),
            "MultiLineString" | "Polygon" => json!({"type": ty, "coordinates": self.rings(arcs?)?}),
            "MultiPolygon" => {
                let polys: Option<Vec<Vec<Vec<[f64; 2]>>>> = arcs?.as_array()?.iter().map(|p| self.rings(p)).collect();
                json!({"type": ty, "coordinates": polys?})
            }
            "GeometryCollection" if depth < 4 => {
                let gs: Vec<Json> = g.get("geometries")?.as_array()?.iter().filter_map(|x| self.geometry(x, depth + 1)).collect();
                json!({"type": ty, "geometries": gs})
            }
            _ => return None,
        })
    }
}

/// Reads a TopoJSON topology into a layer per object.
pub fn read(file_name: &str, text: &str) -> Result<Vec<(Dataset, ReadReport)>> {
    let stem = std::path::Path::new(file_name).file_stem().and_then(|s| s.to_str()).unwrap_or("topology");
    let stem = stem.strip_suffix(".topo").unwrap_or(stem);
    let v: Json = serde_json::from_str(text.trim_start_matches('\u{feff}')).map_err(|e| DataError::Format(format!("{file_name}: {e}")))?;
    if v.get("type").and_then(Json::as_str) != Some("Topology") {
        return Err(DataError::Format(format!("{file_name}: not a TopoJSON topology")));
    }
    let transform = v.get("transform").and_then(|t| {
        let s = t.get("scale")?.as_array()?;
        let tr = t.get("translate")?.as_array()?;
        Some(([s.first()?.as_f64()?, s.get(1)?.as_f64()?], [tr.first()?.as_f64()?, tr.get(1)?.as_f64()?]))
    });
    // Arcs: delta-encoded integers with a transform, plain coordinates without.
    let mut total = 0usize;
    let mut arcs = Vec::new();
    for a in v.get("arcs").and_then(Json::as_array).map(Vec::as_slice).unwrap_or_default() {
        let mut pts = Vec::new();
        let (mut x, mut y) = (0.0f64, 0.0f64);
        for p in a.as_array().map(Vec::as_slice).unwrap_or_default() {
            let (Some(px), Some(py)) = (p.get(0).and_then(Json::as_f64), p.get(1).and_then(Json::as_f64)) else { continue };
            let pt = match transform {
                Some(([sx, sy], [tx, ty])) => {
                    x += px;
                    y += py;
                    [x * sx + tx, y * sy + ty]
                }
                None => [px, py],
            };
            pts.push(pt);
        }
        total += pts.len();
        if total > MAX_COORDS {
            return Err(DataError::Format(format!("{file_name}: too many arc coordinates")));
        }
        arcs.push(pts);
    }
    let mut topo = Topology { arcs, transform, budget: MAX_COORDS };
    let objects = v.get("objects").and_then(Json::as_object).ok_or_else(|| DataError::Format(format!("{file_name}: no objects")))?;
    let mut out = Vec::new();
    for (name, obj) in objects.iter().take(MAX_OBJECTS) {
        let members: Vec<&Json> = match obj.get("type").and_then(Json::as_str) {
            Some("GeometryCollection") => obj.get("geometries").and_then(Json::as_array).map(|g| g.iter().collect()).unwrap_or_default(),
            _ => vec![obj],
        };
        let mut bad = 0usize;
        let features: Vec<Json> = members
            .into_iter()
            .map(|g| {
                let geometry = if g.get("type").and_then(Json::as_str).is_some_and(|t| t != "null") {
                    topo.geometry(g, 0).unwrap_or_else(|| {
                        bad += 1;
                        Json::Null
                    })
                } else {
                    Json::Null
                };
                let mut props = g.get("properties").and_then(Json::as_object).cloned().unwrap_or_else(Map::new);
                if let Some(id) = g.get("id")
                    && !props.contains_key("id")
                {
                    props.insert("id".into(), id.clone());
                }
                json!({"type": "Feature", "properties": props, "geometry": geometry})
            })
            .collect();
        let fc = json!({"type": "FeatureCollection", "features": features});
        let title = if objects.len() == 1 { stem.to_string() } else { format!("{stem} {name}") };
        let (ds, mut rep) = crate::io::read_geojson(&fc.to_string(), &title)?;
        if bad > 0 {
            rep.skipped += bad;
            rep.messages.push(format!("{bad} geometries had arcs that couldn't be read"));
        }
        out.push((ds, rep));
    }
    if objects.len() > MAX_OBJECTS
        && let Some((_, rep)) = out.first_mut()
    {
        rep.messages.push(format!("only the first {MAX_OBJECTS} objects are read"));
    }
    if out.is_empty() {
        return Err(DataError::Format(format!("{file_name}: no objects")));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Geometry, GeometryType};
    use rustgis_geom::Coord;

    // The TopoJSON specification's example: a point, a line and a polygon, quantized.
    const TOPO: &str = r#"{"type":"Topology","transform":{"scale":[0.0005000500050005,0.00010001000100010001],"translate":[100,0]},
"objects":{"example":{"type":"GeometryCollection","geometries":[
{"type":"Point","properties":{"prop0":"value0"},"coordinates":[4000,5000]},
{"type":"LineString","properties":{"prop0":"value0","prop1":0},"arcs":[0]},
{"type":"Polygon","properties":{"prop0":"value0","prop1":{"this":"that"}},"arcs":[[-2]]}]}},
"arcs":[[[4000,0],[1999,9999],[2000,-9999],[2000,9999]],[[0,0],[0,9999],[2000,0],[0,-9999],[-2000,0]]]}"#;

    #[test]
    fn reads_the_specification_example() {
        assert!(looks_like_topojson(TOPO));
        assert!(!looks_like_topojson(r#"{"type": "Feature", "properties": {"kind": "Topology"}}"#));
        // A multi-byte character across byte 512.
        let padded = format!("{{\"type\": \"Topology\", \"x\": \"{}é\"}}", "a".repeat(480));
        assert!(looks_like_topojson(&padded));
        let layers = read("example.topojson", TOPO).unwrap();
        assert_eq!(layers.len(), 1);
        let ds = &layers[0].0;
        assert_eq!(ds.name, "example");
        assert_eq!(ds.features.len(), 3);
        let Some(Geometry::Point(p)) = &ds.features[0].geometry else { panic!() };
        assert!((p.x - 102.0).abs() < 1e-3 && (p.y - 0.5).abs() < 1e-3, "{p:?}");
        let Some(Geometry::Line(l)) = &ds.features[1].geometry else { panic!() };
        assert_eq!(l[0].len(), 4);
        assert!((l[0][3].x - 105.0).abs() < 1e-3);
        // The polygon is arc 1 reversed: still a closed ring of 5.
        let Some(Geometry::Polygon(poly)) = &ds.features[2].geometry else { panic!() };
        assert_eq!(poly[0][0].len(), 5);
        assert_eq!(poly[0][0].first(), poly[0][0].last());
        assert!(poly[0][0].iter().all(|c: &Coord| (100.0 - 1e-3..=101.0 + 1e-3).contains(&c.x)), "{:?}", poly[0][0]);
        assert_ne!(ds.geometry_type, GeometryType::None);
    }

    #[test]
    fn json_topologies_read_every_object() {
        let two = r#"{"type":"Topology","objects":{"a":{"type":"Point","coordinates":[1,2]},"b":{"type":"Point","coordinates":[3,4]}},"arcs":[]}"#;
        let layers = crate::io::read_layers("us.json", two.as_bytes()).unwrap();
        assert_eq!(layers.len(), 2);
        // The first alone says what else the file holds.
        let (ds, rep) = crate::io::read_bytes("us.json", two.as_bytes()).unwrap();
        assert_eq!(ds.features.len(), 1);
        assert!(rep.messages.iter().any(|m| m.contains("also holds")), "{:?}", rep.messages);
    }

    #[test]
    fn junk_is_refused() {
        assert!(read("x.topojson", "{}").is_err());
        assert!(read("x.topojson", "not json").is_err());
        let bad_arc = r#"{"type":"Topology","objects":{"a":{"type":"LineString","arcs":[7]}},"arcs":[]}"#;
        let (_, rep) = read("x.topojson", bad_arc).unwrap().remove(0);
        assert_eq!(rep.skipped, 1);
        for cut in (0..TOPO.len()).step_by(11) {
            let _ = read("x.topojson", TOPO.get(..cut).unwrap_or(""));
        }
    }
}
