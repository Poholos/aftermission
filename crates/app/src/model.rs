//! A scanned log as the review tool sees it: message types with their
//! fields, units and instances, a time for every record, the flight mode
//! changes and the wall-clock base. Built once, off the UI thread, from a
//! [`dflog::Log`]; series are extracted from it on demand.

use dflog::columns::{self, ColumnError};
use dflog::time::TimeBase;
use dflog::{FmtDef, Log, ScanStats};

use crate::modes::Vehicle;

/// One field of a message type, with the units metadata the log carries
/// for it.
#[derive(Debug, Clone, PartialEq)]
pub struct Field {
    pub label: String,
    /// The format character the field decodes by.
    pub code: char,
    /// The unit name from the log's `UNIT` table, such as `deg` or `m/s`.
    pub unit: Option<String>,
    /// The factor that takes the decoded value to its unit, when the
    /// format character does not already apply it; see
    /// [`display_multiplier`].
    pub multiplier: Option<f64>,
    /// Whether the field decodes to a number and so can be plotted.
    pub numeric: bool,
}

impl Field {
    /// The label with its unit: `Roll (deg)`.
    #[must_use]
    pub fn title(&self) -> String {
        match &self.unit {
            Some(unit) if !unit.is_empty() => format!("{} ({unit})", self.label),
            _ => self.label.clone(),
        }
    }
}

/// The field a message type carries its board time in.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TimeField {
    pub label: &'static str,
    /// From the field's unit to seconds.
    pub scale: f64,
}

impl TimeField {
    /// The board-time field of a format: `TimeUS` in microseconds, or in
    /// older logs `TimeMS` in milliseconds. The one exception is the old
    /// GPS record, where `TimeMS` is milliseconds into the GPS week beside
    /// the `Week` number, and the board time is its `T` field.
    fn of(fmt: &FmtDef) -> Option<TimeField> {
        let has = |label: &str| fmt.labels.iter().any(|l| l == label);
        let millis = |label: &'static str| TimeField { label, scale: 1e-3 };
        if has("TimeUS") {
            Some(TimeField {
                label: "TimeUS",
                scale: 1e-6,
            })
        } else if (has("Week") || has("GWk")) && has("T") {
            Some(millis("T"))
        } else if has("TimeMS") {
            Some(millis("TimeMS"))
        } else {
            None
        }
    }
}

/// A message type the log holds records of.
#[derive(Debug, Clone, PartialEq)]
pub struct MessageType {
    pub id: u8,
    pub name: String,
    /// Records of this type in the log.
    pub count: usize,
    pub fields: Vec<Field>,
    /// The label of the field that tells the type's instances apart, such
    /// as `I` of `IMU`, when the log's `FMTU` record marks one.
    pub instance_field: Option<String>,
    /// The instance values seen, when there is more than one; empty for a
    /// type logged as a single instance, which then plots as a whole.
    pub instances: Vec<i64>,
    pub time: Option<TimeField>,
}

impl MessageType {
    #[must_use]
    pub fn field(&self, label: &str) -> Option<&Field> {
        self.fields.iter().find(|f| f.label == label)
    }
}

/// A flight mode the vehicle entered.
#[derive(Debug, Clone, PartialEq)]
pub struct ModeChange {
    /// Seconds since boot.
    pub time: f64,
    pub number: u8,
    pub name: String,
}

/// What names one plotted quantity: a field of a type, of one instance
/// when the type has several.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SeriesKey {
    pub type_name: String,
    pub field: String,
    pub instance: Option<i64>,
}

impl SeriesKey {
    /// `ATT.Roll`, or `IMU[1].GyrX` for an instance.
    #[must_use]
    pub fn label(&self) -> String {
        match self.instance {
            Some(instance) => format!("{}[{instance}].{}", self.type_name, self.field),
            None => format!("{}.{}", self.type_name, self.field),
        }
    }
}

/// The samples of one series, in log order, with a finite value each.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Series {
    /// Seconds since boot.
    pub xs: Vec<f64>,
    /// In the field's unit.
    pub ys: Vec<f64>,
}

