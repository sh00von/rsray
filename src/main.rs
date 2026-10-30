//! rsray — X-ray into JavaScript bundles.
//!
//! Usage:
//!   rsray app.js
//!   rsray https://example.com/app.js --json -o endpoints.json
//!   rsray https://example.com --wordlist words.txt
//!   cat js_urls.txt | rsray --stdin
//!
//! Only run rsray on sites you own or targets you are authorised to test.

mod classifier;
mod extractor;
mod swc_extract;

use classifier::{Class, Endpoint};
use clap::Parser;
use extractor::RawEndpoint;
use regex::Regex;
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

#[derive(Parser, Debug)]
#[command(name = "rsray", version, about = "X-ray into JavaScript bundles. Find the hidden API surface.", long_about = "rsray — X-ray into JavaScript bundles. Find the hidden API surface.\n\nEXAMPLES:\n  rsray app.js\n  rsray https://example.com/app.js --json -o endpoints.json\n  rsray https://example.com --wordlist words.txt\n  cat js_urls.txt | rsray --stdin\n\nOnly run rsray on sites you own or targets you are authorised to test.\nProgress and timing go to stderr, so stdout stays clean for piping.")]


struct Args {
    /// Input: a JS file, a directory, a JS URL, or a website URL.
    /// Use "-" or --stdin to read a list of inputs from stdin.
    input: Option<String>,

    /// Write results to a file instead of stdout.
    #[arg(short, long)]
    output: Option<PathBuf>,

    /// JSON output.
    #[arg(long)]
    json: bool,

    /// CSV output.
    #[arg(long)]
    csv: bool,

    /// Endpoints only, no summary.
    #[arg(short, long)]
    quiet: bool,

    /// Show source file and code snippet per endpoint.
    #[arg(short, long)]
    verbose: bool,

    /// Also list static asset paths (images, CSS, fonts).
    #[arg(long)]
    include_static: bool,

    /// Also report endpoints on external domains (site mode only).
    #[arg(long)]
    include_external: bool,

    /// Also report high-confidence secret patterns (off by default).
    #[arg(long)]
    secrets: bool,

    /// Also write a target wordlist to this file.
    #[arg(long)]
    wordlist: Option<PathBuf>,

    /// Only report endpoints for this domain (e.g. example.com).
    #[arg(long)]
    scope: Option<String>,

    /// Read inputs (one per line) from stdin.
    #[arg(long)]
    stdin: bool,

    /// Chunk-follow depth when crawling a site (default 5).
    #[arg(long, default_value = "5")]
    depth: usize,

    /// Maximum internal pages to crawl from a site (default 20, 0 = homepage only).
    #[arg(long, default_value = "20")]
    max_pages: usize,

    /// Parallel fetch concurrency for site scripts (default 16).
    #[arg(long, default_value = "16")]
    concurrency: usize,

    /// Only show endpoints flagged interesting (admin, internal, debug, etc).
    #[arg(long)]
    interesting_only: bool,

    /// Output full URLs only, one per line, ready for httpx/ffuf.
    #[arg(long)]
    urls: bool,

    /// Disable colours even on a terminal.
    #[arg(long)]
    no_color: bool,

    /// Extra request header, repeatable: --header "Authorization: Bearer X".
    #[arg(long)]
    header: Vec<String>,

    /// Cookie header value, e.g. "session=abc; other=1".
    #[arg(long)]
    cookie: Option<String>,

    /// HTTP(S) proxy, e.g. http://127.0.0.1:8080 (for Burp).
    #[arg(long)]
    proxy: Option<String>,

    /// Per-request timeout in seconds (default 10).
    #[arg(long, default_value = "10")]
    timeout: u64,

    /// Delay in milliseconds between crawled page fetches (default 0).
    #[arg(long, default_value = "0")]
    delay_ms: u64,

    /// Retries on network errors and 5xx responses (default 0).
    #[arg(long, default_value = "0")]
    retries: usize,

    /// Skip URLs containing this substring, repeatable.
    #[arg(long)]
    exclude: Vec<String>,

    /// TOML config file (CLI flags override config values).
    #[arg(long)]
    config: Option<PathBuf>,
}

/// Options that travel with every HTTP request.
#[derive(Clone, Default)]
struct HttpOpts {
    headers: Vec<String>,
    cookie: Option<String>,
    proxy: Option<String>,
    timeout_secs: u64,
    retries: usize,
}

impl HttpOpts {
    fn retries(&self) -> usize {
        self.retries
    }
}

struct Source {
    name: String,
    content: String,
}

