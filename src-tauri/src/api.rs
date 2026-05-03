use reqwest::Client;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct UsageResponse {
    pub period_start: String,
    pub period_end: String,
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
    pub total_cost_usd: f64,
    pub by_model: Vec<ModelUsage>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ModelUsage {
    pub model: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost_usd: f64,
}

/// Fetch usage from Anthropic API
/// Note: Anthropic's usage API returns per-request token counts.
/// This aggregates them into a summary.
pub async fn fetch_usage(api_key: &str) -> anyhow::Result<UsageResponse> {
    let client = Client::new();

    // Get current month range
    let now = chrono::Utc::now();
    let start = chrono::NaiveDate::from_ymd_opt(now.year(), now.month(), 1)
        .unwrap()
        .format("%Y-%m-%d")
        .to_string();
    let end = now.format("%Y-%m-%d").to_string();

    // Anthropic usage endpoint (requires API key with billing read scope)
    let resp = client
        .get("https://api.anthropic.com/v1/usage")
        .header("x-api-key", api_key)
        .header("anthropic-version", "2023-06-01")
        .query(&[("start_date", &start), ("end_date", &end)])
        .send()
        .await?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(anyhow::anyhow!("API error {}: {}", status, body));
    }

    // The API returns a list of usage objects; aggregate them
    let raw: serde_json::Value = resp.json().await?;
    let items = raw.get("data").and_then(|d| d.as_array()).cloned().unwrap_or_default();

    let mut by_model: std::collections::HashMap<String, ModelUsage> = Default::default();

    for item in &items {
        let model = item
            .get("model")
            .and_then(|m| m.as_str())
            .unwrap_or("unknown")
            .to_string();
        let input = item
            .get("input_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let output = item
            .get("output_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let cost = item
            .get("cost")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);

        let entry = by_model.entry(model.clone()).or_insert(ModelUsage {
            model,
            input_tokens: 0,
            output_tokens: 0,
            cost_usd: 0.0,
        });
        entry.input_tokens += input;
        entry.output_tokens += output;
        entry.cost_usd += cost;
    }

    let by_model_vec: Vec<ModelUsage> = by_model.into_values().collect();
    let total_input = by_model_vec.iter().map(|m| m.input_tokens).sum();
    let total_output = by_model_vec.iter().map(|m| m.output_tokens).sum();
    let total_cost = by_model_vec.iter().map(|m| m.cost_usd).sum();

    Ok(UsageResponse {
        period_start: start,
        period_end: end,
        total_input_tokens: total_input,
        total_output_tokens: total_output,
        total_cost_usd: total_cost,
        by_model: by_model_vec,
    })
}

// Needed for chrono year/month
use chrono::Datelike;
