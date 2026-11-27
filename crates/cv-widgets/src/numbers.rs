//! How counts, sizes and pull request numbers read: thousands grouped, huge counts compacted, a
//! count with its noun, byte sizes, the widest text a count column needs, and each platform's
//! number prefix. The widgets, the page, the lists,
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
    format!("{sign}{}", zeroed(value.min(9_999)))
}

/// "0,000" for 1,270: `value` grouped, every digit a zero, as wide as the count (digits are
/// tabular).
pub fn zeroed(value: u64) -> String {
    group_digits(value as usize).chars().map(|c| if c.is_ascii_digit() { '0' } else { c }).collect()
}

/// `one` for a count of 1, else `many`.
pub fn noun<'a>(count: usize, one: &'a str, many: &'a str) -> &'a str {
    if count == 1 { one } else { many }
}

/// "1 file", "3 files", "2 searches".
pub fn counted(count: usize, one: &str, many: &str) -> String {
    format!("{count} {}", noun(count, one, many))
}

/// "0 B", "820 KB", "3.4 MB", "2.31 GB" (binary units, as Explorer shows them). A size whose
/// rounded number would reach 1024 takes the next unit: "1.0 MB", never "1024 KB".
pub fn format_bytes(bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    let value = bytes as f64;
    let kilobytes = value / KIB;
    let megabytes = kilobytes / KIB;
    if value < KIB {
        format!("{bytes} B")
    } else if kilobytes.round() < KIB {
        format!("{kilobytes:.0} KB")
    } else if (megabytes * 10.0).round() / 10.0 < KIB {
        format!("{megabytes:.1} MB")
    } else {
        let gigabytes = value / KIB / KIB / KIB;
        if gigabytes.fract() == 0.0 {
            format!("{gigabytes:.0} GB")
        } else {
            format!("{gigabytes:.2} GB")
        }
    }
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
    use super::{compact_count, counted, digit_template, format_bytes, group_digits, noun, zeroed};

    #[test]
    fn counts_stay_narrow() {
        assert_eq!(compact_count(9_999), "9,999");
        assert_eq!(compact_count(12_345), "12k");
        assert_eq!(compact_count(2_500_000), "2.5M");
        assert_eq!(digit_template('+', 12_345), "+0,000");
        assert_eq!(digit_template('−', 42), "−00");
        assert_eq!(zeroed(12_345), "00,000", "not capped");
        assert_eq!(zeroed(7), "0");
    }

    #[test]
    fn counts_take_the_noun_of_their_number() {
        for (count, file, files, search) in [
            (0, "files", "0 files", "0 searches"),
            (1, "file", "1 file", "1 search"),
            (2, "files", "2 files", "2 searches"),
        ] {
            assert_eq!(noun(count, "file", "files"), file);
            assert_eq!(counted(count, "file", "files"), files);
            assert_eq!(counted(count, "search", "searches"), search);
        }
    }

    #[test]
    fn formats_sizes_like_explorer() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(820 * 1024), "820 KB");
        assert_eq!(format_bytes(3 * 1024 * 1024 + 400 * 1024), "3.4 MB");
        assert_eq!(format_bytes(10 * 1024 * 1024 * 1024), "10 GB");
        assert_eq!(format_bytes(2_480_000_000), "2.31 GB");
    }

    #[test]
    fn a_size_that_rounds_up_to_1024_takes_the_next_unit() {
        assert_eq!(format_bytes(1024 * 1024 - 1), "1.0 MB");
        assert_eq!(format_bytes(1023 * 1024), "1023 KB");
        assert_eq!(format_bytes(1024 * 1024 * 1024 - 1), "1.00 GB");
        assert_eq!(format_bytes(1023 * 1024 * 1024), "1023.0 MB");
    }

    #[test]
    fn groups_thousands() {
        assert_eq!(group_digits(7), "7");
        assert_eq!(group_digits(1284), "1,284");
        assert_eq!(group_digits(96410), "96,410");
        assert_eq!(group_digits(1_000_000), "1,000,000");
    }
}
