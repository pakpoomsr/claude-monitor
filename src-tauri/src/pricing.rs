//! Per-model-version pricing for Anthropic's Claude API. The five-column
//! shape (base input, 5m cache write, 1h cache write, cache hit, output)
//! mirrors the official pricing page at
//! https://platform.claude.com/docs/en/about-claude/pricing.
//!
//! Defaults are baked in at build time. Users can override any cell from
//! the Settings UI; overrides are persisted in `prefs.json`. The merged
//! table is the source of truth for `agents::estimate_cost`.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// All five Anthropic-listed price columns for one model SKU.
/// USD per million tokens.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct ModelPricing {
    pub base_input: f64,
    pub cache_write_5m: f64,
    pub cache_write_1h: f64,
    pub cache_read: f64,
    pub output: f64,
}

impl Default for ModelPricing {
    /// Conservative middle-of-the-road default if a model name doesn't match
    /// any known SKU — uses Sonnet 4.6 numbers, which are by far the most
    /// common path for Claude Code today.
    fn default() -> Self {
        SONNET_FAMILY
    }
}

/// Per-event token counts as parsed from a Claude Code `usage` block.
/// All fields are independent — they do not overlap.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct TokenUsage {
    pub input: u64,
    pub cache_write_5m: u64,
    pub cache_write_1h: u64,
    pub cache_read: u64,
    pub output: u64,
}

/// One pricing entry: an `id` (matched against the model name as a
/// case-insensitive substring), a human label, a deprecation flag, and the
/// five-column rate.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PricingEntry {
    /// Stable key used for override lookup AND substring match against the
    /// model id reported by Claude (e.g. `claude-opus-4-7`). The most
    /// specific entries (longer ids) come first in the table so that
    /// `claude-opus-4-7` doesn't accidentally match the Opus-4 row.
    pub id: String,
    pub label: String,
    pub deprecated: bool,
    pub pricing: ModelPricing,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PricingTable {
    pub entries: Vec<PricingEntry>,
}

impl PricingTable {
    /// Find the pricing for a given model id. Walks `entries` in declared
    /// order and returns the first whose `id` is a case-insensitive
    /// substring of `model`. Falls back to `ModelPricing::default()`.
    pub fn pricing_for(&self, model: &str) -> ModelPricing {
        let model_lc = model.to_lowercase();
        for e in &self.entries {
            if !e.id.is_empty() && model_lc.contains(&e.id.to_lowercase()) {
                return e.pricing;
            }
        }
        ModelPricing::default()
    }
}

// Family rates referenced by multiple SKU rows. Keeping a named constant
// makes it obvious when two SKUs share pricing and helps when Anthropic
// updates a whole family at once.

const OPUS_FAMILY_NEW: ModelPricing = ModelPricing {
    base_input: 5.00,
    cache_write_5m: 6.25,
    cache_write_1h: 10.00,
    cache_read: 0.50,
    output: 25.00,
};

const OPUS_FAMILY_LEGACY: ModelPricing = ModelPricing {
    base_input: 15.00,
    cache_write_5m: 18.75,
    cache_write_1h: 30.00,
    cache_read: 1.50,
    output: 75.00,
};

const SONNET_FAMILY: ModelPricing = ModelPricing {
    base_input: 3.00,
    cache_write_5m: 3.75,
    cache_write_1h: 6.00,
    cache_read: 0.30,
    output: 15.00,
};

const HAIKU_4_5: ModelPricing = ModelPricing {
    base_input: 1.00,
    cache_write_5m: 1.25,
    cache_write_1h: 2.00,
    cache_read: 0.10,
    output: 5.00,
};

const HAIKU_3_5: ModelPricing = ModelPricing {
    base_input: 0.80,
    cache_write_5m: 1.00,
    cache_write_1h: 1.60,
    cache_read: 0.08,
    output: 4.00,
};

const HAIKU_3: ModelPricing = ModelPricing {
    base_input: 0.25,
    cache_write_5m: 0.30,
    cache_write_1h: 0.50,
    cache_read: 0.03,
    output: 1.25,
};

/// Built-in defaults, sourced from Anthropic's pricing page on 2026-05-04.
/// Order matters: more-specific ids must precede less-specific ones (e.g.
/// `claude-opus-4-7` before `claude-opus-4`).
pub fn default_pricing_table() -> PricingTable {
    PricingTable {
        entries: vec![
            entry("claude-opus-4-7", "Claude Opus 4.7", false, OPUS_FAMILY_NEW),
            entry("claude-opus-4-6", "Claude Opus 4.6", false, OPUS_FAMILY_NEW),
            entry("claude-opus-4-5", "Claude Opus 4.5", false, OPUS_FAMILY_NEW),
            entry("claude-opus-4-1", "Claude Opus 4.1", false, OPUS_FAMILY_LEGACY),
            entry("claude-opus-4", "Claude Opus 4", false, OPUS_FAMILY_LEGACY),
            entry("claude-opus-3", "Claude Opus 3", true, OPUS_FAMILY_LEGACY),
            entry("claude-sonnet-4-6", "Claude Sonnet 4.6", false, SONNET_FAMILY),
            entry("claude-sonnet-4-5", "Claude Sonnet 4.5", false, SONNET_FAMILY),
            entry("claude-sonnet-4", "Claude Sonnet 4", false, SONNET_FAMILY),
            entry("claude-sonnet-3-7", "Claude Sonnet 3.7", true, SONNET_FAMILY),
            entry("claude-haiku-4-5", "Claude Haiku 4.5", false, HAIKU_4_5),
            entry("claude-haiku-3-5", "Claude Haiku 3.5", false, HAIKU_3_5),
            entry("claude-haiku-3", "Claude Haiku 3", false, HAIKU_3),
        ],
    }
}

fn entry(id: &str, label: &str, deprecated: bool, pricing: ModelPricing) -> PricingEntry {
    PricingEntry {
        id: id.to_string(),
        label: label.to_string(),
        deprecated,
        pricing,
    }
}

/// Compute the dollar cost of a token-usage delta for one model.
pub fn estimate_cost(usage: &TokenUsage, pricing: &ModelPricing) -> f64 {
    let m = 1_000_000.0;
    (usage.input as f64 / m) * pricing.base_input
        + (usage.cache_write_5m as f64 / m) * pricing.cache_write_5m
        + (usage.cache_write_1h as f64 / m) * pricing.cache_write_1h
        + (usage.cache_read as f64 / m) * pricing.cache_read
        + (usage.output as f64 / m) * pricing.output
}

/// Overlay a map of user overrides onto the default table. Override keys
/// must match `PricingEntry.id` exactly. Unknown keys are ignored. Result
/// preserves the default ordering.
pub fn merge_overrides(
    defaults: PricingTable,
    overrides: &HashMap<String, ModelPricing>,
) -> PricingTable {
    let mut out = defaults;
    for entry in out.entries.iter_mut() {
        if let Some(p) = overrides.get(&entry.id) {
            entry.pricing = *p;
        }
    }
    out
}
