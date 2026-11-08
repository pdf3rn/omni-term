//! Idiomas de la interfaz: catálogos Fluent embebidos y búsqueda de claves.
//!
//! El español es el catálogo base. `t` devuelve el texto de una clave sin argumentos y `tf` el de una clave con
//! argumentos (números para los plurales, cadenas para los nombres).

use std::collections::HashMap;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Mutex, OnceLock};

use fluent_bundle::concurrent::FluentBundle;
use fluent_bundle::{FluentArgs, FluentResource, FluentValue};
use unic_langid::LanguageIdentifier;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Lang {
    Es,
    En,
}

impl Lang {
    pub const ALL: [Lang; 2] = [Lang::Es, Lang::En];

    /// Código guardado en los ajustes.
    pub fn code(self) -> &'static str {
        match self {
            Lang::Es => "es",
            Lang::En => "en",
        }
    }

    /// Nombre del idioma en su propio idioma: no se traduce.
    pub fn native_name(self) -> &'static str {
        match self {
            Lang::Es => "Español",
            Lang::En => "English",
        }
    }

    pub fn from_code(code: &str) -> Option<Lang> {
        let primary = code.split(['-', '_']).next().unwrap_or("").to_ascii_lowercase();
        Lang::ALL.into_iter().find(|l| l.code() == primary)
    }

    fn source(self) -> &'static str {
        match self {
            Lang::Es => include_str!("../assets/i18n/es.ftl"),
            Lang::En => include_str!("../assets/i18n/en.ftl"),
        }
    }
}

/// Idioma del sistema si está entre los soportados; inglés si no.
pub fn detect() -> Lang {
    sys_locale::get_locale().and_then(|l| Lang::from_code(&l)).unwrap_or(Lang::En)
}

/// Idioma del sistema, consultado una sola vez.
pub fn system() -> Lang {
    static SYSTEM: OnceLock<Lang> = OnceLock::new();
    *SYSTEM.get_or_init(detect)
}

static CURRENT: AtomicU8 = AtomicU8::new(0);

pub fn current() -> Lang {
    Lang::ALL[CURRENT.load(Ordering::Relaxed) as usize % Lang::ALL.len()]
}

pub fn set(lang: Lang) {
    CURRENT.store(Lang::ALL.iter().position(|l| *l == lang).unwrap_or(0) as u8, Ordering::Relaxed);
}

type Bundle = FluentBundle<FluentResource>;

fn bundle(lang: Lang) -> &'static Bundle {
    static BUNDLES: OnceLock<Vec<Bundle>> = OnceLock::new();
    let all = BUNDLES.get_or_init(|| {
        Lang::ALL
            .into_iter()
            .map(|l| {
                let id: LanguageIdentifier = l.code().parse().expect("código de idioma válido");
                let mut bundle = FluentBundle::new_concurrent(vec![id]);
                // Sin marcas de aislamiento: el texto sale limpio en egui.
                bundle.set_use_isolating(false);
                let resource = FluentResource::try_new(l.source().to_string()).unwrap_or_else(|(r, _)| r);
                let _ = bundle.add_resource(resource);
                bundle
            })
            .collect()
    });
    &all[Lang::ALL.iter().position(|l| *l == lang).unwrap_or(0)]
}

fn render(lang: Lang, key: &str, args: Option<&FluentArgs>) -> Option<String> {
    let bundle = bundle(lang);
    let pattern = bundle.get_message(key)?.value()?;
    let mut errors = Vec::new();
    Some(bundle.format_pattern(pattern, args, &mut errors).into_owned())
}

/// Texto de la clave en el idioma actual; si falta, el del español, y si tampoco está, la propia clave.
fn lookup(key: &str, args: Option<&FluentArgs>) -> String {
    render(current(), key, args).or_else(|| render(Lang::Es, key, args)).unwrap_or_else(|| key.to_string())
}

