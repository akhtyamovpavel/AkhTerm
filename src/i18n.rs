//! Bilingual localization runtime (`RU` / `EN`) with zero-allocation CStr lookup.

use std::ffi::{c_char, CStr};
use std::sync::atomic::{AtomicU8, Ordering};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum UiLocale {
    #[default]
    Auto,
    Ru,
    En,
}

impl UiLocale {
    pub fn from_config(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "auto" => Some(Self::Auto),
            "ru" => Some(Self::Ru),
            "en" => Some(Self::En),
            _ => None,
        }
    }

    pub const fn as_config_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Ru => "ru",
            Self::En => "en",
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct UiText {
    pub ru: &'static CStr,
    pub en: &'static CStr,
}

impl UiText {
    pub const fn new(ru: &'static CStr, en: &'static CStr) -> Self {
        Self { ru, en }
    }

    pub const fn for_locale(self, locale: UiLocale) -> &'static CStr {
        match locale {
            UiLocale::Ru => self.ru,
            UiLocale::En | UiLocale::Auto => self.en,
        }
    }

    pub fn cstr(self) -> &'static CStr {
        self.for_locale(current_locale())
    }

    pub fn as_str(self) -> &'static str {
        self.cstr().to_str().unwrap_or("")
    }

    #[allow(dead_code)]
    pub fn as_ptr(self) -> *const c_char {
        self.cstr().as_ptr()
    }
}

const LOCALE_AUTO: u8 = 0;
const LOCALE_RU: u8 = 1;
const LOCALE_EN: u8 = 2;

static CONFIGURED_LOCALE: AtomicU8 = AtomicU8::new(LOCALE_AUTO);
static RESOLVED_LOCALE: AtomicU8 = AtomicU8::new(LOCALE_AUTO);

fn encode_locale(locale: UiLocale) -> u8 {
    match locale {
        UiLocale::Auto => LOCALE_AUTO,
        UiLocale::Ru => LOCALE_RU,
        UiLocale::En => LOCALE_EN,
    }
}

#[cfg(test)]
fn decode_locale(raw: u8) -> UiLocale {
    match raw {
        LOCALE_RU => UiLocale::Ru,
        LOCALE_EN => UiLocale::En,
        _ => UiLocale::Auto,
    }
}

fn env_var_opt(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|val| !val.trim().is_empty())
}

pub fn detect_locale_from_vars(
    lc_all: Option<&str>,
    lc_messages: Option<&str>,
    lang: Option<&str>,
) -> UiLocale {
    let chosen = [lc_all, lc_messages, lang]
        .into_iter()
        .flatten()
        .map(str::trim)
        .find(|val| !val.is_empty());
    match chosen {
        Some(val) if val.to_ascii_lowercase().starts_with("ru") => UiLocale::Ru,
        _ => UiLocale::En,
    }
}

pub fn detect_system_locale_from_env() -> UiLocale {
    let lc_all = env_var_opt("LC_ALL");
    let lc_messages = env_var_opt("LC_MESSAGES");
    let lang = env_var_opt("LANG");
    detect_locale_from_vars(lc_all.as_deref(), lc_messages.as_deref(), lang.as_deref())
}

pub fn set_locale(locale: UiLocale) {
    CONFIGURED_LOCALE.store(encode_locale(locale), Ordering::Release);
    let resolved = match locale {
        UiLocale::Auto => detect_system_locale_from_env(),
        other => other,
    };
    RESOLVED_LOCALE.store(encode_locale(resolved), Ordering::Release);
}

#[cfg(test)]
pub fn configured_locale() -> UiLocale {
    decode_locale(CONFIGURED_LOCALE.load(Ordering::Acquire))
}

pub fn current_locale() -> UiLocale {
    match RESOLVED_LOCALE.load(Ordering::Acquire) {
        LOCALE_RU => UiLocale::Ru,
        LOCALE_EN => UiLocale::En,
        _ => detect_system_locale_from_env(),
    }
}

#[cfg(test)]
pub(crate) fn with_locale_lock<R>(f: impl FnOnce() -> R) -> R {
    use std::sync::Mutex;
    static LOCK: Mutex<()> = Mutex::new(());
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let prev = configured_locale();
    let out = f();
    set_locale(prev);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_ru_and_en_from_posix_env_precedence() {
        assert_eq!(detect_locale_from_vars(Some("ru_RU.UTF-8"), None, Some("en_US")), UiLocale::Ru);
        assert_eq!(detect_locale_from_vars(Some("C.UTF-8"), Some("ru_RU.UTF-8"), None), UiLocale::En);
        assert_eq!(detect_locale_from_vars(None, Some("ru"), Some("en_US.UTF-8")), UiLocale::Ru);
        assert_eq!(detect_locale_from_vars(None, None, Some("en_GB.UTF-8")), UiLocale::En);
        assert_eq!(detect_locale_from_vars(None, None, None), UiLocale::En);
    }

    #[test]
    fn parses_config_locale_tokens_case_insensitively() {
        assert_eq!(UiLocale::from_config("auto"), Some(UiLocale::Auto));
        assert_eq!(UiLocale::from_config(" RU "), Some(UiLocale::Ru));
        assert_eq!(UiLocale::from_config("En"), Some(UiLocale::En));
        assert_eq!(UiLocale::from_config("de"), None);
        assert_eq!(UiLocale::Ru.as_config_str(), "ru");
    }

    #[test]
    fn switches_active_locale_and_resolves_ui_text_without_allocation() {
        with_locale_lock(|| {
            let label = UiText::new(c"Настройки", c"Settings");
            set_locale(UiLocale::Ru);
            assert_eq!(configured_locale(), UiLocale::Ru);
            assert_eq!(current_locale(), UiLocale::Ru);
            assert_eq!(label.as_str(), "Настройки");
            set_locale(UiLocale::En);
            assert_eq!(current_locale(), UiLocale::En);
            assert_eq!(label.cstr(), c"Settings");
            set_locale(UiLocale::Auto);
            assert_eq!(configured_locale(), UiLocale::Auto);
        });
    }
}
