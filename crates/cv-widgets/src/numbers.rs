//! How counts and pull request numbers read: thousands grouped, huge counts compacted, the widest
//! text a count column needs, and each platform's number prefix. The widgets, the page, the lists,
//! the tree and the title bar all print numbers this way, so the formatting lives at the bottom of
//! the crate graph, where every one of them can reach it.

use review_core::ProviderKind;

/// 1284 → "1,284".
pub fn group_digits(value: usize) -> String {
    let digits = value.to_string();
    let mut grouped = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, digit) in digits.chars().enumerate() {
        let remaining = digits.len() - index;
        if index > 0 && remaining.is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    grouped
}

/// Exact below 10,000, then "12k" and "1.2M", so one huge change cannot widen a column of counts
/// for every row. Tooltips have the exact numbers.
pub fn compact_count(value: u64) -> String {
    match value {
        0..10_000 => group_digits(value as usize),
        10_000..1_000_000 => format!("{}k", value / 1_000),
        _ => format!("{:.1}M", value as f64 / 1_000_000.0),
    }
}

/// "+0,000" for 1,270: the widest text a count up to `value` needs (digits are tabular). Compact
/// counts are never wider than "9,999".
pub fn digit_template(sign: char, value: u64) -> String {
    let digits: String = group_digits(value.min(9_999) as usize)
        .chars()
        .map(|c| if c.is_ascii_digit() { '0' } else { c })
        .collect();
    format!("{sign}{digits}")
}

/// How each platform writes a pull request number.
pub fn number_prefix(kind: ProviderKind) -> &'static str {
    match kind {
        ProviderKind::AzureDevOps | ProviderKind::GitLab => "!",
        ProviderKind::GitHub => "#",
    }
}

#[cfg(test)]
mod tests {
    use super::{compact_count, digit_template, group_digits};

    #[test]
    fn counts_stay_narrow() {
        assert_eq!(compact_count(9_999), "9,999");
        assert_eq!(compact_count(12_345), "12k");
        assert_eq!(compact_count(2_500_000), "2.5M");
        assert_eq!(digit_template('+', 12_345), "+0,000");
        assert_eq!(digit_template('−', 42), "−00");
    }

    #[test]
    fn groups_thousands() {
        assert_eq!(group_digits(7), "7");
        assert_eq!(group_digits(1284), "1,284");
        assert_eq!(group_digits(96410), "96,410");
        assert_eq!(group_digits(1_000_000), "1,000,000");
    }
}
