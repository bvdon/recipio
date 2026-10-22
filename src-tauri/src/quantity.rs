//! Ingredient quantities: parsing what people type ("1 1/2", "¾", "2-3") and formatting
//! stored numbers as friendly fractions ("1½", "⅓", "2–3").

/// A parsed quantity: a single amount, or a range when `max` is set (always > `min`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Quantity {
    pub min: f64,
    pub max: Option<f64>,
}

const GLYPHS: [(char, u32, u32); 18] = [
    ('½', 1, 2),
    ('⅓', 1, 3),
    ('⅔', 2, 3),
    ('¼', 1, 4),
    ('¾', 3, 4),
    ('⅕', 1, 5),
    ('⅖', 2, 5),
    ('⅗', 3, 5),
    ('⅘', 4, 5),
    ('⅙', 1, 6),
    ('⅚', 5, 6),
    ('⅐', 1, 7),
    ('⅛', 1, 8),
    ('⅜', 3, 8),
    ('⅝', 5, 8),
    ('⅞', 7, 8),
    ('⅑', 1, 9),
    ('⅒', 1, 10),
];

/// Denominators used for display, smallest first so ½ wins over 2/4.
const DISPLAY_DENOMINATORS: [u32; 5] = [2, 3, 4, 6, 8];
const TOLERANCE: f64 = 0.01;

/// Parses a quantity as typed. `Ok(None)` for blank input, `Err(())` if not understood.
///
/// Accepts whole numbers, decimals (`.` or `,`), fractions (`3/4`), mixed numbers
/// (`1 1/2`, `1-1/2`, `1½`), unicode fractions, and ranges (`2-3`, `2–3`, `2 to 3`,
/// `1/2-1`).
#[allow(clippy::result_unit_err)]
pub fn parse(text: &str) -> Result<Option<Quantity>, ()> {
    let mut s = String::with_capacity(text.len() + 4);
    for c in text.trim().chars() {
        match c {
            '–' | '—' | '−' => s.push('-'),
            '⁄' => s.push('/'),
            ',' => s.push('.'),
            _ => match GLYPHS.iter().find(|g| g.0 == c) {
                // "1½" → "1 1/2"
                Some(&(_, n, d)) => s.push_str(&format!(" {n}/{d}")),
                None => s.push(c),
            },
        }
    }
    let s = s.to_lowercase().replace(" to ", "-");
    let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if s.is_empty() {
        return Ok(None);
    }

    let parts: Vec<&str> = s.split('-').map(str::trim).collect();
    match parts[..] {
        [one] => Ok(Some(Quantity {
            min: amount(one)?,
            max: None,
        })),
        [left, right] => {
            let (lo, hi) = (amount(left)?, amount(right)?);
            // "1-1/2" is how some people write 1½: a whole number, then a proper fraction.
            if is_whole(left) && right.contains('/') && !right.contains(' ') && hi < 1.0 {
                return Ok(Some(Quantity {
                    min: lo + hi,
                    max: None,
                }));
            }
            if hi <= lo {
                return Err(());
            }
            Ok(Some(Quantity {
                min: lo,
                max: Some(hi),
            }))
        }
        _ => Err(()),
    }
}

/// One amount: `2`, `1.5`, `.5`, `3/4`, or `1 3/4`.
fn amount(s: &str) -> Result<f64, ()> {
    let tokens: Vec<&str> = s.split(' ').collect();
    match tokens[..] {
        [t] if t.contains('/') => fraction(t),
        [t] => number(t),
        [whole, frac] if is_whole(whole) && frac.contains('/') => {
            let f = fraction(frac)?;
            if f >= 1.0 {
                return Err(());
            }
            Ok(number(whole)? + f)
        }
        _ => Err(()),
    }
}

fn fraction(s: &str) -> Result<f64, ()> {
    let (n, d) = s.split_once('/').ok_or(())?;
    if !is_whole(n) || !is_whole(d) {
        return Err(());
    }
    let (n, d) = (number(n)?, number(d)?);
    if d == 0.0 {
        return Err(());
    }
    Ok(n / d)
}

fn number(s: &str) -> Result<f64, ()> {
    let valid = !s.is_empty()
        && s.chars().all(|c| c.is_ascii_digit() || c == '.')
        && s.chars().filter(|&c| c == '.').count() <= 1
        && s.chars().any(|c| c.is_ascii_digit());
    if !valid {
        return Err(());
    }
    s.parse::<f64>().map_err(|_| ())
}

fn is_whole(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_digit())
}

/// Formats a quantity or range for display, e.g. `1½`, `⅓`, `2–3`, `0.15`.
pub fn format(min: Option<f64>, max: Option<f64>) -> String {
    match (min, max) {
        (None, _) => String::new(),
        (Some(min), None) => format_one(min),
        (Some(min), Some(max)) => format!("{}–{}", format_one(min), format_one(max)),
    }
}

