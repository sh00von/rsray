# Benchmarks (measured 2026-10-30, VM, not his Windows machine)

These are rsray's own numbers, not a comparison claim. LinkFinder/Katana
comparison is still to do on the same targets from the same machine.

## shovon.bd, site crawl

Command: `rsray https://shovon.bd --json -o shovon.json`

- Before crawl (homepage only, v0.1.2 era): 36 endpoints, 14 sources, 2.49s
  (measured on his Windows machine, 2026-10-30)
- After crawl + parallel scripts (this version): 125 endpoints,
  86 sources, 12.65s (VM, 2026-10-30)
- The extra endpoints come from crawling up to 20 internal pages,
  not from new JS tricks. Most of the 12.65s is sequential page
  fetching; script fetching itself is parallel (16 concurrent).

## Fixture

`rsray tests/fixtures/sample.js`: 8 endpoints, 0.00s, 13 tests pass.

## Notes

- Juice Shop from the VM could not be fetched (Heroku blocks this
  network), so no Juice Shop benchmark is claimed here. On his
  Windows machine Juice Shop main.js gave real /api and /rest routes
  in earlier runs.
- testphp.vulnweb.com was down at check time, and demo.testfire.net
  has almost no JavaScript, so neither is a useful benchmark target.
