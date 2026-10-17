//! A plugin for tests, written in the WebAssembly text format (feature `testing`).
//!
//! It answers by the first letter of the command or tool id. Requests are serialized with
//! sorted keys, so the id starts at byte 15 (`{"abi":1,"id":"…`) and a one-letter command's
//! params start at byte 56 (`{"abi":1,"id":"f","kind":"command","lang":"en","params":…}`; every
//! interface language code is two letters).
//!
//! - `h` (hello): logs "hi" and returns `{"hello": "world"}`.
//! - `f` (forward): sends its params to RustGIS as a host request and returns the reply.
//! - `s` (spin): loops forever (stopped by the fuel limit).
//! - `g` (garbage): returns a reply outside its memory.
//! - `o` (out of memory): asks for 4 GB, then traps.
//! - `e` (error): fails with "nope".
//! - `m` (make, a tool): returns a point layer "Made" and a message.
//! - `x` (select, a tool): selects rows 0 and 2 of the layer "Cities".
//! - `r` (read, a tool with a `file` parameter, declared with the `files` permission): returns
//!   what `m` does. A request carrying files has `files` before `id` (byte 15 is then the `"`
//!   closing `"files"`), so it's answered the same way.

use std::io::Write;

const WAT: &str = r#"
(module
  (import "rustgis" "call" (func $call (param i32 i32) (result i32)))
  (import "rustgis" "result" (func $result (param i32)))
  (import "rustgis" "log" (func $log (param i32 i32)))
  (memory (export "memory") 2)
  (global $heap (mut i32) (i32.const 8192))
  (data (i32.const 0) "{\22ok\22:{\22hello\22:\22world\22}}")
  (data (i32.const 64) "hi")
  (data (i32.const 128) "{\22error\22:\22nope\22}")
  (data (i32.const 256) "{\22ok\22:{\22layers\22:[{\22name\22:\22Made\22,\22geojson\22:{\22type\22:\22FeatureCollection\22,\22features\22:[{\22type\22:\22Feature\22,\22properties\22:{\22n\22:1},\22geometry\22:{\22type\22:\22Point\22,\22coordinates\22:[1,2]}}]}}],\22messages\22:[\22made\22]}}")
  (data (i32.const 1024) "{\22ok\22:{\22selection\22:{\22layer\22:\22Cities\22,\22rows\22:[0,2]}}}")
  (func $alloc (export "rustgis_alloc") (param $n i32) (result i32)
    (local $p i32)
    (local.set $p (global.get $heap))
    (global.set $heap (i32.add (global.get $heap) (local.get $n)))
    (if (i32.gt_u (global.get $heap) (i32.mul (memory.size) (i32.const 65536)))
      (then (drop (memory.grow (i32.add (i32.div_u (local.get $n) (i32.const 65536)) (i32.const 1))))))
    (local.get $p))
  (func $reply (param $p i32) (param $n i32) (result i64)
    (i64.or (i64.shl (i64.extend_i32_u (local.get $p)) (i64.const 32)) (i64.extend_i32_u (local.get $n))))
  (func (export "rustgis_call") (param $p i32) (param $n i32) (result i64)
    (local $c i32) (local $len i32) (local $buf i32)
    (local.set $c (i32.load8_u (i32.add (local.get $p) (i32.const 15))))
    (if (i32.eq (local.get $c) (i32.const 104)) (then
      (call $log (i32.const 64) (i32.const 2))
      (return (call $reply (i32.const 0) (i32.const 24)))))
    (if (i32.eq (local.get $c) (i32.const 102)) (then
      (local.set $len (call $call (i32.add (local.get $p) (i32.const 56)) (i32.sub (local.get $n) (i32.const 57))))
      (local.set $buf (call $alloc (local.get $len)))
      (call $result (local.get $buf))
      (return (call $reply (local.get $buf) (local.get $len)))))
    (if (i32.eq (local.get $c) (i32.const 115)) (then (loop $l (br $l))))
    (if (i32.eq (local.get $c) (i32.const 103)) (then (return (i64.const 0x7fff000000000010))))
    (if (i32.eq (local.get $c) (i32.const 111)) (then (drop (memory.grow (i32.const 65000))) unreachable))
    (if (i32.eq (local.get $c) (i32.const 109)) (then (return (call $reply (i32.const 256) (i32.const 195)))))
    (if (i32.or (i32.eq (local.get $c) (i32.const 114)) (i32.eq (local.get $c) (i32.const 34)))
      (then (return (call $reply (i32.const 256) (i32.const 195)))))
    (if (i32.eq (local.get $c) (i32.const 120)) (then (return (call $reply (i32.const 1024) (i32.const 52)))))
    (call $reply (i32.const 128) (i32.const 16)))
)
"#;

