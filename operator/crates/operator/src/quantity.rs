//! Minimal Kubernetes quantity parsing, enough to compare resource values semantically
//! (`1Gi` equals `1024Mi`, `500m` equals `0.5`).

/// The value in thousandths of a unit (milli-units), or `None` for anything unparsable or out of
/// range. Never panics.
pub fn milli(s: &str) -> Option<i128> {
    let s = s.trim();
    let split = s
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(s.len());
    let (num, suffix) = s.split_at(split);
    // Multiplier in milli-units.
    let mult: i128 = match suffix {
        "" => 1_000,
        "m" => 1,
        "k" => 1_000_000,
        "M" => 1_000_000_000,
        "G" => 1_000_000_000_000,
        "T" => 1_000_000_000_000_000,
        "P" => 1_000_000_000_000_000_000,
        "E" => 1_000_000_000_000_000_000_000,
        "Ki" => 1024 * 1_000,
        "Mi" => 1024i128.pow(2) * 1_000,
        "Gi" => 1024i128.pow(3) * 1_000,
        "Ti" => 1024i128.pow(4) * 1_000,
        "Pi" => 1024i128.pow(5) * 1_000,
        "Ei" => 1024i128.pow(6) * 1_000,
        _ => return None,
    };
    let (int, frac) = num.split_once('.').unwrap_or((num, ""));
    if int.is_empty() && frac.is_empty() || frac.len() > 9 || int.len() > 18 {
        return None;
    }
    let int: i128 = if int.is_empty() { 0 } else { int.parse().ok()? };
    let frac_val: i128 = if frac.is_empty() {
        0
    } else {
        frac.parse().ok()?
    };
    let scale = 10i128.pow(u32::try_from(frac.len()).ok()?);
    int.checked_mul(mult)?
        .checked_add(frac_val.checked_mul(mult)? / scale)
}

#[cfg(test)]
mod tests {
    use super::milli;

    #[test]
    fn equivalent_forms_compare_equal() {
        assert_eq!(milli("1Gi"), milli("1024Mi"));
        assert_eq!(milli("500m"), milli("0.5"));
        assert_eq!(milli("2"), milli("2000m"));
        assert_eq!(milli("1G"), milli("1000M"));
        assert_ne!(milli("1G"), milli("1Gi"));
    }

    #[test]
    fn garbage_is_none() {
        for s in [
            "",
            "abc",
            "1x",
            ".",
            "1..2",
            "-1",
            "99999999999999999999Gi",
            "1e3",
        ] {
            assert_eq!(milli(s), None, "{s}");
        }
    }
}