/// A scanned log with everything the panels show derived from it.
#[derive(Debug)]
pub struct LoadedLog {
    log: Log,
    /// The file's name, for titles.
    pub name: String,
    /// Sorted by name; only types the log holds records of.
    pub types: Vec<MessageType>,
    /// Seconds since boot of every indexed record. A record of a type
    /// without a time field takes the time of the last record before it
    /// that had one.
    pub timeline: Vec<f64>,
    pub time_base: Option<TimeBase>,
    /// In log order, consecutive repeats of a mode merged.
    pub modes: Vec<ModeChange>,
    pub vehicle: Vehicle,
    pub stats: ScanStats,
}

impl LoadedLog {
    /// Derive the model from a scanned log. `name` labels it in the UI.
    #[must_use]
    pub fn build(log: Log, name: String) -> LoadedLog {
        let units = log.units();
        let mut counts = [0usize; 256];
        for &t in &log.index.types {
            counts[usize::from(t)] += 1;
        }

        let mut types: Vec<MessageType> = log
            .fmts
            .iter()
            .filter(|(id, _)| counts[usize::from(**id)] > 0)
            .map(|(&id, fmt)| {
                let fields = fmt
                    .format
                    .chars()
                    .zip(&fmt.labels)
                    .enumerate()
                    .map(|(index, (code, label))| {
                        let meta = units.field_meta(id, index);
                        Field {
                            label: label.clone(),
                            code,
                            unit: meta.unit,
                            multiplier: display_multiplier(code, meta.multiplier),
                            numeric: is_numeric(code),
                        }
                    })
                    .collect();
                MessageType {
                    id,
                    name: fmt.name.clone(),
                    count: counts[usize::from(id)],
                    fields,
                    instance_field: units
                        .instance_field_index(id)
                        .and_then(|i| fmt.labels.get(i).cloned()),
                    instances: Vec::new(),
                    time: TimeField::of(fmt),
                }
            })
            .collect();
        types.sort_by(|a, b| a.name.cmp(&b.name));

        for t in &mut types {
            let Some(field) = &t.instance_field else {
                continue;
            };
            if let Ok(column) = columns::get_columns(&log, &t.name, &[field]) {
                let mut seen: Vec<i64> = column.values.iter().map(|v| *v as i64).collect();
                seen.sort_unstable();
                seen.dedup();
                if seen.len() > 1 {
                    t.instances = seen;
                }
            }
        }

        let timeline = timeline(&log, &types);
        let vehicle = log
            .records_of(&["MSG"])
            .find_map(|record| Vehicle::from_banner(record.value("Message")?.as_str()?))
            .unwrap_or_default();
        let modes = mode_changes(&log, &types, &timeline, vehicle);

        LoadedLog {
            name,
            types,
            timeline,
            time_base: log.time_base(),
            modes,
            vehicle,
            stats: log.stats,
            log,
        }
    }

    #[must_use]
    pub fn type_named(&self, name: &str) -> Option<&MessageType> {
        self.types.iter().find(|t| t.name == name)
    }

    /// Indexed records.
    #[must_use]
    pub fn records(&self) -> usize {
        self.log.index.len()
    }

    /// Seconds from the first record's time to the last one's; None for a
    /// log without a timed record.
    #[must_use]
    pub fn duration(&self) -> Option<f64> {
        let first = self.timeline.first()?;
        let last = self.timeline.last()?;
        (first.is_finite() && last.is_finite()).then(|| last - first)
    }

    /// Whether `key` names a numeric field of a type in this log, of one
    /// of its instances when it has several.
    #[must_use]
    pub fn has_series(&self, key: &SeriesKey) -> bool {
        let Some(t) = self.type_named(&key.type_name) else {
            return false;
        };
        let field_ok = t.field(&key.field).is_some_and(|f| f.numeric);
        let instance_ok = match key.instance {
            Some(instance) => t.instances.contains(&instance),
            None => t.instances.is_empty(),
        };
        field_ok && instance_ok
    }

