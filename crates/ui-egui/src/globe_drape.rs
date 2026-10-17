//! The moving globe. While the globe turns or zooms, the map drawn flat once (in Web Mercator)
//! is laid over the sphere as a textured mesh, so a frame only moves vertices, as MapLibre's
//! globe does; the atmosphere and lighting are sprites scaled with the disc. The exact frame is
//! drawn again once the globe settles. Space around it is starry (`render::starfield`).

use egui::epaint::Vertex;
use egui::{Color32, Mesh, Painter, Rect, Shape, TextureFilter, TextureHandle, TextureOptions, TextureWrapMode, pos2, vec2};
use rustgis_engine::geom::{Coord, Extent, merc_to_lonlat};
use rustgis_engine::map::{Basemap, Globe, Map};
use rustgis_engine::render::{self, GLOBE_GLOW, STAR_TILE, View};

use crate::theme::c32;

/// How far past the view the drape reaches on each side, as a fraction of its size: the globe
/// can turn this far before the drape is drawn again.
const MARGIN: f64 = 0.25;
/// Zooming out past the drape draws it this much coarser and with this margin instead, so it
/// keeps covering the view while the zoom goes on, for about the same work.
const ZOOM_OUT_COARSER: f64 = 2.0;
const ZOOM_OUT_MARGIN: f64 = 1.0;
/// Zooming in past this factor of the drape's scale draws it again, sharper.
const MAX_ZOOM_IN: f64 = 2.0;
/// Seconds between drawing the drape again for tiles that have arrived meanwhile.
const TILE_REFRESH: f64 = 0.5;
/// The sprites' disc radius in pixels: drawn once at this size and scaled with the disc (they
/// are smooth gradients, so scaling doesn't show).
const SPRITE_R: f32 = 512.0;

/// What a drape shows, besides the view: a change draws it again.
#[derive(Clone, Copy, PartialEq)]
pub(crate) struct DrapeKey {
    /// The session's revision.
    pub rev: u64,
    /// Streamed layers' reads finished so far.
    pub sources: u64,
}

struct Drape {
    key: DrapeKey,
    /// Pixels per point it was drawn at.
    ppp: f32,
    /// Its detail, in Web Mercator metres per point.
    res: f64,
    /// The camera's resolution when it was drawn.
    cam_res: f64,
    /// The tile cache's generation when it was drawn, and when (seconds).
    tiles: u64,
    built: f64,
    /// Each picture: its longitude/latitude box, its Web Mercator box and its texture.
    patches: Vec<(Extent, Extent, TextureHandle)>,
}

struct Sprites {
    key: Basemap,
    under: TextureHandle,
    over: TextureHandle,
}

/// The drape and sprites of the moving globe, kept between drags while the map doesn't change.
#[derive(Default)]
pub(crate) struct GlobeDrape {
    drape: Option<Drape>,
    sprites: Option<Sprites>,
    /// The starfield tile and the pixels per point it was drawn at.
    stars: Option<(f32, TextureHandle)>,
}

impl GlobeDrape {
    /// Lets go of the textures (the map is no longer a globe).
    pub fn clear(&mut self) {
        self.drape = None;
        self.sprites = None;
        self.stars = None;
    }

