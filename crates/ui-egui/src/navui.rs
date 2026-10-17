//! Directions (Home › Navigate › Directions): a route planned between places in a pane, through a
//! Valhalla routing server (off the frame, through the tile cache's downloads: a thread natively,
//! fetch on the web), drawn on the map with its maneuvers, and replayed as a simulated drive that
//! the map follows. Nothing is sent before the user agrees to share the places with the server.

use egui::{Align2, Color32, Pos2, Rect, Stroke, Ui, vec2};
use rustgis_engine::geom::Coord;
use rustgis_engine::render::rustgis_tiles::{
    cache,
    geocode::{Geocoder, Place},
    routing::{self, Avoid, Mode, Request, Route, Router},
};
use serde_json::json;
use web_time::Instant;

use crate::theme::{Tokens, regular, semibold};
use crate::{RustGisApp, Slot, i18n};

type Answer = Slot<Result<Vec<u8>, String>>;

/// The start, a stop, or the destination.
#[derive(Clone, Debug, Default)]
pub struct Waypoint {
    /// What the user typed, or the place's name.
    pub text: String,
    /// Where it is, once typed as coordinates, found, or picked on the map.
    pub at: Option<Coord>,
}

/// A simulated drive along the chosen route.
#[derive(Clone, Debug)]
pub struct Sim {
    /// Seconds of the route's travel time behind.
    pub t: f64,
    /// How many times faster than the route's own times.
    pub speed: f64,
    pub playing: bool,
    /// When the last frame advanced it (the app's clock, seconds).
    last: Option<f64>,
}

/// The Directions pane's state.
pub struct State {
    pub points: Vec<Waypoint>,
    pub mode: Mode,
    pub avoid: Avoid,
    pub imperial: bool,
    /// Which place the next map click sets.
    pub picking: Option<usize>,
    /// Whether the user agreed to send places to the routing server (read from the preferences
    /// once).
    pub consent: bool,
    loaded: bool,
    /// A request under way, and what it asked.
    pending: Option<(Answer, String)>,
    /// What the routes shown (or the request under way) were asked for.
    asked: String,
    pub routes: Vec<Route>,
    /// The server's answer, kept for Save as Layer (which reads it again rather than asking).
    answer: String,
    pub chosen: usize,
    pub error: Option<String>,
    pub show_steps: bool,
    /// The step highlighted on the map (clicked in the list, or under way in a drive).
    pub step: Option<usize>,
    pub sim: Option<Sim>,
    /// A place being looked up by name: for which row, what it said then and how many places
    /// there were (the answer lands only if that row still waits on that text), the server, and
    /// its answer.
    lookup: Option<(usize, String, usize, Geocoder, Answer)>,
    last_lookup: Option<Instant>,
    /// Places a name matched, for which row, to pick from.
    pub matches: Option<(usize, Vec<Place>)>,
    pub saved: bool,
    /// Whether the pane was open last frame.
    open: bool,
}

impl Default for State {
    fn default() -> Self {
        State {
            points: vec![Waypoint::default(), Waypoint::default()],
            mode: Mode::Drive,
            avoid: Avoid::default(),
            imperial: false,
            picking: None,
            consent: false,
            loaded: false,
            pending: None,
            asked: String::new(),
            routes: Vec::new(),
            answer: String::new(),
            chosen: 0,
            error: None,
            show_steps: true,
            step: None,
            sim: None,
            lookup: None,
            last_lookup: None,
            matches: None,
            saved: false,
            open: false,
        }
    }
}

const START: Color32 = Color32::from_rgb(0x16, 0xA3, 0x4A);
const STOP: Color32 = Color32::from_rgb(0xF5, 0x9E, 0x0B);
const END: Color32 = Color32::from_rgb(0xDC, 0x26, 0x26);
const ROUTE: Color32 = Color32::from_rgb(0x25, 0x63, 0xEB);
const CASING: Color32 = Color32::from_rgb(0x1E, 0x3A, 0x8A);
const SPEEDS: [f64; 5] = [1.0, 2.0, 4.0, 8.0, 16.0];

impl State {
    /// The colour of place `i`'s marker.
    fn color(&self, i: usize) -> Color32 {
        if i == 0 {
            START
        } else if i + 1 == self.points.len() {
            END
        } else {
            STOP
        }
    }

    /// The route shown.
    pub fn route(&self) -> Option<&Route> {
        self.routes.get(self.chosen)
    }

    /// What a request for the places and options now would ask, in `language` (empty while a
    /// place is missing).
    fn key(&self, language: &str) -> String {
        if self.points.iter().any(|p| p.at.is_none()) {
            return String::new();
        }
        let pts: Vec<String> = self.points.iter().filter_map(|p| p.at).map(|c| format!("{:.6},{:.6}", c.x, c.y)).collect();
        format!("{}|{}|{:?}|{}|{language}", pts.join(";"), self.mode.id(), self.avoid, self.imperial)
    }

    /// Forgets the routes (a place or an option changed).
    fn clear_routes(&mut self) {
        self.routes.clear();
        self.answer.clear();
        self.chosen = 0;
        self.step = None;
        self.sim = None;
        self.saved = false;
        self.error = None;
    }
}

