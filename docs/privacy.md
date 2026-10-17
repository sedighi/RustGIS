# Privacy

The RustGIS desktop app and command line collect no analytics, telemetry or usage data. Your
geospatial data and projects stay on your device unless you choose to share or export them.

## Online services

Some features ask a public server for an answer, and send it what the answer needs: **Find Place**
and the search box send the place you look up to OpenStreetMap's Nominatim, and **Directions**
sends a route's start, stops and destination to the Valhalla server run by FOSSGIS (it asks you
first). Web basemaps and data added from a web address are fetched from their servers. Nothing
else about you or your data is sent with them.

## Website analytics

The websites we host, the documentation at [rustgis.app](https://rustgis.app) and the web app at
[web.rustgis.app](https://web.rustgis.app), use Google Analytics 4 to count page visits and see
which pages are used. Google receives your IP address, the page you are on, the referring page and
general device and browser information, and sets its own cookies in your browser. It is governed
by [Google's privacy policy](https://policies.google.com/privacy).

Analytics never see the data you work with: layers, files, projects and coordinates are processed
in your browser and are not sent to Google. The web app reports its address trimmed to the site and
path (`https://web.rustgis.app/`), without the query string. The documentation reports the full
page address, which names the page you are reading and, after a search, the words you searched for.

You can block analytics with a content blocker or your browser's tracking protection; the sites
work normally without them. Analytics run only on the sites we host: the measurement ID is added
when those sites are deployed, so the desktop app, the command line and any copy you build
yourself never load them.
