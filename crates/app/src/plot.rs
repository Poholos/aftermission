//! The plot: the chosen series against time, each on the left or the right
//! axis, over the flight mode bands, with a readout of every series at the
//! cursor, and the playhead that playback and every seek move. While a
//! second log, B, is open, every series draws B's samples of the field
//! too, lighter and moved onto A's clock, and the readout reads both.

use std::fmt::Write as _;
use std::ops::RangeInclusive;
use std::sync::Arc;

use egui::{Align2, Color32, Id, Pos2, Rect, Shape, Stroke, pos2};
use egui_plot::{
    AxisHints, Corner, GridMark, HPlacement, Legend, Line, Plot, PlotBounds, PlotGeometry,
    PlotItem, PlotItemBase, PlotMemory, PlotPoint, PlotPoints, PlotTransform, Span,
};

use crate::model::{
    LoadedLog, ModeChange, Series, SeriesKey, extent, key_in, nearest_index, nearest_within,
};
use crate::settings::Settings;
use crate::timefmt;

/// Which vertical axis a series reads against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum YAxis {
    Left,
    Right,
}

impl YAxis {
    fn flipped(self) -> Self {
        match self {
            YAxis::Left => YAxis::Right,
            YAxis::Right => YAxis::Left,
        }
    }
}

/// A series on the plot.
#[derive(Debug)]
pub struct Selected {
    pub key: SeriesKey,
    /// `IMU[1].GyrX (rad/s)`, as A names the field; as B names it for a
    /// series for B's side alone.
    pub title: String,
    pub axis: YAxis,
    pub color: Color32,
    pub data: Series,
    /// The lowest and highest value, found once; None for an empty series.
    extent: Option<(f64, f64)>,
    /// The field's unit in A.
    unit: Option<String>,
    /// B's samples of the field, on B's clock, while B is open; None where
    /// B lacks it.
    pub data_b: Option<Series>,
    /// B's lowest and highest value over the samples the whole-flight
    /// view draws; see [`PlotPanel::fit_b_extents`].
    extent_b: Option<(f64, f64)>,
    /// The window on B's clock `extent_b` was found over, by its bits;
    /// None until it is found.
    extent_b_over: Option<(u64, u64)>,
    /// The field's unit in B.
    unit_b: Option<String>,
    /// Where B's samples are those of another instance, which stands for
    /// A's: `B's GPS[0].Spd`.
    b_note: Option<String>,
    /// The key A's side was read by, which the side panel ticks: the
    /// series' own, or after a swap that of the instance standing for it
    /// in the new A (see [`key_in`]). None for a series for B's side
    /// alone: picked from what only B logs, or kept by a swap whose new A
    /// lacks the field.
    a_key: Option<SeriesKey>,
    /// The key B's side was read by, the mirror of `a_key`, which the side
    /// panel's list of what only B logs ticks: set with `data_b` and
    /// cleared with it. None while B is closed or lacks the field.
    b_key: Option<SeriesKey>,
    /// A's line as a frame last thinned it, and what for, given again to
    /// a frame that asks for the same; None until a frame thins it, and
    /// cleared with `data`. See [`Thinning`].
    thinned: Option<Thinning>,
    /// B's line's, cleared with `data_b`.
    thinned_b: Option<Thinning>,
    /// The blocks of `data`, built with it, so a view spanning many
    /// samples a pixel is thinned by the block; see [`Summary`].
    summary: Summary,
    /// The blocks of `data_b`, built with it.
    summary_b: Option<Summary>,
}

impl Selected {
    fn new(key: SeriesKey, title: String, axis: YAxis, color: Color32, data: Series) -> Self {
        let a_key = Some(key.clone());
        Self {
            key,
            title,
            axis,
            color,
            extent: extent(data.ys.iter().copied()),
            summary: Summary::new(&data.ys),
            data,
            unit: None,
            data_b: None,
            extent_b: None,
            extent_b_over: None,
            unit_b: None,
            b_note: None,
            a_key,
            b_key: None,
            thinned: None,
            thinned_b: None,
            summary_b: None,
        }
    }

    /// Whether A lacks the field, and the series stands for B's side
    /// alone, which it goes with.
    fn only_b(&self) -> bool {
        self.a_key.is_none()
    }

    fn replace_data(&mut self, data: Series) {
        self.extent = extent(data.ys.iter().copied());
        self.summary = Summary::new(&data.ys);
        self.data = data;
        self.thinned = None;
    }

    /// Take B's side of the series from `b`, by the check a new log's
    /// series pass, so a type logged in several instances is never read
    /// as one; see [`key_in`]. None clears it.
    fn extract_b(&mut self, b: Option<&LoadedLog>) {
        self.data_b = None;
        self.summary_b = None;
        self.thinned_b = None;
        self.extent_b = None;
        self.extent_b_over = None;
        self.unit_b = None;
        self.b_note = None;
        self.b_key = None;
        let Some((b, key)) = b.and_then(|b| Some((b, key_in(b, &self.key)?))) else {
            return;
        };
        let Ok(data) = b.series(&key) else {
            return;
        };
        if key.instance != self.key.instance {
            self.b_note = Some(format!("B's {}", key.label()));
        }
        self.unit_b = unit(b, &key);
        self.extent_b = extent(data.ys.iter().copied());
        self.summary_b = Some(Summary::new(&data.ys));
        self.data_b = Some(data);
        // a series for B's side alone is named as this B names the field,
        // as one picked from what only B logs is, whichever B it is now
        if self.only_b() {
            self.title = title(b, &key);
        }
        self.b_key = Some(key);
    }

    /// Find B's extent over its samples within `window`, on B's clock,
    /// unless it was found over that window already.
    fn fit_b_extent(&mut self, window: (f64, f64)) {
        let over = (window.0.to_bits(), window.1.to_bits());
        if self.extent_b_over == Some(over) {
            return;
        }
        self.extent_b_over = Some(over);
        self.extent_b = self.data_b.as_ref().and_then(|data| {
            let (xs, ys) = (&data.xs, &data.ys);
            let from = xs.partition_point(|x| *x < window.0);
            let to = xs.partition_point(|x| *x <= window.1);
            // where the line crosses an edge, the point drawn on it, as
            // [`clip`] draws it, counts too: a sample just past an edge
            // sizes the axis by what shows of the line to it
            let crossing = |i: usize, x: f64| {
                (i > 0 && i < xs.len() && xs[i - 1] < x && x < xs[i]).then(|| {
                    ys[i - 1] + (ys[i] - ys[i - 1]) * (x - xs[i - 1]) / (xs[i] - xs[i - 1])
                })
            };
            let edges = [crossing(from, window.0), crossing(to, window.1)];
            extent(
                ys.get(from..to)?
                    .iter()
                    .copied()
                    .chain(edges.into_iter().flatten()),
            )
        });
    }

    /// Whether A and B give the field units that differ, so that their
    /// values do not compare. A heading is in degrees as any angle is,
    /// which firmware has logged under either unit.
    fn units_differ(&self) -> bool {
        fn kind(unit: &str) -> &str {
            if unit == "degheading" { "deg" } else { unit }
        }
        matches!((&self.unit, &self.unit_b), (Some(a), Some(b)) if kind(a) != kind(b))
    }

    /// What the chip says on hover while B is open: that B lacks the
    /// field, which instance of B's stands for A's, and a unit of B's that
    /// is not A's, against which no difference is read.
    fn b_hover(&self) -> Option<String> {
        if self.only_b() {
            return Some("not in A".to_string());
        }
        if self.data_b.is_none() {
            return Some("not in B".to_string());
        }
        let unit = self
            .unit_b
            .as_deref()
            .filter(|_| self.units_differ())
            .map(|unit| format!("B logs it in {unit}: no \u{394} is read"));
        match (self.b_note.clone(), unit) {
            (Some(note), Some(unit)) => Some(format!("{note}; {unit}")),
            (note, unit) => note.or(unit),
        }
    }

    /// Whether the field is an angle in degrees, whose difference goes the
    /// short way round: by A's unit, else B's.
    fn is_angle(&self) -> bool {
        matches!(
            self.unit.as_deref().or(self.unit_b.as_deref()),
            Some("deg" | "degheading")
        )
    }

    /// The lowest and highest value of both sides.
    fn extent_both(&self) -> Option<(f64, f64)> {
        match (self.extent, self.extent_b) {
            (Some((a, b)), Some((c, d))) => Some((a.min(c), b.max(d))),
            (one, other) => one.or(other),
        }
    }
}

/// One series' samples nearest the cursor: A's, and B's while B is open.
#[derive(Debug, Clone, PartialEq)]
pub struct Readout {
    pub title: String,
    pub color: Color32,
    /// A's sample, its time since boot and its value. With B open, None
    /// reads as a dash: a hidden line, or a cursor outside its samples.
    pub a: Option<(f64, f64)>,
    /// B's sample nearest the cursor less the offset, its time on B's
    /// clock; None reads as a dash, and is all there is with no B open.
    pub b: Option<(f64, f64)>,
    /// A's value less B's, the short way round for an angle; None reads
    /// as a dash.
    pub difference: Option<f64>,
}

/// Log B as the plot holds it: the log, and the seconds that move its
/// times onto A's clock.
#[derive(Debug)]
struct PlotB {
    log: Arc<LoadedLog>,
    offset: f64,
}

/// The plot's state: what is drawn, where the playhead is and what the
/// cursor is over.
#[derive(Debug, Default)]
pub struct PlotPanel {
    pub selected: Vec<Selected>,
    next_color: usize,
    /// The samples at the cursor, one per series. The pointer leaving
    /// the plot leaves values in it, so they stay readable and the row
    /// keeps its height; empty until the cursor has been anywhere.
    pub readout: Vec<Readout>,
    /// Where playback is and goes on from, in board time: every seek
    /// moves it, and so does playing. None until the first of either on
    /// this log.
    playhead: Option<f64>,
    /// Board time the readout reads at, which the map marks and the
    /// events list highlights: the pointer's while it moves over the plot
    /// with playback paused, the playhead's otherwise. Before the first
    /// seek there is no playhead to go back to, and the cursor stays
    /// where the pointer last was.
    pub cursor: Option<f64>,
    /// Where the pointer was over the plot last frame, on screen. The
    /// cursor follows the pointer only as it moves there, so a pause, a
    /// step, or a view that pans under a resting pointer leaves the
    /// cursor at the playhead.
    hover: Option<Pos2>,
    /// The view paging last set, to tell it from one the user has set
    /// since.
    paged: Option<RangeInclusive<f64>>,
    /// The playhead at the end of the last frame played, to tell a
    /// playhead that left the view from a view moved off it; None while
    /// paused.
    last_played: Option<f64>,
    /// Why the last series could not be added.
    pub error: Option<String>,
    /// Show the whole of the data on the next frame: set when the log
    /// changes under the plot, whose zoom would otherwise carry over.
    reset_view: bool,
    /// The series hidden through the legend, by the id their line carries,
    /// as of the last frame; they stay out of the readout.
    hidden: Vec<Id>,
    /// A time to bring into view on the next frame.
    pending_seek: Option<f64>,
    /// A seek the events list has yet to bring into view: it may be on
    /// the other tab, or filtered empty, when the seek is made.
    follow: bool,
    /// Log B while it is open, set through [`Self::set_b`].
    b: Option<PlotB>,
    /// B changed: the lines hidden for the B before are shown again on
    /// the next frame, where the plot's memory is.
    show_b_lines: bool,
    /// A and B swapped: on the next frame, where the plot's memory is,
    /// the hidden lines swap sides and a zoomed view moves by these
    /// seconds onto the new A's clock.
    swapped: Option<f64>,
    /// Lines thinned afresh since the plot was made, a statistic the
    /// tests read: a frame whose view holds adds none.
    misses: usize,
}

/// The plot's id, fixed so its memory can be read back.
const PLOT_ID: &str = "aftermission_plot";

/// The id a series' line carries, which the legend hides it by.
fn line_id(key: &SeriesKey) -> Id {
    Id::new(key)
}

/// The id of a series' line of B's, which the legend hides by itself.
fn line_id_b(key: &SeriesKey) -> Id {
    Id::new((key, "B"))
}

/// How much of a series' color B's line keeps: half its opacity.
const B_OPACITY: f32 = 0.5;

/// Distinct hues for successive series.
const PALETTE: [Color32; 10] = [
    Color32::from_rgb(77, 148, 255),
    Color32::from_rgb(255, 140, 40),
    Color32::from_rgb(70, 200, 90),
    Color32::from_rgb(235, 70, 70),
    Color32::from_rgb(170, 120, 240),
    Color32::from_rgb(200, 150, 90),
    Color32::from_rgb(240, 120, 200),
    Color32::from_rgb(160, 160, 160),
    Color32::from_rgb(200, 200, 50),
    Color32::from_rgb(60, 200, 220),
];

impl PlotPanel {
    /// Whether A's field `key` is on the plot: a series whose A side was
    /// read by it, as after a swap one keyed by another instance's can be.
    #[must_use]
    pub fn is_selected(&self, key: &SeriesKey) -> bool {
        self.selected.iter().any(|s| s.a_key.as_ref() == Some(key))
    }

    /// Add A's field `key` to the plot, or take it off when it is on, by
    /// the rule [`Self::is_selected`] ticks it by. A series that cannot be
    /// extracted leaves its reason in `error`.
    pub fn toggle(&mut self, key: SeriesKey, log: &LoadedLog) {
        if let Some(index) = self
            .selected
            .iter()
            .position(|s| s.a_key.as_ref() == Some(&key))
        {
            self.selected.remove(index);
            self.refresh_readout();
            return;
        }
        match log.series(&key) {
            Ok(data) => {
                let color = PALETTE[self.next_color % PALETTE.len()];
                self.next_color += 1;
                let title = title(log, &key);
                let unit = unit(log, &key);
                let mut s = Selected::new(key, title, YAxis::Left, color, data);
                s.unit = unit;
                s.extract_b(self.b.as_ref().map(|b| &*b.log));
                self.selected.push(s);
                self.error = None;
                self.refresh_readout();
            }
            Err(message) => self.error = Some(message),
        }
    }

    /// Whether B's field `key` is on the plot as a series for B's side
    /// alone: one whose B side was read by `key`, so a series a swap left
    /// in the other instance form of B's key ticks too. A compare per
    /// series, as the side panel asks it for every checkbox of what only
    /// B logs, every frame.
    #[must_use]
    pub fn is_selected_b(&self, key: &SeriesKey) -> bool {
        self.selected
            .iter()
            .any(|s| s.only_b() && s.b_key.as_ref() == Some(key))
    }

    /// Add B's field `key`, one A lacks, to the plot as a series for B's
    /// side alone, or take it off when it is on, by the rule
    /// [`Self::is_selected_b`] ticks it by. It is read from the B the plot
    /// holds, so it is read and ticked by the same B; without B, or where
    /// B lacks it, nothing changes.
    pub fn toggle_b(&mut self, key: SeriesKey) {
        if let Some(index) = self
            .selected
            .iter()
            .position(|s| s.only_b() && s.b_key.as_ref() == Some(&key))
        {
            self.selected.remove(index);
            self.refresh_readout();
            return;
        }
        let Some(b) = self.b.as_ref().map(|b| Arc::clone(&b.log)) else {
            return;
        };
        let color = PALETTE[self.next_color % PALETTE.len()];
        let title = title(&b, &key);
        let mut s = Selected::new(key, title, YAxis::Left, color, Series::default());
        // B's alone: no A side, so neither A's tree nor the CSV export
        // takes it for one of A's
        s.a_key = None;
        s.extract_b(Some(&b));
        if s.data_b.is_none() {
            return;
        }
        self.next_color += 1;
        self.selected.push(s);
        self.error = None;
        self.refresh_readout();
    }

    /// Whether any series on the plot has an A side, hidden through the
    /// legend or not: what tells a plot of B's lines alone, which no CSV
    /// export writes, from one whose A lines are hidden.
    #[must_use]
    pub fn has_a_series(&self) -> bool {
        self.selected.iter().any(|s| !s.only_b())
    }

    pub fn remove(&mut self, index: usize) {
        if index < self.selected.len() {
            self.selected.remove(index);
            self.refresh_readout();
        }
    }

    /// The readout for the current series at the last cursor position.
    fn refresh_readout(&mut self) {
        let offset = self.b.as_ref().map(|b| b.offset);
        self.readout = match self.cursor {
            Some(cursor) => readout_at(&self.selected, cursor, &self.hidden, offset),
            None => Vec::new(),
        };
    }

    /// Open log B beside A, moved onto A's clock by `offset` seconds, or
    /// close it with None: B's side of every series is taken when the log
    /// changes, and the readout reads again, so a new offset shows at once.
    pub fn set_b(&mut self, b: Option<(Arc<LoadedLog>, f64)>) {
        let same = match (&self.b, &b) {
            (Some(old), Some((log, _))) => Arc::ptr_eq(&old.log, log),
            (old, new) => old.is_none() && new.is_none(),
        };
        self.b = b.map(|(log, offset)| PlotB { log, offset });
        if !same {
            let b = self.b.as_ref().map(|b| &*b.log);
            for s in &mut self.selected {
                s.extract_b(b);
            }
            // a series A lacks, kept by a swap for B's side, goes with it
            self.selected.retain(|s| !s.only_b() || s.data_b.is_some());
            // B's lines keep their ids from one B to the next, and a pair
            // is set up afresh
            self.show_b_lines = true;
            let shown: Vec<Id> = self.selected.iter().map(|s| line_id_b(&s.key)).collect();
            self.hidden.retain(|id| !shown.contains(id));
        }
        self.refresh_readout();
    }

    /// Swap A and B under the series, as the pair's Swap does: `a` is
    /// the new A, and `b`, moved onto its clock by `offset`, the new B.
    /// Every series stays, A's side taken from `a`, or none where `a`
    /// lacks the field, so that a second swap brings back what the first
    /// took. The lines hidden through the legend swap sides, and the
    /// playhead, the cursor and a zoomed view move by `offset` onto the
    /// new A's clock, where the same moment is, the playhead kept within
    /// the new A's range, and a zoomed view then panned onto it.
    pub fn swap(&mut self, a: &LoadedLog, b: Arc<LoadedLog>, offset: f64) {
        for s in &mut self.selected {
            // A's side found as B's is, so the instance B drew a moment
            // ago for this series is A's now; the series keeps its key
            if let Some((Ok(data), key)) = key_in(a, &s.key).map(|key| (a.series(&key), key)) {
                s.replace_data(data);
                s.title = title(a, &key);
                s.unit = unit(a, &key);
                s.a_key = Some(key);
            } else {
                s.replace_data(Series::default());
                s.unit = None;
                s.a_key = None;
            }
            s.extract_b(Some(&b));
        }
        self.selected.retain(|s| !s.only_b() || s.data_b.is_some());
        self.b = Some(PlotB { log: b, offset });
        // the readout reads the swapped sides until the next frame reads
        // the plot's memory, swapped the same way then
        let mut hidden: Vec<Id> = self.hidden.iter().map(|id| self.other_side(*id)).collect();
        hidden.sort_unstable_by_key(Id::value);
        self.hidden = hidden;
        let range = a.time_range();
        let held = |time: f64| match &range {
            Some(range) => time.clamp(*range.start(), *range.end()),
            None => time,
        };
        let moved = self.playhead.map(|time| time + offset);
        self.playhead = moved.map(held);
        // a zoomed view moves by the offset alone, so a playhead held
        // within the new A's range can be off it: the view pans to it
        // then, as to a seek
        self.pending_seek = moved
            .filter(|time| range.as_ref().is_some_and(|range| !range.contains(time)))
            .map(held);
        self.cursor = self.cursor.map(|time| held(time + offset));
        self.hover = None;
        self.paged = None;
        self.last_played = None;
        self.swapped = Some(offset);
        self.refresh_readout();
    }

