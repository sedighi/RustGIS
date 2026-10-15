//! Add Data › Zarr…: a store's variables, a slice picked by its other dimensions (a slider for
//! time), added as a raster layer. A store by web address is read off the frame through the tile
//! cache's downloads (a thread natively, fetch on the web), a few files at a time; a folder (the
//! desktop) through the commands.

use std::collections::HashMap;

use egui::vec2;
use rustgis_engine::data::raster::{Crs, geotiff, zarr};
use rustgis_engine::render::rustgis_tiles::cache;
use serde_json::{Value, json};

use crate::{RustGisApp, Slot, i18n};

type Answer = Slot<Result<Vec<u8>, String>>;

/// Files fetched at once.
const AT_ONCE: usize = 6;

/// What the dialog is fetching.
#[derive(Default)]
enum Phase {
    #[default]
    Idle,
    /// The metadata files.
    Meta,
    /// The coordinates that label the dimensions.
    Labels,
    /// A slice's chunks, to be added as `name` once they land.
    Read(Box<(zarr::Plan, zarr::Selection, String)>),
}

/// The dialog's store and choices, and the fetches under way.
#[derive(Default)]
pub struct State {
    pub address: String,
    pub crs: String,
    /// The store's root: a web address or a folder, as connected.
    root: String,
    web: bool,
    store: Option<zarr::Store>,
    /// `raster.zarrInfo`'s answer: variables, dimensions and their labels.
    info: Option<Value>,
    pub variable: usize,
    /// The steps picked, by dimension name.
    steps: HashMap<String, u64>,
    phase: Phase,
    queue: Vec<String>,
    waiting: Vec<(String, Answer)>,
    got: HashMap<String, Vec<u8>>,
    /// Files of this batch refused with 403: all metadata refused is access denied, not a store
    /// without metadata.
    forbidden: usize,
    pub error: Option<String>,
}

impl State {
    fn busy(&self) -> bool {
        !matches!(self.phase, Phase::Idle)
    }

    fn fetch(&mut self, keys: Vec<String>, phase: Phase) {
        self.queue = keys;
        self.waiting.clear();
        self.got.clear();
        self.forbidden = 0;
        self.phase = phase;
    }
}

fn take(slot: &Answer) -> Option<Result<Vec<u8>, String>> {
    slot.lock().unwrap_or_else(|e| e.into_inner()).take()
}

/// Moves fetches along, and acts when a batch is in.
pub fn poll(app: &mut RustGisApp, ctx: &egui::Context) {
    let st = &mut app.ui.zarr;
    if !st.busy() {
        return;
    }
    // Landed answers: a missing file (404, or 403 as S3 says it, by the HTTP status) is left out.
    let mut failed = None;
    let mut forbidden = 0;
    st.waiting.retain(|(k, slot)| match take(slot) {
        Some(Ok(b)) => {
            st.got.insert(k.clone(), b);
            false
        }
        Some(Err(e)) => {
            if !rustgis_engine::cmd::zarr::is_absent(&e) {
                failed = Some(format!("{k}: {e}"));
            } else if cache::http_status(&e) == Some(403) {
                forbidden += 1;
            }
            false
        }
        None => true,
    });
    st.forbidden += forbidden;
    if let Some(e) = failed {
        st.error = Some(e);
        st.phase = Phase::Idle;
        st.queue.clear();
        st.waiting.clear();
        return;
    }
    while st.waiting.len() < AT_ONCE
        && let Some(k) = st.queue.pop()
    {
        let slot: Answer = Slot::default();
        let land = slot.clone();
        cache::download(&format!("{}/{k}", st.root), Box::new(move |r| *land.lock().unwrap_or_else(|e| e.into_inner()) = Some(r)));
        st.waiting.push((k, slot));
    }
    if !st.waiting.is_empty() || !st.queue.is_empty() {
        ctx.request_repaint_after(std::time::Duration::from_millis(50));
        return;
    }
    // The batch is in.
    match std::mem::take(&mut st.phase) {
        Phase::Idle => {}
        Phase::Meta => {
            let docs: Vec<(String, Vec<u8>)> = std::mem::take(&mut st.got).into_iter().collect();
            if docs.is_empty() && st.forbidden > 0 {
                st.error = Some(i18n::fmt("{url}: access denied (HTTP 403)", &[("url", &st.root)]));
                return;
            }
            match zarr::parse_store(&docs) {
                Ok(store) => {
                    let keys = zarr::info_keys(&store);
                    st.store = Some(store);
                    st.fetch(keys, Phase::Labels);
                }
                Err(e) => st.error = Some(e.to_string()),
            }
        }
        Phase::Labels => {
            if let Some(store) = &st.store {
                st.info = Some(zarr::info(store, &st.got));
                st.variable = 0;
                st.steps.clear();
            }
            st.got.clear();
        }
        Phase::Read(job) => {
            let (plan, sel, name) = *job;
            let got = std::mem::take(&mut st.got);
            let crs = match st.crs.trim() {
                "" => Ok(None),
                c => Crs::parse(c).map(Some),
            };
            let raster = crs.map_err(|e| e.to_string()).and_then(|crs| {
                let store = st.store.as_ref().ok_or("the store is gone")?;
                let r = zarr::assemble(store, &plan, &got, &name, crs).map_err(|e| e.to_string())?;
                geotiff::write(&r, 0).map_err(|e| e.to_string())
            });
            match raster {
                Ok(tif) => {
                    let id = app.session.download_started();
                    app.session.download_landed(id, tif);
                    let index: serde_json::Map<String, Value> = sel.index.iter().map(|(d, i)| (d.clone(), json!(i))).collect();
                    let root = app.ui.zarr.root.clone();
                    let p = json!({"url": root, "variable": sel.variable, "index": index, "ran": id, "name": name});
                    // A failure here (the read handed over, then refused) is said in the dialog.
                    match app.run("layer.addZarr", p) {
                        Ok(_) => app.map.invalidate(),
                        Err(e) => app.ui.zarr.error = Some(e),
                    }
                }
                Err(e) => app.ui.zarr.error = Some(e),
            }
        }
    }
}

