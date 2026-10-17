//! View tab: view effects (illumination, weather, visual and camera effects, post-processing),
//! colour vision simulation, scene drawing mode and depth priority, view clipping, display type,
//! global and local views, navigation modes, the navigator and the camera.

use rustgis_map::view::{ColorVision, Weather};
use serde_json::{Value, json};

use super::layer::with;
use super::{b, f, need_layer, ps};
use crate::{CmdResult, CommandSpec as C, Session};

pub fn commands() -> Vec<C> {
    vec![
        C::new("map.effects", "View Effects", "View › Effects", effects).params(
            r#"{"sunTime"?: seconds since 1970|"now"|null (illumination), "weather"?: "none"|"haze"|"fog"|"rain"|"snow", "vignette"?, "edges"?, "stars"?: bool (visual effects; stars around the globe), "tiltShift"?, "grain"?: bool (camera effects), "brightness"?, "contrast"?, "saturation"?: -100..100, "sepia"?: 0..100 (post-processing), "reset"?: bool}"#,
        ),
        C::new("map.colorVision", "Color Vision Simulator", "View › Accessibility", |s, p| {
            let m = ps(p, "mode").ok_or("mode is required")?;
            let cv = ColorVision::from_code(m).ok_or_else(|| format!("unknown colour vision '{m}'"))?;
            s.map_mut().view.color_vision = cv;
            Ok(json!({"mode": cv.code()}))
        })
        .view()
        .params(r#"{"mode": "normal"|"protanopia"|"deuteranopia"|"tritanopia"|"achromatopsia"}"#),
        C::new("layer.priority", "Depth Priority", "View › Scene", |s, p| {
            let v = p.get("value").and_then(Value::as_i64).ok_or("value is required (higher draws on top)")?.clamp(-1000, 1000) as i32;
            with(s, p, |l| l.priority = v)
        })
        .when(need_layer)
        .params(r#"{"layer"?, "value": integer (higher draws on top)}"#),
        C::new("map.drawingMode", "Drawing Mode", "View › Scene", |s, p| {
            let w = match ps(p, "mode").ok_or("mode is required")? {
                "standard" => false,
                "wireframe" => true,
                m => return Err(format!("unknown drawing mode '{m}'")),
            };
            s.map_mut().view.wireframe = w;
            Ok(json!({"wireframe": w}))
        })
        .params(r#"{"mode": "standard"|"wireframe"}"#),
        C::new("map.clipping", "View Clipping", "View › View Clipping", |s, p| {
            let d = match ps(p, "mode").unwrap_or("distance") {
                "none" => None,
                "distance" => Some(f(p, "distance").filter(|d| *d > 0.0).ok_or("distance is required (metres)")?.min(4e7)),
                m => return Err(format!("unknown clipping mode '{m}'")),
            };
            s.map_mut().view.clip_distance = d;
            Ok(json!({"distance": d}))
        })
        .params(r#"{"mode": "none"|"distance", "distance"?: metres from the view's centre}"#),
        C::new("map.displayType", "Display Type", "View › Options", |s, p| {
            let fast = match ps(p, "mode").ok_or("mode is required")? {
                "quality" => false,
                "speed" => true,
                m => return Err(format!("unknown display type '{m}'")),
            };
            s.map_mut().view.fast = fast;
            Ok(json!({"mode": if fast { "speed" } else { "quality" }}))
        })
        .view()
        .params(r#"{"mode": "quality"|"speed"}"#),
        C::new("map.navigator", "Navigator", "View › Navigation", |s, p| {
            let v = &mut s.map_mut().view;
            v.navigator = b(p, "on").unwrap_or(!v.navigator);
            Ok(json!({"navigator": v.navigator}))
        })
        .view()
        .params(r#"{"on"?: bool}"#),
        C::new("map.camera", "Camera", "View › Navigation", camera).view().params(r#"{"lon"?, "lat"?, "scale"? (the camera's position when omitted)}"#),
        C::new("map.navigation", "Navigation Mode", "Home › Navigate", |s, p| {
            let fp = match ps(p, "mode").ok_or("mode is required")? {
                "planar" => false,
                "firstPerson" => true,
                m => return Err(format!("unknown navigation mode '{m}'")),
            };
            let m = s.map_mut();
            m.view.first_person = fp;
            // Planar navigation is on a flat map.
            if !fp && m.projection.is_globe() {
                m.set_projection(rustgis_map::Projection::WebMercator);
            }
            Ok(json!({"mode": if fp { "firstPerson" } else { "planar" }}))
        })
        .params(r#"{"mode": "planar"|"firstPerson"}"#),
        C::new("map.viewMode", "View", "View › View", |s, p| {
            let next = match ps(p, "mode").ok_or("mode is required")? {
                "global" => rustgis_map::Projection::Globe,
                "local" => {
                    if !rustgis_map::projection::local_views_supported() {
                        return Err("local views aren't available in the web app".to_string());
                    }
                    let c = s.map().center_lonlat();
                    rustgis_map::Projection::from_code(&format!("local:{:.4},{:.4}", c.x, c.y))?
                }
                m => return Err(format!("unknown view '{m}'")),
            };
            let m = s.map_mut();
            m.set_projection(next);
            Ok(json!({"projection": m.projection.code(), "label": m.projection.label()}))
        })
        .params(r#"{"mode": "global" (a globe)|"local" (a flat view centred here; not in the web app)}"#),
        C::new("map.walk", "Walk", "Home › Navigate", walk).view().params(r#"{"forward"?: steps, "right"?: steps, "up"?: steps (first-person navigation)}"#),
    ]
}

fn effects(s: &mut Session, p: &Value) -> CmdResult {
    // Built on a copy and applied only when every parameter is valid.
    let mut fx = s.map().view.effects.clone();
    if b(p, "reset").unwrap_or(false) {
        fx = Default::default();
    }
    if let Some(v) = p.get("sunTime") {
        fx.sun_time = match v {
            Value::Null => None,
            Value::String(t) if t == "now" => {
                Some(web_time::SystemTime::now().duration_since(web_time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0))
            }
            _ => Some(v.as_f64().filter(|t| t.is_finite()).ok_or("sunTime is seconds since 1970, \"now\" or null")?),
        };
    }
    if let Some(w) = ps(p, "weather") {
        fx.weather = match w {
            "none" => Weather::None,
            "haze" => Weather::Haze,
            "fog" => Weather::Fog,
            "rain" => Weather::Rain,
            "snow" => Weather::Snow,
            o => return Err(format!("unknown weather '{o}'")),
        };
    }
    for (k, slot) in [
        ("vignette", &mut fx.vignette),
        ("edges", &mut fx.edges),
        ("stars", &mut fx.stars),
        ("tiltShift", &mut fx.tilt_shift),
        ("grain", &mut fx.grain),
    ] {
        if let Some(v) = b(p, k) {
            *slot = v;
        }
    }
    for (k, slot, lo) in [
        ("brightness", &mut fx.brightness, -100.0),
        ("contrast", &mut fx.contrast, -100.0),
        ("saturation", &mut fx.saturation, -100.0),
        ("sepia", &mut fx.sepia, 0.0),
    ] {
        if let Some(v) = f(p, k) {
            *slot = v.clamp(lo, 100.0);
        }
    }
    let out = serde_json::to_value(&fx).map_err(|e| e.to_string())?;
    s.map_mut().view.effects = fx;
    Ok(out)
}

fn camera(s: &mut Session, p: &Value) -> CmdResult {
    let (lon, lat, scale) = (f(p, "lon"), f(p, "lat"), f(p, "scale"));
    if lon.is_some() || lat.is_some() || scale.is_some() {
        let c = s.map().center_lonlat();
        let mut q = json!({"lon": lon.unwrap_or(c.x), "lat": lat.unwrap_or(c.y)});
        if let Some(sc) = scale {
            q["scale"] = json!(sc);
        }
        super::navigate::go_to_xy(s, &q)?;
    }
    let c = s.map().center_lonlat();
    Ok(json!({"lon": c.x, "lat": c.y, "scale": s.scale(), "projection": s.map().projection.label()}))
}

/// One first-person step: a tenth of the view, forward (north), right (east) or up (zooming out).
fn walk(s: &mut Session, p: &Value) -> CmdResult {
    let (fw, rt, up) = (
        f(p, "forward").unwrap_or(0.0).clamp(-100.0, 100.0),
        f(p, "right").unwrap_or(0.0).clamp(-100.0, 100.0),
        f(p, "up").unwrap_or(0.0).clamp(-20.0, 20.0),
    );
    let (w, h) = s.view_size;
    let m = s.map_mut();
    let step = w.min(h) * 0.1;
    if m.projection.is_globe() {
        rustgis_map::Globe::drag(&mut m.camera, -rt * step, fw * step, w, h);
    } else {
        m.camera.pan_px(-rt * step, fw * step);
    }
    if up != 0.0 {
        m.camera.zoom_at(0.8f64.powf(up), w / 2.0, h / 2.0, w, h);
    }
    let c = m.center_lonlat();
    Ok(json!({"lon": c.x, "lat": c.y}))
}
