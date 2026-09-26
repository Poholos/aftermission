//! The log's parameters: the dump at boot and the changes after it, read
//! from the `PARM` records, and the table that lists them with the
//! history of each one that changed.

use std::collections::{BTreeMap, HashSet};

use dflog::Log;
use egui_extras::{Column, TableBuilder};

use crate::filter::Filter;
use crate::model::{LoadedLog, time_of};
use crate::settings::Settings;
use crate::timefmt;

/// One parameter, with the value it had when the log started and every
/// change since.
#[derive(Debug, Clone, PartialEq)]
pub struct Param {
    pub name: String,
    /// The value in the name's first `PARM` record, which stands for its
    /// value at boot: normally the dump's record, or the record of a set
    /// made while the dump was still streaming.
    pub initial: f32,
    /// The first default among the name's records that is finite as a
    /// float32. None when the log's `PARM` format has no `Default` field,
    /// or every record's is too large for a float32 or NaN, which ArduPilot
    /// writes when it has none and on the records it logs for a set.
    pub default: Option<f32>,
    /// Later records whose value differs from the one before, with the
    /// board time of each, in seconds.
    pub changes: Vec<(f64, f32)>,
}

impl Param {
    /// The value at the end of the log.
    #[must_use]
    pub fn last(&self) -> f32 {
        self.changes
            .last()
            .map_or(self.initial, |&(_, value)| value)
    }

    /// Whether the last value differs from the default; None when the
    /// log records no default for it.
    #[must_use]
    #[expect(
        clippy::float_cmp,
        reason = "values are compared exactly, as logged; NaN never gets in"
    )]
    pub fn off_default(&self) -> Option<bool> {
        self.default.map(|default| default != self.last())
    }
}

/// The parameters of `log`, sorted by name. A record is timed through
/// `timeline`, so the old `Name,Value` layout without a time field takes
/// the time of the record before it. Values are compared as logged: a
/// record repeating the value before it is not a change. The default is
/// the first finite one seen, since a parameter set while the firmware is
/// still streaming its boot dump logs first, with no default, and its
/// dump record follows.
///
/// A name's first record stands for its boot value. A parameter the dump
/// never listed, such as one a script registers later, shows its first
/// set as the boot value, with no time.
#[must_use]
#[expect(
    clippy::float_cmp,
    reason = "values are compared exactly, as logged; NaN is filtered out"
)]
pub fn read(log: &Log, timeline: &[f64]) -> Vec<Param> {
    let mut params: BTreeMap<String, Param> = BTreeMap::new();
    for record in log.records_of(&["PARM"]) {
        let name = record
            .value("Name")
            .and_then(|v| v.as_str().map(|s| s.trim().to_string()));
        let value = record.value("Value").and_then(|v| v.as_f64());
        let (Some(name), Some(value)) = (name, value) else {
            continue;
        };
        // finite as the float32 a parameter is, not only as the wider
        // number a nonstandard layout may log: 1e300 narrows to infinity
        let value = value as f32;
        if name.is_empty() || !value.is_finite() {
            continue;
        }
        let default = record
            .value("Default")
            .and_then(|v| v.as_f64())
            .map(|d| d as f32)
            .filter(|d| d.is_finite());
        if let Some(param) = params.get_mut(&name) {
            if param.last() != value {
                param
                    .changes
                    .push((time_of(timeline, record.lineno), value));
            }
            if param.default.is_none() {
                param.default = default;
            }
        } else {
            let param = Param {
                name: name.clone(),
                initial: value,
                default,
                changes: Vec::new(),
            };
            params.insert(name, param);
        }
    }
    params.into_values().collect()
}

/// The fewest digits that read back as `value`, without an exponent:
/// `0.3` rather than `0.30000001192092896`, `1` rather than `1.0`, and
/// `0.0000001` rather than `1e-7`.
#[must_use]
pub fn value_text(value: f32) -> String {
    format!("{value}")
}

/// The panel's state: the filter, the two narrowing toggles, and the
/// parameters whose history is unfolded.
#[derive(Debug, Default)]
pub struct ParamsPanel {
    filter: String,
    /// Only parameters whose last value is not the default.
    off_default: bool,
    /// Only parameters with a change after boot.
    changed: bool,
    open: HashSet<String>,
}

