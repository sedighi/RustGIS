//! Search Open Data: datasets found by text in a CKAN catalogue (data.gov's by default), on
//! Socrata or on ArcGIS Hub, and their resources added with a click. The search goes off the frame
//! through the tile cache's downloads (a thread natively, fetch on the web); a file is added by its
//! address through `ui.addUrl`, a service opens Add From Service connected to it.

use egui::vec2;
use rustgis_engine::render::rustgis_tiles::cache;
use rustgis_engine::render::rustgis_tiles::portal::{Dataset, Portal, Use};
use serde_json::json;

use crate::{RustGisApp, Slot};

type Answer = Slot<Result<Vec<u8>, String>>;

/// The portals offered: the kind, its name, and what its address field takes.
const PORTALS: [(&str, &str, &str); 3] = [
    ("ckan", "data.gov and CKAN catalogues", "catalog.data.gov, or a CKAN site such as https://open.canada.ca/data"),
    ("socrata", "Socrata", "every Socrata site, or one such as data.cityofnewyork.us"),
    ("arcgis-hub", "ArcGIS Hub", "hub.arcgis.com, or an organisation's Hub site"),
];

/// The dialog's choices, the search under way and what it found.
pub struct State {
    pub portal: &'static str,
    pub url: String,
    pub query: String,
    /// Only datasets where the map is.
    pub here: bool,
    /// The search under way: the portal, its address and the box it keeps to.
    waiting: Option<(Portal, String, Option<[f64; 4]>, Answer)>,
    pub found: Vec<Dataset>,
    pub error: Option<String>,
    pub info: Option<String>,
}

impl Default for State {
    fn default() -> Self {
        State {
            portal: "ckan",
            url: String::new(),
            query: String::new(),
            here: false,
            waiting: None,
            found: Vec::new(),
            error: None,
            info: None,
        }
    }
}

/// Lands the search's answer.
pub fn poll(app: &mut RustGisApp, ctx: &egui::Context) {
    let Some((_, _, _, slot)) = &app.ui.opendata.waiting else { return };
    let Some(r) = slot.lock().unwrap_or_else(|e| e.into_inner()).take() else {
        ctx.request_repaint_after(std::time::Duration::from_millis(100));
        return;
    };
    let Some((portal, url, area, _)) = app.ui.opendata.waiting.take() else { return };
    let st = &mut app.ui.opendata;
    match r.and_then(|b| portal.parse(&url, &b, area)) {
        Ok(found) => {
            st.info = Some(if found.is_empty() {
                tl!("No datasets with data RustGIS can add").to_string()
            } else {
                crate::i18n::fmt("{n} datasets on {portal}", &[("n", &found.len().to_string()), ("portal", &portal.label())])
            });
            st.found = found;
        }
        Err(e) => st.error = Some(e),
    }
}

/// Fills the dialog from `ui.searchOpenData`'s parameters (`portal`, `url`, `query`, `view`) and,
/// with a query, starts the search.
pub fn start(app: &mut RustGisApp, p: &serde_json::Value) {
    let text = |k: &str| p.get(k).and_then(serde_json::Value::as_str).map(str::trim);
    let st = &mut app.ui.opendata;
    if let Some(kind) = text("portal") {
        st.portal = match kind {
            "socrata" => "socrata",
            "arcgis-hub" | "hub" | "arcgis" => "arcgis-hub",
            _ => "ckan",
        };
    }
    if let Some(u) = text("url") {
        st.url = u.to_string();
    }
    if let Some(v) = p.get("view").and_then(serde_json::Value::as_bool) {
        st.here = v;
    }
    if let Some(q) = text("query").filter(|q| !q.is_empty()) {
        st.query = q.to_string();
        if st.waiting.is_none() {
            st.error = None;
            st.info = None;
            search(app);
        }
    }
}

/// Starts a search for the dialog's choices.
fn search(app: &mut RustGisApp) {
    let st = &app.ui.opendata;
    let mut p = json!({"portal": st.portal, "query": st.query.trim(), "view": st.here});
    if !st.url.trim().is_empty() {
        p["url"] = json!(st.url.trim());
    }
    match rustgis_engine::cmd::portals::portal_query(&app.session, &p) {
        Ok((portal, query, area, limit)) => {
            let url = portal.search_url(&query, area, limit);
            let slot: Answer = Slot::default();
            let land = slot.clone();
            cache::download(&url, Box::new(move |r| *land.lock().unwrap_or_else(|e| e.into_inner()) = Some(r)));
            let st = &mut app.ui.opendata;
            st.found.clear();
            st.waiting = Some((portal, url, area, slot));
        }
        Err(e) => app.ui.opendata.error = Some(e),
    }
}

