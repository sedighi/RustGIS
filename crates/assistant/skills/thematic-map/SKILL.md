---
name: thematic-map
description: Style a layer by its data (graduated colours, categories, proportional symbols), label it and export the map. Use when asked to "map", "colour by", "show … by …" or make a choropleth.
---

# Thematic map

1. Find the layer and field: `list_layers`, then `read_features` with a small `limit` to see the values. A numeric
   field suits graduated or proportional symbols; text or a few distinct values suit unique values.
2. Style it with `run_command` `layer.symbology`:
   - Numbers: `{"layer": L, "type": "graduated", "field": F, "classes": 5, "method": "naturalBreaks"}` (or
     `quantile`, `equalInterval`), with an optional `"ramp"`.
     For counts or sizes, `"type": "proportional"` usually reads better than colour.
   - Categories: `{"layer": L, "type": "unique", "field": F}`. More than about 12 categories is hard to read, so
     suggest grouping first.
   - Colours: `layer.ramp` changes the ramp afterwards (`search_commands` "color scheme" for the names).
     Use sequential ramps for amounts, diverging ones for values around a midpoint.
3. Context: dim other layers with `layer.transparency`, or hide them with `layer.visibility`. `map.basemap` `paper`
   keeps a choropleth readable.
4. Labels, if they help: `labels.field`, then `labels.enable` `{"on": true}`. Keep them sparse with `labels.style`
   (size, halo) or a visibility range.
5. Frame the view with `layer.zoomTo`, then export with `share.exportMap` `{"path": "map.png"}`. Exporting writes a
   file, so the user is asked first. For a page with a legend and title, load the `print-layout` skill.

Say what you did in a sentence or two: which field, how many classes, which method. Everything can be undone.