fn fetch(url: &str) -> Answer {
    let slot: Answer = Slot::default();
    let land = slot.clone();
    cache::download(url, Box::new(move |r| *land.lock().unwrap_or_else(|e| e.into_inner()) = Some(r)));
    slot
}

fn take(slot: &Answer) -> Option<Result<Vec<u8>, String>> {
    slot.lock().unwrap_or_else(|e| e.into_inner()).take()
}

/// Reads the pane's preferences once.
fn load(app: &mut RustGisApp) {
    if app.ui.nav.loaded {
        return;
    }
    app.ui.nav.loaded = true;
    let pref = |k: &str| app.services.load_pref.as_ref().and_then(|load| load(k));
    app.ui.nav.consent = pref("directions_consent").as_deref() == Some("1");
    app.ui.nav.imperial = pref("directions_units").as_deref() == Some("miles");
    if let Some(m) = pref("directions_mode").and_then(|m| Mode::parse(&m).ok()) {
        app.ui.nav.mode = m;
    }
}

fn save(app: &RustGisApp, k: &str, v: &str) {
    if let Some(save) = &app.services.save_pref {
        save(k, v);
    }
}

/// Sets place `i` (the start for `Some(0)`, the destination for `None`) and opens the pane: the
/// map's Directions From Here and To Here.
pub fn seed(app: &mut RustGisApp, i: Option<usize>, c: Coord) {
    load(app);
    let st = &mut app.ui.nav;
    let last = st.points.len().saturating_sub(1);
    let i = i.unwrap_or(last).min(last);
    if let Some(pt) = st.points.get_mut(i) {
        *pt = Waypoint { text: format!("{:.5}, {:.5}", c.y, c.x), at: Some(c) };
    }
    st.picking = st.points.iter().position(|p| p.at.is_none());
    st.matches = None;
    app.ui.right = Some(crate::RightPane::Directions);
}

/// Opens the pane on a route (`ui.pane.directions` with `from`, `to`, `via` as `[lon, lat]` or
/// `"lat, lon"`, and `mode`); places that aren't coordinates are typed in, to look up.
pub fn open_with(app: &mut RustGisApp, p: &serde_json::Value) {
    load(app);
    let place = |v: &serde_json::Value| -> Waypoint {
        let at = match v {
            serde_json::Value::Array(a) => {
                let c = Coord::new(
                    a.first().and_then(|x| x.as_f64()).unwrap_or(f64::NAN),
                    a.get(1).and_then(|x| x.as_f64()).unwrap_or(f64::NAN),
                );
                (c.is_finite() && c.x.abs() <= 180.0 && c.y.abs() <= 90.0).then_some(c)
            }
            serde_json::Value::String(s) => routing::parse_latlon(s),
            _ => None,
        };
        let text = match (v, at) {
            (_, Some(c)) => format!("{:.5}, {:.5}", c.y, c.x),
            (serde_json::Value::String(s), None) => s.chars().take(300).collect(),
            _ => String::new(),
        };
        Waypoint { text, at }
    };
    let mut points = vec![p.get("from").map(place).unwrap_or_default()];
    if let Some(via) = p.get("via").and_then(|v| v.as_array()) {
        points.extend(via.iter().take(routing::MAX_POINTS - 2).map(place));
    }
    points.push(p.get("to").map(place).unwrap_or_default());
    let st = &mut app.ui.nav;
    if let Some(m) = p.get("mode").and_then(|m| m.as_str()).and_then(|m| Mode::parse(m).ok()) {
        st.mode = m;
    }
    st.points = points;
    st.matches = None;
    st.picking = st.points.iter().position(|p| p.at.is_none() && p.text.is_empty());
    app.ui.right = Some(crate::RightPane::Directions);
}

/// A map click while a place waits for one: sets it, and moves on to the next place missing.
/// Returns whether the click was taken.
/// `other` is another route under the click, which it picks.
pub fn click(app: &mut RustGisApp, c: Coord, other: Option<usize>) -> bool {
    let st = &mut app.ui.nav;
    if let Some(i) = st.picking.filter(|i| *i < st.points.len()) {
        if let Some(pt) = st.points.get_mut(i) {
            *pt = Waypoint { text: format!("{:.5}, {:.5}", c.y, c.x), at: Some(c) };
        }
        st.picking = st.points.iter().position(|p| p.at.is_none());
        st.matches = None;
        return true;
    }
    // A click on another route picks it.
    if app.ui.right == Some(crate::RightPane::Directions)
        && st.sim.is_none()
        && let Some(k) = other.filter(|k| *k < st.routes.len())
    {
        st.chosen = k;
        st.step = None;
        st.saved = false;
        return true;
    }
    false
}

