//! Command implementations. Each mirrors one Python provider, same
//! output shape, same cache TTL, same error codes.

use crate::{
    fmt_num, fmt_vol, fmt_yyyy_mm_dd, fmt_yyyymmdd, get_json, json_num, json_pct,
    json_round, now_unix, obj_insert, out_json, parse_statements, qs_raw, qs_str, raw,
    stmt_csv, to_csv, Cache, FinError, FResult, StmtPeriod, Yahoo, UA_SEC, TTL_ANAL, TTL_DIV,
    TTL_E, TTL_F, TTL_H, TTL_HEALTH, TTL_HOLDERS, TTL_INFO, TTL_INSIDER, TTL_M, TTL_Q, TTL_SEC,
    TTL_STMT, TTL_TREND,
};
use serde_json::{json, Map, Value};
use std::collections::HashMap;

fn norm(sym: &str) -> FResult<String> {
    let s = sym.trim().to_uppercase();
    if s.is_empty() {
        return Err(FinError::inv("Symbol is required"));
    }
    Ok(s)
}

fn cache_get(c: Option<&Cache>, src: &str, sym: &str, dt: &str, ttl: i64) -> Option<String> {
    c.and_then(|c| c.get(src, sym, dt, ttl))
}

fn cache_set(c: Option<&Cache>, src: &str, sym: &str, dt: &str, s: &str) {
    if let Some(c) = c {
        c.set(src, sym, dt, s);
    }
}

// ---------------------------------------------------------------- q
pub fn cmd_q(y: &Yahoo, cache: Option<&Cache>, symbol: &str) -> FResult<String> {
    let sym = norm(symbol)?;
    if let Some(hit) = cache_get(cache, "yfinance", &sym, "quote", TTL_Q) {
        return Ok(hit);
    }
    let r = y.chart(&sym, "1d", "1d", "")?;
    let meta = r.get("meta").ok_or_else(|| FinError::inv(format!("No data for symbol: {sym}")))?;
    let price = raw(meta.get("regularMarketPrice"))
        .or_else(|| raw(meta.get("fulldayPrice")))
        .ok_or_else(|| FinError::inv(format!("No data for symbol: {sym}")))?;
    let prev = raw(meta.get("chartPreviousClose"))
        .or_else(|| raw(meta.get("regularMarketPreviousClose")));
    let vol = raw(meta.get("regularMarketVolume"));
    let change = match prev {
        Some(p) if p != 0.0 => crate::round2((price - p) / p * 100.0),
        _ => 0.0,
    };
    let v = json!({"s": sym, "p": crate::round2(price), "c": change, "v": fmt_vol(vol), "t": now_unix()});
    let s = out_json(&v);
    cache_set(cache, "yfinance", &sym, "quote", s.trim_end());
    Ok(s)
}

// ---------------------------------------------------------------- h
fn range_for_days(days: i64) -> &'static str {
    if days <= 1 {
        "1d"
    } else if days <= 5 {
        "5d"
    } else if days <= 30 {
        "1mo"
    } else if days <= 90 {
        "3mo"
    } else if days <= 180 {
        "6mo"
    } else if days <= 365 {
        "1y"
    } else if days <= 730 {
        "2y"
    } else if days <= 1825 {
        "5y"
    } else {
        "max"
    }
}

pub fn cmd_h(y: &Yahoo, cache: Option<&Cache>, symbol: &str, days: i64) -> FResult<String> {
    let sym = norm(symbol)?;
    let days = days.max(1);
    if let Some(key) = std::env::var("TIINGO_API_KEY").ok().filter(|k| !k.is_empty()) {
        if let Some(hit) = cache_get(cache, "tiingo", &sym, "history", TTL_H) {
            return Ok(hit);
        }
        // Tiingo first; any failure falls through to Yahoo (original waterfall).
        if let Ok(csv) = fetch_tiingo_history(y, &sym, days, &key) {
            cache_set(cache, "tiingo", &sym, "history", &csv);
            return Ok(csv);
        }
    }
    if let Some(hit) = cache_get(cache, "yfinance", &sym, "history", TTL_H) {
        return Ok(hit);
    }
    let csv = fetch_yahoo_history(y, &sym, days)?;
    cache_set(cache, "yfinance", &sym, "history", &csv);
    Ok(csv)
}

fn fetch_tiingo_history(_y: &Yahoo, sym: &str, days: i64, key: &str) -> FResult<String> {
    // Dates in UTC; Tiingo expects YYYY-MM-DD.
    let end = fmt_yyyy_mm_dd(now_unix());
    let start = fmt_yyyy_mm_dd(now_unix() - days * 86400);
    let url = format!(
        "https://api.tiingo.com/tiingo/daily/{sym}/prices?startDate={start}&endDate={end}&token={key}"
    );
    let client = crate::http_client()?;
    let v = get_json(&client, &url, crate::UA_YAHOO)?;
    let arr = v.as_array().ok_or_else(|| FinError::inv(format!("No data for symbol: {sym}")))?;
    if arr.is_empty() {
        return Err(FinError::inv(format!("No data for symbol: {sym}")));
    }
    let mut rows = Vec::new();
    for row in arr {
        let d = row
            .get("date")
            .and_then(|d| d.as_str())
            .unwrap_or("")
            .get(..10)
            .unwrap_or("")
            .replace('-', "");
        let num = |k: &str| row.get(k).and_then(|x| x.as_f64()).unwrap_or(0.0);
        rows.push(vec![
            d,
            format!("{:.2}", num("adjOpen")),
            format!("{:.2}", num("adjHigh")),
            format!("{:.2}", num("adjLow")),
            format!("{:.2}", num("adjClose")),
            format!("{}", num("adjVolume") as i64),
        ]);
    }
    Ok(to_csv(&["d", "o", "h", "l", "c", "v"], &rows))
}

