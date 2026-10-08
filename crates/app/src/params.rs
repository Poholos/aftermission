//! The log's parameters: the dump at boot and the changes after it, read
//! from the `PARM` records, and the table that lists them with the
//! history of each one that changed. While a second log, B, is open, the
//! table shows B's values beside A's and can narrow to what differs.

use std::cmp::Ordering;
use std::collections::{BTreeMap, HashSet};
use std::fmt::Write as _;

use dflog::Log;
use dflog::time::TimeBase;
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

    /// Whether the last value differs from the default: the log's own,
    /// else `fallback`, another log's; None with neither.
    #[must_use]
    #[expect(
        clippy::float_cmp,
        reason = "values are compared exactly, as logged; NaN never gets in"
    )]
    pub fn off_default(&self, fallback: Option<f32>) -> Option<bool> {
        self.default
            .or(fallback)
            .map(|default| default != self.last())
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

/// Parameters whose values the autopilot changes by itself from flight
/// to flight, so that two logs differ in them though no one set them: the
/// statistics it keeps, the ground pressure it takes at boot, the gyro
/// offsets and the temperatures they were found at, the hover throttle it
/// learns, the airspeed offsets and the declination it looks up. `*`
/// stands for any run of characters, none included: `INS_GYR*OFFS_*` is
/// `INS_GYROFFS_X` as well as `INS_GYR2OFFS_X`. Calibration results, such
/// as the compass and accelerometer offsets, are not on it: a
/// recalibration between two flights is a change worth seeing.
const LEARNED: [&str; 9] = [
    "STAT_*",
    "BARO*_GND_PRESS",
    "GND_ABS_PRESS*",
    "INS_GYR*OFFS_*",
    "INS_GYR*_CALTEMP",
    "MOT_THST_HOVER",
    "Q_M_THST_HOVER",
    "ARSPD*_OFFSET",
    "COMPASS_DEC",
];

/// Whether `name` is a value the autopilot learns; see [`LEARNED`].
#[must_use]
pub fn is_learned(name: &str) -> bool {
    LEARNED.iter().any(|pattern| glob(pattern, name))
}

/// Whether `text` matches `pattern`, whose `*` stands for any run of
/// characters, none included.
fn glob(pattern: &str, text: &str) -> bool {
    let (pattern, text) = (pattern.as_bytes(), text.as_bytes());
    let (mut p, mut t) = (0, 0);
    // the last star seen, and where in the text what it takes ends
    let mut star: Option<(usize, usize)> = None;
    while t < text.len() {
        if pattern.get(p) == Some(&b'*') {
            star = Some((p, t));
            p += 1;
        } else if pattern.get(p) == Some(&text[t]) {
            p += 1;
            t += 1;
        } else if let Some((star_p, star_t)) = star {
            // the star takes one more character, and the rest tries again
            star = Some((star_p, star_t + 1));
            p = star_p + 1;
            t = star_t + 1;
        } else {
            return false;
        }
    }
    pattern[p..].iter().all(|&c| c == b'*')
}

/// A parameter name in either of two logs, with its parameter in each;
/// at least one of them has it.
#[derive(Debug, Clone, Copy)]
pub struct Pair<'a> {
    pub name: &'a str,
    pub a: Option<&'a Param>,
    pub b: Option<&'a Param>,
}

impl<'a> Pair<'a> {
    /// A parameter of A, with no B open.
    fn only_a(p: &'a Param) -> Self {
        Self {
            name: &p.name,
            a: Some(p),
            b: None,
        }
    }

