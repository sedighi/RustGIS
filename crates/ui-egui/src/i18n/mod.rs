//! Interface languages. Strings in the code stay English and are the lookup keys; a catalog per
//! language (`<code>.tsv`) maps them to display text when the UI is drawn. Command ids, project
//! files, the CLI and data never see translated text, so scripts are unaffected. A string with no
//! translation shows in English, so coverage can grow one entry at a time.
//!
//! The `language` preference is `auto` by default: the UI follows the system's languages
//! (`RUSTGIS_LANGUAGE`, then the OS or browser preferences) when there is a catalog, else English.
//! Settings › Language picks one by hand.
//!
//! The language set follows GeoLibre's, minus scripts egui can't yet shape or lay out
//! (Arabic, Persian, Hebrew, Hindi, Thai, Georgian).
//!
//! # Catalog format
//! UTF-8, one `English<TAB>Translation` pair per line; `#` starts a comment line; `\n` and `\t`
//! inside a field are a line break and a tab. `{name}` placeholders must be kept (they may move).
//!
//! # Adding a language
//! Add `xx.tsv` (copy the keys of another catalog) and one row to [`LANGUAGES`].

use std::cell::Cell;
use std::collections::HashMap;
use std::sync::OnceLock;

/// The preference value that follows the system.
pub const AUTO: &str = "auto";

/// Writing systems that need an extra font.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Script {
    /// Latin, Cyrillic, Greek or Georgian: covered by the interface fonts.
    Alphabetic,
    Cjk(CjkFont),
    Persian,
}

/// Which CJK face reads best for a language (the glyph shapes differ by region).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CjkFont {
    SimplifiedChinese,
    Japanese,
    Korean,
}

pub struct LangInfo {
    /// Lowercase BCP 47 code; also the preference value.
    pub code: &'static str,
    /// The language's name in itself.
    pub name: &'static str,
    /// The name in English (for the picker's tooltip).
    pub english: &'static str,
    pub script: Script,
    source: &'static str,
    catalog: OnceLock<HashMap<&'static str, &'static str>>,
}

macro_rules! lang {
    ($code:literal, $name:literal, $en:literal, $script:expr, $file:literal) => {
        LangInfo { code: $code, name: $name, english: $en, script: $script, source: include_str!($file), catalog: OnceLock::new() }
    };
}

/// The registry. English first: it is the source language and the fallback.
pub static LANGUAGES: [LangInfo; 16] = [
    LangInfo { code: "en", name: "English", english: "English", script: Script::Alphabetic, source: "", catalog: OnceLock::new() },
    lang!("de", "Deutsch", "German", Script::Alphabetic, "de.tsv"),
    lang!("es", "Español", "Spanish", Script::Alphabetic, "es.tsv"),
    lang!("fr", "Français", "French", Script::Alphabetic, "fr.tsv"),
    lang!("it", "Italiano", "Italian", Script::Alphabetic, "it.tsv"),
    lang!("nl", "Nederlands", "Dutch", Script::Alphabetic, "nl.tsv"),
    lang!("pt", "Português", "Portuguese", Script::Alphabetic, "pt.tsv"),
    lang!("ru", "Русский", "Russian", Script::Alphabetic, "ru.tsv"),
    lang!("tr", "Türkçe", "Turkish", Script::Alphabetic, "tr.tsv"),
    lang!("az", "Azərbaycanca", "Azerbaijani", Script::Alphabetic, "az.tsv"),
    lang!("id", "Bahasa Indonesia", "Indonesian", Script::Alphabetic, "id.tsv"),
    lang!("vi", "Tiếng Việt", "Vietnamese", Script::Alphabetic, "vi.tsv"),
    lang!("fa", "فارسی", "Persian", Script::Persian, "fa.tsv"),
    lang!("zh", "简体中文", "Chinese (Simplified)", Script::Cjk(CjkFont::SimplifiedChinese), "zh.tsv"),
    lang!("ja", "日本語", "Japanese", Script::Cjk(CjkFont::Japanese), "ja.tsv"),
    lang!("ko", "한국어", "Korean", Script::Cjk(CjkFont::Korean), "ko.tsv"),
    lang!("ka", "ქართული", "Georgian", Script::Alphabetic, "ka.tsv"),
];

fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c == '\\' {
            match it.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some('\\') => out.push('\\'),
                Some(o) => {
                    out.push('\\');
                    out.push(o);
                }
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Parses a catalog. Malformed lines are skipped and reported.
pub fn parse(source: &str) -> (HashMap<String, String>, Vec<String>) {
    let mut map = HashMap::new();
    let mut errors = Vec::new();
    for (i, line) in source.lines().enumerate() {
        let line = line.trim_end_matches('\r');
        if line.trim().is_empty() || line.starts_with('#') {
            continue;
        }
        match line.split_once('\t') {
            Some((en, tr)) if !en.is_empty() && !tr.trim().is_empty() => {
                map.insert(unescape(en), unescape(tr));
            }
            _ => errors.push(format!("line {}: expected English<TAB>translation", i + 1)),
        }
    }
    (map, errors)
}

impl LangInfo {
    fn catalog(&self) -> &HashMap<&'static str, &'static str> {
        self.catalog.get_or_init(|| {
            let (map, errors) = parse(self.source);
            for e in errors {
                log::warn!("{} catalog: {e}", self.code);
            }
            // Leaked once per language for the life of the process, so lookups return 'static.
            map.into_iter().map(|(k, v)| (&*Box::leak(k.into_boxed_str()), &*Box::leak(v.into_boxed_str()))).collect()
        })
    }
}

/// A language the UI can be shown in.
#[derive(Clone, Copy)]
pub struct Lang(&'static LangInfo);

impl PartialEq for Lang {
    fn eq(&self, o: &Self) -> bool {
        self.0.code == o.0.code
    }
}

impl std::fmt::Debug for Lang {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Lang({})", self.0.code)
    }
}

impl Lang {
    pub const EN: Lang = Lang(&LANGUAGES[0]);

    pub fn code(self) -> &'static str {
        self.0.code
    }
    pub fn name(self) -> &'static str {
        self.0.name
    }
    pub fn english(self) -> &'static str {
        self.0.english
    }
    pub fn script(self) -> Script {
        self.0.script
    }
    pub fn from_code(code: &str) -> Option<Lang> {
        LANGUAGES.iter().find(|l| l.code.eq_ignore_ascii_case(code.trim())).map(Lang)
    }
    /// Resolves a preference: a code, or [`AUTO`] (or anything unknown) for the system language.
    pub fn from_pref(pref: &str) -> Lang {
        Lang::from_code(pref).unwrap_or_else(system_lang)
    }
    pub fn all() -> impl Iterator<Item = Lang> {
        LANGUAGES.iter().map(Lang)
    }
    /// Number of translated strings.
    pub fn len(self) -> usize {
        self.0.catalog().len()
    }
    pub fn is_empty(self) -> bool {
        self.len() == 0
    }
}

/// The registered language for a locale tag such as `de_DE.UTF-8`, `pt-BR` or `zh-Hans-CN`.
pub fn lang_from_tag(tag: &str) -> Option<Lang> {
    let base = tag.split(['.', '@']).next().unwrap_or("").replace('_', "-").to_ascii_lowercase();
    if matches!(base.as_str(), "c" | "posix") {
        return Some(Lang::EN);
    }
    let mut parts = base.split('-');
    let primary = parts.next().unwrap_or("");
    // Only Simplified Chinese has a catalog: Traditional-script locales fall back to English
    // rather than get Simplified text.
    if primary == "zh" && parts.any(|p| matches!(p, "hant" | "tw" | "hk" | "mo")) {
        return None;
    }
    Lang::from_code(primary)
}

/// The first of the system's preferred languages that has a catalog.
pub fn first_supported<'a>(tags: impl IntoIterator<Item = &'a str>) -> Option<Lang> {
    tags.into_iter().find_map(lang_from_tag)
}

