//! Panes for tasks, the Workflow Manager, elevation profiles, the Diagnostic Monitor and
//! infographics, and the Browser view shown in place of the map.

use egui::{Color32, Sense, Ui, pos2, vec2};
use serde_json::{Value, json};

use crate::RustGisApp;
use crate::theme::{Tokens, regular, semibold};

/// State of these panes.
#[derive(Default)]
pub struct WorkspaceState {
    pub task: String,
    pub task_step: usize,
    pub job_name: String,
    pub profile_layer: String,
    pub profile_lines: String,
    pub profile: Option<Value>,
    pub message: Option<(String, bool)>,
    pub catalog_view: bool,
    pub item_popups: bool,
    pub link_cursors: bool,
    /// A second map shown beside the active one, by name: deleting or renaming it (by any
    /// command) turns the split off, undoing that brings it back, and deleting other maps
    /// doesn't move it. Read it through `mapview::split_map`.
    pub split: Option<String>,
    pub playing: Option<(f64, usize)>,
    /// Diagnostics (refreshed every second) and infographics (when the view or data change),
    /// with what they were worked out for.
    pub diagnostics: Option<(f64, Value)>,
    pub infographic: Option<([f64; 4], Value)>,
}

fn heading(ui: &mut Ui, text: &str) {
    ui.add_space(6.0);
    ui.label(egui::RichText::new(tl!(text)).font(semibold(12.5)));
    ui.add_space(2.0);
}

fn message(app: &RustGisApp, ui: &mut Ui) {
    if let Some((m, err)) = &app.ws.message {
        let c = if *err { Color32::from_rgb(0xC0, 0x30, 0x30) } else { ui.visuals().weak_text_color() };
        ui.label(egui::RichText::new(m.as_str()).small().color(c));
    }
}

fn run(app: &mut RustGisApp, id: &str, p: Value) -> Option<Value> {
    match app.run(id, p) {
        Ok(v) => {
            app.ws.message = None;
            Some(v)
        }
        Err(e) => {
            app.ws.message = Some((e, true));
            None
        }
    }
}

/// Tasks: pick one, read each step's instructions and run it.
pub fn tasks(app: &mut RustGisApp, ui: &mut Ui, _t: &Tokens) {
    message(app, ui);
    let tasks = app.session.project.tasks.clone();
    if tasks.is_empty() {
        ui.label(egui::RichText::new(tl!("No tasks yet. Data › Task makes one, for example from your geoprocessing history.")).weak());
        return;
    }
    if !tasks.iter().any(|t| t.name == app.ws.task) {
        app.ws.task = tasks.first().map(|t| t.name.clone()).unwrap_or_default();
        app.ws.task_step = 0;
    }
    egui::ComboBox::from_id_salt("task_pick").selected_text(app.ws.task.as_str()).width(ui.available_width() - 8.0).show_ui(ui, |ui| {
        for t in &tasks {
            if ui.selectable_label(t.name == app.ws.task, t.name.as_str()).clicked() {
                app.ws.task = t.name.clone();
                app.ws.task_step = 0;
            }
        }
    });
    let Some(task) = tasks.iter().find(|t| t.name == app.ws.task) else { return };
    if !task.description.is_empty() {
        ui.label(egui::RichText::new(&task.description).small());
    }
    for (i, st) in task.steps.iter().enumerate() {
        let current = i == app.ws.task_step;
        egui::Frame::NONE
            .inner_margin(egui::Margin::symmetric(6, 4))
            .fill(if current { ui.visuals().selection.bg_fill.gamma_multiply(0.25) } else { Color32::TRANSPARENT })
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(format!("{}. {}", i + 1, st.title)).font(if current {
                        semibold(12.0)
                    } else {
                        regular(12.0)
                    }));
                    if ui.small_button(tl!("Run")).clicked() {
                        let name = task.name.clone();
                        if run(app, "task.run", json!({"task": name, "step": i})).is_some() {
                            app.ws.task_step = i + 1;
                            app.ws.message = Some((crate::i18n::fmt("Step {n} done", &[("n", &(i + 1).to_string())]), false));
                            app.map.invalidate();
                        }
                    }
                });
                if !st.instructions.is_empty() && current {
                    ui.label(egui::RichText::new(&st.instructions).small().weak());
                }
            });
    }
    if app.ws.task_step >= task.steps.len() && !task.steps.is_empty() {
        ui.label(egui::RichText::new(tl!("All steps done.")).small());
        if ui.button(tl!("Start Over")).clicked() {
            app.ws.task_step = 0;
        }
    }
}

