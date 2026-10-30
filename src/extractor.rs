//! Endpoint extraction from JavaScript source.
//!
//! Two ideas, no heavy JS parser dependency:
//! 1. Tokenise string literals (single, double and backtick quotes).
//! 2. Re-join literals glued together with `+`, and resolve simple
//!    `const NAME = "literal"` base URLs, so code like
//!    `API + "/users"` or `"/api/" + "v1/users"` yields one endpoint.
//!    Template expressions `${...}` become `{param}` placeholders.

use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RawEndpoint {
    pub value: String,
    pub snippet: String,
}

/// A string literal found in source, with its byte span.
#[derive(Debug, Clone)]
struct Literal {
    value: String,
    start: usize,
    end: usize,
}

fn is_ident_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '$'
}

/// Scan source for string literals. Handles escapes and template
/// expressions (the `${...}` part is replaced by `{param}`).
fn scan_literals(src: &str) -> Vec<Literal> {
    let bytes = src.as_bytes();
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < bytes.len() {
        let c = bytes[i] as char;
        if c == '"' || c == '\'' || c == '`' {
            let quote = c;
            let start = i;
            i += 1;
            let mut value = String::new();
            let mut closed = false;
            while i < bytes.len() {
                let ch = bytes[i] as char;
                if quote == '`' && ch == '$' && i + 1 < bytes.len() && bytes[i + 1] == b'{' {
                    // Template expression: skip to matching '}' (nested braces).
                    let mut depth = 1i32;
                    let mut expr = String::new();
                    i += 2;
                    while i < bytes.len() && depth > 0 {
                        let e = bytes[i] as char;
                        if e == '{' {
                            depth += 1;
                        } else if e == '}' {
                            depth -= 1;
                            if depth == 0 {
                                i += 1;
                                break;
                            }
                        }
                        if depth > 0 {
                            expr.push(e);
                        }
                        i += 1;
                    }
                    let name = expr.trim().trim_start_matches('.');
                    let name = name
                        .chars()
                        .take_while(|ch| is_ident_char(*ch))
                        .collect::<String>();
                    if name.is_empty() {
                        value.push_str("{param}");
                    } else {
                        value.push('{');
                        value.push_str(&name);
                        value.push('}');
                    }
                    continue;
                }
                if ch == '\\' && i + 1 < bytes.len() {
                    // Keep escaped char as-is (e.g. \/ in JSON-ish strings).
                    let next = bytes[i + 1] as char;
                    if next == '/' {
                        value.push('/');
                    } else {
                        value.push(next);
                    }
                    i += 2;
                    continue;
                }
                if ch == quote {
                    closed = true;
                    i += 1;
                    break;
                }
                // Newlines end single/double quoted strings in real JS.
                if (quote == '"' || quote == '\'') && ch == '\n' {
                    break;
                }
                value.push(ch);
                i += 1;
            }
            if closed {
                out.push(Literal { value, start, end: i });
            }
        } else {
            i += 1;
        }
    }
    out
}

/// Collect simple `const|let|var NAME = "literal"` assignments.
fn collect_consts(src: &str, literals: &[Literal]) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for lit in literals {
        // Look backwards for `NAME =` right before this literal.
        let before = &src[..lit.start];
        let trimmed = before.trim_end();
        if !trimmed.ends_with('=') {
            continue;
        }
        let without_eq = trimmed[..trimmed.len() - 1].trim_end();
        // Identifier at the end.
        let mut name = String::new();
        for ch in without_eq.chars().rev() {
            if is_ident_char(ch) {
                name.insert(0, ch);
            } else {
                break;
            }
        }
        if !name.is_empty() && !lit.value.is_empty() {
            map.insert(name, lit.value.clone());
        }
    }
    map
}

/// Between two literals, is there only whitespace and a `+` (possibly
/// with an identifier that resolves to a const literal)?
fn join_token(src: &str, from: usize, to: usize, consts: &HashMap<String, String>) -> Option<Option<String>> {
    let between = src[from..to].trim();
    if between == "+" {
        return Some(None);
    }
    // `+ IDENT +` handled by the caller in two steps; here we only
    // recognise a bare identifier between plus signs on one side.
    if let Some(rest) = between.strip_prefix('+') {
        let ident = rest.trim();
        if !ident.is_empty() && ident.chars().all(is_ident_char) {
            if let Some(v) = consts.get(ident) {
                return Some(Some(v.clone()));
            }
        }
    }
    if let Some(rest) = between.strip_suffix('+') {
        let ident = rest.trim();
        if !ident.is_empty() && ident.chars().all(is_ident_char) {
            if let Some(v) = consts.get(ident) {
                return Some(Some(v.clone()));
            }
        }
    }
    None
}