    /// Makes sure the drape covers the `w × h` view, drawing it again when the view has turned
    /// or zoomed past it or the map changed. False when there is nothing to drape.
    #[allow(clippy::too_many_arguments)]
    pub fn prepare(&mut self, ctx: &egui::Context, map: &mut Map, key: DrapeKey, tiles: u64, w: f64, h: f64, ppp: f32, time: f64) -> bool {
        let g = Globe::new(&map.camera, w, h);
        // A screen's worth of detail: at most one point per pixel, so a dense screen keeps the
        // drawing cheap (the settled frame is sharp).
        let drape_ppp = ppp.min(1.0);
        let res = map.camera.res;
        let fresh = self.drape.as_ref().is_some_and(|d| {
            let boxes: Vec<Extent> = d.patches.iter().map(|p| p.0).collect();
            d.key == key
                && d.ppp == drape_ppp
                && res * MAX_ZOOM_IN >= d.res
                && (d.tiles == tiles || time - d.built < TILE_REFRESH)
                && g.drape_covers(w, h, &boxes)
        });
        if !fresh {
            // Zooming out: coarser and wider, so the next steps out are covered too.
            let zooming_out = self.drape.as_ref().is_some_and(|d| res > d.cam_res * 1.01);
            let (detail, margin) = if zooming_out { (res * ZOOM_OUT_COARSER, ZOOM_OUT_MARGIN) } else { (res, MARGIN) };
            let boxes = g.drape_extents(w, h, margin);
            let max_side = ctx.input(|i| i.max_texture_side).clamp(256, 4096) as u32;
            let camera = rustgis_engine::map::Camera { res: detail, ..map.camera };
            let view = View { width: w, height: h, pixels_per_point: f64::from(drape_ppp), camera };
            let pictures = render::render_globe_drape(map, &view, &boxes, max_side);
            if pictures.is_empty() {
                self.drape = None;
                return false;
            }
            let options = TextureOptions { mipmap_mode: Some(TextureFilter::Linear), ..TextureOptions::LINEAR };
            let mut old = self.drape.take().map(|d| d.patches).unwrap_or_default().into_iter().map(|p| p.2);
            let patches = pictures
                .into_iter()
                .enumerate()
                .map(|(i, p)| {
                    let size = [p.pixmap.width() as usize, p.pixmap.height() as usize];
                    let img = egui::ColorImage::from_rgba_premultiplied(size, p.pixmap.data());
                    let tex = match old.next() {
                        Some(mut t) => {
                            t.set(img, options);
                            t
                        }
                        None => ctx.load_texture(format!("globe_drape_{i}"), img, options),
                    };
                    let ll = Extent::from_points(
                        merc_to_lonlat(Coord::new(p.merc.min_x, p.merc.min_y)),
                        merc_to_lonlat(Coord::new(p.merc.max_x, p.merc.max_y)),
                    );
                    (ll, p.merc, tex)
                })
                .collect();
            self.drape = Some(Drape { key, ppp: drape_ppp, res: detail, cam_res: res, tiles, built: time, patches });
        }
        // The atmosphere and lighting, drawn once for the basemap.
        let skey = map.basemap;
        if self.sprites.as_ref().is_none_or(|s| s.key != skey)
            && let Some((under, over)) = render::globe_sprites(map.basemap, SPRITE_R)
        {
            let tex = |name: &str, pm: &render::Pixmap| {
                let img = egui::ColorImage::from_rgba_premultiplied([pm.width() as usize, pm.height() as usize], pm.data());
                ctx.load_texture(name, img, TextureOptions::LINEAR)
            };
            self.sprites = Some(Sprites { key: skey, under: tex("globe_under", &under), over: tex("globe_over", &over) });
        }
        true
    }

    /// Paints the space around the globe in `rect`, with the stars of the map's view effects
    /// where the settled frame draws them (laid edge to edge from the top left corner), at `ppp`
    /// pixels per point.
    pub fn paint_space(&mut self, ctx: &egui::Context, painter: &Painter, rect: Rect, map: &Map, ppp: f32) {
        painter.rect_filled(rect, 0.0, c32(render::globe_space()));
        if !map.view.effects.stars {
            return;
        }
        if self.stars.as_ref().is_none_or(|s| s.0 != ppp)
            && let Some(pm) = render::starfield(ppp)
        {
            let img = egui::ColorImage::from_rgba_premultiplied([pm.width() as usize, pm.height() as usize], pm.data());
            let options = TextureOptions { wrap_mode: TextureWrapMode::Repeat, ..TextureOptions::NEAREST };
            self.stars = Some((ppp, ctx.load_texture("globe_stars", img, options)));
        }
        if let Some((_, tex)) = &self.stars {
            // One rectangle, its texture repeated across it.
            let uv = Rect::from_min_size(pos2(0.0, 0.0), rect.size() / STAR_TILE);
            painter.image(tex.id(), rect, uv, Color32::WHITE);
        }
    }

