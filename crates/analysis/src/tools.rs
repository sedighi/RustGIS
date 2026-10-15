//! The tools.

use geo::{BooleanOps, Buffer, ConvexHull, InteriorPoint, Intersects, MultiPolygon, Orient, Simplify, orient::Direction};
use rustgis_data::raster::Crs;
use rustgis_data::{Coord, Dataset, Field, FieldType, Geometry, GeometryType, Value};
use rustgis_geom::{geodesic_length_m, geodesic_ring_area_m2};
use serde_json::Value as Json;

use crate::convert::{from_line, from_multipolygon, from_point, to_geo, to_multiline, to_multipolygon};
use crate::{Inputs, Output, ParamKind as K, ParamSpec as P, ToolResult, ToolSpec};

const UNITS: &[&str] = &["Kilometers", "Meters", "Miles"];

pub static TOOLS: &[ToolSpec] = &[
    ToolSpec {
        id: "buffer",
        name: "Buffer",
        toolbox: "Analysis › Proximity",
        summary: "Creates polygons around input features at a given distance.",
        params: &[
            P { name: "input", label: "Input Features", kind: K::Layer, required: true, default: "", help: "" },
            P { name: "distance", label: "Distance", kind: K::Number, required: true, default: "100", help: "" },
            P { name: "units", label: "Units", kind: K::Choice(UNITS), required: false, default: "\"Kilometers\"", help: "" },
            P { name: "dissolve", label: "Dissolve", kind: K::Choice(&["None", "All"]), required: false, default: "\"None\"", help: "" },
            P { name: "output", label: "Output Name", kind: K::Text, required: false, default: "", help: "" },
        ],
        run: buffer,
    },
    ToolSpec {
        id: "clip",
        name: "Clip",
        toolbox: "Analysis › Extract",
        summary: "Extracts the parts of input features that fall inside clip polygons.",
        params: &[
            P { name: "input", label: "Input Features", kind: K::Layer, required: true, default: "", help: "" },
            P { name: "clip", label: "Clip Features", kind: K::PolygonLayer, required: true, default: "", help: "" },
            P { name: "output", label: "Output Name", kind: K::Text, required: false, default: "", help: "" },
        ],
        run: clip,
    },
    ToolSpec {
        id: "erase",
        name: "Erase",
        toolbox: "Analysis › Overlay",
        summary: "Removes the parts of polygon features that overlap erase polygons.",
        params: &[
            P { name: "input", label: "Input Features", kind: K::PolygonLayer, required: true, default: "", help: "" },
            P { name: "erase", label: "Erase Features", kind: K::PolygonLayer, required: true, default: "", help: "" },
            P { name: "output", label: "Output Name", kind: K::Text, required: false, default: "", help: "" },
        ],
        run: erase,
    },
    ToolSpec {
        id: "dissolve",
        name: "Dissolve",
        toolbox: "Data Management › Generalization",
        summary: "Merges polygons that share a field value into one feature per value.",
        params: &[
            P { name: "input", label: "Input Features", kind: K::PolygonLayer, required: true, default: "", help: "" },
            P { name: "field", label: "Dissolve Field", kind: K::Field, required: false, default: "", help: "" },
            P { name: "output", label: "Output Name", kind: K::Text, required: false, default: "", help: "" },
        ],
        run: dissolve,
    },
    ToolSpec {
        id: "convexHull",
        name: "Minimum Bounding Geometry",
        toolbox: "Data Management › Features",
        summary: "Encloses each feature, or all features, in its convex hull.",
        params: &[
            P { name: "input", label: "Input Features", kind: K::Layer, required: true, default: "", help: "" },
            P { name: "group", label: "Group Option", kind: K::Choice(&["All", "None"]), required: false, default: "\"All\"", help: "" },
            P { name: "output", label: "Output Name", kind: K::Text, required: false, default: "", help: "" },
        ],
        run: convex_hull,
    },
    ToolSpec {
        id: "featureToPoint",
        name: "Feature To Point",
        toolbox: "Data Management › Features",
        summary: "Creates a point inside each input feature.",
        params: &[
            P { name: "input", label: "Input Features", kind: K::Layer, required: true, default: "", help: "" },
            P { name: "output", label: "Output Name", kind: K::Text, required: false, default: "", help: "" },
        ],
        run: feature_to_point,
    },
    ToolSpec {
        id: "xyTableToPoint",
        name: "XY Table To Point",
        toolbox: "Data Management › Features",
        summary: "Creates points from a table's x and y coordinate fields.",
        params: &[
            P { name: "input", label: "Input Table", kind: K::Layer, required: true, default: "", help: "" },
            P {
                name: "x",
                label: "X Field",
                kind: K::CoordinateField,
                required: true,
                default: "",
                help: "Longitude or easting: numbers, or text holding them. A single comma is the decimal separator (1,234 is 1.234).",
            },
            P {
                name: "y",
                label: "Y Field",
                kind: K::CoordinateField,
                required: true,
                default: "",
                help: "Latitude or northing, read as the X field is.",
            },
            P {
                name: "crs",
                label: "Coordinate System",
                kind: K::Text,
                required: false,
                default: "\"EPSG:4326\"",
                help: "The coordinates' system, as EPSG:<code> or WKT; longitude/latitude (EPSG:4326) when empty.",
            },
            P { name: "output", label: "Output Name", kind: K::Text, required: false, default: "", help: "" },
        ],
        run: xy_table_to_point,
    },
    ToolSpec {
        id: "simplify",
        name: "Simplify",
        toolbox: "Cartography › Generalization",
        summary: "Removes vertices that deviate less than a tolerance (Douglas–Peucker).",
        params: &[
            P { name: "input", label: "Input Features", kind: K::Layer, required: true, default: "", help: "" },
            P { name: "tolerance", label: "Tolerance (km)", kind: K::Number, required: true, default: "25", help: "" },
            P { name: "output", label: "Output Name", kind: K::Text, required: false, default: "", help: "" },
        ],
        run: simplify,
    },
    ToolSpec {
        id: "selectByLocation",
        name: "Select Layer By Location",
        toolbox: "Data Management › Layers and Table Views",
        summary: "Selects features that intersect features in another layer.",
        params: &[
            P { name: "input", label: "Input Features", kind: K::Layer, required: true, default: "", help: "" },
            P { name: "select", label: "Selecting Features", kind: K::Layer, required: true, default: "", help: "" },
            P { name: "distance", label: "Search Distance (km)", kind: K::Number, required: false, default: "0", help: "" },
            P { name: "invert", label: "Invert", kind: K::Choice(&["No", "Yes"]), required: false, default: "\"No\"", help: "" },
        ],
        run: select_by_location,
    },
    ToolSpec {
        id: "selectByAttributes",
        name: "Select Layer By Attribute",
        toolbox: "Data Management › Layers and Table Views",
        summary: "Selects features with a SQL where clause, e.g. pop_est > 1000000.",
        params: &[
            P { name: "input", label: "Input Rows", kind: K::Layer, required: true, default: "", help: "" },
            P { name: "where", label: "Where Clause", kind: K::Text, required: true, default: "", help: "" },
        ],
        run: select_by_attributes,
    },
    ToolSpec {
        id: "countPoints",
        name: "Summarize Within",
        toolbox: "Analysis › Summarize",
        summary: "Counts the points that fall inside each polygon.",
        params: &[
            P { name: "polygons", label: "Input Polygons", kind: K::PolygonLayer, required: true, default: "", help: "" },
            P { name: "points", label: "Input Summary Features", kind: K::PointLayer, required: true, default: "", help: "" },
            P { name: "output", label: "Output Name", kind: K::Text, required: false, default: "", help: "" },
        ],
        run: count_points,
    },
    ToolSpec {
        id: "calculateGeometry",
        name: "Calculate Geometry Attributes",
        toolbox: "Data Management › Features",
        summary: "Adds geodesic area (km²), length (km) or centroid coordinates as fields.",
        params: &[P { name: "input", label: "Input Features", kind: K::Layer, required: true, default: "", help: "" }],
        run: calculate_geometry,
    },
    ToolSpec {
        id: "summaryStatistics",
        name: "Summary Statistics",
        toolbox: "Analysis › Statistics",
        summary: "Count, sum, mean, min, max and standard deviation of a field, optionally per case field.",
        params: &[
            P { name: "input", label: "Input Table", kind: K::Layer, required: true, default: "", help: "" },
            P { name: "field", label: "Statistics Field", kind: K::NumericField, required: true, default: "", help: "" },
            P { name: "case", label: "Case Field", kind: K::Field, required: false, default: "", help: "" },
        ],
        run: summary_statistics,
    },
    ToolSpec {
        id: "exportFeatures",
        name: "Export Features",
        toolbox: "Conversion",
        summary: "Copies the (selected) features of a layer to a new layer.",
        params: &[
            P { name: "input", label: "Input Features", kind: K::Layer, required: true, default: "", help: "" },
            P { name: "where", label: "Expression", kind: K::Text, required: false, default: "", help: "" },
            P { name: "output", label: "Output Name", kind: K::Text, required: false, default: "", help: "" },
        ],
        run: export_features,
    },
];

