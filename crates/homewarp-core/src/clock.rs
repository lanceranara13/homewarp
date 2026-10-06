//! Dates, and the times a schedule names (PLAN.md §11, Phase 4). Nothing here
//! knows of time zones: a moment is Unix seconds, and whoever wants it on a
//! clock other than UTC's says how far east of UTC that clock is.

const DAY: i64 = 86_400;

/// The year, month and day of a day counted from 1970-01-01, by Howard
/// Hinnant's `civil_from_days`: years are counted from March, so that the day
/// a leap year adds is the last one.
pub(crate) fn civil(days: i64) -> (i64, u32, u32) {
    let days = days + 719_468;
    let (era, day) = (days.div_euclid(146_097), days.rem_euclid(146_097));
    let year = (day - day / 1_460 + day / 36_524 - day / 146_096) / 365;
    let day = day - (365 * year + year / 4 - year / 100);
    let month = (5 * day + 2) / 153;
    let day = day - (153 * month + 2) / 5 + 1;
    let month = if month < 10 { month + 3 } else { month - 9 };
    let year = year + era * 400 + i64::from(month <= 2);
    (year, month as u32, day as u32)
}

/// The other way about: the day counted from 1970-01-01 that a year, month and
/// day name, by the same author's `days_from_civil`.
pub(crate) fn days(year: i64, month: i64, day: i64) -> i64 {
    let year = year - i64::from(month <= 2);
    let (era, year) = (year.div_euclid(400), year.rem_euclid(400));
    let month = if month > 2 { month - 3 } else { month + 9 };
    let day = (153 * month + 2) / 5 + day - 1;
    era * 146_097 + year * 365 + year / 4 - year / 100 + day - 719_468
}

/// Times that come round, as cron writes them: five fields, for the minute,
/// the hour, the day of the month, the month and the day of the week. Each is
/// `*`, a number, a range `a-b`, any of those with a step `/n`, or several of
/// them with commas between. Sunday is 0, and 7 as well.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Cron {
    /// A bit for each value a field lets through.
    minutes: u64,
    hours: u64,
    days: u64,
    months: u64,
    weekdays: u64,
    /// Whether the day of the month, and of the week, was left as `*`. Cron
    /// takes a day that either field names when both name some.
    any_day: bool,
    any_weekday: bool,
}

/// Reads one field, whose values run from `least` to `most`.
fn field(text: &str, least: u32, most: u32, called: &str) -> Result<u64, String> {
    let wrong = || format!("The {called} is {least} to {most}, and \"{text}\" is not that");
    let number = |digits: &str| digits.parse::<u32>().map_err(|_| wrong());
    let mut bits = 0;
    for part in text.split(',') {
        let (span, step) = match part.split_once('/') {
            Some((span, step)) => (span, number(step)?),
            None => (part, 1),
        };
        let (from, to) = match span.split_once('-') {
            _ if span == "*" => (least, most),
            Some((from, to)) => (number(from)?, number(to)?),
            // "5/15" is from 5 on, every fifteenth.
            None if part.contains('/') => (number(span)?, most),
            None => (number(span)?, number(span)?),
        };
        if step == 0 || from < least || to > most || from > to {
            return Err(wrong());
        }
        for value in (from..=to).step_by(step as usize) {
            bits |= 1 << value;
        }
    }
    Ok(bits)
}

impl Cron {
    /// Reads the five fields. The error is a sentence about the field that will not do.
    pub(crate) fn parse(text: &str) -> Result<Self, String> {
        let fields: Vec<&str> = text.split_whitespace().collect();
        let &[minute, hour, day, month, weekday] = fields.as_slice() else {
            return Err(
                "A schedule's time is five fields: minute, hour, day, month and weekday".to_owned(),
            );
        };
        let weekdays = field(weekday, 0, 7, "weekday")?;
        Ok(Self {
            minutes: field(minute, 0, 59, "minute")?,
            hours: field(hour, 0, 23, "hour")?,
            days: field(day, 1, 31, "day of the month")?,
            months: field(month, 1, 12, "month")?,
            // Sunday by either of its numbers.
            weekdays: (weekdays | (weekdays >> 7)) & 0x7f,
            any_day: day == "*",
            any_weekday: weekday == "*",
        })
    }

    /// The first of these times after `after`, read on a clock that is
    /// `offset_minutes` east of UTC's. None if eight years hold none: the
    /// 30th of February is asked for more often than it comes.
    pub(crate) fn next(&self, after: i64, offset_minutes: i32) -> Option<i64> {
        let offset = i64::from(offset_minutes) * 60;
        // The next whole minute on that clock.
        let start = (after + offset).div_euclid(60) * 60 + 60;
        let first_day = start.div_euclid(DAY);
        for day in first_day..first_day + 8 * 366 {
            if !self.on(day) {
                continue;
            }
            // On the day it starts on, only what is left of the day counts.
            let from = if day == first_day {
                start.rem_euclid(DAY) / 60
            } else {
                0
            };
            let minute = (from..24 * 60).find(|minute| {
                self.hours >> (minute / 60) & 1 == 1 && self.minutes >> (minute % 60) & 1 == 1
            });
            if let Some(minute) = minute {
                return Some(day * DAY + minute * 60 - offset);
            }
        }
        None
    }

