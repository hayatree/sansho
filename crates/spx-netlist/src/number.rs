//! SPICE numeric literals with scale suffixes.
//!
//! Provenance: original work, following the number syntax described in Nagel (1975),
//! *SPICE2* user's guide conventions (scale factors T, G, MEG, K, MIL, M, U, N, P, F).
//! See `docs/PROVENANCE.md`.

/// Parse a SPICE number such as `1.5k`, `10MEG`, `3pF`, `-2e-3`, `1mil`.
///
/// Letters after a recognized scale factor (units such as `F`, `Ohm`, `V`) are ignored, as in
/// SPICE. Returns `None` if the token does not start with a valid number.
pub fn parse_number(token: &str) -> Option<f64> {
    let s = token.as_bytes();
    let mut i = 0;
    if i < s.len() && (s[i] == b'+' || s[i] == b'-') {
        i += 1;
    }
    let mantissa_start = i;
    while i < s.len() && s[i].is_ascii_digit() {
        i += 1;
    }
    let int_digits = i - mantissa_start;
    let mut frac_digits = 0;
    if i < s.len() && s[i] == b'.' {
        i += 1;
        let fs = i;
        while i < s.len() && s[i].is_ascii_digit() {
            i += 1;
        }
        frac_digits = i - fs;
    }
    if int_digits == 0 && frac_digits == 0 {
        return None;
    }
    // Exponent: only if followed by digits (otherwise `e` is not part of the number).
    if i < s.len() && (s[i] == b'e' || s[i] == b'E') {
        let mut j = i + 1;
        if j < s.len() && (s[j] == b'+' || s[j] == b'-') {
            j += 1;
        }
        let ds = j;
        while j < s.len() && s[j].is_ascii_digit() {
            j += 1;
        }
        if j > ds {
            i = j;
        }
    }
    let base: f64 = token.get(..i)?.parse().ok()?;
    let rest = token.get(i..)?.to_ascii_lowercase();
    let scale = if rest.starts_with("meg") {
        1e6
    } else if rest.starts_with("mil") {
        25.4e-6
    } else {
        match rest.as_bytes().first() {
            None => 1.0,
            Some(b't') => 1e12,
            Some(b'g') => 1e9,
            Some(b'k') => 1e3,
            Some(b'm') => 1e-3,
            Some(b'u') => 1e-6,
            Some(b'n') => 1e-9,
            Some(b'p') => 1e-12,
            Some(b'f') => 1e-15,
            Some(b'a') => 1e-18,
            Some(c) if c.is_ascii_alphabetic() => 1.0, // unit only, e.g. "5V"
            Some(_) => return None,
        }
    };
    Some(base * scale)
}

/// The scale-suffix part of a numeric token (`"10Meg"` → `"Meg"`, `"1e-3"` → `""`).
pub(crate) fn scale_suffix(token: &str) -> &str {
    let b = token.as_bytes();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if c.is_ascii_alphabetic() {
            let exponent = (c == b'e' || c == b'E')
                && b.get(i + 1).is_some_and(|&d| {
                    d.is_ascii_digit()
                        || ((d == b'+' || d == b'-')
                            && b.get(i + 2).is_some_and(u8::is_ascii_digit))
                });
            if !exponent {
                return &token[i..];
            }
        }
        i += 1;
    }
    ""
}

#[cfg(test)]
mod tests {
    use super::{parse_number, scale_suffix};

    #[test]
    fn suffixes() {
        assert_eq!(scale_suffix("10Meg"), "Meg");
        assert_eq!(scale_suffix("1e-3"), "");
        assert_eq!(scale_suffix("2.2uF"), "uF");
        assert_eq!(scale_suffix("1E3k"), "k");
    }

    fn close(a: Option<f64>, b: f64) -> bool {
        a.is_some_and(|a| (a - b).abs() <= 1e-15 * b.abs().max(1.0))
    }

    #[test]
    fn plain_numbers() {
        assert!(close(parse_number("1"), 1.0));
        assert!(close(parse_number("-2.5"), -2.5));
        assert!(close(parse_number(".5"), 0.5));
        assert!(close(parse_number("1e3"), 1e3));
        assert!(close(parse_number("1.5E-3"), 1.5e-3));
    }

    #[test]
    fn scale_factors() {
        assert!(close(parse_number("1k"), 1e3));
        assert!(close(parse_number("1K"), 1e3));
        assert!(close(parse_number("1meg"), 1e6));
        assert!(close(parse_number("1MEG"), 1e6));
        assert!(close(parse_number("1m"), 1e-3));
        assert!(close(parse_number("1M"), 1e-3));
        assert!(close(parse_number("2u"), 2e-6));
        assert!(close(parse_number("3n"), 3e-9));
        assert!(close(parse_number("4p"), 4e-12));
        assert!(close(parse_number("5f"), 5e-15));
        assert!(close(parse_number("1mil"), 25.4e-6));
        assert!(close(parse_number("2g"), 2e9));
        assert!(close(parse_number("1t"), 1e12));
    }

    #[test]
    fn units_are_ignored() {
        assert!(close(parse_number("10pF"), 10e-12));
        assert!(close(parse_number("5V"), 5.0));
        assert!(close(parse_number("1kOhm"), 1e3));
        assert!(close(parse_number("1e-9s"), 1e-9));
    }

    #[test]
    fn invalid() {
        assert_eq!(parse_number("abc"), None);
        assert_eq!(parse_number(""), None);
        assert_eq!(parse_number("-"), None);
        assert_eq!(parse_number("."), None);
    }
}
