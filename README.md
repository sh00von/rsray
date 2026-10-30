# rsray

**X-ray into JavaScript bundles. Find the hidden API surface.**

rsray is a fast Rust CLI that extracts API endpoints hidden inside JavaScript bundles — routes that never appear in navigation, sitemaps, or public API docs. It understands string concatenation, template literals, and simple base-URL constants, so it finds more than a plain regex scan, and it classifies results so the interesting routes stand out.

> Only run rsray on sites you own or targets you are explicitly authorised to test.

## Install

```bash
cargo install rsray
# or, from a local checkout:
cargo install --path .
```

## Help

```bash
rsray --help   # full help with examples
rsray -h       # short help
rsray --version
```

Like sqlmap, progress and timing go to stderr, so stdout stays clean for piping:

```
[+0.00s] rsray started
[+0.01s] loading tests/fixtures/sample.js
[+0.01s] scanning tests/fixtures/sample.js (536 bytes)
[+0.01s] done: 8 endpoints from 1 source(s) in 0.01s
```

The final summary also prints `Finished in X.XXs`.

## Usage

```bash
# One local bundle
rsray app.js

# One remote bundle, JSON output
rsray https://example.com/app.js --json -o endpoints.json

# A whole website: crawl up to 20 internal pages, sitemap, robots.txt,
# source maps and chunk references, then extract
rsray https://example.com --wordlist words.txt -o endpoints.txt

# Hunter output: full URLs only, or flagged endpoints only
rsray https://example.com --urls -o urls.txt
rsray https://example.com --interesting-only

# Many JS URLs, one per line
cat js_urls.txt | rsray --stdin

# Endpoints only, for piping into other tools
rsray ./bundles/ --scope example.com -q | httpx -silent
```

## Options

| Flag | Meaning |
| --- | --- |
| `-o, --output <file>` | Write results to a file |
| `--json` / `--csv` | Structured output |
| `-q, --quiet` | Endpoints only, no summary |
| `-v, --verbose` | Show source file and code snippet per endpoint |
| `--include-static` | Also list static assets (images, CSS, fonts) |
| `--include-external` | Also report endpoints on external domains |
| `--secrets` | Also report high-confidence secret patterns (off by default) |
| `--wordlist <file>` | Also write a target wordlist |
| `--scope <domain>` | Only report endpoints for this domain |
| `--stdin` | Read inputs from stdin |
| `--depth <n>` | Chunk-follow depth when crawling (default 5) |
| `--max-pages <n>` | Internal pages to crawl (default 20, 0 = homepage only) |
| `--concurrency <n>` | Parallel script fetches (default 16) |
| `--interesting-only` | Only flagged endpoints (admin, internal, debug, etc) |
| `--urls` | Full URLs only, ready for httpx/ffuf |
| `--no-color` | Disable terminal colours |

## How it works

1. **Input** — a file, a directory, a JS URL, a website, or a list from stdin.
2. **Fetch** — site mode crawls internal pages, sitemap.xml and robots.txt, fetches scripts in parallel, follows chunk references recursively, and fetches source maps.
3. **Extract** — string literals are tokenised, literals joined by `+` are re-joined, `const API = "..."` base URLs are resolved, and `` `/api/users/${id}` `` becomes `/api/users/{id}`. A language-aware regex pass also catches endpoints in Python, Go, PHP, Ruby, Java and JSON code. Multi-byte text (for example Bangla) is handled without panics.
4. **Classify** — API routes (`/api`, `/rest`, `/graphql`, `/v1`..), page routes, static assets, external hosts, and WebSockets, with flags for `admin`, `internal`, `debug`, `staging`, versioned APIs, and parameterised routes. Colours appear only on a real terminal, never in JSON, CSV or piped output.
5. **Output** — grouped terminal output, JSON, CSV, or plain text, plus an optional wordlist.

## Example output

```
== API routes (3) ==
  GET /api/v1/users
  POST /api/v1/admin/users [admin,versioned]
  GET /api/v2/orders/{id} [parameterised,versioned]

Scanned 4 source(s). Found 5 endpoints (3 API, 2 flagged interesting).
```

## Testing

```bash
cargo test
```

Unit tests cover concatenation, const base URLs, template literals, WebSockets, classification, and false-positive filtering. A `tests/fixtures/` sample bundle is included for manual checks.

## Comparison

| Tool | Language | Parsing |
| --- | --- | --- |
| LinkFinder | Python | Regex |
| xnLinkFinder | Python | Regex, improved |
| Katana | Go | Crawler + JS parsing |
| **rsray** | **Rust** | **String resolution + classification, single binary** |

## License

MIT
