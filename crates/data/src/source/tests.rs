use super::*;
use crate::{Dataset, Field, FieldType, Geometry, Value, wkb};
use rustgis_geom::{Coord, lonlat_to_merc};
use std::path::PathBuf;
use std::sync::Arc;

/// 30 × 20 points over the world, named and numbered.
fn points() -> Vec<(Coord, String, i64)> {
    let mut v = Vec::new();
    for i in 0..30 {
        for j in 0..20 {
            let c = Coord::new(-174.0 + 12.0 * i as f64, -76.0 + 8.0 * j as f64);
            v.push((c, format!("p{i}_{j}"), (i * 20 + j) as i64));
        }
    }
    v
}

fn in_box(e: &Extent) -> usize {
    points().iter().filter(|(c, ..)| e.contains(*c)).count()
}

fn dir() -> PathBuf {
    let d = std::env::temp_dir().join(format!("rustgis-source-tests-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn write_parquet(path: &Path, covering: bool) {
    use arrow_array::{BinaryArray, Float32Array, Int64Array, RecordBatch, StringArray, StructArray};
    use arrow_schema::{DataType, Field as AField, Schema};
    let pts = points();
    let wkbs: Vec<Vec<u8>> = pts.iter().map(|(c, ..)| wkb::encode(&Geometry::Point(*c))).collect();
    let bbox_fields = ["xmin", "ymin", "xmax", "ymax"].map(|n| Arc::new(AField::new(n, DataType::Float32, false)));
    let coord = |f: fn(&Coord) -> f64| Arc::new(Float32Array::from(pts.iter().map(|(c, ..)| f(c) as f32).collect::<Vec<_>>()));
    let bbox =
        StructArray::new(bbox_fields.iter().cloned().collect(), vec![coord(|c| c.x), coord(|c| c.y), coord(|c| c.x), coord(|c| c.y)], None);
    let mut fields = vec![
        AField::new("name", DataType::Utf8, false),
        AField::new("value", DataType::Int64, false),
        AField::new("geometry", DataType::Binary, false),
    ];
    let mut cols: Vec<arrow_array::ArrayRef> = vec![
        Arc::new(StringArray::from(pts.iter().map(|p| p.1.clone()).collect::<Vec<_>>())),
        Arc::new(Int64Array::from(pts.iter().map(|p| p.2).collect::<Vec<_>>())),
        Arc::new(BinaryArray::from(wkbs.iter().map(|w| w.as_slice()).collect::<Vec<_>>())),
    ];
    let geo = if covering {
        fields.push(AField::new("geometry_bbox", DataType::Struct(bbox_fields.iter().cloned().collect()), false));
        cols.push(Arc::new(bbox));
        r#"{"version":"1.1.0","primary_column":"geometry","columns":{"geometry":{"encoding":"WKB","geometry_types":["Point"],
            "covering":{"bbox":{"xmin":["geometry_bbox","xmin"],"ymin":["geometry_bbox","ymin"],"xmax":["geometry_bbox","xmax"],"ymax":["geometry_bbox","ymax"]}}}}}"#
    } else {
        r#"{"version":"1.0.0","primary_column":"geometry","columns":{"geometry":{"encoding":"WKB","geometry_types":["Point"]}}}"#
    };
    let schema = Arc::new(Schema::new(fields));
    let batch = RecordBatch::try_new(schema.clone(), cols).unwrap();
    let props = ::parquet::file::properties::WriterProperties::builder()
        .set_max_row_group_row_count(Some(50))
        .set_key_value_metadata(Some(vec![::parquet::file::metadata::KeyValue::new("geo".to_string(), geo.to_string())]))
        .build();
    let mut w = ::parquet::arrow::ArrowWriter::try_new(std::fs::File::create(path).unwrap(), schema, Some(props)).unwrap();
    w.write(&batch).unwrap();
    w.close().unwrap();
}

fn write_fgb(path: &Path) {
    use flatgeobuf::geozero::{ColumnValue, PropertyProcessor};
    use flatgeobuf::{ColumnType, FgbWriter, GeometryType as G};
    let mut w = FgbWriter::create("grid", G::Point).unwrap();
    w.add_column("name", ColumnType::String, |_, _| {});
    w.add_column("value", ColumnType::Long, |_, _| {});
    for (c, name, v) in points() {
        let g = flatgeobuf::geozero::wkb::Wkb(wkb::encode(&Geometry::Point(c)));
        w.add_feature_geom(g, |f| {
            f.property(0, "name", &ColumnValue::String(&name)).unwrap();
            f.property(1, "value", &ColumnValue::Long(v)).unwrap();
        })
        .unwrap();
    }
    w.write(std::fs::File::create(path).unwrap()).unwrap();
}

/// A GeoPackage of the points in Web Mercator, with an R-tree index (or without).
fn write_gpkg(path: &Path, rtree: bool) {
    let _ = std::fs::remove_file(path);
    let c = rusqlite::Connection::open(path).unwrap();
    c.execute_batch(
        "CREATE TABLE gpkg_spatial_ref_sys (srs_name TEXT, srs_id INTEGER PRIMARY KEY, organization TEXT, organization_coordsys_id INTEGER, definition TEXT, description TEXT);
         INSERT INTO gpkg_spatial_ref_sys VALUES ('Web Mercator', 3857, 'EPSG', 3857, '', '');
         CREATE TABLE gpkg_contents (table_name TEXT PRIMARY KEY, data_type TEXT, identifier TEXT, description TEXT, last_change TEXT, min_x DOUBLE, min_y DOUBLE, max_x DOUBLE, max_y DOUBLE, srs_id INTEGER);
         CREATE TABLE gpkg_geometry_columns (table_name TEXT, column_name TEXT, geometry_type_name TEXT, srs_id INTEGER, z TINYINT, m TINYINT);
         INSERT INTO gpkg_contents (table_name, data_type, srs_id) VALUES ('grid \"q\"', 'features', 3857);
         INSERT INTO gpkg_geometry_columns VALUES ('grid \"q\"', 'geom', 'POINT', 3857, 0, 0);
         CREATE TABLE \"grid \"\"q\"\"\" (fid INTEGER PRIMARY KEY AUTOINCREMENT, geom BLOB, name TEXT, value MEDIUMINT, ok BOOLEAN);",
    )
    .unwrap();
    if rtree {
        c.execute_batch("CREATE VIRTUAL TABLE \"rtree_grid \"\"q\"\"_geom\" USING rtree(id, minx, maxx, miny, maxy);").unwrap();
    }
    for (i, (p, name, v)) in points().into_iter().enumerate() {
        let m = lonlat_to_merc(p);
        // A header with an xy envelope, then WKB.
        let mut blob = b"GP\x00\x03".to_vec();
        blob.extend_from_slice(&3857i32.to_le_bytes());
        for x in [m.x, m.x, m.y, m.y] {
            blob.extend_from_slice(&x.to_le_bytes());
        }
        blob.extend(wkb::encode(&Geometry::Point(m)));
        let fid = i as i64 + 1;
        c.execute("INSERT INTO \"grid \"\"q\"\"\" VALUES (?1, ?2, ?3, ?4, ?5)", rusqlite::params![fid, blob, name, v, v % 2]).unwrap();
        if rtree {
            c.execute("INSERT INTO \"rtree_grid \"\"q\"\"_geom\" VALUES (?1, ?2, ?3, ?4, ?5)", rusqlite::params![fid, m.x, m.x, m.y, m.y])
                .unwrap();
        }
    }
}

