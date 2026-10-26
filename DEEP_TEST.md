# Deep test: fin-rs vs Python fin-agent-cli, back-to-back

**Date:** 2026-10-26 (HKT). **Method:** same inputs run through both
binaries minutes apart, fresh caches on both sides, no API keys set
(`TIINGO_API_KEY` / `FRED_API_KEY` unset, so `m` tests the error path).
Python side: the original `fin.py` + `providers.py` from
`lesterppo/fin-agent-cli`, run in a venv (typer + yfinance + pandas +
requests) from a scratch copy — the original repo was not modified.
Rust side: `target/release/fin-rs` built from this repo.

**Verdict: PASS with documented gaps.** 9 commands byte-identical or
value-identical, 2 commands where Rust succeeds and Python fails from this
VM, 4 commands Partial for one shared root cause (see §4). No Rust output
was faked: every gap below is a missing/zero row, not an invented value.

## 1. Per-command results

| Case | Rust | Python | Verdict |
|------|------|--------|---------|
| `q AAPL` | `{"s":"AAPL","p":347.73,"c":-0.14,"v":"39.5M"}` | identical except `t` (calls 2–4 s apart; `t` is wall-clock on both) | ✅ identical modulo `t` |
| `q MSFT` | `p:524.51, c:0.66, v:13.7M` | identical except `t` | ✅ identical modulo `t` |
| `q 0700.HK` (non-US) | `p:386.0, c:0.0, v:33.2M` | identical except `t` | ✅ identical modulo `t` |
| `q INVALIDXYZ` | `{"err":"E_INV_SYM","msg":"Symbol not found or delisted"}` exit 1 | byte-identical, exit 1 | ✅ identical |
| `h AAPL 5` | 5 rows, 20261019–20261023 | 5 rows, same values | ✅ **byte-identical** (after CRLF + float-format fixes) |
| `h MSFT 30` | 30 rows, 20260914–20261023 | 30 rows, same values | ✅ **byte-identical** (after range fix; see §3) |
| `h 0700.HK 5` | 6 rows (Yahoo returns 6 rows in the 5d window for HK) | same 6 rows | ✅ identical rows |
| `m DGS10` (no key) | `{"err":"E_KEY_MISS",...}` exit 1 | byte-identical, exit 1 | ✅ identical |
| `sec AAPL` | 20 rows, first `20260731,10-Q,000032019326000020,0000320193` | `{"err":"E_RATE_LIMIT","msg":"SEC rate limit on ticker map (try again later)"}` exit 1 | ⚠️ Rust succeeds, Python fails (§2) |
| `sec MSFT` | 20 rows, first `20261021,DEF 14A,000119312526427849,0000789019` | same `E_RATE_LIMIT` failure | ⚠️ Rust succeeds, Python fails (§2) |
| `sec INVALIDXYZ` | `E_RATE_LIMIT` (not in fallback map; ticker map throttled) | byte-identical `E_RATE_LIMIT` | ✅ identical |
| `e AAPL` | `nd:2026-11-02, ne:2.02, ns:6.74`, 4 quarters | `E_FETCH_FAIL`: yfinance curl timeout after 30000 ms | ⚠️ Rust succeeds, Python fails (§2) |
| `e MSFT` | `nd:2026-10-28, ne:4.74, ns:11.81`, 4 quarters | same curl-timeout failure | ⚠️ Rust succeeds, Python fails (§2) |
| `f AAPL` | 45 keys | 49 keys | ⚠️ Partial — see §4 |
| `f MSFT` | 45 keys | 49 keys | ⚠️ Partial — see §4 |
| `stmt AAPL is` | `r` + `ni` real; `cor/gp/oi` read `0` | full: `cor 220.96B, gp 195.2B, oi 133.05B, ebitda 144.75B, eps 7.46` (FY2025) | ❌ Partial — see §4 |
| `stmt AAPL bs` | header only, no rows | 8 rows (`ta 359.24B, tl 285.51B, …`) | ❌ Partial — see §4 |
| `stmt AAPL cf` | `ni` only | 7 rows (`ocf 111.48B, capex -12.71B, fcf 98.77B, …`) | ❌ Partial — see §4 |
| `info AAPL` / `info MSFT` | all 9 keys | all 9 keys, same values | ✅ identical |
| `cmp AAPL MSFT` | 2 rows | 2 rows | ✅ **byte-identical** |
| `trend AAPL` | 4 quarters, `r` + `ni` real, `gp/oi` zero | 6 quarters, full rows incl. `gp 54.77B, ebitda 39.02B, eps 2.02, ocf 34.37B` (20260630) | ❌ Partial — see §4 |
| `trend MSFT` | 4 quarters, `r` + `ni` real | 6 quarters, full rows | ❌ Partial — see §4 |
| `div AAPL` | 92 dividends, 19870511–20260810 | 92 rows, same values | ✅ **byte-identical** |
| `div MSFT` | 91 dividends | 91 rows, same values | ✅ **byte-identical** |
| `insider AAPL` | 25 rows, first `20261020,NEWSTEAD JENNIFER,Officer,2.4K,814.75K,sell` | 25 rows | ✅ **byte-identical** |
| `holders AAPL` | 4 aggregate rows (`institutions 66.3, …`) | 10 named holders (`Blackrock Inc. 8.0, 1.16B, 404.4B, 1.6`, …) | ❌ Partial — see §4 |
| `anal AAPL` | `0m,6,18,14,3,3` … 4 rows | 4 rows | ✅ **byte-identical** |
| `health AAPL` | `re 2.77M, ne 746.73K, r3 1.8, e3 3.9, nd 0.1` | + `z 12.93, dso 34.9, dio 9.4, dpo 115.4, ccc -71.1, oq 1.0, fq 0.88, ac 0.1, cx -3.1, sb 3.1, nd 0.4` | ⚠️ Partial — see §4 |
| `health MSFT` | same 5 keys | + Z-score/CCC/quality keys, `fl` flags | ⚠️ Partial — see §4 |

