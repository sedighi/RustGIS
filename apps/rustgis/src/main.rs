//! RustGIS desktop app.
//!
//! `rustgis [project.rgis | data.geojson|csv|shp …] [--sample]`
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use rustgis_engine::{Project, Session};
use rustgis_ui_egui::RustGisApp;
use serde_json::json;

#[cfg(target_os = "macos")]
mod macos;

/// The app, and this run's marker (see [`running`]), cleared when eframe closes it.
struct App(RustGisApp, running::Shared);

/// Preferences in `prefs.json` beside the platform's other app settings.
mod prefs {
    use std::path::PathBuf;

    use rustgis_ui_egui::Services;
    use serde_json::{Map, Value};

    pub fn dir() -> Option<PathBuf> {
        if cfg!(target_os = "windows") {
            std::env::var_os("APPDATA").map(|d| PathBuf::from(d).join("RustGIS"))
        } else if cfg!(target_os = "macos") {
            std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Library/Application Support/RustGIS"))
        } else {
            std::env::var_os("XDG_CONFIG_HOME")
                .map(PathBuf::from)
                .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
                .map(|d| d.join("rustgis"))
        }
    }

    fn read() -> Map<String, Value> {
        dir()
            .and_then(|d| std::fs::read_to_string(d.join("prefs.json")).ok())
            .and_then(|t| serde_json::from_str::<Value>(&t).ok())
            .and_then(|v| v.as_object().cloned())
            .unwrap_or_default()
    }

    pub fn services() -> Services {
        Services {
            load_pref: Some(Box::new(|key: &str| read().get(key).and_then(Value::as_str).map(str::to_string))),
            save_pref: Some(Box::new(|key: &str, value: &str| {
                let Some(d) = dir() else { return };
                let mut map = read();
                map.insert(key.to_string(), Value::String(value.to_string()));
                let text = Value::Object(map).to_string();
                // Write a temporary file and rename it over the old one, so a crash mid-write
                // never leaves a truncated prefs.json.
                let tmp = d.join("prefs.json.tmp");
                let saved = std::fs::create_dir_all(&d)
                    .and_then(|_| std::fs::write(&tmp, text))
                    .and_then(|_| std::fs::rename(&tmp, d.join("prefs.json")));
                if let Err(e) = saved {
                    log::warn!("could not save preferences: {e}");
                }
            })),
            ..Services::default()
        }
    }
}

/// Crash reports. A panic, caught or not, is written to `crash.log` in the settings folder (a
/// release build on Windows has no console to print it to), and the next launch says where it is.
mod crash {
    use std::io::Write;
    use std::path::{Path, PathBuf};

    /// The log never grows past this size.
    const MAX_LOG: u64 = 256 * 1024;

    /// Writes every panic to `<dir>/crash.log`, then reports it as before.
    pub fn install(dir: Option<PathBuf>) {
        let Some(dir) = dir else { return };
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            write(&dir, info);
            previous(info);
        }));
    }

    fn write(dir: &Path, info: &std::panic::PanicHookInfo<'_>) {
        let path = dir.join("crash.log");
        let used = std::fs::metadata(&path).map_or(0, |m| m.len());
        let thread = std::thread::current();
        let entry = format!(
            "--- RustGIS {} on {} {}, {} UTC, thread {}\n{info}\n{}\n\n",
            env!("CARGO_PKG_VERSION"),
            std::env::consts::OS,
            std::env::consts::ARCH,
            utc_now(),
            thread.name().unwrap_or("unnamed"),
            std::backtrace::Backtrace::force_capture(),
        );
        // The log starts over when this entry doesn't fit, and one huge entry is cut to the cap.
        let append = used.saturating_add(entry.len() as u64) <= MAX_LOG;
        let mut cut = entry.len().min(usize::try_from(MAX_LOG).unwrap_or(usize::MAX));
        while !entry.is_char_boundary(cut) {
            cut -= 1;
        }
        let file = std::fs::create_dir_all(dir).and_then(|_| {
            let mut options = std::fs::OpenOptions::new();
            options.create(true).write(true).append(append).truncate(!append);
            // Readable by this user only: an error message can quote file paths or data.
            #[cfg(unix)]
            std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
            options.open(&path)
        });
        if let Ok(mut file) = file {
            let _ = file.write_all(entry.get(..cut).unwrap_or_default().as_bytes());
        }
    }

    fn utc_now() -> String {
        utc(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs()))
    }

    /// Seconds since 1970 as `YYYY-MM-DD HH:MM:SS` (UTC).
    pub(crate) fn utc(secs: u64) -> String {
        let (days, rest) = (secs / 86_400, secs % 86_400);
        // Days since 1970-01-01 to a civil date (Howard Hinnant's algorithm).
        let z = i64::try_from(days).unwrap_or(0) + 719_468;
        let era = z.div_euclid(146_097);
        let doe = z.rem_euclid(146_097);
        let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let day = doy - (153 * mp + 2) / 5 + 1;
        let month = if mp < 10 { mp + 3 } else { mp - 9 };
        let year = yoe + era * 400 + i64::from(month <= 2);
        format!("{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02}", rest / 3600, rest % 3600 / 60, rest % 60)
    }

    /// The crash log an earlier session left, moved to `crash-last.log` so it is mentioned once.
    pub fn take_previous(dir: Option<PathBuf>) -> Option<PathBuf> {
        let dir = dir?;
        let log = dir.join("crash.log");
        if !log.is_file() {
            return None;
        }
        let seen = dir.join("crash-last.log");
        std::fs::rename(&log, &seen).ok()?;
        Some(seen)
    }
}

