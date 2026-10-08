//! Lining log B up with A: the offset that moves B's times onto A's
//! clock, worked out from the two logs at their arming, at their start or
//! by the wall clock, and what the row above the plot says about it. A
//! time `t` of B's is drawn at `t + offset` on A's clock.

use std::ops::RangeInclusive;

use crate::model::LoadedLog;
use crate::settings::Align;

/// How B lines up with A.
#[derive(Debug, Clone, PartialEq)]
pub struct Lineup {
    /// Seconds added to B's times to put them on A's clock.
    pub offset: f64,
    /// The alignment that took effect: the one chosen, or the one it fell
    /// back to.
    pub took: Align,
    /// The two times put together, each on its own log's clock, A's then
    /// B's: the armings or the starts, or what stood in for them. None by
    /// UTC, and with nothing to line up.
    pub at: Option<(f64, f64)>,
    /// What stood in for what was asked, or why it did not hold.
    pub note: Option<String>,
}

/// How `b` lines up with `a` under `align`. At arming, a log with no
/// arming lines up at its start, and the note names it; by UTC, a pair
/// without a GPS time in both falls back to at arming, and the note says
/// why. A log with no timed record lines up at no offset.
#[must_use]
pub fn line_up(a: &LoadedLog, b: &LoadedLog, align: Align) -> Lineup {
    let start = |log: &LoadedLog| log.time_range().map(|range| *range.start());
    match align {
        Align::Utc => {
            if let (Some(boot_a), Some(boot_b)) = (a.boot_unix_ms(), b.boot_unix_ms()) {
                return Lineup {
                    offset: (boot_b - boot_a) as f64 / 1000.0,
                    took: Align::Utc,
                    at: None,
                    note: None,
                };
            }
            let missing = which(a.time_base.is_none(), b.time_base.is_none());
            let mut lineup = line_up(a, b, Align::Arming);
            // lined up at nothing, the lineup says so by itself
            if lineup.at.is_some() {
                let why = format!("No GPS time in {missing}; lined up at arming");
                lineup.note = Some(match lineup.note {
                    Some(more) => format!("{why}. {more}"),
                    None => why,
                });
            }
            lineup
        }
        Align::Arming => {
            let (Some(at_a), Some(at_b)) =
                (a.armed.or_else(|| start(a)), b.armed.or_else(|| start(b)))
            else {
                return untimed(a, b, Align::Arming);
            };
            let note = match (a.armed.is_none(), b.armed.is_none()) {
                (false, false) => None,
                (true, true) => Some("No arming in either log; their starts stand in".to_string()),
                (true, false) => Some("No arming in A; its start stands in".to_string()),
                (false, true) => Some("No arming in B; its start stands in".to_string()),
            };
            Lineup {
                offset: at_a - at_b,
                took: Align::Arming,
                at: Some((at_a, at_b)),
                note,
            }
        }
        Align::Start => {
            let (Some(at_a), Some(at_b)) = (start(a), start(b)) else {
                return untimed(a, b, Align::Start);
            };
            Lineup {
                offset: at_a - at_b,
                took: Align::Start,
                at: Some((at_a, at_b)),
                note: None,
            }
        }
    }
}

/// No offset, for a pair one of whose logs has no timed record to line
/// up by.
fn untimed(a: &LoadedLog, b: &LoadedLog, took: Align) -> Lineup {
    let missing = which(a.time_range().is_none(), b.time_range().is_none());
    Lineup {
        offset: 0.0,
        took,
        at: None,
        note: Some(format!("No timed record in {missing}; nothing to line up")),
    }
}

/// Which log a note is about: `A`, `B`, or `either log` when both.
fn which(a: bool, b: bool) -> &'static str {
    match (a, b) {
        (true, true) => "either log",
        (true, false) => "A",
        _ => "B",
    }
}

