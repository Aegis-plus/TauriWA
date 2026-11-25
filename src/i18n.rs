use std::collections::HashMap;

const LOCALES: &[(&str, &str)] = &[
    ("de", include_str!("../locales/de.json")),
    ("en", include_str!("../locales/en.json")),
    ("fr", include_str!("../locales/fr.json")),
    ("it", include_str!("../locales/it.json")),
    ("pt", include_str!("../locales/pt.json")),
];

#[derive(Clone, Default)]
pub struct Translations(HashMap<String, String>);

impl Translations {
    /// `locale` like "fr-FR", "fr_FR.UTF-8" or "fr"
    pub fn load(locale: &str) -> Self {
        let lang = locale
            .split(['-', '_', '.'])
            .next()
            .unwrap_or("")
            .to_lowercase();
        match LOCALES.iter().find(|(name, _)| *name == lang) {
            Some((_, data)) => match serde_json::from_str(data) {
                Ok(map) => Translations(map),
                Err(err) => {
                    log::warn!("cannot load translations {locale}: {err}");
                    Self::default()
                }
            },
            None => {
                log::warn!("cannot load translations {locale}");
                Self::default()
            }
        }
    }

    pub fn get(&self, key: &str, default: &str) -> String {
        self.0
            .get(key)
            .cloned()
            .unwrap_or_else(|| default.to_owned())
    }
}

/// Preferred languages of the user, as BCP 47 tags ("en-US")
pub fn preferred_languages() -> Vec<String> {
    let mut langs: Vec<String> = sys_locale::get_locales()
        .map(|l| l.split('.').next().unwrap_or("").replace('_', "-"))
        .filter(|l| !l.is_empty() && l != "C" && l != "POSIX")
        .collect();
    langs.dedup();
    if langs.is_empty() {
        langs.push("en-US".to_owned());
    }
    langs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_locales_parse() {
        for (name, data) in LOCALES {
            let map: HashMap<String, String> =
                serde_json::from_str(data).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert!(map.contains_key("quit"), "{name}");
        }
        assert_eq!(
            Translations::load("fr_FR.UTF-8").get("quit", "?"),
            "Quitter"
        );
        assert_eq!(Translations::load("xx").get("quit", "Quit"), "Quit");
    }
}
