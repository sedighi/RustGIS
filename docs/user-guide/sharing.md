# Sharing

## Exporting

**Publish › Output › Export Map** writes the map view as a PNG at any DPI, with labels, the scale
bar and tile credits (`share.exportMap` on the command line); **Print Map** sends it through
the system's PDF viewer, **Capture To Clipboard** copies it, and **Export To 3D** writes a layer
as a Wavefront OBJ model with polygons extruded by a field. **Data › Export** (or the Export
Features tool) writes a layer to GeoJSON, CSV, GeoPackage, GeoParquet, Shapefile, FlatGeobuf, KML,
KMZ or GPX (see [Writing](data-formats.md#writing)), **Data › Export** a raster to GeoTIFF, and
**Data Interop** converts layers to KML, GPX or CSV with WKT. Layouts, reports and presentations
export to PDF from their own tabs and panes (see [Layouts and Reports](layouts.md)).

Packages and project item files are described under [Project items](data.md#project-items).

## Sharing as open formats

**Publish › Share As** writes open-format files that work without a server, to a chosen file or the
first **data store** (a folder registered under **Publish › Manage › Data Stores**):

- **Web Map**: a self-contained HTML page with the map's layers (rasters as images), pan and zoom,
  a layer list and pop-ups.
- **Web Layer**: the selected layer as a static OGC API – Features collection, with its style and
  a viewer page, in a zip. **Geodata Service** does the same for every layer and table.
- **Web Style**: the selected layer's symbology as a MapLibre style (unique values and classes as
  expressions).
- **Locator**: the selected layer's names as a place-search file; `locator.find` searches it and
  can zoom to the best match.
- **Web Tool**: the last model as a tool file, which `model.import` adds to another project.
- **Web Scene**: the map in 3D as glTF, polygons extruded by their layer's extrusion setting.
- **Web Link Chart**: the features joined by the map's relationships, as a node-link page.

**Status › Jobs** (the Shared Items pane) lists every share; **Save Web Map**, **Replace Web
Layer**, **Save Web Scene** and **Save Web Link Chart** update a share in place, and **Upload File**
copies a file into a data store.
