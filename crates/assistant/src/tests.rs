use std::collections::VecDeque;

use rustgis_engine::{Project, Session};
use serde_json::{Value, json};

use crate::agent::{Agent, Entry, INTERNAL_ERROR, MAX_STEPS, Step, run_blocking};
use crate::provider::*;
use crate::tools;

fn sample() -> Session {
    let mut s = Session::new(Project::default());
    let _ = s.run("project.sample", &Value::Null);
    s
}

fn cfg(provider: &str) -> Config {
    let p = preset(provider).unwrap_or(&PRESETS[0]);
    Config {
        provider: provider.into(),
        base_url: p.base_url.into(),
        model: if p.model.is_empty() { "m".into() } else { p.model.into() },
        key: "k".into(),
    }
}

fn ok(v: Value) -> Result<HttpResponse, String> {
    Ok(HttpResponse { status: 200, body: v.to_string().into_bytes() })
}

/// An OpenAI reply calling one tool.
fn call(id: &str, name: &str, args: Value) -> Value {
    json!({"choices": [{"message": {"role": "assistant", "content": null,
        "tool_calls": [{"id": id, "type": "function", "function": {"name": name, "arguments": args.to_string()}}]},
        "finish_reason": "tool_calls"}]})
}

fn say(text: &str) -> Value {
    json!({"choices": [{"message": {"role": "assistant", "content": text}, "finish_reason": "stop"}]})
}

fn body(req: &HttpRequest) -> Value {
    serde_json::from_str(req.body.as_deref().unwrap_or("null")).unwrap_or(Value::Null)
}

#[test]
fn urls_allowed() {
    assert!(allowed_url("https://api.openai.com/v1/chat/completions"));
    assert!(allowed_url("http://localhost:11434/v1/models"));
    assert!(allowed_url("http://127.0.0.1:1234/v1"));
    assert!(allowed_url("http://192.168.1.20:11434/v1"));
    assert!(allowed_url("http://[::1]:8080/v1"));
    assert!(allowed_url("http://172.20.0.2/v1"));
    assert!(!allowed_url("http://example.com/v1"));
    assert!(!allowed_url("http://localhost@example.com/v1"));
    assert!(!allowed_url("http://172.40.0.2/v1"));
    assert!(!allowed_url("http://10.example.com/v1"));
    assert!(!allowed_url("file:///etc/passwd"));
    assert!(!allowed_url("https://user:key@api.example.com/v1"));
    assert!(!allowed_url("https://"));
    assert!(!allowed_url("http://010.0.0.1/v1"));
    assert!(!allowed_url("http://0177.0.0.1/v1"));
    assert!(allowed_url("http://10.0.0.1/v1"));
    assert!(!allowed_url(""));
}

#[test]
fn openai_round_trip() {
    let s = sample();
    let mut a = Agent::new(cfg("openai"), &s);
    let Step::Send(req) = a.ask("Zoom to Paris", &s) else { panic!("expected a request") };
    assert_eq!(req.url, "https://api.openai.com/v1/chat/completions");
    assert!(req.headers.iter().any(|(k, v)| k == "Authorization" && v == "Bearer k"));
    let b = body(&req);
    assert_eq!(b["messages"][0]["role"], "system");
    let user = b["messages"][1]["content"].as_str().unwrap_or("");
    assert!(user.contains("<map>") && user.contains("\"Cities\"") && user.ends_with("Zoom to Paris"));
    assert!(b["tools"].as_array().is_some_and(|t| t.iter().any(|t| t["function"]["name"] == "run_command")));
}

#[test]
fn agent_runs_commands_and_answers() {
    let mut s = sample();
    let mut a = Agent::new(cfg("openai"), &s);
    let mut replies: VecDeque<Value> = VecDeque::from(vec![
        call("c1", "run_command", json!({"command": "map.goToXY", "params": {"lon": 2.35, "lat": 48.85, "scale": 5000000}})),
        say("Done: the map is centred on **Paris**."),
    ]);
    let mut requests = Vec::new();
    run_blocking(
        &mut a,
        "Zoom to Paris",
        &mut s,
        |req| {
            requests.push(body(req));
            replies.pop_front().map(ok).unwrap_or_else(|| Err("no more replies".into()))
        },
        |_, _, _| true,
    );
    let c = s.map().center_lonlat();
    assert!((c.x - 2.35).abs() < 0.01 && (c.y - 48.85).abs() < 0.01, "map moved to {c:?}");
    assert!(matches!(a.transcript.get(1), Some(Entry::Tool { error: false, .. })), "{:?}", a.transcript);
    assert!(matches!(a.transcript.last(), Some(Entry::Assistant(t)) if t.contains("Paris")));
    // The second request carries the call and its result.
    let msgs = requests.get(1).and_then(|b| b["messages"].as_array().cloned()).unwrap_or_default();
    assert!(msgs.iter().any(|m| m["role"] == "tool" && m["tool_call_id"] == "c1"));
    assert!(!a.busy());
    // The context is sent again only when the map changed: it did (the camera moved).
    let Step::Send(req) = a.ask("And now?", &s) else { panic!("expected a request") };
    let b = body(&req);
    let last = b["messages"].as_array().and_then(|m| m.last().cloned()).unwrap_or(Value::Null);
    assert!(last["content"].as_str().is_some_and(|t| t.starts_with("<map>")));
}

#[test]
fn unchanged_context_is_not_repeated() {
    let s = sample();
    let mut a = Agent::new(cfg("openai"), &s);
    let Step::Send(_) = a.ask("hi", &s) else { panic!("expected a request") };
    let mut s2 = sample();
    let _ = a.receive(a.generation(), ok(say("Hello")), &mut s2);
    let Step::Send(req) = a.ask("what layers?", &s) else { panic!("expected a request") };
    let b = body(&req);
    let last = b["messages"].as_array().and_then(|m| m.last().cloned()).unwrap_or(Value::Null);
    assert_eq!(last["content"], "what layers?");
}

