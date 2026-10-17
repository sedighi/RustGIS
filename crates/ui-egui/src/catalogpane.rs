//! ArcGIS Pro-style Catalog pane: displays database connections, Feature Datasets,
//! Feature Classes, Standalone Tables, Maps, Toolboxes, and Layouts.
//! Supports multi-selection, dataset bulk adding, and drag-and-drop to Map View & Layers pane.

use std::collections::{BTreeMap, HashMap, HashSet};

use egui::{Align2, Color32, CornerRadius, Rect, Sense, Stroke, Ui, pos2, vec2};
use serde_json::{Value, json};

use crate::theme::{Tokens, regular, semibold};
use crate::{RustGisApp, icons};

/// A table or feature class item in the Catalog pane.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CatalogItem {
    pub schema: String,
    pub name: String,
    pub label: String,
    pub geom_col: Option<String>,
    pub geom_type: Option<String>,
    pub is_sde: bool,
    pub feature_dataset: Option<String>,
    pub srid: Option<i32>,
    pub crs: Option<String>,
}

impl CatalogItem {
    /// Determines the appropriate icon based on geometry type.
    pub fn geometry_icon(&self) -> &'static str {
        let ty = self.geom_type.as_deref().unwrap_or("").to_ascii_lowercase();
        if ty.contains("point") {
            "point_layer"
        } else if ty.contains("line") || ty.contains("string") || ty.contains("curve") {
            "line_layer"
        } else if ty.contains("poly") || ty.contains("area") || ty.contains("surface") {
            "polygon_layer"
        } else if self.geom_col.is_none() && ty.is_empty() {
            "table"
        } else {
            "polygon_layer"
        }
    }
}

/// Payload carried during drag-and-drop from the Catalog pane.
#[derive(Clone, Debug)]
pub struct DraggedCatalogLayers {
    pub conn_params: Value,
    pub layers: Vec<CatalogItem>,
}

/// Cached introspection data for a database connection.
#[derive(Clone, Debug, Default)]
pub struct DatabaseCache {
    pub loading: bool,
    pub error: Option<String>,
    pub feature_datasets: BTreeMap<String, Vec<CatalogItem>>,
    pub standalone_tables: Vec<CatalogItem>,
    pub total_tables: usize,
}

/// State for the Catalog pane.
#[derive(Clone, Debug, Default)]
pub struct CatalogState {
    pub search: String,
    pub selected: HashSet<String>,
    pub expanded_databases: HashSet<String>,
    pub expanded_datasets: HashSet<String>,
    pub cache: HashMap<String, DatabaseCache>,
    pub passwords: HashMap<String, String>,
    pub status: Option<(bool, String)>,
}

/// A database connection entry in the Catalog pane.
#[derive(Clone, Debug, PartialEq)]
pub struct CatalogDatabaseConn {
    pub id: String,
    pub label: String,
    pub params: Value,
    pub is_duckdb: bool,
}

/// Adds a set of catalog layers to the active map.
pub fn add_catalog_layers_to_map(app: &mut RustGisApp, conn_params: &Value, layers: &[CatalogItem]) {
    if layers.is_empty() {
        return;
    }
    let mut added_count = 0;
    for l in layers {
        let mut p = conn_params.clone();
        p["layer"] = json!(l.label);
        if let Some(ref c) = l.crs {
            p["crs"] = json!(c);
        }
        match app.run("layer.addDatabase", p) {
            Ok(_) => added_count += 1,
            Err(e) => {
                app.session.notify(format!("{}: {e}", l.label), true);
            }
        }
    }
    if added_count > 0 {
        app.map.invalidate();
        app.session.notify(crate::i18n::fmt("Added {n} layer(s) to map", &[("n", &added_count.to_string())]), false);
    }
}