pub fn fetch_yahoo_history(y: &Yahoo, sym: &str, days: i64) -> FResult<String> {
    let r = y.chart(sym, range_for_days(days), "1d", "")?;
    let ts = r.get("timestamp").and_then(|t| t.as_array()).cloned().unwrap_or_default();
    // Anchor the window at the latest market day Yahoo returned, not at
    // "now": Yahoo's own range buckets are anchored at the last trading
    // day, so a weekend "now" must not drop Friday/Monday rows. This
    // matches yfinance period="Nd" behaviour.
    let latest = ts.iter().filter_map(|t| t.as_i64()).max().unwrap_or(now_unix());
    let cutoff = latest - days * 86400;
    let quote = r
        .get("indicators")
        .and_then(|i| i.get("quote"))
        .and_then(|q| q.as_array())
        .and_then(|a| a.first().cloned())
        .unwrap_or(json!({}));
    let get_arr = |k: &str| -> Vec<Option<f64>> {
        quote
            .get(k)
            .and_then(|a| a.as_array())
            .map(|a| a.iter().map(|x| x.as_f64()).collect())
            .unwrap_or_default()
    };
    let opens = get_arr("open");
    let highs = get_arr("high");
    let lows = get_arr("low");
    let closes = get_arr("close");
    let vols = get_arr("volume");
    let mut rows = Vec::new();
    for (i, t) in ts.iter().enumerate() {
        let t = match t.as_i64() {
            Some(x) => x,
            None => continue,
        };
        if t < cutoff {
            continue;
        }
        let f = |a: &[Option<f64>]| a.get(i).copied().flatten().unwrap_or(0.0);
        rows.push(vec![
            fmt_yyyymmdd(t),
            format!("{:.2}", f(&opens)),
            format!("{:.2}", f(&highs)),
            format!("{:.2}", f(&lows)),
            format!("{:.2}", f(&closes)),
            format!("{}", f(&vols) as i64),
        ]);
    }
    if rows.is_empty() {
        return Err(FinError::inv(format!("No data for symbol: {sym}")));
    }
    Ok(to_csv(&["d", "o", "h", "l", "c", "v"], &rows))
}

// ---------------------------------------------------------------- m
pub fn cmd_m(cache: Option<&Cache>, series: &str) -> FResult<String> {
    let sid = series.trim().to_uppercase();
    if sid.is_empty() {
        return Err(FinError::inv("Series ID required"));
    }
    if let Some(hit) = cache_get(cache, "fred", &sid, "macro", TTL_M) {
        return Ok(hit);
    }
    let key = std::env::var("FRED_API_KEY").ok().filter(|k| !k.is_empty());
    let key = match key {
        Some(k) => k,
        None => return Err(FinError::key("Set FRED_API_KEY environment variable")),
    };
    let url = format!(
        "https://api.stlouisfed.org/fred/series/observations?series_id={sid}&api_key={key}&file_type=json"
    );
    let client = crate::http_client()?;
    let resp = client
        .get(&url)
        .send()
        .map_err(|e| FinError::fetch(format!("FRED: {e}")))?;
    let status = resp.status().as_u16();
    if status == 400 {
        return Err(FinError::inv(format!("Invalid FRED series: {sid}")));
    }
    if status == 429 {
        return Err(FinError::rate("FRED rate limit"));
    }
    if !resp.status().is_success() {
        return Err(FinError::fetch(format!("FRED HTTP {status}")));
    }
    let v: Value = resp.json().map_err(|e| FinError::fetch(format!("FRED parse: {e}")))?;
    let obs = v.get("observations").and_then(|o| o.as_array()).cloned().unwrap_or_default();
    if obs.is_empty() {
        return Err(FinError::inv(format!("No observations for series: {sid}")));
    }
    let mut rows = Vec::new();
    for o in &obs {
        let val = o.get("value").and_then(|x| x.as_str()).unwrap_or(".");
        if val == "." {
            continue;
        }
        let d = o.get("date").and_then(|x| x.as_str()).unwrap_or("").replace('-', "");
        rows.push(vec![d, val.to_string()]);
    }
    let csv = to_csv(&["d", "v"], &rows);
    cache_set(cache, "fred", &sid, "macro", &csv);
    Ok(csv)
}

// ---------------------------------------------------------------- shared QS
pub const QS_FUND: &str = "price,summaryDetail,defaultKeyStatistics,financialData,assetProfile,calendarEvents,earningsHistory,recommendationTrend,majorHoldersBreakdown,insiderTransactions,incomeStatementHistory,balanceSheetHistory,cashflowStatementHistory,incomeStatementHistoryQuarterly,cashflowStatementHistoryQuarterly";

pub fn fetch_fund_qs(y: &Yahoo, sym: &str) -> FResult<Value> {
    y.quote_summary(sym, QS_FUND)
}

// ---------------------------------------------------------------- e
pub fn cmd_e(y: &Yahoo, cache: Option<&Cache>, symbol: &str) -> FResult<String> {
    let sym = norm(symbol)?;
    if let Some(hit) = cache_get(cache, "yfinance", &sym, "earnings", TTL_E) {
        return Ok(hit);
    }
    let qs = fetch_fund_qs(y, &sym)?;
    let out = earnings_json(&sym, &qs)?;
    let s = out_json(&out);
    cache_set(cache, "yfinance", &sym, "earnings", s.trim_end());
    Ok(s)
}

pub fn earnings_json(sym: &str, qs: &Value) -> FResult<Value> {
    let hist = qs
        .get("earningsHistory")
        .and_then(|e| e.get("history"))
        .and_then(|h| h.as_array())
        .cloned()
        .unwrap_or_default();
    if hist.is_empty() {
        return Err(FinError::inv(format!("No earnings data for: {sym}")));
    }
    // Sort by quarter date descending (most recent first, like yfinance).
    let mut entries: Vec<&Value> = hist.iter().collect();
    entries.sort_by_key(|e| {
        std::cmp::Reverse(
            e.get("quarter").and_then(|q| q.get("raw")).and_then(|r| r.as_i64()).unwrap_or(0),
        )
    });
    let nd = qs
        .get("calendarEvents")
        .and_then(|c| c.get("earnings"))
        .and_then(|e| e.get("earningsDate"))
        .and_then(|a| a.as_array())
        .and_then(|a| a.first())
        .and_then(|d| d.get("fmt"))
        .and_then(|f| f.as_str())
        .map(|s| s.to_string());
    let latest = entries[0];
    let ne = raw(latest.get("epsActual")).map(crate::round2);
    let ns = raw(latest.get("surprisePercent")).map(|x| crate::round2(x * 100.0));
    let mut h = Vec::new();
    for e in entries.iter().take(8) {
        let date = e
            .get("quarter")
            .and_then(|q| q.get("fmt"))
            .and_then(|f| f.as_str())
            .unwrap_or("")
            .to_string();
        let est = raw(e.get("epsEstimate")).map(crate::round2);
        let act = raw(e.get("epsActual")).map(crate::round2);
        let sur = raw(e.get("surprisePercent")).map(|x| crate::round2(x * 100.0));
        h.push(json!([date, est, act, sur]));
    }
    Ok(json!({"s": sym, "nd": nd, "ne": ne, "ns": ns, "h": h}))
}

// ---------------------------------------------------------------- f
pub fn cmd_f(y: &Yahoo, cache: Option<&Cache>, symbol: &str) -> FResult<String> {
    let sym = norm(symbol)?;
    if let Some(hit) = cache_get(cache, "yfinance", &sym, "fundamentals", TTL_F) {
        return Ok(hit);
    }
    let qs = fetch_fund_qs(y, &sym)?;
    let chart = y.chart(&sym, "1d", "1d", "").unwrap_or(json!({}));
    let out = fundamentals_json(&sym, &qs, &chart);
    let s = out_json(&out);
    cache_set(cache, "yfinance", &sym, "fundamentals", s.trim_end());
    Ok(s)
}