#[test]
fn file_access_needs_approval() {
    let mut s = sample();
    let mut a = Agent::new(cfg("openai"), &s);
    let Step::Send(_) = a.ask("export the map", &s) else { panic!("expected a request") };
    let path = std::env::temp_dir().join("rustgis-assistant-never-written.png");
    let step =
        a.receive(a.generation(), ok(call("c1", "run_command", json!({"command": "share.exportMap", "params": {"path": path}}))), &mut s);
    assert_eq!(step, Step::Confirm);
    let (what, why, _) = a.pending(&s).unwrap_or_default();
    assert!(what.starts_with("share.exportMap") && why.contains("files"), "{what} / {why}");
    let Step::Send(req) = a.decide(false, false, &mut s) else { panic!("expected a request") };
    assert!(!path.exists());
    let b = body(&req);
    let tool_msg = b["messages"].as_array().and_then(|m| m.iter().find(|m| m["role"] == "tool").cloned()).unwrap_or(Value::Null);
    assert!(tool_msg["content"].as_str().is_some_and(|t| t.contains("declined")));
}

#[test]
fn gate_rules() {
    let s = sample();
    let g = |name: &str, args: Value| tools::gate(&s, &ToolCall { id: "x".into(), name: name.into(), args });
    assert!(g("run_command", json!({"command": "map.zoomFull"})).is_none());
    assert!(g("run_command", json!({"command": "edit.undo"})).is_none());
    assert!(g("run_command", json!({"command": "layer.addData", "params": {"path": "/etc/passwd"}})).is_some());
    assert!(g("run_command", json!({"command": "project.new"})).is_some());
    assert!(g("run_command", json!({"command": "script.run", "params": {"script": "map.zoomFull {}"}})).is_some());
    assert!(g("run_command", json!({"command": "plugin.install", "params": {"path": "x.rgplugin"}})).is_some());
    assert!(g("run_command", json!({"command": "map.addTiles", "params": {"url": "https://tile.example/{z}/{x}/{y}.png"}})).is_some());
    assert!(g("run_command", json!({"command": "plugin:x/y"})).is_some());
    let sql = g("run_command", json!({"command": "layer.addSql", "params": {"sql": "SELECT ST_Point(0, 0) AS g"}}));
    assert!(sql.as_deref().is_some_and(|w| w.contains("SQL") && w.contains("internet")), "{sql:?}");
    let sql = g("run_command", json!({"command": "layer.addSql", "params": {"sql": "FROM t", "database": "/data/x.duckdb"}}));
    assert!(sql.as_deref().is_some_and(|w| w.contains("views in the database /data/x.duckdb")), "{sql:?}");
    assert!(g("run_tool", json!({"tool": "buffer", "params": {"input": "Cities", "distance": 100}})).is_none());
    assert!(g("list_layers", json!({})).is_none());
    assert!(g("run_command", json!({"command": "info.fileLayers", "params": {"path": "/etc/passwd"}})).is_some());
    assert!(g("run_command", json!({"command": "info.fileColumns", "params": {"path": "/etc/passwd"}})).is_some());
}

/// What a command says beside its result reaches the model (issue #85).
#[test]
fn tool_results_carry_notices() {
    let mut s = sample();
    let call = ToolCall {
        id: "x".into(),
        name: "run_command".into(),
        args: json!({"command": "layer.addDataText", "params": {"fileName": "pts.csv", "text": "lon,lat\n1,2\nx,y\n"}}),
    };
    let out: Value = serde_json::from_str(&tools::run(&mut s, &call).unwrap()).unwrap();
    assert!(
        out["notices"].as_array().is_some_and(|n| n.iter().any(|t| t.as_str().is_some_and(|t| t.contains("no usable coordinates")))),
        "{out}"
    );
    let quiet: Value = serde_json::from_str(
        &tools::run(&mut s, &ToolCall { id: "y".into(), name: "run_command".into(), args: json!({"command": "map.zoomFull"}) }).unwrap(),
    )
    .unwrap();
    assert!(quiet.get("notices").is_none(), "{quiet}");
}

#[test]
fn geoprocessing_and_reading_data() {
    let mut s = sample();
    let t = |name: &str, args: Value| ToolCall { id: "x".into(), name: name.into(), args };
    let found = tools::run(&mut s, &t("search_tools", json!({"query": "buffer"}))).unwrap_or_default();
    assert!(found.contains("\"buffer\""), "{found}");
    let desc = tools::run(&mut s, &t("describe_tool", json!({"tool": "buffer"}))).unwrap_or_default();
    assert!(desc.contains("distance"), "{desc}");
    let before = s.map().layers.len();
    let ran = tools::run(&mut s, &t("run_tool", json!({"tool": "buffer", "params": {"input": "Cities", "distance": 100}})));
    assert!(ran.as_deref().is_ok_and(|r| r.contains("layersAdded")), "{ran:?}");
    assert_eq!(s.map().layers.len(), before + 1);
    let rows = tools::run(
        &mut s,
        &t("read_features", json!({"layer": "Cities", "where": "pop_max > 10000000", "fields": ["name", "pop_max"], "order_by": "pop_max", "descending": true, "limit": 3})),
    )
    .unwrap_or_default();
    let v: Value = serde_json::from_str(&rows).unwrap_or(Value::Null);
    assert!(v["matched"].as_u64().is_some_and(|n| n > 3), "{rows}");
    let r = v["rows"].as_array().cloned().unwrap_or_default();
    assert_eq!(r.len(), 3);
    assert!(r[0]["pop_max"].as_i64() >= r[1]["pop_max"].as_i64());
    assert!(tools::run(&mut s, &t("read_features", json!({"layer": "Cities", "fields": ["nope"]}))).is_err());
    assert!(tools::run(&mut s, &t("read_features", json!({"layer": "Cities", "fields": [1, null]}))).is_err());
    let cmds = tools::run(&mut s, &t("search_commands", json!({"query": "basemap"}))).unwrap_or_default();
    assert!(cmds.contains("map.basemap"), "{cmds}");
}