/// The same checks for every provider.
fn check(src: &dyn FeatureSource, expect_indexed: bool) {
    let info = src.info();
    assert_eq!(info.geometry_type, GeometryType::Point, "{}", info.format);
    assert_eq!(info.count, Some(600), "{}", info.format);
    assert_eq!(info.indexed, expect_indexed, "{}", info.format);
    assert!(info.fields.iter().any(|f| f.name == "name" && f.ty == FieldType::Text), "{:?}", info.fields);
    let vi = info.fields.iter().position(|f| f.name == "value").unwrap();
    assert_eq!(info.fields[vi].ty, FieldType::Integer);

    // Everything.
    let all = src.read(&Request::all()).unwrap();
    assert_eq!(all.features.len(), 600, "{}", info.format);
    assert!(all.complete);
    let f = all.features.iter().find(|f| f.attrs.contains(&Value::Text("p3_4".into()))).unwrap();
    let c = match f.geometry {
        Some(Geometry::Point(c)) => c,
        _ => panic!("not a point"),
    };
    assert!((c.x - (-138.0)).abs() < 1e-4 && (c.y - (-44.0)).abs() < 1e-4, "{c:?} in {}", info.format);
    assert_eq!(f.attrs[vi], Value::Int(64));
    // Ids stay put between reads.
    let near = src.read(&Request { extent: Some(Extent::new(-140.0, -46.0, -130.0, -40.0)), limit: 100 }).unwrap();
    let g = near.features.iter().find(|g| g.attrs.contains(&Value::Text("p3_4".into()))).unwrap();
    assert_eq!(g.id, f.id, "{}", info.format);

    // An extent: exactly the points in it.
    let e = Extent::new(-60.0, -10.0, 40.0, 50.0);
    let got = src.read(&Request { extent: Some(e), limit: 10_000 }).unwrap();
    assert_eq!(got.features.len(), in_box(&e), "{}", info.format);
    assert!(got.complete);
    assert!(got.features.iter().all(|f| f.geometry.as_ref().is_some_and(|g| e.contains(g.extent().center()))));
    // Every feature keeps the id a whole read gives it.
    let id_of: std::collections::HashMap<String, u64> =
        all.features.iter().map(|f| (f.attrs[0].to_string() + &f.attrs[1].to_string(), f.id)).collect();
    for f in &got.features {
        assert_eq!(id_of.get(&(f.attrs[0].to_string() + &f.attrs[1].to_string())), Some(&f.id), "{}", info.format);
    }

    // A limit: a sample spread over the whole extent, not the first rows.
    let s = src.read(&Request { extent: None, limit: 60 }).unwrap();
    assert!(!s.complete);
    assert!(s.features.len() <= 60 && s.features.len() >= 50, "{} of {}", s.features.len(), info.format);
    let xs: Vec<f64> = s.features.iter().filter_map(|f| f.geometry.as_ref()).map(|g| g.extent().min_x).collect();
    assert!(xs.iter().any(|x| *x < -100.0) && xs.iter().any(|x| *x > 100.0), "{} sample isn't spread", info.format);
    assert!(s.matched >= 600);

    // An extent outside the data.
    let none = src.read(&Request { extent: Some(Extent::new(-179.9, 80.0, -179.0, 84.0)), limit: 100 }).unwrap();
    assert!(none.features.is_empty());

    // A whole read into a dataset.
    let (ds, report) = materialize(src).unwrap();
    assert_eq!(ds.features.len(), 600);
    assert!(report.messages.is_empty());
}

#[test]
fn geoparquet_with_and_without_a_covering() {
    let d = dir();
    let p = d.join("grid.parquet");
    write_parquet(&p, true);
    let src = open_path(&p, None).unwrap();
    check(src.as_ref(), true);
    // The covering's row-group statistics give the extent.
    assert!(src.info().extent.min_x <= -174.0 && src.info().extent.max_x >= 174.0);
    let bytes = std::fs::read(&p).unwrap();
    check(open_bytes("grid.parquet", bytes, None).unwrap().as_ref(), true);

    let p2 = d.join("plain.parquet");
    write_parquet(&p2, false);
    check(open_path(&p2, None).unwrap().as_ref(), false);
}

#[test]
fn flatgeobuf() {
    let p = dir().join("grid.fgb");
    write_fgb(&p);
    let src = open_path(&p, None).unwrap();
    check(src.as_ref(), true);
    assert_eq!(src.info().name, "grid");
    check(open_bytes("grid.fgb", std::fs::read(&p).unwrap(), None).unwrap().as_ref(), true);
}

#[test]
fn geopackage_reprojects_and_uses_its_rtree() {
    let d = dir();
    let p = d.join("grid.gpkg");
    write_gpkg(&p, true);
    assert_eq!(layers(&p).unwrap(), vec!["grid \"q\"".to_string()]);
    let src = open_path(&p, Some("GRID \"Q\"")).unwrap();
    assert_eq!(src.info().crs, "EPSG:3857");
    check(src.as_ref(), true);
    // Booleans from integers.
    let all = src.read(&Request::all()).unwrap();
    let ok = src.info().fields.iter().position(|f| f.name == "ok").unwrap();
    assert!(all.features.iter().any(|f| f.attrs[ok] == Value::Bool(true)));
    check(open_bytes("grid.gpkg", std::fs::read(&p).unwrap(), None).unwrap().as_ref(), true);

    let p2 = d.join("scan.gpkg");
    write_gpkg(&p2, false);
    check(open_path(&p2, None).unwrap().as_ref(), false);
    assert!(open_path(&p2, Some("missing")).is_err());
}

/// The browser's GeoPackage reader (pure Rust) gives what SQLite gives, without the R-tree.
#[test]
fn geopackage_in_memory() {
    let p = dir().join("mem.gpkg");
    write_gpkg(&p, true);
    let bytes = Shared(Arc::new(std::fs::read(&p).unwrap()));
    assert_eq!(gpkg_mem::layers(bytes.clone()).unwrap(), vec!["grid \"q\"".to_string()]);
    let src = gpkg_mem::GpkgMemSource::open_shared(bytes.clone(), Some("GRID \"Q\"")).unwrap();
    assert_eq!(src.info().crs, "EPSG:3857");
    check(&src, false);
    // The same features, ids and values as SQLite reads.
    let sql = open_path(&p, None).unwrap();
    let (mut a, mut b) = (src.read(&Request::all()).unwrap().features, sql.read(&Request::all()).unwrap().features);
    a.sort_by_key(|f| f.id);
    b.sort_by_key(|f| f.id);
    assert_eq!(a, b);
    // SQLite's R-tree keeps 32-bit floats; the headers' envelopes are exact.
    let (e, f) = (src.info().extent, sql.info().extent);
    assert!([e.min_x - f.min_x, e.min_y - f.min_y, e.max_x - f.max_x, e.max_y - f.max_y].iter().all(|d| d.abs() < 1e-4), "{e:?} {f:?}");
    assert!(gpkg_mem::GpkgMemSource::open_shared(bytes.clone(), Some("missing")).is_err());
    // Every truncation is an error or a short read, never a panic.
    for n in (0..bytes.0.len()).step_by(131) {
        if let Ok(s) = gpkg_mem::GpkgMemSource::open_shared(Shared(Arc::new(bytes.0[..n].to_vec())), None) {
            let _ = s.read(&Request::all());
        }
    }
}