/// Combine adjacent literals joined by `+`, with const resolution.
fn combine_literals(src: &str, literals: Vec<Literal>, consts: &HashMap<String, String>) -> Vec<String> {
    let mut values = Vec::new();
    let mut idx = 0usize;
    while idx < literals.len() {
        let mut current = literals[idx].value.clone();
        let mut end = literals[idx].end;
        let mut next = idx + 1;
        while next < literals.len() {
            match join_token(src, end, literals[next].start, consts) {
                Some(None) => {
                    current.push_str(&literals[next].value);
                    end = literals[next].end;
                    next += 1;
                }
                Some(Some(resolved)) => {
                    // `IDENT + "literal"` or `"literal" + IDENT`:
                    // the resolved value is one extra piece.
                    current.push_str(&resolved);
                    current.push_str(&literals[next].value);
                    next += 1;
                    break;
                }
                None => break,
            }
        }
        values.push(current);
        idx = next;
    }
    values
}

fn looks_like_endpoint(v: &str) -> bool {
    if v.len() < 2 || v.len() > 2048 {
        return false;
    }
    if v.contains(' ') || v.contains('\n') || v.contains('<') || v.contains('>') {
        return false;
    }
    // Full URLs.
    if v.starts_with("http://") || v.starts_with("https://") || v.starts_with("wss://") || v.starts_with("ws://") {
        // Must have a path beyond the host to be an endpoint (or be a ws url).
        if v.starts_with("ws") {
            return true;
        }
        let after_scheme = &v[v.find("://").unwrap() + 3..];
        return after_scheme.contains('/');
    }
    // Absolute paths.
    if v.starts_with('/') {
        // Drop single slash, protocol-relative //host (handled below), and
        // obvious non-routes.
        if v == "/" || v.starts_with("//") {
            return false;
        }
        // Must look like a path: at least one word char, and not just a file
        // extension alone. Anything with a second segment or a known
        // API-ish first segment qualifies; the classifier sorts the rest.
        return v.chars().any(|c| c.is_ascii_alphanumeric());
    }
    // Relative API paths like `api/v1/users`.
    if v.starts_with("api/") || v.starts_with("v1/") || v.starts_with("v2/") || v.starts_with("v3/") {
        return true;
    }
    false
}

fn snippet_around(src: &str, needle: &str) -> String {
    if let Some(pos) = src.find(needle) {
        // Byte offsets can split a multi-byte UTF-8 character (for
        // example Bangla text in a bundle). Move to the nearest char
        // boundaries first, otherwise this slice panics.
        let mut start = pos.saturating_sub(40);
        while !src.is_char_boundary(start) {
            start += 1;
        }
        let mut end = (pos + needle.len() + 40).min(src.len());
        while !src.is_char_boundary(end) {
            end -= 1;
        }
        src[start..end].replace('\n', " ").trim().to_string()
    } else {
        needle.to_string()
    }
}

/// Extract endpoints from one JavaScript source string.
/// Resolve `IDENT + "literal"` and `"literal" + IDENT` where IDENT is a
/// known const. This covers the common `fetch(API + "/users")` case,
/// where the const definition sits far before the use site.
fn const_joined_values(src: &str, literals: &[Literal], consts: &HashMap<String, String>) -> Vec<String> {
    let mut out = Vec::new();
    for lit in literals {
        // Look backwards: `IDENT +` right before this literal.
        let before = src[..lit.start].trim_end();
        if before.ends_with('+') {
            let prefix = before[..before.len() - 1].trim_end();
            let mut name = String::new();
            for ch in prefix.chars().rev() {
                if is_ident_char(ch) {
                    name.insert(0, ch);
                } else {
                    break;
                }
            }
            if let Some(base) = consts.get(&name) {
                // Skip the const's own definition literal: its preceding
                // text is `NAME =`, not `NAME +`.
                out.push(format!("{}{}", base, lit.value));
            }
        }
        // Look forwards: `+ IDENT` right after this literal.
        let after = src[lit.end..].trim_start();
        if let Some(rest) = after.strip_prefix('+') {
            let rest = rest.trim_start();
            let name: String = rest.chars().take_while(|c| is_ident_char(*c)).collect();
            if let Some(suffix) = consts.get(&name) {
                out.push(format!("{}{}", lit.value, suffix));
            }
        }
    }
    out
}

/// Language-aware regex pass: catches endpoint patterns in code
/// written in any language (Python requests, Go http, PHP curl,
/// Ruby, Java, JSON, HTML) where the string-literal tokenizer
/// alone might miss f-strings, format strings or split literals.
/// This is a second pass, deduplicated against the literal pass.
fn regex_pass(src: &str) -> Vec<String> {
    let mut out = Vec::new();
    if let Ok(re) = regex::Regex::new(r#"https?://[^\s"'`<>]+|/api/[^\s"'`<>]+|/rest/[^\s"'`<>]+|/graphql[^\s"'`<>]*"#) {
        for m in re.find_iter(src) {
            let mut v = m.as_str().trim_end_matches(|c| ".,;:)}]".contains(c)).to_string();
            // Strip a trailing quote/backtick the regex may have kept.
            while v.ends_with('"') || v.ends_with('\'') || v.ends_with('`') {
                v.pop();
            }
            // Skip raw template literals: the literal pass already
            // converts ${...} to {param}, and keeping both would
            // duplicate every templated endpoint.
            if v.contains("${") {
                continue;
            }
            if looks_like_endpoint(&v) {
                out.push(v);
            }
        }
    }
    out
}

