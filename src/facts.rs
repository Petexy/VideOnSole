//! Turning what the filesystem and the container know into what a person
//! reads.
//!
//! Four small conversions, kept together because each of them is the sort that
//! is written slightly differently in two places and then disagrees: how large
//! a file is, how long a film runs, when it was written, and how big its
//! picture is.

use std::time::SystemTime;

/// A file's length, in the units somebody would say it in.
///
/// Powers of two under names of powers of ten, which is what every file
/// manager on this machine does; being right about it here and different from
/// everything else beside it would be its own kind of wrong.
pub fn size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else if value < 10.0 {
        lxb_toolkit::i18n::decimal(format!("{value:.1} {}", UNITS[unit]))
    } else {
        format!("{value:.0} {}", UNITS[unit])
    }
}

/// A position or a length in a film, as a clock reads it.
///
/// **The width follows the film, not the number**: an hour-long film shows
/// `1:04:12` from the first second to the last, and a short one shows `4:12`
/// throughout. A clock that grew a field part way through would move every
/// digit beside it, and on a groove that is a row of numbers twitching under a
/// film nobody was reading them for. So the length is what decides the shape
/// and the position is only fitted into it — which is why this takes both.
///
/// Negative is nought: a position measured against a clock that has not been
/// set yet is not a time before the film.
pub fn clock(seconds: f64, of_length: f64) -> String {
    let whole = if seconds.is_finite() && seconds > 0.0 {
        seconds as u64
    } else {
        0
    };
    let longest = if of_length.is_finite() && of_length > 0.0 {
        of_length as u64
    } else {
        whole
    };
    let (hours, minutes, rest) = (whole / 3600, (whole % 3600) / 60, whole % 60);
    if longest >= 3600 {
        format!("{hours}:{minutes:02}:{rest:02}")
    } else {
        format!("{}:{rest:02}", whole / 60)
    }
}

/// How long a film runs, said as a person would say it rather than as a clock.
///
/// For the details pane and the card, where the question is "is this an evening
/// or ten minutes" rather than "how far in am I". `1 h 47 min`, not `1:47:03`.
pub fn length(seconds: f64) -> String {
    if !seconds.is_finite() || seconds <= 0.0 {
        return String::new();
    }
    let whole = seconds.round() as u64;
    let (hours, minutes) = (whole / 3600, (whole % 3600) / 60);
    if hours > 0 {
        format!("{hours} h {minutes:02} min")
    } else if minutes > 0 {
        format!("{minutes} min")
    } else {
        format!("{whole} s")
    }
}

/// How many pixels across and down, with the name somebody would use for it.
///
/// The name and not only the number, because *1080p* is what is written on the
/// file, on the box and in the conversation, and `1920 × 1080` is what the
/// decoder happens to have said. Matched on the height and only where the
/// shape is near enough the ordinary one — a 1920×816 letterboxed film is
/// still 1080p to everybody, and a 1920×1920 square is not.
pub fn picture(width: u32, height: u32) -> String {
    let plain = format!("{width} × {height}");
    match named_resolution(width, height) {
        Some(name) => format!("{plain}  ·  {name}"),
        None => plain,
    }
}

fn named_resolution(width: u32, height: u32) -> Option<&'static str> {
    if width == 0 || height == 0 {
        return None;
    }
    // Each standard's own width, matched against the film's *longer* edge.
    //
    // The longer edge and not the height, because letterboxing takes height
    // away and never adds it: a 1920×816 film is a 1080p film with black bars
    // that were never encoded, and asking its height would call it 720p. The
    // same rule reads a telephone's portrait recording correctly, which is a
    // 1080p film stood on its end.
    const BY_WIDTH: [(u32, &str); 6] = [
        (7680, "8K"),
        (3840, "4K"),
        (2560, "1440p"),
        (1920, "1080p"),
        (1280, "720p"),
        (854, "480p"),
    ];
    let across = width.max(height);
    BY_WIDTH
        .iter()
        .find(|(wide, _)| across >= *wide)
        .map(|(_, name)| *name)
}

