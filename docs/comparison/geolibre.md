# RustGIS and GeoLibre

RustGIS and [GeoLibre](https://geolibre.app) are sibling [opengeos](https://github.com/opengeos)
projects with the same maintainer, so people often ask how they differ and which to use. This
page answers that: what each is built for, where each is stronger, and how to use them together.
For RustGIS beside QGIS, ArcGIS Pro and ArcGIS Online as well, see the
[full comparison](index.md).

!!! note "How to read this"
    This page is written by the RustGIS project. It reflects RustGIS as of **October 2026** and
    GeoLibre 3.4 as described in [its documentation](https://geolibre.app), not every
    plugin either one can load. Both projects move quickly, so details here age fast. Found
    something wrong or out of date? Please
    [open an issue](https://github.com/opengeos/RustGIS/issues).

## The short answer

**GeoLibre is the broader, web-first GIS.** It is built with web technology (Tauri, React,
MapLibre GL JS, DuckDB-WASM, deck.gl) and runs in a browser tab with nothing installed, as a
desktop app, and as native Android and iOS apps. It reaches further into data (databases,
cloud catalogs, dozens of open-data portals), 3D (Cesium, 3D Tiles, I3S, Gaussian splats), AI
(SAM segmentation, ONNX object detection), sharing (share links, review comments, real-time
collaboration) and the Python and R ecosystems (a Jupyter widget and an R package).

**RustGIS is the narrower, native one.** It is a single program written entirely in Rust that
draws its own interface on the GPU, with no browser engine or JavaScript runtime inside, and the
same program compiled to WebAssembly for the browser. Its interface is a ribbon laid out in the
order of the work, every button is a command you can run from the command line, Python or an AI
agent, its plugins run sandboxed, and it goes further in the classic desktop GIS jobs of data
design, reports and print layouts.

They are complementary rather than competing. Both run the same 1,000+ GeoLibre and Whitebox
geoprocessing tools, both keep your data on your device, and both read and write the same open
formats. Choose GeoLibre for reach, choose RustGIS for a compact, scriptable desktop GIS, or use
both.

## Where GeoLibre is stronger

- **More platforms.** GeoLibre runs as a web app, an installable PWA, a desktop app on Windows,
  macOS and Linux, native [Android and iOS apps](https://geolibre.app), a Docker image, a Chrome
  extension and a Jupyter widget. RustGIS runs on the desktop and in the browser; it has no
  mobile app and no touch-first layout.
- **Data reach.** GeoLibre opens more formats in the app itself, including File Geodatabase,
  DWG, MapInfo TAB, NetCDF-4/HDF5, Kerchunk and Icechunk, ICESat-2 and GEDI. It connects to
  SQL Server and Apache Iceberg as well as PostGIS and DuckDB, and its plugins browse dozens of
  catalogs and portals, from NASA Earthdata and Planet to Hugging Face and about 150 US local
  agencies. RustGIS reads a focused set natively and the rest through GDAL's programs when they
  are installed (see [Data formats](../user-guide/data-formats.md)).
- **SQL in the browser.** GeoLibre runs DuckDB Spatial, PGlite/PostGIS and Apache Sedona inside
  the browser. RustGIS's SQL pane uses DuckDB on the desktop; its web app has no SQL engine yet.
- **Cartography depth.** GeoLibre has rule-based and expression renderers, heat maps,
  clustering, diagrams, blend modes, layer groups, a data-defined label engine, and style
  interchange with SLD, QGIS QML and Mapbox GL JSON. RustGIS covers the common renderers and
  collision-free labels, without that depth.
- **3D.** GeoLibre has a Cesium globe with terrain, 3D Tiles, I3S, Gaussian splats, CZML and
  Google Photorealistic 3D Tiles. RustGIS has a globe, 2.5D extrusion and LiDAR point layers.
- **AI and imagery models.** Besides an AI assistant, GeoLibre has SAM segmentation, in-browser
  ONNX/YOLO object detection and an object-based image analysis workbench. RustGIS has pixel
  classification (k-means, minimum distance and a trainable neural network).
- **Sharing and collaboration.** GeoLibre shares projects through links with roles, expiry and
  passwords, has review comments anchored to the map, story maps, dashboards, and an
  experimental real-time collaboration mode. RustGIS shares by writing files: web map pages,
  packages and static exports.
- **Python, R and notebooks.** The `geolibre` Python package is a two-way Jupyter widget with a
  leafmap-style API, the `geolibre` R package works in RStudio, Quarto and Shiny, and the
  notebook panel runs JupyterLite or JupyterLab. RustGIS has a [Python package](../python.md)
  that runs commands headlessly and built-in notebooks of command cells, but no Python console
  in the app and no R package.
- **Opening other projects.** GeoLibre imports QGIS (`.qgs`/`.qgz`) and ArcGIS Pro
  (`.aprx`/`.mapx`) projects. RustGIS opens neither.
- **Editing.** GeoLibre edits vertices and writes edits back to their sources (GeoPackage,
  PostGIS, ArcGIS feature services). RustGIS cannot yet edit the vertices of existing features,
  and edits stay in the project until exported.
- **Languages.** GeoLibre's interface comes in 21 languages, including right-to-left Arabic,
  Hebrew and Persian. RustGIS has [16](../i18n.md), none right-to-left.

## Where RustGIS is stronger

- **A native program, not a web view.** RustGIS is memory-safe Rust compiled to one native
  program (18–30 MB installers) with no browser engine, JavaScript or Python inside. Large
  GeoPackage, GeoParquet and FlatGeobuf files are streamed a view at a time rather than loaded
  into a browser tab's memory, and nothing depends on a web view's limits.
- **Everything is a command.** Every ribbon button, shortcut and pane action is a command with an
  id and JSON parameters, and the same ids run from [`rustgis-cli`](../cli.md), the
  [`rustgis` Python package](../python.md), notebooks, models, the Script pane, the
  [MCP server](../mcp.md) and the AI Assistant. A batch of maps can be rendered in CI with one
  command line. GeoLibre is driven through its interface, its Python and R packages and its MCP
  server; it has no general command-line tool.
- **Sandboxed plugins.** RustGIS [plugins](../plugins.md) are WebAssembly modules that can do
  only what their declared permissions allow, and the same file runs on the desktop, in
  `rustgis-cli` and in the browser. GeoLibre's TypeScript plugins can do much more inside the
  app (panels, map tools, custom layers), but its plugin API states that they "run as trusted
  code in the app window" and that its policy "is not a security boundary".
- **Data design.** RustGIS defines domains, subtypes, attribute rules, contingent values,
  relationship classes, archiving, topology rules and traverse/COGO in the project itself, the
  geodatabase design work of ArcGIS Pro. GeoLibre has attribute forms with constraints and
  honours the domains and subtypes of the ArcGIS feature services it edits, but doesn't define
  its own.
- **Reports and layouts.** RustGIS builds grouped reports with statistics, charts and
  subreports, and layouts with map frames, grids, extent indicators and map series. GeoLibre's
  print layout and atlas cover the common cases, and it has no report builder.
- **A ribbon interface.** Tabs follow the order of the work (Home, Data, Style, Edit, Analyze,
  View, Publish), with Command Search (++ctrl+k++) to find any command or tool by name. People
  coming from ArcGIS Pro often find it familiar. GeoLibre uses menus and panels, with a command
  palette and beginner, intermediate and advanced interface levels.
- **Self-contained.** Everything RustGIS does with local data runs inside the program, with no
  network, server or helper process, and offline areas sync edited features back. GeoLibre works
  offline too, but some features need a remote host (GDAL export, ONNX detection and story map
  HTML export among them), and on the desktop its raster tools and some vector tools use an
  optional Python sidecar.

## Side by side

| | **RustGIS** | **GeoLibre** |
|---|---|---|
| **Licence** | Apache-2.0 | MIT |
| **Written in** | Rust (egui, wgpu, tiny-skia) | TypeScript and Rust (Tauri, React, MapLibre GL JS, DuckDB-WASM, deck.gl) |
| **First release** | 2026 | 2026 (v1.0 in June) |
| **Platforms** | Windows, macOS, Linux, and the browser (WebAssembly) | Windows, macOS, Linux, the browser and PWA, Android, iOS, Docker |
| **Interface** | Ribbon with Command Search | Menus and panels with a command palette and interface levels |
| **Map engines** | One native renderer (2D map and globe) | MapLibre, Mapbox GL JS, CesiumJS or the ArcGIS Maps SDK |
| **Formats** | A focused set read natively; more through GDAL when installed | A wide set in the app, some desktop only (File Geodatabase, MBTiles) |
| **Databases** | DuckDB and PostgreSQL/PostGIS, through DuckDB, on the desktop | DuckDB, PostGIS, SQL Server, Iceberg; PGlite and Sedona in the browser |
| **Geoprocessing** | 1,000+ (GeoLibre and Whitebox) plus built-in vector tools | 1,000+ (GeoLibre and Whitebox) plus the GeoLibre Toolbox |
| **Editing** | Create, move, rotate, scale, split, merge, snapping; no vertex editing yet | Vertex editing and write-back to sources |
| **Data design** | Domains, subtypes, attribute rules, relationships, topology | Attribute forms; ArcGIS services' domains and subtypes |
| **Output** | Layouts, map series, reports, presentations, web map pages | Print layouts with atlas, story maps, dashboards, offline HTML |
| **Scripting** | Every action a command: CLI, Python package, notebooks, Script pane, MCP | Python and R packages, Jupyter widget, notebooks, MCP |
| **Plugins** | Sandboxed WebAssembly, small registry | TypeScript, trusted, small registry |
| **AI** | Assistant acting through undoable commands; MCP server | Assistant with undoable operations and voice; SAM, ONNX; MCP server |
| **Collaboration** | — | Share links, review comments, real-time (experimental) |
| **Languages** | 16 | 21, including right-to-left |
| **Project file** | `.rgis` (open JSON) | `.geolibre` (open, documented JSON) |

## Using them together

The two read and write the same open formats, so data moves between them without conversion:
GeoPackage, GeoParquet, FlatGeobuf, Shapefile, GeoJSON, KML/KMZ, GPX, CSV and cloud-optimised
GeoTIFF. Neither opens the other's project file yet; move the data across and restyle it.

They also share their geoprocessing. RustGIS's **Analyze › Tools** runs the GeoLibre and
Whitebox suite (natively through the `geolibre` program on the desktop, and on WebAssembly in the
browser), so a tool learned in one works the same way in the other. See
[Analysis](../user-guide/analysis.md).

A common split is to explore, share and collaborate in GeoLibre, especially from a browser,
a phone or a notebook, and to use RustGIS for repeatable desktop work: a project scripted
command by command, maps rendered by `rustgis-cli` in CI, data with domains and attribute rules,
or reports and map series printed to PDF.

## Common questions

**Why are there two?**
They are built on different foundations for different jobs. GeoLibre uses the web platform to
reach as many places and data sources as possible. RustGIS explores what a desktop GIS looks like
as one native Rust program in which every action is a command. Each project borrows ideas from
the other.

**Which one should I start with?**
If you want to open a dataset in a browser tab or on a phone right now, or work in Jupyter or R,
start with GeoLibre. If you want a desktop GIS with a ribbon, a command line that can do
everything the interface can, or reports and data design, start with RustGIS.

**Do they run the same geoprocessing tools?**
Yes: the GeoLibre and Whitebox suite of over 1,000 tools, built from
[geolibre-rust](https://github.com/opengeos/geolibre-rust). RustGIS adds its own built-in vector
tools and guided workflows; GeoLibre adds its Toolbox (routing, H3 grids, spatial statistics).

**Can a GeoLibre plugin run in RustGIS, or the other way round?**
No. GeoLibre plugins are TypeScript modules for its web interface; RustGIS plugins are sandboxed
WebAssembly modules that add commands and tools.

**Can I open a GeoLibre project in RustGIS?**
Not yet. Export the layers (GeoPackage or GeoParquet keep the most) and add them to RustGIS.

## See also

- [Full comparison](index.md): RustGIS beside GeoLibre, QGIS, ArcGIS Pro and ArcGIS Online
- [RustGIS and QGIS](qgis.md): a closer look, for QGIS users
- [GeoLibre](https://geolibre.app) and [its comparison](https://geolibre.app/comparison/) with
  QGIS, ArcGIS, CARTO, Felt and kepler.gl
- [Analysis](../user-guide/analysis.md): the GeoLibre and Whitebox tools in RustGIS