/// Texto sin argumentos. Se guarda una copia por clave e idioma para devolver `&'static str`.
pub fn t(key: &'static str) -> &'static str {
    static CACHE: OnceLock<Mutex<HashMap<(Lang, &'static str), &'static str>>> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    let lang = current();
    let mut cache = cache.lock().unwrap();
    cache.entry((lang, key)).or_insert_with(|| Box::leak(lookup(key, None).into_boxed_str()))
}

/// Texto con argumentos; los números eligen la variante plural.
pub fn tf(key: &str, args: &[(&str, FluentValue<'_>)]) -> String {
    let mut fluent = FluentArgs::new();
    for (name, value) in args {
        fluent.set(name.to_string(), value.clone());
    }
    lookup(key, Some(&fluent))
}

/// Atajo para `tf` con valores que ya implementan `Into<FluentValue>`.
#[macro_export]
macro_rules! tf {
    ($key:expr $(, $name:literal => $value:expr)* $(,)?) => {
        $crate::i18n::tf($key, &[$(($name, ::fluent_bundle::FluentValue::from($value))),*])
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(lang: Lang) -> Vec<String> {
        let resource = FluentResource::try_new(lang.source().to_string()).unwrap_or_else(|(_, errors)| panic!("{}: {errors:?}", lang.code()));
        let mut keys: Vec<String> = resource
            .entries()
            .filter_map(|e| match e {
                fluent_syntax::ast::Entry::Message(m) => Some(m.id.name.to_string()),
                _ => None,
            })
            .collect();
        keys.sort();
        keys
    }

    #[test]
    fn every_language_has_the_same_keys() {
        let base = keys(Lang::Es);
        for lang in Lang::ALL {
            assert_eq!(keys(lang), base, "claves distintas en {}", lang.code());
        }
    }

    fn sources() -> Vec<String> {
        fn walk(dir: &std::path::Path, out: &mut Vec<String>) {
            for entry in std::fs::read_dir(dir).unwrap().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    walk(&path, out);
                } else if path.extension().is_some_and(|e| e == "rs") {
                    out.push(std::fs::read_to_string(path).unwrap());
                }
            }
        }
        let mut out = Vec::new();
        walk(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"), &mut out);
        out
    }

    #[test]
    fn every_key_used_in_the_code_exists() {
        let catalog = keys(Lang::Es);
        let call = regex_lite_calls();
        for source in sources() {
            for key in call(&source) {
                assert!(catalog.contains(&key), "falta la clave «{key}» en los catálogos");
            }
        }
    }

    /// Claves que aparecen en `t("…")` o `tf!("…"`.
    fn regex_lite_calls() -> impl Fn(&str) -> Vec<String> {
        |source: &str| {
            let mut found = Vec::new();
            for marker in ["t(\"", "tf!(\""] {
                let mut rest = source;
                while let Some(at) = rest.find(marker) {
                    // `t(` tiene que ser una llamada, no el final de otro identificador (`format(`, `get(`…).
                    let before = rest[..at].chars().last();
                    let after = &rest[at + marker.len()..];
                    if marker == "t(\"" && before.is_some_and(|c| c.is_alphanumeric() || c == '_') {
                        rest = after;
                        continue;
                    }
                    if let Some(end) = after.find('"') {
                        let key = &after[..end];
                        if key.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-') {
                            found.push(key.to_string());
                        }
                    }
                    rest = after;
                }
            }
            found
        }
    }

    /// Variables (`$name`) que usa cada mensaje del catálogo base.
    fn variables(source: &str) -> std::collections::BTreeMap<String, std::collections::BTreeSet<String>> {
        let mut out: std::collections::BTreeMap<String, std::collections::BTreeSet<String>> = Default::default();
        let mut key = String::new();
        for line in source.lines() {
            if let Some((name, rest)) = line.split_once(" =").filter(|(n, _)| !n.is_empty() && n.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')) {
                key = name.to_string();
                out.entry(key.clone()).or_default();
                collect(rest, out.get_mut(&key).unwrap());
            } else if line.starts_with(' ') || line.starts_with('}') {
                collect(line, out.entry(key.clone()).or_default());
            }
        }
        out
    }

    /// Solo cuenta `$nombre` dentro de llaves: fuera de ellas es texto (por ejemplo `$USER`).
    fn collect(text: &str, into: &mut std::collections::BTreeSet<String>) {
        let mut rest = text;
        while let Some(at) = rest.find('{') {
            let inner = rest[at + 1..].trim_start();
            if let Some(var) = inner.strip_prefix('$') {
                let name: String = var.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '_').collect();
                into.insert(name);
            }
            rest = &rest[at + 1..];
        }
    }

    #[test]
    fn both_languages_use_the_same_variables() {
        let (es, en) = (variables(Lang::Es.source()), variables(Lang::En.source()));
        for (key, vars) in &es {
            assert_eq!(Some(vars), en.get(key), "variables distintas en «{key}»");
        }
    }

    #[test]
    fn calls_pass_the_variables_each_message_needs() {
        let catalog = variables(Lang::Es.source());
        for source in sources() {
            let mut rest = source.as_str();
            while let Some(at) = rest.find("tf!(\"") {
                let after = &rest[at + 5..];
                let Some(end) = after.find('"') else { break };
                let key = &after[..end];
                // Los argumentos llegan hasta el paréntesis que cierra la llamada.
                let mut depth = 1;
                let mut call = String::new();
                for c in after[end + 1..].chars() {
                    match c {
                        '(' => depth += 1,
                        ')' => depth -= 1,
                        _ => {}
                    }
                    if depth == 0 {
                        break;
                    }
                    call.push(c);
                }
                let passed: std::collections::BTreeSet<String> = call.split("\"").skip(1).step_by(2).zip(call.split("\"").skip(2).step_by(2)).filter(|(_, after)| after.trim_start().starts_with("=>")).map(|(name, _)| name.to_string()).collect();
                if let Some(needed) = catalog.get(key) {
                    assert_eq!(&passed, needed, "«{key}» pide {needed:?} y la llamada pasa {passed:?}");
                }
                rest = after;
            }
        }
    }

    #[test]
    fn no_key_is_left_unused() {
        let all = sources().join("\n");
        for key in keys(Lang::Es) {
            assert!(all.contains(&format!("\"{key}\"")), "la clave «{key}» no se usa en el código");
        }
    }

    fn says(lang: Lang, key: &str, n: i64) -> String {
        let mut args = FluentArgs::new();
        args.set("n", n);
        render(lang, key, Some(&args)).unwrap()
    }

    #[test]
    fn plurals_pick_the_right_variant() {
        assert_eq!(says(Lang::Es, "sidebar-tunnels", 1), "1 túnel");
        assert_eq!(says(Lang::Es, "sidebar-tunnels", 3), "3 túneles");
        assert_eq!(says(Lang::En, "sidebar-tunnels", 1), "1 tunnel");
        assert_eq!(says(Lang::En, "sidebar-tunnels", 3), "3 tunnels");
    }

    #[test]
    fn the_system_locale_maps_to_a_supported_language() {
        assert_eq!(Lang::from_code("es-ES"), Some(Lang::Es));
        assert_eq!(Lang::from_code("en_US.UTF-8"), Some(Lang::En));
        assert_eq!(Lang::from_code("fr-FR"), None);
    }
}
