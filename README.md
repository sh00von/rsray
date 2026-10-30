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
rsray --h      # same as --help
rsray -help    # same as --help
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

## Quick start

```bash
# 1. Install
cargo install rsray

# 2. Scan one file
rsray app.js

# 3. Scan a whole site and save the results
rsray https://example.com --json -o endpoints.json
```

## Examples by input type

```bash
# 1. One local JS file
rsray app.js
rsray ./dist/main.js --json -o endpoints.json

# 2. A whole directory (scans .js/.mjs/.cjs/.ts/.tsx/.jsx/.py/.go/
#    .php/.rb/.java/.json/.html files inside it)
rsray ./bundles/
rsray ./src --json -o endpoints.json

# 3. One remote JS URL
rsray https://example.com/app.js
rsray https://example.com/app.js --json -o endpoints.json

# 4. A whole website: crawls internal pages, sitemap.xml,
#    robots.txt, source maps and chunk references, then extracts
rsray https://example.com
rsray https://example.com --json -o endpoints.json

# 5. Many inputs from stdin, one per line (files, URLs, or sites)
cat js_urls.txt | rsray --stdin
cat js_urls.txt | rsray --stdin --json -o endpoints.json

# 6. The same stdin list, using "-" as the input
cat js_urls.txt | rsray -
echo "https://example.com/app.js" | rsray --stdin
```

## Examples by output type

```bash
# Grouped terminal output (default): API / Page / External /
# WebSocket / Static, with flags like [admin,versioned]
rsray https://example.com

# JSON output, to a file or to stdout
rsray https://example.com/app.js --json -o endpoints.json
rsray app.js --json

# CSV output (endpoint,source_file,class,flags,method)
rsray https://example.com --csv -o endpoints.csv

# Endpoints only, one per line, no summary — for piping
rsray ./bundles/ -q
rsray https://example.com -q | httpx -silent

# Full URLs only, one per line — ready for httpx / ffuf
rsray https://example.com --urls -o urls.txt
rsray https://example.com --urls | httpx -silent

# Only flagged endpoints (admin, internal, debug, staging,
# test, beta, backup, old, versioned, parameterised)
rsray https://example.com --interesting-only
rsray https://example.com --interesting-only --json -o interesting.json

# Show source file and code snippet for every endpoint
rsray app.js -v
rsray https://example.com -v --json -o endpoints.json

# Also write a wordlist (path segments) from the endpoints found
rsray https://example.com --wordlist words.txt
rsray https://example.com --wordlist words.txt --urls -o urls.txt
```

## Examples by filtering

```bash
# Only endpoints for one domain
rsray https://example.com --scope example.com
rsray ./bundles/ --scope example.com -q

# Also include static assets (images, CSS, fonts) — off by default
rsray https://example.com --include-static

# Also include endpoints on external domains — off by default,
# site mode only
rsray https://example.com --include-external

# Also report high-confidence secret patterns (off by default;
# printed to stderr, review manually, do not trust blindly)
rsray https://example.com --secrets
```

## Examples by crawl tuning

```bash
# Homepage only, no page crawling
rsray https://example.com --max-pages 0

# Crawl up to 50 internal pages instead of the default 20
rsray https://example.com --max-pages 50

# Follow chunk references only 1 level deep (default 5)
rsray https://example.com --depth 1

# Fetch scripts 32 at a time instead of the default 16
rsray https://example.com --concurrency 32

# Slow / large site: fewer pages, fewer parallel fetches
rsray https://example.com --max-pages 5 --concurrency 4

# Disable colours even on a terminal (colours are already off
# automatically in JSON, CSV, quiet and piped output)
rsray https://example.com --no-color
```

## Hunter workflow, step by step

```bash
# Step 1: Crawl the target and save everything
rsray https://example.com --json -o endpoints.json

# Step 2: Look at the interesting routes first
rsray https://example.com --interesting-only

# Step 3: Build a URL list and probe which ones are live
rsray https://example.com --urls -o urls.txt
httpx -l urls.txt -silent -status-code

# Step 4: Build a wordlist for fuzzing from the paths found
rsray https://example.com --wordlist words.txt
ffuf -u https://example.com/FUZZ -w words.txt

# Step 5: Check for secrets separately, and review every hit by hand
rsray https://example.com --secrets
```

Manually review every discovered endpoint before doing anything with it.
rsray only reads public JavaScript — it does not exploit, brute-force,
or bypass authentication.

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
3. **Extract** — string literals are tokenised, literals joined by `+` are re-joined, `const API = "..."` base URLs are resolved, and `` `/api/users/${id}` `` becomes `/api/users/{id}`. A real SWC parse is a second pass, so exact string and template literals are caught even in minified or nested code; if SWC cannot parse a fragment, the tokenizer pass still stands. A language-aware regex pass also catches endpoints in Python, Go, PHP, Ruby, Java and JSON code. Multi-byte text (for example Bangla) is handled without panics.
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