/// Sends a route request when the places or options changed; lands answers and lookups; moves a
/// simulated drive along.
pub fn poll(app: &mut RustGisApp, ctx: &egui::Context) {
    let open = app.ui.right == Some(crate::RightPane::Directions);
    let opened = open && !app.ui.nav.open;
    app.ui.nav.open = open;
    if opened && app.ui.nav.picking.is_none() {
        // Just opened: the first place missing waits for a click.
        app.ui.nav.picking = app.ui.nav.points.iter().position(|p| p.at.is_none());
    }
    if !open {
        // Closed: the drive stops, and the map no longer waits for clicks.
        app.ui.nav.sim = None;
        app.ui.nav.picking = None;
        if app.ui.nav.pending.is_none() && app.ui.nav.lookup.is_none() {
            return;
        }
    }
    load(app);
    // The interface's language for the instructions (the preference may be "auto").
    let language = app.chosen_lang().code().to_string();
    let time = ctx.input(|i| i.time);
    let st = &mut app.ui.nav;
    // A place looked up by name.
    if let Some((row, text, count, g, slot)) = &st.lookup {
        match take(slot) {
            Some(r) => {
                let row = *row;
                // The places changed meanwhile (edited, picked, removed, swapped): the answer is
                // for a place no longer asked about.
                let still = st.points.len() == *count && st.points.get(row).is_some_and(|p| p.at.is_none() && p.text.trim() == text);
                let found = r.and_then(|b| g.parse(&b));
                st.lookup = None;
                match found {
                    _ if !still => {}
                    Ok(p) if p.is_empty() => st.error = Some(tl!("No place matches that address.").to_string()),
                    Ok(p) if p.len() == 1 => {
                        if let (Some(pt), Some(first)) = (st.points.get_mut(row), p.first()) {
                            *pt = Waypoint { text: first.name.clone(), at: Some(Coord::new(first.lon, first.lat)) };
                        }
                        st.picking = st.points.iter().position(|p| p.at.is_none());
                    }
                    Ok(p) => st.matches = Some((row, p)),
                    Err(e) => st.error = Some(e),
                }
            }
            None => ctx.request_repaint_after(std::time::Duration::from_millis(100)),
        }
    }
    // A route answered.
    if let Some((slot, key)) = &st.pending {
        match take(slot) {
            Some(r) => {
                let key = key.clone();
                st.pending = None;
                // Only the answer to what's asked now (the places may have changed meanwhile).
                if key == st.key(&language) {
                    let router = Router { base: routing::VALHALLA.into() };
                    match r.map_err(|e| routing::explain(&e)).and_then(|b| {
                        let routes = router.parse(&b)?;
                        Ok((routes, String::from_utf8_lossy(&b).into_owned()))
                    }) {
                        Ok((routes, text)) => {
                            st.routes = routes;
                            st.answer = text;
                            st.chosen = 0;
                            st.error = None;
                            // Shown, unless the pane was closed meanwhile.
                            if let Some(r) = st.routes.first().filter(|_| open) {
                                let e = r.extent();
                                fit(app, e);
                            }
                        }
                        Err(e) => {
                            st.clear_routes();
                            st.error = Some(e);
                        }
                    }
                }
            }
            None => ctx.request_repaint_after(std::time::Duration::from_millis(100)),
        }
    }
    let st = &mut app.ui.nav;
    // Ask again when the places or options changed.
    let key = st.key(&language);
    // Only while the pane is open (a lookup landing after it closed sends nothing; reopening asks).
    if open && st.consent && st.pending.is_none() && key != st.asked {
        st.clear_routes();
        st.asked = key.clone();
        if !key.is_empty() {
            let req = Request {
                points: st.points.iter().filter_map(|p| p.at).collect(),
                mode: st.mode,
                avoid: st.avoid,
                alternates: true,
                imperial: st.imperial,
                language,
            };
            match Router::new(None).and_then(|r| r.url(&req)) {
                Ok(url) => st.pending = Some((fetch(&url), key)),
                Err(e) => st.error = Some(e),
            }
        }
    }
    // The drive moves on.
    let st = &mut app.ui.nav;
    if let Some(sim) = &mut st.sim
        && let Some(r) = st.routes.get(st.chosen)
    {
        if sim.playing {
            let dt = sim.last.map_or(0.0, |l| (time - l).clamp(0.0, 0.5));
            sim.t = (sim.t + dt * sim.speed).min(r.duration_s);
            sim.last = Some(time);
            if sim.t >= r.duration_s {
                sim.playing = false;
            }
            ctx.request_repaint();
        } else {
            sim.last = None;
        }
        let d = distance_at(r, sim.t);
        st.step = Some(r.step_at(d + 1.0));
        if sim.playing
            && let Some((p, _)) = r.at(d)
        {
            crate::mapview::follow(app, p, time);
        }
    }
}

/// How far along the route a drive is after `t` seconds: through the steps at their own pace,
/// scaled so the route's whole time ends at its end (the steps' times may not add up to it).
pub fn distance_at(r: &Route, t: f64) -> f64 {
    let steps: f64 = r.steps.iter().map(|s| s.duration_s).sum();
    let t = if steps > 0.0 && r.duration_s > 0.0 { t * steps / r.duration_s } else { t };
    let mut elapsed = 0.0;
    for (i, s) in r.steps.iter().enumerate() {
        let next = r.steps.get(i + 1).map_or(r.distance_m.max(s.at_m), |n| n.at_m);
        if t <= elapsed + s.duration_s && s.duration_s > 0.0 {
            return s.at_m + (next - s.at_m) * ((t - elapsed) / s.duration_s).clamp(0.0, 1.0);
        }
        elapsed += s.duration_s;
    }
    r.along.last().copied().unwrap_or(0.0)
}