#[test]
fn hostile_arguments_never_panic() {
    let mut s = sample();
    let junk = [
        Value::Null,
        json!({}),
        json!([1, 2]),
        json!("text"),
        json!({"command": 5, "params": "not json", "tool": [], "layer": {"a": 1}, "query": null, "limit": -4}),
        json!({"command": "map.goToXY", "params": "{\"lon\": 1e309}"}),
        json!({"command": "ui.about"}),
        json!({"command": "nope.nope"}),
        json!({"tool": "nope", "params": {"input": "Nope"}}),
        json!({"layer": "Cities", "where": "pop_max >", "limit": 1e20, "order_by": "", "fields": [1, null]}),
        json!({"layer": 99999, "selected": true}),
        json!({"query": "∑≈ç√ 🙂 a b c d e f g h i j k l m n o p", "limit": 100000}),
        json!({"tool": "buffer", "params": {"input": "Cities", "distance": "far"}}),
    ];
    for def in tools::defs() {
        for args in &junk {
            let c = ToolCall { id: "x".into(), name: def.name.into(), args: args.clone() };
            let _ = tools::gate(&s, &c);
            let _ = tools::summary(&c);
            if let Ok(r) = tools::run(&mut s, &c) {
                assert!(r.chars().count() <= tools::MAX_RESULT + 60);
            }
        }
    }
    let _ = tools::run(&mut s, &ToolCall { id: "x".into(), name: "rm_rf".into(), args: Value::Null });
}

#[test]
fn hostile_replies_never_panic() {
    let bodies = [
        "",
        "null",
        "[]",
        "{\"choices\": []}",
        "{\"choices\": [{\"message\": {\"tool_calls\": [{\"function\": {\"arguments\": \"{oops\"}}]}}]}",
        "{\"content\": [{\"type\": \"tool_use\"}], \"stop_reason\": \"refusal\"}",
        "{\"candidates\": [{\"content\": {\"parts\": [{\"functionCall\": {}}, {\"text\": 5}]}}]}",
        "{\"error\": {\"message\": \"Invalid API key\"}}",
        "<html>bad gateway</html>",
    ];
    for api in [Api::OpenAi, Api::Anthropic, Api::Gemini] {
        for b in bodies {
            for status in [200, 401, 500] {
                let _ = parse_reply(api, &HttpResponse { status, body: b.as_bytes().to_vec() });
                let _ = parse_models(api, &HttpResponse { status, body: b.as_bytes().to_vec() });
            }
        }
    }
    let e = parse_reply(Api::OpenAi, &HttpResponse { status: 401, body: b"{\"error\": {\"message\": \"Invalid API key\"}}".to_vec() });
    assert_eq!(e.err().as_deref(), Some("Invalid API key"));
    // A bad reply ends the run with an error the user sees.
    let mut s = sample();
    let mut a = Agent::new(cfg("openai"), &s);
    let Step::Send(_) = a.ask("hi", &s) else { panic!("expected a request") };
    assert_eq!(a.receive(a.generation(), Err("offline".into()), &mut s), Step::Done);
    assert!(matches!(a.transcript.last(), Some(Entry::Error(e)) if e == "offline"));
    assert!(!a.busy());
}

#[test]
fn a_panic_while_handling_a_reply_ends_the_turn_instead_of_the_app() {
    // Issue #110: the desktop app quit while the assistant handled a reply. A panic in a step
    // now becomes an error in the transcript, and the conversation stays usable.
    let mut s = sample();
    let mut a = Agent::new(cfg("openai"), &s);
    let Step::Send(_) = a.ask("label the cities", &s) else { panic!("expected a request") };
    // Two calls: the first panics while it runs, the second is still queued.
    let two = json!({"choices": [{"message": {"role": "assistant", "content": null, "tool_calls": [
        {"id": "c1", "type": "function", "function": {"name": "test_panic", "arguments": "{}"}},
        {"id": "c2", "type": "function", "function": {"name": "search_commands", "arguments": "{\"query\": \"label\"}"}}]},
        "finish_reason": "tool_calls"}]});
    let step = a.receive(a.generation(), ok(two), &mut s);
    assert_eq!(step, Step::Done);
    assert!(!a.busy());
    assert!(a.transcript.iter().any(|e| matches!(e, Entry::Error(m) if m == INTERNAL_ERROR)));
    assert!(a.transcript.iter().any(|e| matches!(e, Entry::Note(m) if m.contains("a test tool panicked"))));
    // The next request answers the call that failed, so the provider accepts it.
    let Step::Send(req) = a.ask("try again", &s) else { panic!("expected a request") };
    let b = body(&req);
    let messages = b["messages"].as_array().cloned().unwrap_or_default();
    let result = |id: &str| {
        messages
            .iter()
            .find(|m| m["role"] == "tool" && m["tool_call_id"] == id)
            .and_then(|m| m["content"].as_str())
            .unwrap_or("")
            .to_string()
    };
    // The call that was running may have done something; the queued one never ran.
    assert!(result("c1").contains("may have partly taken effect"), "{messages:?}");
    assert!(result("c2").starts_with("Not run"), "{messages:?}");
    // Results go back in the order of the calls (Gemini matches id-less calls by order).
    let ids: Vec<&str> = messages.iter().filter(|m| m["role"] == "tool").filter_map(|m| m["tool_call_id"].as_str()).collect();
    assert_eq!(ids, ["c1", "c2"]);
    // A reply after the failure is read as usual.
    assert_eq!(a.receive(a.generation(), ok(say("Done.")), &mut s), Step::Done);
    assert!(matches!(a.transcript.last(), Some(Entry::Assistant(t)) if t == "Done."));
}

