//! Token usage and cost per job.
//!
//! A job can have several runs (session-limit waits, sandbox-restart retries, the recovery run);
//! each reports its own [`Usage`] in the `RunFinished` callback and `jobs.usage` is their sum.
//! Fields are optional and `None` means "not reported", never 0, so a sum of two unknowns stays
//! unknown. There is no price logic here: `cost_usd` is the cost as reported by the agent.

use enzarb_proto::Usage;

fn add<T: std::ops::Add<Output = T>>(a: Option<T>, b: Option<T>) -> Option<T> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a + b),
        (a, b) => a.or(b),
    }
}

/// `total + run`, field by field. Unknown on both sides stays unknown; unknown on one side is the
/// other side's value; `models` is the union (`total`'s first).
pub fn merge(total: Option<Usage>, run: &Usage) -> Usage {
    let t = total.unwrap_or_default();
    let mut models = t.models;
    for m in &run.models {
        if !models.contains(m) {
            models.push(m.clone());
        }
    }
    Usage {
        input_tokens: add(t.input_tokens, run.input_tokens),
        output_tokens: add(t.output_tokens, run.output_tokens),
        cache_read_input_tokens: add(t.cache_read_input_tokens, run.cache_read_input_tokens),
        cache_creation_input_tokens: add(
            t.cache_creation_input_tokens,
            run.cache_creation_input_tokens,
        ),
        models,
        cost_usd: add(t.cost_usd, run.cost_usd),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u(i: Option<u64>, o: Option<u64>, cost: Option<f64>, models: &[&str]) -> Usage {
        Usage {
            input_tokens: i,
            output_tokens: o,
            cache_read_input_tokens: None,
            cache_creation_input_tokens: None,
            models: models.iter().map(|m| m.to_string()).collect(),
            cost_usd: cost,
        }
    }

    #[test]
    fn first_run_becomes_the_total() {
        let run = u(Some(10), Some(2), Some(0.5), &["m1"]);
        assert_eq!(merge(None, &run), run);
    }

    #[test]
    fn runs_sum_field_by_field() {
        let a = u(Some(10), Some(2), Some(0.5), &["m1"]);
        let b = u(Some(5), Some(1), Some(0.25), &["m1", "m2"]);
        let m = merge(Some(a), &b);
        assert_eq!((m.input_tokens, m.output_tokens), (Some(15), Some(3)));
        assert_eq!(m.cost_usd, Some(0.75));
        assert_eq!(m.models, vec!["m1".to_string(), "m2".to_string()]);
    }

    #[test]
    fn unknown_stays_unknown_and_is_not_zero() {
        let a = u(Some(10), None, None, &[]);
        let b = u(None, None, None, &[]);
        let m = merge(Some(a), &b);
        assert_eq!(m.input_tokens, Some(10));
        assert_eq!(m.output_tokens, None);
        assert_eq!(m.cost_usd, None); // null + null = null
        // null + x = x, in both directions
        let c = u(None, None, Some(0.1), &[]);
        assert_eq!(merge(Some(m.clone()), &c).cost_usd, Some(0.1));
        assert_eq!(merge(Some(c), &m).cost_usd, Some(0.1));
    }

    #[test]
    fn an_explicit_zero_is_a_value() {
        let m = merge(Some(u(Some(0), None, None, &[])), &u(None, None, None, &[]));
        assert_eq!(m.input_tokens, Some(0));
    }
}
