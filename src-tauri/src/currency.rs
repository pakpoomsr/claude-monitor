//! Currency conversion via api.frankfurter.app — free, no API key, ECB-sourced
//! daily rates. We fetch USD-base rates for a curated 10 currencies and cache
//! them in `prefs.json`. Refresh policy is once per 24h; on failure we keep
//! using the last cached rates and surface USD if there are none.

use crate::prefs::CurrencyCache;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use std::collections::HashMap;

/// Curated list of currencies the UI offers. USD is always available
/// implicitly (rate = 1.0); the rest come from Frankfurter.
pub const SUPPORTED: &[(&str, &str)] = &[
    ("USD", "$"),
    ("EUR", "€"),
    ("GBP", "£"),
    ("JPY", "¥"),
    ("CNY", "¥"),
    ("THB", "฿"),
    ("SGD", "S$"),
    ("INR", "₹"),
    ("KRW", "₩"),
    ("AUD", "A$"),
];

#[derive(Debug, Clone, serde::Serialize)]
pub struct CurrencyInfo {
    pub code: String,
    pub symbol: String,
    pub rate: f64,
}

#[derive(Debug, Deserialize)]
struct FrankfurterResponse {
    #[allow(dead_code)]
    base: String,
    rates: HashMap<String, f64>,
    #[allow(dead_code)]
    date: String,
}

/// Fetch fresh rates from Frankfurter for our supported currency list.
/// Returns a cache struct ready to persist into prefs.
pub async fn fetch_rates() -> Result<CurrencyCache, String> {
    let codes: Vec<&str> = SUPPORTED
        .iter()
        .map(|(c, _)| *c)
        .filter(|c| *c != "USD")
        .collect();
    let url = format!(
        "https://api.frankfurter.app/latest?from=USD&to={}",
        codes.join(",")
    );

    let resp: FrankfurterResponse = reqwest::Client::new()
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("frankfurter: {e}"))?
        .error_for_status()
        .map_err(|e| format!("frankfurter: {e}"))?
        .json()
        .await
        .map_err(|e| format!("frankfurter: {e}"))?;

    let mut rates = HashMap::new();
    rates.insert("USD".to_string(), 1.0);
    for (code, rate) in resp.rates {
        rates.insert(code, rate);
    }

    Ok(CurrencyCache {
        rates,
        fetched_at: Utc::now().to_rfc3339(),
        source: "frankfurter.app".to_string(),
    })
}

/// True when the cache is older than 24h (or unparseable, which means we
/// probably want to refresh too).
pub fn is_stale(cache: &CurrencyCache) -> bool {
    match DateTime::parse_from_rfc3339(&cache.fetched_at) {
        Ok(t) => {
            let age = Utc::now().signed_duration_since(t.with_timezone(&Utc));
            age.num_hours() >= 24
        }
        Err(_) => true,
    }
}

/// Build the response for the frontend — one row per supported code, with
/// the current rate (1.0 if missing). Order matches `SUPPORTED`.
pub fn currency_list(cache: Option<&CurrencyCache>) -> Vec<CurrencyInfo> {
    SUPPORTED
        .iter()
        .map(|(code, symbol)| {
            let rate = cache
                .and_then(|c| c.rates.get(*code).copied())
                .unwrap_or(if *code == "USD" { 1.0 } else { 1.0 });
            CurrencyInfo {
                code: code.to_string(),
                symbol: symbol.to_string(),
                rate,
            }
        })
        .collect()
}