// ---------------------------------------------------------------------------------------------
// helpers

fn s<'a>(p: &'a Json, k: &str) -> &'a str {
    p.get(k).and_then(Json::as_str).unwrap_or("")
}

fn num(p: &Json, k: &str) -> Option<f64> {
    p.get(k).and_then(|v| v.as_f64().or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))).filter(|f: &f64| f.is_finite())
}

fn input<'a>(inp: &'a dyn Inputs, p: &Json, k: &str) -> Result<&'a Dataset, String> {
    // A number is a standalone table's id, and only that; text is a name.
    if let Some(n) = p.get(k).filter(|v| v.is_number()) {
        let table = n.as_u64().and_then(|id| u32::try_from(id).ok()).and_then(|id| inp.table(id));
        return table.ok_or_else(|| format!("no standalone table with id {n}"));
    }
    let name = s(p, k);
    inp.dataset(name).ok_or_else(|| format!("no layer named '{name}'"))
}

/// Selected rows, or all rows when nothing is selected.
fn rows(inp: &dyn Inputs, p: &Json, k: &str, ds: &Dataset) -> Vec<usize> {
    let sel = inp.selection(s(p, k));
    if sel.is_empty() { (0..ds.features.len()).collect() } else { sel.iter().copied().filter(|r| *r < ds.features.len()).collect() }
}

fn out_name(p: &Json, default: String) -> String {
    let n = s(p, "output").trim();
    if n.is_empty() { default } else { n.to_string() }
}

/// Scale factor of Web Mercator at a latitude (metres on the map per metre on the ground).
fn merc_scale(lat: f64) -> f64 {
    1.0 / lat.clamp(-85.0, 85.0).to_radians().cos().max(0.01)
}

fn feature_lat(g: &Geometry) -> f64 {
    g.label_point().map(|c| c.y).unwrap_or(0.0)
}

fn to_metres(d: f64, units: &str) -> f64 {
    match units.to_ascii_lowercase().as_str() {
        "meters" | "metres" | "m" => d,
        "miles" | "mi" => d * 1609.344,
        _ => d * 1000.0,
    }
}

fn result(ds: Dataset, msg: String) -> Result<ToolResult, String> {
    Ok(ToolResult { output: Output::Layer(ds), messages: vec![msg] })
}