/// Whether the last launch ended without a clean exit, told by a marker file this run holds
/// (locked) and empties when it closes. A crash of any kind (a panic, a native abort, a stack
/// overflow, running out of memory), a kill or a power cut leaves it set; a panic that was caught
/// and recovered from doesn't. A second copy running alongside can't lock it, so it doesn't count.
mod running {
    use std::fs::{File, OpenOptions};
    use std::io::{Read, Seek, Write};
    use std::path::Path;

    /// The marker, held while this run lasts.
    pub struct Marker(File);

    /// Takes `<dir>/running` for this run: the marker (none when another copy holds it or the
    /// folder can't be written) and whether the previous run didn't close cleanly.
    pub fn start(dir: Option<&Path>) -> (Option<Marker>, bool) {
        let Some(dir) = dir else { return (None, false) };
        let file = std::fs::create_dir_all(dir)
            .and_then(|_| OpenOptions::new().read(true).write(true).create(true).truncate(false).open(dir.join("running")));
        let mut file = match file {
            Ok(f) => f,
            Err(e) => {
                log::warn!("no run marker in {} ({e}): an unclean exit won't be noticed", dir.display());
                return (None, false);
            }
        };
        match file.try_lock() {
            Ok(()) => {}
            Err(std::fs::TryLockError::WouldBlock) => {
                log::info!("the run marker is held: another copy of RustGIS is running");
                return (None, false);
            }
            // The folder can't lock files (some network or FUSE folders): keep the marker unlocked,
            // so unclean exits are still noticed. Without a lock, copies running side by side share
            // it: the second is taken for an unclean exit (it only loses its start-up project), and
            // when it closes first it clears the marker, so a later crash of the first goes
            // unnoticed. Every copy still clears the marker on a clean exit: none holds a lock here,
            // and a marker nobody clears would make every later launch look unclean.
            Err(std::fs::TryLockError::Error(e)) => {
                log::warn!("could not lock the run marker ({e}): a second copy running alongside will look like an unclean exit");
            }
        }
        // A clean exit leaves the marker empty: anything in it (whatever it reads as) means the
        // last run didn't close, and so does a marker that can't be read.
        let mut bytes = Vec::new();
        let unclean = match file.read_to_end(&mut bytes) {
            Ok(_) => !bytes.is_empty(),
            Err(e) => {
                log::warn!("could not read the run marker ({e}): taking the last run as not closed");
                true
            }
        };
        let marked = file.set_len(0).and_then(|_| file.rewind()).and_then(|_| file.write_all(b"running")).and_then(|_| file.flush());
        if let Err(e) = marked {
            log::warn!("could not mark this run: {e}");
        }
        (Some(Marker(file)), unclean)
    }

    impl Marker {
        /// A clean exit: the next launch starts as usual. Dropping the file releases the lock.
        pub fn close(self) {
            if let Err(e) = self.0.set_len(0) {
                log::warn!("could not clear the run marker: {e}");
            }
        }
    }

    /// The marker as the app and `main` both hold it: whichever sees the clean exit first
    /// clears it (the app's `on_exit`, or `main` when `run_native` returns, which also covers
    /// eframe failing to start before the app exists).
    pub type Shared = std::sync::Arc<std::sync::Mutex<Option<Marker>>>;

    /// Clears a shared marker once.
    pub fn close(marker: &Shared) {
        let taken = marker.lock().map(|mut m| m.take()).unwrap_or_else(|poisoned| poisoned.into_inner().take());
        if let Some(m) = taken {
            m.close();
        }
    }
}