/// Displays the ArcGIS Pro-style Catalog pane on the right side.
pub fn show(app: &mut RustGisApp, ui: &mut Ui, t: &Tokens) {
    let connections = gather_connections(app);

    // 1. Search / Filter bar at the top
    ui.horizontal(|ui| {
        let (icon_r, _) = ui.allocate_exact_size(vec2(16.0, 16.0), Sense::hover());
        icons::paint(ui.painter(), icon_r, "search", t.icon, t.accent);
        let resp = ui.add(
            egui::TextEdit::singleline(&mut app.catalog.search)
                .hint_text(tl!("Search Catalog…"))
                .desired_width(ui.available_width() - 26.0),
        );
        if !app.catalog.search.is_empty() && ui.small_button("×").on_hover_text(tl!("Clear search")).clicked() {
            app.catalog.search.clear();
        }
        let _ = resp;
    });

    ui.add_space(4.0);

    // 2. Action toolbar
    ui.horizontal(|ui| {
        if ui.button(tl!("+ Database")).on_hover_text(tl!("New Database Connection")).clicked() {
            let _ = app.run("ui.dialog", json!({"name": "addDatabase"}));
        }

        if ui.button(tl!("Refresh")).on_hover_text(tl!("Refresh database contents")).clicked() {
            app.catalog.cache.clear();
        }

        let sel_count = app.catalog.selected.len();
        if sel_count > 0 {
            let label = crate::i18n::fmt("Add to Map ({n})", &[("n", &sel_count.to_string())]);
            let add_btn = crate::widgets::dialog_button(ui, &label, Some(t.accent), vec2(100.0, 22.0));
            if add_btn.clicked() {
                add_selected_layers(app, &connections);
            }
        }
    });

    ui.separator();

    // 3. Status message if any
    if let Some((is_err, ref msg)) = app.catalog.status {
        let col = if is_err { egui::Color32::from_rgb(220, 50, 50) } else { egui::Color32::from_rgb(40, 160, 40) };
        ui.label(egui::RichText::new(msg).color(col).small());
    }

    // 4. Tree View
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        // --- Section: Databases ---
        show_databases_section(app, ui, t, &connections);

        ui.add_space(6.0);
        ui.separator();

        // --- Section: Project Maps ---
        show_maps_section(app, ui, t);

        // --- Section: Toolboxes ---
        show_toolboxes_section(app, ui, t);

        // --- Section: Layouts ---
        show_layouts_section(app, ui, t);
    });
}

fn gather_connections(app: &RustGisApp) -> Vec<CatalogDatabaseConn> {
    let mut out = Vec::new();

    // Check active connection in app.dialogs.db
    let db = &app.dialogs.db;
    if db.conn_type == "postgres" && !db.host.trim().is_empty() {
        let host = db.host.trim();
        let port = if db.port.trim().is_empty() { "5432" } else { db.port.trim() };
        let user = if db.user.trim().is_empty() { "postgres" } else { db.user.trim() };
        let dbname = if db.database.trim().is_empty() { "postgres" } else { db.database.trim() };
        let id = format!("pg:{host}:{port}/{dbname}");
        let label = format!("{dbname} ({host}:{port})");
        let mut p = json!({
            "host": host,
            "port": port,
            "user": user,
            "dbname": dbname,
            "password": db.password,
        });
        if !db.schema.trim().is_empty() {
            p["schema"] = json!(db.schema.trim());
        }
        out.push(CatalogDatabaseConn { id, label, params: p, is_duckdb: false });
    } else if db.conn_type == "duckdb" && !db.duckdb_path.trim().is_empty() {
        let p = db.duckdb_path.trim();
        let name = std::path::Path::new(p).file_name().and_then(|n| n.to_str()).unwrap_or(p);
        out.push(CatalogDatabaseConn {
            id: format!("duckdb:{p}"),
            label: name.to_string(),
            params: json!({"database": p}),
            is_duckdb: true,
        });
    }

    // Check saved sources in session
    for s in &app.session.sources.saved {
        if s.kind == "database" && !s.address.trim().is_empty() {
            let id = format!("saved:{}", s.address);
            if !out.iter().any(|c| c.id == id) {
                out.push(CatalogDatabaseConn {
                    id,
                    label: if s.name.is_empty() { s.address.clone() } else { s.name.clone() },
                    params: json!({"database": s.address}),
                    is_duckdb: s.address.ends_with(".duckdb") || s.address.ends_with(".ddb"),
                });
            }
        }
    }

    out
}