/// The Workflow Manager: jobs with their status.
pub fn jobs(app: &mut RustGisApp, ui: &mut Ui, _t: &Tokens) {
    message(app, ui);
    ui.horizontal(|ui| {
        ui.add(egui::TextEdit::singleline(&mut app.ws.job_name).hint_text(tl!("New job name")).desired_width(170.0));
        if ui.button(tl!("Add Job")).on_hover_text(tl!("The job covers the map view")).clicked() {
            let name = app.ws.job_name.trim().to_string();
            if run(app, "job.new", json!({"name": name, "viewExtent": true})).is_some() {
                app.ws.job_name.clear();
            }
        }
    });
    for status in ["new", "in progress", "on hold", "done"] {
        let jobs: Vec<_> = app.session.project.jobs.iter().filter(|j| j.status == status).cloned().collect();
        if jobs.is_empty() {
            continue;
        }
        heading(
            ui,
            match status {
                "new" => "New",
                "in progress" => "In Progress",
                "on hold" => "On Hold",
                _ => "Done",
            },
        );
        for j in jobs {
            ui.horizontal(|ui| {
                ui.label(format!("#{} {}", j.id, j.name));
                if let Some(e) = j.extent
                    && ui.small_button(tl!("Zoom")).clicked()
                {
                    app.session.zoom_to(&e);
                    app.map.invalidate();
                }
                let next = match status {
                    "new" => Some(("Start", "in progress")),
                    "in progress" => Some(("Done", "done")),
                    "on hold" => Some(("Resume", "in progress")),
                    _ => None,
                };
                if let Some((label, to)) = next
                    && ui.small_button(tl!(label)).clicked()
                {
                    run(app, "job.update", json!({"job": j.id, "status": to}));
                }
                if status == "in progress" && ui.small_button(tl!("Hold")).clicked() {
                    run(app, "job.update", json!({"job": j.id, "status": "on hold"}));
                }
                if ui.small_button("×").clicked() {
                    run(app, "job.remove", json!({"job": j.id}));
                }
            });
        }
    }
}