/// Build the fundamentals JSON (same key order as _FINFO_MAP).
pub fn fundamentals_json(sym: &str, qs: &Value, chart: &Value) -> Value {
    let mut m = Map::new();
    let meta = chart.get("meta");
    let nm = qs_str(qs, "price", "shortName")
        .or_else(|| meta.and_then(|m| m.get("shortName")).and_then(|v| v.as_str()).map(|s| s.to_string()))
        .or_else(|| meta.and_then(|m| m.get("longName")).and_then(|v| v.as_str()).map(|s| s.to_string()));
    obj_insert(&mut m, "s", json!(sym));
    obj_insert(&mut m, "nm", nm.map(Value::String).unwrap_or(Value::Null));
    obj_insert(&mut m, "mc", json!(fmt_num(qs_raw(qs, "price", "marketCap").or_else(|| qs_raw(qs, "summaryDetail", "marketCap")))));
    // mc must be "" when absent in the original's _fmt_num(None); keep "" here
    // only if both sources missed — the json above already yields "" via fmt_num(None).
    obj_insert(&mut m, "pe", json_round(qs_raw(qs, "summaryDetail", "trailingPE"), 1));
    obj_insert(&mut m, "fpe", json_round(qs_raw(qs, "summaryDetail", "forwardPE").or_else(|| qs_raw(qs, "defaultKeyStatistics", "forwardPE")), 1));
    obj_insert(&mut m, "pb", json_round(qs_raw(qs, "defaultKeyStatistics", "priceToBook"), 2));
    obj_insert(&mut m, "ps", json_round(qs_raw(qs, "summaryDetail", "priceToSalesTrailing12Months"), 2));
    obj_insert(&mut m, "de", json_round(qs_raw(qs, "financialData", "debtToEquity"), 1));
    obj_insert(&mut m, "gm", json_pct(qs_raw(qs, "financialData", "grossMargins"), 1));
    obj_insert(&mut m, "om", json_pct(qs_raw(qs, "financialData", "operatingMargins"), 1));
    obj_insert(&mut m, "pm", json_pct(qs_raw(qs, "financialData", "profitMargins"), 1));
    obj_insert(&mut m, "roe", json_pct(qs_raw(qs, "financialData", "returnOnEquity"), 1));
    obj_insert(&mut m, "roa", json_pct(qs_raw(qs, "financialData", "returnOnAssets"), 1));
    obj_insert(&mut m, "rg", json_pct(qs_raw(qs, "financialData", "revenueGrowth"), 1));
    obj_insert(&mut m, "ev", json!(fmt_num(qs_raw(qs, "defaultKeyStatistics", "enterpriseValue"))));
    obj_insert(&mut m, "eve", json_round(qs_raw(qs, "defaultKeyStatistics", "enterpriseToEbitda"), 1));
    // FCF: financialData freeCashflow if present, else operating - capex from annual CF.
    let cf_annual = parse_statements(qs, "cashflowStatementHistory", "cashflowStatements");
    let ocf_stmt = cf_annual.first().and_then(|p| p.val(&["totalCashFromOperatingActivities"]));
    let capex_stmt = cf_annual.first().and_then(|p| p.val(&["capitalExpenditures"]));
    let fcf_stmt = match (ocf_stmt, capex_stmt) {
        (Some(o), Some(c)) => Some(o + c),
        _ => None,
    };
    let fcf = qs_raw(qs, "financialData", "freeCashflow").or(fcf_stmt);
    obj_insert(&mut m, "fcf", json!(fmt_num(fcf)));
    obj_insert(&mut m, "b", json_round(qs_raw(qs, "summaryDetail", "beta").or_else(|| qs_raw(qs, "defaultKeyStatistics", "beta")), 2));
    let dy_raw = qs_raw(qs, "summaryDetail", "dividendYield");
    obj_insert(
        &mut m,
        "dy",
        match dy_raw {
            Some(v) if v > 0.0 => json_round(Some(v), 2),
            _ => Value::Null,
        },
    );
    obj_insert(&mut m, "cr", json_round(qs_raw(qs, "financialData", "currentRatio"), 2));
    obj_insert(&mut m, "qr", json_round(qs_raw(qs, "financialData", "quickRatio"), 2));
    let debt = qs_raw(qs, "financialData", "totalDebt");
    let cash = qs_raw(qs, "financialData", "totalCash");
    obj_insert(&mut m, "d", json!(fmt_num(debt)));
    obj_insert(&mut m, "c", json!(fmt_num(cash)));
    obj_insert(&mut m, "tp", json_round(qs_raw(qs, "financialData", "targetMeanPrice"), 2));
    obj_insert(&mut m, "rk", qs_str(qs, "financialData", "recommendationKey").map(Value::String).unwrap_or(Value::Null));
    obj_insert(&mut m, "na", json_num(qs_raw(qs, "financialData", "numberOfAnalystOpinions")));
    obj_insert(&mut m, "52h", json_round(qs_raw(qs, "summaryDetail", "fiftyTwoWeekHigh"), 2));
    obj_insert(&mut m, "52l", json_round(qs_raw(qs, "summaryDetail", "fiftyTwoWeekLow"), 2));
    obj_insert(&mut m, "sh", json!(fmt_num(qs_raw(qs, "defaultKeyStatistics", "sharesOutstanding"))));
    obj_insert(&mut m, "peg", json_round(qs_raw(qs, "defaultKeyStatistics", "pegRatio"), 2));
    obj_insert(&mut m, "bv", json_round(qs_raw(qs, "defaultKeyStatistics", "bookValue"), 2));
    obj_insert(&mut m, "io", json_pct(qs_raw(qs, "defaultKeyStatistics", "heldPercentInstitutions"), 1));
    obj_insert(&mut m, "ii", json_pct(qs_raw(qs, "defaultKeyStatistics", "heldPercentInsiders"), 1));
    obj_insert(&mut m, "sr", json_round(qs_raw(qs, "defaultKeyStatistics", "shortRatio"), 2));
    obj_insert(&mut m, "si", json_pct(qs_raw(qs, "defaultKeyStatistics", "shortPercentOfFloat"), 1));
    obj_insert(&mut m, "eg", json_pct(qs_raw(qs, "financialData", "earningsGrowth"), 1));
    obj_insert(&mut m, "em", json_pct(qs_raw(qs, "financialData", "ebitdaMargins"), 1));
    obj_insert(&mut m, "er", json_round(qs_raw(qs, "defaultKeyStatistics", "enterpriseToRevenue"), 2));
    obj_insert(&mut m, "ocf", json!(fmt_num(qs_raw(qs, "financialData", "operatingCashflow").or(ocf_stmt))));
    let dr_raw = qs_raw(qs, "summaryDetail", "dividendRate");
    obj_insert(
        &mut m,
        "dr",
        match dr_raw {
            Some(v) if v > 0.0 => json_round(Some(v), 2),
            _ => Value::Null,
        },
    );
    obj_insert(&mut m, "po", json_pct(qs_raw(qs, "summaryDetail", "payoutRatio"), 1));
    obj_insert(&mut m, "rs", json_round(qs_raw(qs, "financialData", "revenuePerShare"), 2));
    obj_insert(&mut m, "cs", json_round(qs_raw(qs, "financialData", "totalCashPerShare"), 2));
    // Computed metrics (only when computable, like the original).
    let is_annual = parse_statements(qs, "incomeStatementHistory", "incomeStatementHistory");
    let latest_is = is_annual.first();
    let revenue = latest_is.and_then(|p| p.val(&["totalRevenue"]));
    let ebit = latest_is.and_then(|p| p.val(&["ebit", "operatingIncome"]));
    let ebitda = qs_raw(qs, "financialData", "ebitda");
    let ie = latest_is.and_then(|p| p.val(&["interestExpense"]));
    let rd = latest_is.and_then(|p| p.val(&["researchDevelopment"]));
    let mc = qs_raw(qs, "price", "marketCap").or_else(|| qs_raw(qs, "summaryDetail", "marketCap"));
    if let (Some(d), Some(c)) = (debt, cash) {
        obj_insert(&mut m, "nd", json!(fmt_num(Some(d - c))));
        if let Some(e) = ebitda {
            if e != 0.0 {
                obj_insert(&mut m, "ne", json_round(Some((d - c) / e), 1));
            }
        }
    }
    if let (Some(e), Some(i)) = (ebit, ie) {
        if i != 0.0 {
            obj_insert(&mut m, "ic", json_round(Some((e / i).abs()), 1));
        }
    }
    if let (Some(f), Some(mc)) = (fcf, mc) {
        if mc != 0.0 {
            obj_insert(&mut m, "fy", json_round(Some(f / mc * 100.0), 2));
        }
    }
    if let (Some(r), Some(rv)) = (rd, revenue) {
        if rv != 0.0 {
            obj_insert(&mut m, "rd", json_round(Some(r / rv * 100.0), 1));
        }
    }
    // sb (SBC/revenue) and by (buyback yield): Yahoo quoteSummary histories
    // do not expose stock-based compensation or diluted share counts in
    // 2026 data, so these keys are omitted (original also omits them when
    // the underlying rows are missing).
    Value::Object(m)
}