/// What the row says when B, moved onto A's clock by `offset`, shares no
/// time with A: which way and how far apart they lie, `B starts 1 h 51
/// min after A ends; nothing overlaps`. None when they overlap, or a log
/// has no time.
#[must_use]
pub fn apart(
    a: Option<RangeInclusive<f64>>,
    b: Option<RangeInclusive<f64>>,
    offset: f64,
) -> Option<String> {
    let (a, b) = (a?, b?);
    let (b_start, b_end) = (b.start() + offset, b.end() + offset);
    if b_start > *a.end() {
        Some(format!(
            "B starts {} after A ends; nothing overlaps",
            span(b_start - a.end())
        ))
    } else if b_end < *a.start() {
        Some(format!(
            "B ends {} before A starts; nothing overlaps",
            span(a.start() - b_end)
        ))
    } else {
        None
    }
}

/// A gap, to the minute past a minute: `1 h 51 min`, `12 min`, `40 s`.
fn span(seconds: f64) -> String {
    let whole = seconds.max(0.0) as u64;
    let (hours, minutes) = (whole / 3600, whole % 3600 / 60);
    if hours > 0 {
        format!("{hours} h {minutes} min")
    } else if minutes > 0 {
        format!("{minutes} min")
    } else {
        format!("{whole} s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dflog::Log;
    use dflog::access::Value;
    use dflog::write::LogWriter;

    use crate::demo::{self, Flight};

    /// A log of `ATT` records at `times`, seconds, with an `EV` Armed at
    /// `armed` when given, and a GPS fix, which gives it a time base, at
    /// `fix` when given: `(seconds, milliseconds into the GPS week)`.
    fn log(times: &[f64], armed: Option<f64>, fix: Option<(f64, u64)>) -> LoadedLog {
        let us = |seconds: f64| Value::U64((seconds * 1e6).round() as u64);
        let mut w = LogWriter::new();
        w.define(1, "ATT", "Qf", &["TimeUS", "Roll"]).unwrap();
        w.define(2, "EV", "QB", &["TimeUS", "Id"]).unwrap();
        w.define(
            3,
            "GPS",
            "QBIHLL",
            &["TimeUS", "Status", "GMS", "GWk", "Lat", "Lng"],
        )
        .unwrap();
        for &t in times {
            w.record("ATT", &[us(t), Value::F64(0.0)]).unwrap();
        }
        if let Some(t) = armed {
            w.record("EV", &[us(t), Value::U64(10)]).unwrap();
        }
        if let Some((t, ms)) = fix {
            w.record(
                "GPS",
                &[
                    us(t),
                    Value::U64(3),
                    Value::U64(ms),
                    Value::U64(2418),
                    Value::F64(42.3),
                    Value::F64(-92.7),
                ],
            )
            .unwrap();
        }
        LoadedLog::build(Log::from_bytes(&w.into_bytes()), "t.bin".into())
    }

    #[test]
    fn b_lines_up_at_arming_at_the_start_or_by_utc() {
        // A armed at 45 s of 10..500, B at 33 s of 5..520; B booted an hour
        // after A, both fixed a minute after boot
        let a = log(&[10.0, 500.0], Some(45.0), Some((60.0, 400_000_000)));
        let b = log(&[5.0, 520.0], Some(33.0), Some((60.0, 403_600_000)));
        let at_arming = line_up(&a, &b, Align::Arming);
        assert_eq!(
            at_arming,
            Lineup {
                offset: 12.0,
                took: Align::Arming,
                at: Some((45.0, 33.0)),
                note: None,
            }
        );
        let at_start = line_up(&a, &b, Align::Start);
        assert_eq!(at_start.at, Some((10.0, 5.0)));
        assert!((at_start.offset - 5.0).abs() < 1e-9);
        let by_utc = line_up(&a, &b, Align::Utc);
        assert_eq!((by_utc.offset, by_utc.took), (3600.0, Align::Utc));
        assert_eq!(by_utc.note, None);
    }

    #[test]
    fn a_log_that_lacks_what_is_asked_falls_back_and_says_so() {
        let armed = log(&[10.0, 500.0], Some(45.0), Some((60.0, 400_000_000)));
        let unarmed = log(&[5.0, 520.0], None, None);
        // no arming in B: its start stands in, named
        let lineup = line_up(&armed, &unarmed, Align::Arming);
        assert_eq!(lineup.at, Some((45.0, 5.0)));
        assert!((lineup.offset - 40.0).abs() < 1e-9);
        assert_eq!(
            lineup.note.as_deref(),
            Some("No arming in B; its start stands in")
        );
        assert_eq!(
            line_up(&unarmed, &unarmed, Align::Arming).note.as_deref(),
            Some("No arming in either log; their starts stand in")
        );
        // and in A: A's start with B's arming
        let lineup = line_up(&unarmed, &armed, Align::Arming);
        assert_eq!(lineup.at, Some((5.0, 45.0)));
        assert!((lineup.offset + 40.0).abs() < 1e-9);
        assert_eq!(
            lineup.note.as_deref(),
            Some("No arming in A; its start stands in")
        );
        // no GPS time in B: at arming instead, and why
        let lineup = line_up(&armed, &unarmed, Align::Utc);
        assert_eq!((lineup.offset, lineup.took), (40.0, Align::Arming));
        assert_eq!(
            lineup.note.as_deref(),
            Some("No GPS time in B; lined up at arming. No arming in B; its start stands in")
        );
        let lineup = line_up(&armed, &armed, Align::Utc);
        assert_eq!((lineup.offset, lineup.took), (0.0, Align::Utc));

        // a log with no timed record lines up at nothing
        let untimed = log(&[], None, None);
        let lineup = line_up(&untimed, &armed, Align::Start);
        assert_eq!((lineup.offset, lineup.at), (0.0, None));
        assert_eq!(
            lineup.note.as_deref(),
            Some("No timed record in A; nothing to line up")
        );
        assert_eq!(
            line_up(&armed, &untimed, Align::Arming).note.as_deref(),
            Some("No timed record in B; nothing to line up")
        );
        // by UTC, the same, with no claim of a lineup at arming before it
        let lineup = line_up(&armed, &untimed, Align::Utc);
        assert_eq!((lineup.took, lineup.at), (Align::Arming, None));
        assert_eq!(
            lineup.note.as_deref(),
            Some("No timed record in B; nothing to line up")
        );
    }

    #[test]
    fn two_flights_that_share_no_time_say_how_far_apart_they_lie() {
        assert_eq!(apart(Some(0.0..=100.0), Some(0.0..=50.0), 20.0), None);
        assert_eq!(apart(Some(0.0..=100.0), Some(0.0..=50.0), 90.0), None);
        assert_eq!(
            apart(Some(0.0..=100.0), Some(0.0..=50.0), 140.0).as_deref(),
            Some("B starts 40 s after A ends; nothing overlaps")
        );
        assert_eq!(
            apart(Some(100.0..=200.0), Some(0.0..=50.0), -900.0).as_deref(),
            Some("B ends 15 min before A starts; nothing overlaps")
        );
        assert_eq!(apart(None, Some(0.0..=50.0), 0.0), None);
        assert_eq!(span(3.0 * 3600.0 + 59.9), "3 h 0 min");
    }

    #[test]
    fn the_demo_pair_by_utc_lies_apart_by_the_gap_its_ranges_give() {
        let flight =
            |f: Flight| LoadedLog::build(Log::from_source(demo::bytes(f).into()), f.name().into());
        let (a, b) = (flight(Flight::A), flight(Flight::B));
        // at arming, the two flights line up 12 s apart
        let lineup = line_up(&a, &b, Align::Arming);
        assert!((lineup.offset - 12.0).abs() < 1e-9, "{}", lineup.offset);
        // by UTC, B booted two hours after A and flies after it ends
        let lineup = line_up(&a, &b, Align::Utc);
        assert!((lineup.offset - 7200.0).abs() < 1e-9, "{}", lineup.offset);
        let gap = b.time_range().unwrap().start() + 7200.0 - a.time_range().unwrap().end();
        let minutes = (gap / 60.0).floor() as u64;
        assert_eq!(
            apart(a.time_range(), b.time_range(), lineup.offset),
            Some(format!(
                "B starts {} h {} min after A ends; nothing overlaps",
                minutes / 60,
                minutes % 60
            ))
        );
        assert_eq!(minutes / 60, 1, "{gap} s");
    }
}
