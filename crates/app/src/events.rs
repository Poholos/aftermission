//! The events panel: the log's messages, errors, events, mode changes and
//! parameter changes in one list, in time order, a click on a row seeking
//! the plot and the map to its time, and a seek from the map or a
//! parameter change, or playback reaching the next event, bringing its
//! row into view.

use egui::{Rect, RichText};

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
    /// The event highlighted last frame, by its place in the log's
    /// events, so a filter or a toggle that renumbers the rows is not
    /// playback reaching another.
    highlighted: Option<usize>,
}

impl Default for EventsPanel {
    fn default() -> Self {
        Self {
            // parameter changes start hidden: a mission upload can log
            // dozens
            shown: KINDS.map(|kind| kind != EventKind::Param),
            filter: String::new(),
            highlighted: None,
        }
    }
}

impl EventsPanel {
    /// A new log is on: nothing of the last one is highlighted.
    pub fn reload(&mut self) {
        self.highlighted = None;
    }

    #[cfg(test)]
    pub(crate) fn set_filter(&mut self, filter: &str) {
        self.filter = filter.to_string();
    }

    /// The list, with the kind toggles and the filter above it. The row
    /// last passed by `cursor` is highlighted. `follow` says a seek has
    /// moved the cursor: the highlighted row is scrolled into view, or
    /// the top when the cursor is before every row, and `follow` is
    /// cleared, unless the filters leave no rows, when it waits for them.
    /// While `playing`, the list scrolls to each event the cursor reaches,
    /// unless the pointer is over the panel, so it can be scrolled by hand
    /// at any speed. The cursor's own moves, as the pointer sweeps the
    /// plot, leave the list where it is. The time of a clicked row is
    /// returned, to seek to.
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        log: &LoadedLog,
        settings: &Settings,
        cursor: Option<f64>,
        follow: &mut bool,
        playing: bool,
    ) -> Option<f64> {
        // the panel's whole room: nothing is laid out in it yet
        let pointed_at = ui.rect_contains_pointer(ui.max_rect());
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
        let rows: Vec<(usize, &Event)> = log
            .events
            .iter()
            .enumerate()
            .filter(|(_, e)| self.is_shown(e.kind) && filter.matches(&e.text))
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
            self.highlighted = None;
            return None;
        }
        // the row the cursor has passed most recently
        let current = cursor
            .map(|c| rows.partition_point(|(_, e)| e.time <= c))
            .and_then(|next| next.checked_sub(1));
        let highlighted = current.map(|row| rows[row].0);
        let reached = playing && highlighted != self.highlighted && !pointed_at;
        self.highlighted = highlighted;
        // a cursor before every row has passed none: the top then. A list
        // dragged too short for a row keeps the follow for when it has room.
        let room = ui.available_height() >= ui.spacing().interact_size.y;
        let scroll_to = (room && (reached || std::mem::take(follow))).then(|| current.unwrap_or(0));
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
                if let Some(index) = scroll_to {
                    // the row's place in the list, whether or not it is
                    // among the rows drawn: the scroll area lays the drawn
                    // ones out from `range.start`
                    let step = row_height + ui.spacing().item_spacing.y;
                    let top = ui.max_rect().top() + (index as f32 - range.start as f32) * step;
                    let row =
                        Rect::from_x_y_ranges(ui.max_rect().x_range(), top..=top + row_height);
                    ui.scroll_to_rect(row, None);
                }
                for index in range {
                    let (_, e) = rows[index];
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

#[cfg(test)]
mod tests {
    use super::*;
    use dflog::Log;
    use dflog::access::Value;
    use dflog::write::LogWriter;
    use egui_kittest::{Harness, kittest::Queryable as _};

    /// A log with sixty numbered messages, one a second from 1 s.
    fn notes() -> LoadedLog {
        let mut w = LogWriter::new();
        w.define(1, "MSG", "QZ", &["TimeUS", "Message"]).unwrap();
        for i in 0..60u64 {
            w.record(
                "MSG",
                &[
                    Value::U64(1_000_000 * (i + 1)),
                    Value::Str(format!("note {i:02}")),
                ],
            )
            .unwrap();
        }
        LoadedLog::build(Log::from_source(w.into_bytes().into()), "notes.bin".into())
    }

    /// The panel with what it shows and what it answered.
    struct Bench {
        panel: EventsPanel,
        log: LoadedLog,
        settings: Settings,
        cursor: Option<f64>,
        /// A seek has moved the cursor, as the app reports it.
        follow: bool,
        playing: bool,
        seek: Option<f64>,
        /// The top of the list's viewport, where row 0 sits unscrolled.
        top: f32,
        /// The bottom of the list's viewport, where the panel's room ends.
        bottom: f32,
    }

    /// Put the cursor at `time` by a seek, as the map does.
    fn seek_to(harness: &mut Harness<'_, Bench>, time: f64) {
        let state = harness.state_mut();
        state.cursor = Some(time);
        state.follow = true;
        harness.run();
    }

    /// Where note `i` is drawn, when it is among the rows drawn.
    fn row(harness: &Harness<'_, Bench>, i: usize) -> Option<egui::Rect> {
        harness
            .query_by_label_contains(&format!("note {i:02}"))
            .map(|node| node.rect())
    }

    /// Whether note `i` is wholly inside the list's viewport.
    fn in_view(harness: &Harness<'_, Bench>, i: usize) -> bool {
        let Bench { top, bottom, .. } = *harness.state();
        row(harness, i).is_some_and(|r| r.top() >= top - 0.5 && r.bottom() <= bottom + 0.5)
    }

    /// Whether note `i` is wholly above the viewport: nothing of it shows.
    fn above(harness: &Harness<'_, Bench>, i: usize) -> bool {
        let top = harness.state().top;
        row(harness, i).is_none_or(|r| r.bottom() <= top + 0.5)
    }

    /// Whether note `i` is wholly below the viewport: nothing of it shows.
    fn below(harness: &Harness<'_, Bench>, i: usize) -> bool {
        let bottom = harness.state().bottom;
        row(harness, i).is_none_or(|r| r.top() >= bottom - 0.5)
    }

    #[test]
    fn a_seek_scrolls_the_list_to_its_row_and_a_click_does_not() {
        let bench = Bench {
            panel: EventsPanel::default(),
            log: notes(),
            settings: Settings::default(),
            cursor: None,
            follow: false,
            playing: false,
            seek: None,
            top: 0.0,
            bottom: 0.0,
        };
        // a short panel, so most rows are off the end of the list; the
        // scroll animates over a few frames
        let mut harness = Harness::builder()
            .with_size(egui::vec2(600.0, 200.0))
            .with_max_steps(60)
            .build_ui_state(
                |ui, bench: &mut Bench| {
                    bench.bottom = ui.max_rect().bottom();
                    let seek = bench.panel.show(
                        ui,
                        &bench.log,
                        &bench.settings,
                        bench.cursor,
                        &mut bench.follow,
                        bench.playing,
                    );
                    if seek.is_some() {
                        bench.seek = seek;
                    }
                },
                bench,
            );
        harness.run();
        // unscrolled, row 0 marks where the viewport starts
        let top = row(&harness, 0).unwrap().top();
        harness.state_mut().top = top;
        assert!(in_view(&harness, 0));
        assert!(row(&harness, 45).is_none(), "off the end of the list");

        // the pointer sweeping the plot moves the cursor with no seek:
        // the list stays where it is
        harness.state_mut().cursor = Some(30.5);
        harness.run();
        assert!(in_view(&harness, 0));

        // a seek from the map: the list scrolls just far enough for the
        // row it highlights, the last it shows
        seek_to(&mut harness, 45.5);
        assert!(!harness.state().follow, "the list took the seek");
        assert!(in_view(&harness, 44), "note 44 highlighted");
        assert!(below(&harness, 45), "no further");
        assert!(above(&harness, 0));

        // a click on a row in view seeks, and the cursor the app moves
        // there leaves the list where it is
        let before = row(&harness, 42).unwrap();
        harness.get_by_label_contains("note 42").click();
        harness.run();
        let seek = harness.state().seek.unwrap();
        assert!((seek - 43.0).abs() < 1e-9, "{seek}");
        // the app does not ask the list to follow its own click
        harness.state_mut().cursor = Some(seek);
        harness.run();
        assert_eq!(row(&harness, 42), Some(before));

        // a seek up the list scrolls just far enough for its row, the
        // first it shows
        seek_to(&mut harness, 4.5);
        assert!(in_view(&harness, 3), "note 03 highlighted");
        assert!(above(&harness, 2), "no further");

        // a seek to before every row goes to the top
        seek_to(&mut harness, 45.5);
        seek_to(&mut harness, 0.5);
        assert!(in_view(&harness, 0));

        // with nothing to list the seek waits, and is followed once the
        // filter lets rows through again
        harness.state_mut().panel.set_filter("no such note");
        harness.run();
        seek_to(&mut harness, 45.5);
        assert!(harness.state().follow, "kept for when rows show");
        harness.state_mut().panel.set_filter("");
        harness.run();
        assert!(!harness.state().follow);
        assert!(in_view(&harness, 44));
    }

    /// [`notes`], with an error half a second before each note.
    fn notes_and_errors() -> LoadedLog {
        let mut w = LogWriter::new();
        w.define(1, "MSG", "QZ", &["TimeUS", "Message"]).unwrap();
        w.define(2, "ERR", "QBB", &["TimeUS", "Subsys", "ECode"])
            .unwrap();
        for i in 0..60u64 {
            w.record(
                "ERR",
                &[
                    Value::U64(1_000_000 * (i + 1) - 500_000),
                    Value::U64(3),
                    Value::U64(0),
                ],
            )
            .unwrap();
            w.record(
                "MSG",
                &[
                    Value::U64(1_000_000 * (i + 1)),
                    Value::Str(format!("note {i:02}")),
                ],
            )
            .unwrap();
        }
        LoadedLog::build(Log::from_source(w.into_bytes().into()), "both.bin".into())
    }

    #[test]
    fn while_playing_the_list_keeps_up_with_each_event_reached() {
        let bench = Bench {
            panel: EventsPanel::default(),
            log: notes_and_errors(),
            settings: Settings::default(),
            cursor: Some(0.2),
            follow: false,
            playing: true,
            seek: None,
            top: 0.0,
            bottom: 0.0,
        };
        let mut harness = Harness::builder()
            .with_size(egui::vec2(600.0, 200.0))
            .with_max_steps(60)
            .build_ui_state(
                |ui, bench: &mut Bench| {
                    bench.bottom = ui.max_rect().bottom();
                    bench.panel.show(
                        ui,
                        &bench.log,
                        &bench.settings,
                        bench.cursor,
                        &mut bench.follow,
                        bench.playing,
                    );
                },
                bench,
            );
        harness.run();
        // unscrolled, the first row, an error, marks where the viewport
        // starts
        let top = harness
            .get_all_by_label_contains("ERR ")
            .map(|node| node.rect().top())
            .fold(f32::INFINITY, f32::min);
        harness.state_mut().top = top;

        // playing, the cursor reaching note 44 brings it into view, with
        // no seek to follow
        harness.state_mut().cursor = Some(45.2);
        harness.run();
        assert!(in_view(&harness, 44));

        // scrolled back to the top by hand, the pointer over the list
        // holds it there while events pass
        let middle = egui::pos2(300.0, f32::midpoint(top, harness.state().bottom));
        harness.hover_at(middle);
        harness.event(egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: egui::vec2(0.0, 10_000.0),
            phase: egui::TouchPhase::Move,
            modifiers: egui::Modifiers::NONE,
        });
        harness.run();
        assert!(in_view(&harness, 0));
        harness.state_mut().cursor = Some(50.2);
        harness.run();
        assert!(in_view(&harness, 0));

        // with the pointer gone, a toggle that renumbers the rows reaches
        // no new event, and the list stays
        harness.event(egui::Event::PointerGone);
        harness.run();
        harness.state_mut().panel.shown[1] = false;
        harness.run();
        assert!(in_view(&harness, 0));

        // the next event reached brings the list along again
        harness.state_mut().cursor = Some(51.2);
        harness.run();
        assert!(in_view(&harness, 50));
        assert!(above(&harness, 0));

        // paused, events reached leave the list where it is
        harness.state_mut().playing = false;
        harness.state_mut().cursor = Some(10.2);
        harness.run();
        assert!(in_view(&harness, 50));
    }

    #[test]
    fn a_list_with_no_room_keeps_the_follow() {
        let bench = Bench {
            panel: EventsPanel::default(),
            log: notes(),
            settings: Settings::default(),
            cursor: Some(45.5),
            follow: true,
            playing: false,
            seek: None,
            top: 0.0,
            bottom: 0.0,
        };
        // room for the toggles and not a row under them
        let mut harness = Harness::builder()
            .with_size(egui::vec2(600.0, 40.0))
            .build_ui_state(
                |ui, bench: &mut Bench| {
                    bench.panel.show(
                        ui,
                        &bench.log,
                        &bench.settings,
                        bench.cursor,
                        &mut bench.follow,
                        bench.playing,
                    );
                },
                bench,
            );
        harness.run();
        assert!(harness.state().follow, "kept for when the list has room");
    }
}
