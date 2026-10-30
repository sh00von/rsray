//! Classification, interesting flags, and false-positive filtering.

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Class {
    Api,
    Page,
    Static,
    External,
    Websocket,
}

#[derive(Debug, Clone, Serialize)]
pub struct Endpoint {
    pub endpoint: String,
    pub source_file: String,
    pub class: Class,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub flags: Vec<String>,
    pub method: String,
}

const STATIC_EXTS: [&str; 18] = [
    ".png", ".jpg", ".jpeg", ".gif", ".svg", ".webp", ".ico", ".css", ".woff", ".woff2", ".ttf",
    ".eot", ".mp3", ".mp4", ".webm", ".pdf", ".map", ".txt",
];

const INTERESTING: [(&str, &str); 10] = [
    ("admin", "admin"),
    ("internal", "internal"),
    ("debug", "debug"),
    ("test", "test"),
    ("staging", "staging"),
    ("dev", "dev"),
    ("private", "private"),
    ("beta", "beta"),
    ("backup", "backup"),
    ("old", "old-version"),
];

fn host_of(url: &str) -> Option<String> {
    let after = url.split("://").nth(1)?;
    let host = after.split('/').next()?;
    Some(host.to_string())
}

fn is_static_path(path: &str) -> bool {
    let lower = path.to_lowercase();
    let path_only = lower.split('?').next().unwrap_or(&lower);
    STATIC_EXTS.iter().any(|ext| path_only.ends_with(ext))
}

/// Guess an HTTP method from nearby code, best effort. Defaults to GET.
/// Covers JS/TS (fetch method, axios.*), Python (requests.*),
/// Go (http.Get/Post), PHP (CURLOPT_POST) and generic .post() calls.
pub fn guess_method(snippet: &str) -> String {
    let lower = snippet.to_lowercase();
    for m in ["post", "put", "delete", "patch"] {
        if lower.contains(&format!("method: \"{}\"", m))
            || lower.contains(&format!("method: '{}'", m))
            || lower.contains(&format!("method:{}", m))
            || lower.contains(&format!("requests.{}", m))
            || lower.contains(&format!("axios.{}", m))
            || lower.contains(&format!("http.{}", m))
            || lower.contains(&format!(".{}", m))
            || (m == "post" && lower.contains("curlopt_post"))
        {
            return m.to_uppercase();
        }
    }
    if lower.contains("requests.get") || lower.contains("http.get") || lower.contains("axios.get") {
        return "GET".to_string();
    }
    "GET".to_string()
}

/// API-looking paths across frameworks: /api, /rest, /graphql,
/// /v1..v9, and bare api/ relative paths.
pub fn is_api_path(path: &str) -> bool {
    path.starts_with("/api/")
        || path == "/api"
        || path.starts_with("/rest/")
        || path == "/rest"
        || path.starts_with("/graphql")
        || path.starts_with("api/")
        || regex_api_version(path)
}

pub fn classify(value: &str, source_file: &str, snippet: &str, scope: Option<&str>) -> Option<Endpoint> {
    // Scope filter: for full URLs, drop hosts outside the scope.
    if let Some(scope) = scope {
        if let Some(host) = host_of(value) {
            if !host.ends_with(scope) && host != scope {
                return None;
            }
        }
    }

    let (class, path) = if value.starts_with("ws://") || value.starts_with("wss://") {
        (Class::Websocket, value.to_string())
    } else if value.starts_with("http://") || value.starts_with("https://") {
        let host = host_of(value).unwrap_or_default();
        let path = value
            .split("://")
            .nth(1)
            .and_then(|rest| rest.find('/').map(|i| &rest[i..]))
            .unwrap_or("/")
            .to_string();
        if let Some(scope) = scope {
            if !host.ends_with(scope) && host != scope {
                // External host, but scope was given: caller asked for
                // in-scope only, unless --include-external is handled
                // upstream by passing scope=None. Keep it as External
                // here so the caller can decide; classify() itself
                // does not drop it when scope is None.
                let _ = scope;
            }
        }
        // External if it has a host and the path is not an API path
        // on the same site; we still classify API-looking ones as Api.
        let is_api = is_api_path(&path);
        if is_static_path(&path) {
            (Class::Static, path)
        } else if is_api {
            (Class::Api, value.to_string())
        } else if host.is_empty() {
            (Class::Page, path)
        } else {
            (Class::External, value.to_string())
        }
    } else {
        // Relative / absolute path.
        if is_static_path(value) {
            (Class::Static, value.to_string())
        } else if is_api_path(value) {
            (Class::Api, value.to_string())
        } else {
            (Class::Page, value.to_string())
        }
    };

    let mut flags = Vec::new();
    let lower = path.to_lowercase();
    for (needle, flag) in INTERESTING {
        if lower.contains(&format!("/{}", needle)) || lower.contains(&format!("{}-", needle)) {
            flags.push(flag.to_string());
        }
    }
    if value.contains('{') || value.contains('?') {
        flags.push("parameterised".to_string());
    }
    if lower.contains("graphql") {
        flags.push("graphql".to_string());
    }
    if lower.contains("/v1/") || lower.contains("/v2/") || lower.contains("/v3/") {
        flags.push("versioned".to_string());
    }

    Some(Endpoint {
        endpoint: if class == Class::Api || class == Class::External || class == Class::Websocket {
            value.to_string()
        } else {
            path
        },
        source_file: source_file.to_string(),
        class,
        flags,
        method: guess_method(snippet),
    })
}

fn regex_api_version(path: &str) -> bool {
    // /v1/, /v2/ ... at the start, or /api/v1 style already handled.
    if let Some(rest) = path.strip_prefix('/') {
        if rest.len() > 3 && rest.as_bytes()[0] == b'v' && rest.as_bytes()[1].is_ascii_digit() && rest.as_bytes()[2] == b'/' {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn api_is_api() {
        let e = classify("/api/v1/users", "app.js", "", None).unwrap();
        assert_eq!(e.class, Class::Api);
    }

    #[test]
    fn image_is_static() {
        let e = classify("/static/logo.png", "app.js", "", None).unwrap();
        assert_eq!(e.class, Class::Static);
    }

    #[test]
    fn admin_is_flagged() {
        let e = classify("/api/v1/admin/users", "app.js", "", None).unwrap();
        assert!(e.flags.contains(&"admin".to_string()));
    }

    #[test]
    fn rest_and_graphql_are_api() {
        assert_eq!(classify("/rest/continue-code", "app.js", "", None).unwrap().class, Class::Api);
        assert_eq!(classify("/graphql", "app.js", "", None).unwrap().class, Class::Api);
    }

    #[test]
    fn graphql_is_flagged() {
        let e = classify("/graphql", "app.js", "", None).unwrap();
        assert!(e.flags.contains(&"graphql".to_string()));
    }

    #[test]
    fn page_is_page() {
        let e = classify("/dashboard/settings", "app.js", "", None).unwrap();
        assert_eq!(e.class, Class::Page);
    }
}