/// The test module.
pub fn wasm() -> Vec<u8> {
    wat::parse_str(WAT).unwrap_or_default()
}

/// A manifest for the test module, with the given id and permissions.
pub fn manifest(id: &str, permissions: &[&str]) -> String {
    let mut m = serde_json::json!({
        "id": id,
        "name": format!("Test {id}"),
        "version": "1.0.0",
        "description": "A plugin for tests",
        "permissions": permissions,
        "commands": [
            {"id": "h", "label": "Hello", "icon": "star"},
            {"id": "f", "label": "Forward", "params": "a host request"},
            {"id": "s", "label": "Spin"},
            {"id": "g", "label": "Garbage"},
            {"id": "o", "label": "Out of memory"},
            {"id": "e", "label": "Error"}
        ],
        "tools": [
            {"id": "m", "name": "Make Points", "summary": "Makes a point", "params": [
                {"name": "input", "label": "Input", "kind": "vectorLayer", "required": true},
                {"name": "size", "label": "Size", "kind": "number", "default": 3},
                {"name": "mode", "label": "Mode", "kind": "choice", "choices": ["a", "b"], "default": "a"}
            ]},
            {"id": "x", "name": "Select Two", "params": [{"name": "input", "label": "Input", "kind": "layer"}]}
        ],
        "i18n": {"de": {"Hello": "Hallo"}}
    });
    // A file parameter needs `files`.
    if permissions.contains(&"files")
        && let Some(tools) = m["tools"].as_array_mut()
    {
        tools.push(serde_json::json!({"id": "r", "name": "Read File", "params": [
            {"name": "input", "label": "Input", "kind": "layer"},
            {"name": "model", "label": "Model", "kind": "file", "required": true, "extensions": ["nc", "nc4"]}
        ]}));
    }
    m.to_string()
}

/// A `.rgplugin` package of the test module.
pub fn package(id: &str, permissions: &[&str]) -> Vec<u8> {
    zip_of(&[("plugin.json", manifest(id, permissions).as_bytes()), ("plugin.wasm", &wasm())])
}

/// A skills package (no code) with the given skills, each a `SKILL.md` naming and describing it.
pub fn skills_package(id: &str, skills: &[&str]) -> Vec<u8> {
    let manifest = serde_json::json!({"id": id, "name": format!("Skills {id}"), "version": "1.0.0", "runtime": "skills", "skills": skills})
        .to_string();
    let texts: Vec<(String, String)> = skills
        .iter()
        .map(|s| (format!("skills/{s}/SKILL.md"), format!("---\nname: {s}\ndescription: The {s} skill.\n---\n\n# {s}\nDo {s}.\n")))
        .collect();
    let mut files: Vec<(&str, &[u8])> = vec![("plugin.json", manifest.as_bytes())];
    files.extend(texts.iter().map(|(p, t)| (p.as_str(), t.as_bytes())));
    zip_of(&files)
}

/// A zip archive of `files` (name, bytes), stored uncompressed.
pub fn zip_of(files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (name, bytes) in files {
        if w.start_file(*name, opts).is_err() || w.write_all(bytes).is_err() {
            return Vec::new();
        }
    }
    w.finish().map(|c| c.into_inner()).unwrap_or_default()
}

/// The argument that tells a test binary, started as a process plugin, to act as one (see
/// [`serve_if_child`]). It doubles as a test filter that matches no test.
const CHILD_MARK: &str = "rustgis-plugin-child-mode";

/// `run` for a process plugin played by this test binary: it starts the test named `test` (in
/// the crate's tests, it calls [`serve_if_child`]) and nothing else.
#[cfg(not(target_arch = "wasm32"))]
pub fn child_run(test: &str) -> Vec<String> {
    let exe = std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_default();
    [exe.as_str(), test, CHILD_MARK, "--exact", "--nocapture", "--test-threads=1"].iter().map(|s| s.to_string()).collect()
}

/// A manifest for the process test plugin: the test module's commands and tools, run by `run`.
/// With `files`, its tool `r` takes a file.
pub fn process_manifest(id: &str, permissions: &[&str], run: &[String]) -> String {
    let mut m: serde_json::Value = serde_json::from_str(&manifest(id, permissions)).unwrap_or_default();
    m["runtime"] = serde_json::json!("process");
    m["run"] = serde_json::json!(run);
    if let Some(c) = m["commands"].as_array_mut() {
        c.push(serde_json::json!({"id": "n", "label": "Noisy"}));
        c.push(serde_json::json!({"id": "c", "label": "Crash"}));
    }
    if let Some(t) = m["tools"].as_array_mut() {
        t.push(serde_json::json!({"id": "w", "name": "Work Forever", "params": []}));
    }
    m.to_string()
}