fn show_databases_section(app: &mut RustGisApp, ui: &mut Ui, t: &Tokens, connections: &[CatalogDatabaseConn]) {
    let db_header = egui::CollapsingHeader::new(egui::RichText::new(tl!("Databases")).font(semibold(13.0)))
        .id_salt("cat_databases_header")
        .default_open(true);

    db_header.show(ui, |ui| {
        if connections.is_empty() {
            ui.vertical_centered(|ui| {
                ui.add_space(4.0);
                ui.label(egui::RichText::new(tl!("No databases connected.")).weak().small());
                if ui.button(tl!("+ Add Connection")).clicked() {
                    let _ = app.run("ui.dialog", json!({"name": "addDatabase"}));
                }
            });
            return;
        }

        for conn in connections {
            show_single_database(app, ui, t, conn);
        }
    });
}

fn show_single_database(app: &mut RustGisApp, ui: &mut Ui, t: &Tokens, conn: &CatalogDatabaseConn) {
    let is_open = app.catalog.expanded_databases.contains(&conn.id);
    let mut toggle_open = false;
    let mut do_edit = false;
    let mut do_refresh = false;
    let mut do_add_all = false;
    let mut do_remove = false;

    // Database Connection Header Row
    ui.horizontal(|ui| {
        let (chevron_r, chevron_resp) = ui.allocate_exact_size(vec2(14.0, 16.0), Sense::click());
        icons::paint(ui.painter(), chevron_r, if is_open { "chevron_down" } else { "chevron_right" }, t.text_dim, t.accent);
        if chevron_resp.clicked() {
            toggle_open = true;
        }

        let (icon_r, _) = ui.allocate_exact_size(vec2(16.0, 16.0), Sense::hover());
        icons::paint(ui.painter(), icon_r, "database", t.icon, t.accent);

        let resp = ui.selectable_label(is_open, &conn.label);
        if resp.clicked() {
            toggle_open = true;
        }

        resp.context_menu(|ui| {
            if ui.button(tl!("Edit Connection…")).clicked() {
                do_edit = true;
                ui.close();
            }
            if ui.button(tl!("Refresh")).clicked() {
                do_refresh = true;
                ui.close();
            }
            if ui.button(tl!("Add All Layers to Map")).clicked() {
                do_add_all = true;
                ui.close();
            }
            if ui.button(tl!("Remove Connection")).clicked() {
                do_remove = true;
                ui.close();
            }
        });
    });

    if do_edit {
        edit_database_connection(app, conn);
    }

    if do_remove {
        remove_database_connection(app, &conn.id, &conn.params);
        return;
    }

    if toggle_open {
        if is_open {
            app.catalog.expanded_databases.remove(&conn.id);
        } else {
            app.catalog.expanded_databases.insert(conn.id.clone());
        }
    }

    if do_refresh {
        app.catalog.cache.remove(&conn.id);
    }

    let is_now_open = app.catalog.expanded_databases.contains(&conn.id);
    if !is_now_open {
        return;
    }

    // Ensure cache entry exists
    if !app.catalog.cache.contains_key(&conn.id) {
        load_database_cache(app, conn);
    }

    let cache = match app.catalog.cache.get(&conn.id) {
        Some(c) => c.clone(),
        None => return,
    };

    if do_add_all {
        let mut all_items = Vec::new();
        for items in cache.feature_datasets.values() {
            all_items.extend(items.clone());
        }
        all_items.extend(cache.standalone_tables.clone());
        add_catalog_layers_to_map(app, &conn.params, &all_items);
    }

    if let Some(ref err) = cache.error {
        ui.indent(conn.id.as_str(), |ui| {
            ui.label(egui::RichText::new(err).color(egui::Color32::from_rgb(220, 50, 50)).small());
            if ui.small_button(tl!("Retry")).clicked() {
                app.catalog.cache.remove(&conn.id);
            }
        });
        return;
    }

    let search_lower = app.catalog.search.trim().to_ascii_lowercase();

    ui.indent(conn.id.as_str(), |ui| {
        // 1. Feature Datasets
        for (dataset_name, items) in &cache.feature_datasets {
            let matching_items: Vec<CatalogItem> = if search_lower.is_empty() {
                items.clone()
            } else {
                items
                    .iter()
                    .filter(|it| {
                        it.name.to_ascii_lowercase().contains(&search_lower) || dataset_name.to_ascii_lowercase().contains(&search_lower)
                    })
                    .cloned()
                    .collect()
            };

            if !search_lower.is_empty() && matching_items.is_empty() {
                continue;
            }

            show_feature_dataset(app, ui, t, conn, dataset_name, &matching_items);
        }

        // 2. Standalone Feature Classes
        let matching_standalone: Vec<CatalogItem> = if search_lower.is_empty() {
            cache.standalone_tables.clone()
        } else {
            cache.standalone_tables.iter().filter(|it| it.name.to_ascii_lowercase().contains(&search_lower)).cloned().collect()
        };

        if !matching_standalone.is_empty() {
            if !cache.feature_datasets.is_empty() {
                ui.add_space(2.0);
                ui.label(egui::RichText::new(tl!("Standalone Feature Classes")).font(semibold(11.5)).weak());
            }
            for item in &matching_standalone {
                show_feature_class_row(app, ui, t, conn, item);
            }
        }

        if cache.feature_datasets.is_empty() && cache.standalone_tables.is_empty() {
            ui.label(egui::RichText::new(tl!("No feature classes found with geometry.")).weak().small());
        }
    });
}