/// A copy of a feature's attributes for an output with the same schema.
fn copy_schema(ds: &Dataset, name: String, g: GeometryType) -> Dataset {
    Dataset::new(name, g, ds.fields.clone())
}

/// Union of polygons; rings are oriented first because the overlay infers its fill rule from
/// the first ring's winding, and input files mix orientations.
fn union_all<'a>(polys: impl IntoIterator<Item = &'a geo::Polygon<f64>>) -> MultiPolygon<f64> {
    let oriented: Vec<geo::Polygon<f64>> = polys.into_iter().map(|p| p.orient(Direction::Default)).collect();
    geo::unary_union(oriented.iter())
}

// ---------------------------------------------------------------------------------------------
// tools

fn buffer(inp: &dyn Inputs, p: &Json) -> Result<ToolResult, String> {
    let ds = input(inp, p, "input")?;
    let d = num(p, "distance").ok_or("Distance must be a number")?;
    let metres = to_metres(d, s(p, "units"));
    if metres.abs() > 5_000_000.0 {
        return Err("Distance is too large (limit 5000 km)".into());
    }
    let mut out = copy_schema(ds, out_name(p, format!("{}_Buffer", ds.name)), GeometryType::Polygon);
    let mut all: Vec<MultiPolygon<f64>> = Vec::new();
    for r in rows(inp, p, "input", ds) {
        let Some(f) = ds.features.get(r) else { continue };
        let Some(g) = &f.geometry else { continue };
        let dist = metres * merc_scale(feature_lat(g));
        let mp = match to_geo(g) {
            geo::Geometry::MultiPolygon(m) => m.buffer(dist),
            geo::Geometry::MultiLineString(m) => m.buffer(dist),
            geo::Geometry::MultiPoint(m) => m.buffer(dist),
            geo::Geometry::Point(pt) => pt.buffer(dist),
            _ => continue,
        };
        if s(p, "dissolve") == "All" {
            all.push(mp);
        } else if let Some(geom) = from_multipolygon(&mp) {
            out.push(Some(geom), f.attrs.clone());
        }
    }
    if !all.is_empty() {
        out.fields.clear();
        let u = union_all(all.iter().flat_map(|m| m.0.iter()));
        if let Some(geom) = from_multipolygon(&u) {
            out.push(Some(geom), vec![]);
        }
    }
    let n = out.features.len();
    result(out, format!("Buffer: {n} features at {d} {}", if s(p, "units").is_empty() { "Kilometers" } else { s(p, "units") }))
}

/// Union of a polygon layer's (selected) features.
fn union_of(inp: &dyn Inputs, p: &Json, k: &str) -> Result<MultiPolygon<f64>, String> {
    let ds = input(inp, p, k)?;
    if ds.geometry_type != GeometryType::Polygon {
        return Err(format!("'{}' is not a polygon layer", ds.name));
    }
    let polys: Vec<MultiPolygon<f64>> =
        rows(inp, p, k, ds).iter().filter_map(|r| ds.features.get(*r)?.geometry.as_ref()).filter_map(to_multipolygon).collect();
    Ok(union_all(polys.iter().flat_map(|m| m.0.iter())))
}

fn clip(inp: &dyn Inputs, p: &Json) -> Result<ToolResult, String> {
    let ds = input(inp, p, "input")?;
    let mask = union_of(inp, p, "clip")?;
    let mut out = copy_schema(ds, out_name(p, format!("{}_Clip", ds.name)), ds.geometry_type);
    for r in rows(inp, p, "input", ds) {
        let Some(f) = ds.features.get(r) else { continue };
        let Some(g) = &f.geometry else { continue };
        let clipped = match g {
            Geometry::Polygon(_) => to_multipolygon(g).and_then(|m| from_multipolygon(&m.intersection(&mask))),
            Geometry::Line(_) => to_multiline(g).and_then(|m| from_line(&mask.clip(&m, false))),
            Geometry::Point(_) | Geometry::MultiPoint(_) => {
                let pts: Vec<Coord> = match g {
                    Geometry::Point(c) => vec![*c],
                    Geometry::MultiPoint(v) => v.clone(),
                    _ => vec![],
                };
                let inside: Vec<Coord> = pts
                    .into_iter()
                    .filter(|c| {
                        let m = rustgis_geom::lonlat_to_merc(*c);
                        mask.intersects(&geo::Point::new(m.x, m.y))
                    })
                    .collect();
                match inside.len() {
                    0 => None,
                    1 => inside.first().map(|c| Geometry::Point(*c)),
                    _ => Some(Geometry::MultiPoint(inside)),
                }
            }
        };
        if let Some(c) = clipped {
            out.push(Some(c), f.attrs.clone());
        }
    }
    let n = out.features.len();
    result(out, format!("Clip: {n} features kept"))
}

fn erase(inp: &dyn Inputs, p: &Json) -> Result<ToolResult, String> {
    let ds = input(inp, p, "input")?;
    let mask = union_of(inp, p, "erase")?;
    let mut out = copy_schema(ds, out_name(p, format!("{}_Erase", ds.name)), GeometryType::Polygon);
    for r in rows(inp, p, "input", ds) {
        let Some(f) = ds.features.get(r) else { continue };
        if let Some(rest) = f.geometry.as_ref().and_then(to_multipolygon).and_then(|m| from_multipolygon(&m.difference(&mask))) {
            out.push(Some(rest), f.attrs.clone());
        }
    }
    let n = out.features.len();
    result(out, format!("Erase: {n} features remain"))
}