/// The dialog: a portal, a search, and the datasets found with their resources.
pub fn dialog(app: &mut RustGisApp, ui: &mut egui::Ui) {
    ui.label(egui::RichText::new(tl!("Open data catalogues searched by text. Files are added by their address (read in place when they can be); a service opens Add From Service to pick its layer.")).weak());
    let busy = app.ui.opendata.waiting.is_some();
    let mut go = false;
    egui::Grid::new("opendata").num_columns(2).spacing(vec2(10.0, 8.0)).show(ui, |ui| {
        let st = &mut app.ui.opendata;
        ui.label(tl!("Portal"));
        let shown = PORTALS.iter().find(|(k, _, _)| *k == st.portal).map(|(_, n, _)| tl!(*n).to_string()).unwrap_or_default();
        egui::ComboBox::from_id_salt("od_portal").selected_text(shown).width(300.0).show_ui(ui, |ui| {
            for (kind, name, _) in PORTALS {
                if ui.selectable_label(st.portal == kind, tl!(name)).clicked() && st.portal != kind {
                    st.portal = kind;
                    st.url.clear();
                }
            }
        });
        ui.end_row();
        ui.label(tl!("Address"));
        let hint = PORTALS.iter().find(|(k, _, _)| *k == st.portal).map(|(_, _, h)| tl!(*h)).unwrap_or_default();
        ui.add(egui::TextEdit::singleline(&mut st.url).hint_text(hint).desired_width(300.0));
        ui.end_row();
        ui.label(tl!("Search"));
        ui.horizontal(|ui| {
            let r =
                ui.add(egui::TextEdit::singleline(&mut st.query).hint_text(tl!("parks, bike lanes, flood zones…")).desired_width(200.0));
            let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            go = (ui.add_enabled(!busy, egui::Button::new(tl!("Find Datasets"))).clicked() || enter) && !busy;
        });
        ui.end_row();
        ui.label("");
        ui.checkbox(&mut st.here, tl!("Only where the map is"));
        ui.end_row();
    });
    if go {
        app.ui.opendata.error = None;
        app.ui.opendata.info = None;
        search(app);
    }
    if busy {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label(tl!("Searching…"));
        });
    }
    let t = crate::theme::Tokens::get(ui.ctx());
    if let Some(e) = &app.ui.opendata.error {
        ui.label(egui::RichText::new(e).color(t.red));
    }
    if let Some(i) = &app.ui.opendata.info {
        ui.label(egui::RichText::new(i).weak());
    }
    // A resource clicked: its address, how it is used, and the dataset's title.
    let mut picked = None;
    egui::ScrollArea::vertical().max_height(320.0).id_salt("od_found").show(ui, |ui| {
        for (i, d) in app.ui.opendata.found.iter().enumerate() {
            ui.push_id(i, |ui| {
                ui.label(egui::RichText::new(&d.title).strong());
                if !d.organization.is_empty() {
                    ui.label(egui::RichText::new(&d.organization).small());
                }
                if !d.description.is_empty() {
                    ui.label(egui::RichText::new(&d.description).small().weak());
                }
                ui.horizontal_wrapped(|ui| {
                    for r in &d.resources {
                        let label = crate::i18n::fmt("Add {format}", &[("format", &r.format)]);
                        if ui.add_enabled(!busy, egui::Button::new(label).small()).on_hover_text(&r.url).clicked() {
                            picked = Some((r.url.clone(), r.using.clone(), d.title.clone()));
                        }
                    }
                    if let Some(page) = &d.page {
                        ui.hyperlink_to(tl!("Page"), page);
                    }
                });
                ui.separator();
            });
        }
    });
    if let Some((url, using, title)) = picked {
        app.ui.opendata.error = None;
        let r = match using {
            Use::File => app.run("ui.addUrl", json!({"url": url, "name": title})).map(|_| ()),
            // A service: Add From Service, connected, to pick its layer.
            Use::Service { kind, .. } => app
                .run("ui.dialog", json!({"name": "addService"}))
                .and_then(|_| app.run("ui.addService", json!({"url": url, "type": kind})))
                .map(|_| ()),
        };
        if let Err(e) = r {
            app.ui.opendata.error = Some(e);
        }
    }
}