    /// Take every series off; the next one picked takes the first color
    /// again. The playhead stays, the cursor goes back to it, and a seek
    /// the events list has yet to follow still waits for it.
    pub fn clear(&mut self) {
        self.selected.clear();
        self.next_color = 0;
        self.readout.clear();
        self.cursor = self.playhead;
    }

    /// Where playback is and goes on from.
    #[must_use]
    pub fn playhead(&self) -> Option<f64> {
        self.playhead
    }

    /// Put the playhead and the cursor at `time`, panning the view there
    /// when it is off screen: what a click on the plot, in the events
    /// list, on a parameter change or on the map does, and the
    /// transport's steps and slider. A time that is not finite, as a
    /// record logged before the first timed one has, is no place to go.
    pub fn seek(&mut self, time: f64) {
        if !time.is_finite() {
            return;
        }
        self.playhead = Some(time);
        self.cursor = Some(time);
        self.pending_seek = Some(time);
        self.follow = true;
        self.refresh_readout();
    }

    /// Move the playhead and the cursor to `time` as playing does, the
    /// view left to paging.
    pub fn play_to(&mut self, time: f64) {
        self.playhead = Some(time);
        self.cursor = Some(time);
        self.refresh_readout();
    }

    /// Whether a seek waits for the events list to follow it, for the
    /// list to clear once it has.
    pub fn follow_mut(&mut self) -> &mut bool {
        &mut self.follow
    }

    /// Re-extract every series from a newly opened log, dropping those it
    /// does not have, so a plot set up on one flight carries to the next.
    /// A series is read by the key its A side was read by, the field on
    /// screen and ticked, then by its own, and takes the key found: B goes,
    /// as a new A closes it, and with it the reason the two differed. The
    /// view resets to the whole of the new log.
    pub fn reload(&mut self, log: &LoadedLog) {
        self.b = None;
        self.selected.retain_mut(|s| {
            s.extract_b(None);
            let found = [s.a_key.clone(), Some(s.key.clone())]
                .into_iter()
                .flatten()
                .find_map(|key| match log.series(&key) {
                    Ok(data) if log.has_series(&key) => Some((key, data)),
                    _ => None,
                });
            let Some((key, data)) = found else {
                return false;
            };
            s.replace_data(data);
            s.title = title(log, &key);
            s.unit = unit(log, &key);
            s.key = key.clone();
            s.a_key = Some(key);
            true
        });
        self.readout.clear();
        self.playhead = None;
        self.cursor = None;
        self.hover = None;
        self.paged = None;
        self.last_played = None;
        self.follow = false;
        self.reset_view = true;
    }

    /// The series showing: the selected ones not hidden through the
    /// legend, as of the last frame. What the readout reads and the CSV
    /// export writes.
    pub fn showing(&self) -> impl Iterator<Item = &Selected> {
        self.selected
            .iter()
            .filter(|s| !s.only_b() && !self.hidden.contains(&line_id(&s.key)))
    }

    /// The time range in view while the plot is zoomed or panned; None
    /// while the view follows the data and shows all of it.
    #[must_use]
    pub fn zoomed_range(&self, ctx: &egui::Context) -> Option<RangeInclusive<f64>> {
        let memory = PlotMemory::load(ctx, Id::new(PLOT_ID))?;
        (!memory.auto_bounds.x && !self.reset_view).then(|| memory.bounds().range_x())
    }

    /// Whether the next frame shows the whole of the data, for tests.
    #[cfg(test)]
    pub(crate) fn resets_view(&self) -> bool {
        self.reset_view
    }

    /// Zoom the view to `range` of time, the values fitting what it shows,
    /// as a box zoom drawn around the data does; for tests, once the plot
    /// has drawn.
    #[cfg(test)]
    pub(crate) fn zoom_to(ctx: &egui::Context, range: RangeInclusive<f64>) {
        let id = Id::new(PLOT_ID);
        let mut memory = PlotMemory::load(ctx, id).expect("the plot has drawn");
        let [_, bottom] = memory.bounds().min();
        let [_, top] = memory.bounds().max();
        memory.set_bounds(egui_plot::PlotBounds::from_min_max(
            [*range.start(), bottom],
            [*range.end(), top],
        ));
        memory.auto_bounds = egui::Vec2b::new(false, true);
        memory.store(ctx, id);
    }

    /// Thin every line afresh, hidden ones too, over the range and the
    /// width the plot's memory holds, and give the points' count: what a
    /// frame that has nothing kept pays to thin, for the frame-cost bench
    /// (`frame_cost` in the app's tests). Nothing while the plot has not
    /// drawn.
    #[cfg(test)]
    pub(crate) fn thin_all(&self, ctx: &egui::Context) -> usize {
        let Some(memory) = PlotMemory::load(ctx, Id::new(PLOT_ID)) else {
            return 0;
        };
        let transform = memory.transform();
        let visible = transform.bounds().range_x();
        let b_offset = self.b.as_ref().map(|b| b.offset);
        let (range, within_range) = lines_range(
            &self.selected,
            &self.hidden,
            b_offset,
            memory.auto_bounds.x,
            &visible,
        );
        let lines = Lines {
            map: AxisMap::new(&self.selected),
            range: &range,
            width: transform.frame().width(),
            b_offset,
            within_range,
        };
        self.selected
            .iter()
            .map(|s| {
                let Thinned { b, a } = lines.thin(s);
                b.map_or(0, |points| points.len()) + a.map_or(0, |points| points.len())
            })
            .sum()
    }

    /// The series chips, the lane of flight mode names, the plot and the
    /// cursor readout. The readout takes a panel at the bottom sized to its
    /// lines, and the plot the rest. While `playing`, the pointer leaves
    /// the cursor alone and a zoomed view pages to keep the playhead on
    /// screen.
    pub fn show(&mut self, ui: &mut egui::Ui, log: &LoadedLog, settings: &Settings, playing: bool) {
        self.series_bar(ui);
        egui::Panel::bottom("plot_readout")
            .resizable(false)
            .show_separator_line(false)
            .show(ui, |ui| self.readout_bar(ui, log, settings));
        // a second line for B's mode names while B is open, and has some
        let lanes = if self.b_modes().is_some() { 2 } else { 1 };
        let strip = settings.show_modes.then(|| mode_strip(ui, lanes));
        let plot_height = ui.available_height().max(120.0);
        self.plot(ui, log, settings, plot_height, playing, strip);
    }

    /// One chip per series: its color, name, axis and a way off the plot.
    /// While B is open, a chip whose field B lacks says so on hover, one
    /// whose B side is another instance names it, and one whose unit B
    /// gives otherwise says so.
    fn series_bar(&mut self, ui: &mut egui::Ui) {
        if self.selected.is_empty() {
            ui.weak("Pick fields in the panel on the left to plot them.");
            return;
        }
        let b_open = self.b.is_some();
        let mut remove = None;
        ui.horizontal_wrapped(|ui| {
            for (index, s) in self.selected.iter_mut().enumerate() {
                ui.colored_label(s.color, "\u{25A0}");
                // a series for B's side alone is marked as B's, weaker,
                // so it does not read as one of A's
                let name = if s.only_b() {
                    ui.weak(format!("B {}", s.title))
                } else {
                    ui.label(&s.title)
                };
                if b_open && let Some(text) = s.b_hover() {
                    name.on_hover_text(text);
                }
                let axis = match s.axis {
                    YAxis::Left => "L",
                    YAxis::Right => "R",
                };
                if ui
                    .small_button(axis)
                    .on_hover_text("Left or right axis")
                    .clicked()
                {
                    s.axis = s.axis.flipped();
                }
                if ui.small_button("\u{d7}").on_hover_text("Remove").clicked() {
                    remove = Some(index);
                }
                ui.add_space(8.0);
            }
        });
        if let Some(index) = remove {
            self.remove(index);
        }
    }

    fn plot(
        &mut self,
        ui: &mut egui::Ui,
        log: &LoadedLog,
        settings: &Settings,
        height: f32,
        playing: bool,
        strip: Option<Rect>,
    ) {
        // A reset rebuilds the plot's memory with nothing hidden and the
        // whole of the data in view.
        let reset = std::mem::take(&mut self.reset_view);
        if std::mem::take(&mut self.show_b_lines) {
            self.show_b_lines_in_memory(ui.ctx());
        }
        if let Some(offset) = self.swapped.take() {
            self.swap_in_memory(ui.ctx(), offset);
        }
        self.sync_hidden(ui.ctx(), reset);
        self.fit_b_extents();
        let map = AxisMap::new(&self.selected);
        let plot = new_plot(log, settings, map, height, playing, reset);

        let selected = &mut self.selected;
        let hidden = &self.hidden;
        let (playhead, cursor, last_hover) = (self.playhead, self.cursor, self.hover);
        let paged = self.paged.clone();
        let last_played = self.last_played;
        let seek = self.pending_seek.take();
        let range = log.time_range();
        let b_offset = self.b.as_ref().map(|b| b.offset);
        let show_modes = settings.show_modes;
        let playhead_stroke = Stroke::new(1.5, ui.visuals().strong_text_color());
        let response = plot.show(ui, |plot_ui| {
            // Where the pointer is while it is over the plot, and its time.
            let response = plot_ui.response();
            let at = response.hover_pos();
            let pointer = plot_ui
                .pointer_coordinate()
                .filter(|_| at.is_some())
                .map(|p| p.x);
            // A double-click shows the whole log again, and nothing else
            // moves the view in its frame, which would turn the reset off.
            let resetting = response.double_clicked();
            // A click seeks, within the log's time, as the margins around
            // the data reach past it: not the second click of a
            // double-click. A drag, a box zoom and the legend's entries
            // are no click on the plot.
            let click = pointer
                .filter(|_| response.clicked() && !resetting)
                .map(|x| range.as_ref().map_or(x, |r| x.clamp(*r.start(), *r.end())));
            let playhead = click.or(playhead);
            let cursor = if click.is_some() {
                click
            } else if let Some(x) = pointer.filter(|_| !playing && at != last_hover) {
                Some(x)
            } else if at.is_none() && last_hover.is_some() {
                // the pointer has left: back to the playhead
                playhead.or(cursor)
            } else {
                cursor
            };

            let bounds = plot_ui.plot_bounds();
            // the bounds follow the data, or will from this frame's reset
            let auto_x = plot_ui.auto_bounds().x || resetting;
            // A seek to a time off screen pans a zoomed view there, keeping
            // its width; a view showing the whole log already has it.
            // Paging moves a zoomed view while playing. A move takes effect
            // after this closure, so what is drawn here follows the new
            // window rather than last frame's.
            let mut visible = bounds.range_x();
            if let Some(time) = seek.filter(|t| !auto_x && !visible.contains(t)) {
                let half = bounds.width() / 2.0;
                visible = time - half..=time + half;
                plot_ui.set_plot_bounds_x(visible.clone());
            }
            let mut paged = paged;
            if let Some(time) = playhead.filter(|_| playing && !auto_x)
                && let Some(page) = page(&visible, paged.as_ref(), last_played, time)
            {
                plot_ui.set_plot_bounds_x(page.clone());
                visible = page.clone();
                paged = Some(page);
            }
            let (x_range, within_range) = lines_range(selected, hidden, b_offset, auto_x, &visible);
            let width = plot_ui.transform().frame().width();

            if show_modes {
                mode_bands(plot_ui, log, &visible);
            }
            let lines = Lines {
                map,
                range: &x_range,
                width,
                b_offset,
                within_range,
            };
            let misses = lines.draw_all(plot_ui, selected, hidden);

            time_marks(plot_ui, cursor, playhead, playhead_stroke);
            Found {
                at,
                click,
                cursor,
                paged,
                misses,
            }
        });

        if let Some(strip) = strip {
            self.mode_lanes(ui, log, &response.transform, strip);
        }
        let found = response.inner;
        self.misses += found.misses;
        self.hover = found.at;
        self.paged = found.paged;
        if let Some(time) = found.click {
            self.seek(time);
        } else if found.cursor != self.cursor {
            self.cursor = found.cursor;
            self.refresh_readout();
        }
        self.last_played = self.playhead.filter(|_| playing);
    }

    /// The line on the other side of the one `id` names: B's line of the
    /// series whose A line it is, or the reverse; `id` itself where it is
    /// no line of a series on the plot. What a swap does to the hidden
    /// lines, in the readout's set and in the plot's memory alike.
    fn other_side(&self, id: Id) -> Id {
        for s in &self.selected {
            let (a, b) = (line_id(&s.key), line_id_b(&s.key));
            if id == a {
                return b;
            }
            if id == b {
                return a;
            }
        }
        id
    }

    /// Show the lines of B's the legend hid in the plot's memory, which
    /// the legend reads its ticks from.
    fn show_b_lines_in_memory(&self, ctx: &egui::Context) {
        let id = Id::new(PLOT_ID);
        let Some(mut memory) = PlotMemory::load(ctx, id) else {
            return;
        };
        for s in &self.selected {
            memory.hidden_items.remove(&line_id_b(&s.key));
        }
        memory.store(ctx, id);
    }

    /// Swap the sides of the lines hidden through the legend in the
    /// plot's memory, and move a zoomed view by `offset` onto the new A's
    /// clock; a view that follows the data fits the new A by itself.
    fn swap_in_memory(&self, ctx: &egui::Context, offset: f64) {
        let id = Id::new(PLOT_ID);
        let Some(mut memory) = PlotMemory::load(ctx, id) else {
            return;
        };
        let swapped = memory
            .hidden_items
            .drain()
            .map(|id| self.other_side(id))
            .collect();
        memory.hidden_items = swapped;
        if !memory.auto_bounds.x {
            let bounds = memory.bounds();
            let ([left, bottom], [right, top]) = (bounds.min(), bounds.max());
            memory.set_bounds(PlotBounds::from_min_max(
                [left + offset, bottom],
                [right + offset, top],
            ));
        }
        memory.store(ctx, id);
    }

    /// Fit B's side of each series' extent to what the whole-flight view
    /// draws of it, as [`Lines::thin_b`] clips it: its samples within the
    /// range of A's lines, moved onto B's clock, or all of them where A
    /// has none. A value of B's never drawn then sizes no axis. Lines
    /// hidden through the legend count as shown, as they do in
    /// [`AxisMap`], so a click on the legend moves no other line.
    fn fit_b_extents(&mut self) {
        let Some(offset) = self.b.as_ref().map(|b| b.offset) else {
            return;
        };
        let window = full_range(&self.selected, &[])
            .map_or((f64::NEG_INFINITY, f64::INFINITY), |range| {
                (range.start() - offset, range.end() - offset)
            });
        for s in &mut self.selected {
            s.fit_b_extent(window);
        }
    }

    /// B, while it is open and logs mode changes to name.
    fn b_modes(&self) -> Option<&PlotB> {
        self.b.as_ref().filter(|b| !b.log.modes.is_empty())
    }

    /// The mode names in their own lane above the plot, where neither the
    /// legend nor the data covers them, painted rather than plot items, so
    /// they do not enter the bounds the plot fits its data in; B's in a
    /// lane under A's, the two sharing `strip`.
    fn mode_lanes(&self, ui: &egui::Ui, log: &LoadedLog, transform: &PlotTransform, strip: Rect) {
        let Some(b) = self.b_modes() else {
            mode_labels(ui, log, 0.0, transform, strip, false);
            return;
        };
        let (lane, lane_b) = strip.split_top_bottom_at_fraction(0.5);
        mode_labels(ui, log, 0.0, transform, lane, false);
        mode_labels(ui, &b.log, b.offset, transform, lane_b, true);
    }

    /// Take the series hidden through the legend from the plot's memory,
    /// to leave them out of the readout too. On a `reset` frame the plot
    /// rebuilds its memory with nothing hidden, so the stored set no
    /// longer holds. The id of a line no longer drawn, as of a series
    /// taken off while hidden, is dropped from the memory: the plot drops
    /// them only while its legend has an entry, and kept, it would hide
    /// the line again when the series comes back.
    fn sync_hidden(&mut self, ctx: &egui::Context, reset: bool) {
        let id = Id::new(PLOT_ID);
        let mut hidden = Vec::new();
        if !reset && let Some(mut memory) = PlotMemory::load(ctx, id) {
            let lines: Vec<Id> = self
                .selected
                .iter()
                .flat_map(|s| [line_id(&s.key), line_id_b(&s.key)])
                .collect();
            let before = memory.hidden_items.len();
            memory.hidden_items.retain(|id| lines.contains(id));
            hidden = memory.hidden_items.iter().copied().collect();
            if hidden.len() != before {
                memory.store(ctx, id);
            }
        }
        // sorted, so the same set always compares equal
        hidden.sort_unstable_by_key(Id::value);
        if hidden != self.hidden {
            self.hidden = hidden;
            self.refresh_readout();
        }
    }

    /// Time and value of every series at the cursor, or a hint. While B
    /// is open, B's value and the difference follow A's; no time of B's
    /// shows, only how far off its sample lies.
    fn readout_bar(&self, ui: &mut egui::Ui, log: &LoadedLog, settings: &Settings) {
        ui.horizontal_wrapped(|ui| {
            let Some(cursor) = self.cursor else {
                ui.weak(
                    "Click to seek, drag or scroll to pan, Ctrl+scroll or pinch to zoom, \
                     right-drag a box to zoom into it, double-click to see the whole log.",
                );
                return;
            };
            ui.monospace(timefmt::stamp(log.wall_clock(settings.time_axis), cursor));
            let b_at = self.b.as_ref().map(|b| cursor - b.offset);
            for r in &self.readout {
                ui.colored_label(r.color, "\u{25A0}");
                ui.label(readout_text(r, cursor, b_at));
            }
        });
    }
}

