//! The Data Sources pane: favourites, recent sources, folders, saved services, databases and STAC
//! APIs, S3 connections and a library of well-known public sources. A source is added (or its
//! browser dialog opened, connected) through the commands that add it; the lists are the
//! session's `sources`, kept with the app's preferences (never a secret).

use egui::{RichText, Sense, Ui, vec2};
use rustgis_engine::render::rustgis_tiles::s3;
use rustgis_engine::sources::{Source, library};
use serde_json::{Value, json};

use crate::theme::{Tokens, semibold};
use crate::{RustGisApp, icons};

/// The preference the sources are kept under.
pub const PREF: &str = "data_sources";

/// What the pane's "save" boxes hold.
#[derive(Default)]
pub struct State {
    loaded: bool,
    service: String,
    database: String,
    stac: String,
    /// The sources as last shown, and their generation.
    shown: Option<(u64, std::sync::Arc<rustgis_engine::sources::Sources>)>,
}

/// The sources to show this frame: the last snapshot unless they changed since.
fn snapshot(app: &mut RustGisApp) -> std::sync::Arc<rustgis_engine::sources::Sources> {
    let generation = app.session.sources.generation;
    match &app.ui.sources.shown {
        Some((g, s)) if *g == generation => s.clone(),
        _ => {
            let s = std::sync::Arc::new(app.session.sources.clone());
            app.ui.sources.shown = Some((generation, s.clone()));
            s
        }
    }
}

/// Reads the sources kept with the preferences, once.
pub fn load(app: &mut RustGisApp) {
    if app.ui.sources.loaded {
        return;
    }
    app.ui.sources.loaded = true;
    if let Some(text) = app.services.load_pref.as_ref().and_then(|load| load(PREF)) {
        // What this session already has (added, saved or made favourite before the first read)
        // stays; the stored lists are added after it.
        app.session.sources.merge(rustgis_engine::sources::Sources::from_json(&text));
    }
    app.ui.sources_saved = app.session.sources.generation;
}

/// Keeps the sources with the preferences when they changed.
pub fn save(app: &mut RustGisApp) {
    if !app.ui.sources.loaded {
        load(app);
    }
    if app.session.sources.generation == app.ui.sources_saved {
        return;
    }
    app.ui.sources_saved = app.session.sources.generation;
    if let Some(save) = &app.services.save_pref {
        save(PREF, &app.session.sources.to_json());
    }
}

fn icon_of(kind: &str) -> &'static str {
    match kind {
        "file" => "add_data",
        "url" => "web_map",
        "tiles" => "basemap",
        "service" => "globe_view",
        "database" => "table",
        "zarr" => "raster_layer",
        "stac" => "search",
        "sample" => "map",
        "s3" => "package",
        _ => "add_data",
    }
}

fn text<'a>(v: &'a Value, k: &str) -> Option<&'a str> {
    v.get(k).and_then(Value::as_str).filter(|s| !s.is_empty())
}

/// Adds a source, or opens the dialog that browses it (a service's layers, a database's tables,
/// a STAC API's collections), connected.
pub fn open(app: &mut RustGisApp, s: &Source) -> Result<(), String> {
    let x = &s.extra;
    match s.kind.as_str() {
        "file" => {
            let mut p = json!({"path": s.address});
            if let Some(l) = text(x, "layer") {
                p["layer"] = json!(l);
            }
            app.run("layer.addData", p).map(|_| ())
        }
        "url" => {
            let mut p = json!({"url": s.address, "name": s.name});
            if let Some(l) = text(x, "layer") {
                p["layer"] = json!(l);
            }
            app.run("ui.addUrl", p).map(|_| ())
        }
        "tiles" => {
            let mut p = json!({"url": s.address, "name": s.name});
            for k in ["kind", "attribution"] {
                if let Some(v) = text(x, k) {
                    p[k] = json!(v);
                }
            }
            if let Some(z) = text(x, "maxZoom").and_then(|z| z.parse::<u32>().ok()) {
                p["maxZoom"] = json!(z);
            }
            app.run("map.addTiles", p).map(|_| ())
        }
        "service" => {
            app.run("ui.dialog", json!({"name": "addService"}))?;
            app.run("ui.addService", json!({"url": s.address, "type": text(x, "type").unwrap_or("auto")})).map(|_| ())
        }
        "database" => {
            app.run("ui.dialog", json!({"name": "addDatabase"}))?;
            app.dialogs.a = s.address.clone();
            if let Some(l) = text(x, "layer") {
                app.dialogs.c = l.to_string();
            }
            Ok(())
        }
        "zarr" => app.run("ui.addZarr", json!({"address": s.address})).map(|_| ()),
        "stac" if text(x, "api").is_some() => {
            app.ui.stac.api = s.address.clone();
            app.ui.stac.collections.clear();
            app.ui.stac.items.clear();
            app.run("ui.dialog", json!({"name": "stac"})).map(|_| ())
        }
        "stac" => app.run("ui.stacLink", json!({"url": s.address})).map(|_| ()),
        "sample" => app.run("layer.addSample", json!({"name": s.address})).map(|_| ()),
        "s3" => {
            crate::s3ui::open_connection(app, &s.address);
            app.run("ui.dialog", json!({"name": "s3"})).map(|_| ())
        }
        other => Err(format!("unknown source kind '{other}'")),
    }
}