fn client(opts: &HttpOpts) -> reqwest::blocking::Client {
    let mut builder = reqwest::blocking::Client::builder()
        .user_agent("rsray (endpoint extraction; authorised testing only)")
        .timeout(Duration::from_secs(if opts.timeout_secs == 0 { 10 } else { opts.timeout_secs }));
    // Default headers apply to every request (auth, custom headers).
    let mut headers = reqwest::header::HeaderMap::new();
    for h in &opts.headers {
        if let Some((k, v)) = h.split_once(':') {
            if let (Ok(name), Ok(val)) = (
                reqwest::header::HeaderName::from_bytes(k.trim().as_bytes()),
                reqwest::header::HeaderValue::from_str(v.trim()),
            ) {
                headers.insert(name, val);
            }
        }
    }
    if let Some(cookie) = &opts.cookie {
        if let Ok(val) = reqwest::header::HeaderValue::from_str(cookie) {
            headers.insert(reqwest::header::COOKIE, val);
        }
    }
    if !headers.is_empty() {
        builder = builder.default_headers(headers);
    }
    if let Some(proxy) = &opts.proxy {
        if let Ok(p) = reqwest::Proxy::all(proxy) {
            builder = builder.proxy(p);
        }
    }
    builder.build().expect("http client")
}

fn excluded(url: &str, exclude: &[String]) -> bool {
    exclude.iter().any(|e| !e.is_empty() && url.contains(e.as_str()))
}

/// Quiet fetch: returns None on failure without printing, for
/// optional files like source maps, sitemaps and robots.txt.
/// Retries only network errors and 5xx, never 404/403.
fn fetch_url_quiet(client: &reqwest::blocking::Client, url: &str, retries: usize) -> Option<String> {
    for attempt in 0..=retries {
        match client.get(url).send() {
            Ok(resp) if resp.status().is_success() => return resp.text().ok(),
            Ok(resp) if resp.status().is_server_error() && attempt < retries => {
                std::thread::sleep(Duration::from_millis(200));
                continue;
            }
            Ok(_) => return None,
            Err(_) if attempt < retries => {
                std::thread::sleep(Duration::from_millis(200));
                continue;
            }
            Err(_) => return None,
        }
    }
    None
}

/// Fetch many URLs in parallel, using a thread per URL.
/// Returns (url, body) pairs for the URLs that succeeded, in URL order.
fn fetch_many(
    client: &reqwest::blocking::Client,
    urls: &[String],
    concurrency: usize,
    retries: usize,
) -> Vec<(String, String)> {
    use std::sync::Mutex;
    let concurrency = concurrency.max(1);
    let mut all: Vec<(String, String)> = Vec::new();
    for chunk in urls.chunks(concurrency) {
        let results: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());
        std::thread::scope(|scope| {
            for url in chunk {
                let cl = client.clone();
                let url = url.clone();
                let results = &results;
                scope.spawn(move || {
                    if let Some(body) = fetch_url_quiet(&cl, &url, retries) {
                        results.lock().unwrap().push((url, body));
                    }
                });
            }
        });
        all.extend(results.into_inner().unwrap());
    }
    all.sort_by(|a, b| a.0.cmp(&b.0));
    all
}

