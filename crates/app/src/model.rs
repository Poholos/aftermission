//! A scanned log as the review tool sees it: message types with their
//! fields, units and instances, a time for every record, the flight mode
//! changes, the wall-clock base, the vehicle's track, its events and its
//! parameters. Built once, off the UI thread, from a [`dflog::Log`];
//! series are extracted from it on demand.

use dflog::columns::{self, ColumnError};
use dflog::time::TimeBase;
use dflog::{FmtDef, Log, ScanStats};

use crate::codes;
use crate::modes::Vehicle;
use crate::params::{self, Param};
use crate::settings::TimeAxis;

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
    /// format character does not already apply it, as the log carries it;
    /// see [`display_multiplier`]. [`Scale::of`] applies it.
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

/// Where the vehicle was, from the EKF's `POS` records, or from `GPS`
/// when a log has none; empty when it has neither.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Track {
    /// Seconds since boot, in log order.
    pub times: Vec<f64>,
    /// Degrees.
    pub lats: Vec<f64>,
    /// Degrees.
    pub lons: Vec<f64>,
    /// Meters above sea level; NaN where the record had none.
    pub alts: Vec<f64>,
    /// The type the track was read from: `POS` or `GPS`.
    pub source: &'static str,
    /// The edges, found once; None for an empty track.
    pub bounds: Option<Bounds>,
}

/// The edges of a track, in degrees.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bounds {
    pub south: f64,
    pub west: f64,
    pub north: f64,
    pub east: f64,
}

impl Track {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.times.is_empty()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.times.len()
    }

    /// The point nearest in time to `time`.
    #[must_use]
    pub fn nearest(&self, time: f64) -> Option<usize> {
        nearest_index(&self.times, time)
    }

    /// The edges of the track; None when it is empty.
    #[must_use]
    pub fn bounds(&self) -> Option<Bounds> {
        self.bounds
    }
}

/// What kind of record an event came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventKind {
    /// A `MSG` text from the firmware.
    Message,
    /// An `ERR` record: a subsystem reporting a fault, or its end.
    Error,
    /// An `EV` record: something the vehicle did.
    Event,
    /// A flight mode change.
    Mode,
    /// A parameter set to a new value after boot.
    Param,
}

/// One line of the events list.
#[derive(Debug, Clone, PartialEq)]
pub struct Event {
    /// Seconds since boot.
    pub time: f64,
    pub kind: EventKind,
    pub text: String,
}

/// The rows of some fields of one type, each with its time.
struct TimedColumns {
    /// Seconds since boot, one per row.
    times: Vec<f64>,
    /// Field by field, `rows` values each.
    values: Vec<f64>,
    rows: usize,
}

impl TimedColumns {
    /// The values of the `index`th field asked for.
    fn column(&self, index: usize) -> &[f64] {
        &self.values[index * self.rows..(index + 1) * self.rows]
    }
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
    pub track: Track,
    /// In time order.
    pub events: Vec<Event>,
    /// Sorted by name.
    pub params: Vec<Param>,
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
        let modes = mode_changes(&log, &timeline, vehicle);
        let params = params::read(&log, &timeline);