    /// The samples of `key`, in the field's unit against seconds since
    /// boot, rows whose value or time is not finite left out.
    ///
    /// # Errors
    ///
    /// When the type or field is unknown, the field is not numeric, or the
    /// column extraction fails for a malformed format.
    pub fn series(&self, key: &SeriesKey) -> Result<Series, String> {
        let t = self
            .type_named(&key.type_name)
            .ok_or_else(|| format!("unknown message type {}", key.type_name))?;
        let field = t
            .field(&key.field)
            .ok_or_else(|| format!("{} has no field {}", t.name, key.field))?;
        if !field.numeric {
            return Err(format!("{}.{} is text, not a number", t.name, key.field));
        }
        let factor = field.multiplier.unwrap_or(1.0);
        let describe = |e: ColumnError| format!("{}: {e}", key.label());

        let (xs, ys): (Vec<f64>, Vec<f64>) = if let Some(time) = t.time {
            let column = columns::get_columns_filtered(
                &self.log,
                &t.name,
                &[time.label, &key.field],
                key.instance,
            )
            .map_err(describe)?;
            let rows = column.rows as usize;
            let times = column.values[..rows].iter().map(|t| t * time.scale);
            let values = column.values[rows..2 * rows].iter().map(|v| v * factor);
            times.zip(values).filter(finite).unzip()
        } else {
            let column =
                columns::get_columns_filtered(&self.log, &t.name, &[&key.field], key.instance)
                    .map_err(describe)?;
            let times = column
                .linenos
                .iter()
                .map(|&lineno| self.timeline[lineno as usize]);
            let values = column.values.iter().map(|v| v * factor);
            times.zip(values).filter(finite).unzip()
        };
        Ok(Series { xs, ys })
    }
}

fn finite((x, y): &(f64, f64)) -> bool {
    x.is_finite() && y.is_finite()
}

/// The factor a plotted value is multiplied by. A format character that
/// scales its field (`c`, `C`, `e`, `E`, `L`) already yields the unit the
/// log names, and the `MULT` entry for such a field describes that same
/// scaling, so applying it too would scale twice. Other fields take their
/// multiplier when it is a real factor: the table's own `-` and `?`
/// entries are 0 and 1.
#[must_use]
#[expect(
    clippy::float_cmp,
    reason = "the table's sentinels are the exact values 0 and 1"
)]
pub fn display_multiplier(code: char, multiplier: Option<f64>) -> Option<f64> {
    if matches!(code, 'c' | 'C' | 'e' | 'E' | 'L') {
        return None;
    }
    multiplier.filter(|m| m.is_finite() && *m != 0.0 && *m != 1.0)
}

/// Whether a format character decodes to a number: strings and sample
/// arrays do not.
fn is_numeric(code: char) -> bool {
    !matches!(code, 'n' | 'N' | 'Z' | 'a')
        && u8::try_from(code).is_ok_and(|c| dflog::field_size(c).is_some())
}

/// Seconds since boot of every record, from each type's time field, the
/// records of types without one taking the last time before them and the
/// leading ones the first time seen.
fn timeline(log: &Log, types: &[MessageType]) -> Vec<f64> {
    let mut times = vec![f64::NAN; log.index.len()];
    for t in types {
        let Some(time) = t.time else {
            continue;
        };
        if let Ok(column) = columns::get_columns(log, &t.name, &[time.label]) {
            for (row, &lineno) in column.linenos.iter().enumerate() {
                times[lineno as usize] = column.values[row] * time.scale;
            }
        }
    }
    let mut last = f64::NAN;
    for time in &mut times {
        if time.is_nan() {
            *time = last;
        } else {
            last = *time;
        }
    }
    if let Some(first) = times.iter().copied().find(|t| !t.is_nan()) {
        for time in times.iter_mut().take_while(|t| t.is_nan()) {
            *time = first;
        }
    }
    times
}

/// The `MODE` records as mode changes, a run of the same mode as one.
fn mode_changes(
    log: &Log,
    types: &[MessageType],
    timeline: &[f64],
    vehicle: Vehicle,
) -> Vec<ModeChange> {
    let time_field = types.iter().find(|t| t.name == "MODE").and_then(|t| t.time);
    let mut changes: Vec<ModeChange> = Vec::new();
    for record in log.records_of(&["MODE"]) {
        let number = record
            .value("Mode")
            .or_else(|| record.value("ModeNum"))
            .and_then(|v| v.as_f64());
        let Some(number) = number else {
            continue;
        };
        let number = number as u8;
        if changes.last().is_some_and(|last| last.number == number) {
            continue;
        }
        let time = time_field
            .and_then(|f| record.value(f.label)?.as_f64().map(|t| t * f.scale))
            .or_else(|| timeline.get(record.lineno as usize).copied())
            .unwrap_or(f64::NAN);
        changes.push(ModeChange {
            time,
            number,
            name: vehicle.mode_label(number),
        });
    }
    changes
}

