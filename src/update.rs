//! Versiones nuevas: consulta la última release estable publicada en GitHub.

use std::time::Duration;

use crate::i18n::t;

mod install;
pub use install::*;

const LATEST_URL: &str = "https://api.github.com/repos/pdf3rn/omni-term/releases/latest";
/// Tiempo mínimo entre dos comprobaciones automáticas.
pub const INTERVAL_SECS: i64 = 24 * 60 * 60;

#[derive(Clone, Debug, PartialEq)]
pub struct Release {
    /// Sin la `v` de la etiqueta.
    pub version: String,
    pub notes: String,
    /// Página de la release, con las descargas.
    pub page: String,
    pub assets: Vec<Asset>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Asset {
    pub name: String,
    pub url: String,
    pub size: u64,
}

impl Release {
    pub fn asset(&self, name: &str) -> Option<&Asset> {
        self.assets.iter().find(|a| a.name == name)
    }
}

pub fn current() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// Cliente HTTP de las consultas y descargas; `timeout` cubre la petición entera.
fn agent(timeout: Duration) -> ureq::Agent {
    ureq::Agent::config_builder().timeout_global(Some(timeout)).build().into()
}

fn http_error(error: ureq::Error) -> String {
    match error {
        ureq::Error::StatusCode(404) => t("update-err-none").to_string(),
        ureq::Error::StatusCode(403 | 429) => t("update-err-limit").to_string(),
        ureq::Error::StatusCode(code) => crate::tf!("update-err-status", "code" => code),
        other => crate::tf!("update-err-network", "error" => other.to_string()),
    }
}

fn get(agent: &ureq::Agent, url: &str, accept: &str) -> Result<ureq::http::Response<ureq::Body>, String> {
    agent.get(url).header("Accept", accept).header("User-Agent", concat!("omni-term/", env!("CARGO_PKG_VERSION"))).call().map_err(http_error)
}

/// Bloquea hasta tener respuesta: se llama desde un hilo aparte.
pub fn fetch_latest() -> Result<Release, String> {
    let mut response = get(&agent(Duration::from_secs(20)), LATEST_URL, "application/vnd.github+json")?;
    let body = response.body_mut().read_to_string().map_err(|error| crate::tf!("update-err-network", "error" => error.to_string()))?;
    parse_latest(&body).ok_or_else(|| t("update-err-format").to_string())
}

/// Release estable de la respuesta de GitHub; los borradores, las prerelease y las etiquetas que no son versiones no cuentan.
pub fn parse_latest(json: &str) -> Option<Release> {
    let value: serde_json::Value = serde_json::from_str(json).ok()?;
    if value["draft"].as_bool().unwrap_or(false) || value["prerelease"].as_bool().unwrap_or(false) {
        return None;
    }
    let version = value["tag_name"].as_str()?.trim_start_matches('v').to_string();
    parts(&version)?;
    Some(Release {
        version,
        notes: value["body"].as_str().unwrap_or("").to_string(),
        page: value["html_url"].as_str()?.to_string(),
        assets: value["assets"]
            .as_array()
            .map(|list| {
                list.iter()
                    .filter_map(|a| {
                        Some(Asset { name: a["name"].as_str()?.to_string(), url: a["browser_download_url"].as_str()?.to_string(), size: a["size"].as_u64().unwrap_or(0) })
                    })
                    .collect()
            })
            .unwrap_or_default(),
    })
}

/// `1.2.3` → `[1, 2, 3]`; un sufijo como `-rc1` hace que no sea una versión estable.
fn parts(version: &str) -> Option<Vec<u64>> {
    version.split('.').map(|p| p.parse::<u64>().ok()).collect::<Option<Vec<_>>>().filter(|p| !p.is_empty())
}

pub fn is_newer(candidate: &str, installed: &str) -> bool {
    match (parts(candidate), parts(installed)) {
        (Some(a), Some(b)) => a > b,
        _ => false,
    }
}

/// Toca comprobar si nunca se hizo, si pasó el intervalo o si el reloj fue hacia atrás.
pub fn due(last_check: Option<i64>, now: i64) -> bool {
    last_check.is_none_or(|last| now < last || now - last >= INTERVAL_SECS)
}

/// Notas en Markdown de GitHub como texto plano: sin almohadillas ni negritas, con viñetas.
pub fn plain_notes(markdown: &str) -> String {
    let mut lines: Vec<String> = Vec::new();
    for line in markdown.lines() {
        let line = line.trim_end().replace("**", "");
        let trimmed = line.trim_start();
        let text = if let Some(rest) = trimmed.strip_prefix("* ").or_else(|| trimmed.strip_prefix("- ")) {
            format!("• {rest}")
        } else {
            trimmed.trim_start_matches('#').trim_start().to_string()
        };
        // Sin líneas en blanco seguidas.
        if !(text.is_empty() && lines.last().is_some_and(String::is_empty)) {
            lines.push(text);
        }
    }
    lines.join("\n").trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_compare_by_number() {
        assert!(is_newer("0.1.15", "0.1.14"));
        assert!(is_newer("0.2.0", "0.1.99"));
        assert!(is_newer("0.10.0", "0.9.0"));
        assert!(!is_newer("0.1.14", "0.1.14"));
        assert!(!is_newer("0.1.13", "0.1.14"));
        assert!(!is_newer("0.2.0-rc1", "0.1.14"));
    }

    #[test]
    fn only_stable_releases_count() {
        let stable = r###"{"tag_name":"v0.2.0","draft":false,"prerelease":false,"body":"## Hola","html_url":"https://github.com/x/y/releases/tag/v0.2.0"}"###;
        let release = parse_latest(stable).unwrap();
        assert_eq!(release.version, "0.2.0");
        assert_eq!(release.page, "https://github.com/x/y/releases/tag/v0.2.0");
        assert!(release.assets.is_empty());
        assert!(parse_latest(&stable.replace(r#""prerelease":false"#, r#""prerelease":true"#)).is_none());
        assert!(parse_latest(&stable.replace(r#""draft":false"#, r#""draft":true"#)).is_none());
        assert!(parse_latest(&stable.replace("v0.2.0\"", "nightly\"")).is_none());
        assert!(parse_latest("no es json").is_none());
    }

    #[test]
    fn checks_at_most_once_a_day() {
        assert!(due(None, 1000));
        assert!(!due(Some(1000), 1000 + INTERVAL_SECS - 1));
        assert!(due(Some(1000), 1000 + INTERVAL_SECS));
        assert!(due(Some(5000), 1000));
    }

    #[test]
    fn notes_lose_the_markdown() {
        let notes = "## What's Changed\n* **feat**: menú by @a in #3\n\n\n- fix: algo\n\n**Full Changelog**: v1...v2\n";
        assert_eq!(plain_notes(notes), "What's Changed\n• feat: menú by @a in #3\n\n• fix: algo\n\nFull Changelog: v1...v2");
    }
}