## 2. Cases where Rust succeeds and Python fails (from this VM)

These are genuine Python-side failures, not test rigging — both binaries
ran from the same VM minutes apart:

- **SEC ticker map throttling.** `www.sec.gov/files/company_tickers.json`
  returns "Request Rate Threshold Exceeded" for this VM's datacenter egress
  IP. The Python original surfaces this as `E_RATE_LIMIT` and stops. The
  Rust port falls back to a bundled map of well-known large caps, then
  **verifies the submissions response actually lists the requested ticker**,
  so a wrong map entry can never silently return another company's filings.
  A symbol outside the fallback map (e.g. `INVALIDXYZ`) gets the same
  `E_RATE_LIMIT` as Python — verified.
- **Earnings scrape timeout.** Python `e` failed for both AAPL and MSFT
  with yfinance's curl timeout (30 s, 0 bytes). Rust `e` uses the
  quoteSummary `calendarEvents` + `earningsHistory` modules and returned
  in seconds. Note the Rust history has 4 quarters, not the original's 8 —
  that is all `earningsHistory` returns.

## 3. Bugs the deep test caught and fixed

All four were fixed in the Rust code and re-verified before this report:

1. **History range semantics.** Yahoo chart `range=Nd` counts *trading*
   days, not calendar days (verified: `range=30d` → exactly 30 rows,
   MSFT 20260914–20261023). The first Rust build bucketed 30 days to `1mo`
   (22 rows vs Python's 30). Fix: pass `{days}d` verbatim, like the
   original's `period=f"{days}d"`. After fix: `h MSFT 30` byte-identical.
2. **CSV float formatting.** Rust printed `346.00` / `0.0010`; Python's
   `round()` + `str()` prints `346.0` / `0.001`. Fix: Python-style float
   rendering (trim zeros, keep ≥1 decimal). After fix: byte-identical.
3. **CSV line endings.** Python's `csv.writer` defaults to `\r\n`; Rust
   wrote `\n`. The values looked identical in terminal output but `cmp`
   failed. Fix: Rust CSV now emits `\r\n` everywhere (including statement
   and trend renderers). After fix: byte-identical.
4. **Dividend-yield units.** Yahoo `summaryDetail.dividendYield` is a
   fraction (0.0031); yfinance `info` gives percent (0.31). Rust now
   converts fraction → percent before rounding: AAPL `dy 0.31`, MSFT
   `dy 0.75`, both matching Python exactly.

## 4. Remaining gaps — root cause and why they are not faked

**Root cause (one cause, four commands): yfinance's statement/holder
data does not come from the public quoteSummary endpoint.** The Rust port
reads statements from quoteSummary histories, where in 2026 Yahoo returns
literal zeros for cost-of-revenue / gross-profit / operating-income,
empty objects for R&D / EBIT / interest, period dates only for balance
sheets, and net income only for cashflows. The `ws/fundamentals/v1`
endpoint that used to serve full financials returns `Not Found` from this
VM, and the named-holders table comes from yfinance's HTML scrape of the
Yahoo holders page, not from any quoteSummary module (the
`institutionalHoldings` module is dropped from quoteSummary responses in
2026). So:

- `stmt` (is/bs/cf), `trend`, and the statement-dependent parts of `f`
  and `health` cannot be closed from quoteSummary alone. What Rust does
  instead: emit exactly what Yahoo returns (including its zeros) and omit
  rows/keys with no data. It never synthesises a value.
- `f` specifics: 41/45 shared keys value-identical for AAPL (42nd key
  `ps` is `10.87` in Rust vs `null` in Python — Rust has *more* data;
  MSFT `bv` differs by 0.01 on a rounding boundary, 59.57 vs 59.56).
  Divergent computed keys: `nd`/`ne`/`fy` — Python computes them from
  balance-sheet/cashflow statements (AAPL `nd 62.72B` = debt 98.66B −
  cash 35.93B; `fy 1.95` from statement FCF 98.77B), Rust from
  `financialData` totals (`nd 21.94B` = 84.34B − 62.4B; `fy 2.12` from
  107.72B). Python-only computed keys `by`/`rd`/`sb` (AAPL `rd 8.3`,
  `sb 3.1`, `by 2.6`) need statement rows Rust cannot see; `ic` is
  `null` in Python for AAPL too.
- `health` specifics: shared keys (`re`, `ne`, `r3`, `e3`) identical;
  Z-Score, CCC, earnings quality, capex ratio, SBC ratio, and MSFT's `fl`
  flags need the missing statements and are omitted, not estimated.
- `holders`: Rust returns aggregate rows only (`institutions 66.3`,
  `institutionsFloat 67.5`, `insiders 1.6`, `institutionsCount 7707`)
  under the original headers. Named holders are NOT faked.
- `e` history depth: 4 quarters vs the original's 8 (module limit).

## 5. Error cases

| Case | Rust | Python |
|------|------|--------|
| Bad symbol (`q INVALIDXYZ`) | `E_INV_SYM`, exit 1 | `E_INV_SYM`, exit 1, byte-identical message |
| SEC bad symbol (`sec INVALIDXYZ`) | `E_RATE_LIMIT` (ticker map throttled, not in fallback) | identical `E_RATE_LIMIT` |
| Missing argument (`q` with no symbol) | clap usage error, exit 2 | typer usage error, exit 2 (message style differs, code same) |
| Missing key (`m DGS10`, no `FRED_API_KEY`) | `E_KEY_MISS`, exit 1 | identical `E_KEY_MISS`, exit 1 |

## 6. Performance (timed, not claimed)

Cached `q AAPL` (cache hit on both sides, 5 runs each, min taken):

| | Rust `fin-rs` | Python `fin.py` |
|---|---|---|
| Min wall time | **0.009 s** | **1.807 s** |
| All runs | 0.009–0.017 s (first run 2.033 s, cold disk/proxy warm-up) | 1.807–3.094 s (first run 5.032 s) |
| Binary / install | 8,141,288-byte single binary, no runtime deps | venv with yfinance + pandas + typer + requests |

That is ~200× faster on cached-quote startup, measured — the win is
Python's import time (pandas/yfinance), not network: both hit the same
cache. Uncached network calls are dominated by Yahoo/SEC latency on both
sides and were not benchmarked as a speed claim.

## 7. Cache behaviour

Second identical calls return the same data on both sides (verified for
`q` — identical output including `t` within the 60 s TTL — and for `f`
and `h`, which returned their first-run outputs verbatim from cache).
TTL values match the original: quote 60 s, earnings/SEC 6 h, insider
12 h, all others 24 h. One mirrored quirk: history caches under the
`history` key regardless of the requested day count, exactly like the
original.

## 8. Reproduction

```sh
cargo build --release
# Rust side (fresh cache):
FIN_RS_CACHE=/tmp/rs-cache.db ./target/release/fin-rs q AAPL
# Python side (venv with typer, yfinance, pandas, requests):
python fin.py q AAPL
```

Note: `/tmp` on this VM is wiped between sessions, so the Python venv and
the first deep-test run's scratch files did not survive; the Python side
was re-cloned from `lesterppo/fin-agent-cli` and reinstalled for the
final byte-compare (`h`, `div`, `insider`, `anal`, `cmp` byte-identical,
`q` identical modulo `t`). All figures above come from the actual paired
runs, not from the README's earlier estimates.