/// What the readout says for one series at `cursor`: `ATT.Roll (deg) =
/// 3.21`, and while B is open, read at `b_at` on B's clock, `B 2.87  Δ
/// +0.34`, each side noting how far off its sample lies.
fn readout_text(r: &Readout, cursor: f64, b_at: Option<f64>) -> String {
    let side = |sample: Option<(f64, f64)>, at: f64| match sample {
        Some((time, value)) => format!(
            "{}{}",
            egui_plot::format_number(value, 4),
            offset_note(time, at)
        ),
        None => DASH.to_string(),
    };
    let mut text = format!("{} = {}", r.title, side(r.a, cursor));
    if let Some(b_at) = b_at {
        let difference = r.difference.map_or_else(
            || DASH.to_string(),
            |d| {
                let sign = if d > 0.0 { "+" } else { "" };
                format!("{sign}{}", egui_plot::format_number(d, 4))
            },
        );
        let _ = write!(text, "  B {}  \u{394} {difference}", side(r.b, b_at));
    }
    text
}

/// What stands for a value the readout cannot read.
const DASH: &str = "\u{2013}";

/// How a frame draws each series' lines.
struct Lines<'a> {
    map: Option<AxisMap>,
    /// The time the lines are thinned over, on A's clock.
    range: &'a RangeInclusive<f64>,
    /// The plot's width in points.
    width: f32,
    /// B's offset onto A's clock, while B is open.
    b_offset: Option<f64>,
    /// Whether B's line keeps within `range`, as it does while the bounds
    /// follow the data.
    within_range: bool,
}

/// A series' lines thinned for a frame: B's, on A's clock, and A's;
/// None for a side the series lacks.
#[cfg(test)]
struct Thinned {
    b: Option<Vec<PlotPoint>>,
    a: Option<Vec<PlotPoint>>,
}

/// What a line hidden through the legend is drawn with: one point, so the
/// plot keeps the line, and its legend entry, while it hides it.
const HIDDEN_STAND_IN: &[PlotPoint] = &[PlotPoint { x: 0.0, y: 0.0 }];

/// A line's points as a frame last thinned them, kept on the series with
/// what they were thinned for, so the next frame asking for the same is
/// given them again instead of walking every sample in view.
#[derive(Debug)]
struct Thinning {
    key: ThinKey,
    points: Vec<PlotPoint>,
}

/// What a line's points were thinned for: anything else that changes them
/// (a new log, a new B, a reload, a swap) replaces the series' data and
/// clears the entry with it.
#[derive(Debug, Clone, Copy, PartialEq)]
struct ThinKey {
    /// The range's two ends, by their bits.
    range: (u64, u64),
    /// The plot's width, by its bits.
    width: u32,
    /// The axis map, for a series on the right axis, whose points it maps
    /// into the left axis; None for one on the left, which owes the map
    /// nothing, so a flip changes the key and a right-axis series coming
    /// or going leaves a left-axis line's alone.
    map: Option<AxisMap>,
    /// For B's line: the offset onto A's clock, by its bits, and whether
    /// the line is kept within the range.
    b: Option<(u64, bool)>,
}

impl Lines<'_> {
    /// What this frame thins a side of `s` for.
    fn key(&self, s: &Selected, b_side: bool) -> ThinKey {
        ThinKey {
            range: (self.range.start().to_bits(), self.range.end().to_bits()),
            width: self.width.to_bits(),
            map: self.map.filter(|_| s.axis == YAxis::Right),
            b: b_side.then(|| (self.b_offset.map_or(0, f64::to_bits), self.within_range)),
        }
    }

    /// Thin both sides of `s` afresh, as a frame that has nothing kept
    /// does; what the bench measures.
    #[cfg(test)]
    fn thin(&self, s: &Selected) -> Thinned {
        Thinned {
            b: self.thin_b(s),
            a: self.thin_a(s),
        }
    }

    /// A's line of `s` thinned, None for a series A lacks.
    fn thin_a(&self, s: &Selected) -> Option<Vec<PlotPoint>> {
        (!s.only_b()).then(|| {
            decimate_with(
                &s.data.xs,
                &s.data.ys,
                Some(&s.summary),
                self.range,
                self.width,
                |y| self.to_plot(s, y),
            )
        })
    }

    /// B's line of `s` thinned and moved onto A's clock, None while B is
    /// closed or lacks the field. Where the bounds follow the data, B is
    /// kept within A's range alone, its samples past the edges left out
    /// too, so no B far off, or with a gap in its samples, widens the view
    /// A fills.
    fn thin_b(&self, s: &Selected) -> Option<Vec<PlotPoint>> {
        let (data, offset) = s.data_b.as_ref().zip(self.b_offset)?;
        let to_plot = |y| self.to_plot(s, y);
        let points = decimate_moved(
            &data.xs,
            &data.ys,
            s.summary_b.as_ref(),
            self.range,
            self.width,
            to_plot,
            offset,
        );
        Some(if self.within_range {
            clip(&points, self.range)
        } else {
            points
        })
    }

    /// A value of `s` where the plot draws it: as it is, or mapped into
    /// the left axis for a series on the right one.
    fn to_plot(&self, s: &Selected, y: f64) -> f64 {
        match (s.axis, self.map) {
            (YAxis::Right, Some(m)) => m.to_left(y),
            _ => y,
        }
    }

    /// Thin the sides of `s` that no frame thinned for this range, width
    /// and map yet, and keep them on `s`; a line in `hidden` is left as
    /// it is, since nothing of it is drawn. The count of lines thinned.
    fn refresh(&self, s: &mut Selected, hidden: &[Id]) -> usize {
        let mut misses = 0;
        let key = self.key(s, false);
        if !hidden.contains(&line_id(&s.key))
            && s.thinned.as_ref().is_none_or(|t| t.key != key)
            && let Some(points) = self.thin_a(s)
        {
            s.thinned = Some(Thinning { key, points });
            misses += 1;
        }
        let key = self.key(s, true);
        if !hidden.contains(&line_id_b(&s.key))
            && s.thinned_b.as_ref().is_none_or(|t| t.key != key)
            && let Some(points) = self.thin_b(s)
        {
            s.thinned_b = Some(Thinning { key, points });
            misses += 1;
        }
        misses
    }

    /// Every series' lines: thinned where no frame thinned them for this
    /// view yet, then lent to the plot, so a frame whose view holds walks
    /// no samples and copies nothing. The count of lines thinned afresh.
    fn draw_all<'p>(
        &self,
        plot_ui: &mut egui_plot::PlotUi<'p>,
        selected: &'p mut [Selected],
        hidden: &[Id],
    ) -> usize {
        let mut misses = 0;
        for s in selected.iter_mut() {
            misses += self.refresh(s, hidden);
        }
        let selected: &'p [Selected] = selected;
        for s in selected {
            self.draw(plot_ui, s, hidden);
        }
        misses
    }

    /// B's line first, under A's, lighter, then A's, none for a series A
    /// lacks, so its legend has no entry that hides nothing. The points
    /// are those [`Self::refresh`] kept, lent to the plot. A line in
    /// `hidden` is given [`HIDDEN_STAND_IN`] instead: the plot drops it by
    /// its id before it sizes the view or draws, so its points never
    /// matter, but a line with none it drops at once (`PlotUi::line`),
    /// and with it the legend entry that would show the line again.
    fn draw<'p>(&self, plot_ui: &mut egui_plot::PlotUi<'p>, s: &'p Selected, hidden: &[Id]) {
        let points = |entry: &'p Option<Thinning>, id: Id| -> &'p [PlotPoint] {
            if hidden.contains(&id) {
                HIDDEN_STAND_IN
            } else {
                entry.as_ref().map_or(&[], |t| t.points.as_slice())
            }
        };
        if s.data_b.is_some() && self.b_offset.is_some() {
            let id = line_id_b(&s.key);
            plot_ui.line(
                Line::new(
                    format!("{} (B)", s.title),
                    PlotPoints::Borrowed(points(&s.thinned_b, id)),
                )
                .color(s.color.gamma_multiply(B_OPACITY))
                .id(id),
            );
        }
        if !s.only_b() {
            let id = line_id(&s.key);
            plot_ui.line(
                Line::new(
                    s.title.clone(),
                    PlotPoints::Borrowed(points(&s.thinned, id)),
                )
                .color(s.color)
                .id(id),
            );
        }
    }
}

/// The range the lines are thinned over, on A's clock, and whether B's
/// are kept within it. With the bounds following the data (`auto_x`),
/// every point counts, so a reset zooms back out to the whole flight:
/// A's, B's lines kept within it, or B's while no line of A's shows;
/// otherwise the view, `visible`.
fn lines_range(
    selected: &[Selected],
    hidden: &[Id],
    b_offset: Option<f64>,
    auto_x: bool,
    visible: &RangeInclusive<f64>,
) -> (RangeInclusive<f64>, bool) {
    let data_range = full_range(selected, hidden);
    let b_range = b_offset.and_then(|offset| full_range_b(selected, hidden, offset));
    match (auto_x, data_range, b_range) {
        (true, Some(range), _) => (range, true),
        (true, None, Some(range)) => (range, false),
        _ => (visible.clone(), false),
    }
}

/// Every shown series' sample nearest to board time `cursor`; the series
/// whose line ids are in `hidden` are left out. With B open, moved onto
/// A's clock by `offset`, a series shows while either side does, B read
/// at `cursor - offset`, and a side reads as a dash where its line is
/// hidden, it has no data, or the time falls outside its samples; the
/// difference too where A and B log the field in units that differ.
fn readout_at(
    selected: &[Selected],
    cursor: f64,
    hidden: &[Id],
    offset: Option<f64>,
) -> Vec<Readout> {
    selected
        .iter()
        .filter_map(|s| {
            let shown = !hidden.contains(&line_id(&s.key));
            let Some(offset) = offset else {
                let a = nearest(&s.data, cursor).filter(|_| shown)?;
                return Some(Readout {
                    title: s.title.clone(),
                    color: s.color,
                    a: Some(a),
                    b: None,
                    difference: None,
                });
            };
            let b_shown = s.data_b.is_some() && !hidden.contains(&line_id_b(&s.key));
            if !shown && !b_shown {
                return None;
            }
            let a = within(&s.data, cursor).filter(|_| shown);
            let b = s
                .data_b
                .as_ref()
                .and_then(|data| within(data, cursor - offset))
                .filter(|_| b_shown);
            let difference = a
                .zip(b)
                .filter(|_| !s.units_differ())
                .map(|((_, a), (_, b))| difference(a, b, s.is_angle()));
            Some(Readout {
                title: s.title.clone(),
                color: s.color,
                a,
                b,
                difference,
            })
        })
        .collect()
}

/// The sample of `series` nearest to `x`, when `x` lies within its
/// samples.
fn within(series: &Series, x: f64) -> Option<(f64, f64)> {
    nearest_within(&series.xs, x).map(|i| (series.xs[i], series.ys[i]))
}

/// `a` less `b`, the short way round the circle for an angle in degrees:
/// a yaw of 359 against 1 is -2, not 358.
fn difference(a: f64, b: f64, angle: bool) -> f64 {
    if !angle {
        return a - b;
    }
    let d = (a - b).rem_euclid(360.0);
    if d > 180.0 { d - 360.0 } else { d }
}

/// How far the sample shown lies from the cursor, when the readout's
/// millisecond resolution can tell: a sparse series answers with a sample
/// some way off, and the reader should know.
fn offset_note(sample_time: f64, cursor: f64) -> String {
    let offset = sample_time - cursor;
    if offset.abs() < 0.0005 {
        String::new()
    } else {
        format!(" ({offset:+.3} s)")
    }
}

/// `IMU[1].GyrX (rad/s)`
fn title(log: &LoadedLog, key: &SeriesKey) -> String {
    match unit(log, key) {
        Some(unit) => format!("{} ({unit})", key.label()),
        None => key.label(),
    }
}

/// The unit `log` gives the field `key` names, when it gives one.
fn unit(log: &LoadedLog, key: &SeriesKey) -> Option<String> {
    log.field_of(key)
        .and_then(|f| f.unit.clone())
        .filter(|u| !u.is_empty())
}

/// The earliest to the latest sample over the series shown, those whose
/// line ids are in `hidden` left out, as the plot leaves them out of the
/// bounds it fits.
fn full_range(selected: &[Selected], hidden: &[Id]) -> Option<RangeInclusive<f64>> {
    span(selected, hidden, 0.0, |s| (line_id(&s.key), Some(&s.data)))
}

/// The earliest to the latest sample of B's lines shown, moved onto A's
/// clock by `offset`.
fn full_range_b(selected: &[Selected], hidden: &[Id], offset: f64) -> Option<RangeInclusive<f64>> {
    span(selected, hidden, offset, |s| {
        (line_id_b(&s.key), s.data_b.as_ref())
    })
}

/// The earliest to the latest sample over one side of the series, the
/// line id and the samples `side` gives of each, those whose id is in
/// `hidden` or that have no samples left out, moved by `offset`.
fn span(
    selected: &[Selected],
    hidden: &[Id],
    offset: f64,
    side: impl Fn(&Selected) -> (Id, Option<&Series>),
) -> Option<RangeInclusive<f64>> {
    selected
        .iter()
        .map(side)
        .filter(|(id, _)| !hidden.contains(id))
        .filter_map(|(_, data)| {
            let xs = &data?.xs;
            Some((*xs.first()? + offset, *xs.last()? + offset))
        })
        .reduce(|(a, b), (c, d)| (a.min(c), b.max(d)))
        .map(|(a, b)| a..=b)
}

/// Draws the right-axis series into the left axis's range, and labels the
/// right axis back in its own values.
#[derive(Debug, Clone, Copy, PartialEq)]
struct AxisMap {
    left: (f64, f64),
    right: (f64, f64),
    /// Whether both axes carry series, so the right one is shown.
    two_sided: bool,
}

impl AxisMap {
    /// None when nothing is on the right axis, which then needs no mapping.
    /// B's values count as A's do, so each axis covers both; a line hidden
    /// through the legend counts too, on either side, so hiding one moves
    /// no other.
    fn new(selected: &[Selected]) -> Option<AxisMap> {
        let extent = |axis: YAxis| {
            let range = selected
                .iter()
                .filter(|s| s.axis == axis)
                .filter_map(Selected::extent_both)
                .reduce(|(a, b), (c, d)| (a.min(c), b.max(d)));
            range.map(|(a, b)| if a < b { (a, b) } else { (a - 1.0, b + 1.0) })
        };
        let right = extent(YAxis::Right)?;
        match extent(YAxis::Left) {
            Some(left) => Some(AxisMap {
                left,
                right,
                two_sided: true,
            }),
            None => Some(AxisMap {
                left: right,
                right,
                two_sided: false,
            }),
        }
    }

    fn to_left(self, y: f64) -> f64 {
        self.left.0
            + (y - self.right.0) * (self.left.1 - self.left.0) / (self.right.1 - self.right.0)
    }

    fn to_right(self, y: f64) -> f64 {
        self.right.0
            + (y - self.left.0) * (self.right.1 - self.right.0) / (self.left.1 - self.left.0)
    }
}

/// [`decimate_with`] over every sample, with no summary: the plain walk
/// the tests hold the blocks to.
#[cfg(test)]
pub fn decimate(
    xs: &[f64],
    ys: &[f64],
    range: &RangeInclusive<f64>,
    width: f32,
    map: impl Fn(f64) -> f64,
) -> Vec<PlotPoint> {
    decimate_with(xs, ys, None, range, width, map)
}

/// The points of a series within `range`, with one sample before and one
/// after it so the line runs off both edges, thinned to about two per pixel
/// of `width`: each bucket keeps its lowest and highest sample, in time
/// order, so no spike is lost. `xs` must be non-decreasing. With a
/// `summary`, each bucket is read as the blocks it holds whole and the
/// few samples around them (see [`extremes`]), the same points to the
/// bit, at a cost that follows the width rather than the samples in view.
pub fn decimate_with(
    xs: &[f64],
    ys: &[f64],
    summary: Option<&Summary>,
    range: &RangeInclusive<f64>,
    width: f32,
    map: impl Fn(f64) -> f64,
) -> Vec<PlotPoint> {
    let point = |i: usize| PlotPoint::new(xs[i], map(ys[i]));
    let len = xs.len().min(ys.len());
    let first_inside = xs[..len].partition_point(|x| x < range.start());
    let first_after = xs[..len].partition_point(|x| x <= range.end());
    // every sample on one side of the range: nothing crosses the view
    if first_inside == len || first_after == 0 {
        return Vec::new();
    }
    let lo = first_inside.saturating_sub(1);
    let hi = (first_after + 1).min(len);
    let buckets = (width.max(1.0) as usize).max(1);
    if hi - lo <= 2 * buckets {
        return (lo..hi).map(point).collect();
    }
    let x0 = xs[lo];
    let span = xs[hi - 1] - x0;
    if span.is_nan() || span <= 0.0 {
        return vec![point(lo), point(hi - 1)];
    }

    let mut ends = bucket_ends(&xs[lo..hi], buckets);
    for end in &mut ends {
        *end += lo;
    }
    bucket_points(xs, ys, lo, &ends, summary, map)
}

/// Where each of `buckets` equal spans of time over `xs` ends, as an index
/// one past its last sample; the last ends at `xs.len()`, taking the rest.
/// A sample on an edge belongs to the bucket before it.
fn bucket_ends(xs: &[f64], buckets: usize) -> Vec<usize> {
    let x0 = xs[0];
    let span = xs[xs.len() - 1] - x0;
    let mut ends: Vec<usize> = (1..buckets)
        .map(|bucket| {
            let edge = x0 + span * bucket as f64 / buckets as f64;
            xs.partition_point(|x| *x <= edge)
        })
        .collect();
    ends.push(xs.len());
    ends
}

/// The points of the buckets `lo..ends[0]`, `ends[0]..ends[1]` and so on:
/// each bucket's lowest and highest sample, in time order, an empty bucket
/// giving none; the extremes read through `summary` where there is one.
fn bucket_points(
    xs: &[f64],
    ys: &[f64],
    lo: usize,
    ends: &[usize],
    summary: Option<&Summary>,
    map: impl Fn(f64) -> f64,
) -> Vec<PlotPoint> {
    let point = |i: usize| PlotPoint::new(xs[i], map(ys[i]));
    let mut out = Vec::with_capacity(2 * ends.len());
    let mut start = lo;
    for &end in ends {
        let found = extremes(ys, start, end, summary);
        start = end;
        let Some((min_i, max_i)) = found else {
            continue;
        };
        let (first, second) = if min_i <= max_i {
            (min_i, max_i)
        } else {
            (max_i, min_i)
        };
        out.push(point(first));
        if second != first {
            out.push(point(second));
        }
    }
    out
}