#[test]
fn an_unknown_coordinate_system_is_reported() {
    let p = dir().join("unknown_crs.gpkg");
    write_gpkg(&p, true);
    {
        let c = rusqlite::Connection::open(&p).unwrap();
        c.execute("UPDATE gpkg_spatial_ref_sys SET organization_coordsys_id = 9999999, srs_id = 3857", []).unwrap();
    }
    let src = open_path(&p, None).unwrap();
    assert!(src.info().notes.iter().any(|n| n.contains("EPSG:9999999")), "{:?}", src.info().notes);
    let (_, report) = materialize(src.as_ref()).unwrap();
    assert!(report.messages.iter().any(|m| m.contains("read as longitude/latitude")), "{:?}", report.messages);
}

#[test]
fn flatgeobuf_round_trips_a_dataset() {
    let mut ds = Dataset::new("mixed", GeometryType::Polygon, vec![Field::new("n", FieldType::Integer), Field::new("s", FieldType::Text)]);
    ds.fields.push(Field::new("ok", FieldType::Boolean));
    ds.fields.push(Field::new("x", FieldType::Double));
    let sq = |x: f64| vec![vec![Coord::new(x, 0.0), Coord::new(x + 1.0, 0.0), Coord::new(x + 1.0, 1.0), Coord::new(x, 0.0)]];
    ds.push(Some(Geometry::Polygon(vec![sq(0.0)])), vec![Value::Int(7), Value::Text("a".into()), Value::Bool(true), Value::Float(1.5)]);
    ds.push(Some(Geometry::Polygon(vec![sq(5.0), sq(9.0)])), vec![Value::Null, Value::Text("b".into()), Value::Null, Value::Null]);
    let p = dir().join("rt.fgb");
    crate::io::write_path(&ds, &p).unwrap();
    let (back, _) = crate::io::read_path(&p).unwrap();
    assert_eq!(back.features.len(), 2);
    assert_eq!(back.fields.iter().map(|f| f.ty).collect::<Vec<_>>(), ds.fields.iter().map(|f| f.ty).collect::<Vec<_>>());
    let a = back.features.iter().find(|f| f.attrs[1] == Value::Text("a".into())).unwrap();
    assert_eq!(a.attrs, ds.features[0].attrs);
    let b = back.features.iter().find(|f| f.attrs[1] == Value::Text("b".into())).unwrap();
    assert_eq!(b.attrs[0], Value::Null);
    assert!(matches!(&b.geometry, Some(Geometry::Polygon(p)) if p.len() == 2));
}

#[test]
fn hostile_files_are_errors_not_panics() {
    let mut junk = vec![b"PAR1".to_vec(), b"fgb\x03fgb\x00".to_vec(), b"SQLite format 3\0".to_vec(), vec![0xff; 4096], Vec::new()];
    // Every truncation of a real file.
    let d = dir();
    let p = d.join("t.parquet");
    write_parquet(&p, true);
    let pq = std::fs::read(&p).unwrap();
    let f = d.join("t.fgb");
    write_fgb(&f);
    let fgb = std::fs::read(&f).unwrap();
    for n in (0..pq.len()).step_by(97) {
        junk.push(pq[..n].to_vec());
    }
    for n in (0..fgb.len()).step_by(89) {
        junk.push(fgb[..n].to_vec());
    }
    for bytes in junk {
        for name in ["x.parquet", "x.fgb", "x.gpkg"] {
            if let Ok(s) = open_bytes(name, bytes.clone(), None) {
                let _ = s.read(&Request::all());
                let _ = s.read(&Request { extent: Some(Extent::new(-10.0, -10.0, 10.0, 10.0)), limit: 3 });
            }
        }
    }
    assert!(open_path(Path::new("nope.shp"), None).is_err());
}

/// Reads every vector file in `RUSTGIS_SAMPLE_VECTORS` (the opengeos sample data, e.g.
/// <https://source.coop/giswqs/opengeos>): `cargo test -p rustgis-data -- --ignored vector_samples --nocapture`.
#[test]
#[ignore]
fn vector_samples() {
    let Ok(dir) = std::env::var("RUSTGIS_SAMPLE_VECTORS") else { return };
    let mut paths: Vec<_> =
        std::fs::read_dir(dir).unwrap().filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| is_source_name(&p.to_string_lossy())).collect();
    paths.sort();
    assert!(!paths.is_empty());
    for p in paths {
        let names = match layers(&p) {
            Ok(n) => n,
            Err(e) => {
                println!("{}: {e}", p.display());
                continue;
            }
        };
        for layer in names {
            let t = std::time::Instant::now();
            let src = match open_path(&p, Some(&layer)) {
                Ok(s) => s,
                Err(e) => {
                    println!("{} [{layer}]: {e}", p.display());
                    continue;
                }
            };
            let i = src.info();
            let open = t.elapsed();
            let e = i.extent;
            let view = Extent::new(
                e.center().x - e.width() / 8.0,
                e.center().y - e.height() / 8.0,
                e.center().x + e.width() / 8.0,
                e.center().y + e.height() / 8.0,
            );
            let t2 = std::time::Instant::now();
            let b = match src.read(&Request { extent: Some(view), limit: 200_000 }) {
                Ok(b) => b,
                Err(e) => {
                    println!("{} [{layer}]: {e}", p.display());
                    continue;
                }
            };
            println!(
                "{} [{}] {} {:?} {} fields, {:?} features, {} indexed={} extent={:?} open {:?}; view: {} of {} (complete={}, skipped {}) in {:?}",
                p.file_name().unwrap().to_string_lossy(),
                i.name,
                i.format,
                i.geometry_type,
                i.fields.len(),
                i.count,
                i.crs,
                i.indexed,
                i.extent,
                open,
                b.features.len(),
                b.matched,
                b.complete,
                b.skipped,
                t2.elapsed()
            );
        }
    }
}

/// A polygon layer with every field type, a hole, a multipolygon, a feature without geometry,
/// long and clashing field names and non-ASCII text.
fn export_sample() -> Dataset {
    let mut ds = Dataset::new(
        "Parcels",
        GeometryType::Polygon,
        vec![
            Field::new("population_total", FieldType::Integer),
            Field::new("population_density", FieldType::Double),
            Field::new("name", FieldType::Text),
            Field::new("ok", FieldType::Boolean),
            Field::new("geometry", FieldType::Text),
        ],
    );
    let sq = |x: f64, y: f64, s: f64| {
        vec![Coord::new(x, y), Coord::new(x + s, y), Coord::new(x + s, y + s), Coord::new(x, y + s), Coord::new(x, y)]
    };
    ds.push(
        Some(Geometry::Polygon(vec![vec![sq(0.0, 0.0, 10.0), sq(2.0, 2.0, 2.0)]])),
        vec![Value::Int(1_234_567), Value::Float(12.5), Value::Text("Zürich — 東京".into()), Value::Bool(true), Value::Text("g".into())],
    );
    ds.push(
        Some(Geometry::Polygon(vec![vec![sq(20.0, 20.0, 1.0)], vec![sq(-30.0, -40.0, 5.0)]])),
        vec![Value::Int(-3), Value::Float(-0.25), Value::Null, Value::Bool(false), Value::Null],
    );
    ds.push(None, vec![Value::Null, Value::Null, Value::Text("no shape".into()), Value::Null, Value::Null]);
    ds
}