    /// Whether both logs have it and its values differ, at boot or at the
    /// end.
    #[must_use]
    #[expect(
        clippy::float_cmp,
        reason = "values are compared exactly, as logged; NaN never gets in"
    )]
    pub fn differs(&self) -> bool {
        match (self.a, self.b) {
            (Some(a), Some(b)) => a.initial != b.initial || a.last() != b.last(),
            _ => false,
        }
    }

    /// Whether both logs have it and end on the same value from values at
    /// boot that differ: a difference the last values alone would hide.
    #[must_use]
    #[expect(
        clippy::float_cmp,
        reason = "values are compared exactly, as logged; NaN never gets in"
    )]
    pub fn apart_at_boot(&self) -> bool {
        match (self.a, self.b) {
            (Some(a), Some(b)) => a.initial != b.initial && a.last() == b.last(),
            _ => false,
        }
    }

    /// Whether only one of the logs has it.
    #[must_use]
    pub fn in_one_log(&self) -> bool {
        self.a.is_none() || self.b.is_none()
    }

    /// Whether it changed after boot in either log.
    fn changed(&self) -> bool {
        [self.a, self.b]
            .into_iter()
            .flatten()
            .any(|p| !p.changes.is_empty())
    }

    /// Whether either log's last value is off its default: the log's own,
    /// or the other log's where it has none, so a log from firmware that
    /// logs no defaults reads against one that does.
    fn off_default(&self) -> bool {
        let off = |p: Option<&Param>, other: Option<&Param>| {
            p.and_then(|p| p.off_default(other.and_then(|o| o.default))) == Some(true)
        };
        off(self.a, self.b) || off(self.b, self.a)
    }

    /// The default the table shows: A's, or B's for a name only B has.
    fn default(&self) -> Option<f32> {
        match self.a {
            Some(a) => a.default,
            None => self.b.and_then(|b| b.default),
        }
    }
}

/// Walk two lists of parameters together, each sorted by name as
/// [`read`] returns it: every name either has, in order, with its
/// parameter in each.
#[must_use]
pub fn compare<'a>(a: &'a [Param], b: &'a [Param]) -> Vec<Pair<'a>> {
    let mut pairs = Vec::with_capacity(a.len().max(b.len()));
    let (mut i, mut j) = (0, 0);
    while i < a.len() || j < b.len() {
        let order = match (a.get(i), b.get(j)) {
            (Some(pa), Some(pb)) => pa.name.cmp(&pb.name),
            (Some(_), None) => Ordering::Less,
            _ => Ordering::Greater,
        };
        let pa = a.get(i).filter(|_| order.is_le());
        let pb = b.get(j).filter(|_| order.is_ge());
        i += usize::from(pa.is_some());
        j += usize::from(pb.is_some());
        if let Some(p) = pa.or(pb) {
            pairs.push(Pair {
                name: &p.name,
                a: pa,
                b: pb,
            });
        }
    }
    pairs
}

/// The panel's state: the filter, the narrowing toggles, and the
/// parameters whose history is unfolded.
#[derive(Debug, Default)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "independent toggles, each on screen in its own right"
)]
pub struct ParamsPanel {
    filter: String,
    /// Only parameters whose last value is not the default.
    off_default: bool,
    /// Only parameters with a change after boot.
    changed: bool,
    /// While B is open, only the names both logs have whose values differ.
    differences: bool,
    /// With Differences, the names only one log has as well.
    one_log: bool,
    /// With Differences, the learned values that differ as well.
    learned: bool,
    open: HashSet<String>,
}

/// One row of the table: a name with its parameter in A and, while B is
/// open, in B, with whether A's history follows; or one step of that
/// history, where step 0 is the boot value and step n change n.
#[derive(Clone, Copy)]
enum Row<'a> {
    Param(Pair<'a>, bool),
    History(&'a Param, usize),
}

/// What Differences shows and what each of its two toggles adds, under
/// the other toggles and the filter.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct Counts {
    /// The rows Differences shows, with its toggles as they are.
    differ: usize,
    /// The names only one log has.
    one_log: usize,
    /// The learned values that differ.
    learned: usize,
}

/// The clocks the table's times read through, None for the boot clock:
/// A's, and B's while B is open.
#[derive(Clone, Copy)]
struct Clocks {
    a: Option<TimeBase>,
    b: Option<TimeBase>,
    /// Whether B is open, and the table has its columns.
    with_b: bool,
}

impl ParamsPanel {
    #[cfg(test)]
    pub(crate) fn set_filter(&mut self, filter: &str) {
        self.filter = filter.to_string();
    }

    /// Narrow to what differs, as a click on Differences does; for tests.
    #[cfg(test)]
    pub(crate) fn show_differences(&mut self) {
        self.differences = true;
    }

    /// Forget what was unfolded: the names belonged to the log before.
    pub fn reload(&mut self) {
        self.open.clear();
    }