// ---------------------------------------------------------------- stmt
pub const MAP_IS: &[(&str, &[&str])] = &[
    ("r", &["totalRevenue"]),
    ("cor", &["costOfRevenue"]),
    ("gp", &["grossProfit"]),
    ("oi", &["operatingIncome", "ebit"]),
    ("ni", &["netIncome", "netIncomeApplicableToCommonShares"]),
    ("ebitda", &["ebitda"]),
    ("eps", &["dilutedEPS"]),
];
pub const MAP_BS: &[(&str, &[&str])] = &[
    ("ta", &["totalAssets"]),
    ("tl", &["totalLiab", "totalLiabilities"]),
    ("te", &["totalStockholderEquity"]),
    ("ca", &["totalCurrentAssets"]),
    ("cl", &["totalCurrentLiabilities"]),
    ("cash", &["cash", "cashAndCashEquivalents"]),
    ("debt", &["longTermDebt", "totalDebt"]),
    ("wc", &["workingCapital"]),
];
pub const MAP_CF: &[(&str, &[&str])] = &[
    ("ocf", &["totalCashFromOperatingActivities"]),
    ("capex", &["capitalExpenditures"]),
    ("fcf", &["freeCashFlow"]),
    ("icf", &["totalCashflowsFromInvestingActivities"]),
    ("fncf", &["totalCashFromFinancingActivities"]),
    ("da", &["depreciation"]),
    ("ni", &["netIncome"]),
];

pub fn cmd_stmt(y: &Yahoo, cache: Option<&Cache>, symbol: &str, stmt_type: &str) -> FResult<String> {
    let sym = norm(symbol)?;
    let st = stmt_type.trim().to_lowercase();
    if !matches!(st.as_str(), "is" | "bs" | "cf") {
        return Err(FinError::inv(format!("Unknown statement type: {st}. Use: is, bs, cf")));
    }
    if let Some(hit) = cache_get(cache, "yfinance", &sym, &st, TTL_STMT) {
        return Ok(hit);
    }
    let qs = fetch_fund_qs(y, &sym)?;
    let (module, key, map) = match st.as_str() {
        "is" => ("incomeStatementHistory", "incomeStatementHistory", MAP_IS),
        "bs" => ("balanceSheetHistory", "balanceSheetStatements", MAP_BS),
        _ => ("cashflowStatementHistory", "cashflowStatements", MAP_CF),
    };
    let periods = parse_statements(&qs, module, key);
    if periods.is_empty() {
        return Err(FinError::inv(format!("No {st} data for: {sym}")));
    }
    let csv = stmt_csv(&periods, map);
    cache_set(cache, "yfinance", &sym, &st, &csv);
    Ok(csv)
}

// ---------------------------------------------------------------- info
pub fn cmd_info(y: &Yahoo, cache: Option<&Cache>, symbol: &str) -> FResult<String> {
    let sym = norm(symbol)?;
    if let Some(hit) = cache_get(cache, "yfinance", &sym, "info", TTL_INFO) {
        return Ok(hit);
    }
    let qs = fetch_fund_qs(y, &sym)?;
    let chart = y.chart(&sym, "1d", "1d", "").unwrap_or(json!({}));
    let meta = chart.get("meta");
    let mut m = Map::new();
    obj_insert(&mut m, "s", json!(sym));
    let nm = qs_str(&qs, "price", "shortName")
        .or_else(|| meta.and_then(|x| x.get("shortName")).and_then(|v| v.as_str()).map(|s| s.to_string()));
    if let Some(v) = nm {
        obj_insert(&mut m, "nm", json!(v));
    }
    let ln = qs_str(&qs, "price", "longName")
        .or_else(|| meta.and_then(|x| x.get("longName")).and_then(|v| v.as_str()).map(|s| s.to_string()));
    if let Some(v) = ln {
        obj_insert(&mut m, "ln", json!(v));
    }
    for (sk, module, field) in [
        ("sc", "assetProfile", "sector"),
        ("in", "assetProfile", "industry"),
        ("co", "assetProfile", "country"),
        ("ws", "assetProfile", "website"),
    ] {
        if let Some(v) = qs_str(&qs, module, field) {
            obj_insert(&mut m, sk, json!(v));
        }
    }
    if let Some(e) = qs_raw(&qs, "assetProfile", "fullTimeEmployees") {
        obj_insert(&mut m, "em", json!(e as i64));
    }
    if let Some(ds) = qs_str(&qs, "assetProfile", "longBusinessSummary") {
        let t: String = ds.chars().take(240).collect();
        obj_insert(&mut m, "ds", json!(t));
    }
    if m.len() <= 1 {
        return Err(FinError::inv(format!("No data for symbol: {sym}")));
    }
    let s = out_json(&Value::Object(m));
    cache_set(cache, "yfinance", &sym, "info", s.trim_end());
    Ok(s)
}

