//! Interface languages. Existing bilingual calls keep their English/Russian fallback;
//! additional languages use embedded catalogues, with no network access at runtime.

use std::collections::HashMap;
use std::sync::{OnceLock, atomic::{AtomicU8, Ordering}};

static LANG: AtomicU8 = AtomicU8::new(0);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Lang { En = 0, Ru = 1, PtBr = 2, PtPt = 3, De = 4 }

pub const LANGUAGES: [(Lang, &str); 5] = [
    (Lang::En, "English"), (Lang::PtBr, "Português (Brasil)"),
    (Lang::PtPt, "Português (Portugal)"), (Lang::De, "Deutsch"), (Lang::Ru, "Русский"),
];

pub fn set(lang: Lang) { LANG.store(lang as u8, Ordering::Relaxed); }
pub fn get() -> Lang {
    match LANG.load(Ordering::Relaxed) { 1 => Lang::Ru, 2 => Lang::PtBr, 3 => Lang::PtPt, 4 => Lang::De, _ => Lang::En }
}
pub fn code(l: Lang) -> &'static str {
    match l { Lang::En => "en", Lang::Ru => "ru", Lang::PtBr => "pt-BR", Lang::PtPt => "pt-PT", Lang::De => "de" }
}

fn parse(c: &str) -> Option<Lang> {
    let normalized = c.trim().replace('_', "-").to_ascii_lowercase();
    let locale = normalized.split(['.', '@']).next().unwrap_or("");
    let mut parts = locale.split('-');
    match parts.next()? {
        "en" => Some(Lang::En), "ru" | "uk" | "be" => Some(Lang::Ru),
        "de" => Some(Lang::De),
        "pt" => Some(if parts.next() == Some("pt") { Lang::PtPt } else { Lang::PtBr }),
        _ => None,
    }
}

#[cfg(windows)]
fn native_locale() -> String {
    #[link(name = "kernel32")]
    unsafe extern "system" { fn GetUserDefaultLocaleName(name: *mut u16, count: i32) -> i32; }
    let mut name = [0u16; 85];
    // SAFETY: Windows writes at most count UTF-16 units into this valid buffer.
    let n = unsafe { GetUserDefaultLocaleName(name.as_mut_ptr(), name.len() as i32) };
    if n > 1 { String::from_utf16_lossy(&name[..n as usize - 1]) } else { String::new() }
}
#[cfg(not(windows))]
fn native_locale() -> String {
    if cfg!(target_os = "macos") {
        let s = std::process::Command::new("defaults").args(["read", "-g", "AppleLanguages"]).output().ok().map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default();
        s.split(|c: char| !(c.is_ascii_alphabetic() || c == '-' || c == '_')).find(|s| parse(s).is_some()).unwrap_or("").into()
    } else { String::new() }
}

/// Explicit settings take precedence; otherwise use the OS language.
pub fn from_code(c: &str) -> Lang {
    if let Some(l) = parse(c) { return l; }
    let sys = ["LC_ALL", "LC_MESSAGES", "LANG"].iter().filter_map(|v| std::env::var(v).ok()).find(|v| !v.is_empty()).unwrap_or_else(native_locale);
    parse(&sys).unwrap_or(Lang::En)
}

type Catalogue = HashMap<String, String>;
static BR: OnceLock<Catalogue> = OnceLock::new();
static PT: OnceLock<Catalogue> = OnceLock::new();
static DE: OnceLock<Catalogue> = OnceLock::new();

fn translated(lang: Lang, en: &'static str, ru: &'static str) -> &'static str {
    let (slot, json) = match lang {
        Lang::En => return en, Lang::Ru => return ru,
        Lang::PtBr => (&BR, include_str!("../locales/pt-BR.json")),
        Lang::PtPt => (&PT, include_str!("../locales/pt-PT.json")),
        Lang::De => (&DE, include_str!("../locales/de.json")),
    };
    slot.get_or_init(|| serde_json::from_str(json).unwrap_or_default()).get(en).filter(|s| !s.is_empty()).map(String::as_str).unwrap_or(en)
}

pub fn tr(en: &'static str, ru: &'static str) -> &'static str { translated(get(), en, ru) }

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn settings_and_system_locale_names_keep_the_region() {
        for (name, want) in [("pt_BR.UTF-8", Lang::PtBr), ("pt-PT", Lang::PtPt), ("de_DE@euro", Lang::De), ("ru-RU", Lang::Ru), ("en-US", Lang::En)] { assert_eq!(parse(name), Some(want)); }
        for (lang, _) in LANGUAGES { assert_eq!(parse(code(lang)), Some(lang)); }
        assert_eq!(parse("unknown"), None);
    }
    #[test]
    fn catalogues_cover_the_same_keys_and_preserve_literals() {
        let english: Catalogue = serde_json::from_str(include_str!("../locales/en.json")).unwrap();
        for json in [include_str!("../locales/pt-BR.json"), include_str!("../locales/pt-PT.json"), include_str!("../locales/de.json")] {
            let cat: Catalogue = serde_json::from_str(json).unwrap();
            assert_eq!(cat.len(), english.len());
            for key in english.keys() { assert!(cat.get(key).map(|v| !v.trim().is_empty()).unwrap_or(false), "missing {key}"); }
        }
        assert_eq!(translated(Lang::PtBr, "Save", "Сохранить"), "Salvar");
        assert_eq!(translated(Lang::PtPt, "Save", "Сохранить"), "Guardar");
        assert_eq!(translated(Lang::De, "Save", "Сохранить"), "Speichern");
        assert_eq!(translated(Lang::Ru, "Save", "Сохранить"), "Сохранить");
        assert_eq!(translated(Lang::PtBr, "unrecognised future text", "x"), "unrecognised future text");
    }
}
