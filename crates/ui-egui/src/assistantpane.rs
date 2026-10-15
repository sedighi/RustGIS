//! The AI Assistant pane: a chat with a model of the user's choosing, which acts on the map by
//! running commands (see the `rustgis-assistant` crate). Requests go out off the frame: on a
//! thread on the desktop, through `Services::http` (the browser's fetch) on the web.

use egui::{Align, Layout, RichText, Ui, vec2};
use rustgis_assistant::{Agent, Config, Entry, HttpRequest, HttpResponse, PRESETS, Step, provider};

use crate::theme::{Tokens, regular, semibold};
use crate::{RustGisApp, Slot, icons};

type Reply = Slot<Result<HttpResponse, String>>;

/// Example requests offered in an empty conversation.
const SUGGESTIONS: &[&str] = &[
    "Which 5 countries have the largest GDP?",
    "Zoom to Tokyo",
    "Label the cities with their names",
    "Buffer the cities by 200 km",
    "Switch to a dark basemap",
];

#[derive(Default)]
pub struct AssistantState {
    agent: Option<Agent>,
    pub input: String,
    /// A message to send on the next frame (`ui.pane.assistant {"ask": …}`).
    pub queued: Option<String>,
    /// The request out: its generation and where its response lands.
    inflight: Option<(u64, Reply)>,
    /// The settings form is open.
    pub settings: bool,
    show_key: bool,
    /// Keys are saved with the preferences only when the user says so.
    remember_key: bool,
    models: Vec<String>,
    models_slot: Option<Reply>,
    models_error: Option<String>,
    /// Earlier messages, for Up/Down in the composer.
    history: Vec<String>,
    history_pos: Option<usize>,
    /// "Allow for this conversation" in the approval card.
    always: bool,
    loaded: bool,
    /// Settings changed and not yet saved: they are saved once no settings field is being typed in.
    unsaved: bool,
}

/// The settings fields typed into (saved when none has focus).
const FIELDS: [&str; 3] = ["assistant_model", "assistant_url", "assistant_key"];

fn typing(ctx: &egui::Context) -> bool {
    ctx.memory(|m| FIELDS.iter().any(|f| m.has_focus(egui::Id::new(f))))
}

fn pref(app: &RustGisApp, key: &str) -> Option<String> {
    app.services.load_pref.as_ref().and_then(|l| l(key)).filter(|v| !v.is_empty())
}

fn save_pref(app: &RustGisApp, key: &str, value: &str) {
    if let Some(s) = &app.services.save_pref {
        s(key, value);
    }
}

/// A provider's settings: its defaults and the environment, then what the user saved.
fn load_config(app: &RustGisApp, provider: &str) -> Config {
    let mut c = Config::from_env(provider);
    if let Some(m) = pref(app, &format!("assistant_model_{provider}")) {
        c.model = m;
    }
    if let Some(u) = pref(app, &format!("assistant_url_{provider}")) {
        c.base_url = u;
    }
    if let Some(k) = pref(app, &format!("assistant_key_{provider}")) {
        c.key = k;
    }
    c
}

fn ensure_loaded(app: &mut RustGisApp) {
    if app.assistant.loaded {
        return;
    }
    app.assistant.loaded = true;
    app.assistant.remember_key = pref(app, "assistant_remember_key").as_deref() == Some("on");
    let provider = pref(app, "assistant_provider")
        .or_else(|| std::env::var("RUSTGIS_ASSISTANT_PROVIDER").ok())
        .filter(|p| provider::preset(p).is_some())
        // The first provider with a key in the environment.
        .or_else(|| PRESETS.iter().find(|p| !Config::from_env(p.id).key.is_empty()).map(|p| p.id.to_string()))
        .unwrap_or_else(|| "anthropic".into());
    let mut cfg = load_config(app, &provider);
    if let Ok(m) = std::env::var("RUSTGIS_ASSISTANT_MODEL")
        && !m.trim().is_empty()
        && pref(app, &format!("assistant_model_{provider}")).is_none()
    {
        cfg.model = m.trim().to_string();
    }
    app.assistant.settings = cfg.missing().is_some();
    app.assistant.agent = Some(Agent::new(cfg, &app.session));
}

