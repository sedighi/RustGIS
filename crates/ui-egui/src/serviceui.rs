//! Background group import/refresh for both desktop and browser. Discovery and updates live in
//! engine commands; this state machine only supplies their downloaded documents.

use crate::{RustGisApp, Slot, fetch_into, fetch_into_limited};
use serde_json::{Value, json};
use std::collections::VecDeque;

pub struct Import {
    params: Value,
    refresh: bool,
    token: u64,
    /// The originating map: its index and name (names are unique among a project's maps), so a
    /// deleted or replaced map at that index cancels the import.
    map: (usize, String),
    slot: Slot<Result<Vec<u8>, String>>,
    document: Option<String>,
    current: Option<String>,
    remaining: VecDeque<String>,
    responses: Vec<Value>,
    bytes: usize,
}

pub fn start(app: &mut RustGisApp, p: &Value, refresh: bool) -> Result<(), String> {
    if !p.is_object() {
        return Err(tl!("Give the service's web address").into());
    }
    if refresh && p.get("group").is_none() && p.get("layer").is_none() {
        return Err(tl!("Select a layer in the Layers pane.").into());
    }
    if app.ui.service_import.is_some() {
        return Err(tl!("A service import or refresh is already running.").into());
    }
    let mut source = p.clone();
    source["sourceOnly"] = json!(true);
    let mut params = app.session.run("service.groupPlan", &source)?;
    if let Some(p) = params.as_object_mut() {
        p.remove("sourceOnly");
        // Refresh uses the stored connection, not a previous discovery result.
        if refresh {
            p.remove("text");
            p.remove("documents");
        }
    }
    let mut task = Import {
        params,
        refresh,
        token: app.session.download_started(),
        map: active_map(app),
        slot: Slot::default(),
        document: None,
        current: None,
        remaining: VecDeque::new(),
        responses: Vec::new(),
        bytes: 0,
    };
    if task.params.get("text").and_then(Value::as_str).is_some() {
        if let Err(e) = task.discover(app) {
            app.session.download_failed(task.token);
            return Err(e);
        }
    } else {
        let url = task.params.get("url").and_then(Value::as_str).unwrap_or_default();
        let kind = task.params.get("type").and_then(Value::as_str);
        let caps = rustgis_engine::data::ogc::capabilities_url(url, kind);
        task.slot = fetch_into(&caps);
        task.document = Some(caps);
    }
    app.dialogs.error = None;
    app.ui.service_import = Some(task);
    Ok(())
}

impl Import {
    fn discover(&mut self, app: &mut RustGisApp) -> Result<(), String> {
        let listing = app.session.run("service.groupPlan", &self.params)?;
        if let Some(next) = listing.get("next").and_then(Value::as_str) {
            self.document = Some(next.into());
            self.slot = fetch_into(next);
        } else {
            self.remaining = listing
                .get("layers")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|l| l.get("offer").and_then(|o| o.get("url")).and_then(Value::as_str).map(str::to_string))
                .collect();
            self.next();
        }
        Ok(())
    }

    fn next(&mut self) {
        self.current = self.remaining.pop_front();
        if let Some(url) = &self.current {
            self.slot = fetch_into_limited(url, rustgis_engine::cmd::service::MAX_GROUP_BYTES.saturating_sub(self.bytes));
        }
    }

    fn landed(&mut self, app: &mut RustGisApp, bytes: Vec<u8>) -> Result<(), String> {
        if let Some(document) = self.document.take() {
            let text = crate::service_text(bytes).map_err(|_| tl!("The service's answer isn't UTF-8 text").to_string())?;
            if self.params.get("text").is_none() {
                self.params["text"] = json!(text);
                self.params["documents"] = json!([]);
            } else if let Some(docs) = self.params.get_mut("documents").and_then(Value::as_array_mut) {
                docs.push(json!({"url":document,"text":text}));
            } else {
                self.params["documents"] = json!([{"url":document,"text":text}]);
            }
            self.discover(app)
        } else {
            self.bytes = self.bytes.saturating_add(bytes.len());
            if self.bytes > rustgis_engine::cmd::service::MAX_GROUP_BYTES {
                return Err(tl!("Service group data exceeds 64 MB.").into());
            }
            let text = String::from_utf8(bytes).map_err(|_| tl!("The service's answer isn't UTF-8 text").to_string())?;
            self.responses.push(json!({"url":self.current.take(),"text":text}));
            self.next();
            Ok(())
        }
    }
}

fn active_map(app: &RustGisApp) -> (usize, String) {
    let i = app.session.project.active_map;
    (i, app.session.project.maps.get(i).map(|m| m.name.clone()).unwrap_or_default())
}

pub fn poll(app: &mut RustGisApp, ctx: &egui::Context) {
    let Some(mut task) = app.ui.service_import.take() else { return };
    if !app.session.download_is_pending(task.token) || active_map(app) != task.map {
        app.session.download_failed(task.token);
        return;
    }
    let landed = task.slot.lock().unwrap_or_else(|e| e.into_inner()).take();
    if let Some(answer) = landed
        && let Err(e) = answer.and_then(|bytes| task.landed(app, bytes))
    {
        app.session.download_failed(task.token);
        app.session.notify(e.clone(), true);
        app.dialogs.error = Some(e);
        return;
    }
    if task.document.is_none() && task.current.is_none() {
        app.session.download_failed(task.token);
        task.params["responses"] = json!(task.responses);
        // The plan's connection parameters are not public refresh parameters.
        if task.refresh
            && let Some(p) = task.params.as_object_mut()
        {
            for key in ["url", "type", "name"] {
                p.remove(key);
            }
        }
        let result = app.run(if task.refresh { "service.refresh" } else { "service.import" }, task.params);
        match result {
            Ok(_) => {
                app.dialogs.info = Some(tl!("Done").into());
                app.map.invalidate();
            }
            Err(e) => app.dialogs.error = Some(e),
        }
    } else {
        ctx.request_repaint_after(std::time::Duration::from_millis(30));
        app.ui.service_import = Some(task);
    }
}