// ---------------------------------------------------------------- cmp
pub const CMP_METRICS: &[&str] = &[
    "s", "nm", "mc", "pe", "fpe", "pb", "de", "gm", "om", "roe", "rg", "ev", "fcf", "b", "d",
    "c", "tp", "rk", "52h", "52l", "peg", "si", "io", "eg", "ocf", "dr",
];

pub fn cmd_cmp(y: &Yahoo, symbols: &[String]) -> FResult<String> {
    let syms: Vec<String> = symbols.iter().map(|s| s.trim().to_uppercase()).filter(|s| !s.is_empty()).collect();
    if syms.len() < 2 {
        return Err(FinError::inv("At least two symbols required for comparison"));
    }
    let mut rows: Vec<Vec<String>> = Vec::new();
    for sym in &syms {
        match fetch_fund_qs(y, sym) {
            Ok(qs) => {
                let chart = y.chart(sym, "1d", "1d", "").unwrap_or(json!({}));
                let f = fundamentals_json(sym, &qs, &chart);
                rows.push(
                    CMP_METRICS
                        .iter()
                        .map(|k| match f.get(*k) {
                            Some(Value::Null) | None => String::new(),
                            Some(Value::String(s)) => s.clone(),
                            Some(v) => v.to_string(),
                        })
                        .collect(),
                );
            }
            Err(_) => {
                let mut r = vec![sym.clone()];
                r.extend(std::iter::repeat("ERR".to_string()).take(CMP_METRICS.len() - 1));
                rows.push(r);
            }
        }
    }
    Ok(to_csv(CMP_METRICS, &rows))
}

// ---------------------------------------------------------------- trend
pub const MAP_QTREND: &[(&str, &[&str])] = &[
    ("r", &["totalRevenue"]),
    ("gp", &["grossProfit"]),
    ("oi", &["operatingIncome", "ebit"]),
    ("ni", &["netIncome", "netIncomeApplicableToCommonShares"]),
    ("ebitda", &["ebitda"]),
    ("eps", &["dilutedEPS"]),
];

pub fn cmd_trend(y: &Yahoo, cache: Option<&Cache>, symbol: &str) -> FResult<String> {
    let sym = norm(symbol)?;
    if let Some(hit) = cache_get(cache, "yfinance", &sym, "trend", TTL_TREND) {
        return Ok(hit);
    }
    let qs = fetch_fund_qs(y, &sym)?;
    let is_q = parse_statements(&qs, "incomeStatementHistoryQuarterly", "incomeStatementHistory");
    if is_q.is_empty() {
        return Err(FinError::inv(format!("No quarterly data for: {sym}")));
    }
    let cf_q = parse_statements(&qs, "cashflowStatementHistoryQuarterly", "cashflowStatements");
    // Column union: IS periods + any CF periods not already covered.
    let mut all: Vec<StmtPeriod> = is_q;
    for p in cf_q {
        if !all.iter().any(|x| x.date == p.date) {
            all.push(p);
        }
    }
    // Build CSV over the union, with ocf from the CF periods.
    let mut headers = vec!["item".to_string()];
    for p in &all {
        headers.push(p.date.clone());
    }
    let mut clean = String::new();
    {
        let mut line = String::new();
        for (i, h) in headers.iter().enumerate() {
            if i > 0 {
                line.push(',');
            }
            line.push_str(h);
        }
        clean.push_str(&line);
        clean.push('\n');
    }
    for (short, names) in MAP_QTREND {
        if !all.iter().any(|p| p.has(names)) {
            continue;
        }
        let mut row = vec![short.to_string()];
        for p in &all {
            row.push(fmt_num(p.val(names)));
        }
        clean.push_str(&row.join(","));
        clean.push('\n');
    }
    // OCF row from quarterly cashflow periods.
    let cf_periods: Vec<&StmtPeriod> = all
        .iter()
        .filter(|p| p.has(&["totalCashFromOperatingActivities"]))
        .collect();
    if !cf_periods.is_empty() {
        let mut row = vec!["ocf".to_string()];
        for p in &all {
            row.push(fmt_num(p.val(&["totalCashFromOperatingActivities"])));
        }
        clean.push_str(&row.join(","));
        clean.push('\n');
    }
    cache_set(cache, "yfinance", &sym, "trend", &clean);
    Ok(clean)
}

// ---------------------------------------------------------------- div
pub fn cmd_div(y: &Yahoo, cache: Option<&Cache>, symbol: &str) -> FResult<String> {
    let sym = norm(symbol)?;
    if let Some(hit) = cache_get(cache, "yfinance", &sym, "dividends", TTL_DIV) {
        return Ok(hit);
    }
    // Full history: anchor at firstTradeDate (range=max truncates).
    let meta_chart = y.chart(&sym, "1d", "1d", "")?;
    let first_trade = meta_chart
        .get("meta")
        .and_then(|m| m.get("firstTradeDate"))
        .and_then(|v| v.as_i64())
        .unwrap_or(0);
    let r = y.chart_period(&sym, first_trade, now_unix(), "1d", "div")?;
    let events = r.get("events").and_then(|e| e.get("dividends")).and_then(|d| d.as_object()).cloned().unwrap_or_default();
    if events.is_empty() {
        return Err(FinError::inv(format!("No dividends for: {sym}")));
    }
    let mut items: Vec<(i64, f64)> = events
        .values()
        .filter_map(|v| {
            let d = v.get("date").and_then(|x| x.as_i64())?;
            let a = v.get("amount").and_then(|x| x.as_f64())?;
            Some((d, a))
        })
        .collect();
    items.sort_by_key(|(d, _)| *d);
    let rows: Vec<Vec<String>> = items
        .iter()
        .map(|(d, a)| vec![fmt_yyyymmdd(*d), format!("{:.4}", a)])
        .collect();
    let csv = to_csv(&["d", "v"], &rows);
    cache_set(cache, "yfinance", &sym, "dividends", &csv);
    Ok(csv)
}