/// The system language (looked up once); English when unknown or unsupported.
pub fn system_lang() -> Lang {
    if cfg!(test) {
        return Lang::EN;
    }
    static SYSTEM: OnceLock<usize> = OnceLock::new();
    let i = *SYSTEM.get_or_init(|| {
        let from_env = std::env::var("RUSTGIS_LANGUAGE").ok().and_then(|v| lang_from_tag(&v));
        let lang = from_env.or_else(|| {
            let tags: Vec<String> = sys_locale::get_locales().take(16).collect();
            first_supported(tags.iter().map(String::as_str))
        });
        lang.and_then(|l| LANGUAGES.iter().position(|x| x.code == l.code())).unwrap_or(0)
    });
    LANGUAGES.get(i).map(Lang).unwrap_or(Lang::EN)
}

thread_local! {
    static CURRENT: Cell<Lang> = const { Cell::new(Lang::EN) };
}

/// Sets the language the UI is drawn in.
pub fn set_current(lang: Lang) {
    CURRENT.set(lang);
}

pub fn current() -> Lang {
    CURRENT.get()
}

/// `s` in the current language.
pub fn t(s: &str) -> &str {
    tr(current(), s)
}

/// `s` in `lang`; strings without a translation come back unchanged. Text the catalog doesn't
/// have may be a plugin's (a tool name, a parameter label…): its own `i18n` translates it.
pub fn tr(lang: Lang, s: &str) -> &str {
    if let Some(t) = lang.0.catalog().get(s) {
        return t;
    }
    if lang == Lang::EN {
        return s;
    }
    plugin_tr(lang, s).unwrap_or(s)
}

/// Lookups kept before the plugin translation cache starts over (dynamic text, such as layer
/// names, also misses the catalog).
const PLUGIN_CACHE: usize = 20_000;

thread_local! {
    /// Plugin translations handed out so far, leaked once each so lookups return `&'static str`
    /// like the catalogs. Bounded: plugins' `i18n` tables are limited in size.
    static PLUGIN_STRINGS: std::cell::RefCell<std::collections::HashSet<&'static str>> = Default::default();
    /// Plugin lookups this frame and the ones before, misses included: (plugin revision, language,
    /// results). `tr` runs for every label every frame.
    static PLUGIN_CACHE_MAP: std::cell::RefCell<(u64, &'static str, std::collections::HashMap<String, Option<&'static str>>)> =
        Default::default();
}

/// `s` translated by a loaded plugin's `i18n`.
fn plugin_tr(lang: Lang, s: &str) -> Option<&'static str> {
    let revision = rustgis_engine::plugins::revision();
    PLUGIN_CACHE_MAP.with_borrow_mut(|(rev, code, map)| {
        if *rev != revision || *code != lang.code() || map.len() > PLUGIN_CACHE {
            *rev = revision;
            *code = lang.code();
            map.clear();
        }
        if let Some(hit) = map.get(s) {
            return *hit;
        }
        let found = rustgis_engine::plugins::translate(lang.code(), s).map(|t| {
            PLUGIN_STRINGS.with_borrow_mut(|seen| match seen.get(t.as_str()) {
                Some(&kept) => kept,
                None => {
                    let kept: &'static str = Box::leak(t.into_boxed_str());
                    seen.insert(kept);
                    kept
                }
            })
        });
        map.insert(s.to_string(), found);
        found
    })
}

/// A location such as `Map › Navigate`, translated segment by segment.
pub fn location(loc: &str) -> String {
    loc.split(" › ").map(t).collect::<Vec<_>>().join(" › ")
}

/// Translates `template`, then fills its `{name}` placeholders in one pass (values are never
/// re-read as templates, so a layer named `{n}` stays intact).
pub fn fmt(template: &str, args: &[(&str, &str)]) -> String {
    let template = t(template);
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some((before, after)) = rest.split_once('{') {
        out.push_str(before);
        let Some((name, tail)) = after.split_once('}') else {
            out.push('{');
            out.push_str(after);
            return out;
        };
        match args.iter().find(|(k, _)| *k == name) {
            Some((_, v)) => out.push_str(v),
            None => {
                out.push('{');
                out.push_str(name);
                out.push('}');
            }
        }
        rest = tail;
    }
    out.push_str(rest);
    out
}

/// `s` in the current language: `tl!("Explore")`.
#[macro_export]
macro_rules! tl {
    ($s:expr) => {
        $crate::i18n::t($s)
    };
}

#[cfg(test)]
mod tests;
