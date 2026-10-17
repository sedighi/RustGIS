//! Times map renders of large data files, zoomed to the whole layer and in a few steps:
//! `cargo run --release -p rustgis-engine --example large_bench -- file.tif [file.gpkg …]`.
//!
//! Each file is added on its own (streamed sources are loaded in place first), then rendered at
//! 1600 × 1000, from its full extent down eight halvings of the resolution (streamed layers
//! settle at each first). `--ppp 2` renders as a high-density screen.
use rustgis_engine::render::{View, render};
use rustgis_engine::{Project, Session};
use rustgis_geom::{Coord, extent_to_merc};
use rustgis_map::Camera;
use serde_json::json;
use std::time::Instant;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let ppp = args.iter().position(|a| a == "--ppp").and_then(|i| args.get(i + 1)).and_then(|v| v.parse().ok()).unwrap_or(1.0);
    let flag_value = |i: usize| i > 0 && args.get(i - 1).is_some_and(|a| a.starts_with("--"));
    let files: Vec<&String> = args.iter().enumerate().filter(|(i, a)| !a.starts_with("--") && !flag_value(*i)).map(|(_, a)| a).collect();
    for f in files {
        let mut s = Session::new(Project::default());
        s.map_mut().layers.clear();
        let t = Instant::now();
        if let Err(e) = s.run("layer.addData", &json!({ "path": f })) {
            println!("{f}: {e}");
            continue;
        }
        println!("{f}: added in {:.0} ms", t.elapsed().as_secs_f64() * 1000.0);
        // `--cmd 'id={json}'` styles the layer first (Unique Values, say); a command that fails
        // would time a different map, so the file is skipped.
        let styled = args.windows(2).filter(|w| w.first().is_some_and(|a| a == "--cmd")).filter_map(|w| w.get(1)).all(|c| {
            let (id, p) = c.split_once('=').unwrap_or((c, "{}"));
            let r = serde_json::from_str(p).map_err(|e| format!("bad JSON: {e}")).and_then(|p| s.run(id, &p));
            r.map_err(|e| println!("  {id}: {e}; skipped")).is_ok()
        });
        if !styled {
            continue;
        }
        let e = extent_to_merc(&s.map().full_extent());
        let (w, h) = (1600.0, 1000.0);
        let res0 = (e.width() / w).max(e.height() / h).max(1e-3);
        // Let streamed sources settle at the first view (they read in the background).
        let cam0 = Camera { center: Coord::new((e.min_x + e.max_x) / 2.0, (e.min_y + e.max_y) / 2.0), res: res0 };
        let t = Instant::now();
        if !settle(&mut s, &View { width: w, height: h, pixels_per_point: ppp, camera: cam0 }) {
            println!("  still reading after 10 s; skipped");
            continue;
        }
        let n: usize = s.map().layers.iter().map(|l| l.dataset.features.len()).sum();
        println!("  settled in {:.0} ms ({n} features loaded)", t.elapsed().as_secs_f64() * 1000.0);
        for z in 0..9 {
            let res = res0 / 2f64.powi(z);
            let camera = Camera { center: cam0.center, res };
            let view = View { width: w, height: h, pixels_per_point: ppp, camera };
            // Streamed layers read what this view shows first.
            if !settle(&mut s, &view) {
                println!("  zoom +{z}: still reading after 10 s; skipped");
                continue;
            }
            let n = 5;
            let mut times = Vec::new();
            for _ in 0..n {
                let t = Instant::now();
                if render(s.map_mut(), &view).is_none() {
                    break;
                }
                times.push(t.elapsed().as_secs_f64() * 1000.0);
            }
            if times.len() < n {
                println!("  zoom +{z}: no frame rendered; skipped");
                continue;
            }
            let first = times.first().copied().unwrap_or(0.0);
            let rest = times.iter().skip(1).sum::<f64>() / (n - 1) as f64;
            let n_feat: usize = s.map().layers.iter().map(|l| l.dataset.features.len()).sum();
            println!("  zoom +{z} (res {res:>12.2} m, {n_feat} features): first {first:>8.1} ms, then {rest:>8.1} ms/frame");
        }
    }
}

/// Renders `view` until its streamed layers have read what it shows: `false` when they are still
/// reading after 10 s, or a frame fails.
fn settle(s: &mut Session, view: &View) -> bool {
    for _ in 0..200 {
        match render(s.map_mut(), view) {
            Some(r) if r.data_pending => std::thread::sleep(std::time::Duration::from_millis(50)),
            Some(_) => return true,
            None => return false,
        }
    }
    false
}
