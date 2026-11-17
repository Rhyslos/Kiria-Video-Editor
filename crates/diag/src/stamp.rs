//! When something happened, as reports and markers write it: the moment in
//! UTC to the second, `2026-11-17T11:08:57Z`.

use std::time::SystemTime;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

/// Now, as reports and markers write it.
pub fn now() -> String {
    at(SystemTime::now())
}

/// `moment` in UTC to the second. A moment the formatter can't write (out
/// of its years) is written as the seconds since 1970 instead.
pub fn at(moment: SystemTime) -> String {
    let utc = OffsetDateTime::from(moment).replace_nanosecond(0);
    match utc.ok().and_then(|t| t.format(&Rfc3339).ok()) {
        Some(text) => text,
        None => {
            let seconds = moment
                .duration_since(SystemTime::UNIX_EPOCH)
                .map_or(0, |d| d.as_secs());
            format!("{seconds} s after 1970")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn written_in_utc_to_the_second() {
        let moment = SystemTime::UNIX_EPOCH + Duration::from_millis(1_794_913_737_250);
        assert_eq!(at(moment), "2026-11-17T11:08:57Z");
        assert_eq!(at(SystemTime::UNIX_EPOCH), "1970-01-01T00:00:00Z");
    }
}
