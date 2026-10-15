//! The attribute table, docked under the map: sort, filter to the selection, select rows (synced
//! with the map) and edit values in place.

use egui::{Sense, Stroke, Ui, vec2};
use egui_extras::{Column, TableBuilder};
use rustgis_engine::data::{Dataset, Value as V};
use serde_json::{Value, json};

use crate::theme::{Tokens, semibold};
use crate::widgets::small;
use crate::{RustGisApp, icons};

#[derive(Default)]
pub struct TableState {
    pub folder: Option<std::path::PathBuf>,
    sort: Option<(usize, bool)>,
    selected_only: bool,
    editing: Option<(usize, usize, String)>,
    filter: String,
}

pub fn show(app: &mut RustGisApp, ui: &mut Ui) {
    let layer_id = app.session.table_layer;
    if layer_id.is_none() && app.session.table_standalone.is_none() {
        return;
    }
    let t = Tokens::get(ui.ctx());
    egui::Panel::bottom("attribute_table")
        .resizable(true)
        .default_size(250.0)
        .size_range(120.0..=600.0)
        .frame(egui::Frame::NONE.fill(t.pane))
        .show(ui, |ui| {
            // Data source: a layer or a standalone table.
            let (ds, selection, title): (Dataset, Vec<usize>, String) = match layer_id.and_then(|id| app.session.map().layer(id)) {
                Some(l) => (l.dataset.clone(), l.selection.clone(), l.name.clone()),
                None => match app.session.table_standalone.and_then(|id| app.session.project.table(id)) {
                    Some(tb) => (tb.dataset.clone(), Vec::new(), tb.name.clone()),
                    None => {
                        app.session.table_standalone = None;
                        return;
                    }
                },
            };
            let is_layer = layer_id.is_some();
            // Header.
            let (hr, _) = ui.allocate_exact_size(vec2(ui.available_width(), 34.0), Sense::hover());
            ui.painter().rect_filled(hr, 0.0, t.pane_header);
            ui.painter().line_segment([egui::pos2(hr.min.x, hr.min.y), egui::pos2(hr.max.x, hr.min.y)], Stroke::new(1.0, t.border_strong));
            let mut header_ui = ui.new_child(
                egui::UiBuilder::new().max_rect(hr.shrink2(vec2(10.0, 4.0))).layout(egui::Layout::left_to_right(egui::Align::Center)),
            );
            {
                let ui = &mut header_ui;
                let (r, _) = ui.allocate_exact_size(vec2(18.0, 18.0), Sense::hover());
                icons::paint(ui.painter(), r, "table", t.icon, t.accent);
                ui.label(egui::RichText::new(&title).font(semibold(13.0)));
                ui.label(
                    egui::RichText::new(crate::i18n::fmt(
                        "{n} of {total} selected",
                        &[("n", &selection.len().to_string()), ("total", &ds.features.len().to_string())],
                    ))
                    .size(11.5)
                    .color(t.text_dim),
                );
                ui.add_space(10.0);
                if is_layer {
                    small(
                        ui,
                        app,
                        "select_attr",
                        "Select By Attributes",
                        "Select rows with a SQL expression.",
                        "ui.dialog",
                        json!({"name": "selectByAttributes"}),
                        false,
                    );
                    small(ui, app, "zoom_sel", "", "Zoom to selection", "select.zoomTo", Value::Null, false);
                    small(ui, app, "select", "", "Switch selection", "select.switch", Value::Null, false);
                    small(ui, app, "clear_sel", "", "Clear selection", "select.clear", Value::Null, false);
                    small(ui, app, "delete", "", "Delete selected rows", "edit.delete", Value::Null, false);
                    small(ui, app, "field", "", "Add field", "ui.dialog", json!({"name": "addField"}), false);
                    small(ui, app, "calculate", "", "Calculate field", "ui.dialog", json!({"name": "calculateField"}), false);
                    ui.add_space(6.0);
                    ui.checkbox(&mut app.table.selected_only, egui::RichText::new(tl!("Selected only")).size(11.5));
                }
                ui.add(egui::TextEdit::singleline(&mut app.table.filter).hint_text(tl!("Filter rows…")).desired_width(160.0));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.add(egui::Button::new(tl!("×")).frame(false)).on_hover_text(tl!("Close table")).clicked() {
                        app.session.table_layer = None;
                        app.session.table_standalone = None;
                        app.table.editing = None;
                    }
                });
            }
            // Rows to show.
            let mut rows: Vec<usize> =
                if app.table.selected_only && is_layer { selection.clone() } else { (0..ds.features.len()).collect() };
            let f = app.table.filter.trim().to_lowercase();
            if !f.is_empty() {
                rows.retain(|r| ds.features.get(*r).is_some_and(|ft| ft.attrs.iter().any(|v| v.to_string().to_lowercase().contains(&f))));
            }
            if let Some((c, asc)) = app.table.sort {
                rows.sort_by(|a, b| {
                    let o = ds.value(*a, c).partial_cmp(ds.value(*b, c)).unwrap_or(std::cmp::Ordering::Equal);
                    if asc { o } else { o.reverse() }
                });
            }
            let nf = ds.fields.len();
            let mut clicked: Option<(usize, bool)> = None;
            let mut commit: Option<(usize, usize, String)> = None;
            let mut sort_click: Option<usize> = None;
            egui::ScrollArea::horizontal().show(ui, |ui| {
                let mut tb = TableBuilder::new(ui)
                    .striped(true)
                    .resizable(true)
                    .sense(Sense::click())
                    .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                    .column(Column::exact(54.0));
                for _ in 0..nf {
                    tb = tb.column(Column::initial(120.0).at_least(50.0).clip(true));
                }
                tb.min_scrolled_height(0.0)
                    .header(24.0, |mut h| {
                        h.col(|ui| {
                            ui.label(egui::RichText::new(tl!("OID")).font(semibold(11.5)).color(t.text_dim));
                        });
                        for (i, fd) in ds.fields.iter().enumerate() {
                            h.col(|ui| {
                                let arrow = match app.table.sort {
                                    Some((c, true)) if c == i => " ↑",
                                    Some((c, false)) if c == i => " ↓",
                                    _ => "",
                                };
                                let r = ui.add(
                                    egui::Label::new(egui::RichText::new(format!("{}{arrow}", fd.display())).font(semibold(11.5)))
                                        .sense(Sense::click()),
                                );
                                if r.on_hover_text(crate::i18n::fmt(
                                    "{field} ({type}) · click to sort",
                                    &[("field", &fd.name), ("type", tl!(fd.ty.label()))],
                                ))
                                .clicked()
                                {
                                    sort_click = Some(i);
                                }
                            });
                        }
                    })
                    .body(|body| {
                        body.rows(22.0, rows.len(), |mut row| {
                            let Some(&r) = rows.get(row.index()) else { return };
                            let sel = selection.contains(&r);
                            row.set_selected(sel);
                            let oid = ds.features.get(r).map(|f| f.id).unwrap_or(0);
                            row.col(|ui| {
                                ui.label(egui::RichText::new(oid.to_string()).size(11.5).color(t.text_dim));
                            });
                            for c in 0..nf {
                                row.col(|ui| {
                                    let editing = app.table.editing.as_ref().is_some_and(|(er, ec, _)| *er == r && *ec == c);
                                    if editing {
                                        if let Some((_, _, buf)) = app.table.editing.as_mut() {
                                            let te = ui.add(egui::TextEdit::singleline(buf).desired_width(f32::INFINITY));
                                            te.request_focus();
                                            if te.lost_focus() {
                                                commit = Some((r, c, buf.clone()));
                                            }
                                        }
                                    } else {
                                        let v = ds.value(r, c);
                                        // A picture (a photo's thumbnail) is named, not spelled out.
                                        let picture = matches!(v, V::Text(s) if crate::mapview::is_picture(s));
                                        let shown = if picture { tl!("picture").to_string() } else { v.to_string() };
                                        let txt = egui::RichText::new(shown).size(11.5);
                                        let txt = if matches!(v, V::Null) || picture { txt.color(t.text_disabled).italics() } else { txt };
                                        let resp = ui.add(egui::Label::new(txt).sense(Sense::click()).truncate());
                                        if resp.double_clicked() && is_layer {
                                            app.table.editing =
                                                Some((r, c, if matches!(v, V::Null) { String::new() } else { v.to_string() }));
                                        } else if resp.clicked() {
                                            clicked = Some((r, ui.input(|i| i.modifiers.command || i.modifiers.shift)));
                                        }
                                    }
                                });
                            }
                            if row.response().clicked() {
                                clicked = Some((r, row.response().ctx.input(|i| i.modifiers.command || i.modifiers.shift)));
                            }
                        });
                    });
            });
            if let Some(i) = sort_click {
                app.table.sort = match app.table.sort {
                    Some((c, true)) if c == i => Some((i, false)),
                    Some((c, false)) if c == i => None,
                    _ => Some((i, true)),
                };
            }
            if let Some((r, add)) = clicked
                && is_layer
            {
                let mode = if add { if selection.contains(&r) { "remove" } else { "add" } } else { "new" };
                let _ = app.run("select.rows", json!({"layer": layer_id, "rows": [r], "mode": mode}));
            }
            if let Some((r, c, text)) = commit {
                app.table.editing = None;
                if let Some(fd) = ds.fields.get(c) {
                    let _ = app.run("edit.setValue", json!({"layer": layer_id, "row": r, "field": fd.name, "value": text}));
                }
            }
        });
}