/// One row of the table: a parameter, with whether its history follows,
/// or one step of that history, where step 0 is the boot value and step
/// n change n.
#[derive(Clone, Copy)]
enum Row<'a> {
    Param(&'a Param, bool),
    History(&'a Param, usize),
}

impl ParamsPanel {
    #[cfg(test)]
    pub(crate) fn set_filter(&mut self, filter: &str) {
        self.filter = filter.to_string();
    }

    /// Forget what was unfolded: the names belonged to the log before.
    pub fn reload(&mut self) {
        self.open.clear();
    }

    /// The table, with the toggles and the filter above it. A click on a
    /// parameter with changes unfolds its history; the time of a clicked
    /// change is returned, to seek to.
    pub fn show(&mut self, ui: &mut egui::Ui, log: &LoadedLog, settings: &Settings) -> Option<f64> {
        ui.horizontal(|ui| {
            if has_defaults(log) {
                ui.toggle_value(&mut self.off_default, "Not default");
            }
            ui.toggle_value(&mut self.changed, "Changed after boot");
            ui.add(
                egui::TextEdit::singleline(&mut self.filter)
                    .hint_text("Filter parameters")
                    .desired_width(180.0),
            );
        });
        let rows = self.rows(log);
        if rows.is_empty() {
            ui.weak(if log.params.is_empty() {
                "No parameters in this log."
            } else {
                "Nothing matches."
            });
            // the panel keeps its height: it stores what its content used
            ui.take_available_space();
            return None;
        }
        let (seek, toggled) = table(ui, &rows, log.wall_clock(settings.time_axis));
        if let Some(name) = toggled
            && !self.open.remove(&name)
        {
            self.open.insert(name);
        }
        seek
    }

    /// The parameters that pass the filter and the toggles, each followed
    /// by its history when unfolded. "Not default" counts only while it is
    /// on screen: a log without defaults hides it, and a toggle left on
    /// from the log before would otherwise empty the table with no way to
    /// turn it off.
    fn rows<'a>(&self, log: &'a LoadedLog) -> Vec<Row<'a>> {
        let filter = Filter::new(&self.filter);
        let off_default = self.off_default && has_defaults(log);
        let mut rows = Vec::new();
        for p in &log.params {
            if !filter.matches(&p.name)
                || (off_default && p.off_default() != Some(true))
                || (self.changed && p.changes.is_empty())
            {
                continue;
            }
            let unfolded = !p.changes.is_empty() && self.open.contains(&p.name);
            rows.push(Row::Param(p, unfolded));
            if unfolded {
                rows.extend((0..=p.changes.len()).map(|step| Row::History(p, step)));
            }
        }
        rows
    }
}

/// Whether any parameter of `log` has a default, which the "Not default"
/// toggle needs to mean anything.
fn has_defaults(log: &LoadedLog) -> bool {
    log.params.iter().any(|p| p.default.is_some())
}

