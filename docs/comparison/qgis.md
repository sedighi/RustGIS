# RustGIS and QGIS

"How does RustGIS compare to QGIS?" is the question people ask most. This page answers it
directly: where each one is stronger, how QGIS concepts map onto RustGIS, and how to use the two
together. For RustGIS beside GeoLibre, ArcGIS Pro and ArcGIS Online as well, see the
[full comparison](index.md).

!!! note "How to read this"
    This page is written by the RustGIS project, so read it with that in mind. It reflects
    RustGIS as of **October 2026** and QGIS's out-of-the-box behaviour, not everything
    reachable through its plugins. Found something wrong or out of date? Please
    [open an issue](https://github.com/opengeos/RustGIS/issues).

## The short answer

**QGIS is the mature, complete open-source desktop GIS.** Over twenty years it has grown the
widest format support (through GDAL/OGR) and full coordinate-system support (through PROJ), the
deepest open-source cartography, thorough editing, GRASS and SAGA processing, QGIS Server,
mobile companions (QField, Mergin Maps) and thousands of Python plugins, with a large community,
training material and books behind it.

**RustGIS is younger and narrower, built differently.** It is one native program written
entirely in Rust, an 18–30 MB download that also runs as the whole app in a browser tab. Its
interface is a ribbon laid out in the order of the work, every button is a command you can run
from the command line, Python or an AI agent, and its plugins run sandboxed.

RustGIS is not a QGIS replacement, and doesn't aim to be. If your work depends on what QGIS does
best, keep using QGIS. RustGIS suits people who want something lighter to install, a browser
version with nothing to install, a command-driven engine, or a ribbon workflow without a
licence, and the two read and write the same open formats.

## What they look like

The same World Atlas data, styled the same way, in each app.

[![RustGIS with the World Atlas on the globe: the ribbon, the Layers pane and the AI Assistant](https://assets.rustgis.app/images/hero.webp)](https://assets.rustgis.app/images/hero.webp){ target="_blank" rel="noopener" title="Open the full-size image" }
*RustGIS: the ribbon, the Layers pane and the AI Assistant.*

[![QGIS with the same World Atlas data on an orthographic globe: toolbars, the Browser and Layers panels](https://assets.rustgis.app/images/qgis-world-atlas.webp)](https://assets.rustgis.app/images/qgis-world-atlas.webp){ target="_blank" rel="noopener" title="Open the full-size image" }
*QGIS with the same data and styling: toolbars and the Browser and Layers panels.*

## Where RustGIS is stronger

- **Installing it.** RustGIS is a single program with no Python, Qt or GDAL inside: the
  installers are 18–30 MB depending on the platform (a portable Windows zip, a macOS DMG, an
  AppImage, `.deb`/`.rpm`, Homebrew, winget, the AUR, Fedora COPR). QGIS installers bundle a
  Python interpreter, Qt, GDAL, GRASS and more, and run to hundreds of megabytes. See
  [Installation](../installation.md).
- **In the browser.** The whole app, not a viewer, is compiled to WebAssembly and runs at
  [web.rustgis.app](https://web.rustgis.app): add files, style them, run the 1,000+ GeoLibre and
  Whitebox tools, build layouts and export, with the data staying on your device. You can serve
  the static build yourself. QGIS has no official browser version; QGIS Server and qgis2web
  publish maps made on the desktop.
- **Everything is a command.** Every ribbon button, shortcut and pane action is a command with an
  id and JSON parameters. The same ids run from [`rustgis-cli`](../cli.md), the
  [`rustgis` Python package](../python.md), notebooks, models, the Script pane, the
  [MCP server](../mcp.md) and the AI Assistant, so what you can click you can script, exactly.
  QGIS's processing algorithms are scriptable through `qgis_process` and PyQGIS, but much of
  its interface (styling, layouts, project setup) is reached through the PyQGIS object model
  rather than the same actions the buttons run.
- **An AI Assistant that acts through undoable commands.** The [AI Assistant](../user-guide/assistant.md)
  is built in, works with Anthropic, OpenAI, Gemini, OpenRouter, Ollama or any OpenAI-compatible
  model with your own key, and does everything through the same commands, so every step shows
  in History and can be undone; file, network and plugin commands wait for your approval. An
  [MCP server](../mcp.md) lets agents such as Claude Code, Cursor and VS Code drive RustGIS.
  QGIS has AI through third-party plugins.
- **Sandboxed plugins.** RustGIS [plugins](../plugins.md) are WebAssembly modules that can do
  only what their declared permissions allow (editing the project, reading files, the network),
  and the same file runs on the desktop, in `rustgis-cli` and in the browser. QGIS plugins are
  Python with the full access of the user running QGIS: far more capable, but trusted rather
  than contained.
- **Cloud-native data without extra libraries.** Cloud-optimised GeoTIFF, GeoParquet,
  FlatGeobuf, PMTiles and Zarr are read in place by RustGIS's own readers, over HTTP byte ranges
  and from `s3://` buckets, on the desktop and in the browser alike. QGIS reads all of these too,
  through GDAL on the desktop.
- **A ribbon interface.** Tabs follow the order of the work (Home, Data, Style, Edit, Analyze,
  View, Publish), with Command Search (++ctrl+k++) to find any command or tool by name. People
  coming from ArcGIS Pro often find it familiar; people who know QGIS's toolbars may not want it,
  which is a matter of taste.
- **A permissive licence.** RustGIS is Apache-2.0, so its crates can be embedded in proprietary
  or differently licensed software. QGIS is GPL-2.0-or-later, which requires derived works to
  stay under the GPL. For running either program, both are free.
- **Built not to crash.** RustGIS is memory-safe Rust with no `unsafe` code, treats every file
  and parameter as hostile, and its test suite runs every command and tool with junk input to
  check that none of them panics.

## Where QGIS is stronger

- **Format and database breadth.** QGIS opens the formats its GDAL/OGR build supports, natively,
  and writes many of them. RustGIS reads a focused set of formats with its own code (see
  [Data formats](../user-guide/data-formats.md)) and reaches the rest, such as File Geodatabase,
  DWG, JPEG 2000 or NetCDF, only when GDAL's programs are installed. QGIS also connects to
  PostGIS, SpatiaLite, Oracle and SQL Server and edits in them directly; RustGIS reads
  PostGIS tables live (read only) and DuckDB databases through DuckDB, on the desktop.
- **Coordinate systems.** QGIS uses full PROJ: every CRS, datum transformations and grids, and
  layers kept in their own CRS. RustGIS holds vector coordinates as WGS 84 longitude/latitude
  and reprojects rasters and the map by EPSG code, which covers mapping and most analysis but
  not survey-grade work in a local datum.
- **Editing.** QGIS has a complete digitizing toolkit: vertex editing, reshape, offset curves,
  advanced digitizing with constraints, topological editing, geometry checkers. RustGIS creates,
  moves, rotates, scales, splits and merges features with snapping and templates, but cannot yet
  edit the vertices of existing features.
- **Processing.** Both have over 1,000 tools, but they differ. QGIS combines its native
  algorithms with GDAL, GRASS and SAGA providers, and covers vector analysis RustGIS is thin
  on: spatial joins, intersect and union overlays, network analysis on real road graphs, spatial
  statistics. RustGIS's breadth comes mostly from the GeoLibre and Whitebox suite, which is
  strong on terrain, hydrology, LiDAR and image analysis.
- **Cartography.** QGIS's styling is the richest in open source: rule-based renderers, data-defined
  overrides on nearly every property through its expression engine, blending modes, geometry
  generators, a refined labeling engine. RustGIS covers the common renderers (single, unique
  values, graduated, proportional, aggregation, raster stretch/RGB/classified/hillshade) and
  collision-free labels, without that depth.
- **Print layouts.** QGIS's layout designer and atlas are mature and detailed. RustGIS has map
  frames, legends, scale bars, grids, map series, reports and PDF export, as first versions.
- **Plugins and integrations.** QGIS has thousands of Python plugins for niche tasks, an
  in-app Python console, QGIS Server for OGC services, and QField and Mergin Maps for field
  work. RustGIS's plugin registry is new and small, and it has no server or mobile app.
- **3D and point clouds.** QGIS's 3D map views, mesh layers and point-cloud rendering go
  further than RustGIS's globe, 2.5D extrusion and LiDAR point layers.
- **Maturity and community.** QGIS has two decades of releases, a long-term-release cycle,
  translated documentation, courses, books, user groups and commercial support. RustGIS
  released its first version in 2026; many features are first versions and some workflows will
  have gaps. See the [roadmap](https://github.com/opengeos/RustGIS/blob/main/ROADMAP.md).

## Side by side

| | **RustGIS** | **QGIS** |
|---|---|---|
| **Licence** | Apache-2.0 | GPL-2.0-or-later |
| **Written in** | Rust (egui, wgpu, tiny-skia) | C++ and Python (Qt) |
| **First release** | 2026 | 2002 |
| **Download** | 18–30 MB, one program | Hundreds of MB (bundles Python, Qt, GDAL, GRASS…) |
| **Platforms** | Windows, macOS, Linux, and the browser (WebAssembly) | Windows, macOS, Linux |
| **Interface** | Ribbon with Command Search | Toolbars, menus and docked panels |
| **Formats** | A focused set read natively; more through GDAL when installed | Whatever its GDAL/OGR build supports (read/write varies by driver) |
| **Coordinate systems** | Vectors in WGS 84; rasters and the map by EPSG code | Full PROJ |
| **Geoprocessing** | 1,000+ (GeoLibre and Whitebox) plus built-in vector tools | 1,000+ (native, GDAL, GRASS, SAGA) |
| **Editing** | Create, move, rotate, scale, split, merge, snapping; no vertex editing yet | Complete digitizing and topology tools |
| **Scripting** | Every action a command: CLI, Python package, notebooks, Script pane, MCP | PyQGIS, `qgis_process`, Python console |
| **Plugins** | Sandboxed WebAssembly, small registry | Python, thousands in the official repository |
| **AI** | Built-in assistant (undoable commands) and an MCP server | Through plugins |
| **Server and mobile** | Static web build you can host | QGIS Server; QField and Mergin Maps |
| **Project file** | `.rgis` (open JSON) | `.qgs` / `.qgz` (open XML) |

## RustGIS for QGIS users

Most QGIS ideas have a counterpart under a different name:

| In QGIS | In RustGIS |
|---|---|
| Layers panel | Layers pane (left) |
| Browser panel, Data Source Manager | Catalog pane, **Home › Add Data**, the Data Sources pane |
| Processing Toolbox | Geoprocessing pane (**Analyze › Tools**), or Command Search (++ctrl+k++) |
| Graphical Modeler | ModelBuilder, which can also build a model from your history |
| `qgis_process` | `rustgis-cli run --cmd 'gp.run={…}'` |
| Python console, PyQGIS | The Script pane and notebooks inside the app; the `rustgis` Python package outside it |
| Layer Styling panel | **Style** tab and the Symbology pane |
| Expressions (select, filter) | SQL `WHERE` clauses for selections and definition queries |
| Field Calculator | Calculate Field |
| Print Layout, Atlas | Layouts, Map Series |
| Plugin Manager | **Plugins › Plugin Manager** |
| Project (`.qgz`) | Project (`.rgis`), or a package (`.rgispkg`) with its data inside |

The [User Guide](../user-guide/index.md) walks through the interface, and
[Getting Started](../getting-started.md) is a short tour.

## Using them together

You don't have to choose. RustGIS reads and writes the formats QGIS works in, so data moves
between them without conversion:

- **Vector data**: GeoPackage, GeoParquet, FlatGeobuf, Shapefile, GeoJSON, KML/KMZ, GPX and
  CSV, in both directions.
- **Rasters**: GeoTIFF and cloud-optimised GeoTIFF.
- **Colour ramps**: RustGIS reads QGIS Style Manager exports (`.xml`) and the colour ramp or
  raster colour map in a layer style (`.qml`); see
  [Custom color ramps](../user-guide/cartography.md#custom-color-ramps).
- **Web maps**: MapLibre styles and OGC API Features collections exported from RustGIS can be
  served and opened anywhere, QGIS included.

RustGIS does not open QGIS project files (`.qgz`) or full QGIS layer styles; move the data
across and restyle it.

A common split is to do heavy editing, PROJ-dependent work and detailed cartography in QGIS,
and use RustGIS where a light install, the browser, scripting by command id or the AI Assistant
helps: a quick look at a GeoParquet on a laptop without QGIS, a map shared as a web app, a batch
of maps rendered by `rustgis-cli` in CI, or the 1,000+ GeoLibre and Whitebox tools from a
browser tab.

## Common questions

**Is RustGIS a replacement for QGIS?**
No. It covers the everyday work of a desktop GIS (adding data, styling, labelling, geoprocessing,
layouts, sharing) and does some things QGIS doesn't, but QGIS is far more complete. Use RustGIS
alongside QGIS, or instead of it only if the gaps above don't touch your work.

**Is RustGIS faster than QGIS?**
It starts quickly and is light to install, and its own readers draw large cloud-native files a
view at a time. We haven't published benchmarks against QGIS, and speed depends on the data and
the task, so try your own data in both.

**Can I use my QGIS plugins in RustGIS?**
No. QGIS plugins are Python written against PyQGIS. RustGIS plugins are sandboxed WebAssembly
modules; see [Plugins](../plugins.md) for writing one.

**Does RustGIS need GDAL like QGIS does?**
No. Its common formats, cloud-native formats and GeoTIFF are read by its own Rust code, which is
also why the browser version works. GDAL is optional: when its programs are installed, RustGIS
uses them for the formats it doesn't read itself.

**Can I open a QGIS project?**
Not yet. Open the project's data in RustGIS (GeoPackage is the easiest route) and restyle it.

## See also

- [Full comparison](index.md): RustGIS beside GeoLibre, QGIS, ArcGIS Pro and ArcGIS Online
- [Installation](../installation.md) and [Getting Started](../getting-started.md)
- [Data formats](../user-guide/data-formats.md): what RustGIS reads and writes
- [Command Line](../cli.md) and [Python](../python.md): scripting RustGIS