fn dissolve(inp: &dyn Inputs, p: &Json) -> Result<ToolResult, String> {
    let ds = input(inp, p, "input")?;
    if ds.geometry_type != GeometryType::Polygon {
        return Err("Dissolve needs a polygon layer".into());
    }
    let field = s(p, "field").trim();
    let fi = if field.is_empty() { None } else { Some(ds.field_index(field).ok_or_else(|| format!("no field named '{field}'"))?) };
    let mut groups: Vec<(Value, Vec<MultiPolygon<f64>>)> = Vec::new();
    for r in rows(inp, p, "input", ds) {
        let Some(mp) = ds.features.get(r).and_then(|f| f.geometry.as_ref()).and_then(to_multipolygon) else { continue };
        let key = fi.map(|fi| ds.value(r, fi).clone()).unwrap_or(Value::Null);
        match groups.iter_mut().find(|(k, _)| *k == key) {
            Some(g) => g.1.push(mp),
            None => groups.push((key, vec![mp])),
        }
    }
    let fields = match fi.and_then(|i| ds.fields.get(i)) {
        Some(f) => vec![f.clone(), Field::new("count", FieldType::Integer)],
        None => vec![Field::new("count", FieldType::Integer)],
    };
    let mut out = Dataset::new(out_name(p, format!("{}_Dissolve", ds.name)), GeometryType::Polygon, fields);
    for (key, parts) in groups {
        let n = parts.len() as i64;
        let u = union_all(parts.iter().flat_map(|m| m.0.iter()));
        let attrs = if fi.is_some() { vec![key, Value::Int(n)] } else { vec![Value::Int(n)] };
        out.push(from_multipolygon(&u), attrs);
    }
    let n = out.features.len();
    result(out, format!("Dissolve: {n} features"))
}

fn convex_hull(inp: &dyn Inputs, p: &Json) -> Result<ToolResult, String> {
    let ds = input(inp, p, "input")?;
    let each = s(p, "group") == "None";
    let mut out = if each {
        copy_schema(ds, out_name(p, format!("{}_Hull", ds.name)), GeometryType::Polygon)
    } else {
        Dataset::new(out_name(p, format!("{}_Hull", ds.name)), GeometryType::Polygon, vec![Field::new("count", FieldType::Integer)])
    };
    let mut pts: Vec<geo::Point<f64>> = Vec::new();
    for r in rows(inp, p, "input", ds) {
        let Some(f) = ds.features.get(r) else { continue };
        let Some(g) = &f.geometry else { continue };
        let mut these = Vec::new();
        g.for_each_coord(|c| {
            let m = rustgis_geom::lonlat_to_merc(c);
            these.push(geo::Point::new(m.x, m.y));
        });
        if each {
            let hull = geo::MultiPoint::new(these).convex_hull();
            out.push(from_multipolygon(&MultiPolygon::new(vec![hull])), f.attrs.clone());
        } else {
            pts.extend(these);
        }
    }
    if !each {
        let n = ds.features.len() as i64;
        let hull = geo::MultiPoint::new(pts).convex_hull();
        out.push(from_multipolygon(&MultiPolygon::new(vec![hull])), vec![Value::Int(n)]);
    }
    let n = out.features.len();
    result(out, format!("Minimum Bounding Geometry: {n} hulls"))
}

fn feature_to_point(inp: &dyn Inputs, p: &Json) -> Result<ToolResult, String> {
    let ds = input(inp, p, "input")?;
    let mut out = copy_schema(ds, out_name(p, format!("{}_Points", ds.name)), GeometryType::Point);
    for r in rows(inp, p, "input", ds) {
        let Some(f) = ds.features.get(r) else { continue };
        let pt = match &f.geometry {
            Some(g @ Geometry::Polygon(_)) => to_multipolygon(g).and_then(|m| m.interior_point()).map(|p| from_point(&p)),
            Some(g) => g.label_point().map(Geometry::Point),
            None => None,
        };
        out.push(pt, f.attrs.clone());
    }
    let n = out.features.len();
    result(out, format!("Feature To Point: {n} points"))
}

/// A coordinate from a table cell: a number, or text holding one, written with a decimal point
/// or (when it has no point) a single decimal comma, as spreadsheets in many languages do. A comma
/// never separates thousands: `1,234` is 1.234 (refusing it would refuse `1234,567` too), and
/// `1,234.5` is not a number.
fn coordinate(v: &Value) -> Option<f64> {
    let n = match v {
        Value::Int(i) => *i as f64,
        Value::Float(f) => *f,
        Value::Text(t) => {
            let t = t.trim();
            match t.parse::<f64>() {
                Ok(n) => n,
                Err(_) if !t.contains('.') && t.matches(',').count() == 1 => t.replace(',', ".").parse().ok()?,
                Err(_) => return None,
            }
        }
        Value::Null | Value::Bool(_) => return None,
    };
    n.is_finite().then_some(n)
}

/// The rows looked at to tell whether a text field holds numbers.
const COORDINATE_SAMPLE: usize = 1000;

/// Whether a field can hold coordinates: a numeric field, or a text field where one of the
/// first rows reads as a number, as [`coordinate`] reads it. The Tools pane lists these fields
/// for a coordinate parameter, so a table without any offers none.
pub fn holds_coordinates(ds: &Dataset, field: usize) -> bool {
    match ds.fields.get(field).map(|f| f.ty) {
        Some(FieldType::Text) => (0..ds.features.len().min(COORDINATE_SAMPLE)).any(|r| coordinate(ds.value(r, field)).is_some()),
        Some(ty) => ty.is_numeric(),
        None => false,
    }
}

