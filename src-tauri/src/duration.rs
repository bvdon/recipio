//! Recipe times from web pages: ISO 8601 durations (`PT1H30M`) as schema.org specifies,
//! plus the plain-text forms some sites put there instead ("1 hr 30 mins").

/// Parses a duration into whole minutes (rounded). `None` if it isn't one, or is zero.
pub fn minutes(text: &str) -> Option<u32> {
    let text = text.trim();
    let secs = if text.starts_with(['P', 'p']) {
        iso8601(text)?
    } else {
        plain(text)?
    };
    let mins = (secs / 60.0).round();
    (mins >= 1.0 && mins <= f64::from(u32::MAX)).then_some(mins as u32)
}

/// A point in a video as typed: "2:15", "1:02:03", "90" (seconds), "1m30s".
pub fn timestamp(text: &str) -> Option<u32> {
    let text = text.trim();
    let secs = if text.contains(':') {
        let parts: Vec<&str> = text.split(':').collect();
        if parts.len() > 3
            || parts
                .iter()
                .any(|p| p.is_empty() || p.len() > 4 || !p.bytes().all(|b| b.is_ascii_digit()))
        {
            return None;
        }
        let nums: Vec<u64> = parts
            .iter()
            .map(|p| p.parse().ok())
            .collect::<Option<_>>()?;
        // Everything after the first part is minutes/seconds, so below 60.
        if nums[1..].iter().any(|&n| n >= 60) {
            return None;
        }
        nums.iter().fold(0, |acc, n| acc * 60 + n)
    } else if !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit()) {
        text.parse().ok()?
    } else {
        plain(text)? as u64
    };
    u32::try_from(secs).ok().filter(|&s| s < 100 * 3600)
}

/// Seconds as a clock time: "0:15", "5:00", "1:02:05".
pub fn clock(secs: u64) -> String {
    match (secs / 3600, secs / 60 % 60, secs % 60) {
        (0, m, s) => format!("{m}:{s:02}"),
        (h, m, s) => format!("{h}:{m:02}:{s:02}"),
    }
}

/// `P[nD]T[nH][nM][nS]`, also weeks (`P1W`) and decimals (`PT1.5H`). Years and months
/// make no sense for cooking and are rejected.
fn iso8601(text: &str) -> Option<f64> {
    let upper = text.to_ascii_uppercase();
    let rest = upper.strip_prefix('P')?;
    let (date, time) = match rest.split_once('T') {
        Some((d, t)) => (d, Some(t)),
        None => (rest, None),
    };
    let mut total = 0.0;
    let mut any = false;
    for (part, units) in [
        (date, &[('W', 604_800.0), ('D', 86_400.0)][..]),
        (
            time.unwrap_or(""),
            &[('H', 3600.0), ('M', 60.0), ('S', 1.0)][..],
        ),
    ] {
        let mut number = String::new();
        // Units must appear in order and at most once.
        let mut next_unit = 0;
        for c in part.chars() {
            if c.is_ascii_digit() || c == '.' || c == ',' {
                number.push(if c == ',' { '.' } else { c });
                continue;
            }
            let i = units[next_unit..].iter().position(|&(u, _)| u == c)? + next_unit;
            let n: f64 = number.parse().ok()?;
            total += n * units[i].1;
            number.clear();
            next_unit = i + 1;
            any = true;
        }
        if !number.is_empty() {
            return None;
        }
    }
    if time == Some("") {
        return None;
    }
    any.then_some(total)
}