/// Elevation profiles, drawn as a chart.
pub fn profile(app: &mut RustGisApp, ui: &mut Ui, t: &Tokens) {
    message(app, ui);
    let map = app.session.map();
    let rasters: Vec<String> = map.layers.iter().filter(|l| l.is_raster()).map(|l| l.name.clone()).collect();
    let lines: Vec<String> =
        map.layers.iter().filter(|l| l.geometry_type() == rustgis_engine::data::GeometryType::Polyline).map(|l| l.name.clone()).collect();
    if !rasters.contains(&app.ws.profile_layer) {
        app.ws.profile_layer = rasters.first().cloned().unwrap_or_default();
    }
    if rasters.is_empty() {
        ui.label(egui::RichText::new(tl!("Add an elevation raster first.")).weak());
        return;
    }
    egui::ComboBox::from_id_salt("prof_layer").selected_text(app.ws.profile_layer.as_str()).show_ui(ui, |ui| {
        for r in &rasters {
            ui.selectable_value(&mut app.ws.profile_layer, r.clone(), r.as_str());
        }
    });
    ui.horizontal_wrapped(|ui| {
        for (label, purpose, n) in [
            ("Create (click points)", "p_create", 100),
            ("Auto-complete (2 clicks)", "p_auto", 2),
            ("Cross Section (2 clicks)", "p_cross", 2),
        ] {
            let active = app.workflow.pick.as_ref().is_some_and(|p| p.purpose == purpose);
            if ui.selectable_label(active, tl!(label)).clicked() {
                app.workflow.pick = if active { None } else { Some(crate::analysispane::Pick { purpose, needed: n, points: Vec::new() }) };
            }
        }
    });
    let layer = app.ws.profile_layer.clone();
    let done = |app: &RustGisApp, purpose: &str, min: usize| {
        app.workflow
            .pick
            .as_ref()
            .filter(|p| p.purpose == purpose && p.points.len() >= min)
            .map(|p| p.points.iter().map(|c| [c.x, c.y]).collect::<Vec<_>>())
    };
    for (purpose, cmd) in [("p_auto", "profile.autoComplete"), ("p_cross", "profile.crossSection")] {
        if let Some(pts) = done(app, purpose, 2) {
            app.workflow.pick = None;
            app.ws.profile = run(app, cmd, json!({"layer": layer, "points": pts}));
        }
    }
    if let Some(pts) = done(app, "p_create", 2)
        && ui.button(tl!("Finish Profile")).clicked()
    {
        app.workflow.pick = None;
        app.ws.profile = run(app, "profile.create", json!({"layer": layer, "points": pts}));
    }
    ui.horizontal(|ui| {
        if !lines.contains(&app.ws.profile_lines) {
            app.ws.profile_lines = lines.first().cloned().unwrap_or_default();
        }
        egui::ComboBox::from_id_salt("prof_lines").selected_text(app.ws.profile_lines.as_str()).width(120.0).show_ui(ui, |ui| {
            for l in &lines {
                ui.selectable_value(&mut app.ws.profile_lines, l.clone(), l.as_str());
            }
        });
        if ui.button(tl!("Along Selected Line")).clicked() {
            app.ws.profile = run(app, "profile.alongLine", json!({"layer": layer, "lines": app.ws.profile_lines}));
        }
        if ui.button(tl!("All Selected")).clicked() {
            app.ws.profile = run(app, "profile.alongSelected", json!({"layer": layer, "lines": app.ws.profile_lines}))
                .and_then(|v| v.as_array().and_then(|a| a.first().cloned()));
        }
    });
    let Some(p) = app.ws.profile.clone() else { return };
    let pts: Vec<(f64, f64)> = p["samples"]
        .as_array()
        .map(|a| a.iter().filter_map(|s| Some((s["distance"].as_f64()?, s["elevation"].as_f64()?))).collect())
        .unwrap_or_default();
    if pts.len() < 2 {
        return;
    }
    let (lo, hi) = (p["min"].as_f64().unwrap_or(0.0), p["max"].as_f64().unwrap_or(1.0).max(p["min"].as_f64().unwrap_or(0.0) + 1.0));
    let len = p["length"].as_f64().unwrap_or(1.0).max(1.0);
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 150.0), Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 4.0, t.canvas);
    let to = |(d, z): (f64, f64)| {
        pos2(
            rect.min.x + 6.0 + (d / len) as f32 * (rect.width() - 12.0),
            rect.max.y - 16.0 - ((z - lo) / (hi - lo)) as f32 * (rect.height() - 30.0),
        )
    };
    let line: Vec<egui::Pos2> = pts.iter().map(|p| to(*p)).collect();
    let mut fill = line.clone();
    fill.push(pos2(rect.max.x - 6.0, rect.max.y - 16.0));
    fill.push(pos2(rect.min.x + 6.0, rect.max.y - 16.0));
    painter.add(egui::Shape::Path(egui::epaint::PathShape {
        points: fill,
        closed: true,
        fill: t.accent.gamma_multiply(0.18),
        stroke: egui::epaint::PathStroke::NONE,
    }));
    painter.add(egui::Shape::line(line, egui::Stroke::new(2.0, t.accent)));
    let fmt = |v: f64| rustgis_engine::map::symbology::fmt_num(v);
    painter.text(rect.left_top() + vec2(6.0, 4.0), egui::Align2::LEFT_TOP, format!("{} m", fmt(hi)), regular(10.0), t.text_dim);
    painter.text(rect.left_bottom() + vec2(6.0, -4.0), egui::Align2::LEFT_BOTTOM, format!("{} m", fmt(lo)), regular(10.0), t.text_dim);
    painter.text(
        rect.right_bottom() + vec2(-6.0, -4.0),
        egui::Align2::RIGHT_BOTTOM,
        rustgis_engine::geom::format_distance(len),
        regular(10.0),
        t.text_dim,
    );
    ui.label(
        egui::RichText::new(crate::i18n::fmt(
            "Length {length} · gain {gain} m · loss {loss} m",
            &[
                ("length", &rustgis_engine::geom::format_distance(len)),
                ("gain", &fmt(p["gain"].as_f64().unwrap_or(0.0))),
                ("loss", &fmt(p["loss"].as_f64().unwrap_or(0.0))),
            ],
        ))
        .small(),
    );
}

