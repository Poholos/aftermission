//! The events panel: the log's messages, errors, events, mode changes and
//! parameter changes in one list, in time order, a click on a row seeking
//! the plot and the map to its time, and a seek from the map or a
//! parameter change, or playback reaching the next event, bringing its
//! row into view. While a second log, B, is open, its events can join the
//! list at their times on A's clock.

use egui::{Rect, RichText};

use crate::filter::Filter;
use crate::model::{Event, EventKind, LoadedLog};
use crate::settings::Settings;
use crate::timefmt;
use crate::worker::Slot;

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
    /// While B is open, B's events in the list too.
    with_b: bool,
    filter: String,
    /// The event highlighted last frame, by its log and its place in that
    /// log's events, so a filter or a toggle that renumbers the rows is
    /// not playback reaching another.
    highlighted: Option<(Slot, usize)>,
    /// The list was scrolled by hand, by the wheel, a swipe or a drag,
    /// and the pointer has stayed over it since: playback leaves it where
    /// it was put.
    scrolled_by_hand: bool,
    /// The hand was a finger: lifted, it leaves no pointer to stay over
    /// the list or to leave it, so the hold lasts until a seek the list
    /// follows, or a touch outside it.
    by_touch: bool,
}

impl Default for EventsPanel {
    fn default() -> Self {
        Self {
            // parameter changes start hidden: a mission upload can log
            // dozens
            shown: KINDS.map(|kind| kind != EventKind::Param),
            with_b: false,
            filter: String::new(),
            highlighted: None,
            scrolled_by_hand: false,
            by_touch: false,
        }
    }
}