#[test]
fn anthropic_format() {
    let mut s = sample();
    let mut a = Agent::new(cfg("anthropic"), &s);
    let Step::Send(req) = a.ask("list layers", &s) else { panic!("expected a request") };
    assert_eq!(req.url, "https://api.anthropic.com/v1/messages");
    assert!(req.headers.iter().any(|(k, v)| k == "x-api-key" && v == "k"));
    assert!(req.headers.iter().any(|(k, _)| k == "anthropic-version"));
    let b = body(&req);
    assert!(b["system"].as_str().is_some_and(|t| t.contains("RustGIS")));
    assert_eq!(b["fallbacks"], "default");
    assert!(b["tools"][0]["input_schema"].is_object());
    let reply = json!({"content": [
        {"type": "thinking", "thinking": "", "signature": "sig"},
        {"type": "text", "text": "Let me look."},
        {"type": "tool_use", "id": "toolu_1", "name": "list_layers", "input": {}}
    ], "stop_reason": "tool_use"});
    let Step::Send(req) = a.receive(a.generation(), ok(reply), &mut s) else { panic!("expected a request") };
    let b = body(&req);
    let msgs = b["messages"].as_array().cloned().unwrap_or_default();
    assert_eq!(msgs.len(), 3);
    // The thinking block goes back unchanged; the result follows as a user message.
    assert_eq!(msgs[1]["content"][0]["signature"], "sig");
    assert_eq!(msgs[2]["content"][0]["type"], "tool_result");
    assert_eq!(msgs[2]["content"][0]["tool_use_id"], "toolu_1");
    // The next user message joins the results' user message.
    let mut a2 = Agent::new(cfg("anthropic"), &s);
    let Step::Send(_) = a2.ask("x", &s) else { panic!("expected a request") };
    let _ =
        a2.receive(a2.generation(), ok(json!({"content": [{"type": "tool_use", "id": "t", "name": "list_layers", "input": {}}]})), &mut s);
    a2.stop();
    let Step::Send(req) = a2.ask("y", &s) else { panic!("expected a request") };
    let msgs = body(&req)["messages"].as_array().cloned().unwrap_or_default();
    assert!(msgs.windows(2).all(|w| w[0]["role"] != w[1]["role"]), "{msgs:?}");
    // A refusal runs nothing.
    let mut a3 = Agent::new(cfg("anthropic"), &s);
    let Step::Send(_) = a3.ask("x", &s) else { panic!("expected a request") };
    let refused = json!({"content": [{"type": "tool_use", "id": "t", "name": "run_command", "input": {"command": "layer.remove", "params": {"layer": "Cities"}}}], "stop_reason": "refusal"});
    assert_eq!(a3.receive(a3.generation(), ok(refused), &mut s), Step::Done);
    assert!(s.map().layer_by_name("Cities").is_some());
}

#[test]
fn gemini_format() {
    let mut s = sample();
    let mut a = Agent::new(cfg("gemini"), &s);
    let Step::Send(req) = a.ask("hide the rivers", &s) else { panic!("expected a request") };
    assert!(req.url.ends_with("/models/gemini-3.8-flash:generateContent"), "{}", req.url);
    let b = body(&req);
    let decls = b["tools"][0]["functionDeclarations"].as_array().cloned().unwrap_or_default();
    let run = decls.iter().find(|d| d["name"] == "run_command").cloned().unwrap_or(Value::Null);
    // A free-form object parameter goes as JSON text.
    assert_eq!(run["parameters"]["properties"]["params"]["type"], "string");
    let reply = json!({"candidates": [{"content": {"role": "model", "parts": [
        {"functionCall": {"name": "run_command", "args": {"command": "layer.visibility", "params": "{\"layer\": \"Rivers\", \"visible\": false}"}}, "thoughtSignature": "abc"}
    ]}, "finishReason": "STOP"}]});
    let Step::Send(req) = a.receive(a.generation(), ok(reply), &mut s) else { panic!("expected a request") };
    assert!(s.map().layer_by_name("Rivers").is_some_and(|l| !l.visible));
    let b = body(&req);
    let contents = b["contents"].as_array().cloned().unwrap_or_default();
    assert_eq!(contents[1]["parts"][0]["thoughtSignature"], "abc");
    assert_eq!(contents[2]["parts"][0]["functionResponse"]["name"], "run_command");
    assert!(contents[2]["parts"][0]["functionResponse"].get("id").is_none());
}

#[test]
fn steps_are_bounded_and_stop_works() {
    let mut s = sample();
    let mut a = Agent::new(cfg("openai"), &s);
    let mut n = 0;
    run_blocking(
        &mut a,
        "loop forever",
        &mut s,
        |_| {
            n += 1;
            ok(call(&format!("c{n}"), "list_layers", json!({})))
        },
        |_, _, _| true,
    );
    assert_eq!(n, MAX_STEPS as usize);
    assert!(matches!(a.transcript.last(), Some(Entry::Note(_))));
    // A stale response (after a stop) is dropped.
    let Step::Send(_) = a.ask("again", &s) else { panic!("expected a request") };
    let g = a.generation();
    a.stop();
    assert_eq!(a.receive(g, ok(say("late")), &mut s), Step::Done);
    assert!(!a.transcript.iter().any(|e| matches!(e, Entry::Assistant(t) if t == "late")));
}

