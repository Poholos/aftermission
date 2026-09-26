//! The side panel: the log's summary and its message types, each
//! unfolding to its fields, or to its instances and their fields, with a
//! checkbox per numeric field that puts the series on the plot.

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
/// user left them.
pub fn types(
    ui: &mut egui::Ui,
    log: &LoadedLog,
    plot: &mut PlotPanel,
    filter: &Filter,
    open: Option<bool>,
) {
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
    if shown == 0 {
        ui.weak("Nothing matches the filter.");
    }
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
