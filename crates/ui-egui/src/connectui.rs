//! "Let your Agents Control RustGIS" (desktop), in the AI Assistant settings. Ticking it turns
//! on the MCP relay ([`rustgis_assistant::relay`]): AI agents on this computer that connect
//! (Claude Code, Codex, Cursor…) work on the map open here, with file and network access, no
//! `--yes` needed. While it's on, the section shows what an agent needs to connect, and nothing
//! else. The choice is remembered: the relay starts with the app.

use egui::RichText;
use serde_json::json;

use crate::RustGisApp;
use crate::theme::Tokens;

/// The preference that remembers the choice.
pub const PREF: &str = "agents_control";

#[derive(Default)]
pub struct ConnectState {
    /// The choice, once read from the preferences.
    on: Option<bool>,
    /// The relay was started for a remembered choice (once, at launch).
    resumed: bool,
    /// Why the relay couldn't start, when it couldn't.
    error: Option<String>,
}

/// Whether the user lets agents control RustGIS.
pub fn enabled(app: &RustGisApp) -> bool {
    app.connect.on.unwrap_or_else(|| app.services.load_pref.as_ref().and_then(|l| l(PREF)).as_deref() == Some("on"))
}

/// At launch, starts the relay when the user had agents turned on.
pub fn resume(app: &mut RustGisApp) {
    resume_on(app, rustgis_assistant::relay::DEFAULT_PORT);
}

fn resume_on(app: &mut RustGisApp, port: u16) {
    if std::mem::replace(&mut app.connect.resumed, true) {
        return;
    }
    let on = enabled(app);
    app.connect.on = Some(on);
    if !on || app.relay.is_some() {
        return;
    }
    if let Err(e) = app.run("ui.mcpRelay", json!({"port": port})) {
        app.connect.on = Some(false);
        if let Some(save) = &app.services.save_pref {
            save(PREF, "off");
        }
        app.session.notify(e.clone(), true);
        app.connect.error = Some(e);
    }
}

/// Every explicit stop, including the status bar/command, also withdraws remembered consent.
pub(crate) fn stopped(app: &mut RustGisApp) {
    let changed = app.connect.on != Some(false);
    app.connect.on = Some(false);
    app.connect.error = None;
    if changed && let Some(save) = &app.services.save_pref {
        save(PREF, "off");
    }
}

/// Literal argument syntax for the shell named beside the copyable command.
fn shell_arg(text: &str) -> String {
    #[cfg(windows)]
    let escaped = text.replace('\'', "''");
    #[cfg(not(windows))]
    let escaped = text.replace('\'', "'\\''");
    format!("'{escaped}'")
}

/// `rustgis-cli` beside the app, or its name (for a PATH lookup) when it isn't there.
fn cli() -> String {
    let name = if cfg!(windows) { "rustgis-cli.exe" } else { "rustgis-cli" };
    std::env::current_exe()
        .ok()
        .map(|p| p.with_file_name(name))
        .filter(|p| p.is_file())
        .map_or(name.to_string(), |p| p.display().to_string())
}

/// A line to copy: shown in monospace, with a Copy button.
fn copyable(ui: &mut egui::Ui, text: &str) {
    ui.horizontal(|ui| {
        ui.add(egui::Label::new(RichText::new(text).monospace().size(11.5)).wrap());
        if ui.small_button(tl!("Copy")).clicked() {
            ui.ctx().copy_text(text.to_string());
        }
    });
}