#[test]
fn missing_settings_are_reported() {
    let s = sample();
    let mut c = cfg("openai");
    c.key.clear();
    let mut a = Agent::new(c, &s);
    assert_eq!(a.ask("hi", &s), Step::Done);
    assert!(matches!(a.transcript.last(), Some(Entry::Error(_))));
    let mut c = cfg("ollama");
    c.key.clear();
    assert!(c.missing().is_none());
}

#[test]
fn model_lists() {
    let r = |v: Value| HttpResponse { status: 200, body: v.to_string().into_bytes() };
    assert_eq!(parse_models(Api::OpenAi, &r(json!({"data": [{"id": "b"}, {"id": "a"}]}))), Ok(vec!["a".to_string(), "b".to_string()]));
    let g = json!({"models": [
        {"name": "models/gemini-x", "supportedGenerationMethods": ["generateContent"]},
        {"name": "models/embed", "supportedGenerationMethods": ["embedContent"]}
    ]});
    assert_eq!(parse_models(Api::Gemini, &r(g)), Ok(vec!["gemini-x".to_string()]));
    let req = models_request(&cfg("anthropic"));
    assert_eq!(req.method, "GET");
    assert!(req.url.starts_with("https://api.anthropic.com/v1/models"));
}

#[test]
fn empty_replies_are_reported() {
    let mut s = sample();
    let mut a = Agent::new(cfg("ollama"), &s);
    let Step::Send(_) = a.ask("hi", &s) else { panic!("expected a request") };
    let empty = json!({"choices": [{"message": {"role": "assistant", "content": "", "reasoning": "…"}, "finish_reason": "stop"}]});
    assert_eq!(a.receive(a.generation(), ok(empty), &mut s), Step::Done);
    assert!(matches!(a.transcript.last(), Some(Entry::Note(n)) if n.contains("empty")));
}

#[test]
fn ordering_mixed_columns_never_panics() {
    use rustgis_engine::data::Value as V;
    let mut s = sample();
    let id = s.layer_id(&json!({"layer": "Cities"})).unwrap_or(0);
    // A column of mixed numbers, numeric text, words, NaN and nulls.
    let mixed = [V::Int(5), V::Text("abc".into()), V::Text("10".into()), V::Float(f64::NAN), V::Null, V::Text("9".into()), V::Bool(true)];
    if let Some(l) = s.map_mut().layer_mut(id) {
        let fi = l.dataset.fields.iter().position(|f| f.name == "name").unwrap_or(0);
        for (i, f) in l.dataset.features.iter_mut().enumerate() {
            if let Some(a) = f.attrs.get_mut(fi) {
                *a = mixed[i % mixed.len()].clone();
            }
        }
    }
    let t = ToolCall {
        id: "x".into(),
        name: "read_features".into(),
        args: json!({"layer": "Cities", "order_by": "name DESC", "fields": ["name"], "limit": 30}),
    };
    let r = tools::run(&mut s, &t);
    let v: Value = r.as_deref().ok().and_then(|r| serde_json::from_str(r).ok()).unwrap_or(Value::Null);
    let rows = v["rows"].as_array().cloned().unwrap_or_default();
    assert_eq!(rows.len(), 30, "{r:?}");
    // Numbers (numeric text too) first, largest first; empty values last.
    assert_eq!(rows[0]["name"], "10", "{r:?}");
    assert!(rows.iter().all(|r| !r["name"].is_null()), "{r:?}");
}

#[test]
fn always_allow_is_per_command_and_never_for_reserved_actions() {
    let mut s = sample();
    let mut a = Agent::new(cfg("openai"), &s);
    let Step::Send(_) = a.ask("x", &s) else { panic!("expected a request") };
    let dir = std::env::temp_dir();
    let export = |n: u32| json!({"command": "share.exportLayer", "params": {"layer": "Lakes", "path": dir.join(format!("rustgis-assistant-test-{n}.geojson"))}});
    let reply = json!({"choices": [{"message": {"role": "assistant", "content": null, "tool_calls": [
        {"id": "c0", "type": "function", "function": {"name": "run_command", "arguments": export(1).to_string()}},
        {"id": "c1", "type": "function", "function": {"name": "run_command", "arguments": export(1).to_string()}},
        {"id": "c2", "type": "function", "function": {"name": "run_command", "arguments": export(2).to_string()}},
        {"id": "c3", "type": "function", "function": {"name": "run_command", "arguments": json!({"command": "map.addTiles", "params": {"url": "https://t.example/{z}/{x}/{y}.png"}}).to_string()}},
        {"id": "c4", "type": "function", "function": {"name": "run_command", "arguments": json!({"command": "project.new"}).to_string()}}
    ]}, "finish_reason": "tool_calls"}]});
    assert_eq!(a.receive(a.generation(), ok(reply), &mut s), Step::Confirm);
    assert!(a.pending(&s).is_some_and(|p| p.2));
    // Allowing the export for the conversation allows that file only: the next export (another
    // file) asks again, and so do the tiles.
    assert_eq!(a.decide(true, true, &mut s), Step::Confirm);
    assert!(a.pending(&s).is_some_and(|p| p.0.starts_with("share.exportLayer") && p.2));
    assert_eq!(a.decide(false, false, &mut s), Step::Confirm);
    assert!(a.pending(&s).is_some_and(|p| p.0.starts_with("map.addTiles")));
    assert_eq!(a.decide(false, false, &mut s), Step::Confirm);
    // Replacing the project is never allowed for good.
    assert!(a.pending(&s).is_some_and(|p| p.0.starts_with("project.new") && !p.2));
    let _ = a.decide(false, false, &mut s);
    for n in [1, 2] {
        let _ = std::fs::remove_file(dir.join(format!("rustgis-assistant-test-{n}.geojson")));
    }
}

