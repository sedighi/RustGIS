//! Times map renders of the sample layers, flat and globe, with parts turned off one by one:
//! `cargo run --release -p rustgis-render --example globe_bench`.
use rustgis_data::io::read_geojson;
use rustgis_geom::{Coord, lonlat_to_merc};
use rustgis_map::{Camera, Map, Projection};
use rustgis_render::{View, render, sample_data};

fn main() {
    let mut m = Map::new("m");
    for (t, n) in [
        (sample_data::COUNTRIES, "Countries"),
        (sample_data::LAKES, "Lakes"),
        (sample_data::RIVERS, "Rivers"),
        (sample_data::CITIES, "Cities"),
    ] {
        let (d, _) = read_geojson(t, n).unwrap();
        let id = m.add_dataset(d);
        if n == "Countries" || n == "Cities" {
            m.layer_mut(id).unwrap().labels.enabled = true;
        }
    }
    let variants: Vec<(&str, Box<dyn Fn(&mut Map)>)> = vec![
        ("all", Box::new(|_| {})),
        ("no labels", Box::new(|m: &mut Map| m.layers.iter_mut().for_each(|l| l.labels.enabled = false))),
        ("+ no graticule", Box::new(|m: &mut Map| m.graticule = false)),
        ("+ blank basemap", Box::new(|m: &mut Map| m.basemap = rustgis_map::Basemap::Blank)),
        ("+ no layers", Box::new(|m: &mut Map| m.layers.iter_mut().for_each(|l| l.visible = false))),
    ];
    for (name, f) in variants {
        f(&mut m);
        for (proj, ppp) in [(Projection::WebMercator, 1.0), (Projection::Globe, 1.0), (Projection::Globe, 0.6)] {
            m.projection = proj.clone();
            let t = std::time::Instant::now();
            let n = 20;
            for i in 0..n {
                let camera = Camera { center: lonlat_to_merc(Coord::new(-60.0 + i as f64 * 3.0, 30.0)), res: 20_000.0 };
                let view = View { width: 1100.0, height: 800.0, pixels_per_point: ppp, camera };
                render(&mut m, &view).unwrap();
            }
            println!("{name:16} {proj:?} ppp {ppp}: {:.1} ms/frame", t.elapsed().as_secs_f64() * 1000.0 / n as f64);
        }
    }
}
