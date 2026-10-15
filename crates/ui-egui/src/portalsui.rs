//! Open-data portals: OpenStreetMap data for the map's area (Overpass) and a country's
//! administrative boundaries (geoBoundaries). Requests go off the frame through the tile cache's
//! downloads (a thread natively, fetch on the web); the answers are handed to `osm.download` and
//! `boundaries.add`, so the layers are added like any command's.

use egui::vec2;
use rustgis_engine::render::rustgis_tiles::{boundaries, cache, overpass};
use serde_json::{Value, json};

use crate::{RustGisApp, Slot};

type Answer = Slot<Result<Vec<u8>, String>>;

/// What is being fetched, with the parameters its answer is added with.
enum Asked {
    Osm(Value),
    Countries,
    /// A country's level: its description, then its GeoJSON.
    Meta(Value),
    Boundary(Value),
}

/// The dialogs' choices and the request under way.
pub struct State {
    pub preset: String,
    pub tags: String,
    pub server: String,
    /// geoBoundaries' countries: code and name.
    pub countries: Vec<(String, String)>,
    /// The country list was asked for (once a session: an empty or failed answer leaves the code
    /// to be typed, with `countries_error` saying why).
    countries_asked: bool,
    pub countries_error: Option<String>,
    pub country: String,
    pub level: String,
    waiting: Option<(Asked, Answer)>,
    pub error: Option<String>,
    pub info: Option<String>,
}

impl Default for State {
    fn default() -> Self {
        State {
            preset: "buildings".into(),
            tags: String::new(),
            server: String::new(),
            countries: Vec::new(),
            countries_asked: false,
            countries_error: None,
            country: String::new(),
            level: "ADM1".into(),
            waiting: None,
            error: None,
            info: None,
        }
    }
}

fn fetch(url: &str) -> Answer {
    let slot: Answer = Slot::default();
    let land = slot.clone();
    cache::download(url, Box::new(move |r| *land.lock().unwrap_or_else(|e| e.into_inner()) = Some(r)));
    slot
}

/// Lands the answers.
pub fn poll(app: &mut RustGisApp, ctx: &egui::Context) {
    let Some((_, slot)) = &app.ui.portals.waiting else { return };
    let Some(r) = slot.lock().unwrap_or_else(|e| e.into_inner()).take() else {
        ctx.request_repaint_after(std::time::Duration::from_millis(100));
        return;
    };
    let Some((asked, _)) = app.ui.portals.waiting.take() else { return };
    app.ui.portals.error = None;
    app.ui.portals.info = None;
    let text = |b: Vec<u8>| String::from_utf8(b).map_err(|_| "the answer isn't UTF-8 text".to_string());
    let done = match asked {
        Asked::Osm(mut p) => r.and_then(text).and_then(|t| {
            p["answer"] = json!(t);
            app.run("osm.download", p).map(|v| {
                let n = |k: &str| v["features"][k].as_u64().unwrap_or(0);
                app.ui.portals.info = Some(crate::i18n::fmt(
                    "Added {p} points, {l} lines and {g} polygons",
                    &[("p", &n("points").to_string()), ("l", &n("lines").to_string()), ("g", &n("polygons").to_string())],
                ));
            })
        }),
        // The list's own outcome, kept apart from the dialog's actions (a failure leaves the code to
        // be typed, and isn't asked again).
        Asked::Countries => {
            app.ui.portals.countries_error = match r.and_then(|b| boundaries::parse_countries(&b)) {
                Ok(c) if c.is_empty() => Some(tl!("geoBoundaries listed no countries: type a country's code").to_string()),
                Ok(c) => {
                    app.ui.portals.countries = c;
                    None
                }
                Err(e) => Some(crate::i18n::fmt("The country list couldn't be read ({e}): type a country's code", &[("e", &e)])),
            };
            Ok(())
        }
        Asked::Meta(mut p) => r.and_then(|b| boundaries::parse_meta(&b)).map(|m| {
            p["name"] = json!(format!("{} {}", m.name, m.level));
            app.ui.portals.waiting = Some((Asked::Boundary(p), fetch(&m.geojson)));
        }),
        Asked::Boundary(mut p) => r.and_then(text).and_then(|t| {
            p["text"] = json!(t);
            app.run("boundaries.add", p).map(|_| ())
        }),
    };
    match done {
        Ok(()) => app.map.invalidate(),
        Err(e) => app.ui.portals.error = Some(e),
    }
}

fn status(app: &RustGisApp, ui: &mut egui::Ui) {
    if app.ui.portals.waiting.is_some() {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label(tl!("Downloading…"));
        });
    }
    let t = crate::theme::Tokens::get(ui.ctx());
    if let Some(e) = &app.ui.portals.error {
        ui.label(egui::RichText::new(e).color(t.red));
    }
    if let Some(i) = &app.ui.portals.info {
        ui.label(egui::RichText::new(i).weak());
    }
}

