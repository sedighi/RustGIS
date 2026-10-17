---
name: share-web-map
description: Share the map or a layer as files that work without a server — a web map (HTML page), a web layer (OGC API Features collection), a MapLibre style, a map image — and data stores. Use for "share", "publish", "web map", "send this map", "export as HTML", "MapLibre style".
---

# Sharing a web map

Based on the RustGIS user guide, Sharing (`read_doc` `{"page": "user-guide/sharing.md"}`).

| Want | Command | Writes |
| --- | --- | --- |
| A page anyone can open, with pan and zoom, a layer list and pop-ups | `share.webMap` `{"path": "map.html"}` | A self-contained HTML page (rasters as images) |
| One layer, with its style and a viewer | `share.webLayer` `{"layer": L, "path": "layer.zip"}` | A static OGC API – Features collection, zipped |
| Every layer and table | `share.geodataService` `{"path": "service.zip"}` | OGC API – Features collections |
| A layer's symbology for MapLibre | `share.webStyle` `{"layer": L, "path": "style.json"}` | A MapLibre style |
| A picture of the map | `share.exportMap` `{"path": "map.png", "dpi": 150}` | A PNG with labels, scale bar and credits |
| A layer's data | `share.exportLayer` `{"layer": L, "path": "out.gpkg"}` | GeoJSON, CSV, GeoPackage, GeoParquet, Shapefile, FlatGeobuf, KML, GPX |

- Every share writes files, so the user is asked first. Without a `path`, a share goes to the first
  **data store**: `share.dataStore` with no parameters lists them, and `{"name", "path"}` adds a
  folder as one.
- Before sharing, get the map right: symbology, labels, the layers' visibility and the view.
- `share.jobs` lists what was shared. `share.saveWebMap` and `share.replaceWebLayer` update a share
  in place.

Tell the user what was written and where, and that the HTML page opens in any browser without a
server.
