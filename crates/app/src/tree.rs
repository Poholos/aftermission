//! The side panel: the log's summary and its message types, each
//! unfolding to its fields, or to its instances and their fields, with a
//! checkbox per numeric field that puts the series on the plot. While a
//! second log, B, is open, what B logs and A does not is listed under
//! A's types, to plot from B.

use crate::filter::Filter;
use crate::model::{LoadedLog, MessageType, SeriesKey};
use crate::plot::PlotPanel;
use crate::settings::{Settings, TimeAxis};
use crate::timefmt;

/// The summary lines at the top of the panel.
pub fn summary(ui: &mut egui::Ui, log: &LoadedLog, settings: &Settings) {
    ui.strong(&log.name);
    let line = format!("{}, {} records", log.vehicle.name(), log.records());
    ui.label(match log.duration() {
        Some(duration) => format!("{line}, {}", timefmt::boot_time(duration, 1.0)),
        None => line,
    });
    if let Some(base) = log.time_base {
        let boot_ms = log.timeline.first().copied().unwrap_or(0.0) * 1000.0;
        let start = base.wall_clock_unix_ms(boot_ms);
        ui.label(format!(
            "Boot at {} {} UTC, from the first GPS fix",
            timefmt::utc_date(start),
            timefmt::utc_time(start, 1.0)
        ));
    } else {
        ui.weak("No GPS fix: the time axis reads since boot.");
        if settings.time_axis == TimeAxis::Utc {
            ui.weak("UTC is chosen but cannot be shown for this log.");
        }
    }
    if log.records() == 0 {
        ui.colored_label(
            ui.visuals().warn_fg_color,
            "No records found: is this a dataflash log?",
        );
    }
    for note in notes(log) {
        ui.colored_label(ui.visuals().warn_fg_color, note);
    }
}

/// What the scan skipped, one line each.
fn notes(log: &LoadedLog) -> Vec<String> {
    let mut notes = Vec::new();
    if log.stats.unknown_type_records > 0 {
        notes.push(format!(
            "unknown-type headers skipped: {}",
            log.stats.unknown_type_records
        ));
    }
    if log.stats.rejected_fmts > 0 {
        notes.push(format!(
            "malformed FMT records: {}",
            log.stats.rejected_fmts
        ));
    }
    if log.stats.truncated {
        notes.push("the last record is truncated".to_string());
    }
    notes
}

/// The message types, filtered by `filter` against type names and field
/// labels, with the fields as checkboxes that toggle series on `plot`.
/// `open` forces every header open or closed; None leaves them as the
/// user left them. Returns how many types it shows, for the caller to say
/// when nothing matches, here or in [`b_types`].
pub fn types(
    ui: &mut egui::Ui,
    log: &LoadedLog,
    plot: &mut PlotPanel,
    filter: &Filter,
    open: Option<bool>,
) -> usize {
    let mut shown = 0;
    for t in &log.types {
        let type_matches = filter.matches(&t.name);
        let field_matches = |label: &str| type_matches || filter.matches(label);
        if !type_matches && !t.fields.iter().any(|f| field_matches(&f.label)) {
            continue;
        }
        shown += 1;
        egui::CollapsingHeader::new(format!("{} ({})", t.name, t.count))
            .id_salt(("type", t.id))
            .open(open)
            .show(ui, |ui| {
                if t.instances.is_empty() {
                    fields(ui, log, plot, t, None, &field_matches);
                } else {
                    for &instance in &t.instances {
                        egui::CollapsingHeader::new(format!("{}[{instance}]", t.name))
                            .id_salt(("instance", t.id, instance))
                            .open(open)
                            .show(ui, |ui| {
                                fields(ui, log, plot, t, Some(instance), &field_matches);
                            });
                    }
                }
            });
    }
    shown
}

