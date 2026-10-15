# Attribution

Every asset file in this repository, with its source and licence.

| File | Author / source | Licence |
|---|---|---|
| Sample data (`countries`, `countries_chn`, `cities`, `rivers`, `lakes` GeoJSON), embedded at build time from [rustgis-assets](https://github.com/opengeos/rustgis-assets/tree/main/data) | Natural Earth 1:110m, attributes trimmed; `countries_chn` is Natural Earth's China point of view (1:10m, simplified) | Public domain |
| `apps/rustgis-web/fonts/Vazirmatn-Regular.ttf` | Saber Rastikerdar ([vazirmatn](https://github.com/rastikerdar/vazirmatn)) | SIL Open Font License 1.1 |
| QGIS screenshot (`qgis-world-atlas.webp` in [rustgis-assets](https://github.com/opengeos/rustgis-assets/tree/main/images)), on the [RustGIS and QGIS](docs/comparison/qgis.md) page | Taken by the RustGIS project: [QGIS](https://qgis.org) 4.2 showing the Natural Earth sample data above | QGIS is GPL-2.0-or-later; the data is public domain |

Raster coordinate systems are looked up by EPSG code in the PROJ definitions of the
[crs-definitions](https://crates.io/crates/crs-definitions) crate (CC0-1.0), compiled into the
app.

The GeoLibre tool suite ([geolibre-rust](https://github.com/opengeos/geolibre-rust), MIT, built
on [whitebox-wasm](https://github.com/opengeos/whitebox-wasm), MIT) is not part of RustGIS: the
desktop app and command line run its separately downloaded `geolibre` runner, and the web app
loads its WebAssembly runner (the `geolibre-wasm` npm package, from jsDelivr) when asked. Its
tool names, descriptions and categories come from the runner's manifests.

Icons are original vector drawings in code (`crates/ui-egui/src/icons.rs`). Fonts are not
bundled on desktop: the interface uses an installed system sans-serif when one is found, otherwise the
Ubuntu Light face that ships inside `epaint_default_fonts` (Ubuntu Font Licence 1.0). The web app copies
`Vazirmatn-Regular.ttf` (SIL Open Font License 1.1) for Persian script.