/// The Diagnostic Monitor.
pub fn diagnostics(app: &mut RustGisApp, ui: &mut Ui, _t: &Tokens) {
    ui.ctx().request_repaint_after(std::time::Duration::from_millis(500));
    let dt = ui.input(|i| i.unstable_dt).max(1e-4);
    let now = ui.input(|i| i.time);
    egui::Grid::new("diag").num_columns(2).striped(true).spacing(vec2(12.0, 4.0)).show(ui, |ui| {
        let mut row = |k: &str, v: String| {
            ui.label(egui::RichText::new(tl!(k)).small());
            ui.label(egui::RichText::new(v).small().monospace());
            ui.end_row();
        };
        row("Frame rate", format!("{:.0} fps", 1.0 / dt));
        row("Map draw", format!("{} ms", app.map.render_ms()));
        if app.ws.diagnostics.as_ref().is_none_or(|(t, _)| now - t > 1.0) {
            app.ws.diagnostics = Some((now, rustgis_engine::cmd::workspace::diagnostics(&app.session)));
        }
        if let Some((_, v)) = &app.ws.diagnostics
            && let Some(o) = v.as_object()
        {
            for (k, v) in o {
                row(k, v.to_string());
            }
        }
    });
}

/// Infographics: cards summarising what's in view.
pub fn infographics(app: &mut RustGisApp, ui: &mut Ui, t: &Tokens) {
    ui.label(egui::RichText::new(tl!("Live summary of the features in view.")).small().color(t.text_dim));
    // Worked out again only when the view or the data change.
    let cam = app.session.map().camera;
    // The revision changes with every edit, undo and redo (the undo depth stops at its limit).
    let key = [cam.center.x, cam.center.y, cam.res, app.session.revision as f64];
    if app.ws.infographic.as_ref().is_none_or(|(k, _)| *k != key) {
        app.ws.infographic = Some((key, rustgis_engine::cmd::workspace::infographic_of(&app.session, None)));
    }
    let Some((_, v)) = app.ws.infographic.clone() else { return };
    for card in v["layers"].as_array().cloned().unwrap_or_default() {
        egui::Frame::NONE.fill(t.canvas).corner_radius(8).inner_margin(egui::Margin::same(10)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(egui::RichText::new(card["layer"].as_str().unwrap_or("")).font(semibold(13.0)));
            ui.label(
                egui::RichText::new(crate::i18n::fmt("{n} features in view", &[("n", &card["count"].to_string())]))
                    .size(20.0)
                    .color(t.accent),
            );
            for n in card["numbers"].as_array().cloned().unwrap_or_default() {
                let sum = n["sum"].as_f64().map(rustgis_engine::map::symbology::fmt_num).unwrap_or_default();
                ui.label(
                    egui::RichText::new(crate::i18n::fmt(
                        "{field}: total {sum}",
                        &[("field", n["field"].as_str().unwrap_or("")), ("sum", &sum)],
                    ))
                    .small(),
                );
            }
            let top = card["top"].as_array().cloned().unwrap_or_default();
            let max = top.iter().filter_map(|x| x["count"].as_f64()).fold(1.0, f64::max);
            for x in top {
                ui.horizontal(|ui| {
                    let c = x["count"].as_f64().unwrap_or(0.0);
                    let (r, _) = ui.allocate_exact_size(vec2(80.0, 10.0), Sense::hover());
                    ui.painter().rect_filled(egui::Rect::from_min_size(r.min, vec2(80.0 * (c / max) as f32, 10.0)), 2.0, t.accent);
                    ui.label(egui::RichText::new(format!("{} ({c})", x["value"].as_str().unwrap_or(""))).small());
                });
            }
        });
        ui.add_space(6.0);
    }
}

