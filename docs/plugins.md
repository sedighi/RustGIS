# Plugins

Plugins add commands and geoprocessing tools to RustGIS. A plugin is a WebAssembly module with
a `plugin.json` manifest, packaged as a `.rgplugin` file. The same file runs in the desktop app,
on the command line and in the web app.

Browse the registry at **[plugins.rustgis.app](https://plugins.rustgis.app)**. Plugin sources,
the SDK and the registry live in
[opengeos/rustgis-plugins](https://github.com/opengeos/rustgis-plugins).

## Using plugins

Open **Plugins › Plugin Manager**:

- **Browse** lists the plugins in the registry. **Install** downloads the plugin, checks it
  against the SHA-256 the registry published, and shows what the plugin may do before it loads.
- **Installed** lists your plugins: turn them on or off, update them, or uninstall them.
- **Settings** installs a `.rgplugin` file. You can also drop the file on the window. It also
  sets the registry address (empty means the RustGIS registry). On the desktop, it can load a
  plugin from its folder while you develop it.

A plugin's commands and tools appear on the **Plugins** tab, in a group named after the
plugin, or on the plugin's own tab if it has one. Its commands are also in command search, and
its tools are in the **Tools** pane under **Plugins ›**.

The desktop app keeps installed packages in its data folder (`plugins/` beside the GeoLibre
runner, or `$RUSTGIS_PLUGINS_DIR`) and loads them at startup. The web app remembers the
plugins you installed from the registry and downloads them again on your next visit. Packages
installed from files are kept too, if they are small enough for the browser's storage.

On the command line:

```sh
rustgis-cli plugins                          # installed plugins
rustgis-cli plugins browse                   # the registry
rustgis-cli plugins get vector-toolkit       # install from the registry
rustgis-cli plugins install my.rgplugin      # install a file (or a plugin folder)
rustgis-cli plugins disable vector-toolkit   # enable / disable / uninstall <id>

rustgis-cli run --sample \
  --cmd 'gp.run={"tool":"plugin:vector-toolkit/hexagon-grid","params":{"input":"Countries","size":300}}' \
  --cmd 'share.exportMap={"path":"hex.png"}'
rustgis-cli run --plugin ./dev-plugin/ --sample --cmd 'plugin:my-plugin/hello'   # load for one run
```

## Safety

Plugins run in a sandbox: an interpreter (`wasmi`) with no access to files, the network, the
clock or anything else outside the module. A plugin reaches RustGIS only through the requests
described below, and RustGIS checks each one:

- Every plugin can **read the map**: its layers, their features and the view.
- With the **`edit`** permission, it can change the project. It can run commands that edit it and
  add layers. A plugin command that changes the project is one undo step, and it is rolled back
  if the command fails.
- With the **`files`** permission, it can run commands that read or write files the user names,
  and its tools can take a file the user picks (a [`file` parameter](#file-parameters)).
- With the **`network`** permission (and `edit`), it can add web tile layers (`map.addTiles`).
  The map then loads tiles from the addresses the plugin gave, and an address can carry what the
  plugin read from the project, so this is a permission of its own. A command that would bring
  in a tile layer some other way (importing a map file) has it taken out again without
  `network`.
- No plugin may run the user's scripts, tasks or models, install or locate other software,
  manage plugins, replace the project, or step through undo history.

A [process plugin](#process-plugins) is the exception: it is a program on your computer, not
sandboxed, and the install prompt says so.

The install prompt lists these permissions. Each call has a fuel budget, so a plugin stuck in a
loop is stopped. Memory is capped at 512 MB, and requests and replies are size-limited. A plugin
that traps, panics or returns garbage produces an error message, never a crash.

The registry is curated: plugins are reviewed before they are listed, and packages are
hash-pinned. A `blocklist.json` beside the registry withdraws a plugin (every version, or one
package) from every install. RustGIS checks it when you browse the registry and at startup.

## Writing a plugin

Start from [`plugins/hello`](https://github.com/opengeos/rustgis-plugins/tree/main/plugins/hello)
in the plugins repository. It uses the `rustgis-plugin` SDK crate:

```rust
use rustgis_plugin::{Kind, Request, ToolOutput, Value, geojson, host, json};

fn handle(req: Request) -> Result<Value, String> {
    match (req.kind, req.id.as_str()) {
        (Kind::Command, "layer-report") => {
            let n = host::layers()?.as_array().map_or(0, Vec::len);
            host::notify(&format!("{n} layers"));
            Ok(json!({"layers": n}))
        }
        (Kind::Tool, "bounding-boxes") => {
            let boxes: Vec<Value> = req.features("input")?.iter().filter_map(|f| {
                let b = geojson::bbox(&geojson::positions(&f["geometry"]))?;
                Some(geojson::feature(geojson::polygon(&[[b[0], b[1]], [b[2], b[1]], [b[2], b[3]], [b[0], b[3]]]), json!({})))
            }).collect();
            Ok(ToolOutput::new().layer("Boxes", geojson::collection(boxes)).into())
        }
        (_, other) => Err(format!("unknown '{other}'")),
    }
}

rustgis_plugin::export!(handle);
```

```sh
cargo build --release --target wasm32-unknown-unknown
```

### `plugin.json`

```json
{
  "id": "hello-rustgis",
  "name": "Hello RustGIS",
  "version": "0.1.0",
  "description": "What it does, in a sentence or two.",
  "author": "You",
  "homepage": "https://github.com/you/your-plugin",
  "license": "MIT",
  "entry": "plugin.wasm",
  "abi": 1,
  "minRustgisVersion": "0.1.0",
  "permissions": ["edit"],
  "commands": [
    { "id": "layer-report", "label": "Layer\nReport", "help": "Shown on hover.", "icon": "stats", "params": "{}" }
  ],
  "tools": [
    {
      "id": "bounding-boxes", "name": "Bounding Boxes", "summary": "One line.", "toolbox": "Hello",
      "params": [
        { "name": "input", "label": "Input features", "kind": "vectorLayer", "required": true },
        { "name": "whole", "label": "One box for the whole layer", "kind": "bool", "default": false }
      ]
    }
  ],
  "i18n": { "de": { "Layer\nReport": "Ebenen-\nbericht" } }
}
```

- `id`: lower-case letters, digits and hyphens. Commands and tools get ids
  `plugin:<id>/<command>`, which the ribbon, command search, scripts and `rustgis-cli` use.
- `version` and `minRustgisVersion` look like `1.2.3`. RustGIS refuses a plugin that needs a
  newer RustGIS.
- `permissions`: `edit`, `files` and `network` (see [Safety](#safety)). Ask only for what you use.
- `commands[].icon`: the name of one of RustGIS's own icons (`stats`, `zoom_full`, `bookmark`,
  `buffer`…), or a puzzle piece when it isn't one.
- Tool parameter `kind`s: `layer`, `vectorLayer`, `polygonLayer`, `pointLayer`, `field`,
  `numericField`, `number`, `text`, `choice` (with `choices`), `bool` and `file` (see
  [File parameters](#file-parameters)).
- `i18n` translates the plugin's own text into the interface language, by language code (`de`,
  `zh-CN`…). See [Translations](#translations).
- `skills` (optional) lists skills for the AI Assistant that the package carries. See
  [Skills](#skills).

A package is a zip archive named `<id>-<version>.rgplugin`. It holds `plugin.json` and the module
at its root, or inside one top-level folder. Packages are at most 64 MB, and modules at most 48 MB.

### File parameters

A tool can read a file the user picks, such as a NetCDF model run, with a `file` parameter.
The plugin needs the `files` permission:

```json
{ "name": "model", "label": "Model output", "kind": "file", "required": true, "extensions": ["nc", "nc4"] }
```

- The Tools pane shows a path box with a **Browse** button. `extensions` (without the dot; up
  to 16) filters the file picker; leave it out for any file.
- On the desktop and the command line the value is a path, and RustGIS reads the file. In the
  web app, which has no paths, the user picks the file and its contents go with the run. A
  script can do the same with `{"name": "run.nc", "data": base64}`.
- The tool gets the file in the request's `files`, by parameter name, as
  `{"name": "run.nc", "data": base64}`. The parameter itself holds just the file name.
- Files are at most 64 MB. The Geoprocessing history keeps the file's name, not its contents.
- RustGIS versions before file parameters refuse a plugin that has one, so set
  `minRustgisVersion` to the first version with them.

### Translations

A plugin speaks the interface language through its `i18n` table: for each language code, its
English strings and their translations, written exactly as they appear in `plugin.json`
(`\n` included):

```json
"i18n": {
  "de": { "Bounding Boxes": "Begrenzungsrahmen", "Input features": "Eingabe-Features" },
  "zh-CN": { "Bounding Boxes": "边界框", "Input features": "输入要素" }
}
```

- It covers the plugin's name and description, command labels and help, tool names, summaries
  and toolboxes, parameter labels and `choice` values, and the tab, group and menu labels. A
  string with no translation shows in English.
- RustGIS's interface languages are `de`, `es`, `fr`, `it`, `nl`, `pt`, `ru`, `tr`, `az`, `id`,
  `vi`, `zh`, `ja` and `ko`. A code with a region matches its language: `zh-CN` serves `zh`.
- The registry copies the name and description translations into `registry.json`, so the
  Plugin Manager lists plugins in the interface language before they are installed.
- Messages a plugin builds while it runs can follow the interface language too: every request
  carries its code as `lang`. In the SDK that is `req.lang`, and `req.tr` looks a message up in
  `i18n`.
- Up to 64 languages and 20,000 translated strings in all.

### A tab of its own

By default a plugin's commands and tools get a group on the **Plugins** tab. A plugin can have a
ribbon tab of its own instead. The tab sits at the end of the tab strip, and **Help › Ribbon**
can hide it like any other. Declare it in `plugin.json`:

```json
"tab": {
  "label": "Explorer",
  "groups": [
    {
      "label": "Go To",
      "items": [
        { "menu": "Continent", "icon": "zoom_world", "help": "Go to a continent.", "items": ["go-africa", "go-europe"] },
        "random-place"
      ]
    },
    { "label": "Inspect", "items": ["layer-report", "tool:field-summary"] }
  ]
}
```

- `items` lists the buttons. An item is a command id, `tool:<id>` (which opens that tool in the
  Tools pane), or a dropdown `menu` whose `items` are command ids or `tool:<id>`.
- A tab has 1 to 8 groups, a group has 1 to 16 items, and a menu has 1 to 32 entries. Every
  entry must name one of the plugin's own commands or tools.
- The label can't be the name of one of RustGIS's tabs (Map, Insert, Analysis, Plugins…). The
  `i18n` translations apply to the tab, group and menu labels too.

[Map Explorer](https://github.com/opengeos/rustgis-plugins/tree/main/plugins/map-explorer) is
an example.

### What a plugin can ask for

Inside a request, `host::call(op, params)` sends one of these to RustGIS:

| Request | Answer |
|---|---|
| `host.info` | RustGIS's version, the ABI, the plugin's id and permissions |
| `host.layers` | `[{id, name, raster, visible, geometry, features, selected, active, fields: [{name, type}]}]` |
| `host.features` `{layer, selected?, limit?}` | the layer's features as GeoJSON; each feature has its `row` |
| `host.view` | `{center: [lon, lat], scale, projection}` |
| `host.notify` `{text, error?}` | shows a message in the status bar |
| `host.addLayer` `{name, geojson}` | adds a layer (`edit`) |
| `map.addTiles` `{url, kind?, name?, attribution?, maxZoom?, overlay?, replace?}` | adds a web tile layer (`edit` and `network`) |
| any command id (`layer.zoomTo`, `select.byAttributes`, `layer.symbology`…) | what the command returns, within the plugin's permissions |

In the web app a command can't wait for a download, so where `layer.addUrl` has to download a
file whole it answers `{"pending": true, "url": …}` there: RustGIS downloads the data off the
frame and adds the layer when it lands, as **Home › Add Data › From URL** does. A PMTiles
archive, a cloud-optimised GeoTIFF or a FlatGeobuf or GeoParquet read in place is added at once,
as elsewhere.

`rustgis-cli commands` lists every command id and its parameters.

Tools work differently. A tool receives its layer parameters' features as GeoJSON (only the
selected features when a layer has a selection), and its `file` parameters' files. It returns new layers, standalone tables or a
selection, plus messages for the Geoprocessing history. RustGIS adds the results as it does for
built-in tools, with undo. Tools can log, but they make no host requests.

In the desktop app a plugin tool runs in the background, so the map and the rest of the window
stay usable: the Tools pane shows the line the tool last logged, or a process plugin's last
`progress` message, and a **Cancel** button. Cancel stops a process plugin's program; a WebAssembly tool runs on to its
end and its result is dropped. One plugin tool runs at a time. `gp.run`, scripts and
`rustgis-cli` still wait for the result.

### The ABI (version 1)

The SDK implements this; it is described here for other languages.

The module is `wasm32` with no imports except these three, in module `rustgis`:

- `call(ptr: i32, len: i32) -> i32` runs the host request at `ptr`
  (`{"op": "...", "params": {...}}`). It returns the length of the reply, which is
  `{"ok": value}` or `{"error": "text"}`, or `-1` when there is none.
- `result(ptr: i32)` copies that reply to `ptr`. Allocate the length `call` returned first.
- `log(ptr: i32, len: i32)` logs a UTF-8 line.

The module exports:

- `memory`.
- `rustgis_alloc(len: i32) -> i32`, which returns room for `len` bytes. RustGIS writes the
  request there.
- `rustgis_call(ptr: i32, len: i32) -> i64`, which handles the request and returns
  `reply_ptr << 32 | reply_len`. The reply is `{"ok": value}` or `{"error": "text"}`.

Requests are `{"abi": 1, "kind": "command" | "tool", "id": "...", "params": {...}, "layers": {...}, "lang": "de"}`.
`lang` is the interface language's code (`en` for English).
Tools also get `layers`, which maps each layer's name to its FeatureCollection, and, when
they have `file` parameters, `files`, which maps each one's name to `{"name", "data"}` with the
file's contents in base64. A tool's `ok`
value is `{"layers": [{"name", "geojson"}], "tables": [...], "selection": {"layer", "rows"},
"messages": [...]}`.

Each call gets a fresh instance, so a module keeps no state between calls and need not free
memory. The fuel budget is about 2×10⁹ instructions for a command and 4×10¹⁰ for a tool.

## Process plugins

A process plugin is a program on your computer instead of a WebAssembly module: a Python script,
or an executable in any language. It can use the file system, threads and any library, which
lets existing analysis code (numpy, xarray, GDAL…) become a plugin without a rewrite. It is **not
sandboxed**: it can do anything you can. The install prompt says so, and the Plugin Manager
marks it *Not sandboxed*. Process plugins run on the desktop and the command line, not in the
web app.

```json
{
  "id": "hello-python",
  "name": "Hello Python",
  "version": "0.1.0",
  "runtime": "process",
  "run": { "windows": ["python", "main.py"], "default": ["python3", "main.py"] },
  "permissions": [],
  "commands": [{ "id": "layer-count", "label": "Count\nLayers" }],
  "tools": [{
    "id": "feature-count", "name": "Count Features", "summary": "Counts a layer's features.",
    "params": [{ "name": "input", "label": "Input layer", "kind": "vectorLayer", "required": true }]
  }]
}
```

- `"runtime": "process"` and `run`: the program and its arguments, started in the plugin's
  folder. A bare name (`python3`) is looked up on `PATH`; a relative path (`bin/tool`) is in the
  plugin's folder. `run` can name a program per platform (`windows`, `macos`, `linux`, and
  `default` for the rest). There is no `entry`.
- Commands, tools, tabs, translations and permissions work as for WebAssembly plugins.
  Permissions still decide which host requests RustGIS answers, but they don't limit what the
  program itself can do.
- A `.rgplugin` holds the plugin's files (the manifest, scripts, executables). Installing it
  unpacks them to `unpacked/<id>/` in the plugins folder, keeping executables executable. A
  plugin folder installed for development (Plugin Manager › Settings, or
  `rustgis-cli plugins install ./folder`) runs where it is.

### The protocol

Each command or tool call starts the program once. It speaks the [ABI](#the-abi-version-1)'s
requests and replies over its standard input and output, one JSON object per line:

1. RustGIS writes the request: `{"abi", "kind", "id", "params", "layers", "files", "lang"}`, as
   a WebAssembly module gets it.
2. The program writes lines:
   - `{"call": {"op": "...", "params": {...}}}`: a [host request](#what-a-plugin-can-ask-for).
     RustGIS answers with one line, `{"ok": value}` or `{"error": "text"}`.
   - `{"log": "text"}`: a line for the log (and a tool's run messages).
   - `{"progress": "text"}`: what a tool is doing now, shown while it runs and not kept in its
     messages: a line per step of a long run, say.
   - Last, `{"ok": value}` or `{"error": "text"}`: the result.
3. RustGIS then closes the program's input; a program still running a few seconds later is
   stopped.

Output lines that aren't protocol messages are logged and skipped, since libraries print (even
JSON), and so is the program's error output. A command is stopped after 2 minutes; a tool runs
until it finishes or is cancelled (the desktop app runs it in the background: see
[What a plugin can ask for](#what-a-plugin-can-ask-for)).

The plugin above, in Python with no SDK:

```python
import json
import sys


def call(op, params):
    """Sends a request to RustGIS and returns its answer."""
    print(json.dumps({"call": {"op": op, "params": params}}), flush=True)
    reply = json.loads(sys.stdin.readline())
    if "error" in reply:
        raise RuntimeError(reply["error"])
    return reply["ok"]


req = json.loads(sys.stdin.readline())
try:
    if req["id"] == "layer-count":
        n = len(call("host.layers", {}))
        call("host.notify", {"text": f"The map has {n} layers."})
        result = {"ok": {"layers": n}}
    elif req["id"] == "feature-count":
        layer = req["params"]["input"]
        n = len(req["layers"][layer]["features"])
        result = {"ok": {"layers": [], "messages": [f"{layer} has {n} features."]}}
    else:
        result = {"error": f"unknown command '{req['id']}'"}
except Exception as e:
    result = {"error": str(e)}
print(json.dumps(result), flush=True)
```

### Python environments

A Python plugin can have RustGIS make its environment, so its users don't install Python or
any package themselves:

```json
"runtime": "process",
"python": { "version": "3.12", "requirements": "requirements.txt" },
"run": ["python", "main.py"]
```

- `python` in `run` is then that environment's interpreter, on every platform.
- RustGIS makes the environment when the plugin is installed: in the background in the desktop
  app (the Plugin Manager shows how it's going, and a **Prepare** button makes it again), and
  before the command returns in `rustgis-cli plugins install` and `get`.
  `rustgis-cli plugins prepare <id>` and the `plugin.prepare` command make it on demand.
- It uses [uv](https://github.com/astral-sh/uv), which RustGIS downloads once (a pinned version,
  checked against its pinned SHA-256; `RUSTGIS_UV` names one to use instead, which is then
  trusted as it is, unchecked). uv installs a standalone Python of the version asked for and
  the packages in the requirements file, from PyPI. Pythons and uv's cache are shared in
  `tools/` in the plugins folder; each plugin's environment is in `envs/<id>/`. One
  environment is made at a time per plugin, even across the app and `rustgis-cli`.
- The environment is made again when the Python version or the requirements change (a plugin
  update), and removed with the plugin. Until it's ready, the plugin's calls fail with a
  message saying how to prepare it.
- The first install downloads Python and the packages: tens to hundreds of MB, depending on
  them. The install prompt says so.

## Skills

A plugin can carry [skills](user-guide/assistant.md#skills) for the AI Assistant: instructions
for a kind of task, which the assistant reads when a request needs them. Put each in
`skills/<name>/SKILL.md` in the package and list the names in `plugin.json`:

```json
"skills": ["flood-map"]
```

A `SKILL.md` starts with front matter giving the same `name` and a `description` that says when
to use it, then the instructions in Markdown (the
[Agent Skills](https://agentskills.io) format). The assistant knows it as `<plugin id>:<name>`
(`hello-rustgis:flood-map`) while the plugin is enabled. A `SKILL.md` is at most 64 KB, and a
plugin has at most 32 skills.

A package of skills alone, with no code, says `"runtime": "skills"`. It has no module, commands,
tools, tab or permissions. Nothing in it runs, and it installs and works like any other plugin,
in the desktop app, on the command line and in the web app:

```json
{
  "id": "flood-skills",
  "name": "Flood mapping skills",
  "version": "1.0.0",
  "runtime": "skills",
  "skills": ["flood-map", "gauge-report"]
}
```

## Publishing a plugin

Open a pull request on
[opengeos/rustgis-plugins](https://github.com/opengeos/rustgis-plugins) that adds
`registry/<id>.json`:

- For a plugin whose source is in that repository: `{"id": "...", "path": "plugins/<dir>", "categories": [...]}`.
- For one you publish elsewhere: `{"id": "...", "source": {"url": "https://…/<id>-<version>.rgplugin", "sha256": "…"}, "categories": [...]}`.

CI builds or downloads the package and checks its manifest and module. A maintainer then
reviews it. Once merged, it is published at `plugins.rustgis.app` with its hash.