fn save_config(app: &RustGisApp, c: &Config) {
    save_pref(app, "assistant_provider", &c.provider);
    save_pref(app, &format!("assistant_model_{}", c.provider), c.model.trim());
    save_pref(app, &format!("assistant_url_{}", c.provider), c.base_url.trim());
    // A key from the environment stays there.
    let from_env = Config::from_env(&c.provider).key;
    let keep = app.assistant.remember_key && c.key.trim() != from_env;
    save_pref(app, &format!("assistant_key_{}", c.provider), if keep { c.key.trim() } else { "" });
}

/// Sends a request off the frame.
fn dispatch(app: &mut RustGisApp, req: HttpRequest, ctx: &egui::Context) -> Reply {
    let slot: Reply = Slot::default();
    let fail = |slot: &Reply, e: String| {
        if let Ok(mut s) = slot.lock() {
            *s = Some(Err(e));
        }
    };
    if !provider::allowed_url(&req.url) {
        fail(
            &slot,
            crate::i18n::fmt("{url}: only HTTPS addresses, or plain HTTP on this computer or network, are allowed", &[("url", &req.url)]),
        );
        return slot;
    }
    if let Some(http) = &app.services.http {
        http(req, slot.clone());
        return slot;
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let (out, ctx) = (slot.clone(), ctx.clone());
        std::thread::spawn(move || {
            let r = rustgis_assistant::http::send(&req);
            if let Ok(mut s) = out.lock() {
                *s = Some(r);
            }
            ctx.request_repaint();
        });
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (req, ctx);
        fail(&slot, "this build can't reach AI providers".into());
    }
    slot
}

fn take(slot: &Reply) -> Option<Result<HttpResponse, String>> {
    slot.lock().ok().and_then(|mut s| s.take())
}

/// Carries out what the agent asked for.
fn follow(app: &mut RustGisApp, step: Step, ctx: &egui::Context) {
    app.map.invalidate();
    if let Step::Send(req) = step {
        let generation = app.assistant.agent.as_ref().map_or(0, Agent::generation);
        let slot = dispatch(app, req, ctx);
        app.assistant.inflight = Some((generation, slot));
    }
}

/// Runs `f` on the agent and the session; errors of the commands it ran show in the transcript,
/// not as notices too.
fn with_agent(app: &mut RustGisApp, f: impl FnOnce(&mut Agent, &mut rustgis_engine::Session) -> Step) -> Option<Step> {
    let mut agent = app.assistant.agent.take()?;
    let before = app.session.notices.len();
    let step = f(&mut agent, &mut app.session);
    let mut i = 0;
    app.session.notices.retain(|n| {
        i += 1;
        i <= before || !n.error
    });
    app.assistant.agent = Some(agent);
    Some(step)
}

/// Per-frame: responses that arrived, a message queued by a command, and settings to save.
pub fn poll(app: &mut RustGisApp, ctx: &egui::Context) {
    if app.assistant.unsaved && !typing(ctx) {
        app.assistant.unsaved = false;
        save_pref(app, "assistant_remember_key", if app.assistant.remember_key { "on" } else { "off" });
        if let Some(cfg) = app.assistant.agent.as_ref().map(|a| a.config.clone()) {
            save_config(app, &cfg);
        }
    }
    if let Some(text) = app.assistant.queued.take() {
        ensure_loaded(app);
        if app.assistant.agent.as_ref().is_some_and(Agent::busy) {
            app.session.notify(tl!("The assistant is busy; stop it or wait for it to finish."), true);
        } else {
            send(app, text, ctx);
        }
    }
    if let Some((generation, slot)) = &app.assistant.inflight
        && let Some(r) = take(slot)
    {
        let generation = *generation;
        app.assistant.inflight = None;
        if let Some(step) = with_agent(app, |a, s| a.receive(generation, r, s)) {
            follow(app, step, ctx);
        }
    }
    if let Some(slot) = &app.assistant.models_slot
        && let Some(r) = take(slot)
    {
        app.assistant.models_slot = None;
        let api = app.assistant.agent.as_ref().map_or(rustgis_assistant::Api::OpenAi, |a| a.config.api());
        match r.and_then(|r| provider::parse_models(api, &r)) {
            Ok(m) => {
                app.assistant.models = m;
                app.assistant.models_error = None;
            }
            Err(e) => app.assistant.models_error = Some(e),
        }
    }
}

