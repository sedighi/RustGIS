//! The built-in sample project: a world map from Natural Earth (public domain) data.

use rustgis_data::io::read_geojson;
use rustgis_geom::{Coord, lonlat_to_merc};
use rustgis_map::{Bookmark, Camera, ClassMethod, Marker, Ramp, Renderer, Rgba, Symbol};

use crate::Project;

pub use rustgis_render::sample_data::{CITIES, COUNTRIES, COUNTRIES_CHN, LAKES, RIVERS, Worldview, countries, set_worldview, worldview};

/// The worldview this system calls for: `RUSTGIS_WORLDVIEW` (`default` or `china`) when set, else
/// China when the system's time zone or first locale is mainland China's. Nothing is looked up
/// over the network. Test builds of the engine don't look at the system.
pub fn system_worldview() -> Worldview {
    if let Some(w) = std::env::var("RUSTGIS_WORLDVIEW").ok().and_then(|v| Worldview::from_code(&v)) {
        return w;
    }
    if cfg!(test) {
        return Worldview::Default;
    }
    let tz = iana_time_zone::get_timezone().ok();
    let locales: Vec<String> = sys_locale::get_locales().take(4).collect();
    Worldview::detect(tz.as_deref(), locales.iter().map(String::as_str))
}

/// Applies a worldview preference: `auto` (or anything unknown) follows the system.
pub fn apply_worldview(pref: &str) -> Worldview {
    let w = Worldview::from_code(pref).unwrap_or_else(system_worldview);
    set_worldview(w);
    w
}

fn cam(lon: f64, lat: f64, res: f64) -> Camera {
    Camera { center: lonlat_to_merc(Coord::new(lon, lat)), res }
}

/// The built-in countries in a worldview.
fn countries_in(w: Worldview) -> &'static str {
    match w {
        Worldview::Default => COUNTRIES,
        Worldview::China => COUNTRIES_CHN,
    }
}

/// Swaps an untouched sample project's countries from worldview `from` to `to`: only in a project
/// that is unsaved, unchanged and has nothing to undo, and only a layer named `Countries` holding
/// exactly the built-in countries of `from`. Returns whether a layer was swapped.
pub fn swap_countries(s: &mut crate::Session, from: Worldview, to: Worldview) -> bool {
    if from == to || s.dirty || s.path.is_some() || s.can_undo() {
        return false;
    }
    let read = |w| read_geojson(countries_in(w), "Countries").ok().map(|(ds, _)| ds);
    let (Some(old), Some(new)) = (read(from), read(to)) else {
        return false;
    };
    let Some(l) = s.map_mut().layers.iter_mut().find(|l| l.name == "Countries" && l.dataset.features == old.features) else {
        return false;
    };
    l.dataset = new;
    l.selection.clear();
    l.touch_geometry();
    true
}

/// Slate gray country borders: visible against both the near-white lowest population class and the ocean.
const COUNTRY_BORDER: Rgba = Rgba::rgb(0x7D, 0x8C, 0x96);

/// "World Atlas": countries by population, cities sized by population, rivers and lakes.
pub fn world_project() -> Project {
    let mut p = Project::new("World Atlas");
    let map = p.map_mut();
    map.name = "World".into();
    map.basemap = rustgis_map::Basemap::Ocean;
    let mut add = |name: &str, text: &str| read_geojson(text, name).ok().map(|(ds, _)| map.add_dataset(ds));
    let countries = add("Countries", countries());
    let lakes = add("Lakes", LAKES);
    let rivers = add("Rivers", RIVERS);
    let cities = add("Cities", CITIES);
    if let Some(l) = countries.and_then(|id| map.layer_mut(id)) {
        let base = Symbol { fill: Rgba::rgb(200, 200, 200), stroke: COUNTRY_BORDER, stroke_width: 0.6, size: 0.0, marker: Marker::Circle };
        if let Ok(r) = Renderer::graduated(&l.dataset, &[], "pop_est", 5, ClassMethod::NaturalBreaks, Ramp::Lagoon, base) {
            l.renderer = r;
            l.renderer.map_symbols(|s| {
                s.stroke = COUNTRY_BORDER;
                s.stroke_width = 0.6;
            });
        }
        l.labels.enabled = true;
        l.labels.field = "name".into();
        l.labels.size = 8.5;
        l.labels.color = Rgba::rgb(0x33, 0x3A, 0x44);
    }
    if let Some(l) = lakes.and_then(|id| map.layer_mut(id)) {
        l.renderer = Renderer::Single(Symbol {
            fill: Rgba::rgb(0x9C, 0xC6, 0xE0),
            stroke: Rgba::rgb(0x6E, 0xA4, 0xC8),
            stroke_width: 0.5,
            size: 0.0,
            marker: Marker::Circle,
        });
    }
    if let Some(l) = rivers.and_then(|id| map.layer_mut(id)) {
        l.renderer = Renderer::Single(Symbol {
            fill: Rgba::rgb(0x4F, 0x8F, 0xC6),
            stroke: Rgba::rgb(0x4F, 0x8F, 0xC6),
            stroke_width: 1.2,
            size: 0.0,
            marker: Marker::Circle,
        });
    }
    if let Some(l) = cities.and_then(|id| map.layer_mut(id)) {
        let base = Symbol {
            fill: Rgba::new(0xE0, 0x6A, 0x3B, 220),
            stroke: Rgba::rgb(255, 255, 255),
            stroke_width: 0.8,
            size: 6.0,
            marker: Marker::Circle,
        };
        if let Ok(r) = Renderer::proportional(&l.dataset, &[], "pop_max", base) {
            l.renderer = r;
            if let Renderer::Proportional { min_size, max_size, .. } = &mut l.renderer {
                *min_size = 3.5;
                *max_size = 18.0;
            }
        }
        l.labels.field = "name".into();
        l.labels.size = 8.0;
        l.labels.bold = true;
        l.labels.color = Rgba::rgb(0x5A, 0x2A, 0x14);
        l.max_scale = None;
    }
    map.bookmarks = vec![
        Bookmark::new("World", cam(10.0, 15.0, 26_000.0)),
        Bookmark::new("Europe", cam(12.0, 50.0, 5_200.0)),
        Bookmark::new("Africa", cam(19.0, 2.0, 8_500.0)),
        Bookmark::new("Americas", cam(-80.0, 12.0, 15_000.0)),
        Bookmark::new("East Asia", cam(118.0, 32.0, 6_500.0)),
    ];
    map.camera = cam(10.0, 18.0, 26_000.0);
    p
}
