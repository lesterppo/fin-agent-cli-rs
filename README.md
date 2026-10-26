# fin-rs — AI-Native Financial Data CLI, in Rust

A Rust port of [lesterppo/fin-agent-cli](https://github.com/lesterppo/fin-agent-cli)
(the Python `fin.py` + `providers.py`, 15 commands). Same idea, same output
contract: token-minimal, machine-parseable output for autonomous AI agents.
No ANSI, no rich formatting, no welcome messages.

```sh
$ fin-rs q AAPL
{"s":"AAPL","p":347.73,"c":-0.14,"v":"39.5M","t":1792785601}
```

## Why Rust

- **Single binary, millisecond startup.** The Python original needs a venv
  with yfinance + pandas + typer; `fin-rs` is one statically-linked binary
  (SQLite bundled) with no runtime dependencies.
- **No pandas, no yfinance.** Data comes from plain HTTP APIs: Yahoo
  Finance chart + quoteSummary (with the same crumb flow yfinance uses),
  SEC EDGAR (`data.sec.gov`), FRED, and Tiingo (optional key).
- **Same contract.** Compact single-line JSON with 1–4 char keys, `QUOTE_MINIMAL`
  CSV with `YYYYMMDD` dates, the same error codes (`E_INV_SYM`,
  `E_RATE_LIMIT`, `E_FETCH_FAIL`, `E_KEY_MISS`), the same cache TTLs.

This is **read-only market data**. No trading, no orders.

## Install

```sh
cargo install --path .
# or
cargo build --release
cp target/release/fin-rs ~/.local/bin/
```

Requires a Rust toolchain (1.75+). No Python, no venv, no API keys for the
core commands.

### API keys (optional, same as the original)

```sh
export TIINGO_API_KEY="your_key"   # price-history waterfall (Tiingo → Yahoo)
export FRED_API_KEY="your_key"     # FRED macroeconomic data (required for `m`)
```

Without keys, all commands except `m` work. `m` without `FRED_API_KEY`
returns `{"err":"E_KEY_MISS",...}`, exactly like the original.

### Cache

SQLite cache at `./cache.db` (override with `FIN_RS_CACHE`), same schema as
the original (`source, symbol, data_type, response_json, created_at`) and the
same TTLs: quote 60s, earnings/SEC 6h, insider 12h, everything else 24h.
History caches under the `history` key regardless of the requested day count,
mirroring the original's behaviour. SEC ticker-map cache at
`./sec_tickers.json` (24h, override with `FIN_RS_SEC_TICKERS`).

## Usage

```sh
fin-rs q AAPL              # quote (JSON)
fin-rs h AAPL 30           # price history, 30 days (CSV)
fin-rs h AAPL 5            # last 5 days
fin-rs m DGS10             # FRED series (CSV, needs FRED_API_KEY)
fin-rs e AAPL              # earnings (JSON)
fin-rs f AAPL              # fundamentals (JSON)
fin-rs stmt AAPL is        # income statement (CSV); bs | cf
fin-rs info AAPL           # company identity (JSON)
fin-rs cmp AAPL MSFT GOOGL # side-by-side fundamentals (CSV)
fin-rs trend AAPL          # quarterly trends (CSV)
fin-rs div AAPL            # dividend history (CSV)
fin-rs sec AAPL            # SEC filings (CSV)
fin-rs insider AAPL        # insider transactions (CSV)
fin-rs holders AAPL        # institutional holders (CSV, partial — see below)
fin-rs anal AAPL           # analyst recommendation trends (CSV)
fin-rs health AAPL         # health metrics (JSON, partial — see below)
```

Errors are JSON on stdout with exit code 1, no tracebacks:

```sh
$ fin-rs q INVALIDXYZ
{"err":"E_INV_SYM","msg":"No data for symbol: INVALIDXYZ (...)"}
```

## Parity with the Python original

Honest status, verified live 2026-10-26 (see DEEP_TEST.md for the
back-to-back comparison). "Implemented" means the same command, same output
shape, and live data verified; "Partial" means the command runs and returns
real data but some fields/rows the original provides are missing, for the
reason stated.

| Cmd | Status | Notes |
|-----|--------|-------|
| `q` | Implemented | Chart meta (price, prev close, volume). The v7 quote endpoint now returns `Unauthorized` from datacenter IPs; chart meta is the working source. |
| `h` | Implemented | Yahoo chart (Tiingo first when `TIINGO_API_KEY` is set, same waterfall). |
| `m` | Implemented | FRED observations; `E_KEY_MISS` without a key, same as the original. |
| `e` | Implemented | Next date from `calendarEvents`, 8Q history from `earningsHistory`, surprise% from `surprisePercent`. |
| `f` | Partial | 45 of 49 keys populated from quoteSummary (`price`, `summaryDetail`, `defaultKeyStatistics`, `financialData`) plus computed `nd`/`ne`/`fy`. Missing computed keys only: `ic` (interest expense empty in 2026 histories), `rd` (R&D empty), `sb` (no SBC row), `by` (no diluted share counts). Quirk preserved from the original: `dy` rounds the fractional yield to 2dp, so AAPL shows `0.0`. |
| `stmt` | Partial | Income statement returns revenue + net income reliably; cost-of-revenue / gross-profit / operating-income rows read 0 in 2026 quoteSummary data (Yahoo returns literal zeros, not missing rows). Balance-sheet history returns period dates with no rows; cash-flow returns net income only. The original's yfinance path shows the same degradation where it uses the same endpoint — see DEEP_TEST.md. |
| `info` | Implemented | Name, sector, industry, country, employees, website, summary (240 chars). |
| `cmp` | Implemented | Same metric columns as the original; a failed symbol yields an `ERR` row, as in the original. |
| `trend` | Partial | Quarterly revenue + net income real; gross profit/operating income/EBITDA/EPS rows are absent or zero in 2026 quoteSummary quarterly data; OCF row only when quarterly cashflow exposes it. |
| `div` | Implemented | Full dividend history via chart `period1=firstTradeDate` (AAPL: 92 dividends, 1987→2026), ascending, `d,v` CSV. Note: `range=max` truncates to ~169 points unauthenticated, so periods are used instead. Symbols with no dividends return `E_INV_SYM`, as in the original. |
| `sec` | Implemented | EDGAR submissions, important forms only, max 20 rows. `www.sec.gov` rate-limits the ticker map from datacenter IPs, so a bundled fallback map of well-known large caps is used then — and every submissions response is checked to actually list the requested ticker, so a wrong map entry can never silently return another company's filings. |
| `insider` | Implemented | `insiderTransactions` module, max 25 rows, same buy/grant/sell classification. |
| `holders` | Partial | Yahoo no longer returns the named institutional-holders table via quoteSummary (the `institutionalHoldings` module is dropped from responses in 2026). Returns aggregate rows only (`institutions`, `institutionsFloat`, `insiders`, `institutionsCount`) under the original headers. Named holders are NOT faked. |
| `anal` | Implemented | `recommendationTrend` rows, same columns. |
| `health` | Partial | Revenue/employee, net income/employee, revenue + earnings 3y CAGRs, net-debt/EBITDA, and warning flags are computed. Z-Score, cash-conversion cycle, and earnings-quality metrics need a populated balance sheet / cashflow statement, which 2026 quoteSummary data does not provide — those keys are omitted rather than invented. |

Differences from the original worth knowing:

- **Cache location.** The original caches next to `fin.py`; an installed
  binary must not write next to itself, so `fin-rs` caches in the current
  directory (`./cache.db`).
- **Quote source.** `q` uses the chart endpoint's meta, not yfinance
  `fast_info` (same numbers: last price, previous close, volume).
- **Crumb flow.** `quoteSummary` needs a Yahoo crumb + cookie; `fin-rs`
  fetches both per run (same flow yfinance uses) and refreshes once on a
  401/403. If Yahoo blocks the crumb flow, `f`/`e`/`stmt`/`info`/`cmp`/
  `trend`/`insider`/`holders`/`anal`/`health` fail with `E_FETCH_FAIL`
  while `q`/`h`/`div` (chart endpoint) keep working.
- **No pandas, no yfinance.** All parsing is `serde_json` + the CSV writer
  in `src/lib.rs`.

## Development

```sh
cargo test            # unit tests: formatting, dates, CSV, error codes, cache TTL
cargo build --release # release binary at target/release/fin-rs
```

## License

MIT — same as the original. See [LICENSE](LICENSE).