/// Draw the rows; the time of a clicked change, and the name of a clicked
/// parameter with changes, to fold or unfold.
fn table(
    ui: &mut egui::Ui,
    rows: &[Row<'_>],
    time_base: Option<dflog::time::TimeBase>,
) -> (Option<f64>, Option<String>) {
    let row_height = ui.spacing().interact_size.y;
    // a selectable text would take the click the row is meant to get
    ui.style_mut().interaction.selectable_labels = false;
    let mut seek = None;
    let mut toggled = None;
    TableBuilder::new(ui)
        .id_salt("params")
        // the table takes the height the panel gives it, no more and no
        // less: its default minimum of 200 px would grow the panel once the
        // rows scroll, and shrinking to a few rows would shrink the panel,
        // which stores the height its content used
        .min_scrolled_height(0.0)
        .auto_shrink(false)
        .striped(true)
        .sense(egui::Sense::click())
        .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
        .column(Column::initial(200.0).at_least(80.0).resizable(true))
        .column(Column::initial(110.0).at_least(60.0).resizable(true))
        .column(Column::initial(110.0).at_least(60.0).resizable(true))
        .column(Column::remainder())
        .header(row_height, |mut header| {
            for title in ["Name", "Value", "Default", "Changes"] {
                header.col(|ui| {
                    ui.strong(title);
                });
            }
        })
        .body(|body| {
            body.rows(row_height, rows.len(), |mut row| match rows[row.index()] {
                Row::Param(p, unfolded) => {
                    let unfolds = !p.changes.is_empty();
                    row.col(|ui| {
                        ui.monospace(format!("{} {}", mark(p, unfolded), p.name));
                    });
                    row.col(|ui| {
                        ui.monospace(value_text(p.last()));
                    });
                    row.col(|ui| {
                        ui.monospace(p.default.map(value_text).unwrap_or_default());
                    });
                    row.col(|ui| {
                        if unfolds {
                            ui.label(p.changes.len().to_string());
                        }
                    });
                    if unfolds && row.response().clicked() {
                        toggled = Some(p.name.clone());
                    }
                }
                Row::History(p, 0) => {
                    row.col(|ui| {
                        ui.weak("    at boot");
                    });
                    row.col(|ui| {
                        ui.monospace(value_text(p.initial));
                    });
                    row.col(|_| {});
                    row.col(|_| {});
                }
                Row::History(p, step) => {
                    let (time, value) = p.changes[step - 1];
                    row.col(|ui| {
                        ui.monospace(format!("    {}", timefmt::stamp(time_base, time)));
                    });
                    row.col(|ui| {
                        ui.monospace(value_text(value));
                    });
                    row.col(|_| {});
                    row.col(|_| {});
                    if row.response().clicked() {
                        seek = Some(time);
                    }
                }
            });
        });
    (seek, toggled)
}

/// The fold mark in front of a parameter's name: down when its history
/// follows, right when it has one folded, blank when it never changed.
fn mark(p: &Param, unfolded: bool) -> char {
    if p.changes.is_empty() {
        ' '
    } else if unfolded {
        '\u{25be}'
    } else {
        '\u{25b8}'
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dflog::access::Value;
    use dflog::write::LogWriter;

    /// The `PARM` layouts ArduPilot has logged, and one it never has.
    #[derive(Clone, Copy, PartialEq)]
    enum Layout {
        /// `TimeUS,Name,Value,Default`.
        Current,
        /// `TimeUS,Name,Value`, before the default was added.
        Timed,
        /// `Name,Value`, without a time.
        Untimed,
        /// A value and default logged as doubles, which no ArduPilot
        /// writes; what a float32 cannot hold shows only here.
        Double,
    }

    /// A log whose `PARM` type has `layout`, with `records` as (time in us,
    /// name, value, default) rows; the time and the default are left out
    /// where the layout has no field for them.
    fn log(layout: Layout, records: &[(u64, &str, f64, Option<f64>)]) -> Vec<u8> {
        let (format, labels): (&str, &[&str]) = match layout {
            Layout::Current => ("QNff", &["TimeUS", "Name", "Value", "Default"]),
            Layout::Timed => ("QNf", &["TimeUS", "Name", "Value"]),
            Layout::Untimed => ("Nf", &["Name", "Value"]),
            Layout::Double => ("QNdd", &["TimeUS", "Name", "Value", "Default"]),
        };
        let mut w = LogWriter::new();
        w.define(1, "PARM", format, labels).unwrap();
        w.define(2, "EV", "QB", &["TimeUS", "Id"]).unwrap();
        // a timed record before the first parameter, for the layout
        // without a time to take its time from
        w.record("EV", &[Value::U64(500_000), Value::U64(10)])
            .unwrap();
        for &(time, name, value, default) in records {
            let mut values = Vec::new();
            if layout != Layout::Untimed {
                values.push(Value::U64(time));
            }
            values.push(Value::Str(name.into()));
            values.push(Value::F64(value));
            if matches!(layout, Layout::Current | Layout::Double) {
                values.push(Value::F64(default.unwrap_or(f64::NAN)));
            }
            w.record("PARM", &values).unwrap();
        }
        w.into_bytes()
    }

    fn loaded(bytes: &[u8]) -> LoadedLog {
        LoadedLog::build(Log::from_bytes(bytes), "p.bin".into())
    }

    fn read_log(bytes: &[u8]) -> Vec<Param> {
        loaded(bytes).params
    }

    #[test]
    fn the_boot_dump_and_the_changes_after_it_are_read() {
        let params = read_log(&log(
            Layout::Current,
            &[
                (1_000_000, "WPNAV_SPEED", 1500.0, Some(1200.0)),
                (1_000_000, "MIS_TOTAL", 4.0, Some(4.0)),
                (1_000_000, "SIM_RATE_HZ", 380.0, None),
                (1_000_000, "ATC_RAT_RLL_P", 0.137, Some(0.137)),
                // set while the boot dump was still streaming: the set
                // record comes first without a default, the dump after it
                (1_050_000, "RTL_ALT", 2500.0, None),
                (1_100_000, "RTL_ALT", 2500.0, Some(1800.0)),
                // a signed zero is the same value
                (1_100_000, "TEST_TRIM", 0.0, Some(0.0)),
                (2_310_000, "TEST_TRIM", -0.0, None),
                // set after boot: ArduPilot logs no default then
                (2_320_000, "MIS_TOTAL", 7.0, None),
                // the same value again is not a change
                (2_330_000, "MIS_TOTAL", 7.0, None),
                (2_500_000, "MIS_TOTAL", 4.0, None),
                // a blank name is skipped, as is a NaN value
                (2_600_000, "  ", 1.0, None),
                (2_700_000, "WPNAV_SPEED", f64::NAN, None),
            ],
        ));
        let names: Vec<&str> = params.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "ATC_RAT_RLL_P",
                "MIS_TOTAL",
                "RTL_ALT",
                "SIM_RATE_HZ",
                "TEST_TRIM",
                "WPNAV_SPEED"
            ]
        );
        let roll = &params[0];
        assert_eq!(roll.off_default(), Some(false));

        let mis = &params[1];
        assert_eq!(mis.initial.to_bits(), 4.0f32.to_bits());
        assert_eq!(mis.default, Some(4.0));
        assert_eq!(mis.changes.len(), 2);
        assert!((mis.changes[0].0 - 2.32).abs() < 1e-9);
        assert_eq!(mis.changes[0].1.to_bits(), 7.0f32.to_bits());
        assert!((mis.changes[1].0 - 2.5).abs() < 1e-9);
        assert_eq!(mis.last().to_bits(), 4.0f32.to_bits());
        assert_eq!(mis.off_default(), Some(false), "back at the default");

        let rtl = &params[2];
        assert_eq!(rtl.default, Some(1800.0), "from the dump record");
        assert_eq!(rtl.off_default(), Some(true));
        assert!(rtl.changes.is_empty(), "the dump repeats the set value");

        let sim = &params[3];
        assert_eq!(sim.default, None, "NaN on the boot record");
        assert_eq!(sim.off_default(), None);

        let trim = &params[4];
        assert!(trim.changes.is_empty(), "-0 is not a change from 0");
        assert_eq!(trim.off_default(), Some(false));

        let speed = &params[5];
        assert_eq!(speed.off_default(), Some(true));
        assert!(speed.changes.is_empty(), "a NaN value is not a change");
    }

    #[test]
    fn values_print_in_the_fewest_digits_without_an_exponent() {
        assert_eq!(value_text(0.137), "0.137");
        assert_eq!(value_text(1500.0), "1500");
        assert_eq!(value_text(-0.5), "-0.5");
        assert_eq!(value_text(1e-7), "0.0000001");
    }

    #[test]
    fn older_layouts_read_without_a_default_or_a_time() {
        let timed = read_log(&log(
            Layout::Timed,
            &[
                (1_000_000, "RTL_ALT", 1700.0, None),
                (1_500_000, "RTL_ALT", 2000.0, None),
            ],
        ));
        assert_eq!(timed.len(), 1);
        assert_eq!(timed[0].default, None);
        assert!((timed[0].changes[0].0 - 1.5).abs() < 1e-9);

        let untimed = read_log(&log(
            Layout::Untimed,
            &[(0, "RTL_ALT", 1700.0, None), (0, "RTL_ALT", 2000.0, None)],
        ));
        assert_eq!(untimed.len(), 1);
        // the change takes the time of the last timed record before it
        assert!((untimed[0].changes[0].0 - 0.5).abs() < 1e-9);
    }

    #[test]
    fn what_a_float32_cannot_hold_is_left_out() {
        // a value beyond float32 is skipped like a NaN, a default beyond it
        // is dropped, and a value that fits keeps its float32 digits
        let params = read_log(&log(
            Layout::Double,
            &[
                (1_000_000, "TOO_BIG", 1e300, Some(1.0)),
                (1_000_000, "FITS", 0.137, Some(1e300)),
            ],
        ));
        let names: Vec<&str> = params.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["FITS"]);
        assert_eq!(params[0].default, None);
        assert_eq!(value_text(params[0].initial), "0.137");
    }

    #[test]
    fn a_log_without_parameter_records_has_none() {
        assert!(read_log(&log(Layout::Current, &[])).is_empty());

        // no PARM type at all
        let mut w = LogWriter::new();
        w.define(2, "EV", "QB", &["TimeUS", "Id"]).unwrap();
        w.record("EV", &[Value::U64(500_000), Value::U64(10)])
            .unwrap();
        assert!(read_log(&w.into_bytes()).is_empty());
    }

    #[test]
    fn a_hidden_not_default_toggle_does_not_empty_the_table() {
        // left on from a log with defaults, the toggle is hidden on one
        // without, and must not filter what the user cannot turn off
        let log = loaded(&log(Layout::Timed, &[(1_000_000, "RTL_ALT", 1700.0, None)]));
        let panel = ParamsPanel {
            off_default: true,
            ..ParamsPanel::default()
        };
        assert_eq!(panel.rows(&log).len(), 1);
    }
}
