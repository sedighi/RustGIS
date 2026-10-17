# Workspace

Tools for working across maps and views, and for setting up the app the way you like it.

- **Side by Side** shows a second map beside the active one; **Link Views** keeps maps at the same
  place and scale, and **Link Cursors** marks the pointer's place on the other map.
- **Browser View** shows the project's maps, layouts, tables, styles, tasks, models, folders and
  offline areas as cards (double-click to open); **Item Pop-ups** add hover details.
- **Pane Sets** save the panes you have open and bring them back. **Tasks** steps through a
  task's instructions; the **Workflow Manager** tracks jobs (new, in progress, on hold, done)
  with the area each covers.
- **Thumbnails** for the project and for each map, from the view or an image.
- **Animation**: add keyframes (views), play them, and export the moves as PNG frames.
- **Profile**: elevation profiles along clicked points or selected lines, auto-completed to the
  raster's edge, or across a line (a cross section), with gain, loss and a chart.
- **Infographics** (Home tab) summarise what's in view: counts, totals and top categories.
- **Offline**: Download Map takes the data in view into an offline area; open it to edit, then
  Sync sends changed, new and deleted features back and refreshes it. **Feature Cache** prepares
  every layer's geometry for the current projection (Fill, Empty, Cancel, Auto Cache).
- **Help**: choose the ribbon's tabs and the Quick Access Toolbar's buttons, open the user guide,
  and watch the **Diagnostic Monitor** (frame rate, draw time, data size).
- **Plugins**: **Plugins › Plugin Manager** installs, updates and removes [plugins](../plugins.md)
  from the registry at [plugins.rustgis.app](https://plugins.rustgis.app) or from a file.
- **Language**: **Settings › Language** switches the interface among
  [16 languages](../i18n.md) and chooses the [worldview](maps.md#worldview) of the built-in
  borders.
- **Start-up**: **Settings › Start-up** sets what RustGIS opens with: **Start With** the World Atlas
  sample or a blank map, or, in the desktop app, **Last Project** (the `.rgis` project last opened
  or saved) or **Choose Project…** (one you pick once), the **Projection** the map opens in (a
  preset or a supported projected EPSG code, such as `3010`), and the **Extent** it opens on (the
  full extent, or **Use Current View** to keep the view you are looking at). A project opened at
  start keeps its own projection and view; the Projection and Extent settings apply to the sample
  and a blank map. If the project no longer opens (moved or deleted), RustGIS starts with the sample
  and says why. After a crash it starts with the sample too, in case the project is what crashed;
  starting `rustgis --sample` also skips the start-up project. The choices are remembered on this
  computer, or in this browser for the web app. Opening files from the command line starts a blank
  map.
