//! Localized UI strings loaded from the bundled language files.
//!
//! TontooOS libraries always ship `en_us.json` and `de_de.json`. The active
//! language is detected from the `LANG` / `LC_ALL` environment variables and
//! falls back to English.

use serde_json::Value;

const EN_US: &str = include_str!("../lang/en_us.json");
const DE_DE: &str = include_str!("../lang/de_de.json");

/// The currently active language code (`"en_us"` or `"de_de"`).
pub fn current() -> &'static str {
    let lang = std::env::var("LANG")
        .or_else(|_| std::env::var("LC_ALL"))
        .unwrap_or_default()
        .to_lowercase();
    if lang.starts_with("de") {
        "de_de"
    } else {
        "en_us"
    }
}

/// Load the language table for a language code.
pub fn load(lang: &str) -> Value {
    match lang {
        "de" | "de_de" | "de-DE" => serde_json::from_str(DE_DE).unwrap_or_default(),
        _ => serde_json::from_str(EN_US).unwrap_or_default(),
    }
}

/// The language table for the current locale.
pub fn table() -> Value {
    load(current())
}

/// Look up a localized string by key.
pub fn t(key: &str) -> Option<String> {
    table()
        .get(key)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

/// Look up a localized string by key with a fallback.
pub fn t_or(key: &str, fallback: &str) -> String {
    t(key).unwrap_or_else(|| fallback.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn english_is_the_fallback() {
        let table = load("xx");
        assert!(table.get("mapskit.search.placeholder").is_some());
    }

    #[test]
    fn german_table_exists() {
        let table = load("de_de");
        assert!(table.get("mapskit.search.placeholder").is_some());
    }

    #[test]
    fn lookup_with_fallback() {
        assert_eq!(t_or("does.not.exist", "Fallback"), "Fallback");
        assert!(!t_or("mapskit.map.globe", "?").is_empty());
    }
}