    /// Whether a day, counted from 1970-01-01, is one of the days named.
    fn on(&self, day: i64) -> bool {
        let (_, month, of_month) = civil(day);
        // 1970-01-01 was a Thursday.
        let weekday = (day + 4).rem_euclid(7);
        let by_date = self.days >> of_month & 1 == 1;
        let by_weekday = self.weekdays >> weekday & 1 == 1;
        let named = match (self.any_day, self.any_weekday) {
            (false, false) => by_date || by_weekday,
            _ => by_date && by_weekday,
        };
        named && self.months >> month & 1 == 1
    }
}

#[cfg(test)]
mod tests {
    use super::{Cron, civil};

    /// 2026-10-06 15:30:45 UTC, a Tuesday.
    const NOW: i64 = 1_791_300_645;
    const HOUR: i64 = 3600;

    fn next(cron: &str, after: i64, offset_minutes: i32) -> Option<i64> {
        Cron::parse(cron).unwrap().next(after, offset_minutes)
    }

    #[test]
    fn a_day_count_is_a_date() {
        assert_eq!(civil(0), (1970, 1, 1));
        assert_eq!(civil(11_016), (2000, 2, 29));
        assert_eq!(civil(19_782), (2024, 2, 29));
        assert_eq!(civil(20_732), (2026, 10, 6));
        assert_eq!(civil(-1), (1969, 12, 31));
    }

    #[test]
    fn a_time_that_comes_round_has_a_next_one() {
        let start_of_day = NOW - NOW % 86_400;
        // Every minute: the next whole one.
        assert_eq!(next("* * * * *", NOW, 0), Some(NOW + 15));
        // Every quarter of an hour, on the quarter.
        assert_eq!(
            next("*/15 * * * *", NOW, 0),
            Some(start_of_day + 15 * HOUR + 45 * 60)
        );
        // Four in the morning: tomorrow's, as today's is past.
        assert_eq!(next("0 4 * * *", NOW, 0), Some(start_of_day + 28 * HOUR));
        // On a clock eight hours ahead it is 23:30, and four in the morning
        // there is in four and a half hours.
        assert_eq!(next("0 4 * * *", NOW, 480), Some(start_of_day + 20 * HOUR));
        // And on one five hours behind it is 10:30, with noon to come today.
        assert_eq!(
            next("0 12 * * *", NOW, -300),
            Some(start_of_day + 17 * HOUR)
        );
        // A time that is now is not after now.
        assert_eq!(
            next("30 15 * * *", NOW, 0),
            Some(start_of_day + 24 * HOUR + 15 * HOUR + 30 * 60)
        );

        // Sunday, by either of its numbers: the 11th.
        let sunday = start_of_day + 5 * 86_400;
        assert_eq!(next("0 0 * * 0", NOW, 0), Some(sunday));
        assert_eq!(next("0 0 * * 7", NOW, 0), Some(sunday));
        assert_eq!(next("0 0 * * 6,7", NOW, 0), Some(sunday - 86_400));
        // The first of the month, and the first of a month that is named.
        assert_eq!(next("0 0 1 * *", NOW, 0), Some(start_of_day + 26 * 86_400));
        assert_eq!(next("0 0 1 1 *", NOW, 0), Some(start_of_day + 87 * 86_400));
        // A day of the month and a day of the week together: whichever comes first.
        assert_eq!(next("0 0 1 * 0", NOW, 0), Some(sunday));
        // Lists, ranges and steps in one field.
        assert_eq!(
            next("5,10-12,40/10 16 * * *", NOW, 0),
            Some(start_of_day + 16 * HOUR + 5 * 60)
        );
        assert_eq!(
            next("5,10-12,40/10 15 * * *", NOW, 0),
            Some(start_of_day + 15 * HOUR + 40 * 60)
        );
        // The 29th of February comes, the 30th never does.
        assert_eq!(
            civil(next("0 0 29 2 *", NOW, 0).unwrap() / 86_400),
            (2028, 2, 29)
        );
        assert_eq!(next("0 0 30 2 *", NOW, 0), None);
    }

    #[test]
    fn what_is_not_five_sound_fields_is_refused_in_words() {
        for wrong in [
            "",
            "* * * *",
            "* * * * * *",
            "60 * * * *",
            "* 24 * * *",
            "* * 0 * *",
            "* * * 13 *",
            "* * * * 8",
            "*/0 * * * *",
            "5-1 * * * *",
            "a * * * *",
            "1,,2 * * * *",
            "-1 * * * *",
        ] {
            assert!(Cron::parse(wrong).is_err(), "{wrong:?}");
        }
        assert_eq!(
            Cron::parse("61 * * * *").unwrap_err(),
            "The minute is 0 to 59, and \"61\" is not that"
        );
    }
}