fn send(app: &mut RustGisApp, text: String, ctx: &egui::Context) {
    let text = text.trim().to_string();
    if text.is_empty() {
        return;
    }
    app.assistant.history.retain(|h| *h != text);
    app.assistant.history.push(text.clone());
    app.assistant.history_pos = None;
    app.assistant.always = false;
    let session = &app.session;
    let Some(step) = app.assistant.agent.as_mut().map(|a| a.ask(&text, session)) else { return };
    app.assistant.input.clear();
    follow(app, step, ctx);
}

pub fn show(app: &mut RustGisApp, ui: &mut Ui, t: &Tokens) {
    ensure_loaded(app);
    let ctx = ui.ctx().clone();
    let busy = app.assistant.agent.as_ref().is_some_and(Agent::busy);
    // The model and settings, at the top.
    egui::Frame::NONE.inner_margin(egui::Margin::symmetric(10, 6)).show(ui, |ui| {
        ui.horizontal(|ui| {
            let (label, model) = app
                .assistant
                .agent
                .as_ref()
                .map(|a| (provider::preset(&a.config.provider).map_or("", |p| p.label), a.config.model.clone()))
                .unwrap_or_default();
            let (r, _) = ui.allocate_exact_size(vec2(16.0, 16.0), egui::Sense::hover());
            icons::paint(ui.painter(), r, "assistant", t.text_dim, t.accent);
            let model_label = egui::Label::new(RichText::new(format!("{} · {}", tl!(label), model)).small().color(t.text_dim))
                .truncate()
                .sense(egui::Sense::click());
            // Room for the two buttons; a long model name is cut short.
            let w = (ui.available_width() - 2.0 * (26.0 + ui.spacing().item_spacing.x)).max(0.0);
            if ui
                .allocate_ui(vec2(w, 16.0), |ui| ui.add(model_label))
                .inner
                .on_hover_text(tl!("The model answering; click to change it"))
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .clicked()
            {
                app.assistant.settings = !app.assistant.settings;
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if crate::widgets::icon_button(ui, "settings", tl!("Assistant settings")).clicked() {
                    app.assistant.settings = !app.assistant.settings;
                }
                let has_chat = app.assistant.agent.as_ref().is_some_and(|a| !a.transcript.is_empty());
                if has_chat && crate::widgets::icon_button(ui, "delete", tl!("New conversation")).clicked() {
                    if let Some(a) = app.assistant.agent.as_mut() {
                        a.clear();
                    }
                    app.assistant.inflight = None;
                }
            });
        });
    });
    if app.assistant.settings {
        egui::Frame::NONE.inner_margin(egui::Margin::symmetric(12, 4)).show(ui, |ui| settings(app, ui, t));
        ui.separator();
    }
    // The composer, at the bottom.
    egui::Panel::bottom("assistant_composer").frame(egui::Frame::NONE.fill(t.pane).inner_margin(egui::Margin::symmetric(10, 8))).show(
        ui,
        |ui| {
            composer(app, ui, t, busy, &ctx);
        },
    );
    // The conversation in between.
    egui::ScrollArea::vertical().auto_shrink([false, false]).stick_to_bottom(true).show(ui, |ui| {
        egui::Frame::NONE.inner_margin(egui::Margin::symmetric(12, 8)).show(ui, |ui| {
            // Text in a row (a list item, a quote, the user's message) wraps too, instead of widening the pane.
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Wrap);
            transcript(app, ui, t, &ctx);
        });
    });
}

