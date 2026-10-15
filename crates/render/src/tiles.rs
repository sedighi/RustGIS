//! The map's web tile layers, drawn over the basemap and under the layers.
//!
//! On the flat Web Mercator map tiles draw straight into the map. In any other projection, and
//! on the globe, they draw into a Web Mercator mosaic of the area in view, which is then warped
//! into place pixel by pixel.

use rustgis_geom::{Coord, Extent, MAX_LAT, lonlat_to_merc};
use rustgis_map::projection::Flat;
use rustgis_map::{Map, Rgba, TileKind, TileLayer};
use rustgis_tiles::{Frame, Kind, Spec};
use tiny_skia::Pixmap;

use crate::raster::Place;
use crate::{PlacedIcon, PlacedLabel, View};

/// The largest side of the mosaic warped onto the globe or another projection.
const MOSAIC: f64 = 2048.0;

/// What drawing the tile layers found.
#[derive(Default)]
pub(crate) struct TileOutput {
    pub labels: Vec<(PlacedLabel, f32)>,
    pub attribution: Vec<String>,
    pub pending: bool,
    pub errors: Vec<String>,
}

/// How the tiles crate sees a tile layer.
pub fn spec(t: &TileLayer) -> Spec {
    Spec {
        kind: match t.kind {
            TileKind::Raster => Kind::Raster,
            TileKind::Vector => Kind::Vector,
            TileKind::Style => Kind::Style,
            TileKind::PmTiles => Kind::PmTiles,
            TileKind::MbTiles => Kind::MbTiles,
            TileKind::Cog => Kind::Cog,
        },
        url: t.url.clone(),
        subdomains: t.subdomains.clone(),
        min_zoom: t.min_zoom,
        max_zoom: t.max_zoom,
        tile_size: t.tile_size,
        tms: t.tms,
        attribution: t.attribution.clone(),
        layers: t.layers.clone(),
    }
}