fn load_database_cache(app: &mut RustGisApp, conn: &CatalogDatabaseConn) {
    let res = app.run("info.databaseTables", conn.params.clone());
    match res {
        Ok(v) => {
            let mut datasets: BTreeMap<String, Vec<CatalogItem>> = BTreeMap::new();
            let mut standalone: Vec<CatalogItem> = Vec::new();

            if let Some(tables_arr) = v.get("tables").and_then(Value::as_array) {
                for t in tables_arr {
                    let schema = t.get("schema").and_then(Value::as_str).unwrap_or("").to_string();
                    let name = t.get("name").and_then(Value::as_str).unwrap_or("").to_string();
                    let label = t.get("label").and_then(Value::as_str).unwrap_or(&name).to_string();
                    let geom_col = t.get("geometry_column").and_then(Value::as_str).map(str::to_string);
                    let geom_type = t.get("geometry_type").and_then(Value::as_str).map(str::to_string);
                    let is_sde = t.get("is_sde").and_then(Value::as_bool).unwrap_or(false);
                    let feature_dataset = t.get("feature_dataset").and_then(Value::as_str).map(str::to_string);

                    let srid = t.get("srid").and_then(Value::as_i64).and_then(|v| i32::try_from(v).ok());
                    let crs = t.get("crs").and_then(Value::as_str).map(str::to_string);

                    let item = CatalogItem {
                        schema,
                        name,
                        label,
                        geom_col,
                        geom_type,
                        is_sde,
                        feature_dataset: feature_dataset.clone(),
                        srid,
                        crs,
                    };

                    if let Some(ref fd) = feature_dataset {
                        datasets.entry(fd.clone()).or_default().push(item);
                    } else {
                        standalone.push(item);
                    }
                }
            }

            for items in datasets.values_mut() {
                items.sort_by(|a, b| a.name.cmp(&b.name));
            }
            standalone.sort_by(|a, b| a.name.cmp(&b.name));
            let total = standalone.len() + datasets.values().map(|v| v.len()).sum::<usize>();

            app.catalog.cache.insert(
                conn.id.clone(),
                DatabaseCache {
                    loading: false,
                    error: None,
                    feature_datasets: datasets,
                    standalone_tables: standalone,
                    total_tables: total,
                },
            );
        }
        Err(e) => {
            app.catalog.cache.insert(
                conn.id.clone(),
                DatabaseCache {
                    loading: false,
                    error: Some(e),
                    feature_datasets: BTreeMap::new(),
                    standalone_tables: Vec::new(),
                    total_tables: 0,
                },
            );
        }
    }
}