    /// Paints the globe in `rect`: space, the atmosphere and ocean, the draped map and the
    /// lighting.
    pub fn paint(&mut self, ctx: &egui::Context, painter: &Painter, rect: Rect, map: &Map, ppp: f32) {
        let (w, h) = (rect.width() as f64, rect.height() as f64);
        let g = Globe::new(&map.camera, w, h);
        self.paint_space(ctx, painter, rect, map, ppp);
        let (cx, cy) = g.center();
        let centre = pos2(rect.min.x + cx as f32, rect.min.y + cy as f32);
        let disc = g.disc_radius() as f32;
        let sprites = self.sprites.as_ref().filter(|_| g.disc_radius() < 3.0 * w.hypot(h));
        let sprite_rect = Rect::from_center_size(centre, vec2(2.0, 2.0) * disc * GLOBE_GLOW);
        let full = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
        match sprites {
            Some(s) => {
                painter.image(s.under.id(), sprite_rect, full, Color32::WHITE);
            }
            None => {
                painter.rect_filled(rect, 0.0, c32(map.basemap.palette().0));
            }
        }
        for mesh in self.meshes(rect, &g) {
            painter.add(Shape::mesh(mesh));
        }
        if let Some(s) = sprites {
            painter.image(s.over.id(), sprite_rect, full, Color32::WHITE);
        }
    }

    /// The drape's meshes for the globe `g` in `rect`. A picture wholly behind the horizon has
    /// no triangles and is left out: the GPU can't draw an empty mesh (egui-wgpu panics on an
    /// empty buffer).
    fn meshes(&self, rect: Rect, g: &Globe) -> Vec<Mesh> {
        let Some(d) = &self.drape else { return Vec::new() };
        d.patches
            .iter()
            .filter_map(|(_, merc, tex)| {
                let m = g.drape_mesh(merc);
                if m.indices.is_empty() || !m.pos.iter().flatten().all(|v| v.is_finite()) {
                    return None;
                }
                let mut mesh = Mesh::with_texture(tex.id());
                mesh.vertices = m
                    .pos
                    .iter()
                    .zip(&m.uv)
                    .map(|(p, uv)| Vertex {
                        pos: pos2(rect.min.x + p[0], rect.min.y + p[1]),
                        uv: pos2(uv[0], uv[1]),
                        color: Color32::WHITE,
                    })
                    .collect();
                mesh.indices = m.indices;
                Some(mesh)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use rustgis_engine::geom::lonlat_to_merc;
    use rustgis_engine::map::{Camera, Projection};

    use super::*;

    fn globe(lon: f64, res: f64) -> Map {
        let mut m = Map::new("m");
        m.projection = Projection::Globe;
        m.camera = Camera { center: lonlat_to_merc(Coord::new(lon, 0.0)), res };
        m
    }

    #[test]
    fn a_picture_behind_the_horizon_gives_no_empty_mesh() {
        // Zoomed out, facing 70°E: the drape's margin runs past the antimeridian, so its second
        // picture (180°–170°W) is all on the far side. Its empty mesh crashed egui-wgpu.
        let ctx = egui::Context::default();
        let mut m = globe(70.0, 20_000.0);
        let key = DrapeKey { rev: 0, sources: 0 };
        let mut d = GlobeDrape::default();
        assert!(d.prepare(&ctx, &mut m, key, 0, 800.0, 600.0, 1.0, 0.0));
        assert_eq!(d.drape.as_ref().map(|d| d.patches.len()), Some(2));
        let rect = Rect::from_min_size(pos2(0.0, 0.0), vec2(800.0, 600.0));
        let meshes = d.meshes(rect, &Globe::new(&m.camera, 800.0, 600.0));
        assert_eq!(meshes.len(), 1);
        assert!(meshes.iter().all(|mesh| !mesh.indices.is_empty() && mesh.is_valid()));
    }

    #[test]
    fn zooming_out_draws_a_wider_drape_and_keeps_it() {
        let ctx = egui::Context::default();
        let mut m = globe(10.0, 500.0);
        let key = DrapeKey { rev: 0, sources: 0 };
        let mut d = GlobeDrape::default();
        assert!(d.prepare(&ctx, &mut m, key, 0, 800.0, 600.0, 1.0, 0.0));
        // Out past the margin: drawn again, coarser and wider ...
        m.camera.res = 900.0;
        assert!(d.prepare(&ctx, &mut m, key, 0, 800.0, 600.0, 1.0, 0.1));
        assert_eq!(d.drape.as_ref().map(|d| d.res), Some(1800.0));
        // ... so a further step out is still covered, and nothing is drawn.
        m.camera.res = 1_300.0;
        let built = d.drape.as_ref().map(|d| d.built);
        assert!(d.prepare(&ctx, &mut m, key, 0, 800.0, 600.0, 1.0, 0.2));
        assert_eq!(d.drape.as_ref().map(|d| d.built), built);
        // The sprites are drawn once and kept while zooming.
        assert!(d.sprites.is_some());
    }
}
