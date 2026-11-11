//! Times written the same way everywhere: "2 min ago", "yesterday", "12:04", "7.5 s".

/// How long ago something happened, given the seconds since then.
pub fn ago(secs: u64) -> String {
    const MIN: u64 = 60;
    const HOUR: u64 = 60 * MIN;
    const DAY: u64 = 24 * HOUR;
    match secs {
        0..10 => "just now".to_owned(),
        10..MIN => format!("{secs} s ago"),
        MIN..HOUR => format!("{} min ago", secs / MIN),
        HOUR..DAY => format!("{} h ago", secs / HOUR),
        DAY..172_800 => "yesterday".to_owned(),
        _ if secs < 60 * DAY => format!("{} days ago", secs / DAY),
        _ if secs < 730 * DAY => format!("{} months ago", secs / (30 * DAY)),
        _ => format!("{} years ago", secs / (365 * DAY)),
    }
}

/// A length as "4:05" or "1:02:09" (hours only when needed).
pub fn duration(secs: u64) -> String {
    let (h, m, s) = (secs / 3600, secs / 60 % 60, secs % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// How long something short lasts: "7.5 s" under a minute (a tenth of a
/// second matters for an intro), then as [`duration`].
pub fn length(seconds: f64) -> String {
    let seconds = seconds.max(0.0);
    if seconds < 59.95 {
        format!("{seconds:.1} s")
    } else {
        duration(seconds.round() as u64)
    }
}

/// Roughly how long background work still takes, in steps coarse enough
/// that the wording doesn't flicker: "a few seconds left", "about 20 s
/// left", "about 3 min left".
pub fn time_left(seconds: f32) -> String {
    if !seconds.is_finite() || seconds < 8.0 {
        "a few seconds left".to_owned()
    } else if seconds < 55.0 {
        format!("about {} s left", ((seconds / 10.0).round() as u32).max(1) * 10)
    } else {
        format!("about {} min left", ((seconds / 60.0).round() as u32).max(1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ago_reads_naturally() {
        assert_eq!(ago(3), "just now");
        assert_eq!(ago(42), "42 s ago");
        assert_eq!(ago(125), "2 min ago");
        assert_eq!(ago(3 * 3600), "3 h ago");
        assert_eq!(ago(30 * 3600), "yesterday");
        assert_eq!(ago(5 * 86_400), "5 days ago");
        assert_eq!(ago(90 * 86_400), "3 months ago");
        assert_eq!(ago(800 * 86_400), "2 years ago");
    }

    #[test]
    fn short_lengths_show_tenths() {
        assert_eq!(length(7.5), "7.5 s");
        assert_eq!(length(0.04), "0.0 s");
        assert_eq!(length(59.96), "1:00");
        assert_eq!(length(83.4), "1:23");
        assert_eq!(length(-1.0), "0.0 s");
    }

    #[test]
    fn time_left_is_told_in_coarse_steps() {
        assert_eq!(time_left(3.0), "a few seconds left");
        assert_eq!(time_left(8.0), "about 10 s left");
        assert_eq!(time_left(21.0), "about 20 s left");
        assert_eq!(
            time_left(24.0),
            "about 20 s left",
            "a few seconds more or less read the same"
        );
        assert_eq!(time_left(54.0), "about 50 s left");
        assert_eq!(time_left(70.0), "about 1 min left");
        assert_eq!(time_left(200.0), "about 3 min left");
        assert_eq!(time_left(f32::NAN), "a few seconds left");
    }

    #[test]
    fn durations_show_hours_only_when_needed() {
        assert_eq!(duration(0), "0:00");
        assert_eq!(duration(245), "4:05");
        assert_eq!(duration(3729), "1:02:09");
    }
}
