//! The Bookmarks pane (Home › Navigate › Bookmarks › Manage Bookmarks, #351): every map's
//! bookmarks, grouped by map and filtered by name or description. Each one zooms or pans the map
//! in view to its place, takes the current view (on its own map), and is renamed, described,
//! moved or deleted in place — all through the `bookmark.*` commands, so all of it undoes.

use egui::{RichText, Sense, Ui, vec2};
use serde_json::{Value, json};

use crate::theme::{Tokens, semibold};
use crate::{RustGisApp, i18n, icons, widgets};

/// The most bookmarks the pane lists at once: a map can import thousands, and every row is laid
/// out each frame; the filter narrows the rest.
const MAX_ROWS: usize = 500;

/// The Bookmarks pane's state.
#[derive(Default)]
pub struct State {
    pub filter: String,
    /// The bookmark whose description shows, by map and bookmark name.
    open: Option<(String, String)>,
    /// A name or description being edited.
    editing: Option<Edit>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Field {
    Name,
    Description,
}

struct Edit {
    map: String,
    name: String,
    field: Field,
    text: String,
    /// Whether the box has been given the focus (once, when the edit starts).
    focused: bool,
}

impl Edit {
    fn is(&self, map: &str, name: &str, field: Field) -> bool {
        self.map == map && self.name == name && self.field == field
    }
}

/// A bookmark as the pane lists it.
struct Mark {
    /// Its position in its map's list.
    index: usize,
    name: String,
    description: String,
}

/// The keys an edit box keeps: the arrows move its cursor (as a plain TextEdit's do), and Escape
/// reaches it rather than the global shortcuts.
const KEYS: egui::EventFilter = egui::EventFilter { horizontal_arrows: true, vertical_arrows: true, escape: true, tab: false };

fn run(app: &mut RustGisApp, id: &str, p: Value) {
    if let Err(e) = app.run(id, p) {
        app.session.notify(e, true);
    }
}

/// A 26 px icon button that ui_shot (and screen readers) can find by `tip`.
fn act(ui: &mut Ui, icon: &str, tip: &str) -> egui::Response {
    let enabled = ui.is_enabled();
    let r = widgets::icon_button(ui, icon, tip);
    r.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, tip));
    r
}

pub fn show(app: &mut RustGisApp, ui: &mut Ui, t: &Tokens) {
    ui.horizontal(|ui| {
        let gap = ui.spacing().item_spacing.x;
        let room = (ui.available_width() - 3.0 * (26.0 + gap)).max(60.0);
        ui.allocate_ui(vec2(room, 26.0), |ui| widgets::search_field(ui, &mut app.ui.bookmarks.filter, tl!("Filter bookmarks")));
        if act(ui, "bookmark_new", tl!("New Bookmark…")).clicked() {
            run(app, "ui.dialog", json!({"name": "bookmark"}));
        }
        if act(ui, "bookmark_import", tl!("Import Bookmarks…")).clicked() {
            run(app, "ui.openItem", json!({"kind": "bookmarks"}));
        }
        if act(ui, "bookmark_export", tl!("Export Bookmarks…")).clicked() {
            run(app, "ui.saveItem", json!({"kind": "bookmarks"}));
        }
    });
    ui.add_space(4.0);
    // An edit or a description open on a bookmark that is gone (deleted, renamed, undone) ends.
    {
        let maps = &app.session.project.maps;
        let has = |map: &str, name: &str| maps.iter().any(|m| m.name == map && m.bookmarks.iter().any(|b| b.name == name));
        let st = &mut app.ui.bookmarks;
        if st.editing.as_ref().is_some_and(|e| !has(&e.map, &e.name)) {
            st.editing = None;
        }
        if st.open.as_ref().is_some_and(|(m, n)| !has(m, n)) {
            st.open = None;
        }
    }
    // A snapshot: the rows run commands that change the list under them.
    let q = app.ui.bookmarks.filter.trim().to_lowercase();
    let active = app.session.project.active_map;
    // Each map with bookmarks: its name, whether it is in view, how many it has (for Move Down)
    // and those the filter keeps, up to MAX_ROWS in all (`hidden` counts the rest).
    let (mut room, mut hidden) = (MAX_ROWS, 0usize);
    let maps: Vec<(String, bool, usize, Vec<Mark>)> = app
        .session
        .project
        .maps
        .iter()
        .enumerate()
        .map(|(i, m)| {
            let mut marks = Vec::new();
            for (index, b) in m.bookmarks.iter().enumerate() {
                if !(q.is_empty() || b.name.to_lowercase().contains(&q) || b.description.to_lowercase().contains(&q)) {
                    continue;
                }
                if room == 0 {
                    hidden += 1;
                    continue;
                }
                room -= 1;
                marks.push(Mark { index, name: b.name.clone(), description: b.description.clone() });
            }
            (m.name.clone(), i == active, m.bookmarks.len(), marks)
        })
        .filter(|(_, _, _, marks)| !marks.is_empty())
        .collect();
    if maps.is_empty() {
        let hint = if q.is_empty() {
            tl!("No bookmarks yet. New Bookmark saves the current view of the map.")
        } else {
            tl!("No bookmark matches the filter.")
        };
        ui.label(RichText::new(hint).weak());
        return;
    }
    for (map, in_view, count, marks) in maps {
        let mut title = RichText::new(&map).font(semibold(12.5));
        if in_view {
            title = title.color(t.accent);
        }
        egui::CollapsingHeader::new(title).id_salt(("bookmarks", &map)).default_open(true).show(ui, |ui| {
            for m in &marks {
                row(app, ui, t, &map, in_view, count, m);
            }
        });
    }
    if hidden > 0 {
        ui.add_space(4.0);
        let more = i18n::fmt("{n} more bookmarks aren't listed. Type in the filter to narrow the list.", &[("n", &hidden.to_string())]);
        ui.label(RichText::new(more).small().weak());
    }
}