    /// The table of `log`'s parameters, and of `b`'s beside them while B
    /// is open, with the toggles and the filter above it. A click on a
    /// parameter of A with changes unfolds its history; the time of a
    /// clicked change is returned, to seek to.
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        log: &LoadedLog,
        b: Option<&LoadedLog>,
        settings: &Settings,
    ) -> Option<f64> {
        let before = self.narrowing();
        let (mut rows, counts) = self.rows(log, b);
        ui.horizontal(|ui| {
            if has_defaults(log, b) {
                ui.toggle_value(&mut self.off_default, "Not default");
            }
            ui.toggle_value(&mut self.changed, "Changed after boot");
            if b.is_some() {
                let differences = ui
                    .toggle_value(&mut self.differences, "Differences")
                    .on_hover_text(
                        "Only the parameters both logs have whose values differ, at boot or at the end",
                    );
                // the additions go off with the toggle they add to, or the
                // count would hold what no toggle on screen explains
                if differences.changed() && !self.differences {
                    self.one_log = false;
                    self.learned = false;
                }
                ui.weak(format!("{} differ", counts.differ));
                if self.differences {
                    ui.toggle_value(
                        &mut self.one_log,
                        format!("In one log only ({})", counts.one_log),
                    )
                    .on_hover_text("Add the parameters one log has and the other lacks");
                    ui.toggle_value(
                        &mut self.learned,
                        format!("Learned values ({})", counts.learned),
                    )
                    .on_hover_text(
                        "Add the values the autopilot learns by itself from flight to flight, \
                         such as the hover throttle and the ground pressure",
                    );
                }
            }
            ui.add(
                egui::TextEdit::singleline(&mut self.filter)
                    .hint_text("Filter parameters")
                    .desired_width(180.0),
            );
        });
        if self.narrowing() != before {
            // a toggle or the filter moved: the rows follow it now, and the
            // counts above them on the next frame
            rows = self.rows(log, b).0;
            ui.ctx().request_repaint();
        }
        if rows.is_empty() {
            let none = log.params.is_empty() && b.is_none_or(|b| b.params.is_empty());
            ui.weak(match (none, b.is_some()) {
                (true, false) => "No parameters in this log.",
                (true, true) => "No parameters in either log.",
                (false, _) => "Nothing matches.",
            });
            // the panel keeps its height: it stores what its content used
            ui.take_available_space();
            return None;
        }
        let clocks = Clocks {
            a: log.wall_clock(settings.time_axis),
            b: b.and_then(|b| b.wall_clock(settings.time_axis)),
            with_b: b.is_some(),
        };
        let (seek, toggled) = table(ui, &rows, clocks);
        if let Some(name) = toggled
            && !self.open.remove(&name)
        {
            self.open.insert(name);
        }
        seek
    }

    /// What narrows the table, to tell when it has moved.
    fn narrowing(&self) -> (String, [bool; 5]) {
        (
            self.filter.clone(),
            [
                self.off_default,
                self.changed,
                self.differences,
                self.one_log,
                self.learned,
            ],
        )
    }

    /// The parameters that pass the filter and the toggles, each followed
    /// by A's history when unfolded, and what Differences and its toggles
    /// count. "Not default" counts only while it is on screen, and
    /// Differences only while B is open: a toggle left on from the logs
    /// before would otherwise narrow the table with no way to turn it off.
    fn rows<'a>(&self, log: &'a LoadedLog, b: Option<&'a LoadedLog>) -> (Vec<Row<'a>>, Counts) {
        let filter = Filter::new(&self.filter);
        let off_default = self.off_default && has_defaults(log, b);
        let pairs = match b {
            Some(b) => compare(&log.params, &b.params),
            None => log.params.iter().map(Pair::only_a).collect(),
        };
        let mut rows = Vec::new();
        let mut counts = Counts::default();
        for pair in pairs {
            if !filter.matches(pair.name)
                || (off_default && !pair.off_default())
                || (self.changed && !pair.changed())
            {
                continue;
            }
            if b.is_some() {
                let one_log = pair.in_one_log();
                let learned = pair.differs() && is_learned(pair.name);
                let differs =
                    (pair.differs() && (self.learned || !learned)) || (self.one_log && one_log);
                counts.differ += usize::from(differs);
                counts.one_log += usize::from(one_log);
                counts.learned += usize::from(learned);
                if self.differences && !differs {
                    continue;
                }
            }
            let history = pair
                .a
                .filter(|p| !p.changes.is_empty() && self.open.contains(&p.name));
            rows.push(Row::Param(pair, history.is_some()));
            if let Some(p) = history {
                rows.extend((0..=p.changes.len()).map(|step| Row::History(p, step)));
            }
        }
        (rows, counts)
    }
}

