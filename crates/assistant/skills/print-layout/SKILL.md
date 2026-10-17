---
name: print-layout
description: Make a printable page layout — map frame, title, legend, north arrow, scale bar — and export it to PDF or PNG. Use when asked for a "layout", "print", "PDF", "poster" or a map "with a legend".
---

# Print layout

1. Get the map right first: symbology, labels and extent. A layout's map frame shows the map's current view.
2. `layout.new` `{"name": N, "page": "letter"|"a4"|…, "orientation": "landscape"|"portrait"}` creates the layout and
   opens it.
3. `layout.addFrame` for the map frame (`search_commands` "map frame" for its position and size parameters, in
   millimetres from the page's top-left).
4. `layout.addElement` for each element with a `kind`: `"text"` (the title, large and bold, at the top), `"legend"`,
   `"northArrow"`, `"scaleBar"`, and optionally `"dynamicText"` (the date or a source line). Keep a margin of about
   10 mm. The legend and scale bar usually go along the bottom or the right edge.
5. Adjust with `layout.update` (`{"element": id, "x", "y", "w", "h", "text", "size", "bold"}`). The ids come from the
   results of the earlier calls.
6. `layout.export` `{"path": "map.pdf"}`, or `.png` with a `dpi` (300 for print). Exporting writes a file, so the
   user is asked first. `layout.close` goes back to the map.

Tell the user the page size and what's on it, and where the file is.