/// What only B logs, under A's types: a header, "Only in B (N)", closed
/// by default, N the types it shows, and inside each type laid out as
/// A's are, its name weaker, with a checkbox per field that plots it from
/// B. `filter` matches type names and field names and `open` forces the
/// headers, as in [`types`]. Returns how many types it shows.
pub fn b_types(
    ui: &mut egui::Ui,
    only: &[OnlyType],
    plot: &mut PlotPanel,
    filter: &Filter,
    open: Option<bool>,
) -> usize {
    let type_matches = |t: &OnlyType| filter.matches(&t.name);
    let shown: Vec<&OnlyType> = only
        .iter()
        .filter(|t| {
            type_matches(t)
                || t.groups
                    .iter()
                    .flat_map(|g| &g.fields)
                    .any(|f| filter.matches(&f.key.field))
        })
        .collect();
    if shown.is_empty() {
        return 0;
    }
    egui::CollapsingHeader::new(format!("Only in B ({})", shown.len()))
        .id_salt("only_in_b")
        .default_open(false)
        .open(open)
        .show(ui, |ui| {
            for t in &shown {
                let all = type_matches(t);
                let field_matches = |label: &str| all || filter.matches(label);
                egui::CollapsingHeader::new(
                    egui::RichText::new(format!("{} ({})", t.name, t.count)).weak(),
                )
                .id_salt(("b_type", t.id))
                .open(open)
                .show(ui, |ui| {
                    for g in &t.groups {
                        match g.instance {
                            Some(instance) => {
                                egui::CollapsingHeader::new(
                                    egui::RichText::new(format!("{}[{instance}]", t.name)).weak(),
                                )
                                .id_salt(("b_instance", t.id, instance))
                                .open(open)
                                .show(ui, |ui| {
                                    b_fields(ui, plot, &g.fields, &field_matches);
                                });
                            }
                            None => b_fields(ui, plot, &g.fields, &field_matches),
                        }
                    }
                });
            }
        });
    shown.len()
}

/// The checkboxes of fields only B has, which plot them from B.
fn b_fields(
    ui: &mut egui::Ui,
    plot: &mut PlotPanel,
    fields: &[OnlyField],
    matches: &dyn Fn(&str) -> bool,
) {
    for f in fields.iter().filter(|f| matches(&f.key.field)) {
        let mut on = plot.is_selected_b(&f.key);
        if ui.checkbox(&mut on, &f.title).changed() {
            plot.toggle_b(f.key.clone());
        }
    }
}

/// A type in the list of what only B logs: its format id in B, which its
/// headers are salted by, as A's are, since two formats can share a name;
/// its name and record count in B; and the fields only B has, by
/// instance, in B's order. Owned, so it can be kept between frames.
#[derive(Debug, Clone, PartialEq)]
pub struct OnlyType {
    pub id: u8,
    pub name: String,
    pub count: usize,
    pub groups: Vec<OnlyGroup>,
}

/// The fields only B has under one instance of a type, or under the type
/// for one logged as one instance.
#[derive(Debug, Clone, PartialEq)]
pub struct OnlyGroup {
    /// The instance, each under a header of its own as A's tree shows
    /// them; None for a type B logs as one instance.
    pub instance: Option<i64>,
    pub fields: Vec<OnlyField>,
}

/// A field only B has: its key in B, whose field name the filter
/// matches, and its title, the name with B's unit, which its checkbox
/// shows.
#[derive(Debug, Clone, PartialEq)]
pub struct OnlyField {
    pub key: SeriesKey,
    pub title: String,
}

/// What `b` logs that `a` does not, by type, in `b`'s order: whole types,
/// fields of a type both log, and instances of one. A field counts as
/// `a`'s when `a`'s type of that name has a key for it
/// ([`MessageType::key_for`]), so an instance and a single instance that
/// stand for each other are never listed. Text fields are left out: they
/// cannot be plotted. The two type lists are walked together, both
/// sorted by name, so no type is searched for.
#[must_use]
pub fn only_in(b: &LoadedLog, a: &LoadedLog) -> Vec<OnlyType> {
    let mut a_types = a.types.iter().peekable();
    let mut only = Vec::new();
    for bt in &b.types {
        // A's types named before this one are none of B's
        while a_types.next_if(|at| at.name < bt.name).is_some() {}
        let at = a_types.peek().copied().filter(|at| at.name == bt.name);
        let instances: Vec<Option<i64>> = if bt.instances.is_empty() {
            vec![None]
        } else {
            bt.instances.iter().copied().map(Some).collect()
        };
        let groups: Vec<OnlyGroup> = instances
            .into_iter()
            .filter_map(|instance| {
                let fields: Vec<OnlyField> = bt
                    .fields
                    .iter()
                    .filter(|f| f.numeric)
                    .filter_map(|f| {
                        let key = SeriesKey {
                            type_name: bt.name.clone(),
                            field: f.label.clone(),
                            instance,
                        };
                        at.and_then(|at| at.key_for(&key))
                            .is_none()
                            .then(|| OnlyField {
                                key,
                                title: f.title(),
                            })
                    })
                    .collect();
                (!fields.is_empty()).then_some(OnlyGroup { instance, fields })
            })
            .collect();
        if !groups.is_empty() {
            only.push(OnlyType {
                id: bt.id,
                name: bt.name.clone(),
                count: bt.count,
                groups,
            });
        }
    }
    only
}

