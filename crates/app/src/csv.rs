//! CSV export of the plotted series: one row per sample time, one column
//! per series, nothing interpolated, and values exact where the log's
//! scaling allows: through a power of ten, or from a float32 field.

use std::io::{self, Write};
use std::ops::{Range, RangeInclusive};

use dflog::time::TimeBase;

use crate::model::Series;
use crate::timefmt;
use crate::worker;

/// One column: a series under its plot title.
#[derive(Debug, Clone, PartialEq)]
pub struct Column {
    /// `IMU[1].GyrX (rad/s)`
    pub title: String,
    pub data: Series,
    /// Whether the values came from an `f` field, a float32 in the log,
    /// and so print as one: a stored 123.4 scaled by 1e-3 in `f64` is
    /// 0.1234000015258789, and cast back to the nearest float32 it is
    /// 0.1234. A quotient of a float32 has no more precision than that.
    pub single: bool,
}

/// What one export writes.
#[derive(Debug, Clone, PartialEq)]
pub struct Export {
    pub columns: Vec<Column>,
    /// Adds the `utc` column when the log has a wall-clock base.
    pub time_base: Option<TimeBase>,
    /// Only the samples in this range, when set: the plot's view.
    pub range: Option<RangeInclusive<f64>>,
}

impl Export {
    /// The header's column names, in order.
    #[must_use]
    pub fn header(&self) -> Vec<String> {
        header(
            self.time_base.is_some(),
            self.columns.iter().map(|c| c.title.as_str()),
        )
    }

    /// Write the file to `out`: the header, then one row per sample time
    /// across the columns, a cell empty where its series has no sample at
    /// that time. A series with two samples at one time gives each its own
    /// row, so `time_s` is not always unique. Returns the number of data
    /// rows.
    ///
    /// # Errors
    ///
    /// When `out` fails.
    pub fn write(&self, out: &mut impl Write) -> io::Result<usize> {
        let header: Vec<String> = self.header().iter().map(|h| quoted(h)).collect();
        writeln!(out, "{}", header.join(","))?;

        let range = self.range.as_ref();
        let ordered: Vec<Order> = self
            .columns
            .iter()
            .map(|c| Order::of(&c.data.xs, range))
            .collect();
        let mut next = vec![0usize; self.columns.len()];
        let mut rows = 0;
        // a merge: each row takes the earliest pending time, and one sample
        // from every column that has one at it, so two samples of one
        // series at the same time make two rows
        loop {
            let time = self
                .columns
                .iter()
                .zip(&ordered)
                .zip(&next)
                .filter_map(|((c, order), &n)| order.get(n).map(|i| c.data.xs[i]))
                .min_by(f64::total_cmp);
            let Some(time) = time else {
                break;
            };
            write!(out, "{time:.6}")?;
            if let Some(base) = self.time_base {
                write!(out, ",{}", timefmt::iso_stamp(base, time))?;
            }
            for ((column, order), n) in self.columns.iter().zip(&ordered).zip(&mut next) {
                out.write_all(b",")?;
                if let Some(i) = order
                    .get(*n)
                    .filter(|&i| column.data.xs[i].total_cmp(&time).is_eq())
                {
                    // the fewest digits that read back exactly, as a
                    // float32 when the field stored one
                    let value = column.data.ys[i];
                    if column.single {
                        write!(out, "{}", value as f32)?;
                    } else {
                        write!(out, "{value}")?;
                    }
                    *n += 1;
                }
            }
            out.write_all(b"\n")?;
            rows += 1;
        }
        Ok(rows)
    }
}

/// The column names: `time_s`, `utc` when the log has a wall clock, then
/// the series titles.
pub fn header<'a>(utc: bool, titles: impl Iterator<Item = &'a str>) -> Vec<String> {
    let mut header = vec!["time_s".to_string()];
    if utc {
        header.push("utc".to_string());
    }
    header.extend(titles.map(str::to_string));
    header
}