/// The index of the lowest and of the highest of `ys[start..end]`, the
/// first of each where values tie; None for an empty span. With a
/// `summary`, the span is read as the largest blocks that fit it, aligned
/// to their size, from the smallest up and down again as a segment tree
/// is read, and sample by sample only before the first block and after
/// the last, so a span of thousands of samples costs a few dozen reads.
/// A tie still goes to the first sample: the blocks are met in order,
/// each holds the first of its own ties, and the comparisons are strict.
fn extremes(
    ys: &[f64],
    start: usize,
    end: usize,
    summary: Option<&Summary>,
) -> Option<(usize, usize)> {
    let mut best: Option<(usize, usize)> = None;
    let mut consider = |i: usize| {
        if let Some((min_i, max_i)) = &mut best {
            if ys[i] < ys[*min_i] {
                *min_i = i;
            }
            if ys[i] > ys[*max_i] {
                *max_i = i;
            }
        } else {
            best = Some((i, i));
        }
    };
    let Some(summary) = summary.filter(|s| !s.levels.is_empty()) else {
        (start..end).for_each(consider);
        return best;
    };
    let mut i = start;
    while i < end && !i.is_multiple_of(Summary::BASE) {
        consider(i);
        i += 1;
    }
    while i + Summary::BASE <= end {
        let (level, block) = summary.largest_block_at(i, end);
        let [min_i, max_i] = summary.levels[level][i / block];
        consider(min_i as usize);
        consider(max_i as usize);
        i += block;
    }
    (i..end).for_each(consider);
    best
}

/// The lowest and highest sample of each block of a series, by index, in
/// levels of [`Summary::BASE`], twice that, and so on up: a thinning
/// whose buckets span many samples reads the blocks a bucket holds whole
/// instead of every sample, so its cost follows the plot's width. About
/// a quarter of the samples in indices.
#[derive(Debug, Default)]
pub struct Summary {
    /// Level `i` has blocks of `BASE << i` samples, the last one short;
    /// each block's lowest sample's index and highest sample's index,
    /// the first of each where values tie, in block order.
    levels: Vec<Vec<[u32; 2]>>,
}

impl Summary {
    /// Samples a block on the first level.
    const BASE: usize = 16;

    /// The blocks of `ys`, a linear pass.
    #[must_use]
    pub fn new(ys: &[f64]) -> Summary {
        let lower = |a: u32, b: u32| {
            if ys[b as usize] < ys[a as usize] {
                b
            } else {
                a
            }
        };
        let higher = |a: u32, b: u32| {
            if ys[b as usize] > ys[a as usize] {
                b
            } else {
                a
            }
        };
        let mut levels = Vec::new();
        let mut level: Vec<[u32; 2]> = ys
            .chunks(Self::BASE)
            .enumerate()
            .map(|(block, chunk)| {
                let first = (block * Self::BASE) as u32;
                (first + 1..first + chunk.len() as u32).fold([first, first], |[min_i, max_i], i| {
                    [lower(min_i, i), higher(max_i, i)]
                })
            })
            .collect();
        while level.len() > 1 {
            let next = level
                .chunks(2)
                .map(|pair| match pair {
                    [a, b] => [lower(a[0], b[0]), higher(a[1], b[1])],
                    _ => pair[0],
                })
                .collect();
            levels.push(level);
            level = next;
        }
        if !level.is_empty() {
            levels.push(level);
        }
        Summary { levels }
    }

    /// The level with the largest block that starts at sample `i`, a
    /// multiple of [`Self::BASE`], and ends by `end`, and that block's
    /// size. A block of `BASE << level` starts at `i` while `level` is at
    /// most the trailing zeros of `i / BASE`, and ends by `end` while
    /// `1 << level` is at most `(end - i) / BASE`, so the level is the
    /// smaller of the two bounds, and of the top one. The first level's
    /// where `end` is under a block past `i`, or the summary has no
    /// level, which [`extremes`] rules out before it asks.
    fn largest_block_at(&self, i: usize, end: usize) -> (usize, usize) {
        let aligned = (i / Self::BASE).trailing_zeros() as usize;
        let room = end
            .checked_sub(i)
            .and_then(|room| (room / Self::BASE).checked_ilog2())
            .map_or(0, |room| room as usize);
        let top = self.levels.len().saturating_sub(1);
        let level = aligned.min(room).min(top);
        (level, Self::BASE << level)
    }
}

/// B's points for `range` on A's clock: B's samples thinned over the
/// range moved back by `offset`, onto B's clock, and the points moved
/// forward by it, so B is thinned where it shows and drawn on A's clock.
pub fn decimate_moved(
    xs: &[f64],
    ys: &[f64],
    summary: Option<&Summary>,
    range: &RangeInclusive<f64>,
    width: f32,
    map: impl Fn(f64) -> f64,
    offset: f64,
) -> Vec<PlotPoint> {
    let back = range.start() - offset..=range.end() - offset;
    let mut points = decimate_with(xs, ys, summary, &back, width, map);
    for p in &mut points {
        p.x += offset;
    }
    points
}

/// `points`, in time order, cut to `range`: those within it, and where
/// the line crosses an edge, a point on the edge, so a line runs to the
/// edges, even one with no point inside, without a point beyond them.
fn clip(points: &[PlotPoint], range: &RangeInclusive<f64>) -> Vec<PlotPoint> {
    let (start, end) = (*range.start(), *range.end());
    // the line from p to q where it reaches x, which lies between them
    let at = |p: &PlotPoint, q: &PlotPoint, x: f64| {
        PlotPoint::new(x, p.y + (q.y - p.y) * (x - p.x) / (q.x - p.x))
    };
    let mut out = Vec::with_capacity(points.len());
    for (i, q) in points.iter().enumerate() {
        if let Some(p) = i.checked_sub(1).map(|before| &points[before]) {
            if p.x < start && q.x > start {
                out.push(at(p, q, start));
            }
            if p.x < end && q.x > end {
                out.push(at(p, q, end));
            }
        }
        if range.contains(&q.x) {
            out.push(*q);
        }
    }
    out
}

/// The sample of `series` nearest in time to `x`.
fn nearest(series: &Series, x: f64) -> Option<(f64, f64)> {
    nearest_index(&series.xs, x).map(|i| (series.xs[i], series.ys[i]))
}

/// A hue per mode number, spread so neighboring numbers differ.
fn mode_color(number: u8) -> Color32 {
    let hue = (f32::from(number) * 0.618_034).fract();
    egui::ecolor::Hsva::new(hue, 0.55, 0.9, 1.0).into()
}

/// The mode bands that reach into `visible`: each mode from its change to
/// the next, the last one to the end of the log.
fn visible_bands<'a>(
    log: &'a LoadedLog,
    visible: &RangeInclusive<f64>,
) -> Vec<(&'a ModeChange, f64)> {
    let end_of_log = log.timeline.last().copied().unwrap_or(f64::NAN);
    log.modes
        .iter()
        .enumerate()
        .map(|(index, mode)| {
            let end = log
                .modes
                .get(index + 1)
                .map_or(end_of_log.max(mode.time), |next| next.time);
            (mode, end)
        })
        .filter(|(mode, end)| {
            mode.time.is_finite()
                && end.is_finite()
                && *end >= *visible.start()
                && mode.time <= *visible.end()
        })
        .collect()
}

fn mode_bands(plot_ui: &mut egui_plot::PlotUi<'_>, log: &LoadedLog, visible: &RangeInclusive<f64>) {
    for (mode, end) in visible_bands(log, visible) {
        plot_ui.span(
            Span::new("", mode.time..=end).fill(mode_color(mode.number).gamma_multiply(0.14)),
        );
    }
}

/// The plot before anything is drawn in it: its legend, its time axis
/// read through the clock `settings` choose, the right axis when `map`
/// puts series on both. No label formatter: the readout under the plot
/// stands in for the hover tooltip, which `egui_plot` only shows when
/// one is set. While `playing`, `egui_plot`'s rulers and its dot on the
/// sample nearest the pointer are off, so the one line that moves is the
/// playhead.
fn new_plot(
    log: &LoadedLog,
    settings: &Settings,
    map: Option<AxisMap>,
    height: f32,
    playing: bool,
    reset: bool,
) -> Plot<'static> {
    let time_base = log.wall_clock(settings.time_axis);
    let x_formatter = move |mark: GridMark, _: &RangeInclusive<f64>| {
        timefmt::clock(time_base, mark.value, mark.step_size)
    };
    let mut plot = Plot::new(PLOT_ID)
        .id(Id::new(PLOT_ID))
        .height(height)
        .legend(Legend::default().position(Corner::LeftTop))
        .x_axis_formatter(x_formatter)
        .allow_boxed_zoom(true)
        .show_x(!playing)
        .show_y(!playing);
    if reset {
        plot = plot.reset();
    }
    if let Some(map) = map.filter(|m| m.two_sided) {
        plot = plot.custom_y_axes(vec![
            AxisHints::new_y(),
            AxisHints::new_y().placement(HPlacement::Right).formatter(
                move |mark: GridMark, _: &RangeInclusive<f64>| {
                    egui_plot::format_number(map.to_right(mark.value), 3)
                },
            ),
        ]);
    }
    plot
}

/// What the plot's closure found in a frame.
struct Found {
    /// Where the pointer is on screen, while it is over the plot.
    at: Option<Pos2>,
    /// The time of a click on the plot, to seek to.
    click: Option<f64>,
    cursor: Option<f64>,
    /// The view paging last set, this frame's when it paged.
    paged: Option<RangeInclusive<f64>>,
    /// How many lines the frame thinned afresh.
    misses: usize,
}

/// Where paging moves a zoomed view of `view` for the playhead at `time`,
/// if it moves it: a view paging set, `paged`, turns when the playhead
/// passes 95% of its width or falls before it; a view the user has set
/// since is left alone until the playhead leaves it, in it at
/// `last_played` and out of it now, both against this view, so a view
/// moved off the playhead stays where the hand put it. Play starts with
/// no `last_played`, as if the playhead had been in view, so a view it
/// is outside of pages to it at once. The new view keeps the width and
/// starts 5% of it before the playhead, however far a frame moved it.
fn page(
    view: &RangeInclusive<f64>,
    paged: Option<&RangeInclusive<f64>>,
    last_played: Option<f64>,
    time: f64,
) -> Option<RangeInclusive<f64>> {
    let (start, end) = (*view.start(), *view.end());
    let width = end - start;
    if width.is_nan() || width <= 0.0 {
        return None;
    }
    // the bounds come back as they were set; a hand's pan or zoom moves
    // them by a pixel's worth at least
    let same = |p: &RangeInclusive<f64>| {
        let tolerance = width * 1e-6;
        (p.start() - start).abs() <= tolerance && (p.end() - end).abs() <= tolerance
    };
    let turn = if paged.is_some_and(same) {
        time < start || time > start + 0.95 * width
    } else {
        last_played.is_none_or(|t| view.contains(&t)) && !view.contains(&time)
    };
    turn.then(|| {
        let start = time - 0.05 * width;
        start..=start + width
    })
}

/// The cursor where the pointer has moved it apart from the playhead,
/// and the playhead over it.
fn time_marks(
    plot_ui: &mut egui_plot::PlotUi<'_>,
    cursor: Option<f64>,
    playhead: Option<f64>,
    playhead_stroke: Stroke,
) {
    if let Some(x) = cursor.filter(|&x| Some(x) != playhead) {
        let stroke = Stroke::new(1.0, Color32::from_gray(160));
        plot_ui.add(TimeMark::new(x, stroke, false));
    }
    if let Some(x) = playhead {
        plot_ui.add(TimeMark::new(x, playhead_stroke, true));
    }
}

/// A line across the plot at a time: the playhead, with a mark at its
/// top, or the cursor. Its bounds are empty, so a view that follows the
/// data is never widened to take it in, and it shows wherever the view
/// takes in its time, whether or not a series is plotted there; among
/// the plot's items, it lies under the legend.
#[derive(Debug)]
struct TimeMark {
    base: PlotItemBase,
    time: f64,
    stroke: Stroke,
    head: bool,
}

impl TimeMark {
    fn new(time: f64, stroke: Stroke, head: bool) -> Self {
        Self {
            // unnamed, so the legend leaves it out
            base: PlotItemBase::new(String::new()),
            time,
            stroke,
            head,
        }
    }
}

impl PlotItem for TimeMark {
    fn shapes(&self, _ui: &egui::Ui, transform: &PlotTransform, shapes: &mut Vec<Shape>) {
        let frame = transform.frame();
        let x = transform.position_from_point_x(self.time);
        shapes.push(Shape::line_segment(
            [pos2(x, frame.top()), pos2(x, frame.bottom())],
            self.stroke,
        ));
        if self.head {
            let top = frame.top();
            shapes.push(Shape::convex_polygon(
                vec![pos2(x - 5.0, top), pos2(x + 5.0, top), pos2(x, top + 7.0)],
                self.stroke.color,
                Stroke::NONE,
            ));
        }
    }

    fn initialize(&mut self, _x_range: RangeInclusive<f64>) {}

    fn color(&self) -> Color32 {
        self.stroke.color
    }

    fn allow_hover(&self) -> bool {
        false
    }

    fn geometry(&self) -> PlotGeometry<'_> {
        PlotGeometry::None
    }

    fn bounds(&self) -> PlotBounds {
        PlotBounds::NOTHING
    }

    fn base(&self) -> &PlotItemBase {
        &self.base
    }

    fn base_mut(&mut self) -> &mut PlotItemBase {
        &mut self.base
    }
}

/// The lanes above the plot that the flight modes' names go in, `lanes`
/// lines of small text high, taken from the top of `ui`'s room.
fn mode_strip(ui: &mut egui::Ui, lanes: u8) -> Rect {
    let height = egui::TextStyle::Small.resolve(ui.style()).size + 4.0;
    ui.allocate_space(egui::vec2(ui.available_width(), height * f32::from(lanes)))
        .1
}

/// Where each visible band's mode name goes in `strip`, a lane above the
/// plot's frame: over the band's start, or at the frame's left edge for a
/// band that starts off it. The log's times are moved onto the plot's
/// clock by `offset`, B's by its own, A's by none.
fn label_spots<'a>(
    log: &'a LoadedLog,
    offset: f64,
    transform: &PlotTransform,
    strip: Rect,
) -> Vec<(Pos2, &'a ModeChange)> {
    let frame = transform.frame();
    let view = transform.bounds().range_x();
    let back = view.start() - offset..=view.end() - offset;
    visible_bands(log, &back)
        .into_iter()
        .map(|(mode, _)| {
            let x = transform
                .position_from_point_x(mode.time + offset)
                .max(frame.left() + 2.0);
            (pos2(x + 2.0, strip.top() + 2.0), mode)
        })
        .collect()
}