fn xy_table_to_point(inp: &dyn Inputs, p: &Json) -> Result<ToolResult, String> {
    let ds = input(inp, p, "input")?;
    let field = |k: &str| {
        let name = s(p, k).trim();
        ds.field_index(name).ok_or_else(|| format!("no field named '{name}'"))
    };
    let (xi, yi) = (field("x")?, field("y")?);
    let crs = match s(p, "crs").trim() {
        "" => Crs::LonLat,
        t => Crs::parse(t)?,
    };
    let to = crs.transformer()?;
    let mut out = copy_schema(ds, out_name(p, format!("{}_Points", ds.name)), GeometryType::Point);
    // Rows with numbers in both fields, and those of them that land on the globe.
    let (mut read, mut placed, mut total) = (0usize, 0usize, 0usize);
    for r in rows(inp, p, "input", ds) {
        let Some(f) = ds.features.get(r) else { continue };
        total += 1;
        let xy = coordinate(ds.value(r, xi)).zip(coordinate(ds.value(r, yi)));
        read += usize::from(xy.is_some());
        let pt = xy.and_then(|(x, y)| to.to_lonlat(Coord::new(x, y))).filter(|c| c.is_finite() && c.x.abs() <= 180.0 && c.y.abs() <= 90.0);
        placed += usize::from(pt.is_some());
        // A row without a usable position stays in the layer's table, without a point.
        out.push_next(pt.map(Geometry::Point), f.attrs.clone());
    }
    if placed == 0 {
        return Err(if read == 0 {
            format!("{}: no row has numbers in both '{}' and '{}'", ds.name, s(p, "x").trim(), s(p, "y").trim())
        } else {
            format!(
                "{}: the coordinates aren't in {}: give their coordinate system (\"crs\", such as EPSG:2263) to place them",
                ds.name,
                crs.label()
            )
        });
    }
    let mut messages = vec![format!("XY Table To Point: {placed} points")];
    if placed < total {
        messages.push(format!("{} of {total} rows have no usable coordinates; they are in the table without a point", total - placed));
    }
    Ok(ToolResult { output: Output::Layer(out), messages })
}

/// A geometry simplified (Douglas–Peucker) to a tolerance in metres.
pub fn simplify_geometry(g: &Geometry, tol_m: f64) -> Geometry {
    let eps = tol_m * merc_scale(feature_lat(g));
    let sg = match g {
        Geometry::Polygon(_) => to_multipolygon(g).and_then(|m| from_multipolygon(&m.simplify(eps))),
        Geometry::Line(_) => to_multiline(g).and_then(|m| from_line(&m.simplify(eps))),
        _ => None,
    };
    sg.unwrap_or_else(|| g.clone())
}

fn simplify(inp: &dyn Inputs, p: &Json) -> Result<ToolResult, String> {
    let ds = input(inp, p, "input")?;
    let tol = num(p, "tolerance").ok_or("Tolerance must be a number")?.abs() * 1000.0;
    let mut out = copy_schema(ds, out_name(p, format!("{}_Simplified", ds.name)), ds.geometry_type);
    let (mut before, mut after) = (0usize, 0usize);
    for r in rows(inp, p, "input", ds) {
        let Some(f) = ds.features.get(r) else { continue };
        let Some(g) = &f.geometry else { continue };
        before += g.coord_count();
        let eps = tol * merc_scale(feature_lat(g));
        let sg = match g {
            Geometry::Polygon(_) => to_multipolygon(g).and_then(|m| from_multipolygon(&m.simplify(eps))),
            Geometry::Line(_) => to_multiline(g).and_then(|m| from_line(&m.simplify(eps))),
            other => Some(other.clone()),
        };
        after += sg.as_ref().map(|g| g.coord_count()).unwrap_or(0);
        out.push(sg, f.attrs.clone());
    }
    result(out, format!("Simplify: {before} → {after} vertices"))
}

fn select_by_location(inp: &dyn Inputs, p: &Json) -> Result<ToolResult, String> {
    let ds = input(inp, p, "input")?;
    let sel_ds = input(inp, p, "select")?;
    let dist = num(p, "distance").unwrap_or(0.0).max(0.0) * 1000.0;
    let selectors: Vec<geo::Geometry<f64>> = rows(inp, p, "select", sel_ds)
        .iter()
        .filter_map(|r| sel_ds.features.get(*r)?.geometry.as_ref())
        .map(|g| {
            if dist > 0.0 {
                let d = dist * merc_scale(feature_lat(g));
                let b = match to_geo(g) {
                    geo::Geometry::MultiPolygon(m) => m.buffer(d),
                    geo::Geometry::MultiLineString(m) => m.buffer(d),
                    geo::Geometry::MultiPoint(m) => m.buffer(d),
                    geo::Geometry::Point(pt) => pt.buffer(d),
                    other => return other,
                };
                geo::Geometry::MultiPolygon(b)
            } else {
                to_geo(g)
            }
        })
        .collect();
    let invert = s(p, "invert") == "Yes";
    let hits: Vec<usize> = (0..ds.features.len())
        .filter(|r| {
            let Some(g) = ds.features.get(*r).and_then(|f| f.geometry.as_ref()) else {
                return false;
            };
            let gg = to_geo(g);
            let hit = selectors.iter().any(|s| s.intersects(&gg));
            hit != invert
        })
        .collect();
    let n = hits.len();
    Ok(ToolResult {
        output: Output::Selection { layer: ds.name.clone(), rows: hits },
        messages: vec![format!("Selected {n} features in {}", ds.name)],
    })
}

fn select_by_attributes(inp: &dyn Inputs, p: &Json) -> Result<ToolResult, String> {
    let ds = input(inp, p, "input")?;
    let hits = rustgis_data::query::select(s(p, "where"), ds)?;
    let n = hits.len();
    Ok(ToolResult {
        output: Output::Selection { layer: ds.name.clone(), rows: hits },
        messages: vec![format!("Selected {n} features in {}", ds.name)],
    })
}