/// One source's row: its icon and name (a click adds or opens it), and a menu.
fn row(app: &mut RustGisApp, ui: &mut Ui, t: &Tokens, s: &Source, list: Option<&str>) {
    ui.horizontal(|ui| {
        let (r, _) = ui.allocate_exact_size(vec2(16.0, 16.0), Sense::hover());
        icons::paint(ui.painter(), r, icon_of(&s.kind), t.icon, t.accent);
        let resp = ui.link(&s.name).on_hover_text(&s.address);
        if resp.clicked()
            && let Err(e) = open(app, s)
        {
            app.session.notify(e, true);
        }
        resp.context_menu(|ui| {
            let fav = app.session.sources.favourites.iter().any(|f| f.is(&s.kind, &s.address));
            let p = json!({"kind": s.kind, "address": s.address, "name": s.name, "extra": s.extra});
            if !fav && ui.button(tl!("Add to Favourites")).clicked() {
                let _ = app.run("sources.favourite", p.clone());
                ui.close();
            }
            if fav && ui.button(tl!("Remove from Favourites")).clicked() {
                let mut q = p.clone();
                q["on"] = json!(false);
                let _ = app.run("sources.favourite", q);
                ui.close();
            }
            if let Some(list) = list.filter(|l| *l != "favourites")
                && ui.button(tl!("Remove")).clicked()
            {
                let _ = app.run("sources.remove", json!({"list": list, "kind": s.kind, "address": s.address}));
                ui.close();
            }
            if ui.button(tl!("Copy Address")).clicked() {
                ui.ctx().copy_text(s.address.clone());
                ui.close();
            }
        });
    });
}

fn header(ui: &mut Ui, title: &str, open: bool, body: impl FnOnce(&mut Ui)) {
    egui::CollapsingHeader::new(RichText::new(title).font(semibold(12.5))).id_salt(("sources", title)).default_open(open).show(ui, body);
}

fn empty(ui: &mut Ui, what: &str) {
    ui.label(RichText::new(what).small().weak());
}

/// A box to save an address as a source of `kind`.
fn save_box(app: &mut RustGisApp, ui: &mut Ui, kind: &str, hint: &str, field: fn(&mut State) -> &mut String) {
    ui.horizontal(|ui| {
        let r = ui.add(egui::TextEdit::singleline(field(&mut app.ui.sources)).hint_text(hint).desired_width(ui.available_width() - 52.0));
        let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        if (ui.small_button(tl!("Save")).clicked() || enter) && !field(&mut app.ui.sources).trim().is_empty() {
            let mut address = field(&mut app.ui.sources).trim().to_string();
            // A bare connection string is a PostgreSQL one (as Add From Database takes it).
            if kind == "database"
                && address.contains('=')
                && !address.contains("://")
                && !address.to_ascii_lowercase().starts_with("postgres:")
            {
                address = format!("postgres:{address}");
            }
            let mut p = json!({"kind": kind, "address": address});
            if kind == "stac" {
                p["extra"] = json!({"api": "true"});
            }
            match app.run("sources.save", p) {
                Ok(_) => field(&mut app.ui.sources).clear(),
                Err(e) => app.session.notify(e, true),
            }
        }
    });
}