/// The name to offer for the file: the log's stem, with the range in
/// whole seconds when one applies. `flight.csv`, or `flight_12-30s.csv`.
#[must_use]
pub fn file_name(log_name: &str, range: Option<&RangeInclusive<f64>>) -> String {
    let stem = worker::file_stem(log_name);
    match range {
        Some(range) => format!("{stem}_{:.0}-{:.0}s.csv", range.start(), range.end()),
        None => format!("{stem}.csv"),
    }
}

/// A column's samples in the range, in time order: the `n`th is at
/// [`Order::get`]`(n)` of the series.
enum Order {
    /// A series already in time order, as every log without a clock reset
    /// gives: the samples in the range are a run of positions, walked
    /// without an index.
    Sorted(Range<usize>),
    /// A series out of order: its positions in the range, sorted by time.
    Indexed(Vec<usize>),
}

impl Order {
    fn of(xs: &[f64], range: Option<&RangeInclusive<f64>>) -> Order {
        if xs.is_sorted_by(|a, b| a <= b) {
            let run = match range {
                Some(range) => {
                    xs.partition_point(|x| x < range.start())
                        ..xs.partition_point(|x| x <= range.end())
                }
                None => 0..xs.len(),
            };
            return Order::Sorted(run);
        }
        let mut index: Vec<usize> = (0..xs.len())
            .filter(|&i| range.is_none_or(|r| r.contains(&xs[i])))
            .collect();
        index.sort_by(|&a, &b| xs[a].total_cmp(&xs[b]));
        Order::Indexed(index)
    }

    /// The position in the series of the `n`th sample in time order.
    fn get(&self, n: usize) -> Option<usize> {
        match self {
            Order::Sorted(run) => run.clone().nth(n),
            Order::Indexed(index) => index.get(n).copied(),
        }
    }
}