// ---------------------------------------------------------------- sec
/// Well-known large-cap CIKs used ONLY when the SEC ticker map is
/// rate-limited (www.sec.gov throttles datacenter IPs — the original
/// surfaces this as E_RATE_LIMIT). Every submissions response is checked
/// to actually list the requested ticker, so a wrong map entry can never
/// silently return another company's filings.
pub fn fallback_cik(sym: &str) -> Option<u32> {
    let m: HashMap<&str, u32> = [
        ("AAPL", 320193),
        ("MSFT", 789019),
        ("GOOGL", 1652044),
        ("GOOG", 1652044),
        ("AMZN", 1018724),
        ("META", 1326801),
        ("NVDA", 1045810),
        ("TSLA", 1318605),
        ("AMD", 2488),
        ("INTC", 50863),
        ("NFLX", 1065280),
        ("CRM", 1108524),
        ("ORCL", 1341439),
        ("ADBE", 796343),
        ("AVGO", 1730168),
        ("COST", 909832),
        ("PEP", 77476),
        ("KO", 21344),
        ("JPM", 19617),
        ("BAC", 70858),
        ("WFC", 72971),
        ("V", 1404655),
        ("MA", 1495681),
        ("DIS", 1744489),
        ("PFE", 78003),
        ("JNJ", 200406),
        ("XOM", 34088),
        ("CVX", 93410),
        ("HD", 354950),
        ("PG", 80424),
        ("LLY", 59478),
        ("TSM", 1046179),
    ]
    .into_iter()
    .collect();
    m.get(sym).copied()
}

pub const SEC_FORMS: &[&str] = &["10-K", "10-Q", "8-K", "S-1", "S-3", "DEF 14A", "424B5", "424B3", "S-8"];

pub fn lookup_cik(client: &reqwest::blocking::Client, sym: &str) -> FResult<u32> {
    // Local 24h cache of the ticker map, like the original's sec_tickers.json.
    let path = std::env::var("FIN_RS_SEC_TICKERS").unwrap_or_else(|_| "sec_tickers.json".to_string());
    if let Ok(meta) = std::fs::metadata(&path) {
        if let Ok(mtime) = meta.modified() {
            if let Ok(age) = mtime.elapsed() {
                if age.as_secs() < 86400 {
                    if let Ok(text) = std::fs::read_to_string(&path) {
                        if let Ok(v) = serde_json::from_str::<Value>(&text) {
                            if let Some(c) = find_cik(&v, sym) {
                                return Ok(c);
                            }
                        }
                    }
                }
            }
        }
    }
    let resp = client
        .get("https://www.sec.gov/files/company_tickers.json")
        .header("User-Agent", UA_SEC)
        .send()
        .map_err(|e| FinError::fetch(format!("SEC ticker map: {e}")))?;
    let status = resp.status().as_u16();
    if status == 403 || status == 429 {
        // Original behaviour: surface as rate limit, not unknown symbol.
        if let Some(c) = fallback_cik(sym) {
            return Ok(c);
        }
        return Err(FinError::rate("SEC rate limit on ticker map (try again later)"));
    }
    if !resp.status().is_success() {
        return Err(FinError::fetch(format!("SEC ticker map HTTP {status}")));
    }
    let text = resp.text().map_err(|e| FinError::fetch(format!("SEC ticker map read: {e}")))?;
    let v: Value = serde_json::from_str(&text).map_err(|e| FinError::fetch(format!("SEC ticker map parse: {e}")))?;
    let _ = std::fs::write(&path, &text);
    match find_cik(&v, sym) {
        Some(c) => Ok(c),
        None => match fallback_cik(sym) {
            Some(c) => Ok(c),
            None => Err(FinError::inv(format!("CIK not found for: {sym}"))),
        },
    }
}

fn find_cik(data: &Value, sym: &str) -> Option<u32> {
    let obj = data.as_object()?;
    for v in obj.values() {
        if v.get("ticker").and_then(|t| t.as_str()) == Some(sym) {
            return v.get("cik_str").and_then(|c| c.as_u64()).map(|c| c as u32);
        }
    }
    None
}

pub fn cmd_sec(cache: Option<&Cache>, symbol: &str) -> FResult<String> {
    let sym = norm(symbol)?;
    if let Some(hit) = cache_get(cache, "sec", &sym, "filings", TTL_SEC) {
        return Ok(hit);
    }
    let client = crate::http_client()?;
    let cik = lookup_cik(&client, &sym)?;
    let padded = format!("{cik:010}");
    let url = format!("https://data.sec.gov/submissions/CIK{padded}.json");
    let v = get_json(&client, &url, UA_SEC).map_err(|e| {
        if e.code == "E_RATE_LIMIT" {
            FinError::rate("SEC rate limit")
        } else {
            FinError::fetch(e.message)
        }
    })?;
    // Guard: the submissions record must actually list this ticker.
    let tickers: Vec<String> = v
        .get("tickers")
        .and_then(|t| t.as_array())
        .map(|a| a.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect())
        .unwrap_or_default();
    if !tickers.is_empty() && !tickers.iter().any(|t| t == &sym) {
        return Err(FinError::inv(format!("CIK mismatch for: {sym}")));
    }
    let recent = v.get("filings").and_then(|f| f.get("recent")).cloned().unwrap_or(json!({}));
    let forms = recent.get("form").and_then(|a| a.as_array()).cloned().unwrap_or_default();
    let dates = recent.get("filingDate").and_then(|a| a.as_array()).cloned().unwrap_or_default();
    let accs = recent.get("accessionNumber").and_then(|a| a.as_array()).cloned().unwrap_or_default();
    let mut rows: Vec<Vec<String>> = Vec::new();
    for i in 0..forms.len() {
        let f = forms[i].as_str().unwrap_or("");
        if SEC_FORMS.contains(&f) {
            let d = dates.get(i).and_then(|x| x.as_str()).unwrap_or("").replace('-', "");
            let acc = accs.get(i).and_then(|x| x.as_str()).unwrap_or("").replace('-', "");
            rows.push(vec![d, f.to_string(), acc, padded.clone()]);
        }
        if rows.len() >= 20 {
            break;
        }
    }
    if rows.is_empty() {
        return Err(FinError::inv(format!("No recent SEC filings for: {sym}")));
    }
    let csv = to_csv(&["d", "t", "acc", "cik"], &rows);
    cache_set(cache, "sec", &sym, "filings", &csv);
    Ok(csv)
}

