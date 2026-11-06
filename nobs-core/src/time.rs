const DASH: &str = "—";

pub fn parse_rfc3339(text: &str) -> Option<i64> {
    let (date, rest) = text.split_once('T')?;
    let mut date = date.split('-');
    let (year, month, day) = (
        digits(date.next()?, 4)?,
        digits(date.next()?, 2)?,
        digits(date.next()?, 2)?,
    );
    let (clock, zone) = rest.split_at(rest.find(['Z', '+', '-'])?);
    let whole = match clock.split_once('.') {
        Some((whole, fraction))
            if !fraction.is_empty() && fraction.bytes().all(|byte| byte.is_ascii_digit()) =>
        {
            whole
        }
        Some(_) => return None,
        None => clock,
    };
    let mut parts = whole.split(':');
    let (hour, minute, second) = (
        digits(parts.next()?, 2)?,
        digits(parts.next()?, 2)?,
        digits(parts.next()?, 2)?,
    );
    if date.next().is_some() || parts.next().is_some() {
        return None;
    }
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    if hour > 23 || minute > 59 || second > 59 {
        return None;
    }
    let offset = match zone {
        "Z" => 0,
        _ => {
            let (sign, tail) = match zone.split_at(1) {
                ("+", tail) => (1, tail),
                ("-", tail) => (-1, tail),
                _ => return None,
            };
            let (hours, minutes) = tail.split_once(':')?;
            sign * (digits(hours, 2)? * 3600 + digits(minutes, 2)? * 60)
        }
    };
    Some(days_from_civil(year, month, day) * 86400 + hour * 3600 + minute * 60 + second - offset)
}

pub fn age(start: &str, now: i64) -> String {
    let Some(start) = parse_rfc3339(start) else {
        return DASH.to_string();
    };
    let seconds = now - start;
    if seconds < 60 {
        format!("{seconds}s")
    } else if seconds < 3600 {
        format!("{}m{:02}s", seconds / 60, seconds % 60)
    } else if seconds < 86400 {
        format!("{}h{:02}m", seconds / 3600, seconds % 3600 / 60)
    } else {
        format!("{}d{}h", seconds / 86400, seconds % 86400 / 3600)
    }
}

pub fn age_rough(start: &str, now: i64) -> String {
    let Some(start) = parse_rfc3339(start) else {
        return "unknown".to_string();
    };
    let minutes = (now - start).div_euclid(60);
    if minutes < 1 {
        "<1 min".to_string()
    } else if minutes < 60 {
        format!("~{minutes} min")
    } else {
        format!("~{}h{:02}m", minutes / 60, minutes % 60)
    }
}

pub fn remaining(end: &str, now: i64) -> String {
    let Some(end) = parse_rfc3339(end) else {
        return "?".to_string();
    };
    let left = end - now;
    if left > 0 {
        format!("{}m{}s", left / 60, left % 60)
    } else {
        DASH.to_string()
    }
}

pub fn clock(text: &str) -> String {
    let characters: Vec<char> = text.chars().collect();
    if characters.len() < 19 {
        return String::new();
    }
    characters[11..19].iter().collect()
}

fn digits(text: &str, length: usize) -> Option<i64> {
    if text.len() == length && text.bytes().all(|byte| byte.is_ascii_digit()) {
        text.parse().ok()
    } else {
        None
    }
}

fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let month_from_march = if month > 2 { month - 3 } else { month + 9 };
    let day_of_year = (153 * month_from_march + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146097 + day_of_era - 719468
}

pub(crate) fn rfc3339_utc(seconds: i64) -> String {
    let (year, month, day) = civil_from_days(seconds.div_euclid(86400));
    let of_day = seconds.rem_euclid(86400);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        of_day / 3600,
        of_day % 3600 / 60,
        of_day % 60
    )
}

fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let days = days + 719468;
    let era = days.div_euclid(146097);
    let day_of_era = days - era * 146097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36524 - day_of_era / 146096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_from_march = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_from_march + 2) / 5 + 1;
    let month = if month_from_march < 10 {
        month_from_march + 3
    } else {
        month_from_march - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_793_960_130;

    #[test]
    fn parse_rfc3339_reads_z_a_fraction_and_a_zero_offset_to_the_same_seconds() {
        assert_eq!(parse_rfc3339("2026-11-06T10:15:30.123Z"), Some(NOW));
        assert_eq!(parse_rfc3339("2026-11-06T10:15:30+00:00"), Some(NOW));
    }

    #[test]
    fn parse_rfc3339_applies_the_offset() {
        assert_eq!(parse_rfc3339("2026-11-06T10:15:30+02:00"), Some(NOW - 7200));
        assert_eq!(
            parse_rfc3339("2026-11-06T10:15:30-05:30"),
            Some(NOW + 19800)
        );
    }

    #[test]
    fn parse_rfc3339_refuses_text_that_is_not_rfc3339() {
        for text in [
            "soon",
            "",
            "2026-11-06",
            "2026-11-06T10:15:30",
            "2026-11-06T10:15:30+0200",
            "2026-11-06T10:15:30.Z",
            "2026-11-06T10:15Z",
            "2026-13-06T10:15:30Z",
            "2026-11-06T24:15:30Z",
            "2026-11-06T10:15:30Zjunk",
        ] {
            assert_eq!(parse_rfc3339(text), None, "{text}");
        }
    }

    #[test]
    fn age_prints_seconds_minutes_hours_and_days() {
        let start = "2026-11-06T10:15:30Z";
        assert_eq!(age(start, NOW + 42), "42s");
        assert_eq!(age(start, NOW + 185), "3m05s");
        assert_eq!(age(start, NOW + 7620), "2h07m");
        assert_eq!(age(start, NOW + 97200), "1d3h");
    }

    #[test]
    fn age_is_a_dash_for_an_unreadable_start_and_negative_seconds_for_a_future_one() {
        assert_eq!(age("soon", NOW), "—");
        assert_eq!(age("2026-11-06T10:15:35Z", NOW), "-5s");
    }

    #[test]
    fn age_rough_prints_under_a_minute_minutes_and_hours_with_two_digit_minutes() {
        let start = "2026-11-06T10:15:30Z";
        assert_eq!(age_rough(start, NOW + 30), "<1 min");
        assert_eq!(age_rough(start, NOW + 59), "<1 min");
        assert_eq!(age_rough(start, NOW + 60), "~1 min");
        assert_eq!(age_rough(start, NOW + 240), "~4 min");
        assert_eq!(age_rough(start, NOW + 3599), "~59 min");
        assert_eq!(age_rough(start, NOW + 3600), "~1h00m");
        assert_eq!(age_rough(start, NOW + 3720), "~1h02m");
        assert_eq!(age_rough(start, NOW + 90000), "~25h00m");
    }

    #[test]
    fn age_rough_is_unknown_for_an_empty_or_unreadable_start_and_under_a_minute_for_a_future_one() {
        assert_eq!(age_rough("", NOW), "unknown");
        assert_eq!(age_rough("soon", NOW), "unknown");
        assert_eq!(age_rough("2026-11-06T10:15:35Z", NOW), "<1 min");
    }

    #[test]
    fn rfc3339_utc_prints_whole_seconds_and_reads_back_through_parse_rfc3339() {
        assert_eq!(rfc3339_utc(NOW), "2026-11-06T10:15:30Z");
        for text in [
            "1970-01-01T00:00:00Z",
            "1999-12-31T23:59:59Z",
            "2000-02-29T12:00:00Z",
            "2024-02-29T23:59:59Z",
            "2026-03-01T00:00:00Z",
        ] {
            assert_eq!(rfc3339_utc(parse_rfc3339(text).unwrap()), text);
        }
    }

    #[test]
    fn remaining_counts_minutes_and_seconds_until_the_end_and_flags_the_rest() {
        let end = "2026-11-06T10:15:30Z";
        assert_eq!(remaining(end, NOW - 125), "2m5s");
        assert_eq!(remaining(end, NOW), "—");
        assert_eq!(remaining(end, NOW + 1), "—");
        assert_eq!(remaining("soon", NOW), "?");
    }

    #[test]
    fn clock_gives_the_time_part() {
        assert_eq!(clock("2026-11-06T10:15:30.123Z"), "10:15:30");
    }

    #[test]
    fn clock_is_empty_when_the_text_is_short_and_cuts_by_character() {
        assert_eq!(clock("2026-11-06T10:15"), "");
        assert_eq!(clock("ééééééééééé10:15:30"), "10:15:30");
    }
}
