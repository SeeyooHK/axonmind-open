//! Deterministic metric-cell parser.
//!
//! Converts a raw table cell string into a (value, unit) pair without calling the LLM.
//! Conservative: returns `None` on any ambiguity rather than guessing wrong.
//! Wrong values at `Confidence::RULE` (0.85) are worse than no value.

/// A successfully parsed numeric cell.
pub struct ParsedMetric {
    pub value: f64,
    pub unit: String,
}

/// Parse a single table cell into a numeric metric value.
///
/// Handled formats (examples):
/// - Currency: `"$1.2M"` → (1_200_000, "USD"), `"€1.5K"` → (1500, "EUR")
/// - Percentage: `"12.5%"` → (12.5, "Percent")
/// - Plain numeric: `"3,400"` → (3400, "Count"), `"100"` → (100, "Count")
/// - Negative/positive prefix: `"-$1.2M"`, `"+3,400"`
/// - Magnitude suffixes: `K` ×1_000, `M` ×1_000_000, `B` ×1_000_000_000
///
/// Returns `None` if the cell cannot be cleanly parsed (e.g. `"n/a"`, `"Q1 2024"`, `"—"`).
pub fn parse_metric_cell(raw: &str) -> Option<ParsedMetric> {
    let s = raw.trim();
    if s.is_empty() {
        return None;
    }

    // Strip leading sign
    let (s, negative) = match s.chars().next() {
        Some('-') => (&s[1..], true),
        Some('+') => (&s[1..], false),
        _ => (s, false),
    };

    // Strip currency prefix and record unit; remaining string is numeric + possible suffix
    let (s, currency) = strip_currency_prefix(s);

    // Percentage: must be checked before magnitude (both end with a letter)
    if let Some(num_part) = s.strip_suffix('%') {
        let num_part = num_part.replace(',', "");
        let v: f64 = num_part.trim().parse().ok()?;
        if !v.is_finite() {
            return None;
        }
        let v = if negative { -v } else { v };
        return Some(ParsedMetric {
            value: v,
            unit: "Percent".to_string(),
        });
    }

    // Magnitude suffix (case-insensitive, must be the trailing char after digits)
    let s_no_commas = s.replace(',', "");
    let (num_str, scale) = strip_magnitude_suffix(s_no_commas.trim());

    let v: f64 = num_str.trim().parse().ok()?;
    if !v.is_finite() {
        return None;
    }

    let v = if negative { -v } else { v };
    let v = v * scale;

    Some(ParsedMetric {
        value: v,
        unit: currency.unwrap_or_else(|| "Count".to_string()),
    })
}

/// Strip a leading currency symbol and return (remainder, Some(unit)).
/// If no symbol found, returns (input, None).
fn strip_currency_prefix(s: &str) -> (&str, Option<String>) {
    match s.chars().next() {
        Some('$') => (&s[1..], Some("USD".to_string())),
        Some('€') => (&s['€'.len_utf8()..], Some("EUR".to_string())),
        Some('£') => (&s['£'.len_utf8()..], Some("GBP".to_string())),
        Some('¥') => (&s['¥'.len_utf8()..], Some("JPY".to_string())),
        _ => (s, None),
    }
}

/// Detect and strip a trailing magnitude suffix from the *already comma-stripped* string.
/// Returns (numeric_part, scale_factor).
fn strip_magnitude_suffix(s: &str) -> (&str, f64) {
    match s.chars().last() {
        Some('K') | Some('k') => (&s[..s.len() - 1], 1_000.0),
        Some('M') | Some('m') => (&s[..s.len() - 1], 1_000_000.0),
        Some('B') | Some('b') => (&s[..s.len() - 1], 1_000_000_000.0),
        Some('T') | Some('t') => (&s[..s.len() - 1], 1_000_000_000_000.0),
        _ => (s, 1.0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_currency_with_magnitude() {
        let m = parse_metric_cell("$1.2M").unwrap();
        assert_eq!(m.value, 1_200_000.0);
        assert_eq!(m.unit, "USD");

        let m = parse_metric_cell("€1.5K").unwrap();
        assert_eq!(m.value, 1_500.0);
        assert_eq!(m.unit, "EUR");
    }

    #[test]
    fn parses_percentage() {
        let m = parse_metric_cell("12.5%").unwrap();
        assert_eq!(m.value, 12.5);
        assert_eq!(m.unit, "Percent");
    }

    #[test]
    fn parses_plain_numeric_with_commas() {
        let m = parse_metric_cell("3,400").unwrap();
        assert_eq!(m.value, 3400.0);
        assert_eq!(m.unit, "Count");

        let m = parse_metric_cell("100").unwrap();
        assert_eq!(m.value, 100.0);
        assert_eq!(m.unit, "Count");
    }

    #[test]
    fn returns_none_for_non_numeric() {
        // WHY: wrong values at Confidence::RULE (0.85) are worse than no value
        assert!(parse_metric_cell("n/a").is_none());
        assert!(parse_metric_cell("—").is_none());
        assert!(parse_metric_cell("Q1 2024").is_none());
        assert!(parse_metric_cell("").is_none());
    }

    #[test]
    fn handles_sign_prefix() {
        let m = parse_metric_cell("-$1.2M").unwrap();
        assert_eq!(m.value, -1_200_000.0);
        assert_eq!(m.unit, "USD");

        let m = parse_metric_cell("+3,400").unwrap();
        assert_eq!(m.value, 3400.0);
    }
}