fn settings(app: &mut RustGisApp, ui: &mut Ui, t: &Tokens) {
    let Some(mut cfg) = app.assistant.agent.as_ref().map(|a| a.config.clone()) else { return };
    let before = (cfg.clone(), app.assistant.remember_key);
    ui.label(RichText::new(tl!("AI provider")).font(semibold(12.5)));
    egui::Grid::new("assistant_settings").num_columns(2).spacing(vec2(8.0, 6.0)).show(ui, |ui| {
        ui.label(tl!("Provider"));
        let current = provider::preset(&cfg.provider).map_or("", |p| p.label);
        egui::ComboBox::from_id_salt("assistant_provider").selected_text(tl!(current)).width(180.0).show_ui(ui, |ui| {
            for p in PRESETS {
                if ui.selectable_label(cfg.provider == p.id, tl!(p.label)).clicked() && cfg.provider != p.id {
                    cfg = load_config(app, p.id);
                    app.assistant.models.clear();
                    app.assistant.models_error = None;
                }
            }
        });
        ui.end_row();
        ui.label(tl!("Model"));
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(&mut cfg.model).id(egui::Id::new(FIELDS[0])).desired_width(150.0).hint_text(tl!("model id")));
            if !app.assistant.models.is_empty() {
                egui::ComboBox::from_id_salt("assistant_models").selected_text("").width(24.0).height(320.0).show_ui(ui, |ui| {
                    for m in &app.assistant.models {
                        if ui.selectable_label(cfg.model == *m, m).clicked() {
                            cfg.model = m.clone();
                        }
                    }
                });
            }
            let loading = app.assistant.models_slot.is_some();
            if ui
                .add_enabled(!loading, egui::Button::new(if loading { "…" } else { "↻" }))
                .on_hover_text(tl!("List the provider's models"))
                .clicked()
            {
                let req = provider::models_request(&cfg);
                let ctx = ui.ctx().clone();
                app.assistant.models_slot = Some(dispatch(app, req, &ctx));
            }
        });
        ui.end_row();
        ui.label(tl!("Address"));
        ui.add(egui::TextEdit::singleline(&mut cfg.base_url).id(egui::Id::new(FIELDS[1])).desired_width(200.0));
        ui.end_row();
        let preset = provider::preset(&cfg.provider);
        ui.label(tl!("API key"));
        ui.horizontal(|ui| {
            let hint = if preset.is_some_and(|p| p.keyless) { tl!("not needed") } else { tl!("paste your key") };
            ui.add(
                egui::TextEdit::singleline(&mut cfg.key)
                    .id(egui::Id::new(FIELDS[2]))
                    .password(!app.assistant.show_key)
                    .desired_width(150.0)
                    .hint_text(hint),
            );
            let eye = if app.assistant.show_key { tl!("Hide") } else { tl!("Show") };
            if ui.small_button(eye).clicked() {
                app.assistant.show_key = !app.assistant.show_key;
            }
        });
        ui.end_row();
    });
    ui.checkbox(&mut app.assistant.remember_key, tl!("Remember the key on this computer"));
    let preset = provider::preset(&cfg.provider);
    let mut notes = Vec::new();
    if let Some(p) = preset.filter(|p| !p.key_env.is_empty()) {
        notes.push(crate::i18n::fmt("On the desktop, the key can also come from {vars}.", &[("vars", &p.key_env.join(", "))]));
    }
    notes.push(
        if app.assistant.remember_key {
            tl!("Saved keys are kept unencrypted in the app's preferences (in the browser's storage on the web).")
        } else {
            tl!("The key is kept for this session only.")
        }
        .to_string(),
    );
    notes.push(
        tl!("Requests send your message and the map's layer names, fields and the rows the assistant reads to the provider.").to_string(),
    );
    for n in notes {
        ui.label(RichText::new(n).small().color(t.text_dim));
    }
    if let Some(p) = preset.filter(|p| !p.key_url.is_empty()) {
        ui.hyperlink_to(RichText::new(if p.keyless { tl!("Get it") } else { tl!("Get a key") }).small(), p.key_url);
    }
    if let Some(e) = &app.assistant.models_error {
        ui.label(RichText::new(e.chars().take(300).collect::<String>()).small().color(t.red));
    }
    if let Some(why) = cfg.missing() {
        ui.label(RichText::new(tl!(why)).small().color(t.amber));
    }
    if (cfg.clone(), app.assistant.remember_key) != before {
        app.assistant.unsaved = true;
        if let Some(a) = app.assistant.agent.as_mut() {
            a.config = cfg;
        }
    }
}

