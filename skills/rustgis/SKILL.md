---
name: rustgis
description: |
  Use RustGIS from an agent: list layers, read features, view the map, run commands and geoprocessing, and save. Trigger for the already-open desktop app (MCP Relay, then rustgis-cli mcp --attach), a headless project, or rustgis-cli MCP in Claude, Codex, Cursor, or VS Code. Installing RustGIS is a trigger, not the task.
---

# RustGIS

[RustGIS](https://rustgis.app) is a desktop and browser GIS with an accompanying
`rustgis-cli`. The CLI exposes MCP tools over standard input/output. Choose
which project the user wants to operate on **before** starting the server.

## Choose the session

| User's goal | Entry point |
| --- | --- |
| Install RustGIS or make its CLI available to the agent | Read [install-from-agent](references/install-from-agent.md); detect the user's platform, install the app and CLI, verify the CLI, then register MCP. |
| Manage the map already open in the desktop app | Start **MCP Relay** in the app's Command Search (Ctrl+K), confirm `127.0.0.1:8767`, then register `rustgis-cli mcp --attach`. |
| Work in a separate headless project | Register `rustgis-cli mcp --sample`, `--project file.rgis`, or repeatable `--data file.geojson`. This is the agent's own project, not the desktop app. |
| Use the browser app | Open <https://web.rustgis.app>. The desktop loopback relay is not a browser-management interface. |

**For the open app, `--attach` is mandatory.** Do not silently fall back to a
private project if the relay is unavailable. Do not combine `--attach` with
`--sample`, `--project` or `--data`; the CLI rejects that combination.

## Connect to the open desktop app

1. Ensure the app and `rustgis-cli` are installed on the user's machine. Use the
   install reference to resolve PATH and MCP client configuration.
2. In the open RustGIS desktop app, ask the user to tick **Let your Agents
   Control RustGIS** in the AI Assistant's settings (it also lets attached
   agents use files and the network), or open Command Search (Ctrl+K) and run
   **MCP Relay** (`ui.mcpRelay`). Confirm the status bar says
   `MCP relay listening on 127.0.0.1:8767`. The listener is off until started
   and remains active until the app closes.
3. Register the attached server in the agent, for example:

   ```sh
   claude mcp add rustgis-app -- rustgis-cli mcp --attach
   # Or:
   codex mcp add rustgis-app -- rustgis-cli mcp --attach
   ```

   The MCP client launches the stdio process; do not start a separate HTTP
   server. Add `--yes` only when the user's task authorizes file/network access.
4. Call `list_layers` and `view_map` to establish that this is the user's open
   map before changing it. If connection fails, check that the app is still
   open, MCP Relay is active, and the selected port matches. Do not claim
   remote management from an unattached session.

The agent process must run on the same machine as the desktop app: the relay
binds only to `127.0.0.1`. Remote management here means an external agent
controlling that open app, not an exposed internet service. For a nondefault
port, start the app command with `ui.mcpRelay {"port": 9000}` and use
`rustgis-cli mcp --attach 9000`.

## Operate through RustGIS's tools

1. **Inspect:** `list_layers` supplies layer names, fields, feature counts,
   selections and the current view. Use `read_features` for attribute rows and
   `view_map` for a PNG of the current map.
2. **Discover:** use `search_commands` before choosing a command id; use
   `search_tools` and `describe_tool` before geoprocessing. Use `search_docs`
   and `read_doc` for command details and workflows. Do not guess parameter
   names or borrow tools from another GIS.
3. **Execute:** `run_command` runs a discovered command with JSON parameters;
   `run_tool` runs a discovered geoprocessing tool and adds its output to the
   map. Inspect returned `layersAdded` and `notices`, including ignored
   parameters. Each command run is one undo step.
4. **Verify:** inspect the resulting layers/attributes and call `view_map`
   again. For persistence, discover the save/export command and use the
   user's destination. An in-memory edit is not evidence of a saved file.

## Access and project boundaries

- File and network commands **still require `--yes` when attached**, just as
  in a private server, unless the user ticked **Let your Agents Control
  RustGIS**, which allows them for attached agents. It also permits plugin commands and opening project
  text, which can bring in web layers. Do not enable it to conceal a refusal.
- Plugin management, scripts, tasks, models and the GeoLibre runner are not
  available through this MCP surface; `--yes` does not unlock them.
- In attached mode, opening, replacing, undoing and saving act on the user's
  open project. Inspect first and preserve their work; do not overwrite it
  merely to create an example. In private mode, the project belongs to the
  agent and stays in memory until saved.
- `view_map` can fetch tiles for web tile layers already in the project.
  Do not describe the default access policy as a complete offline guarantee.

## Installation and client setup reference

Read [references/install-from-agent.md](references/install-from-agent.md) for
agent-runnable platform detection, package-manager and checked release installs,
PATH confirmation, Claude/Codex/`mcp.json`/VS Code registration, and the attached
relay procedure. Canonical upstream documentation:
<https://rustgis.app/installation/> and <https://rustgis.app/mcp/>.
