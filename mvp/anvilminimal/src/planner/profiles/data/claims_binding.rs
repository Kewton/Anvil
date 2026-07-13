use std::collections::BTreeMap;
use std::path::Path;
use std::sync::OnceLock;

use regex::Regex;
use serde::{Deserialize, Serialize};

const MAX_CLAIMS: usize = 10_000;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClaimBinding {
    pub report_path: String,
    pub byte_offset: usize,
    pub raw: String,
    pub normalized_value: f64,
    pub printed_precision: u32,
    pub percent: bool,
    pub unit: Option<String>,
    pub matched_key: Option<String>,
    pub matched_result_value: Option<f64>,
    pub rounded_result_value: Option<f64>,
    pub ok: bool,
}

#[derive(Debug, Clone, PartialEq)]
struct ExtractedClaim {
    byte_offset: usize,
    raw: String,
    normalized_value: f64,
    printed_precision: u32,
    percent: bool,
    unit: Option<String>,
}

pub fn bind_report_claims(
    report_path: &str,
    text: &str,
    values: &BTreeMap<String, f64>,
) -> Vec<ClaimBinding> {
    let visible = report_visible_text(Path::new(report_path), text);
    extract_numeric_claims(&visible)
        .into_iter()
        .take(MAX_CLAIMS)
        .map(|claim| bind_claim(report_path, claim, values))
        .collect()
}

pub fn claim_limit_exceeded(report_path: &str, text: &str) -> bool {
    let visible = report_visible_text(Path::new(report_path), text);
    numeric_claim_regex().find_iter(&visible).count() > MAX_CLAIMS
}

fn bind_claim(
    report_path: &str,
    claim: ExtractedClaim,
    values: &BTreeMap<String, f64>,
) -> ClaimBinding {
    let matched = values.iter().find_map(|(key, value)| {
        let rounded = round_to_printed_precision(*value, claim.printed_precision);
        values_match(rounded, claim.normalized_value, claim.printed_precision)
            .then(|| (key.clone(), *value, rounded))
    });
    ClaimBinding {
        report_path: report_path.to_string(),
        byte_offset: claim.byte_offset,
        raw: claim.raw,
        normalized_value: claim.normalized_value,
        printed_precision: claim.printed_precision,
        percent: claim.percent,
        unit: claim.unit,
        matched_key: matched.as_ref().map(|(key, _, _)| key.clone()),
        matched_result_value: matched.as_ref().map(|(_, value, _)| *value),
        rounded_result_value: matched.as_ref().map(|(_, _, rounded)| *rounded),
        ok: matched.is_some(),
    }
}

fn extract_numeric_claims(text: &str) -> Vec<ExtractedClaim> {
    numeric_claim_regex()
        .find_iter(text)
        .filter_map(|found| {
            let raw = found.as_str().trim();
            let percent = raw.ends_with('%') || raw.ends_with('％');
            let numeric = raw.trim_end_matches(['%', '％']).trim().replace(',', "");
            let printed_precision = numeric
                .split_once('.')
                .map_or(0, |(_, decimals)| decimals.len() as u32);
            let normalized_value = numeric.parse::<f64>().ok()?;
            Some(ExtractedClaim {
                byte_offset: found.start(),
                raw: raw.to_string(),
                normalized_value,
                printed_precision,
                percent,
                unit: (!percent).then(|| unit_after(text, found.end())).flatten(),
            })
        })
        .collect()
}

fn numeric_claim_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(r"[-+]?(?:[0-9]{1,3}(?:,[0-9]{3})+|[0-9]+)(?:\.[0-9]+)?(?:[ \t]*[%％])?")
            .expect("numeric claim regex")
    })
}

fn report_visible_text(path: &Path, text: &str) -> String {
    if path.extension().and_then(|extension| extension.to_str()) == Some("html") {
        html_visible_text(text)
    } else {
        markdown_visible_text(text)
    }
}

