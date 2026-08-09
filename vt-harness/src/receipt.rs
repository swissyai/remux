//! Deterministic machine-readable VT harness receipt.

use std::fs;
use std::path::Path;

use crate::mutation::MutationKill;
use crate::{HarnessError, Result};

/// Measured wall durations for each harness tier.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Durations {
    /// Authored golden tier.
    pub authored: f64,
    /// Captured stream tier.
    pub streams: f64,
    /// Adversarial golden tier.
    pub adversarial: f64,
    /// Invariant tier.
    pub invariants: f64,
    /// Generic A/B self-check tier.
    pub differential: f64,
    /// Planted mutation build/probe tier.
    pub mutations: f64,
    /// Structured fuzz tier.
    pub fuzz: f64,
    /// Entire harness invocation.
    pub total: f64,
}

/// Complete receipt payload required by VT1.
pub struct Receipt<'a> {
    /// Source schema version.
    pub schema_version: u32,
    /// Git commit measured.
    pub git_sha: &'a str,
    /// Commit-derived stable timestamp.
    pub timestamp: &'a str,
    /// Authored cases executed.
    pub authored_cases: usize,
    /// Captured sessions replayed.
    pub stream_corpora: usize,
    /// Exact captured bytes replayed.
    pub stream_bytes: usize,
    /// Adversarial cases executed.
    pub adversarial_cases: usize,
    /// Independent invariant properties passed.
    pub invariant_properties: usize,
    /// Committed planted patches executed.
    pub planted_mutations: usize,
    /// Planted patches rejected.
    pub mutations_killed: usize,
    /// Per-mutation detecting tier.
    pub mutation_kills: &'a [MutationKill],
    /// Structured generated sequences executed.
    pub fuzz_executions: u64,
    /// A/B state divergences observed.
    pub fuzz_divergences: u64,
    /// Measured tier durations.
    pub durations: Durations,
    /// Overall gate result.
    pub pass: bool,
}

/// Writes the canonical receipt atomically. If an existing canonical receipt
/// names the same SHA, its measured durations are retained so regeneration at
/// that source SHA is byte-idempotent.
pub fn write(path: &Path, mut receipt: Receipt<'_>) -> Result<()> {
    if let Ok(existing) = fs::read_to_string(path) {
        if extract_string(&existing, "gitSha") == Some(receipt.git_sha) {
            if let Ok(durations) = parse_durations(&existing) {
                receipt.durations = durations;
            }
        }
    }
    let json = serialize(&receipt);
    let parent = path
        .parent()
        .ok_or_else(|| HarnessError::new("receipt path has no parent"))?;
    fs::create_dir_all(parent)?;
    let temporary = path.with_extension("json.tmp");
    fs::write(&temporary, json)?;
    fs::rename(temporary, path)?;
    Ok(())
}

fn serialize(receipt: &Receipt<'_>) -> String {
    let mut output = String::with_capacity(4096);
    output.push_str("{\n");
    output.push_str(&format!(
        "  \"schemaVersion\": {},\n",
        receipt.schema_version
    ));
    output.push_str(&format!(
        "  \"gitSha\": \"{}\",\n",
        json_escape(receipt.git_sha)
    ));
    output.push_str(&format!(
        "  \"timestamp\": \"{}\",\n",
        json_escape(receipt.timestamp)
    ));
    output.push_str(&format!(
        "  \"authoredCases\": {},\n",
        receipt.authored_cases
    ));
    output.push_str(&format!(
        "  \"streamCorpora\": {},\n",
        receipt.stream_corpora
    ));
    output.push_str(&format!("  \"streamBytes\": {},\n", receipt.stream_bytes));
    output.push_str(&format!(
        "  \"adversarialCases\": {},\n",
        receipt.adversarial_cases
    ));
    output.push_str(&format!(
        "  \"invariantProperties\": {},\n",
        receipt.invariant_properties
    ));
    output.push_str(&format!(
        "  \"plantedMutations\": {},\n",
        receipt.planted_mutations
    ));
    output.push_str(&format!(
        "  \"mutationsKilled\": {},\n",
        receipt.mutations_killed
    ));
    output.push_str("  \"mutationKills\": [\n");
    for (index, kill) in receipt.mutation_kills.iter().enumerate() {
        output.push_str(&format!(
            "    {{\"id\": \"{}\", \"detectedBy\": \"{}\"}}{}\n",
            json_escape(&kill.id),
            json_escape(&kill.detected_by),
            if index + 1 == receipt.mutation_kills.len() {
                ""
            } else {
                ","
            }
        ));
    }
    output.push_str("  ],\n");
    output.push_str(&format!(
        "  \"fuzzExecutions\": {},\n",
        receipt.fuzz_executions
    ));
    output.push_str(&format!(
        "  \"fuzzDivergences\": {},\n",
        receipt.fuzz_divergences
    ));
    output.push_str("  \"durationsSec\": {\n");
    output.push_str(&format!(
        "    \"authored\": {:.6},\n",
        receipt.durations.authored
    ));
    output.push_str(&format!(
        "    \"streams\": {:.6},\n",
        receipt.durations.streams
    ));
    output.push_str(&format!(
        "    \"adversarial\": {:.6},\n",
        receipt.durations.adversarial
    ));
    output.push_str(&format!(
        "    \"invariants\": {:.6},\n",
        receipt.durations.invariants
    ));
    output.push_str(&format!(
        "    \"differential\": {:.6},\n",
        receipt.durations.differential
    ));
    output.push_str(&format!(
        "    \"mutations\": {:.6},\n",
        receipt.durations.mutations
    ));
    output.push_str(&format!("    \"fuzz\": {:.6},\n", receipt.durations.fuzz));
    output.push_str(&format!("    \"total\": {:.6}\n", receipt.durations.total));
    output.push_str("  },\n");
    output.push_str(&format!("  \"pass\": {}\n", receipt.pass));
    output.push_str("}\n");
    output
}