#[test]
fn writes_geopackage_geoparquet_and_shapefile() {
    let ds = export_sample();
    let d = dir().join("export");
    std::fs::create_dir_all(&d).unwrap();
    for ext in ["gpkg", "parquet", "shp", "fgb"] {
        let p = d.join(format!("parcels.{ext}"));
        let left_out = crate::io::write_path(&ds, &p).unwrap();
        let holds_all = matches!(ext, "gpkg" | "parquet");
        assert_eq!(left_out, if holds_all { 0 } else { 1 }, "{ext}");
        let (back, _) = crate::io::read_path(&p).unwrap();
        // Readers skip rows without geometry (the GeoPackage still holds three, below).
        assert_eq!(back.features.len(), 2, "{ext}");
        assert_eq!(back.geometry_type, GeometryType::Polygon, "{ext}");
        let e = back.extent();
        assert_eq!((e.min_x, e.min_y, e.max_x, e.max_y), (-30.0, -40.0, 21.0, 21.0), "{ext}");
        let field = |name: &str| back.field_index(name).unwrap_or_else(|| panic!("{ext}: no field {name} in {:?}", back.fields));
        let pop = if ext == "shp" { field("population") } else { field("population_total") };
        let dens = if ext == "shp" { field("populati_1") } else { field("population_density") };
        let name = field("name");
        let first = back.features.iter().find(|f| f.attrs[name] == Value::Text("Zürich — 東京".into())).unwrap();
        assert_eq!(first.attrs[pop].as_f64(), Some(1_234_567.0), "{ext}");
        assert_eq!(first.attrs[dens].as_f64(), Some(12.5), "{ext}");
        assert!(matches!(&first.geometry, Some(Geometry::Polygon(p)) if p.len() == 1 && p[0].len() == 2), "{ext}: the hole is kept");
        let second = back.features.iter().find(|f| f.attrs[pop].as_f64() == Some(-3.0)).unwrap();
        assert!(matches!(&second.geometry, Some(Geometry::Polygon(p)) if p.len() == 2), "{ext}: both parts are kept");
        assert_eq!(second.attrs[name], Value::Null, "{ext}");
        if holds_all {
            // The clashing field keeps its name; the geometry column took another.
            assert_eq!(back.value(0, field("geometry")), &Value::Text("g".into()), "{ext}");
        }
    }
    // The Shapefile's companion files.
    for e in ["shx", "dbf", "prj", "cpg"] {
        assert!(d.join(format!("parcels.{e}")).exists(), "{e}");
    }
    // GeoPackage and GeoParquet keep feature ids and answer extent queries by their index.
    for ext in ["gpkg", "parquet"] {
        let s = open_path(&d.join(format!("parcels.{ext}")), None).unwrap();
        assert_eq!(s.info().count, Some(3), "{ext}");
        let b = s.read(&Request { extent: Some(Extent::new(15.0, 15.0, 25.0, 25.0)), limit: 100 }).unwrap();
        assert_eq!(b.features.len(), 1, "{ext}");
        if ext == "gpkg" {
            assert_eq!(b.features[0].id, 2);
        }
    }
    // Written again over the old files.
    let mut fewer = ds.clone();
    fewer.features.truncate(1);
    for ext in ["gpkg", "parquet", "shp"] {
        let p = d.join(format!("parcels.{ext}"));
        crate::io::write_path(&fewer, &p).unwrap();
        assert_eq!(crate::io::read_path(&p).unwrap().0.features.len(), 1, "{ext}");
    }
}

#[test]
fn writes_points_lines_kmz_and_zipped_shapefiles() {
    let mut pts = Dataset::new("Cities", GeometryType::Point, vec![Field::new("name", FieldType::Text)]);
    pts.push(Some(Geometry::Point(Coord::new(2.35, 48.85))), vec![Value::Text("Paris".into())]);
    pts.push(Some(Geometry::MultiPoint(vec![Coord::new(1.0, 1.0), Coord::new(2.0, 2.0)])), vec![Value::Text("Pair".into())]);
    let mut lines = Dataset::new("Roads", GeometryType::Polyline, vec![Field::new("len", FieldType::Double)]);
    lines.push(
        Some(Geometry::Line(vec![vec![Coord::new(0.0, 0.0), Coord::new(1.0, 1.0)], vec![Coord::new(5.0, 5.0), Coord::new(6.0, 5.0)]])),
        vec![Value::Float(1e30)],
    );
    lines.push(Some(Geometry::Line(vec![vec![Coord::new(3.0, 3.0)]])), vec![Value::Float(2.0)]);
    let d = dir().join("export2");
    std::fs::create_dir_all(&d).unwrap();
    for ds in [&pts, &lines] {
        for ext in ["gpkg", "parquet", "shp"] {
            let p = d.join(format!("{}.{ext}", ds.name));
            let left_out = crate::io::write_path(ds, &p).unwrap();
            let (back, _) = crate::io::read_path(&p).unwrap();
            assert_eq!(back.geometry_type, ds.geometry_type, "{ext}");
            // A one-point line can't be a Shapefile polyline (and isn't read back from the others).
            let lost = usize::from(ds.name == "Roads");
            assert_eq!((left_out, back.features.len()), (if ext == "shp" { lost } else { 0 }, 2 - lost), "{} {ext}", ds.name);
            assert_eq!(
                back.extent(),
                ds.features.iter().take(2 - lost).filter_map(|f| f.geometry.as_ref()).fold(Extent::EMPTY, |e, g| e.union(&g.extent()))
            );
        }
    }
    // A number too large for a dBase field is null rather than cut short.
    let (roads, _) = crate::io::read_path(&d.join("Roads.shp")).unwrap();
    assert_eq!(roads.value(0, 0), &Value::Null);
    // A whole number past 2^53 would only be stored rounded, so it's null too.
    let mut ids = Dataset::new("ids", GeometryType::Point, vec![Field::new("id", FieldType::Integer)]);
    ids.push(Some(Geometry::Point(Coord::new(0.0, 0.0))), vec![Value::Int((1 << 53) + 1)]);
    ids.push(Some(Geometry::Point(Coord::new(1.0, 1.0))), vec![Value::Int(1 << 52)]);
    crate::io::write_path(&ids, &d.join("ids.shp")).unwrap();
    let (back, _) = crate::io::read_path(&d.join("ids.shp")).unwrap();
    assert_eq!((back.value(0, 0), back.value(1, 0)), (&Value::Null, &Value::Int(1 << 52)));
    // An upper-case .SHP keeps its name, so the path given is the file written.
    crate::io::write_path(&ids, &d.join("Upper.SHP")).unwrap();
    assert!(crate::io::read_path(&d.join("Upper.SHP")).is_ok());
    // No temporary files are left beside the outputs.
    assert!(std::fs::read_dir(&d).unwrap().all(|e| !e.unwrap().file_name().to_string_lossy().ends_with(".tmp")));
    // KMZ: the KML zipped as doc.kml.
    let (kmz, left_out) = crate::io::write_bytes(&pts, "kmz").unwrap();
    assert_eq!(left_out, 0);
    let entries = crate::zip::read(&kmz).unwrap();
    assert_eq!(entries[0].0, "doc.kml");
    assert!(String::from_utf8_lossy(&entries[0].1).contains("<name>Paris</name>"));
    // A zipped Shapefile opens as one.
    let (zipped, _) = crate::io::write_bytes(&pts, "shp").unwrap();
    let entries = crate::zip::read(&zipped).unwrap();
    let names: Vec<&str> = entries.iter().map(|e| e.0.as_str()).collect();
    assert_eq!(names, ["Cities.shp", "Cities.shx", "Cities.dbf", "Cities.prj", "Cities.cpg"]);
    let unzipped = d.join("unzipped");
    std::fs::create_dir_all(&unzipped).unwrap();
    for (n, b) in &entries {
        std::fs::write(unzipped.join(n), b).unwrap();
    }
    let (back, _) = crate::io::read_path(&unzipped.join("Cities.shp")).unwrap();
    assert_eq!(back.features.len(), 2);
    // Tables have no Shapefile or FlatGeobuf form, but GeoPackage and GeoParquet hold them.
    let mut table = Dataset::new("t", GeometryType::None, vec![Field::new("a", FieldType::Integer)]);
    table.push(None, vec![Value::Int(1)]);
    assert!(crate::io::write_bytes(&table, "shp").is_err());
    assert!(crate::io::write_bytes(&table, "parquet").is_ok());
    crate::io::write_path(&table, &d.join("t.gpkg")).unwrap();
    // Field names differing only in case are distinct in a dataset but not in SQLite.
    let mut cased = Dataset::new("c", GeometryType::Point, vec![Field::new("A", FieldType::Integer), Field::new("a", FieldType::Text)]);
    cased.push(Some(Geometry::Point(Coord::new(1.0, 2.0))), vec![Value::Int(1), Value::Text("x".into())]);
    crate::io::write_path(&cased, &d.join("c.gpkg")).unwrap();
    let (back, _) = crate::io::read_path(&d.join("c.gpkg")).unwrap();
    assert_eq!(back.fields.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(), ["A", "a_1"]);
    assert_eq!(back.features[0].attrs, [Value::Int(1), Value::Text("x".into())]);
    // An empty layer writes too.
    let empty = Dataset::new("e", GeometryType::Point, vec![]);
    assert!(crate::io::write_bytes(&empty, "parquet").is_ok());
    crate::io::write_path(&empty, &d.join("e.gpkg")).unwrap();
    assert!(crate::io::write_bytes(&empty, "shp").is_err());
}