/// The section in the assistant settings.
pub fn section(app: &mut RustGisApp, ui: &mut egui::Ui, t: &Tokens) {
    let mut on = enabled(app);
    let hint = tl!(
        "Agents on this computer that connect (Claude Code, Codex, Cursor and others) work on the map open here, with RustGIS's tools: they can open and save files and use the web. Scripts and plugin management stay off. The choice is remembered across launches."
    );
    if ui.checkbox(&mut on, tl!("Let your Agents Control RustGIS")).on_hover_text(hint).changed() {
        app.connect.on = Some(on);
        app.connect.error = None;
        if let Some(save) = &app.services.save_pref {
            save(PREF, if on { "on" } else { "off" });
        }
        let started = if on { app.run("ui.mcpRelay", json!({})) } else { app.run("ui.mcpRelay", json!({"stop": true})) };
        if let Err(e) = started {
            // Not started (the port is taken, say): the box and the remembered choice go back off.
            app.connect.error = Some(e);
            if on {
                on = false;
                app.connect.on = Some(false);
                if let Some(save) = &app.services.save_pref {
                    save(PREF, "off");
                }
            }
        }
        if let Some(relay) = &app.relay {
            relay.set_allow_access(on);
        }
    }
    if let Some(e) = &app.connect.error {
        ui.label(RichText::new(e).small().color(t.red));
    }
    let (true, Some(port)) = (on, app.relay.as_ref().map(|r| r.port())) else { return };
    // What an agent needs: RustGIS's MCP server, attached to this app.
    let cli = cli();
    let mut args = vec!["mcp".to_string(), "--attach".to_string()];
    if port != rustgis_assistant::relay::DEFAULT_PORT {
        args.push(port.to_string());
    }
    let config = serde_json::to_string_pretty(&json!({"mcpServers": {"rustgis-app": {"command": cli, "args": args}}})).unwrap_or_default();
    ui.label(RichText::new(tl!("Add this MCP server to your agent:")).small());
    copyable(ui, &config);
    let hint = if cfg!(windows) {
        tl!("Or run in PowerShell, for Claude Code or Codex:")
    } else {
        tl!("Or run in a POSIX shell, for Claude Code or Codex:")
    };
    ui.label(RichText::new(hint).small());
    let quoted = shell_arg(&cli);
    for agent in ["claude", "codex"] {
        copyable(ui, &format!("{agent} mcp add rustgis-app -- {quoted} {}", args.join(" ")));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A remembered relay that cannot start becomes off in memory and preferences, with a notice.
    #[test]
    fn failed_relay_resume_clears_the_saved_choice() {
        use std::sync::{Arc, Mutex};
        let occupied = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = occupied.local_addr().unwrap().port();
        let saved = Arc::new(Mutex::new(Vec::<(String, String)>::new()));
        let mut app = RustGisApp::with_sample();
        app.services.load_pref = Some(Box::new(|key: &str| (key == PREF).then(|| "on".to_string())));
        let sink = saved.clone();
        app.services.save_pref = Some(Box::new(move |key: &str, value: &str| {
            sink.lock().unwrap().push((key.into(), value.into()));
        }));
        let mark = app.session.notice_mark();
        resume_on(&mut app, port);
        assert!(app.relay.is_none());
        assert!(!enabled(&app));
        assert!(saved.lock().unwrap().iter().any(|(k, v)| k == PREF && v == "off"));
        let error = app.connect.error.as_ref().unwrap();
        assert!(app.session.notices_since(mark).iter().any(|n| &n.text == error));
    }
    #[test]
    fn explicit_stop_updates_cached_and_remembered_choice() {
        use std::sync::{Arc, Mutex};
        let saved = Arc::new(Mutex::new(String::from("on")));
        let mut app = RustGisApp::with_sample();
        let read = saved.clone();
        app.services.load_pref = Some(Box::new(move |key| (key == PREF).then(|| read.lock().unwrap().clone())));
        let write = saved.clone();
        app.services.save_pref = Some(Box::new(move |key, value| {
            if key == PREF {
                *write.lock().unwrap() = value.into();
            }
        }));
        resume_on(&mut app, 0);
        assert!(app.relay.is_some());
        assert!(enabled(&app));
        app.run("ui.mcpRelay", json!({"stop":true})).unwrap();
        assert!(app.relay.is_none());
        assert!(!enabled(&app));
        assert_eq!(*saved.lock().unwrap(), "off");
    }

    #[test]
    fn copyable_program_paths_are_literal_shell_arguments() {
        #[cfg(not(windows))]
        {
            let value = r#"space path/'"$(printf BAD)`printf BAD`\agent"#;
            let script = format!("printf '%s' {}", shell_arg(value));
            let result = std::process::Command::new("/bin/sh").args(["-c", &script]).output().unwrap();
            assert!(result.status.success());
            assert_eq!(String::from_utf8(result.stdout).unwrap(), value);
        }
        #[cfg(windows)]
        assert_eq!(shell_arg("C:\\a'b\\$file.exe"), "'C:\\a''b\\$file.exe'");
    }
}