/// The checkboxes of one type, or of one of its instances.
fn fields(
    ui: &mut egui::Ui,
    log: &LoadedLog,
    plot: &mut PlotPanel,
    t: &MessageType,
    instance: Option<i64>,
    matches: &dyn Fn(&str) -> bool,
) {
    for field in t.fields.iter().filter(|f| matches(&f.label)) {
        if !field.numeric {
            ui.weak(format!("{} (text)", field.label));
            continue;
        }
        let key = SeriesKey {
            type_name: t.name.clone(),
            field: field.label.clone(),
            instance,
        };
        let mut on = plot.is_selected(&key);
        if ui.checkbox(&mut on, field.title()).changed() {
            plot.toggle(key, log);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::testlog;
    use dflog::Log;

    fn log(bytes: &[u8]) -> LoadedLog {
        LoadedLog::build(Log::from_bytes(bytes), "t.bin".into())
    }

    /// Each listed field as `TYPE[instance].Field`.
    fn labels(only: &[OnlyType]) -> Vec<String> {
        only.iter()
            .flat_map(|t| &t.groups)
            .flat_map(|g| &g.fields)
            .map(|f| f.key.label())
            .collect()
    }

    #[test]
    fn only_b_s_types_fields_and_instances_are_listed() {
        let a = log(&testlog::bytes());
        let b = log(&testlog::b_extra_bytes());
        let only = only_in(&b, &a);
        let names: Vec<&str> = only.iter().map(|t| t.name.as_str()).collect();
        // a field ATT gained, the third IMU, and VIBE, whose text field
        // is left out; the tables both log list nothing
        assert_eq!(names, ["ATT", "IMU", "VIBE"]);
        assert_eq!(
            labels(&only),
            [
                "ATT.DesRoll",
                "IMU[2].TimeUS",
                "IMU[2].I",
                "IMU[2].GyrX",
                "IMU[2].GyrY",
                "IMU[2].GyrZ",
                "VIBE.TimeUS",
                "VIBE.VibeX",
            ]
        );
        let imu = &only[1];
        assert_eq!(imu.groups.len(), 1);
        assert_eq!(imu.groups[0].instance, Some(2));
        let vibe = &only[2];
        assert_eq!(vibe.groups[0].instance, None);
        assert_eq!((vibe.id, vibe.count), (13, 4));
        // the title is the field's name with B's unit, where B gives one
        assert_eq!(only[0].groups[0].fields[0].title, "DesRoll");
        assert_eq!(imu.groups[0].fields[2].title, "GyrX (rad/s)");
    }

    #[test]
    fn one_instance_and_instance_0_stand_for_each_other() {
        // A's IMU as one instance of TimeUS, I and GyrX; B's as IMU[0]
        // and IMU[1] with GyrY and GyrZ besides: IMU[0] stands for A's,
        // so only its two fields A lacks are listed, and IMU[1] whole
        let a = log(&testlog::b_bytes());
        let b = log(&testlog::bytes());
        let only = only_in(&b, &a);
        let imu = only.iter().find(|t| t.name == "IMU").unwrap();
        assert_eq!(
            labels(std::slice::from_ref(imu)),
            [
                "IMU[0].GyrY",
                "IMU[0].GyrZ",
                "IMU[1].TimeUS",
                "IMU[1].I",
                "IMU[1].GyrX",
                "IMU[1].GyrY",
                "IMU[1].GyrZ",
            ]
        );
        // and the other way round, nothing of IMU
        let only = only_in(&a, &b);
        assert!(only.iter().all(|t| t.name != "IMU"), "{only:?}");
    }

    #[test]
    fn two_logs_of_one_layout_list_nothing() {
        let a = log(&testlog::bytes());
        let b = log(&testlog::bytes());
        assert!(only_in(&b, &a).is_empty());
    }
}