/// A header name as RFC 4180 has it: quoted when it holds a comma, a
/// quote or a line break, with its quotes doubled.
fn quoted(name: &str) -> String {
    if name.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", name.replace('"', "\"\""))
    } else {
        name.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn column(title: &str, xs: &[f64], ys: &[f64], single: bool) -> Column {
        Column {
            title: title.into(),
            data: Series {
                xs: xs.to_vec(),
                ys: ys.to_vec(),
            },
            single,
        }
    }

    fn text(export: &Export) -> (String, usize) {
        let mut out = Vec::new();
        let rows = export.write(&mut out).unwrap();
        (String::from_utf8(out).unwrap(), rows)
    }

    #[test]
    fn rows_merge_by_time_with_empty_cells_and_a_utc_column() {
        let export = Export {
            columns: vec![
                column(
                    "ATT.Roll (deg)",
                    &[1.0, 2.0, 2.5],
                    &[0.0, 1.5, -3.25],
                    false,
                ),
                column("IMU[1].GyrX (rad/s)", &[2.0, 3.0], &[0.5, 0.25], true),
            ],
            // 2023-11-14 22:13:20 UTC at 0.5 s of board time
            time_base: Some(TimeBase {
                gps_start_unix_ms: 1_700_000_000_000,
                ms_offset: 500,
            }),
            range: None,
        };
        assert_eq!(
            export.header(),
            ["time_s", "utc", "ATT.Roll (deg)", "IMU[1].GyrX (rad/s)"]
        );
        let (file, rows) = text(&export);
        assert_eq!(rows, 4);
        assert_eq!(
            file,
            "time_s,utc,ATT.Roll (deg),IMU[1].GyrX (rad/s)\n\
             1.000000,2023-11-14T22:13:20.500000Z,0,\n\
             2.000000,2023-11-14T22:13:21.500000Z,1.5,0.5\n\
             2.500000,2023-11-14T22:13:22.000000Z,-3.25,\n\
             3.000000,2023-11-14T22:13:22.500000Z,,0.25\n"
        );
    }

    #[test]
    fn values_print_as_the_log_stored_them() {
        // a float32 field's 123.4, scaled by 1e-3 in f64, carries the
        // cast's noise
        let scaled = f64::from(123.4f32) / 1000.0;
        assert_eq!(format!("{scaled}"), "0.1234000015258789");
        let export = Export {
            columns: vec![
                column(
                    "BAT.CurrTot (Ah)",
                    &[1.0, 2.0, 3.0],
                    &[scaled, 0.1, 42.0],
                    true,
                ),
                column("GPS.NSats", &[1.0, 2.0, 3.0], &[scaled, 12.0, 1e21], false),
            ],
            time_base: None,
            range: None,
        };
        let (file, _) = text(&export);
        assert_eq!(
            file,
            "time_s,BAT.CurrTot (Ah),GPS.NSats\n\
             1.000000,0.1234,0.1234000015258789\n\
             2.000000,0.1,12\n\
             3.000000,42,1000000000000000000000\n"
        );
    }

    #[test]
    fn the_range_cuts_the_rows_and_the_header_is_quoted() {
        let export = Export {
            columns: vec![
                column(
                    "A,B (m)",
                    &[1.0, 2.0, 3.0, 4.0],
                    &[1.0, 2.0, 3.0, 4.0],
                    false,
                ),
                column("X \"raw\"", &[2.5], &[7.0], false),
                column("Plain", &[], &[], false),
            ],
            time_base: None,
            range: Some(2.0..=3.0),
        };
        let (file, rows) = text(&export);
        assert_eq!(rows, 3);
        assert_eq!(
            file,
            "time_s,\"A,B (m)\",\"X \"\"raw\"\"\",Plain\n\
             2.000000,2,,\n\
             2.500000,,7,\n\
             3.000000,3,,\n"
        );
        assert_eq!(quoted("two\nlines"), "\"two\nlines\"");
    }

    #[test]
    fn sorted_and_unsorted_series_cut_to_the_same_samples() {
        let xs = [1.0, 2.0, 2.0, 3.0, 4.0];
        let range = 2.0..=3.0;
        let Order::Sorted(run) = Order::of(&xs, Some(&range)) else {
            panic!("a series in time order is walked by position");
        };
        assert_eq!(run, 1..4, "both edges of the range are in it");
        assert!(matches!(Order::of(&xs, None), Order::Sorted(run) if run == (0..5)));
        let Order::Indexed(index) = Order::of(&[3.0, 2.0, 5.0, 2.0], Some(&range)) else {
            panic!("a series out of order is indexed");
        };
        assert_eq!(index, [1, 3, 0]);

        // a sorted and an unsorted column merge as one file
        let export = Export {
            columns: vec![
                column("Sorted", &[1.0, 2.0, 3.0], &[10.0, 20.0, 30.0], false),
                column("Unsorted", &[3.0, 1.0], &[0.5, 0.25], true),
            ],
            time_base: None,
            range: Some(1.5..=3.0),
        };
        assert_eq!(
            text(&export),
            (
                "time_s,Sorted,Unsorted\n\
                 2.000000,20,\n\
                 3.000000,30,0.5\n"
                    .to_string(),
                2
            )
        );
    }

    #[test]
    fn unsorted_samples_are_ordered_and_repeats_keep_their_rows() {
        // a series with its clock stepping back, and two samples of one
        // series at the same time: both are in the file, in time order
        let export = Export {
            columns: vec![column(
                "NOTM.Value",
                &[3.0, 1.0, 1.0],
                &[30.0, 10.0, 11.0],
                false,
            )],
            time_base: None,
            range: None,
        };
        let (file, rows) = text(&export);
        assert_eq!(rows, 3);
        assert_eq!(
            file,
            "time_s,NOTM.Value\n\
             1.000000,10\n\
             1.000000,11\n\
             3.000000,30\n"
        );

        let empty = Export {
            columns: Vec::new(),
            time_base: None,
            range: None,
        };
        assert_eq!(text(&empty), ("time_s\n".to_string(), 0));
    }

    #[test]
    fn file_names_carry_the_range_in_whole_seconds() {
        assert_eq!(file_name("flight.bin", None), "flight.csv");
        assert_eq!(
            file_name("flight.bin", Some(&(12.4..=29.6))),
            "flight_12-30s.csv"
        );
        assert_eq!(file_name("log", None), "log.csv");
    }
}