fn show_feature_dataset(
    app: &mut RustGisApp,
    ui: &mut Ui,
    t: &Tokens,
    conn: &CatalogDatabaseConn,
    dataset_name: &str,
    items: &[CatalogItem],
) {
    let ds_key = format!("{}:{}", conn.id, dataset_name);
    let is_open = app.catalog.expanded_datasets.contains(&ds_key);
    let mut toggle = false;
    let mut add_all = false;

    let row_rect = ui.available_rect_before_wrap();
    let row_h = 22.0;
    let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), row_h), Sense::click_and_drag());

    // Drag-and-drop support for the entire Feature Dataset!
    if resp.drag_started_by(egui::PointerButton::Primary) {
        egui::DragAndDrop::set_payload(ui.ctx(), DraggedCatalogLayers { conn_params: conn.params.clone(), layers: items.to_vec() });
    }

    if resp.hovered() {
        ui.painter().rect_filled(r, CornerRadius::same(4), t.hover);
    }

    let mut x = r.min.x + 4.0;
    // Expand chevron
    let chevron_r = Rect::from_min_size(pos2(x, r.center().y - 7.0), vec2(14.0, 14.0));
    icons::paint(ui.painter(), chevron_r, if is_open { "chevron_down" } else { "chevron_right" }, t.text_dim, t.accent);
    x += 16.0;

    // Feature dataset icon
    let icon_r = Rect::from_min_size(pos2(x, r.center().y - 8.0), vec2(16.0, 16.0));
    icons::paint(ui.painter(), icon_r, "feature_dataset", t.icon, t.accent);
    x += 20.0;

    // Dataset name and child count badge
    let text = format!("{} ({})", dataset_name, items.len());
    ui.painter().text(pos2(x, r.center().y), Align2::LEFT_CENTER, text, semibold(12.5), t.text);

    if resp.clicked() {
        toggle = true;
    }

    resp.context_menu(|ui| {
        if ui.button(tl!("Add All to Map")).clicked() {
            add_all = true;
            ui.close();
        }
    });

    if toggle {
        if is_open {
            app.catalog.expanded_datasets.remove(&ds_key);
        } else {
            app.catalog.expanded_datasets.insert(ds_key.clone());
        }
    }

    if add_all {
        add_catalog_layers_to_map(app, &conn.params, items);
    }

    let _ = row_rect;

    if is_open {
        ui.indent(ds_key.as_str(), |ui| {
            for item in items {
                show_feature_class_row(app, ui, t, conn, item);
            }
        });
    }
}