/// OpenStreetMap: a preset or tag filters, downloaded for the map's extent.
pub fn osm(app: &mut RustGisApp, ui: &mut egui::Ui) {
    ui.label(egui::RichText::new(tl!("OpenStreetMap data where the map is, from an Overpass server: a preset, or tag filters such as amenity=cafe, shop. The area can be up to about 50 × 50 km. © OpenStreetMap contributors (ODbL).")).weak());
    let busy = app.ui.portals.waiting.is_some();
    egui::Grid::new("osm").num_columns(2).spacing(vec2(10.0, 8.0)).show(ui, |ui| {
        let st = &mut app.ui.portals;
        ui.label(tl!("Preset"));
        let shown = overpass::PRESETS
            .iter()
            .find(|(id, _, _)| *id == st.preset)
            .map(|(_, l, _)| tl!(*l).to_string())
            .unwrap_or_else(|| tl!("None").to_string());
        egui::ComboBox::from_id_salt("osm_preset").selected_text(shown).width(260.0).show_ui(ui, |ui| {
            ui.selectable_value(&mut st.preset, String::new(), tl!("None"));
            for (id, label, _) in overpass::PRESETS {
                ui.selectable_value(&mut st.preset, (*id).to_string(), tl!(*label));
            }
        });
        ui.end_row();
        ui.label(tl!("Tags"));
        ui.add(egui::TextEdit::singleline(&mut st.tags).hint_text("amenity=cafe, shop").desired_width(260.0));
        ui.end_row();
        ui.label(tl!("Server"));
        ui.add(egui::TextEdit::singleline(&mut st.server).hint_text(overpass::OVERPASS).desired_width(260.0));
        ui.end_row();
    });
    if ui.add_enabled(!busy, egui::Button::new(tl!("Download"))).clicked() {
        let st = &app.ui.portals;
        let mut p = json!({"preset": st.preset, "tags": st.tags});
        if !st.server.trim().is_empty() {
            p["url"] = json!(st.server.trim());
        }
        let url = rustgis_engine::cmd::portals::osm_query(&app.session, &p).and_then(|q| {
            // The box the request was for, so the answer is read for the same one.
            p["bbox"] = json!(q.bbox);
            q.url(Some(app.ui.portals.server.as_str()))
        });
        match url {
            Ok(url) => {
                app.ui.portals.error = None;
                app.ui.portals.waiting = Some((Asked::Osm(p), fetch(&url)));
            }
            Err(e) => app.ui.portals.error = Some(e),
        }
    }
    status(app, ui);
}

/// Boundaries: a country's administrative areas at a level, from geoBoundaries.
pub fn boundaries(app: &mut RustGisApp, ui: &mut egui::Ui) {
    ui.label(egui::RichText::new(tl!("A country's administrative boundaries from geoBoundaries (CC BY 4.0): the country itself (ADM0), its first-level divisions (ADM1, such as states) and finer ones.")).weak());
    let busy = app.ui.portals.waiting.is_some();
    // The country list, asked for once.
    if !app.ui.portals.countries_asked && !busy {
        app.ui.portals.countries_asked = true;
        app.ui.portals.waiting = Some((Asked::Countries, fetch(&boundaries::countries_url())));
    }
    if let Some(e) = &app.ui.portals.countries_error {
        ui.label(egui::RichText::new(e).small().weak());
    }
    egui::Grid::new("boundaries").num_columns(2).spacing(vec2(10.0, 8.0)).show(ui, |ui| {
        let st = &mut app.ui.portals;
        ui.label(tl!("Country"));
        if st.countries.is_empty() {
            ui.add(egui::TextEdit::singleline(&mut st.country).hint_text("CHE").desired_width(260.0));
        } else {
            let shown = st
                .countries
                .iter()
                .find(|(iso, _)| *iso == st.country)
                .map(|(iso, n)| format!("{n} ({iso})"))
                .unwrap_or_else(|| tl!("Choose a country").to_string());
            egui::ComboBox::from_id_salt("gb_country").selected_text(shown).width(260.0).height(360.0).show_ui(ui, |ui| {
                for (iso, name) in &st.countries {
                    ui.selectable_value(&mut st.country, iso.clone(), format!("{name} ({iso})"));
                }
            });
        }
        ui.end_row();
        ui.label(tl!("Level"));
        egui::ComboBox::from_id_salt("gb_level").selected_text(st.level.clone()).width(260.0).show_ui(ui, |ui| {
            for (l, label) in [
                ("ADM0", "ADM0 — the country"),
                ("ADM1", "ADM1 — states, provinces"),
                ("ADM2", "ADM2 — districts, counties"),
                ("ADM3", "ADM3"),
                ("ADM4", "ADM4"),
                ("ADM5", "ADM5"),
            ] {
                ui.selectable_value(&mut st.level, l.to_string(), tl!(label));
            }
        });
        ui.end_row();
    });
    if ui.add_enabled(!busy, egui::Button::new(tl!("Add"))).clicked() {
        let st = &app.ui.portals;
        match boundaries::country(&st.country).and_then(|c| Ok((c, boundaries::level(&st.level)?))) {
            Ok((c, l)) => {
                let url = boundaries::meta_url(&c, &l);
                app.ui.portals.error = None;
                app.ui.portals.waiting = Some((Asked::Meta(json!({"country": c, "level": l})), fetch(&url)));
            }
            Err(e) => app.ui.portals.error = Some(e),
        }
    }
    status(app, ui);
}