fn count_points(inp: &dyn Inputs, p: &Json) -> Result<ToolResult, String> {
    let polys = input(inp, p, "polygons")?;
    let pts = input(inp, p, "points")?;
    let point_geoms: Vec<geo::Geometry<f64>> =
        rows(inp, p, "points", pts).iter().filter_map(|r| pts.features.get(*r)?.geometry.as_ref()).map(to_geo).collect();
    let mut out = polys.clone();
    out.name = out_name(p, format!("{}_Summarize", polys.name));
    out.source = None;
    let fi = out.ensure_field("point_count", FieldType::Integer);
    let mut total = 0i64;
    for f in &mut out.features {
        let n = match f.geometry.as_ref().and_then(to_multipolygon) {
            Some(mp) => point_geoms.iter().filter(|g| mp.intersects(*g)).count() as i64,
            None => 0,
        };
        total += n;
        if let Some(v) = f.attrs.get_mut(fi) {
            *v = Value::Int(n);
        }
    }
    result(out, format!("Summarize Within: {total} points counted"))
}

fn calculate_geometry(inp: &dyn Inputs, p: &Json) -> Result<ToolResult, String> {
    let ds = input(inp, p, "input")?;
    let mut out = ds.clone();
    match ds.geometry_type {
        GeometryType::Polygon => {
            let fa = out.ensure_field("area_km2", FieldType::Double);
            let fp = out.ensure_field("perimeter_km", FieldType::Double);
            for f in &mut out.features {
                let (mut a, mut per) = (0.0, 0.0);
                if let Some(Geometry::Polygon(polys)) = &f.geometry {
                    for rings in polys {
                        for (i, r) in rings.iter().enumerate() {
                            let ra = geodesic_ring_area_m2(r);
                            a += if i == 0 { ra } else { -ra };
                            per += geodesic_length_m(r);
                        }
                    }
                }
                if let Some(v) = f.attrs.get_mut(fa) {
                    *v = Value::Float((a / 1e4).round() / 100.0);
                }
                if let Some(v) = f.attrs.get_mut(fp) {
                    *v = Value::Float((per / 10.0).round() / 100.0);
                }
            }
        }
        GeometryType::Polyline => {
            let fl = out.ensure_field("length_km", FieldType::Double);
            for f in &mut out.features {
                let len: f64 = match &f.geometry {
                    Some(Geometry::Line(parts)) => parts.iter().map(|p| geodesic_length_m(p)).sum(),
                    _ => 0.0,
                };
                if let Some(v) = f.attrs.get_mut(fl) {
                    *v = Value::Float((len / 10.0).round() / 100.0);
                }
            }
        }
        _ => {
            let fx = out.ensure_field("point_x", FieldType::Double);
            let fy = out.ensure_field("point_y", FieldType::Double);
            for f in &mut out.features {
                let c = f.geometry.as_ref().and_then(|g| g.label_point());
                if let Some(v) = f.attrs.get_mut(fx) {
                    *v = c.map(|c| Value::Float(c.x)).unwrap_or(Value::Null);
                }
                if let Some(v) = f.attrs.get_mut(fy) {
                    *v = c.map(|c| Value::Float(c.y)).unwrap_or(Value::Null);
                }
            }
        }
    }
    Ok(ToolResult {
        output: Output::Update { layer: ds.name.clone(), dataset: out },
        messages: vec![format!("Calculated geometry attributes for {}", ds.name)],
    })
}

fn summary_statistics(inp: &dyn Inputs, p: &Json) -> Result<ToolResult, String> {
    let ds = input(inp, p, "input")?;
    let field = s(p, "field");
    let fi = ds.field_index(field).ok_or_else(|| format!("no field named '{field}'"))?;
    let case = s(p, "case").trim();
    let ci = if case.is_empty() { None } else { Some(ds.field_index(case).ok_or_else(|| format!("no field named '{case}'"))?) };
    let mut groups: Vec<(Value, Vec<f64>)> = Vec::new();
    for r in rows(inp, p, "input", ds) {
        let key = ci.map(|c| ds.value(r, c).clone()).unwrap_or(Value::Null);
        let v = ds.value(r, fi).as_f64();
        let g = match groups.iter_mut().position(|(k, _)| *k == key) {
            Some(i) => i,
            None => {
                groups.push((key, Vec::new()));
                groups.len() - 1
            }
        };
        if let (Some(v), Some(g)) = (v, groups.get_mut(g)) {
            g.1.push(v);
        }
    }
    let fname = ds.fields.get(fi).map(|f| f.name.clone()).unwrap_or_default();
    let mut fields = Vec::new();
    if let Some(c) = ci.and_then(|c| ds.fields.get(c)) {
        fields.push(c.clone());
    }
    for st in ["COUNT", "SUM", "MEAN", "MIN", "MAX", "STD"] {
        fields.push(Field::new(format!("{st}_{fname}"), if st == "COUNT" { FieldType::Integer } else { FieldType::Double }));
    }
    let mut out = Dataset::new(format!("{}_Statistics", ds.name), GeometryType::None, fields);
    groups.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    for (key, v) in groups {
        let n = v.len() as f64;
        let sum: f64 = v.iter().sum();
        let mean = if n > 0.0 { sum / n } else { 0.0 };
        let sd = if n > 0.0 { (v.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / n).sqrt() } else { 0.0 };
        let min = v.iter().copied().fold(f64::INFINITY, f64::min);
        let max = v.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        let mut row = Vec::new();
        if ci.is_some() {
            row.push(key);
        }
        let f = |x: f64| if x.is_finite() { Value::Float(x) } else { Value::Null };
        row.extend([Value::Int(n as i64), f(sum), f(mean), f(min), f(max), f(sd)]);
        out.push(None, row);
    }
    let n = out.features.len();
    Ok(ToolResult { output: Output::Table(out), messages: vec![format!("Summary Statistics: {n} rows")] })
}