#[cfg(test)]
pub(crate) mod testlog {
    //! A synthetic log with the metadata records a modern ArduPilot log
    //! carries; every value is invented.

    use dflog::access::Value;
    use dflog::write::LogWriter;

    /// A copter log: units and multipliers, one mode change, `ATT` with a
    /// scaled field, `IMU` in two instances and a `PARM`-like type without
    /// a time field.
    pub fn bytes() -> Vec<u8> {
        let mut w = LogWriter::new();
        define(&mut w);
        metadata(&mut w);
        let us = |t: u64| Value::U64(t);
        w.record(
            "MODE",
            &[us(1_500_000), Value::U64(0), Value::U64(0), Value::U64(26)],
        )
        .unwrap();
        w.record(
            "MODE",
            &[us(1_500_000), Value::U64(0), Value::U64(0), Value::U64(26)],
        )
        .unwrap();
        for i in 0..4u64 {
            let t = 2_000_000 + i * 100_000;
            w.record(
                "ATT",
                &[
                    us(t),
                    Value::F64(1.5 * i as f64),
                    Value::F64(-2.0),
                    Value::F64(90.0 + i as f64),
                ],
            )
            .unwrap();
            // values a float32 holds exactly, so they read back as written
            for inst in 0..2u64 {
                w.record(
                    "IMU",
                    &[
                        us(t + 10 * inst),
                        Value::U64(inst),
                        Value::F64(0.25 * (inst + 1) as f64),
                        Value::F64(0.0),
                        Value::F64(if i == 2 { f64::NAN } else { 0.5 }),
                    ],
                )
                .unwrap();
            }
            if i == 1 {
                w.record("NOTM", &[Value::U64(7), Value::F64(42.0)])
                    .unwrap();
            }
        }
        w.record(
            "MODE",
            &[us(2_250_000), Value::U64(5), Value::U64(5), Value::U64(1)],
        )
        .unwrap();
        w.into_bytes()
    }

    fn define(w: &mut LogWriter) {
        w.define(1, "MSG", "QZ", &["TimeUS", "Message"]).unwrap();
        w.define(2, "UNIT", "QbZ", &["TimeUS", "Id", "Label"])
            .unwrap();
        w.define(3, "MULT", "Qbd", &["TimeUS", "Id", "Mult"])
            .unwrap();
        w.define(
            4,
            "FMTU",
            "QBNN",
            &["TimeUS", "FmtType", "UnitIds", "MultIds"],
        )
        .unwrap();
        w.define(5, "MODE", "QMBB", &["TimeUS", "Mode", "ModeNum", "Rsn"])
            .unwrap();
        w.define(6, "ATT", "QccC", &["TimeUS", "Roll", "Pitch", "Yaw"])
            .unwrap();
        w.define(7, "IMU", "QBfff", &["TimeUS", "I", "GyrX", "GyrY", "GyrZ"])
            .unwrap();
        w.define(8, "NOTM", "Bf", &["Idx", "Value"]).unwrap();
    }

    /// The banner and the units tables, all at one second.
    fn metadata(w: &mut LogWriter) {
        let t = Value::U64(1_000_000);
        let id = |c: char| Value::I64(i64::from(c as u8));
        let text = |s: &str| Value::Str(s.into());
        w.record("MSG", &[t.clone(), text("ArduCopter V4.7.0 (0000000)")])
            .unwrap();
        for (c, name) in [('s', "s"), ('d', "deg"), ('E', "rad/s"), ('#', "instance")] {
            w.record("UNIT", &[t.clone(), id(c), text(name)]).unwrap();
        }
        for (c, factor) in [('-', 0.0), ('?', 1.0), ('F', 1e-6), ('B', 0.01), ('0', 1.0)] {
            w.record("MULT", &[t.clone(), id(c), Value::F64(factor)])
                .unwrap();
        }
        // ATT: TimeUS in seconds through F; Roll, Pitch and Yaw in degrees
        // through B (0.01), which the `c` and `C` format already applies
        w.record(
            "FMTU",
            &[t.clone(), Value::U64(6), text("sddd"), text("FBBB")],
        )
        .unwrap();
        w.record("FMTU", &[t, Value::U64(7), text("s#EEE"), text("F-000")])
            .unwrap();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn loaded() -> LoadedLog {
        LoadedLog::build(Log::from_bytes(&testlog::bytes()), "test.bin".into())
    }

    fn close(a: &[f64], b: &[f64]) -> bool {
        a.len() == b.len() && a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-9)
    }