fn html_visible_text(text: &str) -> String {
    static SCRIPT_STYLE: OnceLock<Regex> = OnceLock::new();
    static TAGS: OnceLock<Regex> = OnceLock::new();
    let without_code = SCRIPT_STYLE
        .get_or_init(|| {
            Regex::new(r"(?is)<(?:script|style)\b[^>]*>.*?</(?:script|style)\s*>")
                .expect("script/style regex")
        })
        .replace_all(text, " ");
    let visible = TAGS
        .get_or_init(|| Regex::new(r"(?s)<[^>]*>").expect("HTML tag regex"))
        .replace_all(&without_code, " ");
    decode_common_entities(&visible)
}

fn markdown_visible_text(text: &str) -> String {
    static LINK_TARGET: OnceLock<Regex> = OnceLock::new();
    LINK_TARGET
        .get_or_init(|| Regex::new(r"\]\([^\r\n)]*\)").expect("Markdown link regex"))
        .replace_all(text, "]")
        .to_string()
}

fn decode_common_entities(text: &str) -> String {
    text.replace("&nbsp;", " ")
        .replace("&#160;", " ")
        .replace("&percnt;", "%")
        .replace("&#37;", "%")
        .replace("&minus;", "-")
}

fn unit_after(text: &str, end: usize) -> Option<String> {
    let suffix = text.get(end..)?.trim_start();
    let unit = suffix
        .chars()
        .take_while(|character| {
            character.is_alphabetic()
                || matches!(character, '円' | '件' | '人' | '個' | '秒' | '分' | '時')
        })
        .take(16)
        .collect::<String>();
    (!unit.is_empty()).then_some(unit)
}

fn round_to_printed_precision(value: f64, precision: u32) -> f64 {
    if precision > 15 {
        return value;
    }
    let scale = 10_f64.powi(precision as i32);
    (value * scale).round() / scale
}

fn values_match(actual: f64, printed: f64, precision: u32) -> bool {
    let scale = 10_f64.powi(-(precision.min(15) as i32));
    let tolerance = (scale * 1e-9).max(f64::EPSILON * actual.abs().max(1.0) * 4.0);
    (actual - printed).abs() <= tolerance
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn binds_grouped_percent_and_unit_claims_by_printed_precision() {
        let values = BTreeMap::from([
            ("total".to_string(), 1234.567),
            ("rate_percent".to_string(), 40.0),
            ("count".to_string(), 3.2),
        ]);

        let claims = bind_report_claims(
            "output/report.md",
            "Total 1,234.57 USD; rate 40%; count 3 件.",
            &values,
        );

        assert_eq!(claims.len(), 3);
        assert!(claims.iter().all(|claim| claim.ok), "{claims:?}");
        assert_eq!(claims[0].matched_key.as_deref(), Some("total"));
        assert_eq!(claims[0].unit.as_deref(), Some("USD"));
        assert!(claims[1].percent);
        assert_eq!(claims[2].matched_key.as_deref(), Some("count"));
    }

    #[test]
    fn percent_is_percentage_points_not_an_ambiguous_fraction() {
        let values = BTreeMap::from([("fraction".to_string(), 0.4)]);
        let claims = bind_report_claims("output/report.md", "Rate 40%", &values);
        assert!(!claims[0].ok);
    }

    #[test]
    fn html_markup_script_and_style_numbers_are_not_claims() {
        let values = BTreeMap::from([("total".to_string(), 12.5)]);
        let html = r#"<html><style>.x{width:99px}</style><script>const x=88</script><h1>Total <b>12.5</b></h1></html>"#;
        let claims = bind_report_claims("output/report.html", html, &values);
        assert_eq!(claims.len(), 1);
        assert!(claims[0].ok);
    }

    #[test]
    fn markdown_link_destinations_are_not_numeric_claims() {
        let values = BTreeMap::from([("total".to_string(), 7.0)]);
        let claims = bind_report_claims(
            "output/report.md",
            "[source](https://example.test/2026/07) Total 7",
            &values,
        );
        assert_eq!(claims.len(), 1);
        assert!(claims[0].ok);
    }
}
