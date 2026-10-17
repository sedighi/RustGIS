---
name: add-data
description: Add data to the map — files (GeoJSON, CSV with coordinates, Shapefile, GeoPackage, GeoParquet, FlatGeobuf, KML, GPX, GeoTIFF, LiDAR), web addresses, DuckDB SQL queries, STAC catalogs and Overture Maps — with the right coordinate system. Use for "open", "load", "add", "import", "show this file/URL".
---

# Adding data

Based on the RustGIS user guide, Data (`read_doc` `{"page": "user-guide/data.md"}`). Every format
and its limits are listed in `user-guide/data-formats.md`.

- **A file on this computer:** `layer.addData` `{"path": P}`. Reading files asks the user first.
  A GeoPackage, GDAL or DuckDB file with several layers takes `"layer"`.
- **CSV / TSV:** points come from `"x"` and `"y"` columns, or a `"wkt"` column. When the
  coordinates aren't longitude/latitude, give their `"crs"` (`"EPSG:2271"`). Without it, projected
  coordinates land in the wrong place. Use `"delimiter"` when it isn't a comma.
- **A web address:** `layer.addUrl` `{"url": U}` for GeoJSON, CSV, GeoParquet, FlatGeobuf,
  GeoPackage, zipped Shapefiles, KML/KMZ, GPX, GeoTIFF or a PMTiles archive. This uses the
  network, so the user is asked first.
- **SQL:** `layer.addSql` `{"sql": "SELECT … FROM read_parquet('…')"}` runs one DuckDB query
  (DuckDB Spatial's `ST_` functions; PostgreSQL through `"database"`). It needs DuckDB installed on
  the desktop.
- **Catalogs:** STAC, Overture Maps, Source Cooperative, OpenStreetMap and open-data portals are
  described in `user-guide/data.md`; find their commands with `search_commands` ("stac", "overture",
  "openstreetmap").
- **Web tiles:** basemaps and tile services go through `map.addTiles` `{"url": …}`, not
  `layer.addData`.

After adding: `list_layers` confirms the layer, its geometry type, feature count and fields, and
`layer.zoomTo` frames it. If a layer lands in the wrong place, its coordinate system was wrong:
remove it (`layer.remove`) and add it again with the right `"crs"`. Large files may load a
sample first; `layer.loadAll` loads every feature.