/// Draws the map's visible tile layers, bottom first.
pub(crate) fn draw(pm: &mut Pixmap, map: &Map, view: &View, place: Place, labels: bool) -> TileOutput {
    let mut out = TileOutput::default();
    let layers: Vec<&TileLayer> = map.tiles.iter().rev().filter(|t| t.visible && t.opacity > 0.0).collect();
    if layers.is_empty() {
        return out;
    }
    let ppp = view.pixels_per_point as f32;
    let collect = |d: rustgis_tiles::Drawn, name: &str, out: &mut TileOutput| {
        out.pending |= d.pending;
        for a in d.attribution {
            if !out.attribution.contains(&a) {
                out.attribution.push(a);
            }
        }
        if let Some(e) = d.error {
            out.errors.push(format!("{name}: {e}"));
        }
        for l in d.labels {
            // Styles often label in two scripts ("Syria\nسوريا"): only the lines the font can
            // draw are kept, so a name never shows as boxes, and a label left with neither text
            // nor an icon is left out.
            let lines: Vec<&str> = l.text.lines().map(str::trim).filter(|s| !s.is_empty() && crate::text::covers(s, l.bold)).collect();
            if lines.is_empty() && l.icon.is_none() {
                continue;
            }
            let text = lines.join(" ");
            // The text sits against the point as the style anchors and offsets it (beside its
            // icon, for instance); without either it is centred on the point. Across, it is
            // placed by the edge it is anchored at, so each renderer lines it up with its own
            // measure of the width.
            let (th, em) = (l.size * 1.25, l.size * 1.33);
            let x = l.x + l.offset[0] * em;
            let y = l.y + l.anchor[1] * th + l.offset[1] * em;
            let icon = l.icon.map(|i| PlacedIcon { image: i.image, x: l.x + i.dx, y: l.y + i.dy, w: i.width, h: i.height });
            out.labels.push((
                PlacedLabel {
                    text,
                    x,
                    y,
                    size: l.size,
                    color: Rgba(l.color),
                    halo: l.halo.map(Rgba),
                    bold: l.bold,
                    icon,
                    align: -l.anchor[0],
                },
                l.priority,
            ));
        }
    };
    if let Place::Flat(Flat::Mercator, _) = place {
        let res = view.camera.res;
        let frame = Frame {
            left: view.camera.center.x - view.width / 2.0 * res,
            top: view.camera.center.y + view.height / 2.0 * res,
            m_per_px: res / view.pixels_per_point.max(0.01),
            ppp,
        };
        for t in layers {
            let d = rustgis_tiles::draw(pm, &spec(t), t.opacity, &frame, labels);
            collect(d, &t.name, &mut out);
        }
        return out;
    }
    // Elsewhere: a mosaic in Web Mercator, warped into place.
    let (w, h) = (pm.width(), pm.height());
    let k = view.pixels_per_point.max(0.01);
    let to_lonlat = |dx: f64, dy: f64| -> Option<Coord> {
        let (sx, sy) = (dx / k, dy / k);
        let c = match place {
            Place::Globe(g) => g.inverse(sx, sy)?,
            Place::Flat(flat, _) => flat.inverse(view.camera.to_map(sx, sy, view.width, view.height))?,
        };
        (c.is_finite() && c.y.abs() <= 90.0).then_some(c)
    };
    let to_merc = |c: Coord| lonlat_to_merc(Coord::new(c.x, c.y.clamp(-MAX_LAT, MAX_LAT)));
    // The area in view, from a grid of points, and the detail needed at its centre.
    let mut e = Extent::EMPTY;
    let n = 24;
    for i in 0..=n {
        for j in 0..=n {
            if let Some(c) = to_lonlat(f64::from(w) * f64::from(i) / f64::from(n), f64::from(h) * f64::from(j) / f64::from(n)) {
                e.expand(to_merc(c));
            }
        }
    }
    if e.is_empty() {
        return out;
    }
    let (cx, cy) = (f64::from(w) / 2.0, f64::from(h) / 2.0);
    let detail = match (to_lonlat(cx, cy), to_lonlat(cx + 4.0, cy)) {
        (Some(a), Some(b)) => (to_merc(a).x - to_merc(b).x).hypot(to_merc(a).y - to_merc(b).y) / 4.0,
        _ => 0.0,
    };
    let m_per_px = detail.max(e.width() / MOSAIC).max(e.height() / MOSAIC).max(0.01);
    let (mw, mh) = ((e.width() / m_per_px).ceil().clamp(1.0, MOSAIC) as u32, (e.height() / m_per_px).ceil().clamp(1.0, MOSAIC) as u32);
    let Some(mut mosaic) = Pixmap::new(mw, mh) else { return out };
    // Pixels per point in the mosaic: line widths and the style's zoom then match the view's
    // centre once the mosaic is warped there.
    let mosaic_ppp = if detail > 0.0 { (detail * k / m_per_px) as f32 } else { ppp };
    let frame = Frame { left: e.min_x, top: e.max_y, m_per_px, ppp: mosaic_ppp.clamp(0.05, 8.0) };
    for t in layers {
        let d = rustgis_tiles::draw(&mut mosaic, &spec(t), t.opacity, &frame, false);
        collect(d, &t.name, &mut out);
    }
    warp(pm, &mosaic, &frame, &to_lonlat, &to_merc);
    out
}