fn composer(app: &mut RustGisApp, ui: &mut Ui, t: &Tokens, busy: bool, ctx: &egui::Context) {
    // An action waiting for approval comes first.
    let pending = app.assistant.agent.as_ref().and_then(|a| a.pending(&app.session));
    if let Some((what, why, can_always)) = pending {
        egui::Frame::NONE.fill(t.amber.gamma_multiply(0.15)).corner_radius(6).inner_margin(8).stroke(egui::Stroke::new(1.0, t.amber)).show(
            ui,
            |ui| {
                ui.label(RichText::new(tl!("Allow this action?")).font(semibold(12.5)));
                ui.label(RichText::new(crate::i18n::fmt("The assistant wants to run this, and {why}:", &[("why", tl!(&why))])).small());
                ui.label(RichText::new(what).monospace().small());
                // Plugin commands, scripts and replacing the project are asked about every time.
                if can_always {
                    ui.checkbox(&mut app.assistant.always, tl!("Allow such actions for this conversation"));
                } else {
                    app.assistant.always = false;
                }
                ui.horizontal(|ui| {
                    let allow = crate::widgets::primary_button(ui, tl!("Allow"), true, vec2(70.0, 26.0)).clicked();
                    let deny = ui.button(tl!("Decline")).clicked();
                    if allow || deny {
                        let always = app.assistant.always;
                        if let Some(step) = with_agent(app, |a, s| a.decide(allow, always, s)) {
                            follow(app, step, ctx);
                        }
                    }
                });
            },
        );
        ui.add_space(6.0);
    }
    let id = egui::Id::new("assistant_input");
    // Enter sends; Shift+Enter starts a new line.
    let focused = ui.memory(|m| m.has_focus(id));
    let mut submit = false;
    if focused && !busy {
        submit = ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter));
        // Up and Down step through earlier messages while the box holds one of them (or nothing).
        let shown = app.assistant.history_pos.and_then(|p| app.assistant.history.get(p)).is_some_and(|h| *h == app.assistant.input);
        if app.assistant.input.is_empty() || shown {
            let n = app.assistant.history.len();
            if ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp)) && n > 0 {
                let p = app.assistant.history_pos.map_or(n - 1, |p| p.saturating_sub(1));
                app.assistant.history_pos = Some(p);
                app.assistant.input = app.assistant.history.get(p).cloned().unwrap_or_default();
            } else if ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown))
                && let Some(p) = app.assistant.history_pos
            {
                app.assistant.history_pos = (p + 1 < n).then_some(p + 1);
                app.assistant.input = app.assistant.history_pos.and_then(|p| app.assistant.history.get(p)).cloned().unwrap_or_default();
            }
        }
    }
    ui.add(
        egui::TextEdit::multiline(&mut app.assistant.input)
            .id(id)
            .desired_rows(2)
            .desired_width(f32::INFINITY)
            .font(regular(13.0))
            .hint_text(tl!("Ask about the map, or tell it what to do…")),
    );
    ui.horizontal(|ui| {
        // Room for the button; the hint gives way in long languages.
        let w = (ui.available_width() - 84.0).max(0.0);
        ui.allocate_ui(vec2(w, 20.0), |ui| {
            ui.add(egui::Label::new(RichText::new(tl!("Enter to send, Shift+Enter for a new line")).small().color(t.text_dim)).truncate());
        });
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if busy {
                if ui.button(tl!("Stop")).clicked() {
                    if let Some(a) = app.assistant.agent.as_mut() {
                        a.stop();
                    }
                    app.assistant.inflight = None;
                }
            } else {
                let can = !app.assistant.input.trim().is_empty();
                if crate::widgets::primary_button(ui, tl!("Send"), can, vec2(64.0, 26.0)).clicked() {
                    submit = true;
                }
            }
        });
    });
    if submit && !busy {
        let text = app.assistant.input.clone();
        send(app, text, ctx);
        ui.memory_mut(|m| m.request_focus(id));
    }
}