#[test]
fn stopping_a_request_drops_its_message() {
    let s = sample();
    let mut a = Agent::new(cfg("openai"), &s);
    let Step::Send(_) = a.ask("first", &s) else { panic!("expected a request") };
    a.stop();
    let Step::Send(req) = a.ask("second", &s) else { panic!("expected a request") };
    let msgs = body(&req)["messages"].as_array().cloned().unwrap_or_default();
    let users: Vec<&Value> = msgs.iter().filter(|m| m["role"] == "user").collect();
    assert_eq!(users.len(), 1);
    assert!(users[0]["content"].as_str().is_some_and(|t| t.starts_with("<map>") && t.ends_with("second")));
}

#[test]
fn malformed_params_are_refused() {
    let mut s = sample();
    let n = s.map().layers.len();
    for params in [json!("{\"layer\": "), json!("[1]"), json!(5)] {
        let c = ToolCall { id: "x".into(), name: "run_command".into(), args: json!({"command": "layer.remove", "params": params}) };
        assert!(tools::run(&mut s, &c).is_err());
    }
    assert_eq!(s.map().layers.len(), n, "no layer may be removed by a call whose parameters didn't parse");
    let c = ToolCall { id: "x".into(), name: "run_command".into(), args: json!({"command": "map.zoomFull", "params": "{}"}) };
    assert!(tools::run(&mut s, &c).is_ok());
}

#[test]
fn stopping_a_follow_up_keeps_the_history_valid() {
    let mut s = sample();
    for provider in ["anthropic", "openai", "gemini"] {
        let mut a = Agent::new(cfg(provider), &s);
        let Step::Send(_) = a.ask("x", &s) else { panic!("expected a request") };
        let reply = match provider {
            "anthropic" => json!({"content": [{"type": "tool_use", "id": "t", "name": "list_layers", "input": {}}]}),
            "gemini" => {
                json!({"candidates": [{"content": {"role": "model", "parts": [{"functionCall": {"name": "list_layers", "args": {}}}]}}]})
            }
            _ => call("t", "list_layers", json!({})),
        };
        // The calls ran and the follow-up request is out when the user stops.
        let Step::Send(_) = a.receive(a.generation(), ok(reply), &mut s) else { panic!("expected a follow-up") };
        a.stop();
        let Step::Send(req) = a.ask("y", &s) else { panic!("expected a request") };
        let b = body(&req);
        match provider {
            // Results and the new text share one user message.
            "anthropic" => {
                let msgs = b["messages"].as_array().cloned().unwrap_or_default();
                assert!(msgs.windows(2).all(|w| w[0]["role"] != w[1]["role"]), "{msgs:?}");
                let last = msgs.last().cloned().unwrap_or(Value::Null);
                assert_eq!(last["content"][0]["type"], "tool_result");
                assert_eq!(last["content"][1]["type"], "text");
            }
            // Tool results, then the user's message.
            "openai" => {
                let msgs = b["messages"].as_array().cloned().unwrap_or_default();
                let roles: Vec<&str> = msgs.iter().filter_map(|m| m["role"].as_str()).collect();
                assert_eq!(roles, ["system", "user", "assistant", "tool", "user"]);
            }
            _ => {
                let c = b["contents"].as_array().cloned().unwrap_or_default();
                assert_eq!(c.len(), 4);
                assert!(c[2]["parts"][0].get("functionResponse").is_some());
            }
        }
    }
}

// The MCP server.

use crate::{docs, mcp};

/// Sends one request and gives back its `result` (or the whole reply when it is an error).
fn rpc(server: &mcp::Server, s: &mut Session, method: &str, params: Value) -> Value {
    let msg = json!({"jsonrpc": "2.0", "id": 7, "method": method, "params": params}).to_string();
    let reply: Value = serde_json::from_str(&server.handle(s, &msg).unwrap()).unwrap();
    assert_eq!(reply["id"], 7);
    if reply.get("error").is_some() { reply } else { reply["result"].clone() }
}

fn call_tool(server: &mcp::Server, s: &mut Session, name: &str, args: Value) -> (bool, String) {
    let r = rpc(server, s, "tools/call", json!({"name": name, "arguments": args}));
    (r["isError"] == true, r["content"][0]["text"].as_str().unwrap_or("").to_string())
}