/// Compares the browser's GeoPackage reader with SQLite on every table of every `.gpkg` in
/// `RUSTGIS_SAMPLE_VECTORS`: `cargo test -p rustgis-data --release -- --ignored gpkg_mem_matches --nocapture`.
#[test]
#[ignore]
fn gpkg_mem_matches_sqlite() {
    let Ok(dir) = std::env::var("RUSTGIS_SAMPLE_VECTORS") else { return };
    let mut paths: Vec<_> = std::fs::read_dir(dir).unwrap().filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| ext(p) == "gpkg").collect();
    paths.sort();
    let (mut tables, mut differ) = (0, 0);
    for p in paths {
        let bytes = Shared(Arc::new(std::fs::read(&p).unwrap()));
        let names = match layers(&p) {
            Ok(n) => n,
            Err(e) => {
                println!("{}: {e}", p.display());
                continue;
            }
        };
        for name in names {
            tables += 1;
            let t = std::time::Instant::now();
            let mem = gpkg_mem::GpkgMemSource::open_shared(bytes.clone(), Some(&name))
                .and_then(|s| s.read(&Request::all()).map(|b| (s.info().clone(), b)));
            let ms = t.elapsed().as_millis();
            let sql = open_path(&p, Some(&name)).and_then(|s| s.read(&Request::all()).map(|b| (s.info().clone(), b)));
            match (mem, sql) {
                (Ok((mi, mut m)), Ok((si, mut s))) => {
                    m.features.sort_by_key(|f| f.id);
                    s.features.sort_by_key(|f| f.id);
                    let same =
                        m.features == s.features && mi.fields == si.fields && mi.count == si.count && mi.geometry_type == si.geometry_type;
                    differ += usize::from(!same);
                    println!("{} {name}: {} features, {ms} ms{}", p.display(), m.features.len(), if same { "" } else { "  DIFFERENT" });
                }
                (m, s) => {
                    differ += usize::from(m.is_ok() != s.is_ok());
                    println!("{} {name}: mem {:?} / sqlite {:?}", p.display(), m.err(), s.err());
                }
            }
        }
    }
    println!("{tables} tables, {differ} different");
    assert_eq!(differ, 0);
}

fn open_driven(url: &str) -> Fetching {
    Fetching(std::sync::Arc::from(open_url_blocking(url).unwrap()))
}

/// `n` × `n` points over the world, a FlatGeobuf or a GeoParquet (row groups of 2 000), larger
/// than one fetch.
fn big(n: usize, parquet: bool) -> Vec<u8> {
    let mut ds =
        Dataset::new("big", GeometryType::Point, vec![Field::new("name", FieldType::Text), Field::new("value", FieldType::Integer)]);
    // Built directly: `push` per feature is slow for this many.
    for i in 0..n {
        for j in 0..n {
            let c = Coord::new(-179.0 + 358.0 * i as f64 / n as f64, -80.0 + 160.0 * j as f64 / n as f64);
            let id = (i * n + j) as u64;
            ds.features.push(crate::Feature {
                id: id + 1,
                geometry: Some(Geometry::Point(c)),
                attrs: vec![Value::Text(format!("p{i}_{j}")), Value::Int(id as i64)],
            });
        }
    }
    if !parquet {
        return fgb::write(&ds).unwrap();
    }
    // GeoParquet in row groups of 2 000 rows (with a bbox covering), as large files are written.
    use arrow_array::{BinaryArray, Float64Array, Int64Array, RecordBatch, StringArray, StructArray};
    use arrow_schema::{DataType, Field as AField, Schema};
    let pts: Vec<Coord> = ds
        .features
        .iter()
        .filter_map(|f| match f.geometry {
            Some(Geometry::Point(c)) => Some(c),
            _ => None,
        })
        .collect();
    let bbox_fields = ["xmin", "ymin", "xmax", "ymax"].map(|n| Arc::new(AField::new(n, DataType::Float64, false)));
    let coord = |f: fn(&Coord) -> f64| Arc::new(Float64Array::from(pts.iter().map(f).collect::<Vec<_>>()));
    let bbox =
        StructArray::new(bbox_fields.iter().cloned().collect(), vec![coord(|c| c.x), coord(|c| c.y), coord(|c| c.x), coord(|c| c.y)], None);
    let schema = Arc::new(Schema::new(vec![
        AField::new("name", DataType::Utf8, false),
        AField::new("value", DataType::Int64, false),
        AField::new("geometry", DataType::Binary, false),
        AField::new("bbox", DataType::Struct(bbox_fields.iter().cloned().collect()), false),
    ]));
    let wkbs: Vec<Vec<u8>> = pts.iter().map(|c| wkb::encode(&Geometry::Point(*c))).collect();
    let cols: Vec<arrow_array::ArrayRef> = vec![
        Arc::new(StringArray::from(ds.features.iter().map(|f| f.attrs[0].to_string()).collect::<Vec<_>>())),
        Arc::new(Int64Array::from(ds.features.iter().map(|f| f.attrs[1].as_f64().unwrap_or(0.0) as i64).collect::<Vec<_>>())),
        Arc::new(BinaryArray::from(wkbs.iter().map(|w| w.as_slice()).collect::<Vec<_>>())),
        Arc::new(bbox),
    ];
    let geo = r#"{"version":"1.1.0","primary_column":"geometry","columns":{"geometry":{"encoding":"WKB","geometry_types":["Point"],
        "covering":{"bbox":{"xmin":["bbox","xmin"],"ymin":["bbox","ymin"],"xmax":["bbox","xmax"],"ymax":["bbox","ymax"]}}}}}"#;
    let props = ::parquet::file::properties::WriterProperties::builder()
        .set_max_row_group_row_count(Some(2_000))
        .set_key_value_metadata(Some(vec![::parquet::file::metadata::KeyValue::new("geo".to_string(), geo.to_string())]))
        .build();
    let mut out = Vec::new();
    let mut w = ::parquet::arrow::ArrowWriter::try_new(&mut out, schema.clone(), Some(props)).unwrap();
    w.write(&RecordBatch::try_new(schema, cols).unwrap()).unwrap();
    w.close().unwrap();
    out
}