/// One bookmark: a chevron that shows its description, its name (a click zooms to it; a double
/// click renames it; right-click for the rest), and Zoom To, Pan To, Update and Delete.
fn row(app: &mut RustGisApp, ui: &mut Ui, t: &Tokens, map: &str, in_view: bool, count: usize, m: &Mark) {
    let key = (map.to_string(), m.name.clone());
    let open = app.ui.bookmarks.open.as_ref() == Some(&key);
    let p = json!({"map": map, "name": m.name});
    let mut go = None;
    // Right to left: the buttons take their room first, and the name what is left.
    let row = egui::Layout::right_to_left(egui::Align::Center);
    ui.allocate_ui_with_layout(vec2(ui.available_width(), 26.0), row, |ui| {
        if act(ui, "delete", tl!("Delete")).clicked() {
            run(app, "bookmark.remove", p.clone());
        }
        let off = tl!("Bring this bookmark's map into view to update it");
        let update = ui.add_enabled_ui(in_view, |ui| act(ui, "bookmark_update", tl!("Update")).on_disabled_hover_text(off).clicked());
        if update.inner {
            run(app, "bookmark.update", p.clone());
        }
        if act(ui, "explore", tl!("Pan To")).clicked() {
            go = Some(true);
        }
        if act(ui, "zoom_in", tl!("Zoom To")).clicked() {
            go = Some(false);
        }
        ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
            let (r, resp) = ui.allocate_exact_size(vec2(16.0, 16.0), Sense::click());
            let tip = if open { tl!("Hide the description") } else { tl!("Show the description") };
            resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, tip));
            icons::paint(ui.painter(), r, if open { "chevron_down" } else { "chevron_right" }, t.text_dim, t.accent);
            if resp.on_hover_text(tip).clicked() {
                app.ui.bookmarks.open = if open { None } else { Some(key.clone()) };
            }
            if app.ui.bookmarks.editing.as_ref().is_some_and(|e| e.is(map, &m.name, Field::Name)) {
                name_box(app, ui, &p);
                return;
            }
            let resp = ui.add(egui::Button::new(m.name.as_str()).frame(false).truncate()).on_hover_text(tl!("Zoom to this bookmark"));
            if resp.clicked() {
                go = Some(false);
            }
            if resp.double_clicked() {
                start(app, map, m, Field::Name);
            }
            resp.context_menu(|ui| menu(app, ui, map, in_view, count, m, &p));
        });
    });
    if let Some(pan) = go {
        let mut q = p.clone();
        q["pan"] = json!(pan);
        run(app, "bookmark.go", q);
    }
    if open {
        let indent = egui::Margin { left: 22, right: 4, top: 0, bottom: 6 };
        egui::Frame::NONE.inner_margin(indent).show(ui, |ui| description(app, ui, t, map, m, &p));
    }
}