/// Internal <a href> links on a page, same host only.
fn internal_links(html: &str, base: &str, host: &str) -> Vec<String> {
    let re = Regex::new(r#"(?i)<a[^>]+href=["']([^"']+)["']"#).unwrap();
    let mut out = Vec::new();
    for cap in re.captures_iter(html) {
        if let Some(m) = cap.get(1) {
            let href = m.as_str().trim();
            if href.is_empty() || href.starts_with('#') || href.starts_with("mailto:") || href.starts_with("tel:") || href.starts_with("javascript:") {
                continue;
            }
            let url = resolve_url(base, href);
            let url = url.split('#').next().unwrap_or(&url).to_string();
            if host_of_url(&url).as_deref() == Some(host) {
                let lower = url.to_lowercase();
                if !lower.ends_with(".css") && !lower.ends_with(".png") && !lower.ends_with(".jpg") && !lower.ends_with(".jpeg") && !lower.ends_with(".gif") && !lower.ends_with(".svg") && !lower.ends_with(".ico") {
                    out.push(url);
                }
            }
        }
    }
    out
}

/// URLs listed in a sitemap.xml body.
fn sitemap_urls(xml: &str, host: &str) -> Vec<String> {
    let re = Regex::new(r"<loc>([^<]+)</loc>").unwrap();
    re.captures_iter(xml)
        .filter_map(|c| c.get(1).map(|m| m.as_str().trim().to_string()))
        .filter(|u| host_of_url(u).as_deref() == Some(host))
        .collect()
}

fn fetch_url(client: &reqwest::blocking::Client, url: &str, retries: usize) -> Option<String> {
    for attempt in 0..=retries {
        match client.get(url).send() {
            Ok(resp) if !resp.status().is_success() => {
                if resp.status().is_server_error() && attempt < retries {
                    std::thread::sleep(Duration::from_millis(200));
                    continue;
                }
                eprintln!("rsray: could not fetch {}: HTTP {}", url, resp.status());
                return None;
            }
            Ok(resp) => match resp.text() {
                Ok(t) => return Some(t),
                Err(e) => {
                    eprintln!("rsray: could not read {}: {}", url, e);
                    return None;
                }
            },
            Err(e) => {
                if attempt < retries {
                    std::thread::sleep(Duration::from_millis(200));
                    continue;
                }
                eprintln!("rsray: could not fetch {}: {}", url, e);
                return None;
            }
        }
    }
    None
}

/// Resolve a possibly-relative script URL against a base page URL.
fn resolve_url(base: &str, rel: &str) -> String {
    if rel.starts_with("http://") || rel.starts_with("https://") || rel.starts_with("//") {
        if rel.starts_with("//") {
            let scheme = base.split("://").next().unwrap_or("https");
            return format!("{}:{}", scheme, rel);
        }
        return rel.to_string();
    }
    // Base origin + path handling.
    let (scheme_host, base_path) = match base.split_once("://") {
        Some((scheme, rest)) => {
            let host = rest.split('/').next().unwrap_or("");
            let path = rest[rest.len().min(host.len())..].to_string();
            (format!("{}://{}", scheme, host), path)
        }
        None => (String::new(), base.to_string()),
    };
    if rel.starts_with('/') {
        return format!("{}{}", scheme_host, rel);
    }
    // Relative to the base page's directory.
    let dir = match base_path.rfind('/') {
        Some(i) => &base_path[..=i],
        None => "/",
    };
    format!("{}{}{}", scheme_host, dir, rel)
}

fn host_of_url(url: &str) -> Option<String> {
    url.split("://").nth(1).and_then(|rest| rest.split('/').next()).map(|s| s.to_string())
}

/// Extract <script src="..."> values from HTML (regex is enough here:
/// we only need src attributes, not a full DOM parse).
fn script_srcs(html: &str) -> Vec<String> {
    let re = Regex::new(r#"(?i)<script[^>]+src=["']([^"']+)["']"#).unwrap();
    re.captures_iter(html).filter_map(|c| c.get(1).map(|m| m.as_str().to_string())).collect()
}

/// JS chunk references inside a bundle: strings ending in .js that look
/// like chunk files (used for depth-following).
fn chunk_refs(js: &str, base_url: &str) -> Vec<String> {
    let mut out = Vec::new();
    for raw in extractor::extract_from_js(js) {
        let v = raw.value;
        if v.ends_with(".js") && !v.starts_with("http") {
            out.push(resolve_url(base_url, &v));
        }
    }
    out
}

fn collect_from_dir(dir: &Path, out: &mut Vec<Source>) {
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                collect_from_dir(&path, out);
            } else if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                if ["js", "mjs", "cjs", "ts", "tsx", "jsx", "py", "go", "php", "rb", "java", "json", "html", "htm"].contains(&ext) {
                    if let Ok(content) = fs::read_to_string(&path) {
                        out.push(Source { name: path.display().to_string(), content });
                    }
                }
            }
        }
    }
}

/// Log a sqlmap-style progress line to stderr: elapsed time + message.
/// Stderr only, so stdout stays clean for piping into other tools.
fn progress(start: &Instant, quiet: bool, msg: &str) {
    if !quiet {
        eprintln!("[+{:.2}s] {}", start.elapsed().as_secs_f64(), msg);
    }
}

/// Parse a HAR file into one combined source: method + URL per
/// entry, plus any response body text. Falls back to None if the
/// file is not a HAR, in which case the raw text is used as-is.
fn parse_har(content: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(content).ok()?;
    let entries = v.get("log")?.get("entries")?.as_array()?;
    let mut out = String::new();
    for e in entries {
        let method = e.get("request").and_then(|r| r.get("method")).and_then(|m| m.as_str()).unwrap_or("GET");
        let url = e.get("request").and_then(|r| r.get("url")).and_then(|u| u.as_str()).unwrap_or("");
        out.push_str(&format!("{} {}\n", method, url));
        if let Some(text) = e.get("response").and_then(|r| r.get("content")).and_then(|c| c.get("text")).and_then(|t| t.as_str()) {
            out.push_str(text);
            out.push('\n');
        }
    }
    if out.is_empty() { None } else { Some(out) }
}