#[test]
fn flatgeobuf_and_geoparquet_by_address() {
    let d = dir();
    // The same checks as for a file on disk, read by range.
    let f = d.join("r.fgb");
    write_fgb(&f);
    remote::testing::serve("https://test.example/grid.fgb", std::fs::read(&f).unwrap());
    let src = open_driven("https://test.example/grid.fgb");
    check(&src, true);
    assert_eq!(src.info().name, "grid");
    let p = d.join("r.parquet");
    write_parquet(&p, true);
    remote::testing::serve("https://test.example/grid.parquet", std::fs::read(&p).unwrap());
    check(&open_driven("https://test.example/grid.parquet"), true);

    // A view of a large file fetches a small part of it, and the same features as the file on disk.
    for (name, parquet) in [("big.fgb", false), ("big.parquet", true)] {
        let bytes = big(400, parquet);
        let total = bytes.len();
        let url = format!("https://test.example/{name}");
        let sent = remote::testing::serve(&url, bytes.clone());
        let src = open_driven(&url);
        assert_eq!(src.info().count, Some(160_000), "{name}");
        let e = Extent::new(10.0, 10.0, 12.0, 12.0);
        let req = Request { extent: Some(e), limit: 10_000 };
        let got = src.read(&req).unwrap();
        let local = open_bytes(name, bytes, None).unwrap().read(&req).unwrap();
        let ids = |b: &Batch| b.features.iter().map(|f| f.id).collect::<std::collections::BTreeSet<_>>();
        assert!(!got.features.is_empty() && ids(&got) == ids(&local), "{name}: {} vs {}", got.features.len(), local.features.len());
        assert_eq!(
            got.features.first().map(|f| &f.attrs),
            local.features.iter().find(|f| Some(f.id) == got.features.first().map(|g| g.id)).map(|f| &f.attrs)
        );
        let fetched = sent.load(std::sync::atomic::Ordering::Relaxed);
        assert!(fetched < total / 4, "{name}: fetched {fetched} of {total} bytes");
        // A sample of the whole file, spread over it.
        let s = src.read(&Request { extent: None, limit: 500 }).unwrap();
        assert!(!s.complete && s.features.len() <= 500 && s.features.len() >= 250, "{name}: {}", s.features.len());
    }

    // Files without an index or a covering are read whole instead.
    let p = d.join("plain.parquet");
    write_parquet(&p, false);
    remote::testing::serve("https://test.example/plain.parquet", std::fs::read(&p).unwrap());
    let e = open_url_blocking("https://test.example/plain.parquet").err().map(|e| e.to_string()).unwrap_or_default();
    assert!(e.contains(remote::READ_WHOLE), "{e}");
    let mut ds = Dataset::new("x", GeometryType::Point, vec![]);
    ds.push(Some(Geometry::Point(Coord::new(1.0, 2.0))), vec![]);
    let unindexed = {
        use flatgeobuf::{FgbWriter, FgbWriterOptions, GeometryType as G};
        let mut w = FgbWriter::create_with_options("x", G::Point, FgbWriterOptions { write_index: false, ..Default::default() }).unwrap();
        w.add_feature_geom(flatgeobuf::geozero::wkb::Wkb(wkb::encode(&Geometry::Point(Coord::new(1.0, 2.0)))), |_| {}).unwrap();
        let mut out = Vec::new();
        w.write(&mut out).unwrap();
        out
    };
    remote::testing::serve("https://test.example/noindex.fgb", unindexed);
    let e = open_url_blocking("https://test.example/noindex.fgb").err().map(|e| e.to_string()).unwrap_or_default();
    assert!(e.contains(remote::READ_WHOLE), "{e}");

    // Cut short and junk: errors, never a panic or an endless read.
    let fgb = big(40, false);
    let pq = big(40, true);
    for (k, n) in (0..fgb.len()).step_by(997).enumerate() {
        let url = format!("https://test.example/cut{k}.fgb");
        remote::testing::serve(&url, fgb[..n].to_vec());
        if let Ok(s) = open_url_blocking(&url) {
            let s: std::sync::Arc<dyn FeatureSource> = std::sync::Arc::from(s);
            let _ = read_blocking(&s, &Request::all());
        }
    }
    for (k, n) in (0..pq.len()).step_by(991).enumerate() {
        let url = format!("https://test.example/cut{k}.parquet");
        remote::testing::serve(&url, pq[pq.len() - n..].to_vec());
        if let Ok(s) = open_url_blocking(&url) {
            let s: std::sync::Arc<dyn FeatureSource> = std::sync::Arc::from(s);
            let _ = read_blocking(&s, &Request::all());
        }
    }
    assert!(open_url_blocking("https://test.example/missing.fgb").is_err());
    assert!(is_remote_name("https://x.org/a/b.fgb?sig=1") && is_remote_name("http://x.org/b.geoparquet") && !is_remote_name("/tmp/a.fgb"));
    assert!(is_remote_name("s3://bucket/a/b.parquet") && !is_remote_name("s3://bucket/a/b.tif") && !is_remote_name("ftp://x.org/a.fgb"));
}

#[test]
fn flatgeobuf_stand_in_is_a_feature() {
    // The stand-in a sample passes over must pass the flatbuffers verifier, as real features do.
    let body = &fgb::STAND_IN_FOR_TESTS[4..];
    assert!(flatgeobuf::size_prefixed_root_as_feature(fgb::STAND_IN_FOR_TESTS.as_slice()).is_ok(), "{body:?}");
}

/// Writes a dataset as a Shapefile at `path` (its `.shp`), with the other files beside it.
fn write_shapefile(ds: &Dataset, path: &Path) {
    let (parts, _) = crate::io::shapefile_parts(ds).unwrap();
    for (ext, bytes) in parts {
        std::fs::write(path.with_extension(ext), bytes).unwrap();
    }
}

/// The points as a Shapefile.
fn points_shapefile(path: &Path) {
    let mut ds =
        Dataset::new("grid", GeometryType::Point, vec![Field::new("name", FieldType::Text), Field::new("value", FieldType::Integer)]);
    for (c, name, v) in points() {
        ds.push(Some(Geometry::Point(c)), vec![Value::Text(name), Value::Int(v)]);
    }
    write_shapefile(&ds, path);
}

