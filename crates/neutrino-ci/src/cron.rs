//! Five-field cron lines, read in UTC as Actions does:
//! `minute hour day-of-month month day-of-week`, each a `*`, a number, a
//! range `a-b`, a step `*/n` or `a-b/n`, or a list of those; months and days
//! may be names (`JAN`, `MON`). When both day fields are restricted, a day
//! matches if either does.

use crate::Error;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cron {
    minutes: u64,
    hours: u64,
    days: u64,
    months: u64,
    weekdays: u64,
    any_day: bool,
    any_weekday: bool,
}

/// A moment broken into the fields cron reads, in UTC.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Moment {
    pub minute: u32,
    pub hour: u32,
    pub day: u32,
    pub month: u32,
    /// 0 is Sunday.
    pub weekday: u32,
    pub year: i32,
}

impl Moment {
    /// From Unix seconds.
    pub fn utc(secs: i64) -> Moment {
        let days = secs.div_euclid(86_400);
        let rem = secs.rem_euclid(86_400);
        // Civil from days (Howard Hinnant's algorithm).
        let z = days + 719_468;
        let era = z.div_euclid(146_097);
        let doe = z.rem_euclid(146_097);
        let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
        let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
        let year = (yoe + era * 400 + i64::from(month <= 2)) as i32;
        Moment { minute: ((rem / 60) % 60) as u32, hour: (rem / 3600) as u32, day, month, weekday: (days + 4).rem_euclid(7) as u32, year }
    }
}

const MONTHS: [&str; 12] = ["JAN", "FEB", "MAR", "APR", "MAY", "JUN", "JUL", "AUG", "SEP", "OCT", "NOV", "DEC"];
const DAYS: [&str; 7] = ["SUN", "MON", "TUE", "WED", "THU", "FRI", "SAT"];

fn value(s: &str, names: &[&str], offset: u32) -> Option<u32> {
    s.parse().ok().or_else(|| names.iter().position(|n| n.eq_ignore_ascii_case(s)).map(|i| i as u32 + offset))
}

fn field(src: &str, lo: u32, hi: u32, names: &[&str], name_offset: u32, what: &str) -> Result<(u64, bool), Error> {
    let mut bits = 0u64;
    let any = src == "*";
    for part in src.split(',') {
        let (range, step) = match part.split_once('/') {
            Some((r, s)) => (r, s.parse::<u32>().ok().filter(|s| *s > 0).ok_or_else(|| Error::new(format!("bad step in the {what} field: {part}")))?),
            None => (part, 1),
        };
        let (a, b) = if range == "*" {
            (lo, hi)
        } else if let Some((a, b)) = range.split_once('-') {
            let a = value(a, names, name_offset).ok_or_else(|| Error::new(format!("bad {what}: {a}")))?;
            let b = value(b, names, name_offset).ok_or_else(|| Error::new(format!("bad {what}: {b}")))?;
            (a, b)
        } else {
            let a = value(range, names, name_offset).ok_or_else(|| Error::new(format!("bad {what}: {range}")))?;
            (a, if part.contains('/') { hi } else { a })
        };
        if a < lo || b > hi || a > b {
            return Err(Error::new(format!("the {what} field {part} is outside {lo}-{hi}")));
        }
        let mut v = a;
        while v <= b {
            bits |= 1 << v;
            v += step;
        }
    }
    Ok((bits, any))
}

impl Cron {
    pub fn parse(line: &str) -> Result<Cron, Error> {
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.len() != 5 {
            return Err(Error::new(format!("{line:?} isn't a five-field cron line (minute hour day month weekday)")));
        }
        let (minutes, _) = field(f[0], 0, 59, &[], 0, "minute")?;
        let (hours, _) = field(f[1], 0, 23, &[], 0, "hour")?;
        let (days, any_day) = field(f[2], 1, 31, &[], 0, "day")?;
        let (months, _) = field(f[3], 1, 12, &MONTHS, 1, "month")?;
        let (mut weekdays, any_weekday) = field(f[4], 0, 7, &DAYS, 0, "weekday")?;
        // 7 is Sunday too.
        if weekdays & (1 << 7) != 0 {
            weekdays |= 1;
        }
        Ok(Cron { minutes, hours, days, months, weekdays, any_day, any_weekday })
    }

    pub fn matches(&self, m: Moment) -> bool {
        let bit = |set: u64, v: u32| set & (1 << v) != 0;
        if !bit(self.minutes, m.minute) || !bit(self.hours, m.hour) || !bit(self.months, m.month) {
            return false;
        }
        match (self.any_day, self.any_weekday) {
            (true, true) => true,
            (false, true) => bit(self.days, m.day),
            (true, false) => bit(self.weekdays, m.weekday),
            (false, false) => bit(self.days, m.day) || bit(self.weekdays, m.weekday),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn moments() {
        // 2026-11-14 21:05 UTC was a Saturday.
        let m = Moment::utc(1_794_690_300);
        assert_eq!((m.year, m.day, m.month, m.hour, m.minute, m.weekday), (2026, 14, 11, 21, 5, 6));
        let epoch = Moment::utc(0);
        assert_eq!((epoch.year, epoch.day, epoch.month, epoch.weekday), (1970, 1, 1, 4), "1970-01-01 was a Thursday");
        assert_eq!(Moment::utc(951_782_400).day, 29, "2000-02-29");
    }

    #[test]
    fn lines() {
        let at = |s: &str| Moment::utc(s.parse::<i64>().unwrap());
        let nightly = Cron::parse("0 3 * * *").unwrap();
        assert!(nightly.matches(Moment { minute: 0, hour: 3, day: 9, month: 5, weekday: 2, year: 2026 }));
        assert!(!nightly.matches(Moment { minute: 1, hour: 3, day: 9, month: 5, weekday: 2, year: 2026 }));
        let every15 = Cron::parse("*/15 9-17 * * MON-FRI").unwrap();
        assert!(every15.matches(Moment { minute: 45, hour: 17, day: 1, month: 1, weekday: 5, year: 2026 }));
        assert!(!every15.matches(Moment { minute: 45, hour: 17, day: 1, month: 1, weekday: 6, year: 2026 }), "not on Saturday");
        assert!(!every15.matches(Moment { minute: 50, hour: 10, day: 1, month: 1, weekday: 1, year: 2026 }));
        let either = Cron::parse("0 0 1 * SUN").unwrap();
        assert!(either.matches(Moment { minute: 0, hour: 0, day: 1, month: 3, weekday: 3, year: 2026 }), "the 1st");
        assert!(either.matches(Moment { minute: 0, hour: 0, day: 8, month: 3, weekday: 0, year: 2026 }), "or a Sunday");
        assert!(Cron::parse("0 0 * * 7").unwrap().matches(Moment { minute: 0, hour: 0, day: 8, month: 3, weekday: 0, year: 2026 }), "7 is Sunday");
        assert!(Cron::parse("30 8 * JAN,jul *").unwrap().matches(Moment { minute: 30, hour: 8, day: 2, month: 7, weekday: 1, year: 2026 }));
        let _ = at("0");
        for bad in ["* * * *", "60 * * * *", "* 24 * * *", "*/0 * * * *", "5-1 * * * *", "* * * FOO *"] {
            assert!(Cron::parse(bad).is_err(), "{bad}");
        }
    }
}