/// Dedup key: template placeholders are normalised, so
/// `/api/users/{user}/orders` (tokenizer) and
/// `/api/users/{param}/orders` (SWC) count as the same endpoint.
fn dedup_key(value: &str) -> String {
    let mut out = String::new();
    let mut in_brace = false;
    for c in value.chars() {
        if c == '{' {
            in_brace = true;
            out.push_str("{}");
        } else if c == '}' {
            in_brace = false;
        } else if !in_brace {
            out.push(c);
        }
    }
    out
}

pub fn extract_from_js(src: &str) -> Vec<RawEndpoint> {
    let literals = scan_literals(src);
    let consts = collect_consts(src, &literals);
    let mut combined = combine_literals(src, literals.clone(), &consts);
    combined.extend(const_joined_values(src, &literals, &consts));
    combined.extend(regex_pass(src));
    // SWC real parsing: exact string and template literals, even in
    // minified or nested code. On parse failure it returns nothing
    // and the passes above still stand. Filter SWC fragments: a
    // literal that is only a piece of an already-joined endpoint
    // ("/api/" or "v2/orders" inside "/api/v2/orders") is not an
    // endpoint on its own, and a trailing-slash literal alone is
    // a base prefix, not a route.
    let tokenizer_values = combined.clone();
    for swc_value in crate::swc_extract::extract_swc(src) {
        if swc_value.ends_with('/') {
            continue;
        }
        if tokenizer_values.iter().any(|t| t != &swc_value && t.contains(&swc_value)) {
            continue;
        }
        combined.push(swc_value);
    }

    let mut out: Vec<RawEndpoint> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for value in combined {
        // Protocol-relative URLs: //api.example.com/v1/users
        let value = if value.starts_with("//") {
            format!("https:{}", value)
        } else {
            value
        };
        // A literal may contain several space-separated URLs in minified
        // code; only keep it whole if it looks like one endpoint.
        // Dedup is on the normalised key, so {user} and {param}
        // versions of the same template count as one endpoint.
        if looks_like_endpoint(&value) && seen.insert(dedup_key(&value)) {
            let snippet = snippet_around(src, &value);
            out.push(RawEndpoint { value, snippet });
        }
    }
    out
}

/// Extract endpoints from a source-map JSON blob: its `sources` and
/// embedded string content can reveal original paths.
pub fn extract_from_sourcemap(json: &str) -> Vec<RawEndpoint> {
    // Reuse the JS extractor: source maps are JSON, and their string
    // values are scanned the same way.
    extract_from_js(json)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn values(src: &str) -> Vec<String> {
        extract_from_js(src).into_iter().map(|e| e.value).collect()
    }

    #[test]
    fn simple_fetch_path() {
        let v = values(r#"fetch("/api/v1/users").then(r => r.json())"#);
        assert!(v.contains(&"/api/v1/users".to_string()));
    }

    #[test]
    fn concatenation_is_joined() {
        let v = values(r#"fetch("/api/" + "v1/users")"#);
        assert!(v.contains(&"/api/v1/users".to_string()));
    }

    #[test]
    fn const_base_is_resolved() {
        let v = values(r#"const API = "https://api.example.com"; fetch(API + "/users")"#);
        assert!(v.contains(&"https://api.example.com/users".to_string()));
    }

    #[test]
    fn template_literal_becomes_placeholder() {
        let v = values("fetch(`/api/users/${id}/orders`)");
        assert!(v.contains(&"/api/users/{id}/orders".to_string()));
    }

    #[test]
    fn junk_strings_are_dropped() {
        let v = values(r#"const x = "hello world"; const y = "red";"#);
        assert!(v.is_empty());
    }

    #[test]
    fn bangla_text_does_not_panic_on_snippet_slice() {
        // Regression test for the Star Cineplex panic: a Bangla string
        // (3 bytes per character) sits within 40 bytes of an endpoint,
        // so a raw byte slice would cut a character in half and panic.
        let src = "const title = \"গগগগগগগগগগগগগগগ\"; fetch(\"/api/movies\"); const tail = \"গগগগগগগগগগগগগগগ\";";
        let v = values(src);
        assert!(v.contains(&"/api/movies".to_string()));
    }

    #[test]
    fn python_and_go_calls_are_found() {
        let v = values("import requests\nrequests.post('https://api.example.com/v1/login', data={})");
        assert!(v.contains(&"https://api.example.com/v1/login".to_string()));
        let v = values("resp, _ := http.Get(\"/api/v2/status\")");
        assert!(v.contains(&"/api/v2/status".to_string()));
    }

    #[test]
    fn websocket_is_kept() {
        let v = values(r#"new WebSocket("wss://example.com/socket")"#);
        assert!(v.contains(&"wss://example.com/socket".to_string()));
    }
}
