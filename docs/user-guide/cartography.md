# Cartography

## Symbology

![The Style tab and the Symbology pane with graduated colours; the larger African countries are selected on the map and in the attribute table](https://assets.rustgis.app/images/symbology-table.webp)

Select a layer in Layers and open **Style › Symbology** (or the Symbology pane):

- **Single Symbol**: one fill, outline and marker for every feature.
- **Unique Values**: one colour per distinct value of a field.
- **Graduated Colors**: 1 to 12 classes of a numeric field by natural breaks (Jenks), equal
  interval or quantile.
- **Proportional Symbols**: point size grows with a numeric field.

Pick a colour scheme from the pane's swatches. The Style tab sets the fill **Color**,
the **Outline** width, **Transparency** and a **Visibility Range** (the scales between which the
layer draws); points can be circles, squares, triangles, diamonds or stars. **Data › Definition
Query** draws only the features matching a SQL clause, and a layer with
[subtypes](editing.md#data-design) can be symbolized by them. **Style › Styles** saves symbology
as styles to reuse on other layers and in other projects.

### Custom color ramps

Besides the built-in color schemes, a project can hold your own color ramps: an organization's
palette, a published scheme such as a ColorBrewer, cmocean or viridis ramp, or one designed in a
tool like [Phosphor](https://www.joshuastevens.net/phosphor/). Add them from a color ramp file
with **Style › Styles › Add Color Ramp File** (or **Add Color Ramp File** under the swatches in
the Symbology pane). They appear under **Custom** in the Symbology pane, for feature layers and
rasters (Stretch, Classify, Unique Values and Shaded Relief) alike, and under **Color Ramps** in
the Browser, where right-click deletes one. **Publish › Save As › Color Ramps** saves the
project's custom ramps to a file.

A color ramp file (`.rgisramp`) is JSON: a list of ramps (or one ramp on its own), each with a
`name`, a `kind` (`sequential`, the default, `diverging` or `qualitative`) and its `stops`:

```json
[{
  "name": "Phosphor Frostfire",
  "kind": "diverging",
  "stops": ["#eff6ff", "#abc5e8", "#87a0d7", "#7b5ca6", "#ca7482", "#f5b88e", "#fff5da"]
}]
```

Stops are `#RRGGBB` or `#RRGGBBAA` colors, evenly spaced from the first to the last. For uneven
spacing, give a stop as `{"pos": 0.25, "color": "#87a0d7"}`, with `pos` from 0 to 1; plain
stops between two positioned ones are spaced evenly between them. A ramp has 2 to 256 stops (a
qualitative one can have 1), and can't take the name of a built-in scheme; adding a ramp with
the name of one already in the project replaces it. Scripts, the CLI and the AI Assistant add
ramps with the `ramp.add` command (`{"path": "ramps.rgisramp"}`, or the JSON itself as
`{"text": …}`), and then name them like built-in schemes, for example
`layer.ramp={"ramp":"Phosphor Frostfire"}`.

### Color ramps from other tools

**Add Color Ramp File** also reads the color ramp files other tools write, so a palette from
QGIS, GDAL, GMT or a tool like Phosphor can be added without converting it first:

| Format | Extension | What RustGIS reads |
| --- | --- | --- |
| QGIS style | `.xml` | Every gradient ramp (discrete ones as steps) and preset ramp (as a qualitative ramp) in a Style Manager export, with its name. ColorBrewer, cpt-city and random ramps hold no colors and are skipped. |
| QGIS layer style | `.qml` | The layer's color ramp, or a raster's color map. |
| GDAL color-relief text | `.txt` | `value R G B [A]` lines (as `gdaldem color-relief` takes them), separated by spaces, tabs, commas or colons; values may be percentages and colors may be names; `nv` lines are skipped. A QGIS color map export reads the same way. |
| GMT palette | `.cpt` | `z0 color0 z1 color1` lines, with colors as `r/g/b`, `r g b`, `#RRGGBB`, grey levels, names or HSV; `B`, `F` and `N` lines are skipped. |

These formats tie colors to data values; the ramp keeps their spacing, from the lowest value
to the highest, and a sharp change between two colors stays sharp. A ramp from a GDAL or GMT
file is named after the file and is sequential; the `ramp.add` command takes a `name` and a
`kind` to change that (`{"path": "relief.cpt", "kind": "diverging"}`), and a `format`
(`rgisramp`, `qgis`, `gdal` or `cpt`) for text without a file name. A file with more than 256
colors is resampled to 256 evenly spaced stops.

Sequential and diverging ramps blend between their stops; when there are as many classes as
stops, each class takes a stop's color exactly, so a stepped palette keeps its colors. A
qualitative ramp gives each class the next stop, starting again after the last. A layer keeps
its own copy of the ramp it uses, so layer files, map files and packages keep their colors in
projects that don't have the ramp, and deleting a ramp from the project leaves layers that use
it unchanged.

## Labeling

Turn labels on per layer (View › Labeling › Label, or the Style tab), then:

- **Visibility Range**: draw labels only between two scales (Out Beyond / In Beyond), or between
  two viewing distances (Closest / Farthest). Clear Limits removes all four.
- **Placement**: automatic (above points, centred on lines and polygons), centred, or one of
  eight positions around the feature.
- **Lock** keeps the labelled features fixed while you pan and zoom; **Pause** stops drawing
  labels for faster navigation; **View Unplaced** draws the labels that didn't fit in red.
- **Convert** turns a layer's labels into an annotation layer: one point per label, carrying its
  text in a `TEXT` field, which you can then edit, move or delete like any other features.

## Layer effects

These draw on top of the symbology. On the Style tab: **Masking** shows a layer only inside another layer's polygons;
**Display Filters** show different features at different scales (a SQL condition per scale
range); **Aggregation** draws points as hexagon or square bins coloured by count; **Extrusion**
raises polygons by a height or a numeric field in an oblique 2.5D view, with **Face Culling**,
**Lighting** (walls shaded by the way they face the light) and **Cast Shadow**. On the Raster
Layer tab: **Layer Blend** (multiply, screen, overlay, darken, lighten, difference), **Swipe**
(the layer shows only left of a divider you slide, to compare it with what's beneath),
**Masking** and **Rotation**.

## Graphics

Edit › Graphics draws shapes in a graphics layer (Home › Add Data › Graphics Layer, or one is
made with the first graphic). Point, Line, Curve and Polygon are clicked vertex by vertex
(double-click to finish); Freehand, Lasso, Rectangle, Circle and Ellipse are dragged. Text places
a label where you click, and Dynamic Text adds tokens that update as the map changes: `{map}`,
`{scale}`, `{date}`, `{time}`, `{crs}`, `{layers}` and `{center}`. Picture: drag a rectangle,
then pick a PNG or JPEG image to stretch over it. Tools return to Explore after each graphic
unless Keep Tool is on. Each graphic keeps its own fill, outline, width and text size in the
layer's attributes, so you can edit them in the attribute table or with `graphics.style`.