        let mut loaded = LoadedLog {
            name,
            types,
            params,
            timeline,
            time_base: log.time_base(),
            modes,
            vehicle,
            stats: log.stats,
            log,
            track: Track::default(),
            events: Vec::new(),
        };
        loaded.track = track(&loaded);
        loaded.events = events(&loaded);
        loaded
    }

    #[must_use]
    pub fn type_named(&self, name: &str) -> Option<&MessageType> {
        self.types.iter().find(|t| t.name == name)
    }

    /// The field `key` names, when the log has it.
    #[must_use]
    pub fn field_of(&self, key: &SeriesKey) -> Option<&Field> {
        self.type_named(&key.type_name)?.field(&key.field)
    }

    /// Indexed records.
    #[must_use]
    pub fn records(&self) -> usize {
        self.log.index.len()
    }

    /// What the Parquet export writes: the number of message types and of
    /// records. A record goes by its type's name, so an id whose name a
    /// later `FMT` gave to another id is left out, with its records.
    #[cfg(feature = "parquet")]
    #[must_use]
    pub fn parquet_counts(&self) -> (usize, usize) {
        self.types
            .iter()
            .filter(|t| self.log.name_to_id.get(&t.name) == Some(&t.id))
            .fold((0, 0), |(types, records), t| (types + 1, records + t.count))
    }

    /// Write every message type of the log into `dir` as Parquet, one file
    /// per type, or per instance value with `split_instances`; see
    /// [`dflog::parquet::export`].
    ///
    /// # Errors
    ///
    /// Any error from the exporter, which leaves the files it opened
    /// unfinished in `dir`.
    #[cfg(feature = "parquet")]
    pub fn export_parquet(
        &self,
        dir: &std::path::Path,
        split_instances: bool,
    ) -> Result<dflog::parquet::ExportSummary, dflog::parquet::ExportError> {
        dflog::parquet::export(&self.log, dir, None, split_instances)
    }

    /// The base the time axis reads through: the GPS one when UTC is
    /// chosen and the log has it, none for the boot clock.
    #[must_use]
    pub fn wall_clock(&self, axis: TimeAxis) -> Option<TimeBase> {
        match axis {
            TimeAxis::Utc => self.time_base,
            TimeAxis::Boot => None,
        }
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
        let scale = Scale::of(field.multiplier);
        let cols = self
            .timed_columns(t, &[&key.field], key.instance)
            .map_err(|e| format!("{}: {e}", key.label()))?;
        let values = cols.column(0).iter().map(|&v| scale.apply(v));
        let (xs, ys) = cols
            .times
            .iter()
            .copied()
            .zip(values)
            .filter(finite)
            .unzip();
        Ok(Series { xs, ys })
    }

    /// The rows of `fields` of type `t`, of one instance when given, each
    /// row with its time in seconds since boot: from the type's own time
    /// field, or from the timeline for a type without one.
    fn timed_columns(
        &self,
        t: &MessageType,
        fields: &[&str],
        instance: Option<i64>,
    ) -> Result<TimedColumns, ColumnError> {
        let requested: Vec<&str> = t
            .time
            .map(|time| time.label)
            .into_iter()
            .chain(fields.iter().copied())
            .collect();
        let column = columns::get_columns_filtered(&self.log, &t.name, &requested, instance)?;
        let rows = column.rows as usize;
        let mut values = column.values;
        let times = match t.time {
            Some(time) => {
                let rest = values.split_off(rows);
                let times = values.into_iter().map(|t| t * time.scale).collect();
                values = rest;
                times
            }
            None => column
                .linenos
                .iter()
                .map(|&lineno| self.timeline[lineno as usize])
                .collect(),
        };
        Ok(TimedColumns {
            times,
            values,
            rows,
        })
    }
}

/// The lowest and highest of `values`, which are finite; None when there
/// are none.
#[must_use]
pub fn extent(values: &[f64]) -> Option<(f64, f64)> {
    values.iter().fold(None, |range: Option<(f64, f64)>, &y| {
        Some(range.map_or((y, y), |(a, b)| (a.min(y), b.max(y))))
    })
}

/// The index of the value of `xs`, which are non-decreasing, nearest to
/// `x`; None for an empty slice.
#[must_use]
pub fn nearest_index(xs: &[f64], x: f64) -> Option<usize> {
    let after = xs.partition_point(|t| *t < x);
    let candidates = [after.checked_sub(1), (after < xs.len()).then_some(after)];
    candidates
        .into_iter()
        .flatten()
        .min_by(|&a, &b| (xs[a] - x).abs().total_cmp(&(xs[b] - x).abs()))
}

fn finite((x, y): &(f64, f64)) -> bool {
    x.is_finite() && y.is_finite()
}

/// How a field's decoded values reach the unit the log names for them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Scale {
    /// Already there: no factor applies.
    Unit,
    /// Divided by an exact power of ten. ArduPilot writes its `MULT` table
    /// through a float cast, so the factor standing for 0.1 arrives as
    /// 0.100000001490116, and multiplying by it puts a raw 41 at
    /// 4.100000061094761; even by an exact 0.1 it is 4.1000000000000005.
    /// Dividing by the power the factor stands for gives 4.1. The value is
    /// the exponent: 1 for a tenth, 9 for a billionth.
    Divide(u8),
    /// Multiplied by any other factor, such as 3.6 or 100; by 100 the
    /// product is exact anyway.
    Multiply(f64),
}

