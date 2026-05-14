use skopos_core::Money;

#[derive(Debug, Clone, PartialEq)]
pub struct ModelPrice {
    pub provider: String,
    pub model: String,
    pub input_per_million: f64,
    pub output_per_million: f64,
    pub cached_input_per_million: Option<f64>,
}

impl ModelPrice {
    pub fn estimate_usd(
        &self,
        input_tokens: u64,
        output_tokens: u64,
        cached_input_tokens: Option<u64>,
    ) -> Money {
        let regular_input = input_tokens.saturating_sub(cached_input_tokens.unwrap_or(0));
        let input_cost = regular_input as f64 / 1_000_000.0 * self.input_per_million;
        let output_cost = output_tokens as f64 / 1_000_000.0 * self.output_per_million;
        let cached_cost = cached_input_tokens.unwrap_or(0) as f64 / 1_000_000.0
            * self
                .cached_input_per_million
                .unwrap_or(self.input_per_million);

        Money::usd(input_cost + output_cost + cached_cost)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn estimates_usd_from_token_counts() {
        let price = ModelPrice {
            provider: "example".to_string(),
            model: "model".to_string(),
            input_per_million: 1.0,
            output_per_million: 2.0,
            cached_input_per_million: Some(0.25),
        };

        let cost = price.estimate_usd(1_000_000, 500_000, Some(100_000));
        assert!((cost.amount - 1.925).abs() < 1e-9);
    }
}