fn transcript(app: &mut RustGisApp, ui: &mut Ui, t: &Tokens, ctx: &egui::Context) {
    let Some(agent) = app.assistant.agent.as_ref() else { return };
    if agent.transcript.is_empty() {
        ui.label(RichText::new(tl!("Ask a question about your data, or describe what to do with the map. The assistant runs RustGIS commands for you; every change can be undone.")).color(t.text_dim));
        ui.add_space(8.0);
        let mut chosen = None;
        for s in SUGGESTIONS {
            let b = egui::Button::new(RichText::new(tl!(s)).small()).corner_radius(12);
            if ui.add(b).clicked() {
                chosen = Some(tl!(s).to_string());
            }
        }
        if let Some(text) = chosen {
            send(app, text, ctx);
        }
        return;
    }
    for e in &agent.transcript {
        match e {
            Entry::User(text) => {
                ui.add_space(6.0);
                ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
                    egui::Frame::NONE.fill(t.selection).corner_radius(8).inner_margin(egui::Margin::symmetric(10, 6)).show(ui, |ui| {
                        ui.set_max_width(ui.available_width() * 0.85);
                        ui.add(egui::Label::new(RichText::new(text).color(t.text)).halign(Align::LEFT));
                    });
                });
                ui.add_space(4.0);
            }
            Entry::Assistant(text) => {
                markdown(ui, text, t);
                ui.add_space(4.0);
            }
            Entry::Tool { name, summary, result, error } => {
                let (mark, col) = if *error { ("✗", t.red) } else { ("✓", t.green) };
                let head = format!("{mark} {}", summary.chars().take(90).collect::<String>());
                // A header never wraps: it is cut to the room there is. Opened, it shows the whole call.
                let room = (ui.available_width() - ui.spacing().indent - ui.spacing().button_padding.x).max(0.0);
                let head = egui::WidgetText::from(RichText::new(head).monospace().small().color(col)).into_galley(
                    ui,
                    Some(egui::TextWrapMode::Truncate),
                    room,
                    egui::TextStyle::Button,
                );
                egui::CollapsingHeader::new(head).id_salt(("assistant_tool", name, summary.as_str(), result.len())).show(ui, |ui| {
                    ui.label(RichText::new(summary).monospace().small());
                    ui.label(RichText::new(result.chars().take(2000).collect::<String>()).monospace().small().color(t.text_dim));
                });
            }
            Entry::Error(e) => {
                ui.label(RichText::new(tl!(e)).color(t.red));
            }
            Entry::Note(n) => {
                ui.label(RichText::new(tl!(n)).small().italics().color(t.text_dim));
            }
        }
    }
    if agent.waiting() {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label(RichText::new(tl!("Thinking…")).small().color(t.text_dim));
        });
    }
}