/// The powers a `MULT` factor can stand for: 1e-1 (`A`) down to 1e-9
/// (`I`), matched by value, since the table has no `H`.
const POWERS_OF_TEN: [f64; 9] = [1e1, 1e2, 1e3, 1e4, 1e5, 1e6, 1e7, 1e8, 1e9];

impl Scale {
    /// The scaling a field's display multiplier calls for.
    #[must_use]
    pub fn of(multiplier: Option<f64>) -> Scale {
        let Some(factor) = multiplier else {
            return Scale::Unit;
        };
        // within 1e-6 relative of a power: the cast's error is under 6e-8,
        // and neighboring powers differ tenfold, so nothing else matches
        POWERS_OF_TEN
            .iter()
            .position(|&power| (factor * power - 1.0).abs() < 1e-6)
            .map_or(Scale::Multiply(factor), |index| {
                Scale::Divide(u8::try_from(index + 1).expect("nine powers at most"))
            })
    }

    #[must_use]
    pub fn apply(self, value: f64) -> f64 {
        match self {
            Scale::Unit => value,
            Scale::Divide(exponent) => value / power_of_ten(exponent),
            Scale::Multiply(factor) => value * factor,
        }
    }
}

/// Ten to `exponent`. Exact for exponents up to 22, since a double holds
/// five to the twenty-second power in its 53 bits and not the next; a
/// [`Scale::Divide`] holds 1 to 9, well inside that.
#[must_use]
pub fn power_of_ten(exponent: u8) -> f64 {
    10f64.powi(i32::from(exponent))
}

/// The factor a plotted value is scaled by. A format character that
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