    #[test]
    fn types_carry_fields_units_and_instances() {
        let log = loaded();
        let names: Vec<&str> = log.types.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "ATT", "FMT", "FMTU", "IMU", "MODE", "MSG", "MULT", "NOTM", "UNIT"
            ]
        );
        let att = log.type_named("ATT").unwrap();
        assert_eq!(att.count, 4);
        assert_eq!(att.time.unwrap().label, "TimeUS");
        let roll = att.field("Roll").unwrap();
        assert_eq!(roll.title(), "Roll (deg)");
        assert_eq!(roll.multiplier, None, "the c format already scales");
        let time = att.field("TimeUS").unwrap();
        assert_eq!(time.title(), "TimeUS (s)");
        assert_eq!(time.multiplier, Some(1e-6));

        let imu = log.type_named("IMU").unwrap();
        assert_eq!(imu.instance_field.as_deref(), Some("I"));
        assert_eq!(imu.instances, [0, 1]);
        assert_eq!(imu.field("GyrX").unwrap().title(), "GyrX (rad/s)");
        assert_eq!(
            imu.field("GyrX").unwrap().multiplier,
            None,
            "0 stands for none"
        );
        assert!(
            log.types
                .iter()
                .filter(|t| !t.instances.is_empty())
                .eq([imu])
        );

        let msg = log.type_named("MSG").unwrap();
        assert!(!msg.field("Message").unwrap().numeric);
        assert!(log.type_named("NOTM").unwrap().time.is_none());
    }

    #[test]
    fn the_timeline_fills_records_without_a_time() {
        let log = loaded();
        assert_eq!(log.timeline.len(), log.records());
        // the FMT records at the start take the first time seen
        assert!((log.timeline[0] - 1.0).abs() < 1e-9);
        // NOTM sits after the second ATT of t = 2.1 s and its IMU pair
        let notm = log.type_named("NOTM").unwrap();
        let series = log
            .series(&SeriesKey {
                type_name: notm.name.clone(),
                field: "Value".into(),
                instance: None,
            })
            .unwrap();
        assert_eq!(series.ys, [42.0]);
        assert!((series.xs[0] - 2.100_01).abs() < 1e-9, "{}", series.xs[0]);
        assert!((log.duration().unwrap() - 1.25).abs() < 1e-9);
    }

    #[test]
    fn series_come_out_scaled_per_instance_and_finite() {
        let log = loaded();
        let key = |type_name: &str, field: &str, instance| SeriesKey {
            type_name: type_name.into(),
            field: field.into(),
            instance,
        };
        let roll = log.series(&key("ATT", "Roll", None)).unwrap();
        assert!(close(&roll.ys, &[0.0, 1.5, 3.0, 4.5]), "{:?}", roll.ys);
        assert!((roll.xs[1] - 2.1).abs() < 1e-9);

        let imu1 = log.series(&key("IMU", "GyrX", Some(1))).unwrap();
        assert_eq!(imu1.ys, [0.5, 0.5, 0.5, 0.5]);
        // the NaN sample of the third record is left out
        let gyrz = log.series(&key("IMU", "GyrZ", Some(0))).unwrap();
        assert_eq!(gyrz.ys, [0.5, 0.5, 0.5]);
        assert_eq!(gyrz.xs.len(), 3);
        // TimeUS itself plots in seconds through its multiplier
        let t = log.series(&key("ATT", "TimeUS", None)).unwrap();
        assert!((t.ys[0] - 2.0).abs() < 1e-9);

        assert!(log.has_series(&key("IMU", "GyrX", Some(1))));
        assert!(
            !log.has_series(&key("IMU", "GyrX", None)),
            "IMU has instances"
        );
        assert!(!log.has_series(&key("ATT", "Roll", Some(0))));
        assert!(!log.has_series(&key("MSG", "Message", None)));
        log.series(&key("MSG", "Message", None)).unwrap_err();
        log.series(&key("NOPE", "X", None)).unwrap_err();
    }

    #[test]
    fn modes_are_named_and_runs_merged() {
        let log = loaded();
        assert_eq!(log.vehicle, Vehicle::Copter);
        let modes: Vec<(&str, u8)> = log
            .modes
            .iter()
            .map(|m| (m.name.as_str(), m.number))
            .collect();
        assert_eq!(modes, [("Stabilize", 0), ("Loiter", 5)]);
        assert!((log.modes[0].time - 1.5).abs() < 1e-9);
        assert!((log.modes[1].time - 2.25).abs() < 1e-9);
        assert!(log.time_base.is_none(), "no GPS record");
        assert_eq!(log.stats, ScanStats::default());
    }

    /// Before `TimeUS`, types carried `TimeMS` as board time, except GPS,
    /// whose `TimeMS` is the GPS time of week and whose board time is `T`.
    #[test]
    fn old_logs_take_their_board_time_from_the_right_field() {
        use dflog::access::Value;
        use dflog::write::LogWriter;

        let mut w = LogWriter::new();
        w.define(1, "ATT", "IccC", &["TimeMS", "Roll", "Pitch", "Yaw"])
            .unwrap();
        w.define(
            2,
            "GPS",
            "BIHBcLLeeEefI",
            &[
                "Status", "TimeMS", "Week", "NSats", "HDop", "Lat", "Lng", "RelAlt", "Alt", "Spd",
                "GCrs", "VZ", "T",
            ],
        )
        .unwrap();
        w.record(
            "ATT",
            &[
                Value::U64(5_000),
                Value::F64(1.0),
                Value::F64(2.0),
                Value::F64(3.0),
            ],
        )
        .unwrap();
        w.record(
            "GPS",
            &[
                Value::U64(3),
                Value::U64(259_200_000),
                Value::U64(1_800),
                Value::U64(9),
                Value::F64(1.2),
                Value::F64(47.0),
                Value::F64(8.0),
                Value::F64(10.0),
                Value::F64(450.0),
                Value::F64(0.0),
                Value::F64(0.0),
                Value::F64(0.0),
                Value::U64(5_200),
            ],
        )
        .unwrap();
        let log = LoadedLog::build(Log::from_bytes(&w.into_bytes()), "old.bin".into());

        assert_eq!(log.type_named("ATT").unwrap().time.unwrap().label, "TimeMS");
        assert_eq!(log.type_named("GPS").unwrap().time.unwrap().label, "T");
        let alt = log
            .series(&SeriesKey {
                type_name: "GPS".into(),
                field: "Alt".into(),
                instance: None,
            })
            .unwrap();
        assert!(
            (alt.xs[0] - 5.2).abs() < 1e-9,
            "board time, not time of week"
        );
        assert!((log.duration().unwrap() - 0.2).abs() < 1e-9);
    }

    #[test]
    fn multipliers_apply_only_where_the_format_does_not() {
        assert_eq!(display_multiplier('c', Some(0.01)), None);
        assert_eq!(display_multiplier('L', Some(1e-7)), None);
        assert_eq!(display_multiplier('Q', Some(1e-6)), Some(1e-6));
        assert_eq!(display_multiplier('f', Some(1.0)), None);
        assert_eq!(display_multiplier('f', Some(0.0)), None);
        assert_eq!(display_multiplier('h', Some(0.1)), Some(0.1));
        assert_eq!(display_multiplier('f', None), None);
        assert!(is_numeric('M'));
        assert!(is_numeric('g'));
        assert!(!is_numeric('Z'));
        assert!(!is_numeric('a'));
        assert!(!is_numeric('x'));
    }

    #[test]
    fn an_empty_log_has_no_types_or_duration() {
        let log = LoadedLog::build(Log::from_bytes(&[]), "empty.bin".into());
        assert!(log.types.is_empty());
        assert!(log.timeline.is_empty());
        assert_eq!(log.duration(), None);
        assert!(log.modes.is_empty());
        assert_eq!(log.vehicle, Vehicle::Unknown);
    }
}
