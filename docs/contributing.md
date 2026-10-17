# Contributing

Contributions are welcome. See [AGENTS.md](https://github.com/opengeos/RustGIS/blob/main/AGENTS.md)
for the conventions.

1. Fork the repository and create a branch.
2. Make your change with tests. New behaviour is a command in `crates/engine/src/cmd/`.
3. Run `cargo xtask ci` and `pre-commit run --all-files`.
4. Open a pull request. CI runs on Linux, macOS and Windows, and the docs build a preview.

## Test data

Generate small datasets during tests instead of committing fixture files. The shared generator
in `tests/support/ogc.rs` builds OGC landing pages, collection lists and GeoJSON in memory;
`tests/support/ogc_server.rs` serves them locally for engine and UI tests. The sorting smoke
test similarly generates its KML, KMZ, CSV and GeoJSON inputs. These run automatically with
`cargo test --workspace --locked` and `cargo xtask ci`, without a separate setup step or a
public service. Extend these generators when adding cases.

## Documentation

The site is built with [Zensical](https://zensical.org):

```sh
pip install -r requirements_docs.txt
zensical serve        # http://localhost:8000
```

Images live in [rustgis-assets](https://github.com/opengeos/rustgis-assets) and are linked by
URL; don't commit images to this repository.

The screenshots are rendered by the app itself. `crates/ui-egui/examples/shots/render.sh`
runs the `ui_shot` scripts beside it and writes WebP files to `target/shots/`; copy them to
`images/` in rustgis-assets. The `assistant` shot asks a real model, so set an AI Assistant
provider and key first (see [AI Assistant](user-guide/assistant.md#choosing-a-model)).

## Releases

1. Bump `version` in the workspace `Cargo.toml` and merge it.
2. Run the **Release dry run** workflow, which checks the version and builds every package
   without publishing anything:
   `gh workflow run release-dry-run.yml -R opengeos/RustGIS --ref main -f version=<version>`
3. When it passes, publish a GitHub release tagged `v<version>` (the tag must match `Cargo.toml`).

The **Release** workflow then builds and attaches the Windows installers, zip and MSIX, the
macOS DMGs, the Linux tarballs, `.deb` and `.rpm` packages and AppImages, and `SHA256SUMS`. To try the packaging locally:

```sh
cargo build --release --locked -p rustgis -p rustgis-cli
packaging/package.sh 0.1.0      # writes dist/
```
