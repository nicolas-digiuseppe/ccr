/// Per-million-token pricing for Claude models.
/// Fields: (input, output, cache_write, cache_read) in USD per million tokens.
#[derive(Copy, Clone)]
struct Pricing {
    input: f64,
    output: f64,
    cache_write: f64,
    cache_read: f64,
}

/// Pricing table: (model_prefix, Pricing). Checked longest-match first.
static PRICING: &[(&str, Pricing)] = &[
    ("claude-opus-4",   Pricing { input: 15.0,  output: 75.0,  cache_write: 18.75, cache_read: 1.50 }),
    ("claude-opus-3",   Pricing { input: 15.0,  output: 75.0,  cache_write: 18.75, cache_read: 1.50 }),
    ("claude-sonnet-4", Pricing { input: 3.0,   output: 15.0,  cache_write: 3.75,  cache_read: 0.30 }),
    ("claude-sonnet-3", Pricing { input: 3.0,   output: 15.0,  cache_write: 3.75,  cache_read: 0.30 }),
    ("claude-haiku-4",  Pricing { input: 0.80,  output: 4.0,   cache_write: 1.0,   cache_read: 0.08 }),
    ("claude-haiku-3",  Pricing { input: 0.25,  output: 1.25,  cache_write: 0.30,  cache_read: 0.03 }),
];

/// Fallback pricing when no prefix matches (Sonnet-class rates).
static FALLBACK: Pricing = Pricing { input: 3.0, output: 15.0, cache_write: 3.75, cache_read: 0.30 };

/// Compute USD cost for a single message.
/// Looks up the longest matching prefix in PRICING; falls back to FALLBACK.
pub fn cost_usd(model: &str, input: u64, output: u64, cache_write: u64, cache_read: u64) -> f64 {
    let pricing = PRICING
        .iter()
        .filter(|(prefix, _)| model.starts_with(prefix))
        .max_by_key(|(prefix, _)| prefix.len())
        .map(|(_, p)| p)
        .unwrap_or(&FALLBACK);

    let per_m = 1_000_000.0_f64;
    pricing.input         * input as f64       / per_m
    + pricing.output      * output as f64      / per_m
    + pricing.cache_write * cache_write as f64 / per_m
    + pricing.cache_read  * cache_read as f64  / per_m
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_opus3_cost() {
        // claude-opus-3 has same rates as opus-4: $15/M input
        let cost = cost_usd("claude-opus-3-20240229", 1_000_000, 0, 0, 0);
        assert!((cost - 15.0).abs() < 0.001, "got {cost}");
    }

    #[test]
    fn test_opus4_cost() {
        // 1M input tokens at $15/M = $15.00
        let cost = cost_usd("claude-opus-4-6", 1_000_000, 0, 0, 0);
        assert!((cost - 15.0).abs() < 0.001, "got {cost}");
    }

    #[test]
    fn test_sonnet4_output_cost() {
        // 1M output tokens at $15/M = $15.00
        let cost = cost_usd("claude-sonnet-4-6", 0, 1_000_000, 0, 0);
        assert!((cost - 15.0).abs() < 0.001, "got {cost}");
    }

    #[test]
    fn test_haiku_cache_read_cost() {
        // 1M cache read tokens at $0.08/M = $0.08
        let cost = cost_usd("claude-haiku-4-5-20251001", 0, 0, 0, 1_000_000);
        assert!((cost - 0.08).abs() < 0.001, "got {cost}");
    }

    #[test]
    fn test_fallback_model() {
        // Unknown model uses sonnet pricing: 1M input = $3.00
        let cost = cost_usd("claude-unknown-model", 1_000_000, 0, 0, 0);
        assert!((cost - 3.0).abs() < 0.001, "got {cost}");
    }

    #[test]
    fn test_zero_tokens() {
        let cost = cost_usd("claude-opus-4-6", 0, 0, 0, 0);
        assert_eq!(cost, 0.0);
    }

    #[test]
    fn test_mixed_tokens() {
        // Opus: input=$15/M, output=$75/M, cache_write=$18.75/M, cache_read=$1.50/M
        // 100k input + 10k output + 50k cache_write + 500k cache_read
        let cost = cost_usd("claude-opus-4-6", 100_000, 10_000, 50_000, 500_000);
        let expected = 0.1 * 15.0 + 0.01 * 75.0 + 0.05 * 18.75 + 0.5 * 1.50;
        assert!((cost - expected).abs() < 0.001, "got {cost}, expected {expected}");
    }
}