#[test]
fn mcp_initializes_and_lists_tools() {
    let (server, mut s) = (mcp::Server::default(), sample());
    let r = rpc(&server, &mut s, "initialize", json!({"protocolVersion": "2025-03-26", "capabilities": {}, "clientInfo": {"name": "t"}}));
    assert_eq!(r["protocolVersion"], "2025-03-26");
    assert_eq!(r["serverInfo"]["name"], "rustgis");
    assert!(r["capabilities"]["tools"].is_object());
    // A version we don't know gets our newest.
    let r = rpc(&server, &mut s, "initialize", json!({"protocolVersion": "1999-01-01"}));
    assert_eq!(r["protocolVersion"], mcp::VERSIONS[0]);
    // Notifications get no reply.
    assert!(server.handle(&mut s, r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#).is_none());
    assert_eq!(rpc(&server, &mut s, "ping", Value::Null), json!({}));

    let tools = rpc(&server, &mut s, "tools/list", json!({}));
    let names: Vec<&str> = tools["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
    for want in [
        "list_layers",
        "search_commands",
        "run_command",
        "search_tools",
        "describe_tool",
        "run_tool",
        "read_features",
        "view_map",
        "search_docs",
        "read_doc",
    ] {
        assert!(names.contains(&want), "{want} missing from {names:?}");
    }
    for t in tools["tools"].as_array().unwrap() {
        assert_eq!(t["inputSchema"]["type"], "object", "{}", t["name"]);
        assert!(t["description"].as_str().is_some_and(|d| !d.is_empty()));
        let runs = matches!(t["name"].as_str(), Some("run_command" | "run_tool"));
        assert_eq!(t["annotations"]["readOnlyHint"], !runs, "{}", t["name"]);
        assert_eq!(t["annotations"]["destructiveHint"], runs, "{}", t["name"]);
    }
}

#[test]
fn mcp_protocol_errors() {
    let (server, mut s) = (mcp::Server::default(), sample());
    let r: Value = serde_json::from_str(&server.handle(&mut s, "{not json").unwrap()).unwrap();
    assert_eq!(r["error"]["code"], -32700);
    assert_eq!(rpc(&server, &mut s, "nope/nothing", json!({}))["error"]["code"], -32601);
    assert_eq!(rpc(&server, &mut s, "tools/call", json!({"name": "no_such_tool"}))["error"]["code"], -32602);
    assert_eq!(rpc(&server, &mut s, "tools/call", json!({"name": "list_layers", "arguments": [1]}))["error"]["code"], -32602);
    assert_eq!(rpc(&server, &mut s, "resources/read", json!({"uri": "rustgis://docs/nope.md"}))["error"]["code"], -32002);
    // Batches: one reply per request, none for notifications only.
    let batch = r#"[{"jsonrpc":"2.0","id":1,"method":"ping"},{"jsonrpc":"2.0","method":"notifications/initialized"},{"jsonrpc":"2.0","id":2,"method":"nope"}]"#;
    let r: Value = serde_json::from_str(&server.handle(&mut s, batch).unwrap()).unwrap();
    assert_eq!(r.as_array().map(Vec::len), Some(2));
    assert_eq!(r[0]["id"], 1);
    assert_eq!(r[1]["error"]["code"], -32601);
    assert!(server.handle(&mut s, r#"[{"jsonrpc":"2.0","method":"notifications/initialized"}]"#).is_none());
    let r: Value = serde_json::from_str(&server.handle(&mut s, "[]").unwrap()).unwrap();
    assert_eq!(r["error"]["code"], -32600);
    let big = Value::Array(vec![json!({"jsonrpc": "2.0", "id": 1, "method": "ping"}); mcp::MAX_BATCH + 1]).to_string();
    let r: Value = serde_json::from_str(&server.handle(&mut s, &big).unwrap()).unwrap();
    assert_eq!(r["error"]["code"], -32600);
    // A response to us (we send no requests) gets no reply.
    assert!(server.handle(&mut s, r#"{"jsonrpc":"2.0","id":1,"result":{}}"#).is_none());
}

#[test]
fn mcp_runs_commands_and_shows_the_map() {
    let (server, mut s) = (mcp::Server::default(), sample());
    let (err, text) = call_tool(&server, &mut s, "list_layers", json!({}));
    assert!(!err && text.contains("Countries"), "{text}");
    let (err, text) = call_tool(&server, &mut s, "run_command", json!({"command": "map.basemap", "params": {"name": "ocean"}}));
    assert!(!err, "{text}");
    let (err, text) = call_tool(&server, &mut s, "read_features", json!({"layer": "Cities", "limit": 2}));
    assert!(!err && text.contains("\"rows\""), "{text}");

    let r = rpc(&server, &mut s, "tools/call", json!({"name": "view_map", "arguments": {"width": 320, "height": 200}}));
    assert_eq!(r["isError"], false);
    assert_eq!(r["content"][0]["type"], "image");
    assert_eq!(r["content"][0]["mimeType"], "image/png");
    use base64::Engine;
    let png = base64::engine::general_purpose::STANDARD.decode(r["content"][0]["data"].as_str().unwrap()).unwrap();
    assert!(png.starts_with(b"\x89PNG"));
    // Junk sizes are clamped, not refused.
    let r = rpc(&server, &mut s, "tools/call", json!({"name": "view_map", "arguments": {"width": -5, "height": 1e300}}));
    assert_eq!(r["isError"], false);
}

#[test]
fn mcp_limits_what_the_agent_may_do() {
    let path = std::env::temp_dir().join(format!("rustgis-mcp-{}.png", std::process::id()));
    let export = json!({"command": "share.exportMap", "params": {"path": path.to_string_lossy()}});

    let (server, mut s) = (mcp::Server::new(false), sample());
    let (err, text) = call_tool(&server, &mut s, "run_command", export.clone());
    assert!(err && text.contains("--yes"), "{text}");
    assert!(!path.exists());
    // The user's own automation never runs, even when allowed.
    let allowed = mcp::Server::new(true);
    for server in [&server, &allowed] {
        let (err, text) = call_tool(server, &mut s, "run_command", json!({"command": "script.run", "params": {"code": "x"}}));
        assert!(err && text.contains("reserved"), "{text}");
        let (err, _) = call_tool(server, &mut s, "run_command", json!({"command": "plugin.uninstall", "params": {"id": "x"}}));
        assert!(err);
    }
    // Opening a project or template reads a file.
    for (cmd, params) in [("project.open", json!({"path": "a.rgis"})), ("project.fromTemplate", json!({"path": "a.rgistemplate"}))] {
        let (err, text) = call_tool(&server, &mut s, "run_command", json!({"command": cmd, "params": params}));
        assert!(err && text.contains("--yes"), "{cmd}: {text}");
    }
    // A project from text can bring web layers in, which drawing the map would fetch.
    let (err, text) = call_tool(&server, &mut s, "run_command", json!({"command": "project.openText", "params": {"text": "{}"}}));
    assert!(err && text.contains("--yes"), "{text}");
    // The project is the agent's own: replacing it and undoing are allowed.
    let (err, text) = call_tool(&server, &mut s, "run_command", json!({"command": "project.new"}));
    assert!(!err, "{text}");
    let (err, text) = call_tool(&server, &mut s, "run_command", json!({"command": "project.sample"}));
    assert!(!err, "{text}");

    let (err, text) = call_tool(&allowed, &mut s, "run_command", export);
    assert!(!err, "{text}");
    assert!(path.exists());
    let _ = std::fs::remove_file(&path);
}

#[test]
fn mcp_serves_the_docs() {
    let (server, mut s) = (mcp::Server::default(), sample());
    let (err, text) = call_tool(&server, &mut s, "search_docs", json!({"query": "graduated colours"}));
    assert!(!err, "{text}");
    let v: Value = serde_json::from_str(&text).unwrap();
    assert!(v["results"].as_array().unwrap().iter().any(|r| r["page"] == "user-guide/cartography.md"), "{text}");
    assert!(v["results"][0]["url"].as_str().unwrap().starts_with(docs::SITE));
    // A plural finds the singular; words ending in "ss" keep their ending.
    let (_, text) = call_tool(&server, &mut s, "search_docs", json!({"query": "classes"}));
    assert!(text.contains("class"), "{text}");
    let (err, _) = call_tool(&server, &mut s, "search_docs", json!({"query": "?"}));
    assert!(err);

    let (err, text) = call_tool(&server, &mut s, "read_doc", json!({"page": "cli"}));
    assert!(!err && text.contains("Command Line"), "{text}");
    let (err, text) = call_tool(&server, &mut s, "read_doc", json!({"page": "cli.md", "section": "chaining commands"}));
    assert!(!err && text.contains("--cmd") && text.contains("#chaining-commands"), "{text}");
    // A section comes with its heading and its subsections.
    let (err, text) = call_tool(&server, &mut s, "read_doc", json!({"page": "mcp.md", "section": "MCP Server"}));
    assert!(!err && text.contains("## Tools") && text.contains("# MCP Server"), "{text}");
    let (err, text) = call_tool(&server, &mut s, "read_doc", json!({"page": "cli.md", "section": "no such section"}));
    assert!(err && text.contains("Chaining commands"), "{text}");
    let (err, _) = call_tool(&server, &mut s, "read_doc", json!({"page": "../../etc/passwd"}));
    assert!(err);

    let list = rpc(&server, &mut s, "resources/list", json!({}));
    assert_eq!(list["resources"].as_array().unwrap().len(), docs::PAGES.len());
    let r = rpc(&server, &mut s, "resources/read", json!({"uri": "rustgis://docs/user-guide/maps.md"}));
    assert!(r["contents"][0]["text"].as_str().unwrap().starts_with("# "));
}

#[test]
fn docs_pages_and_addresses() {
    // Every page in docs/ is built in (add new pages to `docs::PAGES`).
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs");
    let mut found = Vec::new();
    let mut dirs = vec![root.clone()];
    while let Some(d) = dirs.pop() {
        for e in std::fs::read_dir(&d).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                dirs.push(p);
            } else if p.extension().is_some_and(|x| x == "md") {
                found.push(p.strip_prefix(&root).unwrap().to_string_lossy().replace('\\', "/"));
            }
        }
    }
    for f in &found {
        assert!(docs::page(f).is_some(), "docs/{f} is missing from docs::PAGES");
    }
    assert_eq!(found.len(), docs::PAGES.len());

    assert_eq!(docs::page("index.md").unwrap().url(), "https://rustgis.app/");
    assert_eq!(docs::page("user-guide").unwrap().url(), "https://rustgis.app/user-guide/");
    assert_eq!(docs::page("https://rustgis.app/user-guide/maps/").unwrap().path, "user-guide/maps.md");
    assert_eq!(docs::slug("Chaining commands"), "chaining-commands");
    assert_eq!(docs::slug("What the agent may do"), "what-the-agent-may-do");
    // Headings in code blocks aren't sections.
    let text = "# T\nintro\n```sh\n# a comment\n```\n~~~\n# also code\n~~~\n    # indented code\n````md\n```\n# inside a longer fence\n```not a close\n````\n## Next\nbody\n### Sub\nmore\n## Last\n#tag\n";
    let secs = docs::sections(text);
    assert_eq!(secs.iter().map(|s| s.0).collect::<Vec<_>>(), ["T", "Next", "Sub", "Last"]);
    assert_eq!(secs.get(1).map(|s| s.1), Some("body\n"));
}

#[test]
fn mcp_hostile_messages_never_panic() {
    let junk = [
        Value::Null,
        json!(1),
        json!("x"),
        json!([]),
        json!({}),
        json!({"name": 5}),
        json!({"name": "run_command", "arguments": {"command": 5, "params": "{"}}),
        json!({"name": "run_tool", "arguments": {"tool": "buffer", "params": {"input": [], "distance": "far"}}}),
        json!({"name": "read_features", "arguments": {"layer": {}, "limit": -1, "order_by": 3, "fields": [1]}}),
        json!({"name": "view_map", "arguments": {"width": "big"}}),
        json!({"name": "search_docs", "arguments": {"query": "é".repeat(5000), "limit": 1e30}}),
        json!({"name": "read_doc", "arguments": {"page": "", "section": "#"}}),
        json!({"uri": 3}),
        json!({"protocolVersion": []}),
    ];
    let methods = [
        "initialize",
        "ping",
        "tools/list",
        "tools/call",
        "resources/list",
        "resources/read",
        "resources/templates/list",
        "prompts/list",
        "x",
    ];
    for allow in [false, true] {
        let (server, mut s) = (mcp::Server::new(allow), sample());
        for m in methods {
            for p in &junk {
                let msg = json!({"jsonrpc": "2.0", "id": "a", "method": m, "params": p}).to_string();
                assert!(server.handle(&mut s, &msg).is_some(), "{m} {p}");
            }
        }
        for raw in ["", "[]", "[{}]", "null", "\"x\"", r#"{"id":{},"method":"ping"}"#, r#"{"id":1,"method":7}"#, "\u{0}"] {
            let _ = server.handle(&mut s, raw);
        }
    }
}