/// In a test binary started by a process test plugin ([`child_run`]): answers one request as the
/// test module does, then exits. Elsewhere it returns at once.
///
/// - `h` logs "hi" and returns `{"hello": "world"}`; `f` sends its params as a host request
///   and returns the reply; `e` fails with "nope"; `n` prints a line that isn't JSON and a JSON object that isn't a message first;
///   `c` writes "boom" to its error output and exits without answering; `s` never answers;
///   `g` starts another copy of itself that never answers, logs its process id and never
///   answers either.
/// - Tools: `m` returns the layer "Made"; `x` selects rows 0 and 2 of "Cities"; `r` returns the
///   names of the files it was given as its messages; `w` logs "working", reports
///   progress "step 1 of 3" and never answers; `m` reports progress "making" first.
#[cfg(not(target_arch = "wasm32"))]
pub fn serve_if_child() {
    use serde_json::{Value, json};
    use std::io::{BufRead, Write};
    if !std::env::args().any(|a| a == CHILD_MARK) {
        return;
    }
    let mut out = std::io::stdout();
    // The test harness has printed "test … " without a line end: start a line of our own.
    let _ = writeln!(out);
    let mut input = std::io::stdin().lock();
    let mut line = String::new();
    let _ = input.read_line(&mut line);
    let req: Value = serde_json::from_str(&line).unwrap_or_default();
    let params = req.get("params").cloned().unwrap_or_default();
    let reply = match req.get("id").and_then(Value::as_str).and_then(|s| s.chars().next()) {
        Some('h') => {
            let _ = writeln!(out, "{}", json!({"log": "hi"}));
            json!({"ok": {"hello": "world"}})
        }
        Some('f') => {
            let _ = writeln!(out, "{}", json!({"call": {"op": params["op"], "params": params["params"]}}));
            let _ = out.flush();
            let mut answer = String::new();
            let _ = input.read_line(&mut answer);
            serde_json::from_str(&answer).unwrap_or_else(|_| json!({"error": "no answer"}))
        }
        Some('n') => {
            let _ = writeln!(out, "a library printing");
            let _ = writeln!(out, "{}", json!({"library": "a JSON object of its own"}));
            json!({"ok": {"n": 1}})
        }
        Some('c') => {
            eprintln!("boom");
            std::process::exit(3);
        }
        Some('w') => {
            let _ = writeln!(out, "{}", json!({"log": "working"}));
            let _ = writeln!(out, "{}", json!({"progress": "step 1 of 3"}));
            let _ = out.flush();
            loop {
                std::thread::sleep(std::time::Duration::from_secs(1));
            }
        }
        Some('s') => loop {
            std::thread::sleep(std::time::Duration::from_secs(1));
        },
        Some('g') => {
            let argv = child_run(&std::env::args().nth(1).unwrap_or_default());
            let spawned = argv.split_first().and_then(|(exe, args)| {
                std::process::Command::new(exe)
                    .args(args)
                    .stdin(std::process::Stdio::piped())
                    .stdout(std::process::Stdio::null())
                    .spawn()
                    .ok()
            });
            if let Some(mut grandchild) = spawned {
                if let Some(mut stdin) = grandchild.stdin.take() {
                    let _ = writeln!(stdin, "{}", json!({"id": "s", "params": {}}));
                }
                let _ = writeln!(out, "{}", json!({"log": grandchild.id().to_string()}));
                let _ = out.flush();
            }
            loop {
                std::thread::sleep(std::time::Duration::from_secs(1));
            }
        }
        Some('m') => {
            let _ = writeln!(out, "{}", json!({"progress": "making"}));
            json!({"ok": {"layers": [{"name": "Made", "geojson": {"type": "FeatureCollection", "features": [
            {"type": "Feature", "properties": {"n": 1}, "geometry": {"type": "Point", "coordinates": [1, 2]}}]}}], "messages": ["made"]}})
        }
        Some('x') => json!({"ok": {"selection": {"layer": "Cities", "rows": [0, 2]}}}),
        Some('r') => {
            let names: Vec<Value> = req["files"].as_object().map(|f| f.values().map(|v| v["name"].clone()).collect()).unwrap_or_default();
            json!({"ok": {"layers": [], "messages": names}})
        }
        _ => json!({"error": "nope"}),
    };
    let _ = writeln!(out, "{reply}");
    let _ = out.flush();
    std::process::exit(0);
}