pub fn show(app: &mut RustGisApp, ui: &mut Ui, t: &Tokens) {
    load(app);
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        egui::Frame::NONE.inner_margin(egui::Margin::symmetric(10, 8)).show(ui, |ui| {
            ui.label(
                RichText::new(tl!("Click a source to add it, or to open its browser connected. Right-click for favourites."))
                    .small()
                    .weak(),
            );
            // A snapshot shared across frames, taken again only when the sources change.
            let src = snapshot(app);
            header(ui, tl!("Favourites"), true, |ui| {
                if src.favourites.is_empty() {
                    empty(ui, tl!("Right-click a source to add it here"));
                }
                for s in &src.favourites {
                    row(app, ui, t, s, Some("favourites"));
                }
            });
            header(ui, tl!("Recent"), true, |ui| {
                if src.recent.is_empty() {
                    empty(ui, tl!("Data you add is listed here"));
                }
                for s in &src.recent {
                    row(app, ui, t, s, Some("recent"));
                }
            });
            // A web page can't reach local folders.
            if !cfg!(target_arch = "wasm32") {
                header(ui, tl!("Folders"), false, |ui| {
                    let folders = app.session.project.folders.clone();
                    for f in folders {
                        ui.horizontal(|ui| {
                            let (r, _) = ui.allocate_exact_size(vec2(16.0, 16.0), Sense::hover());
                            icons::paint(ui.painter(), r, "add_folder", t.icon, t.accent);
                            if ui.link(&f).on_hover_text(tl!("Browse it in the Browser")).clicked() {
                                app.table.folder = Some(std::path::PathBuf::from(&f));
                                app.ui.left = Some(crate::LeftPane::Catalog);
                            }
                        });
                    }
                    if ui.small_button(tl!("Add Folder…")).clicked() {
                        let _ = app.run("ui.addFolder", Value::Null);
                    }
                });
            }
            let saved = |kind: &str| src.saved.iter().filter(|s| s.kind == kind).cloned().collect::<Vec<_>>();
            header(ui, tl!("Services"), false, |ui| {
                for s in saved("service") {
                    row(app, ui, t, &s, Some("saved"));
                }
                save_box(app, ui, "service", "https://…/wms, …/FeatureServer", |st| &mut st.service);
            });
            if !cfg!(target_arch = "wasm32") {
                header(ui, tl!("Databases"), false, |ui| {
                    for s in saved("database") {
                        row(app, ui, t, &s, Some("saved"));
                    }
                    save_box(app, ui, "database", "host=… dbname=… user=…, or a .duckdb file", |st| &mut st.database);
                });
            }
            header(ui, tl!("S3"), false, |ui| {
                let conns = s3::connections();
                if conns.is_empty() {
                    empty(ui, tl!("No connections this session"));
                }
                for c in conns {
                    let s = Source { kind: "s3".into(), name: c.name.clone(), address: c.name.clone(), extra: Value::Null };
                    row(app, ui, t, &s, None);
                }
                if ui.small_button(tl!("Connect…")).clicked() {
                    let _ = app.run("ui.dialog", json!({"name": "s3"}));
                }
            });
            header(ui, tl!("STAC APIs"), false, |ui| {
                for (_, s) in library().iter().filter(|(_, s)| s.kind == "stac") {
                    row(app, ui, t, s, None);
                }
                for s in src.saved.iter().filter(|s| s.kind == "stac") {
                    row(app, ui, t, s, Some("saved"));
                }
                save_box(app, ui, "stac", "https://…/stac/v1", |st| &mut st.stac);
            });
            header(ui, tl!("Library"), false, |ui| {
                let mut groups: Vec<&str> = Vec::new();
                let lib = library();
                for (g, _) in lib {
                    if !groups.contains(g) {
                        groups.push(g);
                    }
                }
                for g in groups {
                    egui::CollapsingHeader::new(tl!(g)).id_salt(("library", g)).default_open(false).show(ui, |ui| {
                        for (_, s) in lib.iter().filter(|(x, _)| x == &g) {
                            row(app, ui, t, s, None);
                        }
                    });
                }
            });
        });
    });
}