/// Next.js buildId from __NEXT_DATA__ in a page body, if present.
fn next_build_id(html: &str) -> Option<String> {
    let re = Regex::new(r#""buildId"\s*:\s*"([^"]+)""#).ok()?;
    re.captures(html).and_then(|c| c.get(1)).map(|m| m.as_str().to_string())
}

/// Gather all JS sources for one input string.
fn gather(input: &str, depth: usize, start: &Instant, quiet: bool, max_pages: usize, concurrency: usize, http: &HttpOpts, delay_ms: u64, exclude: &[String]) -> Vec<Source> {
    let path = Path::new(input);
    if path.is_file() {
        if let Ok(content) = fs::read_to_string(path) {
            if path.extension().and_then(|e| e.to_str()) == Some("har") {
                if let Some(har) = parse_har(&content) {
                    return vec![Source { name: format!("{} (har)", input), content: har }];
                }
            }
            return vec![Source { name: input.to_string(), content }];
        }
        return vec![];
    }
    if path.is_dir() {
        let mut out = Vec::new();
        collect_from_dir(path, &mut out);
        return out;
    }
    if input.starts_with("http://") || input.starts_with("https://") {
        let cl = client(http);
        let body = match fetch_url(&cl, input, http.retries()) {
            Some(b) => b,
            None => return vec![],
        };
        if input.ends_with(".js") {
            progress(start, quiet, &format!("fetched {} ({} bytes)", input, body.len()));
            let mut sources = vec![Source { name: input.to_string(), content: body.clone() }];
            // Try the source map next to the bundle.
            let map_url = format!("{}.map", input);
            if let Some(map) = fetch_url_quiet(&cl, &map_url, http.retries()) {
                sources.push(Source { name: map_url, content: map });
            }
            return sources;
        }
        if input.ends_with(".map") {
            return vec![Source { name: input.to_string(), content: body }];
        }
        // Otherwise treat it as a website: crawl its pages, then fetch
        // all scripts in parallel, then follow chunks recursively.
        let host = host_of_url(input).unwrap_or_default();
        let origin = {
            let (scheme, rest) = input.split_once("://").unwrap_or(("https", input));
            let h = rest.split('/').next().unwrap_or("");
            format!("{}://{}", scheme, h)
        };
        let mut sources: Vec<Source> = Vec::new();
        let mut script_urls: Vec<String> = Vec::new();
        let mut seen_scripts: HashSet<String> = HashSet::new();
        // Crawl pages: homepage first, then internal links, sitemap, robots.
        let mut page_queue: Vec<String> = vec![input.to_string()];
        let mut seen_pages: HashSet<String> = HashSet::new();
        let mut pages_done = 0usize;
        // Sitemap and robots are one-time seeds from the origin.
        if let Some(xml) = fetch_url_quiet(&cl, &format!("{}/sitemap.xml", origin), http.retries()) {
            for u in sitemap_urls(&xml, &host) {
                if !excluded(&u, exclude) && !page_queue.contains(&u) {
                    page_queue.push(u);
                }
            }
        }
        let mut robots_disallow: Vec<String> = Vec::new();
        if let Some(robots) = fetch_url_quiet(&cl, &format!("{}/robots.txt", origin), http.retries()) {
            for line in robots.lines() {
                let line = line.trim();
                if let Some(rest) = line.strip_prefix("Disallow:") {
                    let p = rest.trim();
                    if p.starts_with('/') {
                        robots_disallow.push(p.to_string());
                    }
                }
                if let Some(rest) = line.strip_prefix("Sitemap:") {
                    let sm = rest.trim();
                    if let Some(xml) = fetch_url_quiet(&cl, sm, http.retries()) {
                        for u in sitemap_urls(&xml, &host) {
                            if !excluded(&u, exclude) && !page_queue.contains(&u) {
                                page_queue.push(u);
                            }
                        }
                    }
                }
            }
        }
        if !robots_disallow.is_empty() {
            // Disallowed paths are hunter gold: surface them as a source
            // so the extractor finds them as endpoints.
            sources.push(Source { name: format!("{}/robots.txt", origin), content: robots_disallow.join("\n") });
        }
        let mut build_id: Option<String> = next_build_id(&body);
        let mut crawled_pages: Vec<String> = Vec::new();
        while let Some(page) = page_queue.first().cloned() {
            page_queue.remove(0);
            if !seen_pages.insert(page.clone()) {
                continue;
            }
            if excluded(&page, exclude) {
                continue;
            }
            if max_pages > 0 && pages_done >= max_pages {
                break;
            }
            pages_done += 1;
            crawled_pages.push(page.clone());
            let page_body = if page == input { body.clone() } else { match fetch_url_quiet(&cl, &page, http.retries()) { Some(b) => b, None => continue } };
            if build_id.is_none() {
                build_id = next_build_id(&page_body);
            }
            if delay_ms > 0 && page != input {
                std::thread::sleep(Duration::from_millis(delay_ms));
            }
            for s in script_srcs(&page_body) {
                let u = resolve_url(&page, &s);
                if !excluded(&u, exclude) && seen_scripts.insert(u.clone()) {
                    script_urls.push(u);
                }
            }
            // Inline scripts on every crawled page.
            let inline_re = Regex::new(r"(?is)<script([^>]*)>(.*?)</script>").unwrap();
            for cap in inline_re.captures_iter(&page_body) {
                let attrs = cap.get(1).map(|m| m.as_str()).unwrap_or("");
                if !attrs.to_ascii_lowercase().contains("src=") {
                    if let Some(m) = cap.get(2) {
                        let script = m.as_str().trim();
                        if !script.is_empty() {
                            sources.push(Source { name: format!("{} (inline)", page), content: script.to_string() });
                        }
                    }
                }
            }
            if max_pages == 0 {
                break;
            }
            for link in internal_links(&page_body, &page, &host) {
                if !excluded(&link, exclude) && !seen_pages.contains(&link) && !page_queue.contains(&link) {
                    page_queue.push(link);
                }
            }
        }
        // Next.js: fetch _next/data JSON for each crawled page, if a
        // buildId was found. These payloads carry API routes that
        // the page HTML alone does not.
        if let Some(bid) = &build_id {
            let mut data_urls: Vec<String> = Vec::new();
            for page in &crawled_pages {
                let path = page.strip_prefix(&origin).unwrap_or("/");
                let path = path.split('?').next().unwrap_or(path);
                let data_path = if path.is_empty() || path == "/" {
                    "/index.json".to_string()
                } else {
                    format!("{}.json", path.trim_end_matches('/'))
                };
                let u = format!("{}/_next/data/{}{}", origin, bid, data_path);
                if !excluded(&u, exclude) {
                    data_urls.push(u);
                }
            }
            for (u, json_body) in fetch_many(&cl, &data_urls, concurrency, http.retries()) {
                sources.push(Source { name: u, content: json_body });
            }
        }
        // Fetch scripts in parallel, following chunks level by level.
        let mut queue = script_urls;
        let mut seen: HashSet<String> = seen_scripts;
        let mut current_depth = 0usize;
        while !queue.is_empty() && current_depth <= depth {
            let fetched = fetch_many(&cl, &queue, concurrency, http.retries());
            let mut next_queue: Vec<String> = Vec::new();
            let mut fetched_urls: Vec<String> = Vec::new();
            for (url, js) in fetched {
                progress(start, quiet, &format!("fetched {} ({} bytes)", url, js.len()));
                fetched_urls.push(url.clone());
                if current_depth < depth {
                    for chunk in chunk_refs(&js, &url) {
                        if !seen.contains(&chunk) {
                            seen.insert(chunk.clone());
                            next_queue.push(chunk);
                        }
                    }
                }
                sources.push(Source { name: url, content: js });
            }
            // Source maps for this level, in parallel, quietly.
            let map_urls: Vec<String> = fetched_urls.iter().map(|u| format!("{}.map", u)).collect();
            for (map_url, map) in fetch_many(&cl, &map_urls, concurrency, http.retries()) {
                sources.push(Source { name: map_url, content: map });
            }
            queue = next_queue;
            current_depth += 1;
        }
        return sources;
    }
    eprintln!("rsray: cannot read input: {}", input);
    vec![]
}

fn wordlist_from(endpoints: &[Endpoint]) -> Vec<String> {
    let mut words: HashSet<String> = HashSet::new();
    for e in endpoints {
        let path = e.endpoint.split("://").nth(1).and_then(|rest| rest.find('/').map(|i| &rest[i..])).unwrap_or(&e.endpoint);
        for seg in path.split(|c| "/?&=#".contains(c)) {
            let clean = seg.trim_matches(|c| "{}[]".contains(c));
            if clean.len() >= 2 && clean.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
                words.insert(clean.to_string());
            }
        }
    }
    let mut v: Vec<String> = words.into_iter().collect();
    v.sort();
    v
}