// ---------------------------------------------------------------------------------------------
// A small Markdown renderer: headings, lists, quotes, code blocks, tables (as text), **bold**,
// *italic*, `code` and links (shown as text). Model output is text, never markup to run.

fn markdown(ui: &mut Ui, text: &str, t: &Tokens) {
    let mut code: Option<Vec<&str>> = None;
    for line in text.lines() {
        if line.trim_start().starts_with("```") {
            match code.take() {
                Some(lines) => code_block(ui, &lines.join("\n"), t),
                None => code = Some(Vec::new()),
            }
            continue;
        }
        if let Some(lines) = code.as_mut() {
            lines.push(line);
            continue;
        }
        let trimmed = line.trim_start();
        let indent = (line.len() - trimmed.len()) as f32 * 4.0;
        if trimmed.is_empty() {
            ui.add_space(4.0);
        } else if let Some(h) = ["### ", "## ", "# "].iter().find_map(|p| trimmed.strip_prefix(p)) {
            ui.add_space(2.0);
            ui.label(inline(ui, h, t, 14.0, true));
        } else if let Some(item) = ["- ", "* ", "+ "].iter().find_map(|p| trimmed.strip_prefix(p)) {
            bullet(ui, indent, "•", item, t);
        } else if let Some((num, item)) = numbered(trimmed) {
            bullet(ui, indent, &format!("{num}."), item, t);
        } else if let Some(q) = trimmed.strip_prefix("> ") {
            ui.horizontal(|ui| {
                ui.add_space(4.0);
                ui.label(RichText::new("▎").color(t.border_strong));
                ui.label(inline(ui, q, t, 13.0, false));
            });
        } else if trimmed.starts_with('|') {
            // Table rows as monospace text; the divider row is dropped.
            if !trimmed.chars().all(|c| matches!(c, '|' | '-' | ':' | ' ')) {
                let cells: Vec<&str> = trimmed.trim_matches('|').split('|').map(str::trim).collect();
                ui.label(RichText::new(cells.join("  │  ")).monospace().small());
            }
        } else {
            ui.label(inline(ui, trimmed, t, 13.0, false));
        }
    }
    if let Some(lines) = code {
        code_block(ui, &lines.join("\n"), t);
    }
}

fn numbered(s: &str) -> Option<(&str, &str)> {
    let (num, rest) = s.split_once(". ")?;
    (!num.is_empty() && num.len() <= 3 && num.chars().all(|c| c.is_ascii_digit())).then_some((num, rest))
}

fn bullet(ui: &mut Ui, indent: f32, mark: &str, item: &str, t: &Tokens) {
    ui.horizontal_top(|ui| {
        ui.add_space(4.0 + indent);
        ui.label(RichText::new(mark).color(t.text_dim));
        ui.label(inline(ui, item, t, 13.0, false));
    });
}

fn code_block(ui: &mut Ui, code: &str, t: &Tokens) {
    egui::Frame::NONE.fill(t.input).stroke(egui::Stroke::new(1.0, t.border)).corner_radius(4).inner_margin(6).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.label(RichText::new(code).monospace().small());
    });
}

