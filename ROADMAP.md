# Roadmap

Many features are first versions: there are no full 3D scenes or enterprise portal sharing yet.

## Done
- Layered workspace, command registry with undo, project files (`.rgis`), `rustgis-cli`, the
  web app (WebAssembly, installable with offline start, opened with a project or data by its
  web address), 15 interface languages; a ribbon laid out in the order of the work
  (Home, Data, Style, Edit, Analyze, View, Publish), start-up preferences, Check for Updates.
- Data: files by their web address; GeoJSON, CSV/TSV (x/y in any EPSG system, or WKT), Excel and OpenDocument spreadsheets,
  GeoRSS, LandXML, encoded polylines, Shapefile (zipped too, with the coordinate system in its
  `.prj`), GeoPackage, GeoParquet, FlatGeobuf, KML/KMZ (styles included), GPX, GML, MapInfo MIF, OSM PBF, TopoJSON, DXF and LiDAR (LAS/LAZ/COPC) in, all of them
  in the web app too (large files read in place, a view at a time); the vector formats GDAL
  reads when it is installed, PostGIS tables read live from PostgreSQL (desktop), and SQL queries
  and DuckDB databases (DuckDB Spatial) when DuckDB is; GeoJSON, CSV, GeoPackage, GeoParquet, Shapefile, FlatGeobuf, KML/KMZ, GPX and WKT out;
  standalone tables; geotagged photos and KML ground overlays; geocoding (Find Place, address
  tables); a Data Sources pane (favourites, recent, saved services, a library) and an SQL pane
  (DuckDB Spatial); layer, map, style, template, task and package files.
- Open data: STAC search (Planetary Computer assets), Overture Maps, Source Cooperative,
  OpenStreetMap by area (Overpass), geoBoundaries, CKAN, Socrata and ArcGIS Hub portals, and
  S3 buckets with signed connections and a bucket browser.
- Rasters: GeoTIFF/COG (by web address, read in place by range; Zstandard too) and ASCII grids,
  Zarr 2 and 3 stores (a variable's step, by folder or web address), and JPEG 2000, NetCDF, HDF, ERDAS Imagine… through GDAL,
  EPSG reprojection, stretch/RGB/classified/shaded-relief
  symbology, raster functions, indices, change detection, classification, georeferencing.
- Services: WMS, WMTS, WFS, WCS, OGC API Features and Tiles, and ArcGIS REST, listed from their
  capabilities; CSW catalogue search.
- Maps: multiple maps, a GPU renderer (beta) with smooth zoom and pan, a 3D globe with the flat
  map draped over it, projection presets and EPSG codes (features cut along the edge of world
  projections), a search box for layers, places and coordinates, bookmarks, web tile
  layers (XYZ/TMS/WMS, Mapbox Vector Tiles, MapLibre styles, PMTiles and MBTiles archives),
  a basemap gallery (OpenFreeMap, USGS Imagery), worldviews.
- Cartography: single, unique values, graduated colours and proportional symbols; labels with
  placement and annotation; custom colour ramps (and QGIS, GDAL and GMT palette files);
  masking, display filters, aggregation, extrusion; graphics.
- Editing: create, move, rotate, scale, merge, split, explode; snapping, templates, elevation,
  topology; domains, subtypes, attribute rules, contingent values, relationships, archiving.
- Analysis: selection by attributes and location; overlay, proximity and summary tools; guided
  workflows (viewshed, line of sight, IDW, shortest path, suitability); ModelBuilder and
  scripts; the GeoLibre and Whitebox suite (1,000+ tools) natively and in the browser.
- Layouts (map frames, surrounds, map series, PDF), reports, presentations and notebooks.
- Sharing as open formats (web maps, OGC API Features collections, MapLibre styles, glTF
  scenes, locators), offline areas with sync, deep learning pixel classification, hosted
  imagery tiles.
- The AI Assistant (Anthropic, OpenAI, ChatGPT sign-in, Gemini, OpenRouter, Ollama, OpenAI-compatible) in the
  app, the web app and `rustgis-cli ask`; an MCP server (`rustgis-cli mcp`) for AI agents such
  as Claude Code, Cursor and VS Code, with the documentation built in.
- The AI Assistant's agent skill and loopback relay, which let external AI clients drive the app.
- Sandboxed WebAssembly plugins with a registry at plugins.rustgis.app; process plugins that run
  as programs (with Python environments RustGIS makes), in the background with progress and Cancel.
- The `rustgis` Python package on PyPI: the engine in-process, and `rustgis.Map` in Jupyter.
- Desktop packages: Windows installers (setup.exe, .msi), a portable zip and a Store MSIX; a
  signed and notarized macOS DMG; Linux .deb, .rpm, AppImage and tarballs; Homebrew, the AUR,
  Fedora COPR and winget; a download mirror at downloads.rustgis.app.

## Next
- Editing: vertex editing (move, add and delete vertices of existing features).
- Analysis: spatial join, intersect and union, near, heat maps.
- 3D: full scenes with terrain, beyond the globe and 2.5D extrusion.
