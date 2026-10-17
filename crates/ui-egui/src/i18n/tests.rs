use super::*;

#[test]
fn parse_and_escape() {
    let (m, e) = parse("# c\nOpen\tÖffnen\nA\\nB\tX\\nY\nbad line\n");
    assert_eq!(m.get("Open").map(String::as_str), Some("Öffnen"));
    assert_eq!(m.get("A\nB").map(String::as_str), Some("X\nY"));
    assert_eq!(e.len(), 1);
}

#[test]
fn every_catalog_parses_cleanly_and_keeps_placeholders() {
    for l in &LANGUAGES[1..] {
        let (m, errors) = parse(l.source);
        assert!(errors.is_empty(), "{}: {errors:?}", l.code);
        for (en, tr) in &m {
            let names = |s: &str| -> Vec<String> {
                let mut v: Vec<String> = s.split('{').skip(1).filter_map(|p| p.split_once('}').map(|x| x.0.to_string())).collect();
                v.sort();
                v
            };
            assert_eq!(names(en), names(tr), "{}: placeholders differ in {en:?}", l.code);
        }
    }
}

#[test]
fn lookup_fallback_and_tags() {
    assert_eq!(tr(Lang::EN, "Explore"), "Explore");
    assert_eq!(tr(Lang::EN, "no such string"), "no such string");
    assert_eq!(lang_from_tag("de_DE.UTF-8").map(Lang::code), Some("de"));
    assert_eq!(lang_from_tag("pt-BR").map(Lang::code), Some("pt"));
    assert_eq!(lang_from_tag("fa_IR.UTF-8").map(Lang::code), Some("fa"));
    assert_eq!(lang_from_tag("zh-Hans-CN").map(Lang::code), Some("zh"));
    assert_eq!(lang_from_tag("zh_CN.UTF-8").map(Lang::code), Some("zh"));
    assert_eq!(lang_from_tag("zh-TW"), None);
    assert_eq!(lang_from_tag("zh-Hant-HK"), None);
    assert_eq!(lang_from_tag("C").map(Lang::code), Some("en"));
    assert_eq!(lang_from_tag("xx-YY"), None);
    assert_eq!(first_supported(["xx", "fr-CA", "de"]).map(Lang::code), Some("fr"));
    assert_eq!(fmt("{n} of {total} selected", &[("n", "3"), ("total", "{n}")]), "3 of {n} selected");
}

#[test]
fn plugin_strings_use_the_plugins_translations() {
    use rustgis_engine::plugins;
    let pkg = plugins::Package::from_zip(&plugins::testing::package("i18n-fallback", &[])).unwrap();
    plugins::load(pkg, "test").unwrap();
    let de = Lang::from_code("de").unwrap();
    // Not in RustGIS's catalog: the plugin's `i18n` has it.
    assert_eq!(tr(de, "Hello"), "Hallo");
    assert_eq!(tr(Lang::EN, "Hello"), "Hello");
    assert_eq!(tr(Lang::from_code("fr").unwrap(), "Hello"), "Hello");
    // RustGIS's own catalog comes first.
    assert_eq!(tr(de, "Open"), de.0.catalog().get("Open").copied().unwrap_or("Open"));
    plugins::unload("i18n-fallback");
}

#[test]
fn start_up_project_strings_have_real_translations() {
    let keys = [
        "Last Project",
        "Choose Project…",
        "No project opened or saved yet",
        "What RustGIS opens with: the World Atlas sample, a blank map, a project you choose, or the last project opened or saved.",
        "RustGIS didn't close properly last time, so it started with the World Atlas instead of your start-up project. Open the project from the Project menu when you're ready.",
        "The start-up project {path} isn't a RustGIS project file.",
        "The start-up project {path} didn't open: {error}",
    ];
    for l in &LANGUAGES[1..] {
        let (catalog, _) = parse(l.source);
        for key in keys {
            let translated = catalog.get(key).unwrap_or_else(|| panic!("{}: missing {key}", l.code));
            assert_ne!(translated, key, "{}: untranslated {key}", l.code);
        }
    }
}

#[test]
fn chatgpt_recovery_and_controls_have_real_translations() {
    use rustgis_assistant::{Error, Recovery};
    let mut keys = vec![
        "Weekly usage",
        "Open browser again",
        "Copy sign-in link",
        "Sign in to choose a model.",
        "Choose an account",
        "Diagnostics",
        "No models are available for this account.",
        "The reply stopped before completion; no actions were run.",
    ];
    let errors: Vec<_> = [
        Recovery::SignIn,
        Recovery::AccountChanged,
        Recovery::Consent,
        Recovery::Timeout,
        Recovery::Cancelled,
        Recovery::Auth,
        Recovery::Configuration,
        Recovery::Unsupported,
        Recovery::Ineligible,
        Recovery::Usage,
        Recovery::Later,
    ]
    .into_iter()
    .map(|r| Error::new(r, ""))
    .collect();
    keys.extend(errors.iter().map(Error::message));
    for l in &LANGUAGES[1..] {
        let (catalog, _) = parse(l.source);
        for key in &keys {
            let translated = catalog.get(*key).unwrap_or_else(|| panic!("{}: missing {key}", l.code));
            assert_ne!(translated, key, "{}: untranslated {key}", l.code);
        }
    }
}

#[test]
fn assistant_note_translations_are_real_when_available() {
    for l in &LANGUAGES[1..] {
        let (m, _) = parse(l.source);
        for template in rustgis_assistant::provider::NOTE_TEMPLATES {
            // AGENTS.md permits English fallback for new keys until the maintainer translates.
            if let Some(tr) = m.get(*template) {
                assert_ne!(tr, template, "{}: {template:?} isn't translated", l.code);
            }
        }
    }
}

#[test]
fn skills_controls_are_in_every_catalog() {
    let keys = [
        "Skills",
        "Built in",
        "Plugin",
        "Yours",
        "Shared",
        "Add skill…",
        "Skill (SKILL.md or .zip)",
        "Add folder…",
        "Added: {names}",
        "Skills for the AI Assistant: {names}",
    ];
    // Sentences, which no language leaves in English.
    let sentences = [
        "Your skills are kept in the skills folder of the app's settings folder. Shared skills (~/.agents/skills) are off until you turn them on.",
        "The assistant follows a skill's instructions when it uses it. Whatever they say, its commands that need your approval still ask.",
        "It has no code: nothing in it runs.",
    ];
    for l in &LANGUAGES[1..] {
        let (m, _) = parse(l.source);
        for key in keys.iter().chain(&sentences) {
            assert!(m.contains_key(*key), "{}: missing {key:?}", l.code);
        }
        for key in sentences {
            assert_ne!(m.get(key).map(String::as_str), Some(key), "{}: {key:?} isn't translated", l.code);
        }
    }
}

#[test]
fn letting_agents_control_rustgis_is_in_every_catalog() {
    let keys = [
        "Let your Agents Control RustGIS",
        "Agents on this computer that connect (Claude Code, Codex, Cursor and others) work on the map open here, with RustGIS's tools: they can open and save files and use the web. Scripts and plugin management stay off.",
        "Add this MCP server to your agent:",
        "Or run, for Claude Code or Codex:",
    ];
    for l in &LANGUAGES[1..] {
        let (m, _) = parse(l.source);
        for key in keys {
            let tr = m.get(key).unwrap_or_else(|| panic!("{}: missing {key:?}", l.code));
            assert_ne!(tr, key, "{}: {key:?} isn't translated", l.code);
        }
    }
}