/// The vehicle's positions: the EKF's `POS` records when the log has them,
/// otherwise the `GPS` records with a fix, from the first receiver that
/// has one when a log has several.
fn track(log: &LoadedLog) -> Track {
    let sources: [(&'static str, Option<&str>); 2] = [("POS", None), ("GPS", Some("Status"))];
    for (name, fix_field) in sources {
        let Some(t) = log.type_named(name) else {
            continue;
        };
        let instances: Vec<Option<i64>> = if t.instances.is_empty() {
            vec![None]
        } else {
            t.instances.iter().map(|&i| Some(i)).collect()
        };
        for instance in instances {
            if let Some(track) = positions(log, t, instance, fix_field, name)
                && !track.is_empty()
            {
                return track;
            }
        }
    }
    Track::default()
}

/// The `Lat`, `Lng` and `Alt` of type `t`'s records, of one `instance`
/// when it has several, each in its unit; rows without a finite time and
/// position, or at 0, 0, are left out, and with `fix_field`, rows whose
/// value there is under 3 (no 3D fix).
fn positions(
    log: &LoadedLog,
    t: &MessageType,
    instance: Option<i64>,
    fix_field: Option<&str>,
    source: &'static str,
) -> Option<Track> {
    let scale = |label: &str| Scale::of(t.field(label).and_then(|f| f.multiplier));
    let scales = [scale("Lat"), scale("Lng"), scale("Alt")];
    let mut fields = vec!["Lat", "Lng", "Alt"];
    fields.extend(fix_field);
    let cols = log.timed_columns(t, &fields, instance).ok()?;
    let mut track = Track {
        source,
        ..Track::default()
    };
    for row in 0..cols.rows {
        let time = cols.times[row];
        let [lat, lon, alt] = [0, 1, 2].map(|i| scales[i].apply(cols.column(i)[row]));
        let fixed = fix_field.is_none() || cols.column(3)[row] >= 3.0;
        let placed = time.is_finite() && lat.is_finite() && lon.is_finite();
        if !fixed || !placed || (lat == 0.0 && lon == 0.0) {
            continue;
        }
        track.times.push(time);
        track.lats.push(lat);
        track.lons.push(lon);
        track.alts.push(alt);
    }
    if let (Some((south, north)), Some((west, east))) = (extent(&track.lats), extent(&track.lons)) {
        track.bounds = Some(Bounds {
            south,
            west,
            north,
            east,
        });
    }
    Some(track)
}

/// The `MSG`, `ERR` and `EV` records, the mode changes and the parameter
/// changes after boot, in time order.
fn events(log: &LoadedLog) -> Vec<Event> {
    let mut events: Vec<Event> = log
        .modes
        .iter()
        .map(|m| Event {
            time: m.time,
            kind: EventKind::Mode,
            text: m.name.clone(),
        })
        .collect();
    let sources = [
        ("MSG", EventKind::Message),
        ("ERR", EventKind::Error),
        ("EV", EventKind::Event),
    ];
    for (name, kind) in sources {
        for record in log.log.records_of(&[name]) {
            let byte = |field: &str| record.value(field)?.as_f64().map(|v| v as u8);
            let text = match kind {
                EventKind::Message => record
                    .value("Message")
                    .and_then(|v| v.as_str().map(|s| s.trim().to_string()))
                    .filter(|s| !s.is_empty()),
                EventKind::Error => match (byte("Subsys"), byte("ECode")) {
                    (Some(subsys), Some(code)) => {
                        Some(codes::error_label(subsys, code, log.vehicle))
                    }
                    _ => None,
                },
                EventKind::Event => byte("Id").map(codes::event_label),
                EventKind::Mode | EventKind::Param => None,
            };
            let Some(text) = text else {
                continue;
            };
            let time = time_of(&log.timeline, record.lineno);
            if time.is_finite() {
                events.push(Event { time, kind, text });
            }
        }
    }
    for param in &log.params {
        let mut before = param.initial;
        for &(time, value) in &param.changes {
            if time.is_finite() {
                events.push(Event {
                    time,
                    kind: EventKind::Param,
                    text: format!(
                        "{} {} -> {}",
                        param.name,
                        params::value_text(before),
                        params::value_text(value)
                    ),
                });
            }
            before = value;
        }
    }
    events.sort_by(|a, b| a.time.total_cmp(&b.time));
    events
}

/// Seconds since boot of the record at `lineno` of the timeline.
pub(crate) fn time_of(timeline: &[f64], lineno: u64) -> f64 {
    timeline.get(lineno as usize).copied().unwrap_or(f64::NAN)
}

/// The `MODE` records as mode changes, a run of the same mode as one.
fn mode_changes(log: &Log, timeline: &[f64], vehicle: Vehicle) -> Vec<ModeChange> {
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
        let time = time_of(timeline, record.lineno);
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
    /// scaled field, `IMU` in two instances, a type without a time field,
    /// three `POS` positions, an error, an event, and four parameters, one
    /// of them changed after boot.
    pub fn bytes() -> Vec<u8> {
        let mut w = LogWriter::new();
        define(&mut w);
        metadata(&mut w);
        let us = |t: u64| Value::U64(t);
        let parm = |w: &mut LogWriter, t: u64, name: &str, value: f64, default: f64| {
            w.record(
                "PARM",
                &[
                    us(t),
                    Value::Str(name.into()),
                    Value::F64(value),
                    Value::F64(default),
                ],
            )
            .unwrap();
        };
        // the dump at boot; SIM_RATE_HZ has no default to log
        parm(&mut w, 1_000_000, "WPNAV_SPEED", 1500.0, 1200.0);
        parm(&mut w, 1_000_000, "MIS_TOTAL", 4.0, 4.0);
        parm(&mut w, 1_000_000, "SIM_RATE_HZ", 380.0, f64::NAN);
        parm(&mut w, 1_000_000, "ATC_RAT_RLL_P", 0.137, 0.137);
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
            if i == 0 {
                w.record("ERR", &[us(2_020_000), Value::U64(3), Value::U64(0)])
                    .unwrap();
                w.record("EV", &[us(2_030_000), Value::U64(10)]).unwrap();
            }
            if i == 1 {
                w.record("NOTM", &[Value::U64(7), Value::F64(42.0)])
                    .unwrap();
            }
            if i == 3 {
                // a mission uploaded after boot, its count logged twice
                parm(&mut w, 2_320_000, "MIS_TOTAL", 7.0, f64::NAN);
                parm(&mut w, 2_330_000, "MIS_TOTAL", 7.0, f64::NAN);
            }
            if i < 3 {
                // a short hop north-east, 50 ms after each attitude sample
                let step = 1e-4 * i as f64;
                w.record(
                    "POS",
                    &[
                        us(t + 50_000),
                        Value::F64(47.0 + step),
                        Value::F64(8.0 + step),
                        Value::F64(500.0 + i as f64),
                        Value::F64(10.0),
                        Value::F64(10.0),
                    ],
                )
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
        w.define(
            9,
            "POS",
            "QLLfff",
            &["TimeUS", "Lat", "Lng", "Alt", "RelHomeAlt", "RelOriginAlt"],
        )
        .unwrap();
        w.define(10, "ERR", "QBB", &["TimeUS", "Subsys", "ECode"])
            .unwrap();
        w.define(11, "EV", "QB", &["TimeUS", "Id"]).unwrap();
        w.define(12, "PARM", "QNff", &["TimeUS", "Name", "Value", "Default"])
            .unwrap();
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
        // the factors pass through a float32 on the way into a real log
        for (c, factor) in [
            ('-', 0.0f32),
            ('?', 1.0),
            ('F', 1e-6),
            ('B', 0.01),
            ('0', 1.0),
        ] {
            w.record("MULT", &[t.clone(), id(c), Value::F64(f64::from(factor))])
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
                "ATT", "ERR", "EV", "FMT", "FMTU", "IMU", "MODE", "MSG", "MULT", "NOTM", "PARM",
                "POS", "UNIT"
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
        assert_eq!(
            time.multiplier,
            Some(f64::from(1e-6f32)),
            "as the log carries it"
        );

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
        // TimeUS itself plots in seconds through its multiplier, exactly:
        // divided by 1e6, not multiplied by the cast factor
        let t = log.series(&key("ATT", "TimeUS", None)).unwrap();
        assert_eq!(t.ys, [2.0, 2.1, 2.2, 2.3]);
        assert_eq!(log.field_of(&key("ATT", "TimeUS", None)).unwrap().code, 'Q');
        assert!(log.field_of(&key("ATT", "Nope", None)).is_none());

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

    #[test]
    fn the_track_comes_from_pos_and_the_events_sort_by_time() {
        let log = loaded();
        let track = &log.track;
        assert_eq!(track.source, "POS");
        assert_eq!(track.len(), 3);
        assert!(
            close(&track.times, &[2.05, 2.15, 2.25]),
            "{:?}",
            track.times
        );
        assert!(close(&track.lats, &[47.0, 47.0001, 47.0002]));
        assert!(close(&track.lons, &[8.0, 8.0001, 8.0002]));
        assert_eq!(track.alts, [500.0, 501.0, 502.0]);
        let b = track.bounds().unwrap();
        assert!(close(
            &[b.south, b.west, b.north, b.east],
            &[47.0, 8.0, 47.0002, 8.0002]
        ));
        assert_eq!(track.nearest(2.16), Some(1));
        assert_eq!(track.nearest(9.0), Some(2));
        assert_eq!(Track::default().nearest(1.0), None);
        assert_eq!(Track::default().bounds(), None);
        assert_eq!(nearest_index(&[0.0, 1.0, 2.0], 0.4), Some(0));
        assert_eq!(nearest_index(&[0.0, 1.0, 2.0], 0.6), Some(1));
        assert_eq!(nearest_index(&[], 0.6), None);
        assert!((time_of(&[1.0, 2.0], 1) - 2.0).abs() < 1e-12);
        assert!(time_of(&[1.0, 2.0], 5).is_nan());

        let lines: Vec<String> = log
            .events
            .iter()
            .map(|e| format!("{:.3} {:?} {}", e.time, e.kind, e.text))
            .collect();
        assert_eq!(
            lines,
            [
                "1.000 Message ArduCopter V4.7.0 (0000000)",
                "1.500 Mode Stabilize",
                "2.020 Error Compass: resolved",
                "2.030 Event Armed",
                "2.250 Mode Loiter",
                "2.320 Param MIS_TOTAL 4 -> 7",
            ]
        );
        assert_eq!(log.wall_clock(TimeAxis::Utc), None, "no GPS");

        // what the window tests rely on: each parameter against its
        // default, and when it changed; params.rs tests the reading itself
        let params: Vec<(&str, Option<bool>, Vec<f64>)> = log
            .params
            .iter()
            .map(|p| {
                let times = p.changes.iter().map(|&(time, _)| time).collect();
                (p.name.as_str(), p.off_default(), times)
            })
            .collect();
        assert_eq!(params.len(), 4);
        assert_eq!(params[0].0, "ATC_RAT_RLL_P");
        assert_eq!(params[0].1, Some(false), "at its default");
        assert_eq!(params[1].0, "MIS_TOTAL");
        assert_eq!(params[1].1, Some(true), "set away from it after boot");
        assert!(close(&params[1].2, &[2.32]), "{:?}", params[1].2);
        assert_eq!(params[2].0, "SIM_RATE_HZ");
        assert_eq!(params[2].1, None, "no default logged");
        assert_eq!(params[3].0, "WPNAV_SPEED");
        assert_eq!(params[3].1, Some(true), "off it from boot");
        assert!(
            params
                .iter()
                .filter(|p| p.0 != "MIS_TOTAL")
                .all(|p| p.2.is_empty())
        );
    }

    #[test]
    fn each_parameter_change_event_reads_from_the_value_before_it() {
        use dflog::access::Value;
        use dflog::write::LogWriter;

        let mut w = LogWriter::new();
        w.define(1, "PARM", "QNf", &["TimeUS", "Name", "Value"])
            .unwrap();
        for (time, value) in [
            (1_000_000, 1250.0),
            (3_000_000, 875.5),
            (4_000_000, 875.5),
            (5_000_000, 1250.0),
        ] {
            w.record(
                "PARM",
                &[
                    Value::U64(time),
                    Value::Str("LOIT_SPEED".into()),
                    Value::F64(value),
                ],
            )
            .unwrap();
        }
        let log = LoadedLog::build(Log::from_bytes(&w.into_bytes()), "parm.bin".into());
        let lines: Vec<String> = log
            .events
            .iter()
            .map(|e| format!("{:.3} {:?} {}", e.time, e.kind, e.text))
            .collect();
        assert_eq!(
            lines,
            [
                "3.000 Param LOIT_SPEED 1250 -> 875.5",
                "5.000 Param LOIT_SPEED 875.5 -> 1250",
            ]
        );
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

        // without POS the track comes from the GPS fix
        assert_eq!(log.track.source, "GPS");
        assert_eq!(log.track.len(), 1);
        assert!(close(&log.track.lats, &[47.0]) && close(&log.track.lons, &[8.0]));
        assert!((log.track.alts[0] - 450.0).abs() < 1e-6);
        assert!((log.track.times[0] - 5.2).abs() < 1e-9);
        assert!(log.events.is_empty());
    }

    /// A log whose `ATT` moves to a new id by a second `FMT`: the model
    /// keeps both ids, the Parquet export writes only the one that owns
    /// the name.
    #[cfg(feature = "parquet")]
    #[test]
    fn the_parquet_counts_leave_out_an_id_that_lost_its_name() {
        use dflog::access::Value;
        use dflog::write::LogWriter;

        let mut w = LogWriter::new();
        w.define(1, "ATT", "Qf", &["TimeUS", "Roll"]).unwrap();
        for t in [1_000_000, 2_000_000] {
            w.record("ATT", &[Value::U64(t), Value::F64(0.5)]).unwrap();
        }
        w.define(2, "ATT", "Qff", &["TimeUS", "Roll", "Pitch"])
            .unwrap();
        w.record(
            "ATT",
            &[Value::U64(3_000_000), Value::F64(0.5), Value::F64(0.25)],
        )
        .unwrap();
        let log = LoadedLog::build(Log::from_bytes(&w.into_bytes()), "twice.bin".into());

        // FMT and both ATT ids, with every record: three FMT records, one
        // of them FMT's own, and three ATT
        let names: Vec<&str> = log.types.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, ["ATT", "ATT", "FMT"]);
        assert_eq!(log.records(), 6);
        // FMT and the ATT that owns the name: two types, with FMT's three
        // records and the one ATT record under the new id
        assert_eq!(log.parquet_counts(), (2, 4));
    }

    /// A log with two receivers and no `POS`: the first receiver never
    /// gets a fix, so the track comes from the second.
    #[test]
    fn the_track_takes_the_first_receiver_with_a_fix() {
        use dflog::access::Value;
        use dflog::write::LogWriter;

        let mut w = LogWriter::new();
        w.define(1, "UNIT", "QbZ", &["TimeUS", "Id", "Label"])
            .unwrap();
        w.define(
            2,
            "FMTU",
            "QBNN",
            &["TimeUS", "FmtType", "UnitIds", "MultIds"],
        )
        .unwrap();
        w.define(
            3,
            "GPS",
            "QBBLLe",
            &["TimeUS", "I", "Status", "Lat", "Lng", "Alt"],
        )
        .unwrap();
        let t = Value::U64(1_000_000);
        let text = |s: &str| Value::Str(s.into());
        w.record(
            "UNIT",
            &[t.clone(), Value::I64(i64::from(b'#')), text("instance")],
        )
        .unwrap();
        w.record(
            "FMTU",
            &[t.clone(), Value::U64(3), text("s#-DUm"), text("F-----")],
        )
        .unwrap();
        for (instance, status, lat) in [(0, 1, 0.0), (1, 3, 47.5), (0, 1, 0.0), (1, 3, 47.5001)] {
            w.record(
                "GPS",
                &[
                    t.clone(),
                    Value::U64(instance),
                    Value::U64(status),
                    Value::F64(lat),
                    Value::F64(8.5),
                    Value::F64(400.0),
                ],
            )
            .unwrap();
        }
        let log = LoadedLog::build(Log::from_bytes(&w.into_bytes()), "gps2.bin".into());
        assert_eq!(log.type_named("GPS").unwrap().instances, [0, 1]);
        assert_eq!(log.track.source, "GPS");
        assert!(
            close(&log.track.lats, &[47.5, 47.5001]),
            "{:?}",
            log.track.lats
        );
        let b = log.track.bounds().unwrap();
        assert!(close(
            &[b.south, b.north, b.west, b.east],
            &[47.5, 47.5001, 8.5, 8.5]
        ));
    }

    #[test]
    fn factors_snap_to_the_power_of_ten_they_stand_for_and_divide() {
        // the copter corpus's A and F, cast through float32
        assert_eq!(Scale::of(Some(f64::from(0.1f32))), Scale::Divide(1));
        assert_eq!(Scale::of(Some(f64::from(1e-6f32))), Scale::Divide(6));
        assert_eq!(Scale::of(Some(f64::from(1e-9f32))), Scale::Divide(9));
        assert_eq!(Scale::of(Some(0.01)), Scale::Divide(2));
        assert_eq!(power_of_ten(9).to_bits(), 1e9f64.to_bits());
        assert_eq!(Scale::of(Some(3.6)), Scale::Multiply(3.6));
        assert_eq!(Scale::of(Some(100.0)), Scale::Multiply(100.0));
        assert_eq!(Scale::of(None), Scale::Unit);

        // a raw 41 at A is 4.1, not 4.100000061094761; and 1e-5 and 1e-9
        // divide exactly where 1.0 / factor would not
        let values = [
            Scale::of(Some(f64::from(0.1f32))).apply(41.0),
            Scale::of(Some(f64::from(1e-5f32))).apply(3.0),
            Scale::of(Some(f64::from(1e-9f32))).apply(7.0),
            Scale::of(Some(3.6)).apply(2.0),
            Scale::Unit.apply(-1.5),
        ];
        let expected = [4.1, 3e-5, 7e-9, 7.2, -1.5];
        assert_eq!(
            values.map(f64::to_bits),
            expected.map(f64::to_bits),
            "{values:?}"
        );
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
        assert!(log.track.is_empty());
        assert!(log.events.is_empty());
    }
}