fn show_feature_class_row(app: &mut RustGisApp, ui: &mut Ui, t: &Tokens, conn: &CatalogDatabaseConn, item: &CatalogItem) {
    let item_key = format!("{}:{}", conn.id, item.label);
    let is_selected = app.catalog.selected.contains(&item_key);
    let row_h = 22.0;
    let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), row_h), Sense::click_and_drag());

    // Drag-and-drop: carries this layer, or all selected layers if multiple are selected!
    if resp.drag_started_by(egui::PointerButton::Primary) {
        let layers_to_drag =
            if is_selected && app.catalog.selected.len() > 1 { collect_all_selected_layers(app, conn) } else { vec![item.clone()] };
        egui::DragAndDrop::set_payload(ui.ctx(), DraggedCatalogLayers { conn_params: conn.params.clone(), layers: layers_to_drag });
    }

    if is_selected {
        ui.painter().rect_filled(r, CornerRadius::same(4), t.selection);
    } else if resp.hovered() {
        ui.painter().rect_filled(r, CornerRadius::same(4), t.hover);
    }

    let mut x = r.min.x + 4.0;

    // Selection checkbox
    let cb_rect = Rect::from_min_size(pos2(x, r.center().y - 6.0), vec2(12.0, 12.0));
    ui.painter().rect_stroke(cb_rect, CornerRadius::same(2), Stroke::new(1.0, t.border), egui::StrokeKind::Inside);
    if is_selected {
        ui.painter().rect_filled(cb_rect, CornerRadius::same(2), t.accent);
        ui.painter().line_segment(
            [pos2(cb_rect.min.x + 2.0, cb_rect.center().y), pos2(cb_rect.center().x, cb_rect.max.y - 3.0)],
            Stroke::new(1.4, Color32::WHITE),
        );
        ui.painter().line_segment(
            [pos2(cb_rect.center().x, cb_rect.max.y - 3.0), pos2(cb_rect.max.x - 2.0, cb_rect.min.y + 2.0)],
            Stroke::new(1.4, Color32::WHITE),
        );
    }
    x += 18.0;

    // Geometry type icon
    let icon_r = Rect::from_min_size(pos2(x, r.center().y - 8.0), vec2(16.0, 16.0));
    icons::paint(ui.painter(), icon_r, item.geometry_icon(), t.icon, t.accent);
    x += 20.0;

    // Name label
    ui.painter().text(pos2(x, r.center().y), Align2::LEFT_CENTER, &item.name, regular(12.0), t.text);

    // Click toggles selection, double-click adds to map immediately
    if resp.double_clicked() {
        add_catalog_layers_to_map(app, &conn.params, std::slice::from_ref(item));
    } else if resp.clicked() {
        if ui.input(|i| i.modifiers.shift || i.modifiers.command || i.modifiers.ctrl) {
            if is_selected {
                app.catalog.selected.remove(&item_key);
            } else {
                app.catalog.selected.insert(item_key.clone());
            }
        } else {
            // Normal click toggles
            if is_selected {
                app.catalog.selected.remove(&item_key);
            } else {
                app.catalog.selected.insert(item_key.clone());
            }
        }
    }

    let mut do_add = false;
    resp.context_menu(|ui| {
        if ui.button(tl!("Add To Current Map")).clicked() {
            do_add = true;
            ui.close();
        }
        if ui.button(tl!("Copy Table Name")).clicked() {
            ui.ctx().copy_text(item.label.clone());
            ui.close();
        }
    });

    if do_add {
        add_catalog_layers_to_map(app, &conn.params, std::slice::from_ref(item));
    }

    let badge = if item.is_sde {
        format!("Esri SDE: {}", item.geom_col.as_deref().unwrap_or("shape"))
    } else if let Some(ref col) = item.geom_col {
        format!("PostGIS: {col}")
    } else {
        "Spatial Feature".to_string()
    };
    resp.on_hover_ui(|ui| {
        ui.label(egui::RichText::new(&item.label).strong());
        ui.label(egui::RichText::new(badge).small().weak());
        if let Some(ref ds) = item.feature_dataset {
            ui.label(egui::RichText::new(format!("Feature Dataset: {ds}")).small().weak());
        }
        if let Some(ref ty) = item.geom_type {
            ui.label(egui::RichText::new(format!("Type: {ty}")).small().weak());
        }
    });
}