/// When a file was last written, as a date and a time.
///
/// In UTC, deliberately and only because the alternative is reading the zone
/// database, and a browser that shipped its own half-right idea of local time
/// would be wrong in a way nobody could see. The date is what is being asked
/// for here — which of two recordings this is — and that is the same date
/// either way for all but a few hours of it.
pub fn when(time: SystemTime) -> String {
    let Ok(since) = time.duration_since(SystemTime::UNIX_EPOCH) else {
        return String::from(crate::i18n::text("before-1970"));
    };
    let seconds = since.as_secs();
    let days = (seconds / 86_400) as i64;
    let rest = seconds % 86_400;
    let (year, month, day) = civil(days);
    crate::message!("file-date", "day" => day.to_string(),
        "month" => lxb_toolkit::i18n::month(month as usize), "year" => year.to_string(),
        // The clock the session is set to, which is a setting rather than a
        // language: Settings > System > Clock, read out of the shell's own
        // file. See `lxb_toolkit::settings::twelve_hour_clock`.
        "time" => lxb_toolkit::i18n::time_of_day((rest / 3600) as u32, ((rest % 3600) / 60) as u32))
}

/// Days since 1970 to a calendar date.
///
/// Howard Hinnant's `civil_from_days`, which is the standard way of doing this
/// without a table: shift the era so that March is the first month and the
/// leap day falls at the end of the cycle, where it stops being a special
/// case.
fn civil(days: i64) -> (i64, u32, u32) {
    let days = days + 719_468;
    let era = if days >= 0 { days } else { days - 146_096 } / 146_097;
    let day_of_era = (days - era * 146_097) as u64;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era as i64 + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * shifted + 2) / 5 + 1) as u32;
    let month = if shifted < 10 {
        shifted + 3
    } else {
        shifted - 9
    } as u32;
    (if month <= 2 { year + 1 } else { year }, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// The figures the lines below are written with, carrying whatever
    /// decimal mark the session's language uses — a comma in Polish. Asked
    /// for the same way `size` asks for it, because what this test is about
    /// is the rounding and the units, and a run under a Polish session that
    /// failed on a full stop would be failing about the wrong thing.
    fn as_written(number: &str) -> String {
        lxb_toolkit::i18n::decimal(number.to_string())
    }

    /// The same for a date, which is a sentence the catalog builds.
    fn date(day: u32, month: usize, year: i64, time: &str) -> String {
        crate::message!("file-date", "day" => day.to_string(),
            "month" => lxb_toolkit::i18n::month(month), "year" => year.to_string(),
            "time" => time.to_string())
    }

    #[test]
    fn sizes_read_the_way_a_file_manager_says_them() {
        assert_eq!(size(0), "0 B");
        assert_eq!(size(999), "999 B");
        assert_eq!(size(1024), as_written("1.0 KB"));
        assert_eq!(
            size(1024 * 1024 * 1024 * 4 + 1024 * 1024 * 512),
            as_written("4.5 GB")
        );
    }

    /// And what the session's language makes of the same number, which is the
    /// half the line above cannot state. The two Englishes, Hindi and Chinese
    /// keep the full stop; the six others write a comma. It follows the
    /// language and not the country — see `lxb_toolkit::i18n::decimal`.
    #[test]
    fn a_quantity_carries_the_decimal_mark_of_the_language() {
        assert_eq!(
            lxb_toolkit::i18n::decimal("4.5 GB".to_string()),
            match lxb_toolkit::i18n::language() {
                "en-GB" | "en-US" | "hi" | "zh-CN" => "4.5 GB",
                _ => "4,5 GB",
            }
        );
    }

    /// The one that matters on the groove: the shape is the film's, so nothing
    /// under it moves as the film plays.
    #[test]
    fn a_clock_keeps_the_shape_the_film_gave_it() {
        // A long film: an hour field from the first second.
        assert_eq!(clock(0.0, 5000.0), "0:00:00");
        assert_eq!(clock(63.0, 5000.0), "0:01:03");
        assert_eq!(clock(3723.0, 5000.0), "1:02:03");
        // A short one never grows the field.
        assert_eq!(clock(0.0, 300.0), "0:00");
        assert_eq!(clock(63.0, 300.0), "1:03");
        assert_eq!(clock(599.0, 600.0), "9:59");
    }

    #[test]
    fn a_position_before_the_film_started_is_the_start_of_it() {
        assert_eq!(clock(-4.0, 300.0), "0:00");
        assert_eq!(clock(f64::NAN, 300.0), "0:00");
    }

    /// With no length to measure against, the position sets its own shape —
    /// which is what a film whose container declares no duration gets.
    #[test]
    fn a_clock_with_no_film_behind_it_fits_itself() {
        assert_eq!(clock(3723.0, 0.0), "1:02:03");
        assert_eq!(clock(63.0, 0.0), "1:03");
    }

    #[test]
    fn a_length_is_said_the_way_somebody_would_say_it() {
        assert_eq!(length(6420.0), "1 h 47 min");
        assert_eq!(length(1500.0), "25 min");
        assert_eq!(length(42.0), "42 s");
        assert_eq!(length(0.0), "");
    }

    #[test]
    fn a_film_is_named_by_the_format_it_was_made_in() {
        assert!(picture(1920, 1080).contains("1080p"));
        assert!(picture(3840, 2160).contains("4K"));
        // Letterboxed, and still 1080p to everybody who has ever said the word.
        assert!(picture(1920, 816).contains("1080p"));
        // And a shape nothing is named for is only its numbers.
        assert_eq!(picture(320, 200), "320 × 200");
    }

    /// Midnight, and it writes the hour the way the shell's own clock does —
    /// without a leading zero. One `clock-24-hour` in the toolkit's catalogs
    /// answers for the corner of the start screen, the guide and this line
    /// alike, so there is one place to change it and no way for the three to
    /// drift apart.
    #[test]
    fn the_epoch_is_the_first_of_january() {
        assert_eq!(when(SystemTime::UNIX_EPOCH), date(1, 1, 1970, "0:00"));
    }

    /// The date each language writes, named rather than taken off this
    /// machine: Polish puts the month in the genitive.
    #[test]
    fn a_date_is_written_the_way_each_language_writes_one() {
        let catalog = crate::i18n::Catalog::new(crate::i18n::RESOURCES);
        let months = lxb_toolkit::i18n::Catalog::new(lxb_toolkit::i18n::RESOURCES);
        let written = |locale: &str| {
            let mut args = crate::i18n::FluentArgs::new();
            args.set("day", "29");
            args.set(
                "month",
                months.text_for(locale, "month-february").to_string(),
            );
            args.set("year", "2024");
            args.set("time", "12:00");
            catalog.format_for(locale, "file-date", &args)
        };
        assert_eq!(written("en-GB"), "29 February 2024, 12:00");
        assert_eq!(written("pl"), "29 lutego 2024, 12:00");
        // America puts the month first, which is the whole of `en-US.ftl`.
        assert_eq!(written("en-US"), "February 29, 2024, 12:00");
        // And an English that is neither reads out of the British catalog.
        assert_eq!(written("en_AU.UTF-8"), "29 February 2024, 12:00");
        assert_eq!(written("fr"), "29 février 2024, 12:00");
        // Spanish fences the month with *de* on both sides, which a separator
        // and three values could not have written; Portuguese does the same.
        assert_eq!(written("es"), "29 de febrero de 2024, 12:00");
        assert_eq!(written("pt_BR"), "29 de fevereiro de 2024, 12:00");
        // German and Russian point the day, and Russian's month is in the
        // genitive as Polish's is.
        assert_eq!(written("de_AT.UTF-8"), "29. Februar 2024, 12:00");
        assert_eq!(written("ru"), "29 февраля 2024 г., 12:00");
        assert_eq!(written("hi"), "29 फ़रवरी 2024, 12:00");
        // Chinese goes largest first, and the month's name is its number and
        // a character — so the whole date is one message here as well.
        assert_eq!(written("zh_CN.UTF-8"), "2024年2月29日 12:00");
    }

    #[test]
    fn a_leap_day_is_a_leap_day() {
        // 2024-02-29T12:00:00Z
        let time = SystemTime::UNIX_EPOCH + Duration::from_secs(1_709_208_000);
        assert_eq!(when(time), date(29, 2, 2024, "12:00"));
    }

    #[test]
    fn a_century_that_is_not_a_leap_year() {
        // 1900-03-01 is day -25508 from the epoch; 1900 was not a leap year.
        assert_eq!(civil(-25_508), (1900, 3, 1));
        assert_eq!(civil(-25_509), (1900, 2, 28));
    }
}