/// The right-click menu of a bookmark's name.
fn menu(app: &mut RustGisApp, ui: &mut Ui, map: &str, in_view: bool, count: usize, m: &Mark, p: &Value) {
    if ui.button(tl!("Rename")).clicked() {
        start(app, map, m, Field::Name);
        ui.close();
    }
    if ui.button(tl!("Edit Description")).clicked() {
        app.ui.bookmarks.open = Some((map.to_string(), m.name.clone()));
        start(app, map, m, Field::Description);
        ui.close();
    }
    let update = ui.add_enabled(in_view, egui::Button::new(tl!("Update")));
    if update.on_disabled_hover_text(tl!("Bring this bookmark's map into view to update it")).clicked() {
        run(app, "bookmark.update", p.clone());
        ui.close();
    }
    ui.separator();
    for (label, to, can) in [("Move Up", m.index.saturating_sub(1), m.index > 0), ("Move Down", m.index + 1, m.index + 1 < count)] {
        if ui.add_enabled(can, egui::Button::new(tl!(label))).clicked() {
            let mut q = p.clone();
            q["to"] = json!(to);
            run(app, "bookmark.move", q);
            ui.close();
        }
    }
    ui.separator();
    if ui.button(tl!("Delete")).clicked() {
        run(app, "bookmark.remove", p.clone());
        ui.close();
    }
}

fn start(app: &mut RustGisApp, map: &str, m: &Mark, field: Field) {
    let text = if field == Field::Name { m.name.clone() } else { m.description.clone() };
    app.ui.bookmarks.editing = Some(Edit { map: map.to_string(), name: m.name.clone(), field, text, focused: false });
}

/// The box a bookmark is renamed in: Enter (or clicking away) renames it, Escape leaves it as it was.
fn name_box(app: &mut RustGisApp, ui: &mut Ui, p: &Value) {
    let Some(mut e) = app.ui.bookmarks.editing.take() else { return };
    let r = ui.add(egui::TextEdit::singleline(&mut e.text).desired_width(ui.available_width()).event_filter(KEYS));
    if !e.focused {
        r.request_focus();
        e.focused = true;
    }
    if r.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        return;
    }
    if r.lost_focus() {
        let new = e.text.trim();
        if !new.is_empty() && new != e.name {
            let mut q = p.clone();
            q["newName"] = json!(new);
            run(app, "bookmark.rename", q);
        }
        return;
    }
    app.ui.bookmarks.editing = Some(e);
}

/// A bookmark's description, or the box it is edited in.
fn description(app: &mut RustGisApp, ui: &mut Ui, t: &Tokens, map: &str, m: &Mark, p: &Value) {
    if !app.ui.bookmarks.editing.as_ref().is_some_and(|e| e.is(map, &m.name, Field::Description)) {
        if m.description.is_empty() {
            if ui.link(tl!("Add a description.")).clicked() {
                start(app, map, m, Field::Description);
            }
        } else {
            let r = ui.add(egui::Label::new(RichText::new(&m.description).color(t.text_dim)).sense(Sense::click()));
            if r.on_hover_text(tl!("Click to edit the description")).clicked() {
                start(app, map, m, Field::Description);
            }
        }
        return;
    }
    let Some(mut e) = app.ui.bookmarks.editing.take() else { return };
    // As in the New Bookmark dialog: Enter saves, Shift+Enter breaks the line.
    let new_line = egui::KeyboardShortcut::new(egui::Modifiers::SHIFT, egui::Key::Enter);
    let edit = egui::TextEdit::multiline(&mut e.text).hint_text(tl!("Shift+Enter starts a new line.")).desired_rows(3).return_key(new_line);
    let r = ui.add(edit.desired_width(ui.available_width()).event_filter(KEYS));
    if !e.focused {
        r.request_focus();
        e.focused = true;
    }
    // Escape cancels; clicking away keeps the box (Save and Cancel take the focus from it too).
    let escape = r.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Escape));
    let enter = r.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter) && !i.modifiers.shift);
    let (mut save, mut cancel) = (enter, false);
    ui.horizontal(|ui| {
        save |= ui.small_button(tl!("Save")).clicked();
        cancel = ui.small_button(tl!("Cancel")).clicked();
    });
    if save {
        let mut q = p.clone();
        q["description"] = json!(e.text);
        run(app, "bookmark.describe", q);
    } else if !cancel && !escape {
        app.ui.bookmarks.editing = Some(e);
    }
}
