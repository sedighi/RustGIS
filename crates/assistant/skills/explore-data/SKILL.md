---
name: explore-data
description: Find out what's in a layer and answer questions from its data — fields, value ranges, top records, counts, filtering and summary statistics. Use for "what's in", "how many", "which … have", "largest", "average".
---

# Exploring data

- **What's there:** `list_layers` gives every layer's fields, feature count and selection. `layer.fields` describes
  one layer's fields in detail.
- **Rows:** use `read_features` with `fields` (only the ones you need), `where`
  (`pop_max > 1000000 AND continent = 'Asia'`), `order_by` with `descending`, and a small `limit`. The reply gives
  the number of matching rows, so you don't need to read them all to count.
- **Statistics:** for sums, means, minima and maxima, or counts by category, use the `summaryStatistics` tool
  (`describe_tool` for its parameters, then `run_tool`). Its output table goes on the map for the next step.
- **Showing the answer on the map:** `select.byAttributes` with the same `where`, then `select.zoomTo`. A lasting
  filter is `layer.definitionQuery`; tell the user, because it hides the other features until it's cleared.
- **Spatial questions** ("within 50 km of", "inside"): use the `selectByLocation` tool, or `buffer` and then
  `clip`.

Report numbers exactly as read, with units if the field name or values give them. Never estimate what you can
read. If a field's meaning is unclear, say what you assumed.