/// Zooms to an extent (west, south, east, north) with a margin.
fn fit(app: &mut RustGisApp, e: [f64; 4]) {
    let [w, s, ea, n] = e;
    if !(w.is_finite() && s.is_finite() && ea.is_finite() && n.is_finite()) || w > ea || s > n {
        return;
    }
    let (mx, my) = (((ea - w) * 0.12).max(0.002), ((n - s) * 0.12).max(0.002));
    let _ = app.run("map.zoomToExtent", json!({"extent": [w - mx, s - my, ea + mx, n + my]}));
}

/// Looks a typed place up: coordinates are taken as they are; a name goes to the geocoder.
fn resolve(app: &mut RustGisApp, row: usize) {
    let st = &mut app.ui.nav;
    let Some(pt) = st.points.get_mut(row) else { return };
    let text = pt.text.trim().to_string();
    if text.is_empty() {
        return;
    }
    if let Some(c) = routing::parse_latlon(&text) {
        pt.at = Some(c);
        st.picking = st.points.iter().position(|p| p.at.is_none());
        return;
    }
    // The public geocoder asks for a second between requests.
    if st.lookup.is_some() || st.last_lookup.is_some_and(|l| l.elapsed().as_millis() < 1100) {
        st.error = Some(tl!("Searching… try again in a moment.").to_string());
        return;
    }
    match Geocoder::new(None, None) {
        Ok(g) => {
            let slot = fetch(&g.url(&text, 5));
            st.lookup = Some((row, text, st.points.len(), g, slot));
            st.last_lookup = Some(Instant::now());
            st.matches = None;
            st.error = None;
        }
        Err(e) => st.error = Some(e),
    }
}

/// The pane.
pub fn show(app: &mut RustGisApp, ui: &mut Ui, t: &Tokens) {
    load(app);
    if !app.ui.nav.consent {
        consent(app, ui, t);
        return;
    }
    // Travel mode.
    ui.horizontal(|ui| {
        for (m, label) in [(Mode::Drive, "Drive"), (Mode::Bike, "Bike"), (Mode::Walk, "Walk")] {
            if ui.selectable_label(app.ui.nav.mode == m, tl!(label)).clicked() && app.ui.nav.mode != m {
                app.ui.nav.mode = m;
                save(app, "directions_mode", m.id());
            }
        }
    });
    ui.add_space(6.0);
    places(app, ui, t);
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        let n = app.ui.nav.points.len();
        if ui.add_enabled(n < routing::MAX_POINTS, egui::Button::new(tl!("+ Add Stop"))).clicked() {
            app.ui.nav.points.insert(n - 1, Waypoint::default());
            app.ui.nav.picking = Some(n - 1);
        }
        if ui.button(tl!("Swap")).on_hover_text(tl!("Swap the start and the destination")).clicked() {
            app.ui.nav.points.reverse();
            app.ui.nav.matches = None;
        }
    });
    let st = &mut app.ui.nav;
    if st.mode == Mode::Drive {
        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            ui.label(tl!("Avoid"));
            ui.checkbox(&mut st.avoid.tolls, tl!("Tolls"));
            ui.checkbox(&mut st.avoid.highways, tl!("Highways"));
            ui.checkbox(&mut st.avoid.ferries, tl!("Ferries"));
        });
    }
    let was = st.imperial;
    ui.checkbox(&mut st.imperial, tl!("Miles"));
    if st.imperial != was {
        let v = if st.imperial { "miles" } else { "kilometers" };
        save(app, "directions_units", v);
    }
    ui.add_space(6.0);
    status(app, ui, t);
    routes(app, ui, t);
    if app.ui.nav.route().is_some() {
        actions(app, ui, t);
        if app.ui.nav.show_steps {
            steps(app, ui, t);
        }
    }
    ui.add_space(10.0);
    ui.label(
        egui::RichText::new(tl!("Routes by Valhalla (FOSSGIS) on OpenStreetMap data. Travel times don't include live traffic."))
            .small()
            .color(t.text_dim),
    );
}

/// The notice before anything is sent.
fn consent(app: &mut RustGisApp, ui: &mut Ui, t: &Tokens) {
    ui.label(egui::RichText::new(tl!("Directions use an external routing server")).font(semibold(14.0)).color(t.text));
    ui.add_space(6.0);
    ui.label(tl!("Planning a route sends its start, stops and destination to a routing server, and looking up a place by name sends the name to OpenStreetMap's Nominatim."));
    ui.add_space(6.0);
    ui.label(egui::RichText::new(tl!("Data privacy")).strong());
    ui.label(i18n::fmt(
        "The places leave this device for the public Valhalla server at {server}, run by FOSSGIS. Don't continue if your project is bound by data privacy or data sovereignty rules.",
        &[("server", "valhalla1.openstreetmap.de")],
    ));
    ui.add_space(6.0);
    ui.label(egui::RichText::new(tl!("Limits")).strong());
    ui.label(tl!("The public server is free and shared by many: it answers light use, and may ask you to wait."));
    ui.add_space(10.0);
    ui.horizontal(|ui| {
        if ui.button(tl!("Continue")).clicked() {
            app.ui.nav.consent = true;
            save(app, "directions_consent", "1");
        }
        if ui.button(tl!("Cancel")).clicked() {
            app.ui.right = None;
        }
    });
}