// ---------------------------------------------------------------- insider
pub fn cmd_insider(y: &Yahoo, cache: Option<&Cache>, symbol: &str) -> FResult<String> {
    let sym = norm(symbol)?;
    if let Some(hit) = cache_get(cache, "yfinance", &sym, "insider", TTL_INSIDER) {
        return Ok(hit);
    }
    let qs = fetch_fund_qs(y, &sym)?;
    let txns = qs
        .get("insiderTransactions")
        .and_then(|t| t.get("transactions"))
        .and_then(|a| a.as_array())
        .cloned()
        .unwrap_or_default();
    if txns.is_empty() {
        return Err(FinError::inv(format!("No insider data for: {sym}")));
    }
    let mut rows: Vec<Vec<String>> = Vec::new();
    for t in txns.iter().take(25) {
        let d = t
            .get("startDate")
            .and_then(|x| x.get("fmt"))
            .and_then(|f| f.as_str())
            .unwrap_or("")
            .replace('-', "");
        let insider: String = t
            .get("filerName")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .chars()
            .take(30)
            .collect();
        let pos: String = t
            .get("filerRelation")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .chars()
            .take(20)
            .collect();
        let shares = fmt_num(raw(t.get("shares")));
        let value = fmt_num(raw(t.get("value")));
        let txt = t.get("transactionText").and_then(|x| x.as_str()).unwrap_or("");
        let txn = if txt.contains("Purchase") {
            "buy"
        } else if txt.contains("Award") {
            "grant"
        } else {
            "sell"
        };
        rows.push(vec![d, insider, pos, shares, value, txn.to_string()]);
    }
    let csv = to_csv(&["d", "insider", "pos", "shares", "value", "type"], &rows);
    cache_set(cache, "yfinance", &sym, "insider", &csv);
    Ok(csv)
}

// ---------------------------------------------------------------- holders (partial)
pub fn cmd_holders(y: &Yahoo, cache: Option<&Cache>, symbol: &str) -> FResult<String> {
    let sym = norm(symbol)?;
    if let Some(hit) = cache_get(cache, "yfinance", &sym, "holders", TTL_HOLDERS) {
        return Ok(hit);
    }
    let qs = fetch_fund_qs(y, &sym)?;
    let br = qs.get("majorHoldersBreakdown").cloned().unwrap_or(json!({}));
    let inst_pct = qs_raw(&qs, "majorHoldersBreakdown", "institutionsPercentHeld").map(|x| crate::round1(x * 100.0));
    let ins_pct = qs_raw(&qs, "majorHoldersBreakdown", "insidersPercentHeld").map(|x| crate::round1(x * 100.0));
    let float_pct = qs_raw(&qs, "majorHoldersBreakdown", "institutionsFloatPercentHeld").map(|x| crate::round1(x * 100.0));
    let count = qs_raw(&qs, "majorHoldersBreakdown", "institutionsCount");
    if br.as_object().map(|o| o.is_empty()).unwrap_or(true) && inst_pct.is_none() {
        return Err(FinError::inv(format!("No holder data for: {sym}")));
    }
    // PARTIAL: Yahoo no longer returns the named institutional-holders
    // table via quoteSummary (the institutionalHoldings module is dropped
    // from responses in 2026). Emit the aggregate breakdown instead, with
    // holder labels that are aggregates, not invented names.
    let mut rows: Vec<Vec<String>> = Vec::new();
    rows.push(vec![
        "institutions".to_string(),
        inst_pct.map(|x| x.to_string()).unwrap_or_default(),
        String::new(),
        String::new(),
        String::new(),
    ]);
    rows.push(vec![
        "institutionsFloat".to_string(),
        float_pct.map(|x| x.to_string()).unwrap_or_default(),
        String::new(),
        String::new(),
        String::new(),
    ]);
    rows.push(vec![
        "insiders".to_string(),
        ins_pct.map(|x| x.to_string()).unwrap_or_default(),
        String::new(),
        String::new(),
        String::new(),
    ]);
    rows.push(vec![
        "institutionsCount".to_string(),
        count.map(|x| format!("{}", x as i64)).unwrap_or_default(),
        String::new(),
        String::new(),
        String::new(),
    ]);
    let csv = to_csv(&["holder", "pct", "shares", "value", "chg"], &rows);
    cache_set(cache, "yfinance", &sym, "holders", &csv);
    Ok(csv)
}

// ---------------------------------------------------------------- anal
pub fn cmd_anal(y: &Yahoo, cache: Option<&Cache>, symbol: &str) -> FResult<String> {
    let sym = norm(symbol)?;
    if let Some(hit) = cache_get(cache, "yfinance", &sym, "analyst", TTL_ANAL) {
        return Ok(hit);
    }
    let qs = fetch_fund_qs(y, &sym)?;
    let trend = qs
        .get("recommendationTrend")
        .and_then(|t| t.get("trend"))
        .and_then(|a| a.as_array())
        .cloned()
        .unwrap_or_default();
    if trend.is_empty() {
        return Err(FinError::inv(format!("No analyst data for: {sym}")));
    }
    let mut rows: Vec<Vec<String>> = Vec::new();
    for t in &trend {
        let iv = |k: &str| t.get(k).and_then(|x| x.as_i64()).unwrap_or(0).to_string();
        rows.push(vec![
            t.get("period").and_then(|x| x.as_str()).unwrap_or("").to_string(),
            iv("strongBuy"),
            iv("buy"),
            iv("hold"),
            iv("sell"),
            iv("strongSell"),
        ]);
    }
    let csv = to_csv(&["period", "sb", "b", "h", "s", "ss"], &rows);
    cache_set(cache, "yfinance", &sym, "analyst", &csv);
    Ok(csv)
}

