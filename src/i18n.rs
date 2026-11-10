use serde::{Deserialize, Serialize};
use std::env;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LanguagePreference {
    #[serde(rename = "auto", alias = "system")]
    Auto,
    #[serde(rename = "pt-br", alias = "pt-BR", alias = "pt")]
    PtBr,
    #[serde(rename = "en", alias = "en-US", alias = "en-us")]
    En,
}

impl Default for LanguagePreference {
    fn default() -> Self {
        Self::Auto
    }
}

impl LanguagePreference {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "auto" | "system" => Some(Self::Auto),
            "pt" | "pt-br" | "pt_br" | "pt-pt" | "pt_pt" => Some(Self::PtBr),
            "en" | "en-us" | "en_us" | "en-gb" | "en_gb" => Some(Self::En),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Language {
    PtBr,
    En,
}

impl Language {
    pub fn code(self) -> &'static str {
        match self {
            Self::PtBr => "pt-BR",
            Self::En => "en",
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            Self::PtBr => "Português (Brasil)",
            Self::En => "English",
        }
    }
}

#[derive(Debug, Clone)]
pub struct I18n {
    pub preference: LanguagePreference,
    pub language: Language,
}

impl I18n {
    pub fn new(preference: LanguagePreference) -> Self {
        let language = resolve_language(preference);
        Self {
            preference,
            language,
        }
    }

    pub fn set_preference(&mut self, preference: LanguagePreference) {
        self.preference = preference;
        self.language = resolve_language(preference);
    }

    pub fn t(&self, key: &str) -> String {
        rust_i18n::t!(key, locale = self.language.code()).to_string()
    }

    pub fn tf<I, K, V>(&self, key: &str, args: I) -> String
    where
        I: IntoIterator<Item = (K, V)>,
        K: AsRef<str>,
        V: std::fmt::Display,
    {
        let mut value = self.t(key);
        for (name, replacement) in args {
            let replacement = replacement.to_string();
            value = value.replace(&format!("{{{}}}", name.as_ref()), &replacement);
        }
        value
    }

    pub fn language_label(&self, preference: LanguagePreference) -> String {
        let key = match preference {
            LanguagePreference::Auto => "language.auto",
            LanguagePreference::PtBr => "language.pt_br",
            LanguagePreference::En => "language.en",
        };
        self.t(key)
    }
}

fn config_home() -> PathBuf {
    env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|p| p.join(".config")))
        .unwrap_or_else(|| PathBuf::from(".config"))
}

fn omarchy_language_file() -> PathBuf {
    env::var_os("OMARCHY_LANGUAGE_FILE")
        .map(PathBuf::from)
        .unwrap_or_else(|| config_home().join("omarchy/language"))
}

fn language_from_code(code: &str) -> Option<Language> {
    let normalized = code.trim().to_ascii_lowercase().replace('_', "-");
    if normalized == "pt" || normalized.starts_with("pt-") {
        Some(Language::PtBr)
    } else if normalized == "en" || normalized.starts_with("en-") {
        Some(Language::En)
    } else {
        None
    }
}

pub fn resolve_language(preference: LanguagePreference) -> Language {
    match preference {
        LanguagePreference::PtBr => Language::PtBr,
        LanguagePreference::En => Language::En,
        LanguagePreference::Auto => {
            if let Ok(code) = std::fs::read_to_string(omarchy_language_file()) {
                if let Some(language) = language_from_code(&code) {
                    return language;
                }
            }
            for variable in ["LC_ALL", "LANGUAGE", "LC_MESSAGES", "LANG"] {
                if let Ok(value) = env::var(variable) {
                    for candidate in value.split(':') {
                        if let Some(language) = language_from_code(candidate) {
                            return language;
                        }
                    }
                }
            }
            Language::En
        }
    }
}

pub fn language_file_exists() -> bool {
    omarchy_language_file().is_file()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_supported_locales() {
        assert_eq!(language_from_code("pt_BR.UTF-8"), Some(Language::PtBr));
        assert_eq!(language_from_code("en_US.UTF-8"), Some(Language::En));
        assert_eq!(language_from_code("de_DE.UTF-8"), None);
    }

    #[test]
    fn formats_named_arguments() {
        let i18n = I18n::new(LanguagePreference::En);
        assert_eq!(
            i18n.tf("search.placeholder", [("kind", "tracks")]),
            "Type to search tracks, albums or artists..."
        );
    }

    #[test]
    fn loads_supported_locale_catalogs() {
        assert_eq!(
            I18n::new(LanguagePreference::PtBr).t("player.pause_button"),
            "[⏸ Pausar]"
        );
        assert_eq!(
            I18n::new(LanguagePreference::En).t("player.pause_button"),
            "[⏸ Pause]"
        );
    }
}