fn extract_string<'a>(text: &'a str, key: &str) -> Option<&'a str> {
    let prefix = format!("  \"{key}\": \"");
    let line = text.lines().find(|line| line.starts_with(&prefix))?;
    let value = line.strip_prefix(&prefix)?;
    value.strip_suffix("\",")
}

fn parse_durations(text: &str) -> Result<Durations> {
    Ok(Durations {
        authored: duration(text, "authored")?,
        streams: duration(text, "streams")?,
        adversarial: duration(text, "adversarial")?,
        invariants: duration(text, "invariants")?,
        differential: duration(text, "differential")?,
        mutations: duration(text, "mutations")?,
        fuzz: duration(text, "fuzz")?,
        total: duration(text, "total")?,
    })
}

fn duration(text: &str, key: &str) -> Result<f64> {
    let prefix = format!("    \"{key}\": ");
    let line = text
        .lines()
        .find(|line| line.starts_with(&prefix))
        .ok_or_else(|| HarnessError::new(format!("receipt duration {key} missing")))?;
    let value = line
        .strip_prefix(&prefix)
        .expect("prefix matched")
        .strip_suffix(',')
        .unwrap_or_else(|| line.strip_prefix(&prefix).expect("prefix matched"));
    let parsed = value
        .parse::<f64>()
        .map_err(|_| HarnessError::new(format!("receipt duration {key} invalid")))?;
    if !parsed.is_finite() || parsed < 0.0 {
        return Err(HarnessError::new(format!(
            "receipt duration {key} is not finite and non-negative"
        )));
    }
    Ok(parsed)
}

fn json_escape(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            value if value.is_control() => output.push_str(&format!("\\u{:04x}", u32::from(value))),
            value => output.push(value),
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    const VALID: &str = concat!(
        "{\n",
        "  \"gitSha\": \"abc\",\n",
        "  \"durationsSec\": {\n",
        "    \"authored\": 1.000000,\n",
        "    \"streams\": 2.000000,\n",
        "    \"adversarial\": 3.000000,\n",
        "    \"invariants\": 4.000000,\n",
        "    \"differential\": 5.000000,\n",
        "    \"mutations\": 6.000000,\n",
        "    \"fuzz\": 7.000000,\n",
        "    \"total\": 8.000000\n",
        "  }\n",
        "}\n",
    );

    #[test]
    fn prior_receipt_parser_accepts_exact_canonical_shapes() {
        assert_eq!(extract_string(VALID, "gitSha"), Some("abc"));
        assert_eq!(parse_durations(VALID).expect("durations").total, 8.0);
    }

    #[test]
    fn prior_receipt_parser_rejects_missing_negative_nan_and_injected_fields() {
        for bad in [
            VALID.replace("    \"fuzz\": 7.000000,\n", ""),
            VALID.replace("1.000000", "-1"),
            VALID.replace("2.000000", "NaN"),
            VALID.replace("    \"total\": 8.000000", "    \"total\": 8 trailing"),
        ] {
            assert!(parse_durations(&bad).is_err(), "accepted {bad}");
        }
    }

    #[test]
    fn json_escaper_handles_quotes_slashes_controls_and_unicode() {
        assert_eq!(json_escape("a\"\\\n\u{1}界"), "a\\\"\\\\\\n\\u0001界");
    }
}