fn export_features(inp: &dyn Inputs, p: &Json) -> Result<ToolResult, String> {
    let ds = input(inp, p, "input")?;
    let mut rs = rows(inp, p, "input", ds);
    let w = s(p, "where").trim();
    if !w.is_empty() {
        let e = rustgis_data::query::parse(w, ds)?;
        rs.retain(|r| rustgis_data::query::eval(&e, ds, *r));
    }
    let mut out = copy_schema(ds, out_name(p, format!("{}_Export", ds.name)), ds.geometry_type);
    for r in rs {
        if let Some(f) = ds.features.get(r) {
            out.push(f.geometry.clone(), f.attrs.clone());
        }
    }
    let n = out.features.len();
    result(out, format!("Export Features: {n} features"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    struct Map(HashMap<String, Dataset>);
    impl Inputs for Map {
        fn dataset(&self, layer: &str) -> Option<&Dataset> {
            self.0.get(layer)
        }
        fn selection(&self, _: &str) -> &[usize] {
            &[]
        }
    }

    fn sq(x: f64, y: f64, s: f64) -> Geometry {
        Geometry::Polygon(vec![vec![vec![
            Coord::new(x, y),
            Coord::new(x + s, y),
            Coord::new(x + s, y + s),
            Coord::new(x, y + s),
            Coord::new(x, y),
        ]]])
    }

    fn inputs() -> Map {
        let mut polys = Dataset::new("polys", GeometryType::Polygon, vec![Field::new("k", FieldType::Text)]);
        polys.push(Some(sq(0.0, 0.0, 2.0)), vec![Value::Text("a".into())]);
        polys.push(Some(sq(1.0, 0.0, 2.0)), vec![Value::Text("a".into())]);
        polys.push(Some(sq(10.0, 10.0, 1.0)), vec![Value::Text("b".into())]);
        let mut pts = Dataset::new("pts", GeometryType::Point, vec![Field::new("v", FieldType::Double)]);
        for (x, y, v) in [(0.5, 0.5, 1.0), (1.5, 1.5, 2.0), (50.0, 50.0, 3.0)] {
            pts.push(Some(Geometry::Point(Coord::new(x, y))), vec![Value::Float(v)]);
        }
        Map(HashMap::from([("polys".to_string(), polys), ("pts".to_string(), pts)]))
    }

    fn layer(r: ToolResult) -> Dataset {
        match r.output {
            Output::Layer(d) | Output::Table(d) | Output::Update { dataset: d, .. } => d,
            Output::Selection { .. } | Output::Layers(_) => panic!("expected a dataset"),
        }
    }

    #[test]
    fn all_tools_run() {
        let m = inputs();
        let j = |v: serde_json::Value| v;
        let b = layer(crate::run("buffer", &m, &j(serde_json::json!({"input": "pts", "distance": 50}))).unwrap());
        assert_eq!(b.features.len(), 3);
        let a = b.features[0].geometry.as_ref().unwrap().extent();
        assert!((a.width() - 0.9).abs() < 0.1, "≈100 km wide at the equator: {}", a.width());
        let d = layer(crate::run("dissolve", &m, &j(serde_json::json!({"input": "polys", "field": "k"}))).unwrap());
        assert_eq!(d.features.len(), 2);
        let c = layer(crate::run("clip", &m, &j(serde_json::json!({"input": "pts", "clip": "polys"}))).unwrap());
        assert_eq!(c.features.len(), 2);
        let cnt = layer(crate::run("countPoints", &m, &j(serde_json::json!({"polygons": "polys", "points": "pts"}))).unwrap());
        assert_eq!(cnt.value(0, 1), &Value::Int(2));
        let st = layer(crate::run("summaryStatistics", &m, &j(serde_json::json!({"input": "pts", "field": "v"}))).unwrap());
        assert_eq!(st.value(0, 2), &Value::Float(2.0));
        match crate::run("selectByLocation", &m, &j(serde_json::json!({"input": "pts", "select": "polys"}))).unwrap().output {
            Output::Selection { rows, .. } => assert_eq!(rows, vec![0, 1]),
            _ => panic!(),
        }
        for id in ["convexHull", "featureToPoint", "simplify", "calculateGeometry", "exportFeatures", "erase"] {
            let r = crate::run(id, &m, &j(serde_json::json!({"input": "polys", "erase": "polys"})));
            assert!(r.is_ok(), "{id}: {r:?}");
        }
    }

    #[test]
    fn dissolved_buffers_merge() {
        let mut pts = Dataset::new("p", GeometryType::Point, vec![]);
        pts.push(Some(Geometry::Point(Coord::new(0.0, 0.0))), vec![]);
        pts.push(Some(Geometry::Point(Coord::new(0.5, 0.0))), vec![]);
        let m = Map(HashMap::from([("p".to_string(), pts)]));
        let d = layer(crate::run("buffer", &m, &serde_json::json!({"input": "p", "distance": 50, "dissolve": "All"})).unwrap());
        match d.features[0].geometry.as_ref().unwrap() {
            Geometry::Polygon(polys) => assert_eq!(polys.len(), 1, "overlapping buffers merge into one polygon"),
            _ => panic!(),
        }
    }

    #[test]
    fn xy_table_to_point_places_rows_by_their_coordinates() {
        let fields = vec![Field::new("name", FieldType::Text), Field::new("lon", FieldType::Text), Field::new("lat", FieldType::Double)];
        let mut stops = Dataset::new("stops", GeometryType::None, fields);
        for (name, lon, lat) in [
            // A decimal comma, a decimal point with spaces around, and an integer in text.
            ("comma", Value::Text("-74,08".into()), Value::Float(4.6)),
            ("point", Value::Text(" -75.5 ".into()), Value::Float(6.25)),
            ("whole", Value::Text("-76".into()), Value::Int(3)),
            // No number, two commas, nothing at all, and off the globe: rows without a point.
            ("words", Value::Text("n/a".into()), Value::Float(1.0)),
            ("commas", Value::Text("1,234,5".into()), Value::Float(1.0)),
            ("empty", Value::Null, Value::Null),
            ("far", Value::Text("200".into()), Value::Float(0.0)),
        ] {
            stops.push(None, vec![Value::Text(name.into()), lon, lat]);
        }
        // The fields offered for coordinates: those holding numbers, as text or not.
        assert_eq!([0, 1, 2, 3].map(|i| holds_coordinates(&stops, i)), [false, true, true, false]);
        // A single comma is the decimal separator, never one for thousands.
        assert_eq!(["1,234", "1234,567", "1,234.5"].map(|t| coordinate(&Value::Text(t.into()))), [Some(1.234), Some(1234.567), None]);
        // Web Mercator metres: 10° east on the equator.
        let fields = vec![Field::new("e", FieldType::Double), Field::new("n", FieldType::Double)];
        let mut metres = Dataset::new("metres", GeometryType::None, fields);
        metres.push(None, vec![Value::Float(1_113_194.9), Value::Float(0.0)]);
        let m = Map(HashMap::from([("stops".to_string(), stops), ("metres".to_string(), metres)]));
        let run = |p: serde_json::Value| crate::run("xyTableToPoint", &m, &p);

        let r = run(serde_json::json!({"input": "stops", "x": "lon", "y": "lat"})).unwrap();
        assert_eq!(r.messages.len(), 2, "{:?}", r.messages);
        let out = layer(r);
        assert_eq!((out.name.as_str(), out.geometry_type), ("stops_Points", GeometryType::Point));
        assert_eq!((out.fields.len(), out.features.len()), (3, 7));
        let points: Vec<Option<(f64, f64)>> = out
            .features
            .iter()
            .map(|f| match &f.geometry {
                Some(Geometry::Point(c)) => Some((c.x, c.y)),
                _ => None,
            })
            .collect();
        assert_eq!(points, [Some((-74.08, 4.6)), Some((-75.5, 6.25)), Some((-76.0, 3.0)), None, None, None, None]);
        assert_eq!(out.value(3, 0), &Value::Text("words".into()), "rows without a point keep their attributes");

        // Another coordinate system, and the output's name.
        let out = layer(run(serde_json::json!({"input": "metres", "x": "e", "y": "n", "crs": "EPSG:3857", "output": "East"})).unwrap());
        assert_eq!(out.name, "East");
        match out.features.first().and_then(|f| f.geometry.as_ref()) {
            Some(Geometry::Point(c)) => assert!((c.x - 10.0).abs() < 1e-6 && c.y.abs() < 1e-6, "{c:?}"),
            g => panic!("{g:?}"),
        }

        // Metres read as longitude/latitude, fields without numbers, and junk parameters.
        let e = run(serde_json::json!({"input": "metres", "x": "e", "y": "n"})).unwrap_err();
        assert!(e.contains("coordinate system"), "{e}");
        let e = run(serde_json::json!({"input": "stops", "x": "name", "y": "lat"})).unwrap_err();
        assert!(e.contains("no row has numbers"), "{e}");
        for p in [
            serde_json::json!({"input": "stops", "x": "nope", "y": "lat"}),
            serde_json::json!({"input": "stops", "x": "lon"}),
            serde_json::json!({"input": "stops", "x": "lon", "y": "lat", "crs": "EPSG:99999999"}),
            serde_json::json!({"input": "stops", "x": "lon", "y": "lat", "crs": "somewhere"}),
            serde_json::json!({"input": "nope", "x": "lon", "y": "lat"}),
        ] {
            assert!(run(p.clone()).is_err(), "{p}");
        }
    }

    #[test]
    fn hostile_params_never_panic() {
        let m = inputs();
        let junk = [
            serde_json::json!({"input": "pts", "x": "v", "y": "v", "crs": "((", "output": 7}),
            serde_json::json!(null),
            serde_json::json!({}),
            serde_json::json!({"input": 5, "distance": "x"}),
            serde_json::json!({"input": "polys", "distance": 1e308, "tolerance": -1, "field": "nope", "where": "(((", "clip": "pts"}),
            serde_json::json!({"input": "pts", "polygons": "pts", "points": "polys", "select": "pts", "distance": -5}),
        ];
        for t in TOOLS {
            for p in &junk {
                let _ = crate::run(t.id, &m, p);
            }
        }
    }

    #[test]
    fn select_by_location_invert_excludes_features_without_geometry() {
        let mut polys = Dataset::new("polys", GeometryType::Polygon, vec![]);
        polys.push(Some(sq(0.0, 0.0, 2.0)), vec![]);
        let mut pts = Dataset::new("pts", GeometryType::Point, vec![]);
        pts.push(Some(Geometry::Point(Coord::new(1.0, 1.0))), vec![]); // inside
        pts.push(Some(Geometry::Point(Coord::new(10.0, 10.0))), vec![]); // outside
        pts.push(None, vec![]); // no geometry (unplaced record)
        let m = Map(HashMap::from([("polys".to_string(), polys), ("pts".to_string(), pts)]));

        let res_normal = crate::run("selectByLocation", &m, &serde_json::json!({"input": "pts", "select": "polys"})).unwrap();
        match res_normal.output {
            Output::Selection { rows, .. } => assert_eq!(rows, vec![0], "normal select should only select intersecting feature"),
            _ => panic!("expected selection"),
        }

        let res_inverted =
            crate::run("selectByLocation", &m, &serde_json::json!({"input": "pts", "select": "polys", "invert": "Yes"})).unwrap();
        match res_inverted.output {
            Output::Selection { rows, .. } => {
                assert_eq!(rows, vec![1], "inverted select should exclude unplaced records with null geometry")
            }
            _ => panic!("expected selection"),
        }
    }
}