#[test]
fn shapefile_read_in_place() {
    let p = dir().join("grid.shp");
    points_shapefile(&p);
    let src = open_path(&p, None).unwrap();
    assert_eq!(src.info().format, "Shapefile");
    check(src.as_ref(), false);
    assert_eq!(shp::record_count(&p), Some(600));
}

#[test]
fn shapefile_in_place_matches_one_read_whole() {
    let d = dir().join("shp-whole");
    std::fs::create_dir_all(&d).unwrap();
    let mut lines =
        Dataset::new("roads", GeometryType::Polyline, vec![Field::new("len", FieldType::Double), Field::new("ok", FieldType::Boolean)]);
    lines.push(
        Some(Geometry::Line(vec![vec![Coord::new(0.0, 0.0), Coord::new(1.5, 2.25)], vec![Coord::new(3.0, 3.0), Coord::new(4.0, 5.0)]])),
        vec![Value::Float(2.5), Value::Bool(true)],
    );
    lines.push(Some(Geometry::Line(vec![vec![Coord::new(-7.0, 8.0), Coord::new(9.0, -10.0)]])), vec![Value::Null, Value::Bool(false)]);
    let mut multi = Dataset::new("pairs", GeometryType::Point, vec![Field::new("name", FieldType::Text)]);
    multi.push(Some(Geometry::MultiPoint(vec![Coord::new(1.0, 1.0), Coord::new(2.0, 2.0)])), vec![Value::Text("pair".into())]);
    multi.push(Some(Geometry::MultiPoint(vec![Coord::new(-3.0, 4.0)])), vec![Value::Text("one".into())]);
    for ds in [export_sample(), lines, multi] {
        let p = d.join(format!("{}.shp", ds.name));
        write_shapefile(&ds, &p);
        let (whole, _) = crate::io::read_path(&p).unwrap();
        let (inplace, report) = materialize(open_path(&p, None).unwrap().as_ref()).unwrap();
        assert!(report.messages.is_empty(), "{:?}", report.messages);
        assert_eq!(inplace.geometry_type, whole.geometry_type, "{}", ds.name);
        assert_eq!(inplace.fields, whole.fields, "{}", ds.name);
        assert_eq!(inplace.features, whole.features, "{}", ds.name);
    }
}

#[test]
fn shapefile_in_place_reprojects_from_its_prj() {
    let d = dir().join("shp-utm");
    std::fs::create_dir_all(&d).unwrap();
    let p = d.join("paris.shp");
    let mut ds = Dataset::new("paris", GeometryType::Point, vec![Field::new("name", FieldType::Text)]);
    // Paris in UTM zone 31N.
    ds.push(Some(Geometry::Point(Coord::new(451_219.33, 5_411_550.20))), vec![Value::Text("paris".into())]);
    write_shapefile(&ds, &p);
    let utm = r#"PROJCS["WGS_1984_UTM_Zone_31N",GEOGCS["GCS_WGS_1984",DATUM["D_WGS_1984",SPHEROID["WGS_1984",6378137.0,298.257223563]],PRIMEM["Greenwich",0.0],UNIT["Degree",0.0174532925199433]],PROJECTION["Transverse_Mercator"],PARAMETER["False_Easting",500000.0],PARAMETER["False_Northing",0.0],PARAMETER["Central_Meridian",3.0],PARAMETER["Scale_Factor",0.9996],PARAMETER["Latitude_Of_Origin",0.0],UNIT["Meter",1.0]]"#;
    std::fs::write(p.with_extension("prj"), utm).unwrap();
    let src = open_path(&p, None).unwrap();
    let e = src.info().extent;
    assert!((e.min_x - 2.335).abs() < 1e-6 && (e.min_y - 48.855).abs() < 1e-6, "{e:?}");
    assert!(src.info().notes.is_empty(), "{:?}", src.info().notes);
    // A longitude/latitude extent finds it; one beside it doesn't.
    let near = src.read(&Request { extent: Some(Extent::new(2.0, 48.5, 2.5, 49.0)), limit: 10 }).unwrap();
    let c = near.features.first().and_then(|f| f.geometry.as_ref()).and_then(Geometry::label_point).unwrap();
    assert!((c.x - 2.335).abs() < 1e-6 && (c.y - 48.855).abs() < 1e-6, "{c:?}");
    assert!(src.read(&Request { extent: Some(Extent::new(3.0, 48.5, 3.5, 49.0)), limit: 10 }).unwrap().features.is_empty());
    // Without the .prj the coordinates are taken as they are, and that's said.
    std::fs::remove_file(p.with_extension("prj")).unwrap();
    let src = open_path(&p, None).unwrap();
    assert!(src.info().notes.iter().any(|n| n.contains("not longitude/latitude")), "{:?}", src.info().notes);
}

#[test]
fn damaged_shapefiles_are_errors_or_skips_not_panics() {
    let d = dir().join("shp-junk");
    std::fs::create_dir_all(&d).unwrap();
    let good = d.join("good.shp");
    write_shapefile(&export_sample(), &good);
    let (shp, dbf) = (std::fs::read(&good).unwrap(), std::fs::read(good.with_extension("dbf")).unwrap());
    let p = d.join("cut.shp");
    let read_all = |p: &Path| {
        if let Ok(s) = open_path(p, None) {
            let _ = s.read(&Request::all());
            let _ = s.read(&Request { extent: Some(Extent::new(-10.0, -10.0, 10.0, 10.0)), limit: 1 });
        }
    };
    std::fs::write(p.with_extension("dbf"), &dbf).unwrap();
    for n in 0..shp.len() {
        std::fs::write(&p, &shp[..n]).unwrap();
        read_all(&p);
    }
    // Every byte of the shapes flipped in turn.
    for i in 100..shp.len() {
        let mut bad = shp.clone();
        bad[i] ^= 0xA5;
        std::fs::write(&p, &bad).unwrap();
        read_all(&p);
    }
    std::fs::write(&p, &shp).unwrap();
    for n in 0..dbf.len() {
        std::fs::write(p.with_extension("dbf"), &dbf[..n]).unwrap();
        read_all(&p);
    }
    std::fs::remove_file(p.with_extension("dbf")).unwrap();
    assert!(open_path(&p, None).is_err());
}

/// Reads every Shapefile under `RUSTGIS_SAMPLE_SHAPEFILES` both ways, in place and whole, and
/// compares them: `cargo test --release -p rustgis-data -- --ignored shapefile_samples --nocapture`.
#[test]
#[ignore]
fn shapefile_samples() {
    let Ok(dir) = std::env::var("RUSTGIS_SAMPLE_SHAPEFILES") else { return };
    fn walk(d: &Path, out: &mut Vec<PathBuf>) {
        for e in std::fs::read_dir(d).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(&p, out);
            } else if p.extension().is_some_and(|e| e.eq_ignore_ascii_case("shp")) {
                out.push(p);
            }
        }
    }
    let mut paths = Vec::new();
    walk(Path::new(&dir), &mut paths);
    paths.sort();
    let mut differ = Vec::new();
    for p in paths {
        let t = std::time::Instant::now();
        let whole = crate::io::read_path(&p);
        let whole_t = t.elapsed();
        let t = std::time::Instant::now();
        let inplace = open_path(&p, None).and_then(|s| materialize(s.as_ref()));
        let inplace_t = t.elapsed();
        let (Ok((whole, _)), Ok((inplace, _))) = (&whole, &inplace) else {
            println!("{}: whole {:?} / in place {:?}", p.display(), whole.as_ref().err(), inplace.as_ref().err());
            continue;
        };
        let same_geometry = whole.features.iter().zip(&inplace.features).all(|(a, b)| a.geometry == b.geometry);
        let same = whole.features == inplace.features && whole.fields == inplace.fields;
        println!(
            "{}: {} features, whole {whole_t:?}, in place {inplace_t:?}; same={same} (geometry {same_geometry}, fields {})",
            p.display(),
            whole.features.len(),
            whole.fields == inplace.fields
        );
        if !same_geometry || whole.features.len() != inplace.features.len() {
            differ.push(p.display().to_string());
        }
    }
    assert!(differ.is_empty(), "{differ:?}");
}

