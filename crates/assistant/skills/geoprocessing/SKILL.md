---
name: geoprocessing
description: Run geoprocessing tools and chain them — buffer, clip, erase, dissolve, select by location or attributes, summarize within, summary statistics, simplify, export features. Use for "within N km of", "inside", "merge by", "count per", "overlay", or any multi-step analysis.
---

# Geoprocessing

Based on the RustGIS user guide, Analysis › Geoprocessing tools
(`read_doc` `{"page": "user-guide/analysis.md", "section": "Geoprocessing tools"}`).

1. **Find the tool:** `search_tools` with words ("buffer", "overlay", "count points"), then
   `describe_tool` for its parameters. Never guess parameter names. The built-in tools are:
   - proximity: `buffer` (`input`, `distance`, `units`, `dissolve`)
   - overlay: `clip` (`input`, `clip`), `erase` (`input`, `erase`)
   - generalization: `dissolve` (`input`, `field`), `simplify` (`input`, `tolerance`)
   - geometry: `convexHull`, `featureToPoint`, `xyTableToPoint` (`input`, `x`, `y`, `crs`),
     `calculateGeometry`
   - selection: `selectByLocation` (`input`, `select`, `distance`, `invert`), `selectByAttributes`
     (`input`, `where`)
   - statistics: `countPoints` (Summarize Within: `polygons`, `points`), `summaryStatistics`
     (`input`, `field`, `case`)
   - conversion: `exportFeatures` (`input`, `where`)
2. **Run it:** use `run_tool` with layers named in `params`. The output is a new layer, and the
   result names it: use that name as the next step's input. Give `output` a clear name when the
   chain is long.
3. **The selection:** tools process only the selected features when a layer has a selection. Clear it
   with `select.clear` first unless the user meant the selection.
4. **Units:** `buffer` distances take `units`. Check `describe_tool` for the default and say which
   you used.
5. **Check:** read the output with `list_layers` (feature count) or `read_features` before going on,
   and report counts and the names of the layers made. Each run is one undo step.

For raster analysis (slope, NDVI, viewshed, suitability), the guided workflows are in
`user-guide/analysis.md` › Analysis workflows. The GeoLibre/Whitebox tools are in
`user-guide/analysis.md` › GeoLibre tools; find them with `search_tools` too.