fn collect_all_selected_layers(app: &RustGisApp, conn: &CatalogDatabaseConn) -> Vec<CatalogItem> {
    let mut out = Vec::new();
    let prefix = format!("{}:", conn.id);
    let cache = match app.catalog.cache.get(&conn.id) {
        Some(c) => c,
        None => return out,
    };
    for items in cache.feature_datasets.values() {
        for it in items {
            let key = format!("{}{}", prefix, it.label);
            if app.catalog.selected.contains(&key) {
                out.push(it.clone());
            }
        }
    }
    for it in &cache.standalone_tables {
        let key = format!("{}{}", prefix, it.label);
        if app.catalog.selected.contains(&key) {
            out.push(it.clone());
        }
    }
    out
}

fn add_selected_layers(app: &mut RustGisApp, connections: &[CatalogDatabaseConn]) {
    for conn in connections {
        let layers = collect_all_selected_layers(app, conn);
        if !layers.is_empty() {
            add_catalog_layers_to_map(app, &conn.params, &layers);
        }
    }
    app.catalog.selected.clear();
}

fn show_maps_section(app: &mut RustGisApp, ui: &mut Ui, t: &Tokens) {
    egui::CollapsingHeader::new(egui::RichText::new(tl!("Maps")).font(semibold(12.5))).id_salt("cat_maps_header").default_open(true).show(
        ui,
        |ui| {
            let maps: Vec<String> = app.session.project.maps.iter().map(|m| m.name.clone()).collect();
            for (i, m) in maps.iter().enumerate() {
                ui.horizontal(|ui| {
                    let (r, _) = ui.allocate_exact_size(vec2(16.0, 16.0), Sense::hover());
                    icons::paint(ui.painter(), r, "map", t.icon, t.accent);
                    let active = i == app.session.project.active_map;
                    let resp = ui.selectable_label(active, m);
                    if resp.clicked() {
                        let _ = app.run("map.activate", json!({"name": m}));
                    }
                    resp.context_menu(|ui| {
                        if ui.button(tl!("Delete")).clicked() {
                            let _ = app.run("map.remove", json!({"name": m}));
                            ui.close();
                        }
                    });
                });
            }
        },
    );
}

fn show_toolboxes_section(_app: &mut RustGisApp, ui: &mut Ui, t: &Tokens) {
    egui::CollapsingHeader::new(egui::RichText::new(tl!("Toolboxes")).font(semibold(12.5)))
        .id_salt("cat_toolboxes_header")
        .default_open(false)
        .show(ui, |ui| {
            let mut boxes: Vec<&str> =
                rustgis_engine::analysis::tools().map(|t| t.toolbox.split(" › ").next().unwrap_or(t.toolbox)).collect();
            boxes.sort();
            boxes.dedup();
            for b in boxes {
                ui.horizontal(|ui| {
                    let (r, _) = ui.allocate_exact_size(vec2(16.0, 16.0), Sense::hover());
                    icons::paint(ui.painter(), r, "tools", t.icon, t.accent);
                    ui.label(crate::i18n::location(b));
                });
            }
        });
}

fn show_layouts_section(app: &mut RustGisApp, ui: &mut Ui, t: &Tokens) {
    let layouts: Vec<String> = app.session.project.layouts.iter().map(|l| l.name.clone()).collect();
    if layouts.is_empty() {
        return;
    }
    egui::CollapsingHeader::new(egui::RichText::new(tl!("Layouts")).font(semibold(12.5)))
        .id_salt("cat_layouts_header")
        .default_open(false)
        .show(ui, |ui| {
            for n in layouts {
                ui.horizontal(|ui| {
                    let (r, _) = ui.allocate_exact_size(vec2(16.0, 16.0), Sense::hover());
                    icons::paint(ui.painter(), r, "new_layout", t.icon, t.accent);
                    let active =
                        app.session.project.layouts.get(app.session.active_layout.unwrap_or(usize::MAX)).is_some_and(|l| l.name == n);
                    let resp = ui.selectable_label(active, &n);
                    if resp.clicked() {
                        let _ = app.run("layout.activate", json!({"name": n}));
                        app.ui.tab = "Publish".into();
                    }
                    resp.context_menu(|ui| {
                        if ui.button(tl!("Delete")).clicked() {
                            let _ = app.run("layout.remove", json!({"name": n}));
                            ui.close();
                        }
                    });
                });
            }
        });
}