#[test]
fn shapefile_in_place_keeps_large_whole_numbers_exact() {
    let d = dir().join("shp-ids");
    std::fs::create_dir_all(&d).unwrap();
    let p = d.join("ids.shp");
    let mut ds = Dataset::new("ids", GeometryType::Point, vec![Field::new("id", FieldType::Integer)]);
    for i in 0..5 {
        ds.push(Some(Geometry::Point(Coord::new(i as f64, 0.0))), vec![Value::Int(i)]);
    }
    write_shapefile(&ds, &p);
    // The id column (18 digits, no decimals) as another program writes it: values past f64's
    // exact integers, whole ones written with a point or exponent, and one that isn't whole.
    let mut dbf = std::fs::read(p.with_extension("dbf")).unwrap();
    let (header, row) = (u16::from_le_bytes([dbf[8], dbf[9]]) as usize, u16::from_le_bytes([dbf[10], dbf[11]]) as usize);
    for (i, text) in ["123456789012345678", "-98765432109876543", "12.0", "1e3", "12.5"].iter().enumerate() {
        let at = header + i * row + 1;
        dbf[at..at + 18].copy_from_slice(format!("{text:>18}").as_bytes());
    }
    std::fs::write(p.with_extension("dbf"), &dbf).unwrap();
    let src = open_path(&p, None).unwrap();
    assert_eq!(src.info().fields[0].ty, FieldType::Integer);
    let got: Vec<Value> = src.read(&Request::all()).unwrap().features.into_iter().map(|f| f.attrs[0].clone()).collect();
    assert_eq!(
        got,
        vec![
            Value::Int(123_456_789_012_345_678),
            Value::Int(-98_765_432_109_876_543),
            Value::Int(12),
            Value::Int(1000),
            Value::Float(12.5)
        ]
    );
}

#[test]
fn geojson_read_in_place() {
    let mut ds =
        Dataset::new("grid", GeometryType::Point, vec![Field::new("name", FieldType::Text), Field::new("value", FieldType::Integer)]);
    for (c, name, v) in points() {
        ds.push(Some(Geometry::Point(c)), vec![Value::Text(name), Value::Int(v)]);
    }
    let p = dir().join("grid.geojson");
    crate::io::write_path(&ds, &p).unwrap();
    let src = open_path(&p, None).unwrap();
    assert_eq!(src.info().format, "GeoJSON");
    check(src.as_ref(), false);
}

#[test]
fn geojson_in_place_matches_one_read_whole() {
    let d = dir().join("geojson-whole");
    std::fs::create_dir_all(&d).unwrap();
    // Types widened over features, a feature with geometry that can't be read (its properties
    // still count), one without geometry, mixed kinds, nested values and repeated keys.
    let mixed = r#"{"features":[
        {"type":"Feature","properties":{"a":1,"b":null,"n":{"x":[1,2]}},"geometry":{"type":"Point","coordinates":[1,2]}},
        {"type":"Feature","properties":{"a":2.5,"c":"x"},"geometry":{"type":"Nope"}},
        {"type":"Feature","properties":{"c":true,"d":null},"geometry":null},
        {"type":"Feature","properties":{"a":3},"geometry":{"type":"Polygon","coordinates":[[[0,0],[4,0],[4,4],[0,0]],[[1,1],[2,1],[2,2],[1,1]]]}},
        {"type":"Feature","properties":{"b":"t"},"geometry":{"type":"MultiLineString","coordinates":[[[0,0],[1,1]],[[5,5],[6,7]]]}},
        null,
        {"type":"Feature","properties":{},"geometry":{"type":"MultiPoint","coordinates":[[9,9],[8,8]]}}
    ],"type":"FeatureCollection","name":"m"}"#;
    let docs = [("mixed.geojson", mixed.to_string()), ("parcels.json", crate::io::to_geojson(&export_sample()).to_string())];
    for ((file, text), in_memory) in docs.iter().flat_map(|d| [(d, crate::io::MAX_BYTES), (d, 0)]) {
        let p = d.join(file);
        std::fs::write(&p, text).unwrap();
        let (whole, whole_report) = crate::io::read_geojson(text, "x").unwrap();
        // Scanned in memory, and through a buffer, as a file too large to hold is.
        let src = geojson::GeoJsonSource::open_with(&p, "x", in_memory).unwrap();
        let (inplace, report) = materialize(&src).unwrap();
        assert_eq!(inplace.fields, whole.fields, "{file}");
        assert_eq!(inplace.geometry_type, whole.geometry_type, "{file}");
        assert_eq!(inplace.features, whole.features, "{file}");
        assert_eq!(report.messages.len(), whole_report.messages.len(), "{file}: {:?}", report.messages);
        // Extent reads skip features without geometry, and find each by its box.
        let b = src.read(&Request { extent: Some(Extent::new(0.5, 0.5, 1.5, 2.5)), limit: 100 }).unwrap();
        assert!(b.features.iter().all(|f| f.geometry.is_some()), "{file}");
    }
    // Not a FeatureCollection, or not GeoJSON: not read in place.
    for (file, text) in [
        ("one.geojson", r#"{"type":"Feature","properties":{},"geometry":{"type":"Point","coordinates":[1,2]}}"#),
        ("bare.geojson", r#"{"type":"Point","coordinates":[1,2]}"#),
        ("topo.json", r#"{"type":"Topology","objects":{},"arcs":[]}"#),
        ("list.json", "[1, 2]"),
        ("trailing.geojson", r#"{"type":"FeatureCollection","features":[]} {"#),
    ] {
        let p = d.join(file);
        std::fs::write(&p, text).unwrap();
        assert!(open_path(&p, None).is_err() && geojson::GeoJsonSource::open_with(&p, "x", 0).is_err(), "{file}");
    }
}

#[test]
fn damaged_geojson_is_an_error_not_a_panic() {
    let d = dir().join("geojson-junk");
    std::fs::create_dir_all(&d).unwrap();
    let text = crate::io::to_geojson(&export_sample()).to_string();
    let p = d.join("cut.geojson");
    for n in 0..text.len() {
        std::fs::write(&p, &text.as_bytes()[..n]).unwrap();
        for in_memory in [crate::io::MAX_BYTES, 0] {
            if let Ok(s) = geojson::GeoJsonSource::open_with(&p, "x", in_memory) {
                let _ = s.read(&Request::all());
            }
        }
    }
    // A file that changes after it's opened: features are skipped, not misread or a panic.
    std::fs::write(&p, &text).unwrap();
    let src = open_path(&p, None).unwrap();
    std::fs::write(&p, &text.as_bytes()[..text.len() / 2]).unwrap();
    let b = src.read(&Request::all()).unwrap();
    assert!(b.skipped > 0);
}