/// One row of the list: an event of A's or of B's, with its time on A's
/// clock, where it sits in the list.
#[derive(Clone, Copy)]
struct Row<'a> {
    slot: Slot,
    /// Its place in its log's events.
    index: usize,
    time: f64,
    event: &'a Event,
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
    /// a pointer resting over it included, until the list is scrolled by
    /// hand: then it stays where it was put while the pointer stays over
    /// the panel, so it can be read at any speed, and follows again once
    /// the pointer leaves. The cursor's own moves, as the pointer sweeps the
    /// plot, leave the list where it is. With `b` open, B's events join
    /// the list when its toggle is on, each at its time moved onto A's
    /// clock by the offset, tagged B, lighter, and hovered with its time
    /// on B's own clock; the kind toggles and the filter apply to both.
    /// The time of a clicked row is returned, on its own log's clock, with
    /// whose event it is, to seek to.
    #[expect(
        clippy::too_many_arguments,
        reason = "what the list shows each frame, each from another owner"
    )]
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        log: &LoadedLog,
        b: Option<(&LoadedLog, f64)>,
        settings: &Settings,
        cursor: Option<f64>,
        follow: &mut bool,
        playing: bool,
    ) -> Option<(Slot, f64)> {
        // the panel's whole room: nothing is laid out in it yet
        self.note_hand_scroll(ui, ui.max_rect());
        ui.horizontal(|ui| {
            for (kind, shown) in KINDS.iter().zip(&mut self.shown) {
                ui.toggle_value(shown, kind_name(*kind));
            }
            if b.is_some() {
                ui.toggle_value(&mut self.with_b, "B's events")
                    .on_hover_text("B's events too, at their times on A's clock");
            }
            ui.add(
                egui::TextEdit::singleline(&mut self.filter)
                    .hint_text("Filter events")
                    .desired_width(180.0),
            );
        });
        // B's events count only while its toggle is on screen
        let b = b.filter(|_| self.with_b);
        let rows = self.rows(log, b);
        if rows.is_empty() {
            let logs = || [Some(log), b.map(|(b, _)| b)].into_iter().flatten();
            ui.weak(if logs().all(|l| l.events.is_empty()) {
                "No messages, errors or events in this log."
            } else if !logs().any(|l| l.events.iter().any(|e| self.is_shown(e.kind))) {
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
            .map(|c| rows.partition_point(|row| row.time <= c))
            .and_then(|next| next.checked_sub(1));
        let highlighted = current.map(|row| (rows[row].slot, rows[row].index));
        let reached = playing && highlighted != self.highlighted && !self.scrolled_by_hand;
        self.highlighted = highlighted;
        // a cursor before every row has passed none: the top then. A list
        // dragged too short for a row keeps the follow for when it has room.
        let room = ui.available_height() >= ui.spacing().interact_size.y;
        let followed = room && std::mem::take(follow);
        // a seek followed is where the finger that swiped meant to go: the
        // hold it left, which no pointer leaving ends, ends there
        if followed && self.by_touch {
            self.scrolled_by_hand = false;
        }
        let scroll_to = (room && (reached || followed)).then(|| current.unwrap_or(0));
        let time_base = log.wall_clock(settings.time_axis);
        let time_base_b = b.and_then(|(b, _)| b.wall_clock(settings.time_axis));
        let error_color = ui.visuals().error_fg_color;
        let weak_color = ui.visuals().weak_text_color();

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
                    let row = rows[index];
                    let e = row.event;
                    let whose = row.slot.prefix();
                    let text = format!(
                        "{}  {:<4} {whose}{}",
                        timefmt::stamp(time_base, row.time),
                        kind_tag(e.kind),
                        e.text
                    );
                    let mut label = RichText::new(text).monospace();
                    let color = match (e.kind, row.slot) {
                        (EventKind::Error, Slot::A) => Some(error_color),
                        (EventKind::Error, Slot::B) => Some(error_color.gamma_multiply(0.6)),
                        (_, Slot::B) => Some(weak_color),
                        (_, Slot::A) => None,
                    };
                    if let Some(color) = color {
                        label = label.color(color);
                    }
                    let mut response = ui.selectable_label(current == Some(index), label);
                    if row.slot == Slot::B {
                        response = response
                            .on_hover_text(format!("B's {}", timefmt::stamp(time_base_b, e.time)));
                    }
                    if response.clicked() {
                        seek = Some((row.slot, e.time));
                    }
                }
            });
        seek
    }

    /// Note a scroll by hand: the wheel with the pointer over `panel`, or
    /// a drag that began in it, a swipe included. The user scrolls the
    /// list, and keeps it until the pointer leaves. A drag from elsewhere,
    /// as of the transport's slider or the panel's edge, scrolls nothing,
    /// wherever it ends; and a finger lifted leaves no pointer, which
    /// keeps the hold.
    fn note_hand_scroll(&mut self, ui: &egui::Ui, panel: egui::Rect) {
        let pointed_at = ui.rect_contains_pointer(panel);
        let (scrolling, touching, gone) = ui.input(|i| {
            let wheel = pointed_at && i.smooth_scroll_delta != egui::Vec2::ZERO;
            let drag = i.pointer.is_decidedly_dragging()
                && i.pointer
                    .press_origin()
                    .is_some_and(|at| panel.contains(at));
            (
                wheel || drag,
                i.any_touches(),
                i.pointer.latest_pos().is_none(),
            )
        });
        if scrolling {
            self.by_touch = touching;
        }
        self.scrolled_by_hand =
            scrolling || (self.scrolled_by_hand && (pointed_at || (gone && self.by_touch)));
    }

    /// The events the kind toggles and the filter let through, A's and,
    /// with `b`, B's moved onto A's clock, in time order, A's first of any
    /// at the same time. Each log's events are in time order already, so
    /// the two merge in one pass, as they do each frame.
    fn rows<'a>(&self, log: &'a LoadedLog, b: Option<(&'a LoadedLog, f64)>) -> Vec<Row<'a>> {
        let filter = Filter::new(&self.filter);
        let of = |slot: Slot, log: &'a LoadedLog, offset: f64| {
            log.events
                .iter()
                .enumerate()
                .filter(|(_, e)| self.is_shown(e.kind) && filter.matches(&e.text))
                .map(move |(index, event)| Row {
                    slot,
                    index,
                    time: event.time + offset,
                    event,
                })
        };
        let Some((b, offset)) = b else {
            return of(Slot::A, log, 0.0).collect();
        };
        let (mut a, mut b) = (
            of(Slot::A, log, 0.0).peekable(),
            of(Slot::B, b, offset).peekable(),
        );
        let mut rows = Vec::new();
        loop {
            // A's before B's at the same time
            let next = match (a.peek(), b.peek()) {
                (Some(x), Some(y)) if y.time < x.time => b.next(),
                (Some(_), _) => a.next(),
                (None, _) => b.next(),
            };
            let Some(row) = next else {
                return rows;
            };
            rows.push(row);
        }
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

    /// The height of a strip above the list, as the app's tabs are, for
    /// a drag to start from.
    const STRIP: f32 = 40.0;

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
                        None,
                        &bench.settings,
                        bench.cursor,
                        &mut bench.follow,
                        bench.playing,
                    );
                    if let Some((_, time)) = seek {
                        bench.seek = Some(time);
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

    /// The list of the notes-and-errors log, playing from its start and
    /// drawn once, unscrolled: its first row, an error, marks where the
    /// viewport starts.
    fn playing() -> Harness<'static, Bench> {
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
                    ui.add_space(STRIP);
                    ui.scope(|ui| {
                        bench.panel.show(
                            ui,
                            &bench.log,
                            None,
                            &bench.settings,
                            bench.cursor,
                            &mut bench.follow,
                            bench.playing,
                        );
                    });
                },
                bench,
            );
        harness.run();
        let top = harness
            .get_all_by_label_contains("ERR ")
            .map(|node| node.rect().top())
            .fold(f32::INFINITY, f32::min);
        harness.state_mut().top = top;
        harness
    }

    /// Where a drag of the list starts: its middle.
    fn middle(harness: &Harness<'_, Bench>) -> egui::Pos2 {
        egui::pos2(
            300.0,
            f32::midpoint(harness.state().top, harness.state().bottom),
        )
    }

    /// A finger's drag down the list from its middle, in three steps of
    /// 20 px, lifted at the end, so the pointer is gone.
    fn swipe(harness: &mut Harness<'_, Bench>) {
        let middle = middle(harness);
        let touch = |phase| egui::Event::Touch {
            device_id: egui::TouchDeviceId(0),
            id: egui::TouchId(0),
            phase,
            pos: middle,
            force: None,
        };
        harness.event(touch(egui::TouchPhase::Start));
        harness.hover_at(middle);
        harness.run();
        harness.drag_at(middle);
        harness.run();
        for step in 1..=3u8 {
            harness.hover_at(middle + egui::vec2(0.0, 20.0 * f32::from(step)));
            harness.run();
        }
        harness.event(egui::Event::PointerButton {
            pos: middle + egui::vec2(0.0, 60.0),
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        });
        harness.event(touch(egui::TouchPhase::End));
        harness.event(egui::Event::PointerGone);
        harness.run();
    }

    #[test]
    fn while_playing_the_list_keeps_up_with_each_event_reached() {
        let mut harness = playing();
        let middle = middle(&harness);

        // playing, the cursor reaching note 44 brings it into view, with
        // no seek to follow
        harness.state_mut().cursor = Some(45.2);
        harness.run();
        assert!(in_view(&harness, 44));

        // a pointer resting over the list leaves it following
        harness.hover_at(middle);
        harness.run();
        harness.state_mut().cursor = Some(47.2);
        harness.run();
        assert!(in_view(&harness, 46));

        // scrolled back to the top by hand, the pointer over the list
        // holds it there while events pass
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

        // a drag over the list, as of its scroll bar, holds it too, while
        // the pointer stays over it: released there, events pass it by
        harness.hover_at(middle);
        harness.run();
        harness.drag_at(middle);
        harness.run();
        for step in 1..=3u8 {
            harness.hover_at(middle + egui::vec2(0.0, 20.0 * f32::from(step)));
            harness.run();
        }
        harness.event(egui::Event::PointerButton {
            pos: middle + egui::vec2(0.0, 60.0),
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        });
        harness.run();
        harness.state_mut().cursor = Some(55.2);
        harness.run();
        assert!(in_view(&harness, 50));
        assert!(below(&harness, 54));
        // and the pointer gone, the next event brings it along
        harness.event(egui::Event::PointerGone);
        harness.run();
        harness.state_mut().cursor = Some(56.2);
        harness.run();
        assert!(in_view(&harness, 55));

        // paused, events reached leave the list where it is
        harness.state_mut().playing = false;
        harness.state_mut().cursor = Some(10.2);
        harness.run();
        assert!(in_view(&harness, 55));
    }

    #[test]
    fn a_drag_from_above_the_list_is_no_scroll_of_it() {
        let mut harness = playing();
        harness.state_mut().cursor = Some(45.2);
        harness.run();
        assert!(in_view(&harness, 44));

        // pressed above the list, as on the transport's slider, and
        // released in it: no scroll of the list, which follows on with
        // the pointer resting over it
        let above = egui::pos2(300.0, STRIP / 2.0);
        let middle = middle(&harness);
        harness.hover_at(above);
        harness.run();
        harness.drag_at(above);
        harness.run();
        for step in 1..=3u8 {
            let y = above.y + (middle.y - above.y) * f32::from(step) / 3.0;
            harness.hover_at(egui::pos2(300.0, y));
            harness.run();
        }
        harness.event(egui::Event::PointerButton {
            pos: middle,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        });
        harness.run();
        assert!(!harness.state().panel.scrolled_by_hand);
        harness.state_mut().cursor = Some(47.2);
        harness.run();
        assert!(in_view(&harness, 46));
    }

    #[test]
    fn a_swipe_holds_the_list_until_a_seek_it_follows() {
        let mut harness = playing();
        harness.state_mut().cursor = Some(45.2);
        harness.run();
        assert!(in_view(&harness, 44));

        // a swipe scrolls the list back by hand; the finger lifted leaves
        // no pointer to leave the list, and the hold stays through the
        // events reached
        swipe(&mut harness);
        assert!(harness.state().panel.by_touch);
        harness.state_mut().cursor = Some(48.2);
        harness.run();
        assert!(below(&harness, 47));

        // until a seek the list follows: there, and along again after
        harness.state_mut().follow = true;
        harness.run();
        assert!(in_view(&harness, 47));
        assert!(!harness.state().panel.scrolled_by_hand);
        harness.state_mut().cursor = Some(49.2);
        harness.run();
        assert!(in_view(&harness, 48));
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
                        None,
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

    #[test]
    fn b_events_join_the_list_at_their_moved_times() {
        let a = notes();
        let b = notes_and_errors();
        let mut panel = EventsPanel::default();
        let lines = |rows: &[Row<'_>]| -> Vec<String> {
            rows.iter()
                .map(|r| format!("{:?} {:.1} {}", r.slot, r.time, r.event.text))
                .collect()
        };
        // off by default, and ignored while B is not open
        assert_eq!(panel.rows(&a, None).len(), 60);
        panel.with_b = true;
        // B moved 0.25 s later: its note 00 at 1.25 s, its first error at
        // 0.75 s, both among A's notes in time order
        let rows = panel.rows(&a, Some((&b, 0.25)));
        assert_eq!(rows.len(), 180);
        assert_eq!(
            lines(&rows[..4]),
            [
                "B 0.8 Compass: resolved",
                "A 1.0 note 00",
                "B 1.2 note 00",
                "B 1.8 Compass: resolved"
            ]
        );
        assert_eq!((rows[2].index, rows[2].event.time), (1, 1.0));
        // A's first where both have an event at the same time
        let rows = panel.rows(&a, Some((&b, 0.0)));
        assert_eq!(lines(&rows[1..3]), ["A 1.0 note 00", "B 1.0 note 00"]);

        // the kind toggles and the filter apply to both
        panel.shown[1] = false;
        assert_eq!(panel.rows(&a, Some((&b, 0.25))).len(), 120);
        panel.filter = "note 01".into();
        let rows = panel.rows(&a, Some((&b, 0.25)));
        assert_eq!(lines(&rows), ["A 2.0 note 01", "B 2.2 note 01"]);
    }

    #[test]
    fn a_click_on_a_b_event_asks_for_its_time_on_b_clock() {
        let a = notes();
        let b = notes_and_errors();
        let mut seek = None;
        let mut panel = EventsPanel {
            filter: "note 59".into(),
            ..EventsPanel::default()
        };
        let settings = Settings::default();
        let mut follow = false;
        let mut harness = Harness::builder()
            .with_size(egui::vec2(600.0, 400.0))
            .build_ui(|ui| {
                if let Some(asked) = panel.show(
                    ui,
                    &a,
                    Some((&b, 100.0)),
                    &settings,
                    None,
                    &mut follow,
                    false,
                ) {
                    seek = Some(asked);
                }
            });
        harness.run();
        // the toggle shows while B is open, and is off
        harness.get_by_label_contains("MSG  note 59");
        assert!(harness.query_by_label_contains("MSG  B note 59").is_none());
        harness.get_by_label("B's events").click();
        harness.run();
        // B's note, tagged, 100 s after its own time, on A's clock
        harness
            .get_by_label_contains("2:40.000  MSG  B note 59")
            .click();
        harness.run();
        drop(harness);
        assert_eq!(seek, Some((Slot::B, 60.0)));
    }
}