/// Draws the mosaic over `pm`: each pixel takes the mosaic pixel under its longitude/latitude.
fn warp(pm: &mut Pixmap, mosaic: &Pixmap, frame: &Frame, to_lonlat: &dyn Fn(f64, f64) -> Option<Coord>, to_merc: &dyn Fn(Coord) -> Coord) {
    let (w, h) = (pm.width() as usize, pm.height() as usize);
    let (mw, mh) = (mosaic.width() as usize, mosaic.height() as usize);
    let src = mosaic.data();
    let dst = pm.data_mut();
    for y in 0..h {
        for x in 0..w {
            let Some(c) = to_lonlat(x as f64 + 0.5, y as f64 + 0.5) else { continue };
            let m = to_merc(c);
            let (mx, my) = ((m.x - frame.left) / frame.m_per_px, (frame.top - m.y) / frame.m_per_px);
            if !(mx >= 0.0 && my >= 0.0) {
                continue;
            }
            let (mx, my) = (mx as usize, my as usize);
            if mx >= mw || my >= mh {
                continue;
            }
            let si = (my * mw + mx) * 4;
            let di = (y * w + x) * 4;
            let (Some(s), Some(d)) = (src.get(si..si + 4), dst.get_mut(di..di + 4)) else { continue };
            // Source-over, premultiplied.
            let a = u32::from(s[3]);
            if a == 0 {
                continue;
            }
            for i in 0..4 {
                d[i] = (u32::from(s[i]) + u32::from(d[i]) * (255 - a) / 255).min(255) as u8;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use rustgis_geom::{Coord, lonlat_to_merc};
    use rustgis_map::{Camera, Map, Projection, TileKind, TileLayer};

    use crate::View;

    /// Serves tiles and documents for `https://tiles.test/…` without a network.
    fn serve() {
        rustgis_tiles::cache::set_fetcher(|url, done| {
            if url.starts_with("https://tiles.test/red/") {
                let mut pm = tiny_skia::Pixmap::new(256, 256).unwrap();
                pm.fill(tiny_skia::Color::from_rgba8(220, 20, 20, 255));
                done(Ok(pm.encode_png().unwrap()));
            } else if url == "https://tiles.test/style.json" {
                let style = r##"{"version": 8, "sources": {}, "layers": [{"id": "bg", "type": "background", "paint": {"background-color": "#2050e0"}}]}"##;
                done(Ok(style.as_bytes().to_vec()));
            } else {
                done(Err("404".into()));
            }
        });
    }

    fn pixel(map: &mut Map) -> [u8; 4] {
        let camera = Camera { center: lonlat_to_merc(Coord::new(10.0, 50.0)), res: 2000.0 };
        let view = View { width: 200.0, height: 100.0, pixels_per_point: 1.0, camera, dark: false };
        let png = crate::render_png(map, &view).unwrap();
        let pm = tiny_skia::Pixmap::decode_png(&png).unwrap();
        let p = pm.pixel(100, 50).unwrap();
        [p.red(), p.green(), p.blue(), p.alpha()]
    }

    #[test]
    fn draws_raster_tiles_and_styles() {
        serve();
        let mut m = Map::new("Map");
        m.graticule = false;
        let mut t = TileLayer::new("Red", TileKind::Raster, "https://tiles.test/red/{z}/{x}/{y}.png");
        t.attribution = "© Test".into();
        m.tiles.push(t);
        assert_eq!(pixel(&mut m), [220, 20, 20, 255]);
        // Half transparent over the basemap.
        m.tiles[0].opacity = 0.5;
        let p = pixel(&mut m);
        assert!(p[0] > 200 && (100..180).contains(&p[1]), "{p:?}");
        // A style on top, and the globe.
        m.tiles.insert(0, TileLayer::new("Blue", TileKind::Style, "https://tiles.test/style.json"));
        assert_eq!(pixel(&mut m), [0x20, 0x50, 0xE0, 255]);
        m.projection = Projection::Globe;
        let p = pixel(&mut m);
        // Shaded a little on the globe.
        assert!(p[2] > 150 && p[0] < 90, "{p:?}");
        // A layer that can't load draws nothing and says why.
        m.projection = Projection::WebMercator;
        m.tiles = vec![TileLayer::new("Gone", TileKind::Style, "https://tiles.test/missing.json")];
        let camera = Camera { center: lonlat_to_merc(Coord::new(10.0, 50.0)), res: 2000.0 };
        let view = View { width: 200.0, height: 100.0, pixels_per_point: 1.0, camera, dark: false };
        let r = crate::render_complete(&mut m, &view).unwrap();
        assert!(!r.tiles_pending);
        assert_eq!(r.tile_errors.len(), 1, "{:?}", r.tile_errors);
    }
}