/// The start, stops and destination: a box each, with a pick-on-map button.
fn places(app: &mut RustGisApp, ui: &mut Ui, t: &Tokens) {
    let n = app.ui.nav.points.len();
    let mut remove = None;
    let mut look = None;
    for i in 0..n {
        let color = app.ui.nav.color(i);
        let hint = if i == 0 {
            tl!("Choose a start")
        } else if i + 1 == n {
            tl!("Choose a destination")
        } else {
            tl!("Choose a stop")
        };
        ui.horizontal(|ui| {
            let (r, _) = ui.allocate_exact_size(vec2(14.0, 20.0), egui::Sense::hover());
            ui.painter().circle_filled(r.center(), 5.5, color);
            ui.painter().circle_stroke(r.center(), 5.5, Stroke::new(1.5, Color32::WHITE));
            let extra = if i > 0 && i + 1 < n { 3 } else { 2 };
            let w = (ui.available_width() - 34.0 * extra as f32).max(60.0);
            let st = &mut app.ui.nav;
            let Some(pt) = st.points.get_mut(i) else { return };
            let r = ui.add(egui::TextEdit::singleline(&mut pt.text).hint_text(hint).desired_width(w));
            if r.changed() {
                pt.at = None;
            }
            if r.lost_focus() && ui.input(|k| k.key_pressed(egui::Key::Enter)) {
                look = Some(i);
            }
            let picking = st.picking == Some(i);
            if ui.selectable_label(picking, "⌖").on_hover_text(tl!("Pick it on the map")).clicked() {
                st.picking = if picking { None } else { Some(i) };
            }
            if i > 0 && i + 1 < n && ui.button("×").on_hover_text(tl!("Remove this stop")).clicked() {
                remove = Some(i);
            }
            if pt.at.is_some() {
                ui.label(egui::RichText::new("✔").color(t.green));
            }
        });
        // Places a name matched.
        if let Some((row, found)) = &app.ui.nav.matches
            && *row == i
        {
            let mut pick = None;
            ui.label(egui::RichText::new(tl!("Matching places")).small().color(t.text_dim));
            for (k, p) in found.iter().enumerate() {
                if ui
                    .add(egui::Button::new(egui::RichText::new(&p.name).small()).wrap().min_size(vec2(ui.available_width(), 0.0)))
                    .clicked()
                {
                    pick = Some(k);
                }
            }
            if let Some(k) = pick {
                let st = &mut app.ui.nav;
                if let (Some((row, found)), true) = (st.matches.take(), true)
                    && let (Some(p), Some(pt)) = (found.get(k), st.points.get_mut(row))
                {
                    *pt = Waypoint { text: p.name.clone(), at: Some(Coord::new(p.lon, p.lat)) };
                }
                st.picking = st.points.iter().position(|p| p.at.is_none());
            }
        }
    }
    if let Some(i) = remove {
        app.ui.nav.points.remove(i);
        app.ui.nav.picking = None;
        app.ui.nav.matches = None;
    }
    if let Some(i) = look {
        resolve(app, i);
    }
}

/// What's happening, in a line.
fn status(app: &RustGisApp, ui: &mut Ui, t: &Tokens) {
    let st = &app.ui.nav;
    let (text, busy) = if st.lookup.is_some() {
        (tl!("Looking up the place…"), true)
    } else if st.pending.is_some() {
        (tl!("Finding routes…"), true)
    } else if let Some(e) = &st.error {
        ui.label(egui::RichText::new(e).color(t.red));
        return;
    } else if st.picking.is_some() || st.points.iter().any(|p| p.at.is_none()) {
        (tl!("Type an address or coordinates and press Enter, or click the map."), false)
    } else if st.routes.len() > 1 && st.sim.is_none() {
        (tl!("Pick a route in the list, or click a grey route on the map."), false)
    } else if st.routes.is_empty() {
        return;
    } else {
        (tl!("Simulate the drive, or save the route as a layer."), false)
    };
    ui.horizontal_wrapped(|ui| {
        if busy {
            ui.spinner();
        }
        ui.label(egui::RichText::new(text).color(t.text_dim));
    });
}

/// The routes found: time, distance and main roads; the one picked is drawn blue.
fn routes(app: &mut RustGisApp, ui: &mut Ui, t: &Tokens) {
    let st = &app.ui.nav;
    let many = st.routes.len() > 1;
    let mut pick = None;
    for (i, r) in st.routes.iter().enumerate() {
        let chosen = i == st.chosen;
        let fill = if chosen { t.checked } else { t.pane };
        let resp = egui::Frame::NONE
            .fill(fill)
            .stroke(Stroke::new(1.0, if chosen { t.accent } else { t.border }))
            .corner_radius(6.0)
            .inner_margin(egui::Margin::symmetric(10, 6))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(routing::format_duration(r.duration_s)).font(semibold(15.0)).color(t.text));
                    ui.label(egui::RichText::new(routing::format_distance(r.distance_m, st.imperial)).color(t.text_dim));
                    if many && i == 0 {
                        ui.label(egui::RichText::new(tl!("Best")).small().color(t.green));
                    }
                });
                if !r.summary.is_empty() {
                    ui.label(egui::RichText::new(i18n::fmt("Via {roads}", &[("roads", &r.summary)])).small().color(t.text_dim));
                }
            })
            .response
            .interact(egui::Sense::click());
        if resp.clicked() && !chosen {
            pick = Some(i);
        }
        ui.add_space(4.0);
    }
    if let Some(i) = pick {
        let st = &mut app.ui.nav;
        st.chosen = i;
        st.step = None;
        st.sim = None;
        st.saved = false;
    }
}