/// The Browser view: the project's items as a grid of cards, in place of the map.
pub fn catalog_view(app: &mut RustGisApp, ui: &mut Ui, t: &Tokens) {
    let p = &app.session.project;
    let mut items: Vec<(&str, String, &str, Option<Value>)> = Vec::new();
    for m in &p.maps {
        items.push(("map", m.name.clone(), "Map", Some(json!({"cmd": "map.activate", "name": m.name}))));
    }
    for l in &p.layouts {
        items.push(("new_layout", l.name.clone(), "Layout", Some(json!({"cmd": "layout.activate", "name": l.name}))));
    }
    for tb in &p.tables {
        items.push(("table", tb.name.clone(), "Table", None));
    }
    for s in &p.styles {
        items.push(("symbology", s.name.clone(), "Style", None));
    }
    for tk in &p.tasks {
        items.push(("run", tk.name.clone(), "Task", None));
    }
    for m in &p.models {
        items.push(("modelbuilder", m.name.clone(), "Model", None));
    }
    for f in &p.folders {
        items.push(("add_folder", f.clone(), "Folder", None));
    }
    for a in &p.offline_areas {
        items.push(("map", a.name.clone(), "Offline area", Some(json!({"cmd": "offline.open", "name": a.name}))));
    }
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(tl!("Browser")).font(semibold(16.0)));
        if ui.button(tl!("Back to the Map")).clicked() {
            app.ws.catalog_view = false;
        }
    });
    ui.add_space(8.0);
    let mut open: Option<Value> = None;
    egui::ScrollArea::vertical().show(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            for (icon, name, kind, action) in &items {
                let (r, resp) = ui.allocate_exact_size(vec2(150.0, 110.0), Sense::click());
                ui.painter().rect_filled(r, 8.0, if resp.hovered() { t.hover } else { t.canvas });
                crate::icons::paint(
                    ui.painter(),
                    egui::Rect::from_center_size(pos2(r.center().x, r.min.y + 36.0), vec2(40.0, 40.0)),
                    icon,
                    t.icon,
                    t.accent,
                );
                let short: String = name.chars().take(20).collect();
                ui.painter().text(pos2(r.center().x, r.min.y + 72.0), egui::Align2::CENTER_CENTER, short, semibold(12.0), t.text);
                ui.painter().text(pos2(r.center().x, r.min.y + 90.0), egui::Align2::CENTER_CENTER, tl!(*kind), regular(10.5), t.text_dim);
                let resp = if app.ws.item_popups { resp.on_hover_text(format!("{} · {name}", tl!(*kind))) } else { resp };
                if resp.double_clicked() {
                    open = action.clone();
                }
            }
        });
    });
    if let Some(a) = open {
        let cmd = a["cmd"].as_str().unwrap_or("").to_string();
        let _ = app.run(&cmd, json!({"name": a["name"]}));
        app.ws.catalog_view = false;
        if cmd == "layout.activate" {
            app.ui.tab = "Publish".into();
        }
        app.map.invalidate();
    }
}