/// Each visible band's mode name in `strip`, clipped to the width of the
/// plot's frame, the log's times moved by `offset`; with `ticks`, a short
/// line at each change in view, as B's lane has in place of bands.
fn mode_labels(
    ui: &egui::Ui,
    log: &LoadedLog,
    offset: f64,
    transform: &PlotTransform,
    strip: Rect,
    ticks: bool,
) {
    let lane = Rect::from_x_y_ranges(transform.frame().x_range(), strip.y_range());
    let painter = ui.painter().with_clip_rect(lane);
    let font = egui::TextStyle::Small.resolve(ui.style());
    for (at, mode) in label_spots(log, offset, transform, strip) {
        let color = mode_color(mode.number);
        if ticks {
            let x = transform.position_from_point_x(mode.time + offset);
            painter.line_segment(
                [pos2(x, strip.top() + 2.0), pos2(x, strip.bottom())],
                Stroke::new(1.5, color),
            );
        }
        painter.text(at, Align2::LEFT_TOP, &mode.name, font.clone(), color);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::kittest::Queryable as _;

    fn series(n: usize) -> Series {
        let xs: Vec<f64> = (0..n).map(|i| i as f64 * 0.01).collect();
        // a slow wave with one spike, to see the spike survive thinning
        let ys: Vec<f64> = (0..n)
            .map(|i| {
                if i == 5000 {
                    50.0
                } else {
                    (i as f64 * 0.001).sin()
                }
            })
            .collect();
        Series { xs, ys }
    }

    #[test]
    fn decimation_keeps_extremes_within_a_pixel_budget() {
        let s = series(20_000);
        let all = decimate(&s.xs, &s.ys, &(0.0..=1000.0), 800.0, |y| y);
        assert!(all.len() <= 1600, "{}", all.len());
        assert!(all.len() > 800);
        assert!(all.iter().any(|p| p.y > 49.0), "the spike is kept");
        assert!(all.windows(2).all(|w| w[0].x <= w[1].x), "in time order");
        assert_eq!(all.first().map(|p| p.x), Some(0.0));

        // a narrow window is not thinned at all, and carries one sample past each edge
        let window = decimate(&s.xs, &s.ys, &(10.0..=11.0), 800.0, |y| y);
        assert_eq!(window.len(), 103);
        assert!(window[0].x < 10.0 && window[102].x > 11.0);

        // the mapping applies to every value
        let mapped = decimate(&s.xs, &s.ys, &(0.0..=0.05), 800.0, |y| y * 2.0 + 1.0);
        assert!((mapped[0].y - 1.0).abs() < 1e-12);

        // a gap between two samples still draws the segment across it
        let gap = decimate(&[0.0, 10.0], &[1.0, 2.0], &(3.0..=4.0), 800.0, |y| y);
        assert_eq!(gap.len(), 2);

        // nothing in range on either side, and degenerate inputs
        assert!(decimate(&s.xs, &s.ys, &(500.0..=600.0), 800.0, |y| y).is_empty());
        assert!(decimate(&s.xs, &s.ys, &(-5.0..=-1.0), 800.0, |y| y).is_empty());
        assert!(decimate(&[], &[], &(0.0..=1.0), 800.0, |y| y).is_empty());
        let flat = decimate(
            &vec![1.0; 5000],
            &vec![0.0; 5000],
            &(0.0..=2.0),
            10.0,
            |y| y,
        );
        assert_eq!(flat.len(), 2);
    }

    #[test]
    fn the_nearest_sample_is_found_on_either_side() {
        let s = Series {
            xs: vec![0.0, 1.0, 2.0],
            ys: vec![10.0, 11.0, 12.0],
        };
        assert_eq!(nearest(&s, -5.0), Some((0.0, 10.0)));
        assert_eq!(nearest(&s, 0.4), Some((0.0, 10.0)));
        assert_eq!(nearest(&s, 0.6), Some((1.0, 11.0)));
        assert_eq!(nearest(&s, 1.0), Some((1.0, 11.0)));
        assert_eq!(nearest(&s, 9.0), Some((2.0, 12.0)));
        assert_eq!(nearest(&Series::default(), 1.0), None);
    }

    #[test]
    fn the_right_axis_maps_into_the_left_range_and_back() {
        let key = |field: &str| SeriesKey {
            type_name: "T".into(),
            field: field.into(),
            instance: None,
        };
        let pick = |field: &str, axis, ys: Vec<f64>| {
            let data = Series {
                xs: (0..ys.len()).map(|i| i as f64).collect(),
                ys,
            };
            Selected::new(key(field), field.into(), axis, Color32::WHITE, data)
        };
        assert_eq!(extent([]), None);
        assert_eq!(extent([3.0, -1.0, 2.0]), Some((-1.0, 3.0)));
        assert!(AxisMap::new(&[pick("a", YAxis::Left, vec![0.0, 10.0])]).is_none());

        let both = [
            pick("a", YAxis::Left, vec![0.0, 10.0]),
            pick("b", YAxis::Right, vec![100.0, 300.0]),
        ];
        let map = AxisMap::new(&both).unwrap();
        assert!(map.two_sided);
        assert!((map.to_left(100.0)).abs() < 1e-12);
        assert!((map.to_left(300.0) - 10.0).abs() < 1e-12);
        assert!((map.to_left(200.0) - 5.0).abs() < 1e-12);
        assert!((map.to_right(5.0) - 200.0).abs() < 1e-12);

        // right alone draws in its own values; a flat series gets a range
        let alone = AxisMap::new(&[pick("b", YAxis::Right, vec![7.0, 7.0])]).unwrap();
        assert!(!alone.two_sided);
        assert!((alone.to_left(7.0) - 7.0).abs() < 1e-12);
        assert_eq!(alone.right, (6.0, 8.0));
    }

    #[test]
    fn selection_toggles_reloads_and_reports_errors() {
        let log = LoadedLog::build(
            dflog::Log::from_bytes(&crate::model::testlog::bytes()),
            "test.bin".into(),
        );
        let mut plot = PlotPanel::default();
        let roll = SeriesKey {
            type_name: "ATT".into(),
            field: "Roll".into(),
            instance: None,
        };
        plot.toggle(roll.clone(), &log);
        assert!(plot.is_selected(&roll));
        assert_eq!(plot.selected[0].title, "ATT.Roll (deg)");
        assert_eq!(plot.selected[0].data.ys.len(), 4);

        let gyr = SeriesKey {
            type_name: "IMU".into(),
            field: "GyrX".into(),
            instance: Some(1),
        };
        plot.toggle(gyr.clone(), &log);
        assert_ne!(plot.selected[0].color, plot.selected[1].color);
        assert_eq!(plot.selected[1].title, "IMU[1].GyrX (rad/s)");

        let text = SeriesKey {
            type_name: "MSG".into(),
            field: "Message".into(),
            instance: None,
        };
        plot.toggle(text.clone(), &log);
        assert!(!plot.is_selected(&text));
        assert!(plot.error.as_deref().unwrap().contains("text"));

        // a log without IMU keeps Roll and drops the gyro, and the view resets
        assert!(!plot.resets_view());
        let other = LoadedLog::build(dflog::Log::from_bytes(&other_log()), "other.bin".into());
        plot.reload(&other);
        assert!(plot.is_selected(&roll));
        assert!(!plot.is_selected(&gyr));
        assert_eq!(plot.selected[0].data.ys, [1.0]);
        assert_eq!(
            plot.selected[0].extent,
            Some((1.0, 1.0)),
            "extent follows the data"
        );
        assert_eq!(plot.selected[0].title, "ATT.Roll", "no units in this log");
        assert!(plot.resets_view());

        plot.toggle(roll.clone(), &log);
        assert!(plot.selected.is_empty());
        plot.remove(3);

        // a cleared plot starts the palette again
        plot.toggle(gyr.clone(), &log);
        assert_ne!(plot.selected[0].color, PALETTE[0]);
        plot.clear();
        plot.toggle(gyr, &log);
        assert_eq!(plot.selected[0].color, PALETTE[0]);
    }

    /// A log with one ATT record and no units metadata.
    fn other_log() -> Vec<u8> {
        use dflog::access::Value;
        let mut w = dflog::write::LogWriter::new();
        w.define(6, "ATT", "Qc", &["TimeUS", "Roll"]).unwrap();
        w.record("ATT", &[Value::U64(1_000_000), Value::F64(1.0)])
            .unwrap();
        w.into_bytes()
    }

    /// Only the bands that reach into the view are drawn and labeled, so a
    /// mode that ended before the view does not pin its name to the edge.
    #[test]
    fn only_bands_in_view_are_drawn() {
        let log = LoadedLog::build(
            dflog::Log::from_bytes(&crate::model::testlog::bytes()),
            "test.bin".into(),
        );
        let names = |range: RangeInclusive<f64>| -> Vec<(&str, f64)> {
            visible_bands(&log, &range)
                .into_iter()
                .map(|(m, end)| (m.name.as_str(), end))
                .collect()
        };
        // Stabilize from 1.5 s to the Loiter change at 2.25 s, which runs to
        // the end of the log, also at 2.25 s
        assert_eq!(names(0.0..=10.0), [("Stabilize", 2.25), ("Loiter", 2.25)]);
        assert_eq!(names(1.0..=2.0), [("Stabilize", 2.25)]);
        assert_eq!(names(2.3..=3.0), []);
        assert_eq!(names(2.2..=3.0), [("Stabilize", 2.25), ("Loiter", 2.25)]);
        assert_eq!(names(0.0..=1.4), []);
    }

    #[test]
    fn the_readout_keeps_its_last_cursor_and_notes_sparse_samples() {
        let key = |field: &str| SeriesKey {
            type_name: "T".into(),
            field: field.into(),
            instance: None,
        };
        let dense = Selected::new(
            key("a"),
            "a".into(),
            YAxis::Left,
            Color32::WHITE,
            Series {
                xs: vec![0.0, 0.1, 0.2, 0.3],
                ys: vec![1.0, 2.0, 3.0, 4.0],
            },
        );
        let sparse = Selected::new(
            key("b"),
            "b".into(),
            YAxis::Left,
            Color32::WHITE,
            Series {
                xs: vec![0.0, 1.0],
                ys: vec![10.0, 20.0],
            },
        );
        let both = [dense, sparse];
        let readout = readout_at(&both, 0.21, &[], None);
        assert_eq!(readout.len(), 2);
        assert_eq!(readout[0].a, Some((0.2, 3.0)));
        assert_eq!(readout[1].a, Some((0.0, 10.0)));
        assert_eq!(readout[0].b, None);
        assert_eq!(readout_text(&readout[0], 0.21, None), "a = 3 (-0.010 s)");
        assert_eq!(offset_note(0.2, 0.21), " (-0.010 s)");
        assert_eq!(offset_note(0.0, 0.21), " (-0.210 s)");
        assert_eq!(offset_note(0.2, 0.2002), "");
        assert_eq!(offset_note(1.5, 1.0), " (+0.500 s)");

        // a series hidden through the legend leaves the readout
        let shown = readout_at(&both, 0.21, &[line_id(&key("b"))], None);
        assert_eq!(shown.len(), 1);
        assert_eq!(shown[0].title, "a");
        // and from the range the cursor line is kept within
        assert_eq!(full_range(&both, &[]), Some(0.0..=1.0));
        assert_eq!(full_range(&both, &[line_id(&key("b"))]), Some(0.0..=0.3));
        assert_eq!(
            full_range(&both, &[line_id(&key("a")), line_id(&key("b"))]),
            None
        );

        // removing a series refreshes the readout at the same cursor
        let mut plot = PlotPanel {
            cursor: Some(0.21),
            ..PlotPanel::default()
        };
        plot.refresh_readout();
        assert!(plot.readout.is_empty());
        plot.selected.push(Selected::new(
            key("c"),
            "c".into(),
            YAxis::Left,
            Color32::WHITE,
            Series {
                xs: vec![0.0],
                ys: vec![7.0],
            },
        ));
        plot.refresh_readout();
        assert_eq!(plot.readout.len(), 1);
        plot.remove(0);
        assert!(plot.readout.is_empty());
        assert_eq!(plot.cursor, Some(0.21), "the cursor stays");

        // a seek moves the playhead and the cursor, and reads out there
        plot.selected.extend(both);
        plot.seek(1.0);
        assert_eq!((plot.playhead, plot.cursor), (Some(1.0), Some(1.0)));
        assert_eq!(plot.pending_seek, Some(1.0));
        assert!(plot.follow);
        assert_eq!(plot.readout.len(), 2);
        assert_eq!(plot.readout[1].a, Some((1.0, 20.0)));

        // playing moves them too, and leaves the view and the events list
        // to paging and following
        plot.pending_seek = None;
        plot.follow = false;
        plot.play_to(0.21);
        assert_eq!((plot.playhead, plot.cursor), (Some(0.21), Some(0.21)));
        assert_eq!(plot.pending_seek, None);
        assert!(!plot.follow);
        assert_eq!(plot.readout[0].a, Some((0.2, 3.0)));

        // cleared, the plot keeps the playhead and puts the cursor on it
        plot.cursor = Some(0.5);
        plot.clear();
        assert_eq!((plot.playhead, plot.cursor), (Some(0.21), Some(0.21)));

        // a seek the events list has yet to follow still waits for it
        plot.seek(0.3);
        plot.clear();
        assert!(plot.follow);

        // a time that is not finite, as a record before the first timed
        // one has, moves nothing
        plot.seek(f64::NAN);
        assert_eq!((plot.playhead, plot.cursor), (Some(0.3), Some(0.3)));
    }

    #[test]
    fn paging_turns_a_view_it_set_at_95_percent_and_waits_for_the_users() {
        let view = 10.0..=20.0;
        let ours = Some(&view);
        // in the page, short of 95%: nothing
        assert_eq!(page(&view, ours, Some(19.3), 19.4), None);
        // past it: the next page starts 5% before the playhead
        assert_eq!(page(&view, ours, Some(19.4), 19.6), Some(19.1..=29.1));
        // a 60x step far past the end lands at 5% all the same
        assert_eq!(page(&view, ours, Some(19.4), 40.0), Some(39.5..=49.5));
        // and a seek back before the page
        assert_eq!(page(&view, ours, Some(19.4), 3.0), Some(2.5..=12.5));

        // a view panned by hand since is left alone while the playhead is
        // in it, past 95% included
        let panned = 12.0..=22.0;
        assert_eq!(page(&panned, ours, Some(21.7), 21.8), None);
        // until it leaves, at either side
        assert_eq!(page(&panned, ours, Some(21.8), 22.5), Some(22.0..=32.0));
        assert_eq!(page(&panned, ours, Some(12.5), 11.0), Some(10.5..=20.5));
        // a view moved off the playhead stays where the hand put it: the
        // playhead was not in it last frame either
        assert_eq!(page(&panned, ours, Some(29.9), 30.0), None);
        assert_eq!(page(&panned, None, Some(29.9), 30.0), None);
        // Play starting outside the view pages to the playhead at once
        assert_eq!(page(&panned, None, None, 30.0), Some(29.5..=39.5));
        assert_eq!(page(&panned, None, None, 15.0), None);

        // no width, no paging
        assert_eq!(page(&(5.0..=5.0), None, None, 9.0), None);
    }

    /// The plot of the test log's `ATT.Roll`, 2.0 s to 2.3 s, in a harness
    /// whose state says whether playback runs.
    fn roll_plot<'a>(
        log: &'a LoadedLog,
        settings: &'a Settings,
    ) -> egui_kittest::Harness<'a, (PlotPanel, bool)> {
        let mut plot = PlotPanel::default();
        plot.toggle(
            SeriesKey {
                type_name: "ATT".into(),
                field: "Roll".into(),
                instance: None,
            },
            log,
        );
        // a twentieth of a second a frame, so two clicks a frame or two
        // apart make a double-click
        let mut harness = egui_kittest::Harness::builder()
            .with_size(egui::vec2(800.0, 500.0))
            .with_step_dt(0.05)
            .build_ui_state(
                |ui, (plot, playing): &mut (PlotPanel, bool)| {
                    plot.show(ui, log, settings, *playing);
                },
                (plot, false),
            );
        harness.run();
        harness
    }

    /// The plot's frame on screen and its transform, as last drawn.
    fn transform(harness: &egui_kittest::Harness<'_, (PlotPanel, bool)>) -> PlotTransform {
        PlotMemory::load(&harness.ctx, Id::new(PLOT_ID))
            .expect("the plot has drawn")
            .transform()
    }

    /// The board time at `fraction` of the plot's width, and where that is
    /// on screen, halfway down.
    fn at(
        harness: &egui_kittest::Harness<'_, (PlotPanel, bool)>,
        fraction: f32,
    ) -> (f64, egui::Pos2) {
        let transform = transform(harness);
        let frame = transform.frame();
        let pos = pos2(frame.left() + frame.width() * fraction, frame.center().y);
        (transform.value_from_position(pos).x, pos)
    }

    /// Press and let go at `pos`, with no move between.
    fn click_at(harness: &mut egui_kittest::Harness<'_, (PlotPanel, bool)>, pos: egui::Pos2) {
        harness.hover_at(pos);
        for pressed in [true, false] {
            harness.event(egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            });
        }
        harness.run();
    }

    #[test]
    fn the_pointer_moves_the_cursor_while_paused_and_a_click_seeks() {
        let log = LoadedLog::build(
            dflog::Log::from_bytes(&crate::model::testlog::bytes()),
            "test.bin".into(),
        );
        let settings = Settings::default();
        let mut harness = roll_plot(&log, &settings);
        let state = |h: &egui_kittest::Harness<'_, (PlotPanel, bool)>| {
            (h.state().0.playhead, h.state().0.cursor)
        };

        // paused, the pointer moves the cursor and not the playhead; with
        // no playhead yet, the cursor stays when the pointer leaves
        let (t30, p30) = at(&harness, 0.3);
        harness.hover_at(p30);
        harness.run();
        assert_eq!(state(&harness), (None, Some(t30)));
        assert_eq!(harness.state().0.readout.len(), 1);
        harness.event(egui::Event::PointerGone);
        harness.run();
        assert_eq!(state(&harness), (None, Some(t30)));

        // a click seeks
        let (t60, p60) = at(&harness, 0.6);
        click_at(&mut harness, p60);
        assert_eq!(state(&harness), (Some(t60), Some(t60)));
        assert!(harness.state().0.follow, "the events list follows");

        // the pointer moves the cursor apart, and leaving brings it back
        let (t40, p40) = at(&harness, 0.4);
        harness.hover_at(p40);
        harness.run();
        assert_eq!(state(&harness), (Some(t60), Some(t40)));
        harness.event(egui::Event::PointerGone);
        harness.run();
        assert_eq!(state(&harness), (Some(t60), Some(t60)));

        // a pointer resting on the plot leaves a seek where it put the
        // cursor, as a step by a key does
        harness.hover_at(p40);
        harness.run();
        harness.state_mut().0.seek(t30);
        harness.run();
        assert_eq!(state(&harness), (Some(t30), Some(t30)));
        // and so does a seek that pans a zoomed view under it, which puts
        // another time under the resting pointer
        PlotPanel::zoom_to(&harness.ctx, 2.0..=2.1);
        harness.run();
        harness.state_mut().0.seek(2.25);
        harness.step();
        harness.step();
        let view = transform(&harness).bounds().range_x();
        assert!(view.contains(&2.25) && !view.contains(&2.05), "{view:?}");
        assert_eq!(state(&harness), (Some(2.25), Some(2.25)));
        harness.state_mut().0.seek(t30);
        harness.event(egui::Event::PointerGone);
        harness.run();
        harness.state_mut().0.reset_view = true;
        harness.run();

        // playing, the pointer moves neither
        harness.state_mut().1 = true;
        let (_, p50) = at(&harness, 0.5);
        harness.hover_at(p50);
        harness.step();
        assert_eq!(state(&harness), (Some(t30), Some(t30)));
        // and a click seeks
        let (t70, p70) = at(&harness, 0.7);
        click_at(&mut harness, p70);
        assert_eq!(state(&harness), (Some(t70), Some(t70)));
    }

    #[test]
    fn a_drag_and_a_click_on_the_legend_do_not_seek() {
        let log = LoadedLog::build(
            dflog::Log::from_bytes(&crate::model::testlog::bytes()),
            "test.bin".into(),
        );
        let settings = Settings::default();
        let mut harness = roll_plot(&log, &settings);
        let (t50, p50) = at(&harness, 0.5);
        click_at(&mut harness, p50);
        assert_eq!(harness.state().0.playhead, Some(t50));

        // a drag pans the view and seeks nowhere
        let (_, from) = at(&harness, 0.3);
        harness.hover_at(from);
        harness.run();
        harness.drag_at(from);
        harness.run();
        for step in 1..=5u8 {
            harness.hover_at(from + egui::vec2(20.0 * f32::from(step), 0.0));
            harness.run();
        }
        harness.drop_at(from + egui::vec2(100.0, 0.0));
        harness.run();
        assert_eq!(harness.state().0.playhead, Some(t50));
        assert!(
            harness.state().0.zoomed_range(&harness.ctx).is_some(),
            "the drag panned"
        );

        // a click past the log's time, in the margin a view panned beyond
        // it shows, seeks to its end
        PlotPanel::zoom_to(&harness.ctx, 2.2..=2.6);
        harness.run();
        let (_, beyond) = at(&harness, 0.9);
        click_at(&mut harness, beyond);
        assert_eq!(harness.state().0.playhead, Some(2.33));
        harness.state_mut().0.seek(t50);
        harness.state_mut().0.reset_view = true;
        harness.run();

        // a click on the legend hides the series and seeks nowhere; the
        // series' chip above the plot carries its name too
        let frame = *transform(&harness).frame();
        let entry = harness
            .get_all_by_label("ATT.Roll (deg)")
            .map(|node| node.rect().center())
            .find(|center| frame.contains(*center))
            .expect("the legend names the series");
        click_at(&mut harness, entry);
        assert_eq!(harness.state().0.showing().count(), 0);
        assert_eq!(harness.state().0.playhead, Some(t50));
    }

    #[test]
    fn a_zoomed_view_pages_with_the_playhead_while_playing() {
        let log = LoadedLog::build(
            dflog::Log::from_bytes(&crate::model::testlog::bytes()),
            "test.bin".into(),
        );
        let settings = Settings::default();
        let mut harness = roll_plot(&log, &settings);
        let range_x =
            |h: &egui_kittest::Harness<'_, (PlotPanel, bool)>| transform(h).bounds().range_x();
        PlotPanel::zoom_to(&harness.ctx, 2.0..=2.1);
        harness.state_mut().0.play_to(2.05);
        harness.state_mut().1 = true;
        harness.step();
        assert_eq!(range_x(&harness), 2.0..=2.1, "the playhead is in view");

        // a view zoomed by hand stays while the playhead is in it
        harness.state_mut().0.play_to(2.098);
        harness.step();
        assert_eq!(range_x(&harness), 2.0..=2.1);
        // and turns when it leaves, to start 5% of the width before it
        harness.state_mut().0.play_to(2.102);
        harness.step();
        let view = range_x(&harness);
        assert!((view.start() - 2.097).abs() < 1e-9, "{view:?}");
        assert!((view.end() - view.start() - 0.1).abs() < 1e-9, "{view:?}");
        // a view paging set turns at 95% of its width
        harness.state_mut().0.play_to(2.19);
        harness.step();
        assert_eq!(range_x(&harness), view);
        harness.state_mut().0.play_to(2.195);
        harness.step();
        let view = range_x(&harness);
        assert!((view.start() - 2.19).abs() < 1e-9, "{view:?}");

        // paused, nothing pages
        harness.state_mut().1 = false;
        harness.state_mut().0.play_to(2.35);
        harness.step();
        assert_eq!(range_x(&harness), view);

        // playing again with the playhead off screen pages to it at once
        harness.state_mut().1 = true;
        harness.step();
        let view = range_x(&harness);
        assert!((view.start() - 2.345).abs() < 1e-9, "{view:?}");

        // a view the hand moves off the playhead stays there, however far
        // the playhead goes on
        PlotPanel::zoom_to(&harness.ctx, 2.0..=2.1);
        harness.step();
        harness.state_mut().0.play_to(2.36);
        harness.step();
        assert_eq!(range_x(&harness), 2.0..=2.1);
    }

    #[test]
    fn a_double_click_shows_the_whole_log_though_a_page_would_turn() {
        let log = LoadedLog::build(
            dflog::Log::from_bytes(&crate::model::testlog::bytes()),
            "test.bin".into(),
        );
        let settings = Settings::default();
        let mut harness = roll_plot(&log, &settings);
        PlotPanel::zoom_to(&harness.ctx, 2.0..=2.1);
        harness.state_mut().0.play_to(2.05);
        harness.state_mut().1 = true;
        harness.step();

        // two clicks, one event a frame; before the second one lets go,
        // the playhead leaves the view, which would turn the page in the
        // frame the reset is made
        let (_, pos) = at(&harness, 0.5);
        let button = |pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        harness.hover_at(pos);
        harness.step();
        for event in [button(true), button(false), button(true)] {
            harness.event(event);
            harness.step();
        }
        harness.state_mut().0.play_to(2.15);
        harness.event(button(false));
        harness.step();
        harness.step();
        assert!(
            harness.state().0.zoomed_range(&harness.ctx).is_none(),
            "{:?}",
            transform(&harness).bounds().range_x()
        );
    }

    /// The playhead and the cursor are lines across the plot that a view
    /// following the data never widens to take in.
    #[test]
    fn the_time_marks_never_widen_a_view_that_follows_the_data() {
        let log = LoadedLog::build(
            dflog::Log::from_bytes(&crate::model::testlog::bytes()),
            "test.bin".into(),
        );
        let settings = Settings::default();
        let mut harness = roll_plot(&log, &settings);
        let before = transform(&harness).bounds().range_x();
        assert!(*before.start() < 2.0 && *before.end() > 2.3, "{before:?}");
        harness.state_mut().0.seek(100.0);
        harness.state_mut().0.cursor = Some(-50.0);
        harness.run();
        assert_eq!(transform(&harness).bounds().range_x(), before);
        assert!(harness.state().0.zoomed_range(&harness.ctx).is_none());
    }

    /// A series hidden through the legend is shown again when a new log
    /// resets the plot, from the reset frame on.
    #[test]
    fn a_reset_frame_reads_nothing_as_hidden() {
        let log = LoadedLog::build(
            dflog::Log::from_bytes(&crate::model::testlog::bytes()),
            "test.bin".into(),
        );
        let settings = Settings::default();
        let roll = SeriesKey {
            type_name: "ATT".into(),
            field: "Roll".into(),
            instance: None,
        };
        let mut plot = PlotPanel::default();
        plot.toggle(roll.clone(), &log);
        let mut harness = egui_kittest::Harness::new_ui_state(
            |ui, plot: &mut PlotPanel| plot.show(ui, &log, &settings, false),
            plot,
        );
        harness.run();
        assert_eq!(harness.state().showing().count(), 1);
        assert!(
            harness.state().zoomed_range(&harness.ctx).is_none(),
            "the view follows the data"
        );

        // zoomed, the view is a range to export
        let id = Id::new(PLOT_ID);
        let mut memory = PlotMemory::load(&harness.ctx, id).unwrap();
        memory.auto_bounds.x = false;
        memory.store(&harness.ctx, id);
        let range = harness.state().zoomed_range(&harness.ctx).unwrap();
        assert!(range.contains(&2.0) && range.contains(&2.3), "{range:?}");

        // hide the series as a click on its legend entry would
        let mut memory = PlotMemory::load(&harness.ctx, id).unwrap();
        memory.hidden_items.insert(line_id(&roll));
        memory.store(&harness.ctx, id);
        harness.step();
        assert_eq!(harness.state().hidden, [line_id(&roll)]);
        assert_eq!(harness.state().showing().count(), 0);

        // a reset shows everything and stops trusting the stored view
        harness.state_mut().reload(&log);
        assert!(harness.state().zoomed_range(&harness.ctx).is_none());
        harness.step();
        assert!(harness.state().hidden.is_empty(), "shown from the reset on");
        assert!(!harness.state().resets_view());
        harness.step();
        assert!(harness.state().hidden.is_empty(), "and after it");
        assert!(harness.state().zoomed_range(&harness.ctx).is_none());
    }

    /// The mode names sit in a lane of their own above the plot, clear of
    /// the legend in its top left corner, and the lane goes with the
    /// bands.
    #[test]
    fn the_mode_names_sit_above_the_plot() {
        let log = LoadedLog::build(
            dflog::Log::from_bytes(&crate::model::testlog::bytes()),
            "test.bin".into(),
        );
        let with = Settings::default();
        let without = Settings {
            show_modes: false,
            ..Settings::default()
        };
        let frame = |settings: &Settings| *transform(&roll_plot(&log, settings)).frame();
        let (frame, bare) = (frame(&with), frame(&without));
        let lane = frame.top() - bare.top();
        assert!(lane > 10.0, "the lane takes {lane}");

        let view = transform(&roll_plot(&log, &with));
        let strip = Rect::from_x_y_ranges(frame.x_range(), frame.top() - lane..=frame.top());
        let spots = label_spots(&log, 0.0, &view, strip);
        let names: Vec<&str> = spots.iter().map(|(_, m)| m.name.as_str()).collect();
        // Stabilize began before the view, so it is named at its left edge
        assert_eq!(names, ["Stabilize", "Loiter"]);
        assert!(spots.iter().all(|(at, _)| at.y < frame.top()));
        assert!((spots[0].0.x - (frame.left() + 4.0)).abs() < 1e-3);
        let loiter = view.position_from_point_x(2.25) + 2.0;
        assert!((spots[1].0.x - loiter).abs() < 1e-3);

        // B's names at their moved places, in a second lane, only with B
        // open and the bands on: the same log 0.05 s later
        let b_log = Arc::new(LoadedLog::build(
            dflog::Log::from_bytes(&crate::model::testlog::bytes()),
            "b.bin".into(),
        ));
        let spots = label_spots(&b_log, 0.05, &view, strip);
        let moved = view.position_from_point_x(2.3) + 2.0;
        assert!((spots[1].0.x - moved).abs() < 1e-3);
        let two_lanes = |settings: &Settings| {
            let mut harness = roll_plot(&log, settings);
            harness
                .state_mut()
                .0
                .set_b(Some((Arc::clone(&b_log), 0.05)));
            harness.run();
            *transform(&harness).frame()
        };
        // a line of small text more, without the spacing the first brings
        let added = two_lanes(&with).top() - frame.top();
        assert!(added > 10.0 && added < lane, "{added} against {lane}");
        assert!((two_lanes(&without).top() - bare.top()).abs() < 1.0);
        // and none for a B that logs no mode changes
        let mut harness = roll_plot(&log, &with);
        let b_without_modes = Arc::new(LoadedLog::build(
            dflog::Log::from_bytes(&crate::model::testlog::b_bytes()),
            "b.bin".into(),
        ));
        harness.state_mut().0.set_b(Some((b_without_modes, 0.0)));
        harness.run();
        assert!((transform(&harness).frame().top() - frame.top()).abs() < 1.0);
    }

    /// The points a frame keeps are those a fresh thinning over its range
    /// and width gives, and frames whose view holds thin nothing again.
    #[test]
    fn a_hit_gives_the_points_a_fresh_thinning_gives() {
        let log = test_log();
        let settings = Settings::default();
        let mut harness = roll_plot(&log, &settings);
        harness.state_mut().0.set_b(Some((b_log(), -8.0)));
        harness.run();
        let misses = harness.state().0.misses;
        assert!(misses >= 2, "both lines were thinned: {misses}");
        harness.step();
        harness.step();
        assert_eq!(harness.state().0.misses, misses, "the view holds");

        let width = transform(&harness).frame().width();
        let plot = &harness.state().0;
        let range = full_range(&plot.selected, &[]).unwrap();
        let s = &plot.selected[0];
        let fresh_a = decimate(&s.data.xs, &s.data.ys, &range, width, |y| y);
        assert_eq!(s.thinned.as_ref().unwrap().points, fresh_a);
        let b = s.data_b.as_ref().unwrap();
        let fresh_b = clip(
            &decimate_moved(&b.xs, &b.ys, None, &range, width, |y| y, -8.0),
            &range,
        );
        assert_eq!(s.thinned_b.as_ref().unwrap().points, fresh_b);
        assert_eq!(plot.thin_all(&harness.ctx), fresh_a.len() + fresh_b.len());
    }

    /// Every part of the key misses on its own, and only the lines it
    /// concerns: a left-axis line owes the axis map nothing, and B's
    /// offset and clipping are B's alone.
    #[test]
    fn each_part_of_the_key_misses_alone() {
        fn lines(
            map: Option<AxisMap>,
            range: &RangeInclusive<f64>,
            width: f32,
            offset: f64,
            within: bool,
        ) -> Lines<'_> {
            Lines {
                map,
                range,
                width,
                b_offset: Some(offset),
                within_range: within,
            }
        }

        let mut s = pair("Roll", [0.0, 1.0, 0.0], [1.0, 2.0, 1.0], "deg");
        let full = 0.0..=2.0;
        let map = Some(AxisMap {
            left: (0.0, 1.0),
            right: (0.0, 2.0),
            two_sided: true,
        });
        let other_map = Some(AxisMap {
            left: (0.0, 1.0),
            right: (0.0, 4.0),
            two_sided: true,
        });
        let same = lines(None, &full, 100.0, 0.5, true);
        assert_eq!(same.refresh(&mut s, &[]), 2, "nothing kept yet");
        assert_eq!(same.refresh(&mut s, &[]), 0, "the same frame again");
        assert_eq!(
            lines(None, &full, 101.0, 0.5, true).refresh(&mut s, &[]),
            2,
            "the width"
        );
        assert_eq!(
            lines(None, &(0.0..=1.5), 101.0, 0.5, true).refresh(&mut s, &[]),
            2,
            "the range"
        );
        // the map is nothing to a left-axis line
        assert_eq!(
            lines(map, &(0.0..=1.5), 101.0, 0.5, true).refresh(&mut s, &[]),
            0
        );
        s.axis = YAxis::Right;
        assert_eq!(
            lines(map, &(0.0..=1.5), 101.0, 0.5, true).refresh(&mut s, &[]),
            2,
            "the axis"
        );
        assert_eq!(
            lines(other_map, &(0.0..=1.5), 101.0, 0.5, true).refresh(&mut s, &[]),
            2,
            "the map"
        );
        assert_eq!(
            lines(other_map, &(0.0..=1.5), 101.0, 0.6, true).refresh(&mut s, &[]),
            1,
            "B's offset"
        );
        assert_eq!(
            lines(other_map, &(0.0..=1.5), 101.0, 0.6, false).refresh(&mut s, &[]),
            1,
            "B's clip"
        );
        // a hidden line is left as it is
        let a = line_id(&s.key);
        assert_eq!(
            lines(other_map, &full, 101.0, 0.6, false).refresh(&mut s, &[a]),
            1,
            "B alone"
        );
        assert_eq!(
            lines(other_map, &full, 101.0, 0.6, false).refresh(&mut s, &[]),
            1,
            "then A"
        );
    }

    #[test]
    fn a_reload_a_new_b_and_a_swap_clear_the_entries() {
        let log = test_log();
        let b = b_log();
        let mut plot = PlotPanel::default();
        plot.toggle(roll(), &log);
        plot.set_b(Some((Arc::clone(&b), -8.0)));
        let full = 0.0..=20.0;
        let lines = Lines {
            map: None,
            range: &full,
            width: 100.0,
            b_offset: Some(-8.0),
            within_range: true,
        };
        let kept = |plot: &PlotPanel| {
            let s = &plot.selected[0];
            (s.thinned.is_some(), s.thinned_b.is_some())
        };
        assert_eq!(lines.refresh(&mut plot.selected[0], &[]), 2);
        assert_eq!(kept(&plot), (true, true));
        plot.set_b(Some((b_log(), -8.0)));
        assert_eq!(kept(&plot), (true, false), "a new B clears B's");
        lines.refresh(&mut plot.selected[0], &[]);
        plot.swap(&b, Arc::new(test_log()), 8.0);
        assert_eq!(kept(&plot), (false, false), "a swap clears both");
        lines.refresh(&mut plot.selected[0], &[]);
        plot.reload(&log);
        assert_eq!(kept(&plot), (false, false), "a reload clears both");
    }

    /// The bounds of a zoomed view come back from the plot's memory as they
    /// were set, so a still view hits, and only a move thins again. A zoom
    /// takes a few frames to settle: the values fit the view anew, their
    /// labels can widen or narrow the plot, and the lines are thinned to
    /// the width of the frame before.
    #[test]
    fn a_still_zoomed_view_thins_nothing_again() {
        let log = test_log();
        let settings = Settings::default();
        let mut harness = roll_plot(&log, &settings);
        let misses = harness.state().0.misses;
        PlotPanel::zoom_to(&harness.ctx, 2.2..=2.6);
        harness.run_steps(3);
        let zoomed = harness.state().0.misses;
        assert!(zoomed > misses, "the new view was thinned");
        harness.step();
        harness.step();
        assert_eq!(harness.state().0.misses, zoomed, "the view holds");
        PlotPanel::zoom_to(&harness.ctx, 2.3..=2.7);
        harness.step();
        assert!(harness.state().0.misses > zoomed, "a pan thins again");
    }

    /// A line hidden through the legend is not thinned when the view
    /// moves, and its entry stays in the legend to show it again.
    #[test]
    fn a_hidden_line_is_not_thinned_and_its_legend_entry_stays() {
        let log = test_log();
        let settings = Settings::default();
        let mut harness = roll_plot(&log, &settings);
        harness.state_mut().0.set_b(Some((b_log(), -8.0)));
        harness.run();
        let id = Id::new(PLOT_ID);
        let mut memory = PlotMemory::load(&harness.ctx, id).unwrap();
        memory.hidden_items.insert(line_id(&roll()));
        memory.store(&harness.ctx, id);
        harness.run();
        assert_eq!(harness.state().0.hidden, [line_id(&roll())]);
        // with nothing kept for it either, as after a swap
        harness.state_mut().0.selected[0].thinned = None;
        let misses = harness.state().0.misses;
        PlotPanel::zoom_to(&harness.ctx, 2.2..=2.6);
        harness.step();
        assert_eq!(harness.state().0.misses, misses + 1, "B's line alone");
        assert!(harness.state().0.selected[0].thinned.is_none());
        harness.step();
        assert_eq!(harness.state().0.hidden, [line_id(&roll())], "still hidden");
        let frame = *transform(&harness).frame();
        assert!(
            harness
                .get_all_by_label("ATT.Roll (deg)")
                .any(|node| frame.contains(node.rect().center())),
            "the legend still names the hidden line"
        );
        // shown again, it is thinned for the view it missed
        let mut memory = PlotMemory::load(&harness.ctx, id).unwrap();
        memory.hidden_items.clear();
        memory.store(&harness.ctx, id);
        harness.step();
        assert_eq!(harness.state().0.misses, misses + 2);
        assert!(harness.state().0.hidden.is_empty());
    }

    /// Numbers in `0..1` from a fixed seed.
    fn random(seed: u64) -> impl FnMut() -> f64 {
        let mut state = seed;
        move || {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (state >> 33) as f64 / f64::from(1u32 << 31)
        }
    }

    /// Samples at a jittered rate with random values and a spike every so
    /// often; with `sparse_half`, the second half a hundred times sparser,
    /// as a field logged at 400 Hz then 4 Hz is.
    fn jittered(n: usize, seed: u64, sparse_half: bool) -> Series {
        let mut next = random(seed);
        let mut x = 0.0;
        let mut xs = Vec::with_capacity(n);
        let mut ys = Vec::with_capacity(n);
        for i in 0..n {
            let step = if sparse_half && i >= n / 2 {
                100.0
            } else {
                1.0
            };
            x += (0.5 + next()) * step;
            xs.push(x);
            let spike = if i % 997 == 0 { 50.0 } else { 1.0 };
            ys.push((next() * 2.0 - 1.0) * spike);
        }
        Series { xs, ys }
    }

    /// Read through the blocks, a thinning gives the plain walk's points
    /// to the bit, whatever the view, the width or the rate: the same
    /// buckets, each bucket's extremes exact, ties to the first.
    #[test]
    fn the_blocks_give_the_walk_s_points_to_the_bit() {
        let identity = |y| y;
        for s in [jittered(50_000, 7, false), jittered(50_000, 11, true)] {
            let summary = Summary::new(&s.ys);
            let n = s.xs.len();
            let last = *s.xs.last().unwrap();
            let mut next = random(3);
            let mut read_by_blocks = 0;
            for _ in 0..60 {
                let (a, b) = (next() * last, next() * last);
                let range = a.min(b)..=a.max(b);
                let width = 1.0 + (next() * 400.0).floor() as f32;
                let plain = decimate(&s.xs, &s.ys, &range, width, identity);
                let summed = decimate_with(&s.xs, &s.ys, Some(&summary), &range, width, identity);
                assert_eq!(summed, plain, "{range:?} at {width}");
                // thinned at all: more samples in range than points kept
                let in_range = s.xs.partition_point(|x| x <= range.end())
                    - s.xs.partition_point(|x| x < range.start());
                if in_range > 2 * (width as usize) + 2 {
                    read_by_blocks += 1;
                    assert!(summed.len() <= 2 * (width as usize) + 2);
                }
            }
            assert!(read_by_blocks >= 50, "{read_by_blocks} views were thinned");
            // spans with partial blocks at either end, over the buckets
            // themselves
            for (lo, hi) in [(0, n), (37, n - 51), (1000, 9000), (5, 20), (16, 48)] {
                for buckets in [1, 7, 50, 156, 2000] {
                    let mut ends = bucket_ends(&s.xs[lo..hi], buckets);
                    for end in &mut ends {
                        *end += lo;
                    }
                    assert_eq!(*ends.last().unwrap(), hi);
                    let by_blocks =
                        bucket_points(&s.xs, &s.ys, lo, &ends, Some(&summary), identity);
                    let by_walk = bucket_points(&s.xs, &s.ys, lo, &ends, None, identity);
                    assert_eq!(by_blocks, by_walk, "lo {lo} hi {hi} buckets {buckets}");
                }
            }
        }
        // ties go to the first sample, across blocks and the samples
        // around them alike
        let flat = [2.0; 40];
        let summary = Summary::new(&flat);
        assert_eq!(extremes(&flat, 3, 40, Some(&summary)), Some((3, 3)));
        assert_eq!(extremes(&flat, 16, 40, Some(&summary)), Some((16, 16)));
        assert_eq!(extremes(&flat, 0, 40, Some(&summary)), Some((0, 0)));
        let mut ys = vec![0.0; 100];
        ys[20] = 5.0;
        ys[50] = -1.0;
        ys[70] = -1.0;
        ys[90] = 5.0;
        assert_eq!(
            extremes(&ys, 0, 100, Some(&Summary::new(&ys))),
            Some((50, 20))
        );
        assert_eq!(
            extremes(&ys, 60, 100, Some(&Summary::new(&ys))),
            Some((70, 90))
        );
        assert_eq!(extremes(&ys, 7, 7, Some(&Summary::new(&ys))), None);
    }

    /// The blocks hold the first lowest and first highest of their samples,
    /// level over level, the last block of each short; and a span is read
    /// by the largest aligned blocks that fit it.
    #[test]
    fn the_summary_s_levels_and_the_blocks_read() {
        let mut ys = vec![0.0; 40];
        ys[5] = -3.0;
        ys[9] = 7.0;
        ys[20] = -3.0;
        ys[30] = 9.0;
        ys[39] = -4.0;
        let summary = Summary::new(&ys);
        assert_eq!(summary.levels.len(), 3);
        assert_eq!(summary.levels[0], [[5, 9], [20, 30], [39, 32]]);
        assert_eq!(summary.levels[1], [[5, 30], [39, 32]], "the first of a tie");
        assert_eq!(summary.levels[2], [[39, 30]]);
        // at 0 with room for 64: the top level; at 0 with room for 40:
        // 32; at 16: 16, as 32 does not align there; at 32: 16
        assert_eq!(summary.largest_block_at(0, 64), (2, 64));
        assert_eq!(summary.largest_block_at(0, 40), (1, 32));
        assert_eq!(summary.largest_block_at(16, 40), (0, 16));
        assert_eq!(summary.largest_block_at(32, 48), (0, 16));
        // the same choice as trying every level from the top down
        let long = Summary::new(&vec![0.0; 5000]);
        let top = long.levels.len() - 1;
        for i in (0..5000_usize).step_by(16) {
            for end in i + 16..=5000 {
                let block = |level: &usize| Summary::BASE << level;
                let tried = (1..=top)
                    .rev()
                    .find(|level| i.is_multiple_of(block(level)) && i + block(level) <= end)
                    .unwrap_or(0);
                assert_eq!(
                    long.largest_block_at(i, end),
                    (tried, block(&tried)),
                    "{i} {end}"
                );
            }
        }
        // nothing to read: the first level, no panic
        assert_eq!(summary.largest_block_at(32, 40), (0, 16));
        assert_eq!(Summary::new(&[]).largest_block_at(0, 64), (0, 16));
        assert!(Summary::new(&[]).levels.is_empty());
        assert_eq!(Summary::new(&[1.0]).levels, [[[0, 0]]]);
    }

    /// A series carries the blocks of each side, built with the side.
    #[test]
    fn a_series_summarizes_both_sides_as_they_are_set() {
        let log = test_log();
        let mut plot = PlotPanel::default();
        plot.toggle(roll(), &log);
        let s = &plot.selected[0];
        assert_eq!(s.summary.levels[0].len(), s.data.ys.len().div_ceil(16));
        assert!(s.summary_b.is_none());
        plot.set_b(Some((b_log(), -8.0)));
        let s = &plot.selected[0];
        let b = s.data_b.as_ref().unwrap();
        assert_eq!(
            s.summary_b.as_ref().unwrap().levels[0].len(),
            b.ys.len().div_ceil(16)
        );
        plot.reload(&log);
        let s = &plot.selected[0];
        assert!(s.summary_b.is_none());
        assert_eq!(s.summary.levels[0].len(), s.data.ys.len().div_ceil(16));
    }

    #[test]
    fn a_clipped_line_runs_to_the_edges() {
        let p = |x: f64, y: f64| PlotPoint::new(x, y);
        let xy = |points: Vec<PlotPoint>| -> Vec<(f64, f64)> {
            points.iter().map(|q| (q.x, q.y)).collect()
        };
        // no point inside: the segment across the range still shows
        assert_eq!(
            xy(clip(&[p(0.0, 0.0), p(10.0, 10.0)], &(2.0..=4.0))),
            [(2.0, 2.0), (4.0, 4.0)]
        );
        // points inside kept, the line ended on each edge it crosses
        assert_eq!(
            xy(clip(
                &[p(0.0, 0.0), p(3.0, 6.0), p(5.0, 4.0), p(9.0, 0.0)],
                &(1.0..=7.0)
            )),
            [(1.0, 2.0), (3.0, 6.0), (5.0, 4.0), (7.0, 2.0)]
        );
        // all on one side: nothing
        assert!(clip(&[p(8.0, 1.0), p(9.0, 1.0)], &(1.0..=7.0)).is_empty());
    }

    fn roll() -> SeriesKey {
        SeriesKey {
            type_name: "ATT".into(),
            field: "Roll".into(),
            instance: None,
        }
    }

    fn test_log() -> LoadedLog {
        LoadedLog::build(
            dflog::Log::from_bytes(&crate::model::testlog::bytes()),
            "test.bin".into(),
        )
    }

    fn b_log() -> Arc<LoadedLog> {
        Arc::new(LoadedLog::build(
            dflog::Log::from_bytes(&crate::model::testlog::b_bytes()),
            "b.bin".into(),
        ))
    }

    #[test]
    fn a_series_takes_b_side_where_b_has_the_field() {
        let a = test_log();
        let b = b_log();
        let key = |type_name: &str, field: &str, instance| SeriesKey {
            type_name: type_name.into(),
            field: field.into(),
            instance,
        };
        let mut plot = PlotPanel::default();
        plot.toggle(roll(), &a);
        plot.toggle(key("ATT", "Pitch", None), &a);
        plot.toggle(key("IMU", "GyrX", Some(0)), &a);
        plot.toggle(key("IMU", "GyrX", Some(1)), &a);
        assert!(plot.selected.iter().all(|s| s.data_b.is_none()), "no B yet");

        // B has the roll, no pitch, and its one IMU stands for A's first
        plot.set_b(Some((Arc::clone(&b), -8.0)));
        let sides: Vec<(bool, Option<&str>)> = plot
            .selected
            .iter()
            .map(|s| (s.data_b.is_some(), s.b_note.as_deref()))
            .collect();
        assert_eq!(
            sides,
            [
                (true, None),
                (false, None),
                (true, Some("B's IMU.GyrX")),
                (false, None)
            ]
        );
        assert_eq!(
            plot.selected[0].data_b.as_ref().unwrap().ys,
            [5.0, 6.0, 7.0, 8.0, 9.0]
        );

        // a seek, as the map's, reads both, B at the cursor less the offset
        plot.seek(2.1);
        let r = &plot.readout[0];
        assert_eq!((r.a, r.b), (Some((2.1, 1.5)), Some((10.1, 6.0))));
        assert!((r.difference.unwrap() + 4.5).abs() < 1e-9);
        // the pitch reads A alone, B's place a dash
        assert!(plot.readout[1].a.is_some() && plot.readout[1].b.is_none());
        // a new offset reads again at once, keeping the extracted sides
        let data = plot.selected[0].data_b.as_ref().unwrap().xs.as_ptr();
        plot.set_b(Some((Arc::clone(&b), -7.9)));
        assert_eq!(plot.readout[0].b, Some((10.0, 5.0)));
        assert_eq!(plot.selected[0].data_b.as_ref().unwrap().xs.as_ptr(), data);

        // a series picked while B is open takes its side at once
        plot.toggle(roll(), &a);
        plot.toggle(roll(), &a);
        assert!(plot.selected.last().unwrap().data_b.is_some());

        // closed, B's sides go, and the readout reads A alone
        plot.set_b(None);
        assert!(plot.selected.iter().all(|s| s.data_b.is_none()));
        assert!(plot.readout.iter().all(|r| r.b.is_none()));
        // a new A closes B
        plot.set_b(Some((Arc::clone(&b), -8.0)));
        plot.reload(&a);
        assert!(plot.b.is_none());
        assert!(plot.selected.iter().all(|s| s.data_b.is_none()));

        // the other way round: A logs the type as one instance, B as two
        let mut swapped = PlotPanel::default();
        swapped.toggle(key("IMU", "GyrX", None), &b);
        swapped.set_b(Some((Arc::new(test_log()), 0.0)));
        assert_eq!(
            swapped.selected[0].b_note.as_deref(),
            Some("B's IMU[0].GyrX")
        );
        assert_eq!(swapped.selected[0].data_b.as_ref().unwrap().ys[..1], [0.25]);
    }

    /// A selected series with `ys` on the left axis at 0, 1, 2 s, and B's
    /// `ys_b` at the same times, in `unit`.
    fn pair(field: &str, ys: [f64; 3], ys_b: [f64; 3], unit: &str) -> Selected {
        let xs = vec![0.0, 1.0, 2.0];
        let mut s = Selected::new(
            SeriesKey {
                type_name: "T".into(),
                field: field.into(),
                instance: None,
            },
            field.into(),
            YAxis::Left,
            Color32::WHITE,
            Series {
                xs: xs.clone(),
                ys: ys.to_vec(),
            },
        );
        s.unit = Some(unit.into());
        s.extent_b = extent(ys_b);
        s.summary_b = Some(Summary::new(&ys_b));
        s.data_b = Some(Series {
            xs,
            ys: ys_b.to_vec(),
        });
        s
    }

    #[test]
    fn the_readout_reads_both_sides_and_their_difference() {
        let alt = pair("alt", [10.0, 20.0, 30.0], [9.0, 18.0, 33.0], "m");
        let both = [alt];
        // B read 0.1 s before the cursor, on its own clock
        let r = &readout_at(&both, 1.0, &[], Some(-0.1))[0];
        assert_eq!(
            (r.a, r.b, r.difference),
            (Some((1.0, 20.0)), Some((1.0, 18.0)), Some(2.0))
        );
        assert_eq!(
            readout_text(r, 1.0, Some(1.1)),
            "alt = 20  B 18 (-0.100 s)  \u{394} +2"
        );
        // outside a side's samples, that side is a dash, and so is the
        // difference; the places stay
        let r = &readout_at(&both, 2.5, &[], Some(0.6))[0];
        assert_eq!((r.a, r.b), (None, Some((2.0, 33.0))));
        assert_eq!(
            readout_text(r, 2.5, Some(1.9)),
            "alt = \u{2013}  B 33 (+0.100 s)  \u{394} \u{2013}"
        );
        // a hidden line reads as a dash; with both hidden the row goes
        let key = both[0].key.clone();
        let r = &readout_at(&both, 1.0, &[line_id_b(&key)], Some(0.0))[0];
        assert_eq!((r.a.is_some(), r.b, r.difference), (true, None, None));
        let r = &readout_at(&both, 1.0, &[line_id(&key)], Some(0.0))[0];
        assert_eq!((r.a, r.b.is_some()), (None, true));
        assert!(readout_at(&both, 1.0, &[line_id(&key), line_id_b(&key)], Some(0.0)).is_empty());

        // a heading goes the short way round; a rate does not
        let yaw = [pair("yaw", [359.0; 3], [1.0; 3], "degheading")];
        assert_eq!(
            readout_at(&yaw, 1.0, &[], Some(0.0))[0].difference,
            Some(-2.0)
        );
        let back = [pair("yaw", [1.0; 3], [359.0; 3], "deg")];
        assert_eq!(
            readout_at(&back, 1.0, &[], Some(0.0))[0].difference,
            Some(2.0)
        );
        let rate = [pair("rate", [359.0; 3], [1.0; 3], "deg/s")];
        assert_eq!(
            readout_at(&rate, 1.0, &[], Some(0.0))[0].difference,
            Some(358.0)
        );

        // no difference across units, and the chip says why
        let mut alt = pair("alt", [10.0; 3], [900.0; 3], "m");
        alt.unit_b = Some("cm".into());
        assert_eq!(
            alt.b_hover().as_deref(),
            Some("B logs it in cm: no \u{394} is read")
        );
        let r = &readout_at(std::slice::from_ref(&alt), 1.0, &[], Some(0.0))[0];
        assert_eq!(
            (r.a, r.b, r.difference),
            (Some((1.0, 10.0)), Some((1.0, 900.0)), None)
        );
        alt.unit_b = Some("m".into());
        assert_eq!(alt.b_hover(), None);
        // a heading is degrees, as an angle is
        let mut yaw = [pair("yaw", [359.0; 3], [1.0; 3], "deg")];
        yaw[0].unit_b = Some("degheading".into());
        assert_eq!(
            readout_at(&yaw, 1.0, &[], Some(0.0))[0].difference,
            Some(-2.0)
        );
        alt.data_b = None;
        assert_eq!(alt.b_hover().as_deref(), Some("not in B"));
        assert!((difference(10.0, 190.0, true) - 180.0).abs() < 1e-12);
    }

    #[test]
    fn b_is_thinned_where_it_shows_and_drawn_on_a_clock() {
        let s = series(20_000);
        // B's 10 s to 11 s drawn at 60 s to 61 s
        let moved = decimate_moved(&s.xs, &s.ys, None, &(60.0..=61.0), 800.0, |y| y, 50.0);
        let plain = decimate(&s.xs, &s.ys, &(10.0..=11.0), 800.0, |y| y);
        assert_eq!(moved.len(), plain.len());
        for (m, p) in moved.iter().zip(&plain) {
            assert!((m.x - p.x - 50.0).abs() < 1e-9 && (m.y - p.y).abs() < 1e-12);
        }
        assert!(moved[0].x < 60.0 && moved.last().unwrap().x > 61.0);
        // nowhere near: nothing
        assert!(decimate_moved(&s.xs, &s.ys, None, &(0.0..=1.0), 800.0, |y| y, 7200.0).is_empty());
    }

    #[test]
    fn a_swap_finds_a_side_as_b_side_is_found() {
        let a = test_log();
        let b = b_log();
        let gyr = |instance| SeriesKey {
            type_name: "IMU".into(),
            field: "GyrX".into(),
            instance,
        };
        // the title without its unit, which only the test log gives
        let name = |s: &Selected| s.title.split(' ').next().unwrap().to_string();
        let mut plot = PlotPanel::default();
        plot.toggle(gyr(Some(0)), &a);
        plot.toggle(gyr(Some(1)), &a);
        // B's one IMU stands for A's first
        plot.set_b(Some((Arc::clone(&b), -8.0)));
        assert_eq!(plot.selected[0].b_note.as_deref(), Some("B's IMU.GyrX"));

        // swapped, that IMU is A's side of the first, under its own name,
        // and the second, which the new A lacks, is B's alone
        plot.swap(&b, Arc::new(test_log()), 8.0);
        let sides: Vec<(String, bool, bool)> = plot
            .selected
            .iter()
            .map(|s| (name(s), s.only_b(), s.data_b.is_some()))
            .collect();
        assert_eq!(
            sides,
            [
                ("IMU.GyrX".to_string(), false, true),
                ("IMU[1].GyrX".to_string(), true, true)
            ]
        );
        assert_eq!(plot.selected[0].data.ys, b.series(&gyr(None)).unwrap().ys);
        assert_eq!(
            plot.selected[0].key,
            gyr(Some(0)),
            "the series keeps its key"
        );
        // the side panel ticks the new A's field, and the second ticks none
        assert!(plot.is_selected(&gyr(None)));
        assert!(!plot.is_selected(&gyr(Some(0))) && !plot.is_selected(&gyr(Some(1))));

        // and back, as it was
        plot.swap(&a, Arc::clone(&b), -8.0);
        let titles: Vec<String> = plot.selected.iter().map(name).collect();
        assert_eq!(titles, ["IMU[0].GyrX", "IMU[1].GyrX"]);
        assert!(plot.selected.iter().all(|s| !s.only_b()));
        assert!(plot.is_selected(&gyr(Some(0))) && !plot.is_selected(&gyr(None)));

        // swapped again, unticking the new A's field takes off the series
        // already there rather than adding a second
        plot.swap(&b, Arc::new(test_log()), 8.0);
        plot.toggle(gyr(None), &b);
        let keys: Vec<&SeriesKey> = plot.selected.iter().map(|s| &s.key).collect();
        assert_eq!(keys, [&gyr(Some(1))]);
        assert!(!plot.is_selected(&gyr(None)));
    }

    #[test]
    fn a_field_only_b_has_plots_for_b_side_alone() {
        let a = test_log();
        let extra = Arc::new(LoadedLog::build(
            dflog::Log::from_bytes(&crate::model::testlog::b_extra_bytes()),
            "c.bin".into(),
        ));
        let gyr2 = SeriesKey {
            type_name: "IMU".into(),
            field: "GyrX".into(),
            instance: Some(2),
        };
        let mut plot = PlotPanel::default();
        // without B, nothing
        plot.toggle_b(gyr2.clone());
        assert!(plot.selected.is_empty());

        plot.toggle(roll(), &a);
        plot.set_b(Some((Arc::clone(&extra), -10.0)));
        plot.toggle_b(gyr2.clone());
        assert_eq!(plot.selected.len(), 2);
        let s = &plot.selected[1];
        assert!(s.only_b() && s.data.xs.is_empty());
        assert_eq!(s.b_key.as_ref(), Some(&gyr2));
        assert_eq!(
            (s.unit.as_deref(), s.unit_b.as_deref()),
            (None, Some("rad/s"))
        );
        assert_eq!(s.title, "IMU[2].GyrX (rad/s)");
        assert_eq!(s.color, PALETTE[1], "the next color");
        assert_eq!(s.b_hover().as_deref(), Some("not in A"));
        // ticked as B's, not A's, and never written as one of A's
        assert!(plot.is_selected_b(&gyr2) && !plot.is_selected(&gyr2));
        assert_eq!(plot.showing().count(), 1);
        assert!(plot.has_a_series());
        // A's line hidden through the legend: still a series of A's
        plot.hidden = vec![line_id(&roll())];
        assert_eq!(plot.showing().count(), 0);
        assert!(plot.has_a_series());
        plot.hidden.clear();
        // A's series off: B's line alone, which no export writes
        plot.toggle(roll(), &a);
        assert!(!plot.has_a_series());
        // a second tick takes it off
        plot.toggle_b(gyr2.clone());
        assert!(plot.selected.is_empty() && !plot.is_selected_b(&gyr2));
        // a field B lacks puts nothing on
        plot.toggle_b(SeriesKey {
            field: "AccX".into(),
            ..gyr2.clone()
        });
        assert!(plot.selected.is_empty());

        // B's key is kept with B's side, and goes with it
        plot.toggle(roll(), &a);
        assert_eq!(plot.selected[0].b_key.as_ref(), Some(&roll()));
        plot.set_b(None);
        assert_eq!(plot.selected[0].b_key, None);
    }

    #[test]
    fn a_swap_left_series_ticks_by_b_key_in_its_other_instance_form() {
        let one = Arc::new(LoadedLog::build(
            dflog::Log::from_bytes(&crate::model::testlog::b_bytes()),
            "one.bin".into(),
        ));
        let two = Arc::new(test_log());
        let mut w = dflog::write::LogWriter::new();
        w.define(6, "ATT", "Qc", &["TimeUS", "Roll"]).unwrap();
        w.record(
            "ATT",
            &[
                dflog::access::Value::U64(1_000_000),
                dflog::access::Value::F64(1.0),
            ],
        )
        .unwrap();
        let no_imu = Arc::new(LoadedLog::build(
            dflog::Log::from_bytes(&w.into_bytes()),
            "none.bin".into(),
        ));
        let gyr = |instance| SeriesKey {
            type_name: "IMU".into(),
            field: "GyrX".into(),
            instance,
        };
        // IMU.GyrX picked from a log of one IMU, the series keyed so
        let mut plot = PlotPanel::default();
        plot.toggle(gyr(None), &one);
        // swapped with a log of two, then compared with one of none, and
        // swapped again: B's alone, keyed IMU, B's side read as IMU[0]
        plot.set_b(Some((Arc::clone(&two), 0.0)));
        plot.swap(&two, Arc::clone(&one), 0.0);
        plot.set_b(Some((Arc::clone(&no_imu), 0.0)));
        plot.swap(&no_imu, Arc::clone(&two), 0.0);
        let s = &plot.selected[0];
        assert!(s.only_b());
        assert_eq!(
            (s.key.clone(), s.b_key.clone()),
            (gyr(None), Some(gyr(Some(0))))
        );
        // and named as B names it
        assert_eq!(s.title, "IMU[0].GyrX (rad/s)");
        // B's list names it IMU[0]: ticked, and a tick takes it off rather
        // than plotting it twice
        assert!(plot.is_selected_b(&gyr(Some(0))) && !plot.is_selected_b(&gyr(None)));
        plot.toggle_b(gyr(Some(0)));
        assert!(plot.selected.is_empty());
    }

    #[test]
    fn a_field_from_b_then_a_swap_is_a_s() {
        let a = test_log();
        let extra = Arc::new(LoadedLog::build(
            dflog::Log::from_bytes(&crate::model::testlog::b_extra_bytes()),
            "c.bin".into(),
        ));
        let vibe = SeriesKey {
            type_name: "VIBE".into(),
            field: "VibeX".into(),
            instance: None,
        };
        let mut plot = PlotPanel::default();
        plot.set_b(Some((Arc::clone(&extra), 0.0)));
        plot.toggle_b(vibe.clone());
        // swapped, the field is the new A's, which A's tree ticks
        plot.swap(&extra, Arc::new(test_log()), 0.0);
        assert!(!plot.selected[0].only_b());
        assert!(plot.is_selected(&vibe) && !plot.is_selected_b(&vibe));
        // swapped back, B's alone again
        plot.swap(&a, Arc::clone(&extra), 0.0);
        assert!(plot.selected[0].only_b() && plot.is_selected_b(&vibe));
        // Close B drops it
        plot.set_b(None);
        assert!(plot.selected.is_empty());

        // picked from B, then a new A that logs the field: A's
        plot.set_b(Some((Arc::clone(&extra), 0.0)));
        plot.toggle_b(vibe.clone());
        let other = LoadedLog::build(
            dflog::Log::from_bytes(&crate::model::testlog::b_extra_bytes()),
            "d.bin".into(),
        );
        plot.reload(&other);
        let s = &plot.selected[0];
        assert!(!s.only_b() && !s.data.xs.is_empty());
        assert_eq!(s.key, vibe);
        assert!(plot.is_selected(&vibe));
    }

    #[test]
    fn a_new_a_reads_a_swapped_series_by_the_field_it_shows() {
        let a = test_log();
        let b = b_log();
        let gyr = |instance| SeriesKey {
            type_name: "IMU".into(),
            field: "GyrX".into(),
            instance,
        };
        let mut plot = PlotPanel::default();
        plot.toggle(gyr(Some(0)), &a);
        plot.toggle(gyr(Some(1)), &a);
        plot.set_b(Some((Arc::clone(&b), -8.0)));
        plot.swap(&b, Arc::new(test_log()), 8.0);
        assert!(plot.is_selected(&gyr(None)));

        // another log of one IMU opened as A: the series showing its IMU
        // stays, keyed as that log keys it; the one B alone had goes with B
        plot.reload(&b_log());
        let keys: Vec<&SeriesKey> = plot.selected.iter().map(|s| &s.key).collect();
        assert_eq!(keys, [&gyr(None)]);
        assert!(plot.is_selected(&gyr(None)));
        assert!(plot.selected[0].data_b.is_none() && !plot.selected[0].only_b());

        // and a log of two IMUs then reads it as one no more, as before
        plot.reload(&a);
        assert!(plot.selected.is_empty());
    }

    #[test]
    fn a_swap_keeps_the_hidden_sides_and_the_moment_in_view() {
        let log = test_log();
        let settings = Settings::default();
        let mut harness = roll_plot(&log, &settings);
        let b = b_log();
        harness.state_mut().0.set_b(Some((Arc::clone(&b), -8.0)));
        harness.run();
        // A's roll hidden, the view on 2 s to 3 s, the playhead at 2.1 s
        let id = Id::new(PLOT_ID);
        let mut memory = PlotMemory::load(&harness.ctx, id).unwrap();
        memory.hidden_items.insert(line_id(&roll()));
        memory.store(&harness.ctx, id);
        PlotPanel::zoom_to(&harness.ctx, 2.0..=3.0);
        harness.state_mut().0.seek(2.1);
        harness.run();
        assert_eq!(harness.state().0.hidden, [line_id(&roll())]);

        // B the new A, and the test log B, 8 s on on its clock
        harness.state_mut().0.swap(&b, Arc::new(test_log()), 8.0);
        assert_eq!(harness.state().0.hidden, [line_id_b(&roll())]);
        harness.run();
        let memory = PlotMemory::load(&harness.ctx, id).unwrap();
        assert!(memory.hidden_items.contains(&line_id_b(&roll())));
        assert!(!memory.hidden_items.contains(&line_id(&roll())));
        assert_eq!(harness.state().0.hidden, [line_id_b(&roll())]);
        let view = transform(&harness).bounds().range_x();
        assert!(
            (view.start() - 10.0).abs() < 1e-6 && (view.end() - 11.0).abs() < 1e-6,
            "{view:?}"
        );
        let playhead = harness.state().0.playhead.unwrap();
        assert!((playhead - 10.1).abs() < 1e-9, "{playhead}");

        // swapped back with an offset that puts the moment past the new
        // A's end: the playhead is held at the end, and the view, moved
        // by the offset alone, pans onto it
        let end = *log.time_range().unwrap().end();
        harness.state_mut().0.swap(&log, Arc::clone(&b), 30.0);
        harness.run();
        let playhead = harness.state().0.playhead.unwrap();
        assert!((playhead - end).abs() < 1e-9, "{playhead} at the end {end}");
        let view = transform(&harness).bounds().range_x();
        assert!(view.contains(&playhead), "{view:?} holds {playhead}");
        // panned, not reset: still the second the zoom set up
        assert!((view.end() - view.start() - 1.0).abs() < 1e-6, "{view:?}");
    }

    #[test]
    fn a_swap_pans_a_zoomed_view_onto_a_playhead_the_clamp_held_back() {
        let a = test_log();
        let b = b_log();
        let end = *a.time_range().unwrap().end();
        let mut plot = PlotPanel::default();
        plot.toggle(roll(), &a);
        plot.set_b(Some((Arc::clone(&b), 0.0)));
        // within the new A's range, the view moves with the playhead
        plot.playhead = Some(1.0);
        plot.swap(&a, Arc::clone(&b), 0.5);
        assert_eq!((plot.playhead, plot.pending_seek), (Some(1.5), None));
        // held back to the new A's end, the view pans there
        plot.playhead = Some(end + 5.0);
        plot.swap(&a, b, 0.0);
        assert_eq!((plot.playhead, plot.pending_seek), (Some(end), Some(end)));
    }

    #[test]
    fn b_s_extent_takes_in_the_points_drawn_on_the_edges() {
        let mut s = pair("alt", [0.0, 0.0, 0.0], [0.0, 0.0, 0.0], "m");
        // B's samples at 9.9 s and 10.5 s, 0 and 1000, A's range to 10 s:
        // the line is drawn to 10 s, up to about 167
        s.data_b = Some(Series {
            xs: vec![9.9, 10.5],
            ys: vec![0.0, 1000.0],
        });
        s.fit_b_extent((0.0, 10.0));
        let (low, high) = s.extent_b.unwrap();
        assert!(low == 0.0 && (high - 1000.0 / 6.0).abs() < 1e-9, "{high}");
        // a sample on the edge is drawn as it is, and no point beyond it
        s.fit_b_extent((0.0, 10.5));
        assert_eq!(s.extent_b, Some((0.0, 1000.0)));
        s.fit_b_extent((0.0, 9.9));
        assert_eq!(s.extent_b, Some((0.0, 0.0)));
        // no sample inside: the line across the range alone
        s.fit_b_extent((10.0, 10.2));
        let (low, high) = s.extent_b.unwrap();
        assert!((low - 1000.0 / 6.0).abs() < 1e-9 && (high - 500.0).abs() < 1e-9);
        s.fit_b_extent((20.0, 30.0));
        assert_eq!(s.extent_b, None);
    }

    #[test]
    fn each_axis_covers_both_sides_as_drawn() {
        let a = pair("a", [0.0, 5.0, 10.0], [-10.0, 0.0, 20.0], "m");
        let mut b = pair("b", [100.0, 200.0, 300.0], [50.0, 100.0, 100.0], "m");
        b.axis = YAxis::Right;
        let both = [a, b];
        let map = AxisMap::new(&both).unwrap();
        assert_eq!((map.left, map.right), ((-10.0, 20.0), (50.0, 300.0)));

        // B's samples the whole-flight view clips off size no axis: B 1 s
        // later than A on A's clock, A's 0 s to 2 s are B's -1 s to 1 s
        let mut plot = PlotPanel {
            selected: both.into(),
            b: Some(PlotB {
                log: b_log(),
                offset: 1.0,
            }),
            ..PlotPanel::default()
        };
        plot.fit_b_extents();
        let map = AxisMap::new(&plot.selected).unwrap();
        assert_eq!((map.left, map.right), ((-10.0, 10.0), (50.0, 300.0)));
        assert_eq!(plot.selected[1].extent_b, Some((50.0, 100.0)));
        // hiding lines through the legend changes nothing of it, on either
        // side, so no other line moves
        let hidden: Vec<Id> = plot
            .selected
            .iter()
            .flat_map(|s| [line_id(&s.key), line_id_b(&s.key)])
            .collect();
        plot.hidden = hidden;
        plot.fit_b_extents();
        assert_eq!(plot.selected[1].extent_b, Some((50.0, 100.0)));
        assert_eq!(AxisMap::new(&plot.selected).unwrap(), map);
        // where A has no samples, all of B counts
        for s in &mut plot.selected {
            s.replace_data(Series::default());
        }
        plot.fit_b_extents();
        assert_eq!(plot.selected[0].extent_b, Some((-10.0, 20.0)));
    }

    #[test]
    fn the_legend_hides_b_line_alone_and_b_never_widens_the_whole_flight_view() {
        let log = test_log();
        let settings = Settings::default();
        let mut harness = roll_plot(&log, &settings);
        let alone = transform(&harness).bounds().range_x();

        // B lined up over A, with a sample past a gap far after A's end:
        // the view stays A's
        harness.state_mut().0.set_b(Some((b_log(), -8.0)));
        harness.run();
        assert_eq!(transform(&harness).bounds().range_x(), alone);
        // and B flying two hours after A draws nothing in it
        harness.state_mut().0.set_b(Some((b_log(), 7200.0)));
        harness.run();
        assert_eq!(transform(&harness).bounds().range_x(), alone);

        // B's entry in the legend hides B's line alone
        harness.state_mut().0.set_b(Some((b_log(), -8.0)));
        harness.run();
        let frame = *transform(&harness).frame();
        let entry = harness
            .get_all_by_label("ATT.Roll (deg) (B)")
            .map(|node| node.rect().center())
            .find(|center| frame.contains(*center))
            .expect("the legend names B's line");
        click_at(&mut harness, entry);
        assert_eq!(harness.state().0.hidden, [line_id_b(&roll())]);
        assert_eq!(harness.state().0.showing().count(), 1, "A's stays");
        harness.state_mut().0.seek(2.1);
        let r = &harness.state().0.readout[0];
        assert!(r.a.is_some() && r.b.is_none());
    }

    #[test]
    fn with_no_line_of_a_shown_the_whole_flight_view_fits_b() {
        let log = test_log();
        let settings = Settings::default();
        let mut harness = roll_plot(&log, &settings);
        harness.state_mut().0.set_b(Some((b_log(), -8.0)));
        let id = Id::new(PLOT_ID);
        let mut memory = PlotMemory::load(&harness.ctx, id).unwrap();
        memory.hidden_items.insert(line_id(&roll()));
        memory.store(&harness.ctx, id);
        harness.run();
        // B's roll, 10.0 s to 20 s on its clock, 2.0 s to 12 s on A's
        let view = transform(&harness).bounds().range_x();
        assert!(*view.start() < 2.0 && *view.end() > 12.0, "{view:?}");
        assert!(*view.end() < 13.0, "the view settles on B: {view:?}");
        harness.run();
        assert_eq!(transform(&harness).bounds().range_x(), view);
    }

    #[test]
    fn a_new_b_starts_with_its_lines_shown() {
        let log = test_log();
        let settings = Settings::default();
        let mut harness = roll_plot(&log, &settings);
        let first = b_log();
        harness
            .state_mut()
            .0
            .set_b(Some((Arc::clone(&first), -8.0)));
        harness.run();
        let id = Id::new(PLOT_ID);
        let mut memory = PlotMemory::load(&harness.ctx, id).unwrap();
        memory.hidden_items.insert(line_id_b(&roll()));
        memory.store(&harness.ctx, id);
        harness.run();
        assert_eq!(harness.state().0.hidden, [line_id_b(&roll())]);
        // a new offset for the same B keeps what the legend hid
        harness
            .state_mut()
            .0
            .set_b(Some((Arc::clone(&first), -7.0)));
        harness.run();
        assert_eq!(harness.state().0.hidden, [line_id_b(&roll())]);
        // another B, or the same log opened again, starts afresh, and so
        // does the readout
        harness.state_mut().0.set_b(Some((b_log(), -8.0)));
        harness.state_mut().0.seek(2.1);
        assert!(harness.state().0.readout[0].b.is_some());
        harness.run();
        assert!(harness.state().0.hidden.is_empty());
        let memory = PlotMemory::load(&harness.ctx, id).unwrap();
        assert!(memory.hidden_items.is_empty());

        // B's line hidden and its series taken off, the legend is empty
        // and forgets nothing: the line's id goes from the memory all the
        // same, so the series picked again starts with B's line shown
        let mut memory = PlotMemory::load(&harness.ctx, id).unwrap();
        memory.hidden_items.insert(line_id_b(&roll()));
        memory.store(&harness.ctx, id);
        harness.run();
        assert_eq!(harness.state().0.hidden, [line_id_b(&roll())]);
        harness.state_mut().0.remove(0);
        harness.run();
        let memory = PlotMemory::load(&harness.ctx, id).unwrap();
        assert!(memory.hidden_items.is_empty());
        harness.state_mut().0.toggle(roll(), &log);
        harness.state_mut().0.seek(2.1);
        harness.run();
        assert!(harness.state().0.hidden.is_empty());
        assert!(harness.state().0.readout[0].b.is_some());
    }

    #[test]
    fn a_chip_says_what_b_has_of_it() {
        let log = test_log();
        let settings = Settings::default();
        let mut harness = roll_plot(&log, &settings);
        for key in [
            SeriesKey {
                type_name: "ATT".into(),
                field: "Pitch".into(),
                instance: None,
            },
            SeriesKey {
                type_name: "IMU".into(),
                field: "GyrX".into(),
                instance: Some(0),
            },
        ] {
            harness.state_mut().0.toggle(key, &log);
        }
        harness.state_mut().0.set_b(Some((b_log(), -8.0)));
        harness.run();
        let frame = *transform(&harness).frame();
        let chip = |harness: &egui_kittest::Harness<'_, (PlotPanel, bool)>, title: &str| {
            harness
                .get_all_by_label(title)
                .map(|node| node.rect().center())
                .find(|center| !frame.contains(*center))
                .expect("the series has a chip above the plot")
        };
        for (title, hover) in [
            ("ATT.Pitch (deg)", "not in B"),
            ("IMU[0].GyrX (rad/s)", "B's IMU.GyrX"),
        ] {
            harness.hover_at(chip(&harness, title));
            harness.run_steps(20);
            harness.get_by_label(hover);
        }
        // the roll, which B has as A does, says nothing
        harness.hover_at(chip(&harness, "ATT.Roll (deg)"));
        harness.run_steps(20);
        assert!(harness.query_by_label("not in B").is_none());
        assert!(harness.query_by_label_contains("B's ").is_none());
    }

    #[test]
    fn mode_colors_differ_for_neighbors() {
        assert_ne!(mode_color(0), mode_color(1));
        assert_ne!(mode_color(5), mode_color(6));
        assert_eq!(mode_color(3), mode_color(3));
    }
}