/// Simulate, Save as Layer, Steps.
fn actions(app: &mut RustGisApp, ui: &mut Ui, t: &Tokens) {
    ui.add_space(4.0);
    let Some(r) = app.ui.nav.route() else { return };
    let (total_s, total_m) = (r.duration_s, r.distance_m);
    let n = r.steps.len();
    let st = &mut app.ui.nav;
    match &mut st.sim {
        None => {
            let (mut simulate, mut keep, mut toggle) = (false, false, false);
            ui.horizontal_wrapped(|ui| {
                simulate = ui.button(tl!("▶ Simulate")).on_hover_text(tl!("Replay the route as a drive the map follows")).clicked();
                let label = if st.saved { tl!("Saved") } else { tl!("Save as Layer") };
                keep = ui.add_enabled(!st.saved, egui::Button::new(label)).clicked();
                let label =
                    if st.show_steps { tl!("Hide Steps").to_string() } else { i18n::fmt("Show {n} Steps", &[("n", &n.to_string())]) };
                toggle = ui.button(label).clicked();
            });
            if simulate {
                st.sim = Some(Sim { t: 0.0, speed: 4.0, playing: true, last: None });
                st.picking = None;
                // Close in on the start, which the map then follows.
                if let Some(c) = app.ui.nav.route().and_then(|r| r.line.first().copied()) {
                    let d = 0.004;
                    let _ = app.run("map.zoomToExtent", json!({"extent": [c.x - d, c.y - d, c.x + d, c.y + d]}));
                }
            } else if toggle {
                st.show_steps = !st.show_steps;
            } else if keep {
                let p = json!({"answer": st.answer, "route": st.chosen, "mode": st.mode.id(), "imperial": st.imperial, "zoom": false});
                if app.run("map.route", p).is_ok() {
                    app.ui.nav.saved = true;
                    app.map.invalidate();
                }
            }
        }
        Some(_) => {
            let (sim_t, imperial) = (st.sim.as_ref().map_or(0.0, |s| s.t), st.imperial);
            let left_s = (total_s - sim_t).max(0.0);
            let left_m = app.ui.nav.route().map_or(0.0, |r| (total_m - distance_at(r, sim_t)).max(0.0));
            ui.label(
                egui::RichText::new(i18n::fmt(
                    "{time} · {distance} to go",
                    &[("time", &routing::format_duration(left_s)), ("distance", &routing::format_distance(left_m, imperial))],
                ))
                .font(semibold(13.0))
                .color(t.text),
            );
            let st = &mut app.ui.nav;
            let mut end = false;
            if let Some(sim) = &mut st.sim {
                let mut frac = if total_s > 0.0 { sim.t / total_s } else { 1.0 };
                if ui.add(egui::Slider::new(&mut frac, 0.0..=1.0).show_value(false)).changed() {
                    sim.t = frac * total_s;
                }
                ui.horizontal(|ui| {
                    let label = if sim.playing { tl!("Pause") } else { tl!("Play") };
                    if ui.button(label).clicked() {
                        if !sim.playing && sim.t >= total_s {
                            sim.t = 0.0;
                        }
                        sim.playing = !sim.playing;
                    }
                    egui::ComboBox::from_id_salt("nav_speed").selected_text(format!("{}×", sim.speed)).width(56.0).show_ui(ui, |ui| {
                        for s in SPEEDS {
                            ui.selectable_value(&mut sim.speed, s, format!("{s}×"));
                        }
                    });
                    end = ui.button(tl!("End")).clicked();
                });
            }
            if end {
                st.sim = None;
                st.step = None;
            }
        }
    }
}

/// The maneuvers, each with its arrow; one clicked is zoomed to.
fn steps(app: &mut RustGisApp, ui: &mut Ui, t: &Tokens) {
    ui.add_space(6.0);
    let Some(r) = app.ui.nav.route() else { return };
    let imperial = app.ui.nav.imperial;
    let current = app.ui.nav.step;
    let driving = app.ui.nav.sim.is_some();
    let mut go = None;
    for (i, s) in r.steps.iter().enumerate() {
        let on = current == Some(i);
        let resp = egui::Frame::NONE
            .fill(if on { t.checked } else { Color32::TRANSPARENT })
            .corner_radius(4.0)
            .inner_margin(egui::Margin::symmetric(4, 3))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    let (rect, _) = ui.allocate_exact_size(vec2(22.0, 22.0), egui::Sense::hover());
                    crate::icons::paint(ui.painter(), rect, maneuver_icon(&s.kind, &s.modifier), t.text, t.accent);
                    ui.vertical(|ui| {
                        ui.add(egui::Label::new(egui::RichText::new(&s.instruction).color(t.text)).wrap());
                        if s.distance_m > 0.0 {
                            ui.label(egui::RichText::new(routing::format_distance(s.distance_m, imperial)).small().color(t.text_dim));
                        }
                    });
                });
            })
            .response
            .interact(egui::Sense::click());
        if driving && on && app.ui.nav.sim.as_ref().is_some_and(|s| s.playing) {
            resp.scroll_to_me(Some(egui::Align::Center));
        }
        if resp.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        if resp.clicked() {
            go = Some((i, s.location));
        }
    }
    if let Some((i, c)) = go {
        app.ui.nav.step = Some(i);
        let d = 0.0015;
        let _ = app.run("map.zoomToExtent", json!({"extent": [c.x - d, c.y - d, c.x + d, c.y + d]}));
    }
}