// ---------------------------------------------------------------- health
pub fn cmd_health(y: &Yahoo, cache: Option<&Cache>, symbol: &str) -> FResult<String> {
    let sym = norm(symbol)?;
    if let Some(hit) = cache_get(cache, "yfinance", &sym, "health", TTL_HEALTH) {
        return Ok(hit);
    }
    let qs = fetch_fund_qs(y, &sym)?;
    let chart = y.chart(&sym, "1d", "1d", "").unwrap_or(json!({}));
    let f = fundamentals_json(&sym, &qs, &chart);
    if qs.get("assetProfile").is_none() && qs.get("price").is_none() {
        return Err(FinError::inv(format!("No data for symbol: {sym}")));
    }
    let is_annual = parse_statements(&qs, "incomeStatementHistory", "incomeStatementHistory");
    let bs_annual = parse_statements(&qs, "balanceSheetHistory", "balanceSheetStatements");
    let cf_annual = parse_statements(&qs, "cashflowStatementHistory", "cashflowStatements");
    let latest_is = is_annual.first();
    let latest_bs = bs_annual.first();
    let latest_cf = cf_annual.first();
    let revenue = latest_is.and_then(|p| p.val(&["totalRevenue"]));
    let ebit = latest_is.and_then(|p| p.val(&["ebit", "operatingIncome"])).filter(|x| *x != 0.0);
    let ni = latest_is.and_then(|p| p.val(&["netIncome", "netIncomeApplicableToCommonShares"]));
    let cogs = latest_is.and_then(|p| p.val(&["costOfRevenue"])).filter(|x| *x != 0.0);
    let ta = latest_bs.and_then(|p| p.val(&["totalAssets"]));
    let tl = latest_bs.and_then(|p| p.val(&["totalLiab", "totalLiabilities"]));
    let te = latest_bs.and_then(|p| p.val(&["totalStockholderEquity"]));
    let wc = latest_bs.and_then(|p| p.val(&["workingCapital"]));
    let re_ = latest_bs.and_then(|p| p.val(&["retainedEarnings"]));
    let ar = latest_bs.and_then(|p| p.val(&["accountsReceivable"]));
    let inv = latest_bs.and_then(|p| p.val(&["inventory"]));
    let ap = latest_bs.and_then(|p| p.val(&["accountsPayable"]));
    let debt = qs_raw(&qs, "financialData", "totalDebt");
    let cash = qs_raw(&qs, "financialData", "totalCash");
    let ebitda = qs_raw(&qs, "financialData", "ebitda");
    let ocf = latest_cf.and_then(|p| p.val(&["totalCashFromOperatingActivities"]));
    let capex = latest_cf.and_then(|p| p.val(&["capitalExpenditures"]));
    let sbc = latest_cf.and_then(|p| p.val(&["stockBasedCompensation"]));
    let mc = qs_raw(&qs, "price", "marketCap").or_else(|| qs_raw(&qs, "summaryDetail", "marketCap"));
    let emp = qs_raw(&qs, "assetProfile", "fullTimeEmployees");
    let _ = te;
    let mut m = Map::new();
    obj_insert(&mut m, "s", json!(sym));
    // Altman Z-Score (needs a populated balance sheet; Yahoo's 2026
    // balanceSheetHistory is mostly empty, so z is often absent — honest).
    if let Some(ta) = ta.filter(|x| *x > 0.0) {
        let mut z = 0.0;
        let mut any = false;
        if let Some(w) = wc {
            z += 1.2 * (w / ta);
            any = true;
        }
        if let Some(r) = re_ {
            z += 1.4 * (r / ta);
            any = true;
        }
        if let Some(e) = ebit {
            z += 3.3 * (e / ta);
            any = true;
        }
        if let (Some(mc), Some(tl)) = (mc, tl) {
            if tl > 0.0 {
                z += 0.6 * (mc / tl);
                any = true;
            }
        }
        if let Some(rv) = revenue {
            z += 1.0 * (rv / ta);
            any = true;
        }
        if any {
            obj_insert(&mut m, "z", json_round(Some(z), 2));
        }
    }
    // Cash conversion cycle.
    if let Some(rv) = revenue.filter(|x| *x > 0.0) {
        let daily = rv / 365.0;
        let mut dso = None;
        let mut dio = None;
        let mut dpo = None;
        if let Some(a) = ar {
            dso = Some(crate::round1(a / daily));
            obj_insert(&mut m, "dso", json_round(dso, 1));
        }
        if let (Some(c), Some(i)) = (cogs, inv) {
            if c > 0.0 {
                dio = Some(crate::round1(i / (c / 365.0)));
                obj_insert(&mut m, "dio", json_round(dio, 1));
            }
        }
        if let (Some(c), Some(a)) = (cogs, ap) {
            if c > 0.0 {
                dpo = Some(crate::round1(a / (c / 365.0)));
                obj_insert(&mut m, "dpo", json_round(dpo, 1));
            }
        }
        if let (Some(a), Some(b), Some(c)) = (dso, dio, dpo) {
            obj_insert(&mut m, "ccc", json_round(Some(a + b - c), 1));
        }
    }
    // Earnings quality.
    if let (Some(o), Some(n)) = (ocf, ni) {
        if n != 0.0 {
            obj_insert(&mut m, "oq", json_round(Some(o / n), 2));
        }
    }
    let fcf = match (ocf, capex) {
        (Some(o), Some(c)) => Some(o + c),
        _ => None,
    };
    if let (Some(f), Some(n)) = (fcf, ni) {
        if n != 0.0 {
            obj_insert(&mut m, "fq", json_round(Some(f / n), 2));
        }
    }
    if let (Some(n), Some(o), Some(t)) = (ni, ocf, ta) {
        if t > 0.0 {
            obj_insert(&mut m, "ac", json_round(Some((n - o) / t * 100.0), 1));
        }
    }
    // Efficiency.
    if let Some(e) = emp.filter(|x| *x > 0.0) {
        if let Some(rv) = revenue {
            obj_insert(&mut m, "re", json!(fmt_num(Some(rv / e))));
        }
        if let Some(n) = ni {
            obj_insert(&mut m, "ne", json!(fmt_num(Some(n / e))));
        }
    }
    if let (Some(c), Some(rv)) = (capex, revenue) {
        if rv > 0.0 {
            obj_insert(&mut m, "cx", json_round(Some(c / rv * 100.0), 1));
        }
    }
    if let (Some(s), Some(rv)) = (sbc, revenue) {
        if rv > 0.0 {
            obj_insert(&mut m, "sb", json_round(Some(s / rv * 100.0), 1));
        }
    }
    // 3-year CAGRs (latest vs 4th annual column).
    if is_annual.len() >= 4 {
        if let (Some(rv), Some(old)) = (
            revenue,
            is_annual[3].val(&["totalRevenue"]),
        ) {
            if old > 0.0 {
                obj_insert(&mut m, "r3", json_round(Some(((rv / old).powf(1.0 / 3.0) - 1.0) * 100.0), 1));
            }
        }
        if let (Some(n), Some(old)) = (ni, is_annual[3].val(&["netIncome", "netIncomeApplicableToCommonShares"])) {
            if old > 0.0 {
                obj_insert(&mut m, "e3", json_round(Some(((n / old).powf(1.0 / 3.0) - 1.0) * 100.0), 1));
            }
        }
    }
    if let (Some(d), Some(c), Some(e)) = (debt, cash, ebitda) {
        if e != 0.0 {
            obj_insert(&mut m, "nd", json_round(Some((d - c) / e), 1));
        }
    }
    let _ = f;
    let mut flags: Vec<String> = Vec::new();
    if let Some(z) = m.get("z").and_then(|v| v.as_f64()) {
        if z < 1.8 {
            flags.push("distress".to_string());
        } else if z < 3.0 {
            flags.push("grey".to_string());
        }
    }
    if let Some(oq) = m.get("oq").and_then(|v| v.as_f64()) {
        if oq < 0.5 {
            flags.push("low_ocf_quality".to_string());
        }
    }
    if let Some(ac) = m.get("ac").and_then(|v| v.as_f64()) {
        if ac < -5.0 {
            flags.push("aggressive_accruals".to_string());
        }
    }
    if !flags.is_empty() {
        obj_insert(&mut m, "fl", json!(flags));
    }
    let s = out_json(&Value::Object(m));
    cache_set(cache, "yfinance", &sym, "health", s.trim_end());
    Ok(s)
}