/// Whether any parameter of `log`, or of `b`, has a default, which the
/// "Not default" toggle needs to mean anything.
fn has_defaults(log: &LoadedLog, b: Option<&LoadedLog>) -> bool {
    [Some(log), b]
        .into_iter()
        .flatten()
        .any(|log| log.params.iter().any(|p| p.default.is_some()))
}

/// Draw the rows, under the columns of one log or of two; the time of a
/// clicked change, and the name of a clicked parameter with changes, to
/// fold or unfold. Each layout has its own id, so each keeps its widths.
fn table(ui: &mut egui::Ui, rows: &[Row<'_>], clocks: Clocks) -> (Option<f64>, Option<String>) {
    let two = clocks.with_b;
    let (id, titles): (&str, &[&str]) = if two {
        ("params_ab", &["Name", "A", "B", "Default", "Changes"])
    } else {
        ("params", &["Name", "Value", "Default", "Changes"])
    };
    let row_height = ui.spacing().interact_size.y;
    // a selectable text would take the click the row is meant to get
    ui.style_mut().interaction.selectable_labels = false;
    let mut seek = None;
    let mut toggled = None;
    TableBuilder::new(ui)
        .id_salt(id)
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
        .columns(
            Column::initial(110.0).at_least(60.0).resizable(true),
            titles.len() - 2,
        )
        .column(Column::remainder())
        .header(row_height, |mut header| {
            for title in titles {
                header.col(|ui| {
                    ui.strong(*title);
                });
            }
        })
        .body(|body| {
            body.rows(row_height, rows.len(), |mut row| match rows[row.index()] {
                Row::Param(pair, unfolded) => {
                    let unfolds = pair.a.is_some_and(|p| !p.changes.is_empty());
                    row.col(|ui| {
                        ui.monospace(format!("{} {}", mark(unfolds, unfolded), pair.name));
                    });
                    let last = |p: Option<&Param>| p.map(|p| value_text(p.last()));
                    row.col(|ui| {
                        ui.monospace(last(pair.a).unwrap_or_default());
                    });
                    if two {
                        row.col(|ui| {
                            let mut text = last(pair.b).unwrap_or_default();
                            if pair.apart_at_boot() {
                                text.push_str(" *");
                            }
                            ui.monospace(text);
                        });
                    }
                    row.col(|ui| {
                        ui.monospace(pair.default().map(value_text).unwrap_or_default());
                    });
                    row.col(|ui| {
                        let text = changes_text(&pair);
                        if !text.is_empty() {
                            ui.label(text);
                        }
                    });
                    let mut response = row.response();
                    // told only while hovered: most rows never are
                    if two
                        && response.hovered()
                        && let Some(note) = note(&pair, clocks)
                    {
                        response = response.on_hover_text(note);
                    }
                    if unfolds && response.clicked() {
                        toggled = Some(pair.name.to_string());
                    }
                }
                Row::History(p, step) => {
                    let (time, value) = if step == 0 {
                        (None, p.initial)
                    } else {
                        let (time, value) = p.changes[step - 1];
                        (Some(time), value)
                    };
                    row.col(|ui| match time {
                        Some(time) => {
                            ui.monospace(format!("    {}", timefmt::stamp(clocks.a, time)));
                        }
                        None => {
                            ui.weak("    at boot");
                        }
                    });
                    row.col(|ui| {
                        ui.monospace(value_text(value));
                    });
                    for _ in 2..titles.len() {
                        row.col(|_| {});
                    }
                    if let Some(time) = time
                        && row.response().clicked()
                    {
                        seek = Some(time);
                    }
                }
            });
        });
    (seek, toggled)
}

/// The fold mark in front of a parameter's name: down when A's history
/// follows, right when it has one folded, blank when it never changed in
/// A.
fn mark(unfolds: bool, unfolded: bool) -> char {
    if !unfolds {
        ' '
    } else if unfolded {
        '\u{25be}'
    } else {
        '\u{25b8}'
    }
}

/// The Changes column: A's count of changes, and B's after a slash when
/// B has any, `2 / 1`; blank when neither changed.
fn changes_text(pair: &Pair<'_>) -> String {
    let count = |p: Option<&Param>| p.map_or(0, |p| p.changes.len());
    match (count(pair.a), count(pair.b)) {
        (a, b) if b > 0 => format!("{a} / {b}"),
        (0, _) => String::new(),
        (a, _) => a.to_string(),
    }
}

/// The most changes a hover tells of, per log.
const STORY_CHANGES: usize = 4;

/// What a row of two logs says on hover: both logs' values over their
/// flights, for a name that differs or changed, marked when only their
/// values at boot differ; B's default where it is not A's; and whether
/// the value is one the autopilot learns. None when there is nothing to
/// add to the row.
fn note(pair: &Pair<'_>, clocks: Clocks) -> Option<String> {
    let mut lines = Vec::new();
    if let (Some(a), Some(b)) = (pair.a, pair.b) {
        if pair.differs() || pair.changed() {
            let lead = if pair.apart_at_boot() {
                "Apart at boot: "
            } else {
                ""
            };
            lines.push(format!(
                "{lead}A {}; B {}",
                story(a, clocks.a),
                story(b, clocks.b)
            ));
        }
        if let Some(default) = b.default.filter(|&d| Some(d) != a.default) {
            lines.push(format!("B's default {}", value_text(default)));
        }
    }
    if is_learned(pair.name) {
        lines.push("Learned: the autopilot sets it by itself from flight to flight".into());
    }
    (!lines.is_empty()).then(|| lines.join("\n"))
}

/// A parameter's values over its log, `800 at boot, 1000 from 4:10`, the
/// times to the second through `base`, `14:34:10` on the UTC axis; or the
/// one value it kept.
fn story(p: &Param, base: Option<TimeBase>) -> String {
    if p.changes.is_empty() {
        return value_text(p.initial);
    }
    let mut text = format!("{} at boot", value_text(p.initial));
    for &(time, value) in p.changes.iter().take(STORY_CHANGES) {
        // a change logged before the first timed record has no time
        if time.is_finite() {
            let when = timefmt::clock(base, time, 1.0);
            let _ = write!(text, ", {} from {when}", value_text(value));
        } else {
            let _ = write!(text, ", then {}", value_text(value));
        }
    }
    let more = p.changes.len().saturating_sub(STORY_CHANGES);
    if more > 0 {
        let _ = write!(text, ", and {more} more");
    }
    text
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
        assert_eq!(roll.off_default(None), Some(false));

        let mis = &params[1];
        assert_eq!(mis.initial.to_bits(), 4.0f32.to_bits());
        assert_eq!(mis.default, Some(4.0));
        assert_eq!(mis.changes.len(), 2);
        assert!((mis.changes[0].0 - 2.32).abs() < 1e-9);
        assert_eq!(mis.changes[0].1.to_bits(), 7.0f32.to_bits());
        assert!((mis.changes[1].0 - 2.5).abs() < 1e-9);
        assert_eq!(mis.last().to_bits(), 4.0f32.to_bits());
        assert_eq!(mis.off_default(None), Some(false), "back at the default");

        let rtl = &params[2];
        assert_eq!(rtl.default, Some(1800.0), "from the dump record");
        assert_eq!(rtl.off_default(None), Some(true));
        assert!(rtl.changes.is_empty(), "the dump repeats the set value");

        let sim = &params[3];
        assert_eq!(sim.default, None, "NaN on the boot record");
        assert_eq!(sim.off_default(None), None);
        assert_eq!(sim.off_default(Some(380.0)), Some(false), "another's");

        let trim = &params[4];
        assert!(trim.changes.is_empty(), "-0 is not a change from 0");
        assert_eq!(trim.off_default(None), Some(false));

        let speed = &params[5];
        assert_eq!(speed.off_default(Some(1500.0)), Some(true), "its own first");
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
        assert_eq!(panel.rows(&log, None).0.len(), 1);
    }

    /// The names of the parameter rows, histories left out.
    fn names(rows: &[Row<'_>]) -> Vec<String> {
        rows.iter()
            .filter_map(|row| match row {
                Row::Param(pair, _) => Some(pair.name.to_string()),
                Row::History(..) => None,
            })
            .collect()
    }

    #[test]
    fn two_lists_walk_together_by_name() {
        let a = read_log(&log(
            Layout::Current,
            &[
                (1_000_000, "SAME", 1.0, Some(1.0)),
                (1_000_000, "AT_END", 1.0, Some(1.0)),
                (1_000_000, "AT_BOOT", 800.0, Some(1000.0)),
                (1_000_000, "ONLY_A", 5.0, Some(5.0)),
                (2_000_000, "AT_END", 3.0, None),
                (2_000_000, "AT_BOOT", 1000.0, None),
            ],
        ));
        let b = read_log(&log(
            Layout::Current,
            &[
                (1_000_000, "SAME", 1.0, Some(1.0)),
                (1_000_000, "AT_END", 1.0, Some(1.0)),
                (1_000_000, "AT_BOOT", 1000.0, Some(1000.0)),
                (1_000_000, "ONLY_B", 3.0, Some(3.0)),
                (2_000_000, "AT_END", 2.0, None),
            ],
        ));
        let pairs = compare(&a, &b);
        let seen: Vec<(&str, bool, bool, bool, bool)> = pairs
            .iter()
            .map(|p| {
                (
                    p.name,
                    p.a.is_some(),
                    p.b.is_some(),
                    p.differs(),
                    p.apart_at_boot(),
                )
            })
            .collect();
        assert_eq!(
            seen,
            [
                ("AT_BOOT", true, true, true, true),
                ("AT_END", true, true, true, false),
                ("ONLY_A", true, false, false, false),
                ("ONLY_B", false, true, false, false),
                ("SAME", true, true, false, false),
            ]
        );
        assert!(pairs[2].in_one_log() && pairs[3].in_one_log());
        assert!(!pairs[0].in_one_log());
        // a side alone walks too
        assert_eq!(compare(&a, &[]).len(), a.len());
        assert!(compare(&[], &b).iter().all(|p| p.a.is_none()));
        assert!(compare(&[], &[]).is_empty());
    }

    #[test]
    fn differences_narrow_to_what_differs_and_add_what_is_asked_for() {
        let a = loaded(&log(
            Layout::Current,
            &[
                (1_000_000, "ATC_RAT_RLL_P", 0.135, Some(0.135)),
                (1_000_000, "BARO1_GND_PRESS", 97_682.0, None),
                (1_000_000, "COMPASS_OFS_X", -12.4, Some(0.0)),
                (1_000_000, "FENCE_RADIUS", 300.0, Some(300.0)),
                (1_000_000, "INS_GYROFFS_X", 0.01, Some(0.0)),
                (1_000_000, "OLD_NAME", 1.0, Some(1.0)),
            ],
        ));
        let b = loaded(&log(
            Layout::Current,
            &[
                (1_000_000, "ATC_RAT_RLL_P", 0.15, Some(0.135)),
                (1_000_000, "BARO1_GND_PRESS", 97_655.0, None),
                (1_000_000, "COMPASS_OFS_X", -10.0, Some(0.0)),
                (1_000_000, "FENCE_RADIUS", 300.0, Some(300.0)),
                (1_000_000, "INS_GYROFFS_X", 0.02, Some(0.0)),
                (1_000_000, "NEW_NAME", 2.0, Some(2.0)),
            ],
        ));
        let mut panel = ParamsPanel::default();
        // off, the table lists every name of either log, and the toggles
        // count what they would show and add
        let (rows, counts) = panel.rows(&a, Some(&b));
        assert_eq!(names(&rows).len(), 7);
        assert_eq!(
            counts,
            Counts {
                differ: 2,
                one_log: 2,
                learned: 2
            }
        );
        // on, it narrows to the names both have whose values differ,
        // a recalibrated compass among them and the learned values not
        panel.differences = true;
        let (rows, _) = panel.rows(&a, Some(&b));
        assert_eq!(names(&rows), ["ATC_RAT_RLL_P", "COMPASS_OFS_X"]);
        panel.learned = true;
        let (rows, counts) = panel.rows(&a, Some(&b));
        assert_eq!(
            names(&rows),
            [
                "ATC_RAT_RLL_P",
                "BARO1_GND_PRESS",
                "COMPASS_OFS_X",
                "INS_GYROFFS_X"
            ]
        );
        assert_eq!(counts.differ, 4);
        panel.one_log = true;
        let (rows, counts) = panel.rows(&a, Some(&b));
        assert_eq!(names(&rows).len(), 6);
        assert!(names(&rows).contains(&"NEW_NAME".to_string()));
        assert!(names(&rows).contains(&"OLD_NAME".to_string()));
        assert_eq!(counts.differ, 6);
        // the filter narrows the counts too
        panel.set_filter("compass");
        let (rows, counts) = panel.rows(&a, Some(&b));
        assert_eq!(names(&rows), ["COMPASS_OFS_X"]);
        assert_eq!(
            counts,
            Counts {
                differ: 1,
                one_log: 0,
                learned: 0
            }
        );
        // without B, Differences is not on screen and narrows nothing
        panel.set_filter("");
        let (rows, counts) = panel.rows(&a, None);
        assert_eq!(names(&rows).len(), 6);
        assert_eq!(counts, Counts::default());
    }

    #[test]
    fn a_change_with_no_time_is_told_without_one() {
        let p = Param {
            name: "X".into(),
            initial: 800.0,
            default: None,
            changes: vec![(f64::NAN, 1000.0), (250.0, 1200.0)],
        };
        assert_eq!(story(&p, None), "800 at boot, then 1000, 1200 from 4:10");
    }

    #[test]
    fn the_learned_list_takes_what_changes_by_itself_and_not_calibration() {
        for name in [
            "STAT_BOOTCNT",
            "BARO1_GND_PRESS",
            "BARO_GND_PRESS",
            "GND_ABS_PRESS",
            "GND_ABS_PRESS2",
            "INS_GYROFFS_X",
            "INS_GYR2OFFS_X",
            "INS_GYR1_CALTEMP",
            "MOT_THST_HOVER",
            "Q_M_THST_HOVER",
            "ARSPD_OFFSET",
            "ARSPD2_OFFSET",
            "COMPASS_DEC",
        ] {
            assert!(is_learned(name), "{name}");
        }
        for name in [
            "COMPASS_OFS_X",
            "INS_ACCOFFS_X",
            "INS_ACC2OFFS_Y",
            "MOT_THST_EXPO",
            "STAT",
            "COMPASS_DEC2",
            "XSTAT_RUNTIME",
            "INS_GYR_CAL",
        ] {
            assert!(!is_learned(name), "{name}");
        }
        assert!(glob("*", ""));
        assert!(glob("A*B*C", "AXBYC"));
        assert!(glob("A*B*C", "ABBC"));
        assert!(!glob("A*B", "AXBC"));
        assert!(!glob("AB", "A"));
    }

    #[test]
    fn rows_only_b_has_do_not_unfold_and_changes_count_both_logs() {
        let a = loaded(&log(
            Layout::Current,
            &[
                (1_000_000, "RTL_ALT", 1500.0, Some(1500.0)),
                (2_000_000, "RTL_ALT", 2000.0, None),
            ],
        ));
        let b = loaded(&log(
            Layout::Current,
            &[
                (1_000_000, "B_ONLY", 1.0, Some(1.0)),
                (1_000_000, "RTL_ALT", 1500.0, Some(1500.0)),
                (2_000_000, "B_ONLY", 2.0, None),
                (2_000_000, "RTL_ALT", 1800.0, None),
                (3_000_000, "RTL_ALT", 2000.0, None),
            ],
        ));
        let mut panel = ParamsPanel::default();
        panel
            .open
            .extend(["B_ONLY".to_string(), "RTL_ALT".to_string()]);
        let (rows, _) = panel.rows(&a, Some(&b));
        // B_ONLY, then RTL_ALT and A's history: the value at boot and its
        // one change
        assert_eq!(rows.len(), 4);
        let Row::Param(only_b, false) = rows[0] else {
            panic!("a name only B has does not unfold");
        };
        assert_eq!(only_b.name, "B_ONLY");
        assert_eq!(changes_text(&only_b), "0 / 1");
        assert_eq!(only_b.default(), Some(1.0), "B's, with none in A");
        let Row::Param(rtl, true) = rows[1] else {
            panic!("A's history unfolds");
        };
        assert_eq!(changes_text(&rtl), "1 / 2");
        assert!(matches!(rows[2], Row::History(_, 0)));
        assert!(matches!(rows[3], Row::History(_, 1)));
        // without B, A's count alone, and none for a parameter never set
        assert_eq!(changes_text(&Pair::only_a(rtl.a.unwrap())), "1");
        let (rows, _) = panel.rows(&b, None);
        let Row::Param(only_b, true) = rows[0] else {
            panic!("as A, its history unfolds");
        };
        assert_eq!(changes_text(&only_b), "1");
    }

    #[test]
    fn not_default_reads_a_log_without_defaults_against_the_other() {
        // A from firmware that logs no defaults, B from one that does
        let a = loaded(&log(
            Layout::Timed,
            &[
                (1_000_000, "FENCE_RADIUS", 300.0, None),
                (1_000_000, "RTL_ALT", 2000.0, None),
            ],
        ));
        let b = loaded(&log(
            Layout::Current,
            &[
                (1_000_000, "FENCE_RADIUS", 300.0, Some(300.0)),
                (1_000_000, "RTL_ALT", 1500.0, Some(1500.0)),
            ],
        ));
        let panel = ParamsPanel {
            off_default: true,
            ..ParamsPanel::default()
        };
        let (rows, _) = panel.rows(&a, Some(&b));
        assert_eq!(names(&rows), ["RTL_ALT"], "A's 2000 against B's default");
        // A alone has no defaults, so the toggle is off screen
        assert_eq!(names(&panel.rows(&a, None).0).len(), 2);
    }

    #[test]
    fn changed_after_boot_takes_a_change_in_either_log() {
        let a = loaded(&log(
            Layout::Current,
            &[
                (1_000_000, "X", 1.0, Some(1.0)),
                (1_000_000, "Y", 1.0, Some(1.0)),
                (1_000_000, "Z", 1.0, Some(1.0)),
                (2_000_000, "Y", 2.0, None),
            ],
        ));
        let b = loaded(&log(
            Layout::Current,
            &[
                (1_000_000, "X", 1.0, Some(1.0)),
                (1_000_000, "Y", 1.0, Some(1.0)),
                (1_000_000, "Z", 1.0, Some(1.0)),
                (2_000_000, "X", 3.0, None),
            ],
        ));
        let panel = ParamsPanel {
            changed: true,
            ..ParamsPanel::default()
        };
        assert_eq!(names(&panel.rows(&a, Some(&b)).0), ["X", "Y"]);
    }

    #[test]
    fn a_row_of_two_logs_tells_both_stories_on_hover() {
        let a = read_log(&log(
            Layout::Current,
            &[
                (1_000_000, "MOT_THST_HOVER", 0.42, Some(0.35)),
                (1_000_000, "WPNAV_SPEED", 800.0, Some(1000.0)),
                (250_000_000, "WPNAV_SPEED", 1000.0, None),
            ],
        ));
        let b = read_log(&log(
            Layout::Current,
            &[
                (1_000_000, "MOT_THST_HOVER", 0.44, Some(0.3)),
                (1_000_000, "WPNAV_SPEED", 1000.0, Some(1000.0)),
            ],
        ));
        let pairs = compare(&a, &b);
        let clocks = Clocks {
            a: None,
            b: None,
            with_b: true,
        };
        assert_eq!(
            note(&pairs[1], clocks).as_deref(),
            Some("Apart at boot: A 800 at boot, 1000 from 4:10; B 1000")
        );
        assert_eq!(
            note(&pairs[0], clocks).as_deref(),
            Some(
                "A 0.42; B 0.44\nB's default 0.3\n\
                 Learned: the autopilot sets it by itself from flight to flight"
            )
        );
        // on the UTC axis each log's story reads through its own clock: A
        // booted at 14:30:00, B two hours later
        let base = |unix_ms: i64| TimeBase {
            gps_start_unix_ms: unix_ms,
            ms_offset: 0,
        };
        let utc = Clocks {
            a: Some(base(1_778_769_000_000)),
            b: Some(base(1_778_776_200_000)),
            with_b: true,
        };
        let a_story = &a[1];
        let mut b_story = b[1].clone();
        b_story.changes = vec![(250.0, 1200.0)];
        let pair = Pair {
            name: "WPNAV_SPEED",
            a: Some(a_story),
            b: Some(&b_story),
        };
        assert_eq!(
            note(&pair, utc).as_deref(),
            Some("A 800 at boot, 1000 from 14:34:10; B 1000 at boot, 1200 from 16:34:10")
        );

        // many changes are told in part
        let mut p = a[1].clone();
        p.changes = (1..=6u8)
            .map(|i| (f64::from(i) * 60.0, 1000.0 + f32::from(i)))
            .collect();
        assert_eq!(
            story(&p, None),
            "800 at boot, 1001 from 1:00, 1002 from 2:00, 1003 from 3:00, 1004 from 4:00, and 2 more"
        );
    }
}