/// The icon for a maneuver.
pub fn maneuver_icon(kind: &str, modifier: &str) -> &'static str {
    match kind {
        "arrive" => "maneuver_arrive",
        "depart" => "maneuver_depart",
        "roundabout" | "rotary" | "roundabout turn" | "exit roundabout" | "exit rotary" => "maneuver_roundabout",
        "merge" if matches!(modifier, "" | "straight") => "maneuver_merge",
        _ => match modifier {
            "uturn" => "maneuver_uturn",
            "sharp right" => "maneuver_sharp_right",
            "right" => "maneuver_right",
            "slight right" => "maneuver_slight_right",
            "slight left" => "maneuver_slight_left",
            "left" => "maneuver_left",
            "sharp left" => "maneuver_sharp_left",
            _ => "maneuver_straight",
        },
    }
}

/// Draws the routes, the places and the drive on the map. `to_screen` places a
/// longitude/latitude (`None` behind the globe).
pub fn paint(st: &State, p: &egui::Painter, rect: Rect, t: &Tokens, to_screen: impl Fn(Coord) -> Option<Pos2>) {
    let line = |r: &Route, from: f64, to: f64| screen_line(r, from, to, &to_screen);
    let driving = st.sim.as_ref();
    // The other routes, grey, beneath.
    if driving.is_none() {
        for (i, r) in st.routes.iter().enumerate() {
            if i != st.chosen {
                let pts = line(r, f64::MIN, f64::MAX);
                p.add(egui::Shape::line(pts.clone(), Stroke::new(9.0, Color32::from_rgba_unmultiplied(0x47, 0x55, 0x69, 128))));
                p.add(egui::Shape::line(pts, Stroke::new(6.0, Color32::from_rgb(0x94, 0xA3, 0xB8))));
            }
        }
    }
    if let Some(r) = st.route() {
        let done = driving.map(|s| distance_at(r, s.t));
        let mut pts = line(r, done.unwrap_or(f64::MIN), f64::MAX);
        // The way ahead starts at the car itself, not at the next point of the line past it.
        if let Some(at) = done.and_then(|d| r.at(d)).and_then(|(c, _)| to_screen(c)) {
            pts.insert(0, at);
        }
        p.add(egui::Shape::line(pts.clone(), Stroke::new(10.0, CASING)));
        p.add(egui::Shape::line(pts, Stroke::new(6.0, ROUTE)));
        // What's been driven, grey.
        if let Some(d) = done {
            let mut pts = line(r, f64::MIN, d);
            if let Some(at) = r.at(d).and_then(|(c, _)| to_screen(c)) {
                pts.push(at);
            }
            p.add(egui::Shape::line(pts, Stroke::new(6.0, Color32::from_rgba_unmultiplied(0x9C, 0xA3, 0xAF, 210))));
        }
        // The step picked or under way.
        if let Some(s) = st.step.and_then(|i| r.steps.get(i))
            && let Some(sp) = to_screen(s.location)
        {
            p.circle_filled(sp, 7.0, Color32::WHITE);
            p.circle_stroke(sp, 7.0, Stroke::new(3.0, CASING));
        }
    }
    // The places.
    for (i, pt) in st.points.iter().enumerate() {
        if driving.is_some() && i == 0 {
            continue;
        }
        if let Some(sp) = pt.at.and_then(&to_screen) {
            p.circle_filled(sp, 8.0, st.color(i));
            p.circle_stroke(sp, 8.0, Stroke::new(2.5, Color32::WHITE));
        }
    }
    // The car, and the next maneuver in a banner.
    if let (Some(sim), Some(r)) = (driving, st.route()) {
        let d = distance_at(r, sim.t);
        if let Some((c, heading)) = r.at(d)
            && let Some(sp) = to_screen(c)
        {
            puck(p, sp, heading as f32);
        }
        banner(st, r, d, p, rect, t);
    }
}

/// The position: a blue disc with an arrow along the way.
fn puck(p: &egui::Painter, at: Pos2, heading: f32) {
    p.circle_filled(at, 11.0, Color32::WHITE);
    p.circle_filled(at, 9.0, ROUTE);
    let a = heading.to_radians();
    let dir = vec2(a.sin(), -a.cos());
    let side = vec2(-dir.y, dir.x);
    let tip = at + dir * 6.0;
    let pts = [tip, at - dir * 4.0 + side * 4.5, at - dir * 1.5, at - dir * 4.0 - side * 4.5];
    p.add(egui::Shape::convex_polygon(vec![pts[0], pts[1], pts[2]], Color32::WHITE, Stroke::NONE));
    p.add(egui::Shape::convex_polygon(vec![pts[0], pts[2], pts[3]], Color32::WHITE, Stroke::NONE));
}