/// "1 hr 30 mins", "45 minutes", "1h30m", "2 hours". Every number needs a unit.
fn plain(text: &str) -> Option<f64> {
    let lower = text.to_lowercase();
    let mut total = 0.0;
    let mut any = false;
    let mut chars = lower.chars().peekable();
    while chars.peek().is_some() {
        while chars.next_if(|c| !c.is_ascii_digit()).is_some() {}
        let mut number = String::new();
        while let Some(c) = chars.next_if(|c| c.is_ascii_digit() || *c == '.') {
            number.push(c);
        }
        if number.is_empty() {
            break;
        }
        while chars.next_if(|c| *c == ' ').is_some() {}
        let mut unit = String::new();
        while let Some(c) = chars.next_if(|c| c.is_alphabetic()) {
            unit.push(c);
        }
        let factor = match unit.as_str() {
            "h" | "hr" | "hrs" | "hour" | "hours" => 3600.0,
            "m" | "min" | "mins" | "minute" | "minutes" => 60.0,
            "s" | "sec" | "secs" | "second" | "seconds" => 1.0,
            _ => return None,
        };
        total += number.parse::<f64>().ok()? * factor;
        any = true;
    }
    any.then_some(total)
}

#[cfg(test)]
mod tests {
    use super::minutes;

    #[test]
    fn parses_iso8601_durations() {
        assert_eq!(minutes("PT15M"), Some(15));
        assert_eq!(minutes("PT1H30M"), Some(90));
        assert_eq!(minutes("PT1H"), Some(60));
        assert_eq!(minutes("PT90M"), Some(90));
        assert_eq!(minutes("P0DT1H5M"), Some(65));
        assert_eq!(minutes("P1D"), Some(1440));
        assert_eq!(minutes("P1DT2H"), Some(1560));
        assert_eq!(minutes("P1W"), Some(10080));
        assert_eq!(minutes("PT1.5H"), Some(90));
        assert_eq!(minutes("PT0,5H"), Some(30));
        assert_eq!(minutes("PT20M30S"), Some(21));
        assert_eq!(minutes("PT45S"), Some(1));
        assert_eq!(minutes(" pt10m "), Some(10));
    }

    #[test]
    fn rejects_bad_or_zero_durations() {
        assert_eq!(minutes(""), None);
        assert_eq!(minutes("P"), None);
        assert_eq!(minutes("PT"), None);
        assert_eq!(minutes("PT0M"), None);
        assert_eq!(minutes("PT0S"), None);
        assert_eq!(minutes("P1M"), None, "months are not minutes");
        assert_eq!(minutes("PT10"), None);
        assert_eq!(minutes("PT30M1H"), None, "units out of order");
        assert_eq!(minutes("PT1H1H"), None);
        assert_eq!(minutes("PTxM"), None);
    }

    #[test]
    fn parses_video_timestamps() {
        use super::{clock, timestamp};
        assert_eq!(timestamp("2:15"), Some(135));
        assert_eq!(timestamp(" 0:05 "), Some(5));
        assert_eq!(timestamp("1:02:03"), Some(3723));
        assert_eq!(timestamp("90"), Some(90));
        assert_eq!(
            timestamp("75:00"),
            Some(4500),
            "minutes may exceed 59 when leading"
        );
        assert_eq!(timestamp("1m30s"), Some(90));
        assert_eq!(timestamp("2 min"), Some(120));
        assert_eq!(timestamp("0"), Some(0));
        for bad in [
            "", "1:60", "1:2:3:4", "1::2", ":30", "abc", "-5", "1.5:00", "2:15pm",
        ] {
            assert_eq!(timestamp(bad), None, "{bad:?}");
        }
        assert_eq!(clock(0), "0:00");
        assert_eq!(clock(135), "2:15");
        assert_eq!(clock(3723), "1:02:03");
    }

    #[test]
    fn parses_plain_text_times() {
        assert_eq!(minutes("45 minutes"), Some(45));
        assert_eq!(minutes("1 hr 30 mins"), Some(90));
        assert_eq!(minutes("1h30m"), Some(90));
        assert_eq!(minutes("2 Hours"), Some(120));
        assert_eq!(minutes("1.5 hours"), Some(90));
        assert_eq!(minutes("10"), None, "a bare number has no unit");
        assert_eq!(minutes("about an hour"), None);
        assert_eq!(minutes("5 days"), None);
    }
}