fn format_one(q: f64) -> String {
    let mut whole = q.trunc();
    let frac = q - whole;
    let glyph = if frac < TOLERANCE {
        None
    } else if frac > 1.0 - TOLERANCE {
        whole += 1.0;
        None
    } else {
        match nearest_glyph(frac) {
            Some(g) => Some(g),
            // No friendly fraction: plain decimal, at most 2 places.
            None => return trim_decimal(q),
        }
    };
    match (whole as u64, glyph) {
        (w, None) => w.to_string(),
        (0, Some(g)) => g.to_string(),
        (w, Some(g)) => format!("{w}{g}"),
    }
}

fn nearest_glyph(frac: f64) -> Option<char> {
    DISPLAY_DENOMINATORS.iter().find_map(|&d| {
        let n = (frac * d as f64).round() as u32;
        if n == 0 || n >= d || (frac - n as f64 / d as f64).abs() > TOLERANCE {
            return None;
        }
        GLYPHS.iter().find(|g| g.1 * d == n * g.2).map(|g| g.0)
    })
}

fn trim_decimal(q: f64) -> String {
    let s = format!("{q:.2}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one(text: &str) -> f64 {
        let q = parse(text).unwrap().unwrap();
        assert_eq!(q.max, None, "{text}");
        q.min
    }

    fn range(text: &str) -> (f64, f64) {
        let q = parse(text).unwrap().unwrap();
        (q.min, q.max.unwrap())
    }

    #[test]
    fn parses_numbers_and_fractions() {
        assert_eq!(parse("  ").unwrap(), None);
        assert_eq!(one("2"), 2.0);
        assert_eq!(one("1.5"), 1.5);
        assert_eq!(one(".5"), 0.5);
        assert_eq!(one("0,75"), 0.75);
        assert_eq!(one("3/4"), 0.75);
        assert_eq!(one("1 1/2"), 1.5);
        assert_eq!(one(" 1   1/2 "), 1.5);
        assert_eq!(one("1-1/2"), 1.5);
        assert_eq!(one("½"), 0.5);
        assert_eq!(one("1½"), 1.5);
        assert_eq!(one("1 ½"), 1.5);
        assert_eq!(one("1⁄3"), 1.0 / 3.0);
    }

    #[test]
    fn parses_ranges() {
        assert_eq!(range("2-3"), (2.0, 3.0));
        assert_eq!(range("2 – 3"), (2.0, 3.0));
        assert_eq!(range("2 to 3"), (2.0, 3.0));
        assert_eq!(range("1/2-1"), (0.5, 1.0));
        assert_eq!(range("½–¾"), (0.5, 0.75));
        assert_eq!(range("1 1/2 - 2"), (1.5, 2.0));
        assert_eq!(range("1½–2"), (1.5, 2.0));
    }

    #[test]
    fn rejects_nonsense() {
        for bad in [
            "abc", "1/0", "1/", "/2", "1.2.3", "3-2", "2-2", "1-2-3", "1 3/2", "1.5 1/2", "-1",
            "1 2", "1/2/3", "a/b", "1.5/2",
        ] {
            assert_eq!(parse(bad), Err(()), "{bad}");
        }
    }

    #[test]
    fn formats_friendly_fractions() {
        let f = |q: f64| format(Some(q), None);
        assert_eq!(format(None, None), "");
        assert_eq!(f(2.0), "2");
        assert_eq!(f(0.5), "½");
        assert_eq!(f(1.5), "1½");
        assert_eq!(f(0.75), "¾");
        assert_eq!(f(1.0 / 3.0), "⅓");
        assert_eq!(f(2.0 / 3.0), "⅔");
        assert_eq!(f(0.333), "⅓");
        assert_eq!(f(0.125), "⅛");
        assert_eq!(f(2.625), "2⅝");
        assert_eq!(f(1.0 / 6.0), "⅙");
        assert_eq!(f(0.995), "1");
        assert_eq!(f(0.15), "0.15");
        assert_eq!(f(1.1), "1.1");
        assert_eq!(f(250.0), "250");
        assert_eq!(format(Some(2.0), Some(3.0)), "2–3");
        assert_eq!(format(Some(0.5), Some(1.0)), "½–1");
    }

    #[test]
    fn formatted_text_parses_back() {
        for q in [0.5, 1.5, 0.75, 1.0 / 3.0, 2.625, 0.15, 3.0] {
            let text = format(Some(q), None);
            assert!((one(&text) - q).abs() < TOLERANCE, "{text}");
        }
        assert_eq!(range(&format(Some(1.5), Some(2.0))), (1.5, 2.0));
    }
}
