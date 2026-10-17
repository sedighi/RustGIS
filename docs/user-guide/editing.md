# Editing

## Creating features

**Edit › Create** draws new points, lines and polygons into the layer selected in Layers:
click each vertex and double-click to finish (++esc++ cancels). **Delete** (++delete++) removes
the selected features. Edit attributes in the attribute table (++ctrl+t++): double-click a cell
to change it. **Data › Data Design** (and **Data › Fields**) adds and deletes fields and calculates a
field from an expression; **Calculate Geometry** fills a field with each feature's area, length or
coordinates. Every edit is one undo step.

## Modifying features

The Edit tab's **Edit** button turns editing on or off for the map; with it off, the map is
read-only. The Editing pane (Settings, Templates, Status, Modify) holds the rest:

- **Modify**: move the selected features by metres east and north, rotate them, scale them,
  merge them (polygons are unioned), explode multipart features, split a line where you click,
  and flip line direction. Edit › Tools lists every editing tool.
- **Feature templates**: named default values for new features, made from a selected feature;
  the Create tool uses the chosen template.
- **Snapping**: new vertices snap to nearby vertices, edges and line ends (magenta marker).
- **Elevation**: new features get a `z` value, constant or sampled from a raster surface.
- **Ground to grid**: a scale factor and rotation applied to entered distances and directions
  (Move, and `edit.traverse`, which draws a line or polygon from distances and bearings).
- **Topology**: rules (must not overlap, must not have gaps, must not self-intersect, must not
  have dangles, must be covered by); Validate lists the errors, and clicking one zooms to it.
- **Status**: features added, changed and deleted per layer since the last save.

## Data design

The Data tab's Data Design pane keeps attributes valid, on every edit (creating, pasting,
attribute edits, Calculate Field):

- **Domains**: coded values (`code=description`) or a numeric range for a field.
- **Subtypes**: an integer field's codes, each with a name and default values for new features;
  the layer is symbolized by subtype.
- **Attribute rules**: calculations (a field set from an expression on every insert and update)
  and constraints (a SQL condition every feature must meet).
- **Contingent values**: the combinations of values allowed across fields.
- **Archiving**: keeps every edit with its time; restore the layer as it was minutes ago.
- **Relationships**: link two layers by key fields; **Related Data** selects the rows related to
  the selection.

## Clipboard and pop-ups

Copy (++ctrl+c++) and Cut (++ctrl+x++) take the selected features. Paste (++ctrl+v++) adds them to
the layer selected in Layers, matching fields by name; Paste Special adds them as a new layer.
Copied features also go on the system clipboard as GeoJSON, so they can be pasted into another
RustGIS window or a text editor, and GeoJSON copied elsewhere pastes into RustGIS. Copy Path copies
the selected layer's data source path.

Pop-ups (Home › Navigate) choose which layers a click with Explore identifies: the topmost layer
under the click, every visible layer, only selectable layers, only the layer selected in Layers, or
nothing. The pop-up lists every feature under the click in those layers, overlapping ones included
(up to 100): step through them with ‹ ›. The feature shown is the one selected. The magnifier in
the pop-up's header zooms to it.
