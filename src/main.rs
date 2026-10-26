//! fin-rs binary: thin clap surface over fin_rs::cmds.
//! Output: compact JSON or CSV on stdout; errors as JSON on stdout, exit 1.

use clap::{Parser, Subcommand};
use fin_rs::cmds;
use fin_rs::{Cache, FinError, Yahoo};
use std::io::Write;

#[derive(Parser)]
#[command(name = "fin-rs", version, about = "AI-native financial data CLI (Rust port of fin-agent-cli). Read-only market data.")]
struct Cli {
    #[command(subcommand)]
    cmd: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Current quote (price, change%, volume) — JSON, 60s cache.
    #[command(name = "q")]
    Q { symbol: String },
    /// Price history — CSV, 24h cache.
    #[command(name = "h")]
    H {
        symbol: String,
        #[arg(default_value_t = 30)]
        days: i64,
    },
    /// FRED macroeconomic data — CSV, 24h cache. Requires FRED_API_KEY.
    #[command(name = "m")]
    M { series: String },
    /// Earnings: next date, surprise, 8Q history — JSON, 6h cache.
    #[command(name = "e")]
    E { symbol: String },
    /// Fundamentals + computed metrics — JSON, 24h cache.
    #[command(name = "f")]
    F { symbol: String },
    /// Financial statements: is | bs | cf — CSV, 24h cache.
    #[command(name = "stmt")]
    Stmt { symbol: String, stmt_type: String },
    /// Company identity — JSON, 24h cache.
    #[command(name = "info")]
    Info { symbol: String },
    /// Side-by-side fundamentals comparison — CSV.
    #[command(name = "cmp")]
    Cmp { symbols: Vec<String> },
    /// Quarterly trends (8 quarters) — CSV, 24h cache.
    #[command(name = "trend")]
    Trend { symbol: String },
    /// Dividend payment history — CSV, 24h cache.
    #[command(name = "div")]
    Div { symbol: String },
    /// SEC filings (10-K, 10-Q, 8-K, ...) — CSV, 6h cache.
    #[command(name = "sec")]
    Sec { symbol: String },
    /// Insider transactions — CSV, 12h cache.
    #[command(name = "insider")]
    Insider { symbol: String },
    /// Top institutional holders — CSV, 24h cache (PARTIAL: aggregates only).
    #[command(name = "holders")]
    Holders { symbol: String },
    /// Analyst recommendation trends — CSV, 24h cache.
    #[command(name = "anal")]
    Anal { symbol: String },
    /// Z-Score, cash cycle, earnings quality, CAGRs — JSON, 24h cache.
    #[command(name = "health")]
    Health { symbol: String },
}

fn main() {
    let cli = Cli::parse();
    // Cache is best-effort: if cache.db cannot be opened (e.g. read-only
    // cwd), commands still run uncached rather than failing.
    let cache = match Cache::open() {
        Ok(c) => Some(c),
        Err(e) => {
            eprintln!("fin-rs: cache unavailable, running uncached: {}", e.message);
            None
        }
    };
    let cache_ref = cache.as_ref();
    let result: Result<String, FinError> = (|| {
        match &cli.cmd {
            Commands::Q { symbol } => {
                let y = Yahoo::new()?;
                cmds::cmd_q(&y, cache_ref, symbol)
            }
            Commands::H { symbol, days } => {
                let y = Yahoo::new()?;
                cmds::cmd_h(&y, cache_ref, symbol, *days)
            }
            Commands::M { series } => cmds::cmd_m(cache_ref, series),
            Commands::E { symbol } => {
                let y = Yahoo::new()?;
                cmds::cmd_e(&y, cache_ref, symbol)
            }
            Commands::F { symbol } => {
                let y = Yahoo::new()?;
                cmds::cmd_f(&y, cache_ref, symbol)
            }
            Commands::Stmt { symbol, stmt_type } => {
                let y = Yahoo::new()?;
                cmds::cmd_stmt(&y, cache_ref, symbol, stmt_type)
            }
            Commands::Info { symbol } => {
                let y = Yahoo::new()?;
                cmds::cmd_info(&y, cache_ref, symbol)
            }
            Commands::Cmp { symbols } => {
                let y = Yahoo::new()?;
                cmds::cmd_cmp(&y, symbols)
            }
            Commands::Trend { symbol } => {
                let y = Yahoo::new()?;
                cmds::cmd_trend(&y, cache_ref, symbol)
            }
            Commands::Div { symbol } => {
                let y = Yahoo::new()?;
                cmds::cmd_div(&y, cache_ref, symbol)
            }
            Commands::Sec { symbol } => cmds::cmd_sec(cache_ref, symbol),
            Commands::Insider { symbol } => {
                let y = Yahoo::new()?;
                cmds::cmd_insider(&y, cache_ref, symbol)
            }
            Commands::Holders { symbol } => {
                let y = Yahoo::new()?;
                cmds::cmd_holders(&y, cache_ref, symbol)
            }
            Commands::Anal { symbol } => {
                let y = Yahoo::new()?;
                cmds::cmd_anal(&y, cache_ref, symbol)
            }
            Commands::Health { symbol } => {
                let y = Yahoo::new()?;
                cmds::cmd_health(&y, cache_ref, symbol)
            }
        }
    })();
    match result {
        Ok(out) => {
            let stdout = std::io::stdout();
            let mut h = stdout.lock();
            let _ = h.write_all(out.as_bytes());
            if !out.ends_with('\n') {
                let _ = h.write_all(b"\n");
            }
        }
        Err(e) => {
            eprintln!("fin-rs: {}: {}", e.code, e.message);
            println!("{}", e.to_json());
            std::process::exit(1);
        }
    }
}
