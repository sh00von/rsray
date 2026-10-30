# rsray

[![Crates.io](https://img.shields.io/crates/v/rsray.svg)](https://crates.io/crates/rsray)

Extract endpoints from JavaScript bundles and websites.

Use only on systems you own or are authorized to test.

## Installation

```bash
cargo install rsray
```

Also on crates.io: https://crates.io/crates/rsray

From source:

```bash
git clone https://github.com/sh00von/rsray
cd rsray
cargo install --path .
```

## Usage

```bash
rsray [OPTIONS] [INPUT]
```

INPUT is a JavaScript file, a directory, a JavaScript URL, or a
website URL. Use `-` or `--stdin` to read a list of inputs from stdin.

## Examples

Input:

```bash
rsray app.js
rsray ./bundles/
rsray https://example.com/app.js
rsray https://example.com
cat js_urls.txt | rsray --stdin
cat js_urls.txt | rsray -
```

Output:

```bash
rsray https://example.com --json -o endpoints.json
rsray https://example.com --csv -o endpoints.csv
rsray https://example.com -q
rsray https://example.com --urls -o urls.txt
rsray https://example.com --interesting-only
rsray https://example.com -v
rsray https://example.com --wordlist words.txt
```

Filters:

```bash
rsray https://example.com --scope example.com
rsray https://example.com --include-static
rsray https://example.com --include-external
rsray https://example.com --secrets
rsray https://example.com --exclude "/logout" --exclude "/static"
```

Authenticated scanning and proxy (authorized targets only):

```bash
rsray https://example.com --header "Authorization: Bearer TOKEN"
rsray https://example.com --header "Authorization: Bearer TOKEN" --header "X-Custom: 1"
rsray https://example.com --cookie "session=abc; other=1"
rsray https://example.com --proxy http://127.0.0.1:8080
```

HAR input:

```bash
rsray session.har --json -o endpoints.json
```

Config file:

```bash
rsray https://example.com --config rsray.toml
```

See `rsray.toml.example` for all supported keys. CLI flags
override config values.

Crawling (website input only):

```bash
rsray https://example.com --max-pages 0
rsray https://example.com --max-pages 50
rsray https://example.com --depth 1
rsray https://example.com --concurrency 32
```

Piping:

```bash
rsray https://example.com --urls | httpx -silent
rsray ./bundles/ -q | httpx -silent
rsray https://example.com --wordlist words.txt
ffuf -u https://example.com/FUZZ -w words.txt
```

## Options

| Option | Description |
| --- | --- |
| `-o, --output <file>` | Write results to a file instead of stdout |
| `--json` | JSON output |
| `--csv` | CSV output |
| `-q, --quiet` | Endpoints only, no summary |
| `-v, --verbose` | Show source file and code snippet per endpoint |
| `--include-static` | Include static asset paths (images, CSS, fonts) |
| `--include-external` | Include endpoints on external domains (site mode only) |
| `--secrets` | Report high-confidence secret patterns (off by default, printed to stderr) |
| `--wordlist <file>` | Write a wordlist of path segments to a file |
| `--scope <domain>` | Only report endpoints for this domain |
| `--stdin` | Read inputs, one per line, from stdin |
| `--depth <n>` | Chunk-follow depth (default: 5) |
| `--max-pages <n>` | Internal pages to crawl (default: 20, 0 = homepage only) |
| `--concurrency <n>` | Parallel script fetches (default: 16) |
| `--header <h>` | Extra request header, repeatable |
| `--cookie <v>` | Cookie header value |
| `--proxy <url>` | HTTP(S) proxy, e.g. for Burp |
| `--timeout <s>` | Per-request timeout in seconds (default: 10) |
| `--delay-ms <n>` | Delay between crawled page fetches (default: 0) |
| `--retries <n>` | Retries on network errors and 5xx (default: 0) |
| `--exclude <s>` | Skip URLs containing this substring, repeatable |
| `--config <file>` | TOML config file |
| `--interesting-only` | Only endpoints flagged admin, internal, debug, staging, test, beta, backup, old, versioned, or parameterised |
| `--urls` | Full URLs only, one per line |
| `--no-color` | Disable terminal colours |
| `-h, --help` | Print help (`--h` and `-help` work the same) |
| `-V, --version` | Print version |

## Output

```text
== API routes (5) ==
  GET /api/v1/users
  POST /api/v1/admin/users [admin,versioned]
  POST /api/v1/internal/debug [internal,debug,versioned]
  GET /api/v1/users/{userId}/orders [parameterised,versioned]
  GET /api/v2/orders [versioned]

== Page routes (1) ==
  GET /products

== External (1) ==
  GET https://api.example.com/products

== WebSockets (1) ==
  GET wss://example.com/socket

Scanned 1 source(s). Found 8 endpoints (5 API, 5 flagged interesting).
Finished in 0.01s.
```

Progress and timing go to stderr, so stdout stays clean for piping.
Colours are used only on a terminal, never in JSON, CSV, quiet, or
piped output.

## Notes

Site mode crawls internal pages, sitemap.xml, and robots.txt,
fetches scripts in parallel, follows chunk references recursively,
fetches source maps, and fetches `_next/data` JSON for Next.js
pages when a buildId is found.

Extraction combines a string tokenizer, an SWC parse, and a
language-aware regex pass. It handles string concatenation,
`const` base URLs, template literals (`` `/api/users/${id}` ``
becomes `/api/users/{id}`), and code in Python, Go, PHP, Ruby,
Java, and JSON. `/api`, `/rest`, and `/graphql` paths are
classified as API routes.

Directory mode scans `.js`, `.mjs`, `.cjs`, `.ts`, `.tsx`,
`.jsx`, `.py`, `.go`, `.php`, `.rb`, `.java`, `.json`,
`.html`, and `.htm` files.

## Development

Requires a stable Rust toolchain. No other setup is needed.

```bash
git clone https://github.com/sh00von/rsray
cd rsray
cargo test
cargo build --release
```

Pull requests are welcome. Run `cargo test` before submitting one.

## License

MIT
