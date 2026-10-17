# Installation

## Install with an agent

Point your agent at the [RustGIS skill](https://github.com/opengeos/RustGIS/blob/main/skills/rustgis/SKILL.md)
and ask it to install RustGIS. The skill picks the platform, installs the app and `rustgis-cli`,
checks the release checksum, and registers the MCP server. The same skill is how an agent manages
the desktop app that is already open.

## Microsoft Store

On Windows 10 and 11, the easiest way to install RustGIS is from the Microsoft Store, which
signs the app and keeps it up to date:

[![Get it from Microsoft](https://get.microsoft.com/images/en-us%20dark.svg){ width="200" }](https://apps.microsoft.com/detail/9ppjw8h561lq?mode=direct)

## Download

Each [GitHub release](https://github.com/opengeos/RustGIS/releases/latest) carries ready-to-run
builds of the desktop app and `rustgis-cli`. The same files, grouped by platform, are mirrored at
**[downloads.rustgis.app](https://downloads.rustgis.app)**:

| Platform | File |
| --- | --- |
| Windows x64 (installer, no admin rights) | `RustGIS-<version>-windows-x86_64-setup.exe` |
| Windows x64 (installer for all users) | `RustGIS-<version>-windows-x86_64.msi` |
| Windows x64 (no install) | `RustGIS-<version>-windows-x86_64-portable.zip` (unzip and run `rustgis.exe`) |
| Windows x64 (Microsoft Store package) | `RustGIS-<version>-windows-x86_64.msix` |
| macOS Apple Silicon | `RustGIS-<version>-macos-aarch64.dmg` |
| macOS Intel | `RustGIS-<version>-macos-x86_64.dmg` |
| Linux x64 / arm64 (Debian, Ubuntu) | `rustgis_<version>_amd64.deb` / `rustgis_<version>_arm64.deb` |
| Linux x64 / arm64 (Fedora, RHEL, openSUSE) | `rustgis-<version>-1.x86_64.rpm` / `rustgis-<version>-1.aarch64.rpm` |
| Linux x64 / arm64 (any distribution, no install) | `RustGIS-<version>-linux-x86_64.AppImage` / `RustGIS-<version>-linux-aarch64.AppImage` |
| Linux x64 / arm64 (other) | `RustGIS-<version>-linux-x86_64.tar.gz` / `RustGIS-<version>-linux-aarch64.tar.gz` |

On macOS the command line tool is a separate `rustgis-cli-<version>-macos-<arch>.tar.gz`;
the other packages include it (the Windows installers and the MSIX add `rustgis-cli` to every
new terminal). `SHA256SUMS` lists the checksum of every file.

On Windows, `setup.exe` installs for your account in `%LOCALAPPDATA%\Programs\RustGIS` and needs
no admin rights; the `.msi` installs for every user in `Program Files` and suits managed
deployment (`msiexec /i RustGIS-<version>-windows-x86_64.msi /qn`). Both add a Start menu
entry, open `.rgis` projects and uninstall from **Settings › Apps**.

The `.msix` is the package submitted to the Microsoft Store, which signs it. It is unsigned
on the release page, so Windows only installs it from the
[Store](https://apps.microsoft.com/detail/9ppjw8h561lq); outside the Store, download an installer
or the portable zip instead.

The macOS DMG and command line tool are signed with a Developer ID and notarized by Apple, so
they open without a warning. From the release after 0.1.0, the notarization ticket is also
stapled to RustGIS.app, so it opens even when the Mac cannot reach Apple's servers. If macOS
says Apple could not verify RustGIS 0.1.0, connect to the internet and open it again, or run
`xattr -dr com.apple.quarantine /Applications/RustGIS.app`. The Windows builds are not yet
signed: choose **More info › Run anyway** if SmartScreen warns about an unknown publisher.

## Package managers

RustGIS is also published to these package managers, which keep it up to date:

| Platform | Command |
| --- | --- |
| Windows ([winget](https://learn.microsoft.com/windows/package-manager/)) | `winget install OpenGeos.RustGIS` |
| macOS ([Homebrew](https://brew.sh)) | `brew install --cask opengeos/rustgis/rustgis` |
| macOS and Linux, command line tool only (Homebrew) | `brew install opengeos/rustgis/rustgis-cli` |
| Arch Linux ([AUR](https://aur.archlinux.org/packages/rustgis-bin)) | `yay -S rustgis-bin` (or any AUR helper) |
| Fedora ([COPR](https://copr.fedorainfracloud.org/coprs/giswqs/rustgis/)) | `sudo dnf copr enable giswqs/rustgis && sudo dnf install rustgis` |

winget installs for your account with the setup.exe; add `--scope machine` for the `.msi`. On
Homebrew 5.1 and later, run `brew trust opengeos/rustgis` once before installing from the tap.
Each package is the same build as the release page, and a new release reaches the package
managers shortly after it is published (winget after Microsoft's review).

## From source

RustGIS builds from source with a recent stable Rust toolchain (1.90 or later).

```sh
git clone https://github.com/opengeos/RustGIS.git
cd RustGIS
cargo run --release -p rustgis            # the desktop app
cargo build --release -p rustgis-cli      # the command line tool
```

The build downloads the Natural Earth sample data (a few hundred KB) from
[rustgis-assets](https://github.com/opengeos/rustgis-assets). To build offline, download the
four files from `https://assets.rustgis.app/data/` once and point the build at them:

```sh
RUSTGIS_DATA_DIR=/path/to/data cargo build --release
```

## Platforms

| Platform | Status |
| --- | --- |
| Linux (X11 and Wayland) | Supported |
| macOS | Supported |
| Windows | Supported |
| Web browser | Supported: [web.rustgis.app](https://web.rustgis.app) (WebGPU or WebGL2), installable as an app on iOS, Android and desktops ([how](#install-the-web-app)) |

## Web app

RustGIS also runs in the browser, compiled to WebAssembly: open
**[web.rustgis.app](https://web.rustgis.app)**. It is the same app with three differences:

- **Open** and **Add Data** use the browser's file picker, and you can drop files on the page.
  GeoJSON, CSV, GeoTIFF, ASCII grids and `.rgis` projects work; Shapefiles need the desktop
  app.
- **Save** and **Export** download the file. **Save As**, and the first **Save** of a project,
  ask for the file name first; the browser decides which folder downloads go to (Firefox, for
  one, saves to its Downloads folder unless told to ask). A project opened from a file is named
  after it, and **Save** downloads it under that name again.
- The interface uses the built-in font, because a web page can't read installed fonts.

The web app opens the way **Settings › Start-up** says (remembered in the browser). The URL can
override that for one visit: add `?blank` to start with an empty project,
`?projection=EPSG:3010` (a supported projected EPSG code, a preset id such as `robinson`, or `globe`) to
start in another projection, `?extent=10.5,55,24.5,69.5` (west, south, east, north in degrees) to
open on an area, `?lang=de` (or any language code) to pick the interface language, or `?webgl` to
force WebGL2. Combine them with `&`, so a self-hosted build can link to
`?blank&projection=EPSG:3010&extent=10.5,55,24.5,69.5`.

Links can carry data too: `?data=<address>` adds a file by its web address (any format Add Data
reads, downloaded in the background; repeat it for up to 16, and the map zooms to the first that
lands with something to show), `?style=<address>` adds a MapLibre style, `?tiles=<template>`
a tile layer, and `?zarr=<address>` opens Add Zarr Variable on a Zarr store. Percent-encode an address holding `&`, `#` or spaces (`encodeURIComponent`); a `+`
stays a `+`. The server must allow the browser to fetch the file (CORS); a file that can't be
added is said in a notice. For example,
`?blank&data=https://earthquake.usgs.gov/earthquakes/feed/v1.0/summary/2.5_week.geojson` opens
the past week's earthquakes.

`?project=<address>` opens a whole project by its web address: a `.rgis` file (or a
`.rgispkg` package), or any address that answers with a project's JSON, such as an API that
builds the project on the fly. Its maps, layers, symbology and view open as saved, so one link
shares a styled map. The project replaces the start-up one, so `?data=`, `?stac=`, `?style=`,
`?tiles=` and `?zarr=` are left out beside it (a notice says so). The data the project points to
is read as it would be from a saved file, and its servers must allow the browser to fetch it
(CORS), as must the project's.

`?stac=<address>` opens STAC Search on a [STAC](https://stacspec.org) item or collection: an
item's footprint is drawn and its default asset (its true-colour `visual` image, else its first
GeoTIFF) added, signed first when it is a Microsoft Planetary Computer one; a collection is chosen
in the dialog, with its API, ready to search. For example,
`?stac=https://planetarycomputer.microsoft.com/api/stac/v1/collections/sentinel-2-l2a/items/S2B_MSIL2A_20240727T102559_R108_T32TLS_20240727T131405`
opens a Sentinel-2 scene of Lake Geneva.

`?embed` runs the app inside another page, as the [Python package's notebook widget](python.md)
does: it starts blank, keeps its settings in memory rather than in the browser, and exchanges
the project with the page that frames it through `postMessage` (the messages are described in
`apps/rustgis-web/src/embed.rs`).

### Install the web app

The web app installs like an app on phones, tablets and computers, with its own icon and a window
without the browser's toolbars, and starts without a network connection once it has been opened
online. Nothing is downloaded from an app store, and it updates itself from web.rustgis.app.

| Platform | How |
| --- | --- |
| iPhone and iPad | Open web.rustgis.app in Safari, tap **Share**, then **Add to Home Screen** |
| Android | Open it in Chrome, tap the **⋮** menu, then **Install app** (or **Add to Home screen**) |
| Windows, macOS, Linux, ChromeOS | In Chrome or Edge, click the install icon at the right of the address bar (or **⋮ › Cast, save and share › Install page as app**) |
| Mac (Safari 17 or later) | **File › Add to Dock** |

To remove it, delete it like any other app: long-press the icon on a phone or tablet, or use
**Uninstall** in the app window's menu on a computer.

Offline, the installed app opens the World Atlas sample and the files you open, but not basemap
tiles, web data, plugins or the AI Assistant, which need the network. When a new version has been
published while the app is open, a notice offers to reload into it; otherwise the next launch
picks it up. Two things to know on iPhone and iPad: iOS gives a web page less memory than a
desktop browser, so very large rasters may not open, and it can clear the offline copy when the
device runs short of storage, so the next launch then needs a connection.

To build it yourself, install [Trunk](https://trunkrs.dev), the `wasm32-unknown-unknown` target
and clang (it compiles the Zstandard decoder used for GeoTIFFs to WebAssembly). Linux
distributions' clang can; on macOS, Apple's can't, so install `llvm` with Homebrew and put its
`bin` first on your `PATH`. Then:

```sh
cd apps/rustgis-web
trunk serve          # http://127.0.0.1:8772
trunk build --release   # output in dist/web
cargo xtask web-pwa ../../dist/web   # add the service worker (offline start), as the deploy does
```

The service worker is left out of `trunk serve`, so a development build is never served from a
stale cache.

## Fonts

RustGIS uses an installed sans-serif when it finds one (Segoe UI, San Francisco, Inter, Noto
Sans, Open Sans or DejaVu Sans) and falls back to a built-in face. Set
`RUSTGIS_BUILTIN_FONTS=1` to always use the built-in face.

## If RustGIS closes unexpectedly

The desktop app writes its internal errors to `crash.log` in its settings folder: one that closed
the app, and also one it recovered from (an error in the AI Assistant, for example, stops the
request and says so in the chat instead of closing the app). The folder is `%APPDATA%\RustGIS` on
Windows, `~/Library/Application Support/RustGIS` on macOS, and `$XDG_CONFIG_HOME/rustgis` on
Linux (`~/.config/rustgis` when that isn't set). The next launch moves the log to
`crash-last.log` and names the file in a notice.

Attach it to a [bug report](https://github.com/opengeos/RustGIS/issues/new?template=bug_report.yml).
It holds the version, the operating system, the error message and where in RustGIS it happened.
An error message can quote what RustGIS was working on, such as a file path or a layer name, so
read the log first and remove anything you don't want to share.