/// The next maneuver across the top of the map: its arrow, how far, what to do.
fn banner(st: &State, r: &Route, d: f64, p: &egui::Painter, rect: Rect, t: &Tokens) {
    let i = r.step_at(d + 1.0);
    // The maneuver ahead: the next step's, or arriving.
    let Some(next) = r.steps.get(i + 1).or_else(|| r.steps.last()) else { return };
    let to_go = (next.at_m - d).max(0.0);
    let w = (rect.width() - 32.0).clamp(160.0, 520.0);
    let b = Rect::from_min_size(Pos2::new(rect.center().x - w / 2.0, rect.min.y + 12.0), vec2(w, 64.0));
    p.rect_filled(b, 10.0, Color32::from_rgb(0x1E, 0x3A, 0x8A));
    let icon = Rect::from_min_size(b.min + vec2(12.0, 12.0), vec2(40.0, 40.0));
    crate::icons::paint(p, icon, maneuver_icon(&next.kind, &next.modifier), Color32::WHITE, t.accent);
    let x = icon.max.x + 12.0;
    p.text(Pos2::new(x, b.min.y + 20.0), Align2::LEFT_CENTER, routing::format_distance(to_go, st.imperial), semibold(18.0), Color32::WHITE);
    let galley = p.layout(next.instruction.clone(), regular(13.0), Color32::from_rgb(0xDB, 0xEA, 0xFE), b.max.x - x - 12.0);
    p.galley(Pos2::new(x, b.min.y + 34.0), galley, Color32::WHITE);
}

/// The stretch of a route's line from `from` to `to` metres along, on screen, leaving out points
/// within a pixel of the last one kept (a long route has many more points than pixels).
fn screen_line(r: &Route, from: f64, to: f64, to_screen: &impl Fn(Coord) -> Option<Pos2>) -> Vec<Pos2> {
    let a = r.along.partition_point(|&x| x < from);
    let b = r.along.partition_point(|&x| x <= to).max(a);
    let mut out: Vec<Pos2> = Vec::new();
    let mut skipped = None;
    for c in r.line.get(a..b).unwrap_or(&[]) {
        let Some(sp) = to_screen(*c) else { continue };
        if out.last().is_none_or(|l: &Pos2| (*l - sp).length_sq() >= 1.0) {
            out.push(sp);
            skipped = None;
        } else {
            skipped = Some(sp);
        }
    }
    // The line still ends where it ends.
    out.extend(skipped);
    out
}

/// Whether a screen point is within a few pixels of a route's line.
pub fn near(r: &Route, at: Pos2, to_screen: impl Fn(Coord) -> Option<Pos2>) -> bool {
    let mut prev: Option<Pos2> = None;
    for c in &r.line {
        let b = to_screen(*c);
        if let (Some(a), Some(b)) = (prev, b) {
            let ab = b - a;
            let len2 = ab.length_sq();
            let k = if len2 > 0.0 { ((at - a).dot(ab) / len2).clamp(0.0, 1.0) } else { 0.0 };
            if (a + ab * k - at).length() <= 7.0 {
                return true;
            }
        }
        prev = b;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maneuvers_have_icons() {
        assert_eq!(maneuver_icon("turn", "left"), "maneuver_left");
        assert_eq!(maneuver_icon("arrive", ""), "maneuver_arrive");
        assert_eq!(maneuver_icon("merge", "slight left"), "maneuver_slight_left");
        assert_eq!(maneuver_icon("merge", ""), "maneuver_merge");
        assert_eq!(maneuver_icon("exit roundabout", "right"), "maneuver_roundabout");
        assert_eq!(maneuver_icon("new name", "weird"), "maneuver_straight");
        for name in ["maneuver_left", "maneuver_arrive", "maneuver_merge", "maneuver_roundabout", "maneuver_straight", "directions"] {
            assert!(crate::icons::ALL.contains(&name), "{name}");
        }
    }

    #[test]
    fn a_drive_moves_at_each_steps_pace() {
        let line = vec![Coord::new(0.0, 0.0), Coord::new(0.001, 0.0), Coord::new(0.002, 0.0)];
        let step = |at_m: f64, duration_s: f64| routing::Step {
            kind: "turn".into(),
            modifier: String::new(),
            instruction: String::new(),
            road: String::new(),
            location: Coord::new(0.0, 0.0),
            at_m,
            distance_m: 100.0,
            duration_s,
            voice: Vec::new(),
        };
        let r = Route {
            along: vec![0.0, 100.0, 200.0],
            line,
            distance_m: 200.0,
            duration_s: 30.0,
            summary: String::new(),
            steps: vec![step(0.0, 10.0), step(100.0, 20.0), step(200.0, 0.0)],
            stops_m: vec![200.0],
        };
        assert_eq!(distance_at(&r, 0.0), 0.0);
        assert!((distance_at(&r, 5.0) - 50.0).abs() < 1e-9);
        assert!((distance_at(&r, 20.0) - 150.0).abs() < 1e-9);
        assert_eq!(distance_at(&r, 1e9), 200.0);
        // The route's time is the drive's, whatever its steps' times add up to.
        let slow = Route { duration_s: 60.0, ..r.clone() };
        assert!((distance_at(&slow, 10.0) - 50.0).abs() < 1e-9);
        assert_eq!(distance_at(&slow, 60.0), 200.0);
    }
}