impl eframe::App for App {
    fn logic(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        // The GPU map backend draws inside egui's wgpu pass; hand over the render state.
        self.0.set_render_state(frame.wgpu_render_state().cloned());
        #[cfg(target_os = "macos")]
        {
            let titlebar = macos::titlebar();
            // egui repaints on events only: when the window system answers late, draw again.
            if titlebar != self.0.ui.titlebar {
                ctx.request_repaint();
            }
            self.0.ui.titlebar = titlebar;
        }
        self.0.logic(ctx);
        if self.0.quit_requested {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.0.ui(ui);
    }
    /// Every way eframe closes the app comes here, Cmd+Q on macOS included, which may not
    /// return from `run_native`: the run ended cleanly.
    fn on_exit(&mut self) {
        running::close(&self.1);
    }
}

fn main() -> eframe::Result {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--version" || a == "-V") {
        println!("rustgis {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    // What an earlier session left, before this one can write a new log.
    let crashed = crash::take_previous(prefs::dir());
    // Whether it closed cleanly: if not, the Start With project isn't opened (it may be what
    // crashed, and reopening it on every launch would leave no way into Settings).
    let (marker, unclean) = running::start(prefs::dir().as_deref());
    crash::install(prefs::dir());
    let files: Vec<&String> = args.iter().filter(|a| !a.starts_with("--")).collect();
    // Installed plugins, before the first session lists commands and tools.
    let plugin_errors = rustgis_engine::cmd::plugins::load_installed();
    let app = if args.iter().any(|a| a == "--sample") || files.is_empty() {
        RustGisApp::with_sample()
    } else {
        RustGisApp::new(Session::new(Project::default()))
    };
    let mut app = app.with_services(prefs::services());
    // Settings › Start-up; `--sample` still opens the sample.
    let blank = if args.iter().any(|a| a == "--sample") { Some(false) } else { (!files.is_empty()).then_some(true) };
    if let Err(e) = app.start(&rustgis_ui_egui::Start { blank, after_crash: unclean, ..Default::default() }) {
        log::warn!("start-up: {e}");
    }
    if let Some(log) = crashed {
        // Notices are shown as written: translate this one now that the language is known.
        rustgis_ui_egui::i18n::set_current(app.chosen_lang());
        let text = rustgis_ui_egui::i18n::fmt(
            "RustGIS ran into an internal error last time. The details are in {path}; please attach them to a bug report.",
            &[("path", &log.display().to_string())],
        );
        app.session.notify(text, true);
    }
    for e in plugin_errors {
        log::warn!("{e}");
        app.session.notify(e, true);
    }
    for f in files {
        let cmd = if rustgis_engine::project::is_project_file(f) { "project.open" } else { "layer.addData" };
        if let Err(e) = app.run(cmd, json!({"path": f})) {
            eprintln!("rustgis: {f}: {e}");
        }
    }
    app.session.dirty = false;
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("RustGIS")
        .with_app_id("org.rustgis.RustGIS")
        .with_inner_size([1480.0, 920.0])
        .with_min_inner_size([900.0, 560.0])
        .with_drag_and_drop(true);
    // macOS: the header is the title bar, with the window buttons floating over its left end.
    #[cfg(target_os = "macos")]
    {
        viewport = viewport.with_fullsize_content_view(true).with_titlebar_shown(false).with_title_shown(false);
    }
    if let Some((size, rgba)) = rustgis_engine::render::logo::rgba(256) {
        viewport = viewport.with_icon(egui::IconData { rgba, width: size, height: size });
    }
    let options = eframe::NativeOptions { viewport, ..Default::default() };
    let marker: running::Shared = std::sync::Arc::new(std::sync::Mutex::new(marker));
    let held = marker.clone();
    let result = eframe::run_native("RustGIS", options, Box::new(move |_cc| Ok(Box::new(App(app, held)))));
    running::close(&marker);
    result
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::{crash, running};

    #[test]
    fn a_run_that_did_not_close_is_noticed_once() {
        let dir = std::env::temp_dir().join(format!("rustgis-running-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        // Removed however the test ends (declared first, so dropped after the markers).
        let _cleanup = TempDir(dir.clone());
        let (first, unclean) = running::start(Some(&dir));
        assert!(first.is_some() && !unclean, "a first run");
        // Another copy running alongside can't take it, and doesn't count as a crash.
        let (second, unclean) = running::start(Some(&dir));
        assert!(second.is_none() && !unclean);
        // This run ends without closing (a crash): the next one notices, once it closes cleanly.
        drop(first);
        let (after_crash, unclean) = running::start(Some(&dir));
        assert!(unclean);
        after_crash.unwrap().close();
        let (next, unclean) = running::start(Some(&dir));
        assert!(!unclean);
        drop(next);
    }

    /// A folder removed when the test ends, passed or failed.
    struct TempDir(std::path::PathBuf);

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn crash_times_are_utc_dates() {
        assert_eq!(crash::utc(0), "1970-01-01 00:00:00");
        assert_eq!(crash::utc(951_782_400), "2000-02-29 00:00:00");
        assert_eq!(crash::utc(1_700_000_000), "2023-11-14 22:13:20");
    }

    #[test]
    fn a_panic_is_logged_and_reported_once() {
        let dir = std::env::temp_dir().join(format!("rustgis-crash-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        crash::install(Some(dir.clone()));
        let _ = std::thread::spawn(|| panic!("boom for the crash log")).join();
        let log = std::fs::read_to_string(dir.join("crash.log")).unwrap();
        assert!(log.contains("boom for the crash log") && log.contains(env!("CARGO_PKG_VERSION")), "{log}");
        // The next launch finds it once.
        let seen = crash::take_previous(Some(dir.clone())).unwrap();
        assert!(seen.ends_with("crash-last.log") && seen.is_file());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&seen).unwrap().permissions().mode() & 0o777, 0o600);
        }
        assert!(crash::take_previous(Some(dir.clone())).is_none());
        let _ = std::panic::take_hook();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
