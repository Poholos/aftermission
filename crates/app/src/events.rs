//! The events panel: the log's messages, errors, events, mode changes and
//! parameter changes in one list, in time order, a click on a row seeking
//! the plot and the map to its time.

use egui::RichText;

use crate::filter::Filter;
use crate::model::{Event, EventKind, LoadedLog};
use crate::settings::Settings;
use crate::timefmt;

/// The kinds of event, in the order the panel offers them.
const KINDS: [EventKind; 5] = [
    EventKind::Message,
    EventKind::Error,
    EventKind::Event,
    EventKind::Mode,
    EventKind::Param,
];

/// The panel's state: which kinds are listed and a text filter.
#[derive(Debug)]
pub struct EventsPanel {
    /// Per kind of [`KINDS`].
    shown: [bool; 5],
    filter: String,
}

impl Default for EventsPanel {
    fn default() -> Self {
        Self {
            // parameter changes start hidden: a mission upload can log
            // dozens
            shown: KINDS.map(|kind| kind != EventKind::Param),
            filter: String::new(),
        }
    }
}

impl EventsPanel {
    #[cfg(test)]
    pub(crate) fn set_filter(&mut self, filter: &str) {
        self.filter = filter.to_string();
    }

    /// The list, with the kind toggles and the filter above it. The row
    /// last passed by `cursor` is highlighted; the time of a clicked row
    /// is returned, to seek to.
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        log: &LoadedLog,
        settings: &Settings,
        cursor: Option<f64>,
    ) -> Option<f64> {
        ui.horizontal(|ui| {
            for (kind, shown) in KINDS.iter().zip(&mut self.shown) {
                ui.toggle_value(shown, kind_name(*kind));
            }
            ui.add(
                egui::TextEdit::singleline(&mut self.filter)
                    .hint_text("Filter events")
                    .desired_width(180.0),
            );
        });
        let filter = Filter::new(&self.filter);
        let rows: Vec<&Event> = log
            .events
            .iter()
            .filter(|e| self.is_shown(e.kind) && filter.matches(&e.text))
            .collect();
        if rows.is_empty() {
            ui.weak(if log.events.is_empty() {
                "No messages, errors or events in this log."
            } else if !log.events.iter().any(|e| self.is_shown(e.kind)) {
                // parameter changes start hidden, and may be all a log has
                "Every kind this log has is toggled off."
            } else {
                "Nothing matches."
            });
            // the panel keeps its height: it stores what its content used
            ui.take_available_space();
            return None;
        }
        // the row the cursor has passed most recently
        let current = cursor
            .map(|c| rows.partition_point(|e| e.time <= c))
            .and_then(|next| next.checked_sub(1));
        let time_base = log.wall_clock(settings.time_axis);
        let error_color = ui.visuals().error_fg_color;

        let mut seek = None;
        let row_height = ui.spacing().interact_size.y;
        // the list takes the height the panel gives it, as the parameter
        // table does, so switching tabs does not resize the panel
        egui::ScrollArea::vertical()
            .auto_shrink(false)
            .min_scrolled_height(0.0)
            .show_rows(ui, row_height, rows.len(), |ui, range| {
                for index in range {
                    let e = rows[index];
                    let text = format!(
                        "{}  {:<4} {}",
                        timefmt::stamp(time_base, e.time),
                        kind_tag(e.kind),
                        e.text
                    );
                    let mut label = RichText::new(text).monospace();
                    if e.kind == EventKind::Error {
                        label = label.color(error_color);
                    }
                    if ui.selectable_label(current == Some(index), label).clicked() {
                        seek = Some(e.time);
                    }
                }
            });
        seek
    }

    fn is_shown(&self, kind: EventKind) -> bool {
        KINDS
            .iter()
            .position(|k| *k == kind)
            .is_some_and(|i| self.shown[i])
    }
}

/// The toggle's caption.
fn kind_name(kind: EventKind) -> &'static str {
    match kind {
        EventKind::Message => "Messages",
        EventKind::Error => "Errors",
        // not "Events": the bottom panel's tab has that name
        EventKind::Event => "Vehicle events",
        EventKind::Mode => "Modes",
        EventKind::Param => "Parameter changes",
    }
}

/// The short tag in front of a row, from the record type it came from.
fn kind_tag(kind: EventKind) -> &'static str {
    match kind {
        EventKind::Message => "MSG",
        EventKind::Error => "ERR",
        EventKind::Event => "EV",
        EventKind::Mode => "MODE",
        EventKind::Param => "PARM",
    }
}