/// A line with **bold**, *italic*, `code` and [links](url) (as their text) laid out.
fn inline(ui: &Ui, text: &str, t: &Tokens, size: f32, heading: bool) -> egui::text::LayoutJob {
    use egui::text::{LayoutJob, TextFormat};
    let mut job = LayoutJob::default();
    job.wrap.max_width = ui.available_width();
    let base = |bold: bool, italic: bool, code: bool| {
        let font = if code {
            egui::FontId::monospace(size - 1.0)
        } else if bold || heading {
            semibold(size)
        } else {
            regular(size)
        };
        TextFormat {
            font_id: font,
            color: if code { t.accent_text } else { t.text },
            italics: italic,
            background: if code { t.input } else { egui::Color32::TRANSPARENT },
            ..Default::default()
        }
    };
    let (mut bold, mut italic) = (false, false);
    let mut buf = String::new();
    let flush = |job: &mut LayoutJob, buf: &mut String, bold: bool, italic: bool| {
        if !buf.is_empty() {
            job.append(buf, 0.0, base(bold, italic, false));
            buf.clear();
        }
    };
    let mut rest = text;
    while let Some(c) = rest.chars().next() {
        if let Some(r) = rest.strip_prefix("**").or_else(|| rest.strip_prefix("__")) {
            flush(&mut job, &mut buf, bold, italic);
            bold = !bold;
            rest = r;
        } else if c == '`'
            && let Some(end) = rest.get(1..).and_then(|r| r.find('`'))
        {
            flush(&mut job, &mut buf, bold, italic);
            job.append(rest.get(1..1 + end).unwrap_or(""), 0.0, base(bold, italic, true));
            rest = rest.get(end + 2..).unwrap_or("");
        } else if c == '*' && rest.get(1..).is_some_and(|r| !r.starts_with(' ')) {
            flush(&mut job, &mut buf, bold, italic);
            italic = !italic;
            rest = rest.get(1..).unwrap_or("");
        } else if c == '['
            && let Some(close) = rest.find("](")
            && let Some(end) = rest.get(close..).and_then(|r| r.find(')'))
        {
            buf.push_str(rest.get(1..close).unwrap_or(""));
            rest = rest.get(close + end + 1..).unwrap_or("");
        } else {
            buf.push(c);
            rest = rest.get(c.len_utf8()..).unwrap_or("");
        }
    }
    flush(&mut job, &mut buf, bold, italic);
    job
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The right pane's width with `entries` in the conversation and `model` as the model's name.
    fn pane_width(entries: Vec<Entry>, model: &str) -> f32 {
        let mut h = egui_kittest::Harness::builder().with_size(vec2(1400.0, 860.0)).with_max_steps(64).build_ui_state(
            |ui, app: &mut RustGisApp| {
                let ctx = ui.ctx().clone();
                app.logic(&ctx);
                app.ui(ui);
            },
            RustGisApp::with_sample(),
        );
        h.state_mut().run("ui.pane.assistant", serde_json::Value::Null).unwrap();
        for _ in 0..4 {
            h.step();
        }
        let app = h.state_mut();
        app.assistant.settings = false;
        let agent = app.assistant.agent.as_mut().unwrap();
        agent.config.model = model.into();
        agent.transcript = entries;
        for _ in 0..6 {
            h.step();
        }
        egui::containers::panel::PanelState::load(&h.ctx, egui::Id::new("right_pane")).unwrap().size().x
    }

    #[test]
    fn long_lines_wrap_instead_of_widening_the_pane() {
        let sentence = "The assistant ran the buffer tool on the cities layer and added the result to the map. ".repeat(4);
        let empty = pane_width(Vec::new(), "m");
        let mut widened = Vec::new();
        for (what, entry, model) in [
            ("a list item", Entry::Assistant(format!("- {sentence}")), "m"),
            ("a numbered item", Entry::Assistant(format!("1. {sentence}")), "m"),
            ("a quote", Entry::Assistant(format!("> {sentence}")), "m"),
            ("a word with no spaces", Entry::Assistant(format!("- {}", "x".repeat(400))), "m"),
            ("the user's message", Entry::User(sentence.clone()), "m"),
            (
                "a tool call",
                Entry::Tool { name: "run_command".into(), summary: "gp.run ".repeat(20), result: sentence.clone(), error: false },
                "m",
            ),
            (
                "the model's name",
                Entry::Assistant("Done.".into()),
                "provider/a-very-long-model-name-with-many-parts-preview-2026-10-14-extended",
            ),
        ] {
            let w = pane_width(vec![entry], model);
            if (w - empty).abs() > 0.5 {
                widened.push(format!("{what}: {empty} to {w}"));
            }
        }
        assert!(widened.is_empty(), "the pane grew wider with {widened:?}");
    }
}