pub(crate) fn connect(app: &mut RustGisApp) {
    let st = &mut app.ui.zarr;
    let a = st.address.trim().trim_end_matches('/').to_string();
    st.error = None;
    st.info = None;
    st.store = None;
    if a.is_empty() {
        st.error = Some(crate::tl!("Give a Zarr store's address or folder").to_string());
        return;
    }
    let lower = a.to_ascii_lowercase();
    st.root = a.clone();
    st.web = lower.starts_with("http://") || lower.starts_with("https://");
    if st.web {
        let keys = zarr::METADATA_KEYS.iter().map(|k| k.to_string()).collect();
        st.fetch(keys, Phase::Meta);
        return;
    }
    if cfg!(target_arch = "wasm32") {
        st.error = Some(crate::tl!("In the web app, give the store's web address (https://…)").to_string());
        return;
    }
    match app.run("raster.zarrInfo", json!({"path": a})) {
        Ok(v) => {
            let st = &mut app.ui.zarr;
            st.info = Some(v);
            st.variable = 0;
            st.steps.clear();
        }
        Err(e) => app.ui.zarr.error = Some(e),
    }
}

/// The dialog. Returns whether it should close.
pub fn dialog(app: &mut RustGisApp, ui: &mut egui::Ui) -> bool {
    ui.label(
        egui::RichText::new(crate::tl!(
            "A Zarr store (format 2 or 3) by its web address, with consolidated metadata, or a folder on this computer. A variable's map is read for the steps picked of its other dimensions, such as time."
        ))
        .weak(),
    );
    let mut go = false;
    ui.horizontal(|ui| {
        let r = ui.add(egui::TextEdit::singleline(&mut app.ui.zarr.address).hint_text("https://…/store.zarr").desired_width(380.0));
        go |= r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        #[cfg(not(target_arch = "wasm32"))]
        if ui.button(crate::tl!("Folder…")).clicked()
            && let Some(d) = rfd::FileDialog::new().pick_folder()
        {
            app.ui.zarr.address = d.display().to_string();
            go = true;
        }
        go |= ui.add_enabled(!app.ui.zarr.busy(), egui::Button::new(crate::tl!("Connect"))).clicked();
    });
    if go {
        connect(app);
    }
    let busy = app.ui.zarr.busy();
    if busy {
        ui.horizontal(|ui| {
            ui.spinner();
            let n = app.ui.zarr.queue.len() + app.ui.zarr.waiting.len();
            ui.label(i18n::fmt("Reading the store… ({n} files to go)", &[("n", &n.to_string())]));
        });
    }
    if let Some(e) = &app.ui.zarr.error {
        ui.label(egui::RichText::new(e).color(ui.visuals().error_fg_color));
    }
    let Some(info) = app.ui.zarr.info.clone() else { return false };
    let vars: Vec<Value> = info["variables"].as_array().cloned().unwrap_or_default();
    if vars.is_empty() {
        ui.label(crate::tl!("The store has no variables with two or more dimensions."));
        return false;
    }
    let st = &mut app.ui.zarr;
    st.variable = st.variable.min(vars.len() - 1);
    let title = |v: &Value| {
        let mut t = v["name"].as_str().unwrap_or("").to_string();
        if let Some(l) = v["longName"].as_str() {
            t = format!("{t} — {l}");
        }
        if let Some(u) = v["units"].as_str() {
            t = format!("{t} ({u})");
        }
        t
    };
    egui::Grid::new("zarr").num_columns(2).spacing(vec2(10.0, 8.0)).show(ui, |ui| {
        ui.label(crate::tl!("Variable"));
        egui::ComboBox::from_id_salt("zarr_var").selected_text(title(&vars[st.variable])).width(380.0).show_ui(ui, |ui| {
            for (i, v) in vars.iter().enumerate() {
                if ui.selectable_value(&mut st.variable, i, title(v)).changed() {
                    st.steps.clear();
                }
            }
        });
        ui.end_row();
        let var = &vars[st.variable];
        for d in var["dims"].as_array().into_iter().flatten().filter(|d| d["map"] != json!(true)) {
            let (name, size) = (d["name"].as_str().unwrap_or("").to_string(), d["size"].as_u64().unwrap_or(1).max(1));
            ui.label(name.clone());
            let step = st.steps.entry(name).or_insert(0);
            ui.horizontal(|ui| {
                ui.add(egui::Slider::new(step, 0..=size - 1).show_value(true));
                let label = d["labels"]
                    .get(*step as usize)
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .or_else(|| Some(i18n::fmt("{first} … {last}", &[("first", d["first"].as_str()?), ("last", d["last"].as_str()?)])));
                if let Some(l) = label {
                    ui.label(egui::RichText::new(l).weak());
                }
            });
            ui.end_row();
        }
        ui.label(crate::tl!("Coordinate system"));
        ui.add(egui::TextEdit::singleline(&mut st.crs).hint_text(crate::tl!("for a store that doesn't say")).desired_width(200.0));
        ui.end_row();
    });
    if !vars[st.variable]["readable"].as_bool().unwrap_or(true) {
        ui.label(egui::RichText::new(vars[st.variable]["why"].as_str().unwrap_or("")).color(ui.visuals().warn_fg_color));
    }
    let var = vars[st.variable]["name"].as_str().unwrap_or("").to_string();
    let mut index: Vec<(String, u64)> = st.steps.iter().map(|(d, i)| (d.clone(), *i)).collect();
    index.sort();
    // The steps' labels name the layer (`tas 1850-01-15`).
    let labels: Vec<String> = vars[st.variable]["dims"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|d| d["map"] != json!(true))
        .filter_map(|d| {
            let step = st.steps.get(d["name"].as_str()?).copied().unwrap_or(0);
            d["labels"].get(step as usize).and_then(Value::as_str).map(str::to_string)
        })
        .collect();
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        if ui.add_enabled(!busy, egui::Button::new(crate::tl!("Add to Map"))).clicked() {
            add(app, zarr::Selection { variable: var.clone(), index }, &labels);
        }
    });
    // The dialog stays open, to add other steps.
    false
}

/// Adds the slice: through the command for a folder, or fetches its chunks first for a web store.
fn add(app: &mut RustGisApp, sel: zarr::Selection, labels: &[String]) {
    let st = &mut app.ui.zarr;
    st.error = None;
    let name = rustgis_engine::cmd::zarr::layer_name(&st.root, &sel, labels);
    if !st.web {
        let index: serde_json::Map<String, Value> = sel.index.iter().map(|(d, i)| (d.clone(), json!(i))).collect();
        let mut p = json!({"path": st.root, "variable": sel.variable, "index": index, "name": name});
        if !st.crs.trim().is_empty() {
            p["crs"] = json!(st.crs.trim());
        }
        match app.run("layer.addZarr", p) {
            Ok(_) => app.map.invalidate(),
            Err(e) => app.ui.zarr.error = Some(e),
        }
        return;
    }
    let Some(store) = &st.store else { return };
    match zarr::plan(store, &sel) {
        Ok(plan) => {
            let keys = plan.keys.clone();
            st.fetch(keys, Phase::Read(Box::new((plan, sel, name))));
        }
        Err(e) => st.error = Some(e.to_string()),
    }
}