/// Pre-populates the Database connection modal from a Catalog connection and opens it for editing.
pub fn edit_database_connection(app: &mut RustGisApp, conn: &CatalogDatabaseConn) {
    if conn.is_duckdb {
        app.dialogs.db.conn_type = "duckdb".into();
        let path = conn.params.get("database").and_then(Value::as_str).unwrap_or("");
        app.dialogs.db.duckdb_path = path.to_string();
    } else {
        app.dialogs.db.conn_type = "postgres".into();
        if let Some(h) = conn.params.get("host").and_then(Value::as_str) {
            app.dialogs.db.host = h.to_string();
        }
        if let Some(p) = conn.params.get("port") {
            if let Some(s) = p.as_str() {
                app.dialogs.db.port = s.to_string();
            } else if let Some(i) = p.as_i64() {
                app.dialogs.db.port = i.to_string();
            }
        }
        if let Some(u) = conn.params.get("user").and_then(Value::as_str) {
            app.dialogs.db.user = u.to_string();
        }
        if let Some(db) = conn.params.get("dbname").and_then(Value::as_str) {
            app.dialogs.db.database = db.to_string();
        }
        if let Some(s) = conn.params.get("schema").and_then(Value::as_str) {
            app.dialogs.db.schema = s.to_string();
        }
        if let Some(pw) = conn.params.get("password").and_then(Value::as_str) {
            app.dialogs.db.password = pw.to_string();
        } else if let Some(pw) = app.catalog.passwords.get(&conn.id) {
            app.dialogs.db.password = pw.clone();
        }

        // If host/database were missing, parse key=value or connection string
        if (app.dialogs.db.host.is_empty() || app.dialogs.db.database.is_empty())
            && let Some(db_str) = conn.params.get("database").and_then(Value::as_str)
        {
            let stripped = db_str.strip_prefix("postgres:").unwrap_or(db_str);
            for part in stripped.split_whitespace() {
                if let Some((k, v)) = part.split_once('=') {
                    match k {
                        "host" => app.dialogs.db.host = v.to_string(),
                        "port" => app.dialogs.db.port = v.to_string(),
                        "user" => app.dialogs.db.user = v.to_string(),
                        "dbname" => app.dialogs.db.database = v.to_string(),
                        "password" => app.dialogs.db.password = v.to_string(),
                        _ => {}
                    }
                }
            }
        }
    }
    let _ = app.run("ui.dialog", json!({"name": "addDatabase"}));
}

/// Removes a database connection from the Catalog and saved sources.
pub fn remove_database_connection(app: &mut RustGisApp, conn_id: &str, params: &Value) {
    app.catalog.expanded_databases.remove(conn_id);
    app.catalog.cache.remove(conn_id);
    app.catalog.passwords.remove(conn_id);
    if let Some(addr) = conn_id.strip_prefix("saved:") {
        let _ = app.run(
            "sources.remove",
            json!({
                "list": "saved",
                "kind": "database",
                "address": addr,
            }),
        );
    } else if let Some(addr) = params.get("database").and_then(Value::as_str) {
        let _ = app.run(
            "sources.remove",
            json!({
                "list": "saved",
                "kind": "database",
                "address": addr,
            }),
        );
    }
    if conn_id.starts_with("pg:") {
        app.dialogs.db.host.clear();
        app.dialogs.db.database.clear();
    } else if conn_id.starts_with("duckdb:") {
        app.dialogs.db.duckdb_path.clear();
    }
}
