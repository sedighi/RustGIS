# RustGIS

**A fast, free and open-source GIS in pure Rust for mapping, analyzing and sharing geospatial
data.**

RustGIS runs natively on Windows, macOS and Linux, and in your web browser. Your data stays on
your machine unless you send it to an AI model you choose, and every action is a command you can
script from the command line, a notebook or the AI Assistant.

[Try it in your browser](https://web.rustgis.app){ .md-button .md-button--primary }
[Download](installation.md){ .md-button }
[Get started](getting-started.md){ .md-button }
[Compare](comparison/index.md){ .md-button }

[![Get it from Microsoft](https://get.microsoft.com/images/en-us%20dark.svg){ width="160" }](https://apps.microsoft.com/detail/9ppjw8h561lq?mode=direct)

[![RustGIS with the Home tab of the ribbon open over a world map in the Equal Earth projection, countries shaded by population](https://assets.rustgis.app/images/hero.webp)](https://assets.rustgis.app/images/hero.webp){ target="_blank" rel="noopener" title="Open the full-size image" }

## Highlights

<div class="grid cards" markdown>

-   **Your data, any map**

    GeoJSON, CSV, Shapefile, GeoPackage, GeoParquet, FlatGeobuf, KML, GPX, GeoTIFF and
    cloud-optimised GeoTIFF, web tiles, vector tiles, PMTiles, MBTiles and MapLibre styles, drawn on a 3D
    globe or in any projection, on the desktop and in the browser; STAC catalogs, Overture
    Maps, OpenStreetMap, open-data portals and S3 buckets; SQL, DuckDB and live PostGIS
    databases on the desktop.
    [Data](user-guide/data.md) · [Formats](user-guide/data-formats.md) · [Maps](user-guide/maps.md)

-   **Cartography**

    Unique values, graduated colours and proportional symbols, collision-free labels, effects,
    extrusion, page layouts, map series, reports and presentations.
    [Cartography](user-guide/cartography.md) · [Layouts](user-guide/layouts.md)

-   **Analysis**

    SQL selection, overlay and proximity tools, guided workflows, ModelBuilder, imagery tools and
    over 1,000 GeoLibre and Whitebox tools for terrain, hydrology and LiDAR.
    [Analysis](user-guide/analysis.md)

-   **Editing**

    Create and modify features with snapping and templates, keep attributes valid with domains,
    subtypes and rules, and undo anything.
    [Editing](user-guide/editing.md)

-   **AI Assistant**

    Ask about your data or tell the map what to do. The assistant runs RustGIS commands with the
    model you choose, and every change can be undone.
    [AI Assistant](user-guide/assistant.md)

-   **Scriptable and extensible**

    `rustgis-cli` and the `rustgis` Python package run any command by id, render maps and chain
    tools; the MCP server lets AI agents make maps; sandboxed WebAssembly plugins add commands,
    tools and ribbon tabs.
    [Command Line](cli.md) · [Python](python.md) · [MCP Server](mcp.md) · [Plugins](plugins.md)

</div>

## A tour

<div class="grid" markdown>

[![The Style tab and the Symbology pane, with the larger African countries selected](https://assets.rustgis.app/images/symbology-table.webp)](https://assets.rustgis.app/images/symbology-table.webp){ target="_blank" rel="noopener" title="Open the full-size image" }

[![The AI Assistant selecting the five most populous African countries](https://assets.rustgis.app/images/assistant.webp)](https://assets.rustgis.app/images/assistant.webp){ target="_blank" rel="noopener" title="Open the full-size image" }

[![The Analyze tab's tool gallery and dissolved buffers around African cities](https://assets.rustgis.app/images/buffer-history.webp)](https://assets.rustgis.app/images/buffer-history.webp){ target="_blank" rel="noopener" title="Open the full-size image" }

[![A shaded-relief elevation model with the Style tab](https://assets.rustgis.app/images/raster.webp)](https://assets.rustgis.app/images/raster.webp){ target="_blank" rel="noopener" title="Open the full-size image" }

[![The World Atlas on the 3D globe](https://assets.rustgis.app/images/globe.webp)](https://assets.rustgis.app/images/globe.webp){ target="_blank" rel="noopener" title="Open the full-size image" }

[![An OpenFreeMap vector style over Paris with the Basemap gallery](https://assets.rustgis.app/images/tiles.webp)](https://assets.rustgis.app/images/tiles.webp){ target="_blank" rel="noopener" title="Open the full-size image" }

[![A page layout with a map frame, legend, north arrow and scale bar](https://assets.rustgis.app/images/layout.webp)](https://assets.rustgis.app/images/layout.webp){ target="_blank" rel="noopener" title="Open the full-size image" }

[![The Plugin Manager browsing the plugin registry](https://assets.rustgis.app/images/plugins.webp)](https://assets.rustgis.app/images/plugins.webp){ target="_blank" rel="noopener" title="Open the full-size image" }

[![Dark mode over Europe on the Midnight basemap](https://assets.rustgis.app/images/dark.webp)](https://assets.rustgis.app/images/dark.webp){ target="_blank" rel="noopener" title="Open the full-size image" }

[![The Project backstage](https://assets.rustgis.app/images/backstage.webp)](https://assets.rustgis.app/images/backstage.webp){ target="_blank" rel="noopener" title="Open the full-size image" }

</div>
