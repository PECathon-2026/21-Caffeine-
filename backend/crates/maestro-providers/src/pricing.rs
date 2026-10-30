//! Static pricing table: model-name prefix → USD per million tokens
//! (input, output). Longest matching prefix wins. Unknown models cost 0 and
//! are flagged (`known == false`) so the daemon can surface them.
//!
//! Prices as of 2026-08 (Anthropic first-party rates; others best known).

use maestro_types::provider::Usage;

/// (model-name prefix, usd per 1M input tokens, usd per 1M output tokens)
const TABLE: &[(&str, f64, f64)] = &[
    // Anthropic — current families
    ("claude-fable-5", 10.0, 50.0),
    ("claude-mythos-5", 10.0, 50.0),
    ("claude-opus-5", 5.0, 25.0),
    ("claude-opus-4-8", 5.0, 25.0),
    ("claude-opus-4-7", 5.0, 25.0),
    ("claude-opus-4-6", 5.0, 25.0),
    ("claude-opus-4-5", 5.0, 25.0),
    ("claude-opus-4", 15.0, 75.0), // opus 4.0 / 4.1 legacy pricing
    ("claude-sonnet-5", 2.0, 10.0),
    ("claude-sonnet-4-6", 3.0, 15.0),
    ("claude-sonnet-4", 3.0, 15.0),
    ("claude-3-7-sonnet", 3.0, 15.0),
    ("claude-3-5-sonnet", 3.0, 15.0),
    ("claude-haiku-4-5", 1.0, 5.0),
    ("claude-3-5-haiku", 0.8, 4.0),
    ("claude-3-haiku", 0.25, 1.25),
    // Reference estimates verified 2026-10-29, not Azure invoice reconciliation.
    // GPT: developers.openai.com/api/docs/models/gpt-6.1-sol
    // Kimi: Argmin apps/gateway/src/domain/usage-list-prices.ts (vendor benchmark).
    ("gpt-6.1-sol", 2.0, 10.0),
    // Astra: developers.openai.com/api/docs/models/gpt-6-astra.
    // Other Azure deployments: Argmin domain/azure-list-prices.ts, 2026-09-23.
    ("gpt-6-astra", 10.0, 50.0),
    ("gpt-6-sol", 2.0, 10.0),
    ("gpt-6-luna", 0.1, 0.5),
    ("gpt-5.6-sol", 4.0, 20.0),
    ("gpt-5.6-terra", 2.0, 12.0),
    ("gpt-5.6-luna", 0.2, 1.2),
    ("DeepSeek-V4-Pro", 1.74, 3.48),
    ("grok-4.6", 1.25, 6.0),
    ("Kimi-K2.7-Code", 0.95, 4.0),
    // OpenAI
    ("gpt-4o-mini", 0.15, 0.6),
    ("gpt-4o", 2.5, 10.0),
    ("gpt-5-nano", 0.05, 0.4),
    ("gpt-5-mini", 0.25, 2.0),
    ("gpt-5", 1.25, 10.0),
    ("o3-mini", 1.1, 4.4),
    ("o3", 2.0, 8.0),
    // DeepSeek
    ("deepseek-reasoner", 0.55, 2.19),
    ("deepseek", 0.27, 1.1),
    // Llama (typical hosted rates; local serving is free)
    ("llama-3.1-405b", 3.5, 3.5),
    ("llama-3.3-70b", 0.9, 0.9),
    ("llama-3.1-70b", 0.9, 0.9),
    ("llama", 0.6, 0.7),
];

/// Look up `(usd_per_mtok_in, usd_per_mtok_out)` by longest matching prefix.
pub fn price_for(model: &str) -> Option<(f64, f64)> {
    TABLE
        .iter()
        .filter(|(prefix, _, _)| model.starts_with(prefix))
        .max_by_key(|(prefix, _, _)| prefix.len())
        .map(|&(_, input, output)| (input, output))
}

/// Compute the cost of an invocation in micro-USD (integer). Returns
/// `(0, false)` for unknown models. $X per 1M tokens == X micro-USD per
/// token, so the math is exact modulo the final rounding.
pub fn cost_micro_usd(model: &str, usage: &Usage) -> (i64, bool) {
    match price_for(model) {
        Some((input, output)) => {
            let cost = usage.input_tokens.max(0) as f64 * input
                + usage.output_tokens.max(0) as f64 * output;
            (cost.round() as i64, true)
        }
        None => (0, false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn usage(input: i64, output: i64) -> Usage {
        Usage {
            input_tokens: input,
            output_tokens: output,
        }
    }

    #[test]
    fn sonnet_cost_math() {
        // 1M in @ $2 + 1M out @ $10 = $12 = 12_000_000 micro-USD
        let (cost, known) = cost_micro_usd("claude-sonnet-5", &usage(1_000_000, 1_000_000));
        assert!(known);
        assert_eq!(cost, 12_000_000);
    }

    #[test]
    fn per_token_micro_usd() {
        // opus 5: $5/Mtok in → 5 micro-USD per input token
        let (cost, known) = cost_micro_usd("claude-opus-5", &usage(3, 2));
        assert!(known);
        assert_eq!(cost, 3 * 5 + 2 * 25);
    }

    #[test]
    fn longest_prefix_wins() {
        // gpt-4o-mini must not match the shorter gpt-4o entry
        assert_eq!(price_for("gpt-4o-mini"), Some((0.15, 0.6)));
        assert_eq!(price_for("gpt-4o"), Some((2.5, 10.0)));
        // current opus generations are cheaper than the opus-4 legacy fallback
        assert_eq!(price_for("claude-opus-4-6"), Some((5.0, 25.0)));
        assert_eq!(price_for("claude-opus-4-1"), Some((15.0, 75.0)));
    }

    #[test]
    fn unknown_model_costs_zero_and_is_flagged() {
        let (cost, known) = cost_micro_usd("totally-unknown-model", &usage(1000, 1000));
        assert_eq!(cost, 0);
        assert!(!known);
        // mock role models are intentionally unknown
        assert!(!cost_micro_usd("planner", &usage(10, 10)).1);
    }

    #[test]
    fn zero_usage_costs_zero_but_is_known() {
        let (cost, known) = cost_micro_usd("claude-haiku-4-5", &usage(0, 0));
        assert_eq!(cost, 0);
        assert!(known);
    }

    #[test]
    fn negative_tokens_clamp_to_zero() {
        let (cost, known) = cost_micro_usd("claude-sonnet-5", &usage(-5, -5));
        assert_eq!(cost, 0);
        assert!(known);
    }
}