fn secret_hits(content: &str) -> Vec<String> {
    let patterns = [
        (r#"(?i)(api[_-]?key|apikey)\s*[:=]\s*["'][A-Za-z0-9_\-]{16,}["']"#, "api-key"),
        (r#"(?i)(secret|token)\s*[:=]\s*["'][A-Za-z0-9_\-\.]{20,}["']"#, "token/secret"),
        (r#"AKIA[0-9A-Z]{16}"#, "aws-access-key"),
        (r#"ghp_[A-Za-z0-9]{20,}"#, "github-token"),
        (r#"sk_live_[A-Za-z0-9]{16,}"#, "stripe-live-key"),
        (r#"rk_live_[A-Za-z0-9]{16,}"#, "stripe-restricted-key"),
        (r#"xox[baprs]-[A-Za-z0-9\-]{10,}"#, "slack-token"),
        (r#"AIza[0-9A-Za-z_\-]{35}"#, "google-api-key"),
        (r#"-----BEGIN (?:RSA |EC |OPENSSH )?PRIVATE KEY-----"#, "private-key"),
        (r#"eyJ[A-Za-z0-9_\-]{10,}\.[A-Za-z0-9_\-]{10,}\.[A-Za-z0-9_\-]{10,}"#, "jwt"),
        // Second batch: only prefix-anchored, high-confidence formats.
        (r#"sk-(?:proj-)?[A-Za-z0-9_\-]{20,}"#, "openai-key"),
        (r#"sk-ant-[A-Za-z0-9_\-]{20,}"#, "anthropic-key"),
        (r#"github_pat_[A-Za-z0-9_]{20,}"#, "github-fine-grained-pat"),
        (r#"gho_[A-Za-z0-9]{36,}"#, "github-oauth-token"),
        (r#"glpat-[A-Za-z0-9_\-]{20}"#, "gitlab-pat"),
        (r#"npm_[A-Za-z0-9]{36}"#, "npm-token"),
        (r#"pypi-[A-Za-z0-9_\-]{20,}"#, "pypi-token"),
        (r#"SG\.[\w\-]{22}\.[\w\-]{43}"#, "sendgrid-key"),
        (r#"SK[0-9a-f]{32}"#, "twilio-key"),
        (r#"key-[0-9a-f]{32}"#, "mailgun-key"),
        (r#"sq0atp-[A-Za-z0-9_\-]{22}"#, "square-token"),
        (r#"shpat_[0-9a-f]{32}"#, "shopify-token"),
        (r#"(?i)aws_secret_access_key\s*[:=]\s*["'][A-Za-z0-9/+=]{40}["']"#, "aws-secret-key"),
        (r#"[0-9]{8,10}:[A-Za-z0-9_\-]{35}"#, "telegram-bot-token"),
        (r#"(?i)(?:postgres|mysql|mongodb)(?:\+\w+)?://[^\s:"']+:[^\s@"']+@"#, "db-url-with-password"),
    ];
    let mut hits = Vec::new();
    for (pat, label) in patterns {
        if let Ok(re) = Regex::new(pat) {
            for m in re.find_iter(content) {
                let mut sstart = m.start().saturating_sub(10);
                while !content.is_char_boundary(sstart) {
                    sstart += 1;
                }
                let mut send = m.end().min(content.len());
                while !content.is_char_boundary(send) {
                    send -= 1;
                }
                let snippet = &content[sstart..send];
                hits.push(format!("{}: {}", label, snippet.chars().take(60).collect::<String>()));
            }
        }
    }
    hits
}

/// Optional TOML config file. Every field is optional; see
/// --config. Example:
///   depth = 5
///   max_pages = 20
///   header = ["Authorization: Bearer X"]
///   exclude = ["/logout"]
#[derive(serde::Deserialize, Default)]
struct ConfigFile {
    output: Option<String>,
    json: Option<bool>,
    csv: Option<bool>,
    quiet: Option<bool>,
    verbose: Option<bool>,
    include_static: Option<bool>,
    include_external: Option<bool>,
    secrets: Option<bool>,
    wordlist: Option<String>,
    scope: Option<String>,
    depth: Option<usize>,
    max_pages: Option<usize>,
    concurrency: Option<usize>,
    interesting_only: Option<bool>,
    urls: Option<bool>,
    no_color: Option<bool>,
    header: Option<Vec<String>>,
    cookie: Option<String>,
    proxy: Option<String>,
    timeout: Option<u64>,
    delay_ms: Option<u64>,
    retries: Option<usize>,
    exclude: Option<Vec<String>>,
}

#[derive(Serialize)]
struct JsonEndpoint<'a> {
    endpoint: &'a str,
    source_file: &'a str,
    class: Class,
    flags: &'a [String],
    method: &'a str,
}

fn main() {
    // Help aliases: `--h` and `-help` are not standard clap flags,
    // but users type them, so treat both exactly like `--help`
    // (full help) instead of erroring on `--h`.
    let argv: Vec<String> = std::env::args()
        .map(|a| if a == "--h" || a == "-help" { "--help".to_string() } else { a })
        .collect();
    let mut args = Args::parse_from(argv);
    // TOML config: --config rsray.toml. CLI flags override config
    // for values the user actually set (bools are OR-ed, since a
    // bool flag has no off form; numbers/options fall back to
    // config only when still at their clap default).
    if let Some(cfg_path) = &args.config {
        if let Ok(text) = fs::read_to_string(cfg_path) {
            if let Ok(cfg) = toml::from_str::<ConfigFile>(&text) {
                if cfg.json.unwrap_or(false) { args.json = true; }
                if cfg.csv.unwrap_or(false) { args.csv = true; }
                if cfg.quiet.unwrap_or(false) { args.quiet = true; }
                if cfg.verbose.unwrap_or(false) { args.verbose = true; }
                if cfg.include_static.unwrap_or(false) { args.include_static = true; }
                if cfg.include_external.unwrap_or(false) { args.include_external = true; }
                if cfg.secrets.unwrap_or(false) { args.secrets = true; }
                if cfg.interesting_only.unwrap_or(false) { args.interesting_only = true; }
                if cfg.urls.unwrap_or(false) { args.urls = true; }
                if cfg.no_color.unwrap_or(false) { args.no_color = true; }
                if args.output.is_none() { args.output = cfg.output.map(PathBuf::from); }
                if args.wordlist.is_none() { args.wordlist = cfg.wordlist.map(PathBuf::from); }
                if args.scope.is_none() { args.scope = cfg.scope; }
                if args.cookie.is_none() { args.cookie = cfg.cookie; }
                if args.proxy.is_none() { args.proxy = cfg.proxy; }
                if args.depth == 5 { if let Some(v) = cfg.depth { args.depth = v; } }
                if args.max_pages == 20 { if let Some(v) = cfg.max_pages { args.max_pages = v; } }
                if args.concurrency == 16 { if let Some(v) = cfg.concurrency { args.concurrency = v; } }
                if args.timeout == 10 { if let Some(v) = cfg.timeout { args.timeout = v; } }
                if args.delay_ms == 0 { if let Some(v) = cfg.delay_ms { args.delay_ms = v; } }
                if args.retries == 0 { if let Some(v) = cfg.retries { args.retries = v; } }
                if args.header.is_empty() { args.header = cfg.header.unwrap_or_default(); }
                if args.exclude.is_empty() { args.exclude = cfg.exclude.unwrap_or_default(); }
            } else {
                eprintln!("rsray: could not parse config {}", cfg_path.display());
            }
        } else {
            eprintln!("rsray: could not read config {}", cfg_path.display());
        }
    }
    let start = Instant::now();
    progress(&start, args.quiet, "rsray started");

    let mut inputs: Vec<String> = Vec::new();
    if args.stdin || args.input.as_deref() == Some("-") {
        let mut buf = String::new();
        if io::stdin().read_to_string(&mut buf).is_ok() {
            for line in buf.lines() {
                let line = line.trim();
                if !line.is_empty() {
                    inputs.push(line.to_string());
                }
            }
        }
    } else if let Some(input) = &args.input {
        inputs.push(input.clone());
    } else {
        eprintln!("Usage: rsray <file.js | dir/ | https://site.com | https://site.com/app.js>");
        eprintln!("       cat js_urls.txt | rsray --stdin");
        std::process::exit(2);
    }

    // Effective scope: explicit --scope, else the host of a site input.
    let mut scope: Option<String> = args.scope.clone();
    if scope.is_none() && !args.include_external {
        for input in &inputs {
            if input.starts_with("http") && !input.ends_with(".js") && !input.ends_with(".map") {
                scope = host_of_url(input);
                break;
            }
        }
    }
    if args.include_external {
        scope = args.scope.clone();
    }

    let mut all: Vec<Endpoint> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut secret_list: Vec<String> = Vec::new();
    let mut files_scanned = 0usize;
    let mut snippet_map: HashMap<String, String> = HashMap::new();

    let http = HttpOpts {
        headers: args.header.clone(),
        cookie: args.cookie.clone(),
        proxy: args.proxy.clone(),
        timeout_secs: args.timeout,
        retries: args.retries,
    };
    for input in &inputs {
        progress(&start, args.quiet, &format!("loading {}", input));
        let sources = gather(&input, args.depth, &start, args.quiet, args.max_pages, args.concurrency, &http, args.delay_ms, &args.exclude);
        files_scanned += sources.len();
        for source in sources {
            progress(&start, args.quiet, &format!("scanning {} ({} bytes)", source.name, source.content.len()));
            let raws: Vec<RawEndpoint> = if source.name.ends_with(".map") {
                extractor::extract_from_sourcemap(&source.content)
            } else {
                extractor::extract_from_js(&source.content)
            };
            if args.secrets {
                for hit in secret_hits(&source.content) {
                    secret_list.push(format!("{} :: {}", source.name, hit));
                }
            }
            for raw in raws {
                if let Some(ep) = classifier::classify(&raw.value, &source.name, &raw.snippet, scope.as_deref()) {
                    if ep.class == Class::Static && !args.include_static {
                        continue;
                    }
                    if seen.insert(ep.endpoint.clone()) {
                        snippet_map.insert(ep.endpoint.clone(), raw.snippet.clone());
                        all.push(ep);
                    }
                }
            }
        }
    }

    all.sort_by(|a, b| a.endpoint.cmp(&b.endpoint));
    if args.interesting_only {
        all.retain(|e| !e.flags.is_empty());
    }

    // Colours only on a real terminal, never in JSON/CSV/piped output.
    let use_color = !args.no_color && !args.json && !args.csv && std::io::IsTerminal::is_terminal(&std::io::stdout());
    let paint = |code: &str, text: &str| -> String {
        if use_color {
            format!("\x1b[{}m{}\x1b[0m", code, text)
        } else {
            text.to_string()
        }
    };

    let mut out_text = String::new();
    if args.urls {
        // Full URLs ready for httpx/ffuf: relative endpoints get the
        // first site input's origin prepended.
        let origin = inputs.iter().find(|i| i.starts_with("http")).map(|i| {
            let (scheme, rest) = i.split_once("://").unwrap_or(("https", i.as_str()));
            let h = rest.split('/').next().unwrap_or("");
            format!("{}://{}", scheme, h)
        });
        for e in &all {
            if e.endpoint.starts_with("http://") || e.endpoint.starts_with("https://") || e.endpoint.starts_with("ws") {
                out_text.push_str(&e.endpoint);
            } else if let Some(o) = &origin {
                out_text.push_str(&format!("{}{}", o, e.endpoint));
            } else {
                out_text.push_str(&e.endpoint);
            }
            out_text.push('\n');
        }
    } else if args.json {
        let json_eps: Vec<JsonEndpoint> = all
            .iter()
            .map(|e| JsonEndpoint { endpoint: &e.endpoint, source_file: &e.source_file, class: e.class, flags: &e.flags, method: &e.method })
            .collect();
        out_text = serde_json::to_string_pretty(&json_eps).unwrap_or_else(|_| "[]".to_string());
        out_text.push('\n');
    } else if args.csv {
        out_text.push_str("endpoint,source_file,class,flags,method\n");
        for e in &all {
            out_text.push_str(&format!("\"{}\",\"{}\",{:?},\"{}\",{}\n", e.endpoint.replace('"', "\"\""), e.source_file.replace('"', "\"\""), e.class, e.flags.join(";"), e.method));
        }
    } else {
        // Grouped terminal output.
        let mut groups: HashMap<&str, Vec<&Endpoint>> = HashMap::new();
        for e in &all {
            let key = match e.class {
                Class::Api => "API routes",
                Class::Page => "Page routes",
                Class::Static => "Static assets",
                Class::External => "External",
                Class::Websocket => "WebSockets",
            };
            groups.entry(key).or_default().push(e);
        }
        for title in ["API routes", "Page routes", "External", "WebSockets", "Static assets"] {
            if let Some(list) = groups.get(title) {
                if list.is_empty() {
                    continue;
                }
                out_text.push_str(&format!("\n{}\n", paint("1;36", &format!("== {} ({}) ==", title, list.len()))));
                for e in list {
                    let flag_str = if e.flags.is_empty() { String::new() } else { format!(" [{}]", paint("33", &e.flags.join(","))) };
                    out_text.push_str(&format!("  {} {}{}\n", e.method, e.endpoint, flag_str));
                    if args.verbose {
                        out_text.push_str(&format!("     from {} :: {}\n", e.source_file, snippet_map.get(&e.endpoint).cloned().unwrap_or_default()));
                    }
                }
            }
        }
        if !args.quiet {
            let api = all.iter().filter(|e| e.class == Class::Api).count();
            let interesting = all.iter().filter(|e| !e.flags.is_empty()).count();
            out_text.push_str(&format!("\nScanned {} source(s). Found {} endpoints ({} API, {} flagged interesting).\nFinished in {:.2}s.\n", files_scanned, all.len(), api, interesting, start.elapsed().as_secs_f64()));
        }
    }

    if args.quiet && !args.json && !args.csv && !args.urls {
        out_text.clear();
        for e in &all {
            out_text.push_str(&e.endpoint);
            out_text.push('\n');
        }
    }

    match &args.output {
        Some(path) => {
            if let Err(e) = fs::write(path, &out_text) {
                eprintln!("rsray: could not write {}: {}", path.display(), e);
                std::process::exit(1);
            }
            if !args.quiet {
                eprintln!("rsray: wrote {} endpoints to {}", all.len(), path.display());
            }
        }
        None => print!("{}", out_text),
    }

    progress(&start, args.quiet, &format!("done: {} endpoints from {} source(s) in {:.2}s", all.len(), files_scanned, start.elapsed().as_secs_f64()));

    if let Some(wordlist_path) = &args.wordlist {
        let words = wordlist_from(&all);
        if let Err(e) = fs::write(wordlist_path, words.join("\n") + "\n") {
            eprintln!("rsray: could not write wordlist: {}", e);
        } else if !args.quiet {
            eprintln!("rsray: wrote {} wordlist entries to {}", words.len(), wordlist_path.display());
        }
    }

    if args.secrets && !secret_list.is_empty() {
        eprintln!("\nSecrets (high-confidence patterns — review manually, do not trust blindly):");
        for hit in secret_list.iter().take(50) {
            eprintln!("  {}", hit);
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn har_entries_become_source() {
        let har = r#"{"log":{"entries":[{"request":{"method":"GET","url":"https://example.com/api/v1/users"},"response":{"content":{"text":"{\"ok\":true}"}}}]}}"#;
        let out = parse_har(har).unwrap();
        assert!(out.contains("https://example.com/api/v1/users"));
    }

    #[test]
    fn next_build_id_is_parsed() {
        assert_eq!(next_build_id(r#"{"buildId":"abc123"}"#).as_deref(), Some("abc123"));
        assert_eq!(next_build_id("no build here"), None);
    }

    #[test]
    fn exclude_matches_substring() {
        assert!(excluded("https://example.com/logout", &["/logout".to_string()]));
        assert!(!excluded("https://example.com/login", &["/logout".to_string()]));
    }

    #[test]
    fn new_secret_patterns_hit() {
        let hits = secret_hits("key = \"sk_live_abcdefghijklmnop\" token = \"xoxb-123456789012-abc\"");
        assert!(hits.iter().any(|h| h.contains("stripe-live-key")));
        assert!(hits.iter().any(|h| h.contains("slack-token")));
        assert!(secret_hits("hello world").is_empty());
    }

    #[test]
    fn more_secret_patterns_hit() {
        let content = "npm_token = \"npm_abcdefghijklmnopqrstuvwxyz1234567890\" glpat = \"glpat-abcdefghijklmnopqrst\" sg = \"SG.abcdefghijklmnopqrstuv.abcdefghijklmnopqrstuvwxyz12345678901234567\" db = \"postgres://user:pass123@db.example.com/app\"";
        let hits = secret_hits(content);
        assert!(hits.iter().any(|h| h.contains("npm-token")), "hits: {:?}", hits);
        assert!(hits.iter().any(|h| h.contains("gitlab-pat")), "hits: {:?}", hits);
        assert!(hits.iter().any(|h| h.contains("sendgrid-key")), "hits: {:?}", hits);
        assert!(hits.iter().any(|h| h.contains("db-url-with-password")), "hits: {:?}", hits);
    }
}
