//! The plot: the chosen series against time, each on the left or the right
//! axis, over the flight mode bands, with a readout of every series at the
//! cursor, and the playhead that playback and every seek move.

use std::ops::RangeInclusive;

use egui::{Align2, Color32, Id, Pos2, Shape, Stroke, pos2};
use egui_plot::{
    AxisHints, Corner, GridMark, HPlacement, Legend, Line, Plot, PlotBounds, PlotGeometry,
    PlotItem, PlotItemBase, PlotMemory, PlotPoint, PlotPoints, PlotTransform, Span,
};

use crate::model::{LoadedLog, ModeChange, Series, SeriesKey, extent, nearest_index};
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
    /// `IMU[1].GyrX (rad/s)`
    pub title: String,
    pub axis: YAxis,
    pub color: Color32,
    pub data: Series,
    /// The lowest and highest value, found once; None for an empty series.
    extent: Option<(f64, f64)>,
}

impl Selected {
    fn new(key: SeriesKey, title: String, axis: YAxis, color: Color32, data: Series) -> Self {
        Self {
            key,
            title,
            axis,
            color,
            extent: extent(data.ys.iter().copied()),
            data,
        }
    }

    fn replace_data(&mut self, data: Series) {
        self.extent = extent(data.ys.iter().copied());
        self.data = data;
    }
}

/// One series' sample nearest the cursor.
#[derive(Debug, Clone, PartialEq)]
pub struct Readout {
    pub title: String,
    pub color: Color32,
    /// Seconds since boot of the sample.
    pub time: f64,
    pub value: f64,
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
}

/// The plot's id, fixed so its memory can be read back.
const PLOT_ID: &str = "aftermission_plot";

/// The id a series' line carries, which the legend hides it by.
fn line_id(key: &SeriesKey) -> Id {
    Id::new(key)
}

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
    #[must_use]
    pub fn is_selected(&self, key: &SeriesKey) -> bool {
        self.selected.iter().any(|s| &s.key == key)
    }

    /// Add `key` to the plot, or take it off when it is on. A series that
    /// cannot be extracted leaves its reason in `error`.
    pub fn toggle(&mut self, key: SeriesKey, log: &LoadedLog) {
        if let Some(index) = self.selected.iter().position(|s| s.key == key) {
            self.selected.remove(index);
            self.refresh_readout();
            return;
        }
        match log.series(&key) {
            Ok(data) => {
                let color = PALETTE[self.next_color % PALETTE.len()];
                self.next_color += 1;
                let title = title(log, &key);
                self.selected
                    .push(Selected::new(key, title, YAxis::Left, color, data));
                self.error = None;
                self.refresh_readout();
            }
            Err(message) => self.error = Some(message),
        }
    }

    pub fn remove(&mut self, index: usize) {
        if index < self.selected.len() {
            self.selected.remove(index);
            self.refresh_readout();
        }
    }

    /// The readout for the current series at the last cursor position.
    fn refresh_readout(&mut self) {
        self.readout = match self.cursor {
            Some(cursor) => readout_at(&self.selected, cursor, &self.hidden),
            None => Vec::new(),
        };
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
    /// The view resets to the whole of the new log.
    pub fn reload(&mut self, log: &LoadedLog) {
        self.selected.retain_mut(|s| match log.series(&s.key) {
            Ok(data) if log.has_series(&s.key) => {
                s.replace_data(data);
                s.title = title(log, &s.key);
                true
            }
            _ => false,
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
            .filter(|s| !self.hidden.contains(&line_id(&s.key)))
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

    /// The series chips, the plot and the cursor readout. The readout takes
    /// a panel at the bottom sized to its lines, and the plot the rest.
    /// While `playing`, the pointer leaves the cursor alone and a zoomed
    /// view pages to keep the playhead on screen.
    pub fn show(&mut self, ui: &mut egui::Ui, log: &LoadedLog, settings: &Settings, playing: bool) {
        self.series_bar(ui);
        egui::Panel::bottom("plot_readout")
            .resizable(false)
            .show_separator_line(false)
            .show(ui, |ui| self.readout_bar(ui, log, settings));
        let plot_height = ui.available_height().max(120.0);
        self.plot(ui, log, settings, plot_height, playing);
    }

    /// One chip per series: its color, name, axis and a way off the plot.
    fn series_bar(&mut self, ui: &mut egui::Ui) {
        if self.selected.is_empty() {
            ui.weak("Pick fields in the panel on the left to plot them.");
            return;
        }
        let mut remove = None;
        ui.horizontal_wrapped(|ui| {
            for (index, s) in self.selected.iter_mut().enumerate() {
                ui.colored_label(s.color, "\u{25A0}");
                ui.label(&s.title);
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
                if ui
                    .small_button("\u{2715}")
                    .on_hover_text("Remove")
                    .clicked()
                {
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
    ) {
        let map = AxisMap::new(&self.selected);
        // A reset rebuilds the plot's memory with nothing hidden and the
        // whole of the data in view.
        let reset = std::mem::take(&mut self.reset_view);
        self.sync_hidden(ui.ctx(), reset);
        let plot = new_plot(log, settings, map, height, playing, reset);

        let selected = &self.selected;
        let hidden = &self.hidden;
        let (playhead, cursor, last_hover) = (self.playhead, self.cursor, self.hover);
        let paged = self.paged.clone();
        let last_played = self.last_played;
        let seek = self.pending_seek.take();
        let range = log.time_range();
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
            // With the bounds following the data, every point counts, so a
            // reset zooms back out to the whole flight.
            let data_range = full_range(selected, hidden);
            let x_range = match (auto_x, &data_range) {
                (true, Some(range)) => range.clone(),
                _ => visible.clone(),
            };
            let width = plot_ui.transform().frame().width();

            if show_modes {
                mode_bands(plot_ui, log, &visible);
            }
            for s in selected {
                let to_plot = |y: f64| match (s.axis, map) {
                    (YAxis::Right, Some(m)) => m.to_left(y),
                    _ => y,
                };
                let points = decimate(&s.data.xs, &s.data.ys, &x_range, width, to_plot);
                plot_ui.line(
                    Line::new(s.title.clone(), PlotPoints::Owned(points))
                        .color(s.color)
                        .id(line_id(&s.key)),
                );
            }

            time_marks(plot_ui, cursor, playhead, playhead_stroke);
            Found {
                at,
                click,
                cursor,
                paged,
            }
        });

        // The mode names are painted over the frame rather than as plot
        // items, so they do not enter the bounds the plot fits its data in.
        if show_modes {
            mode_labels(ui, log, &response.transform);
        }
        let found = response.inner;
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

    /// Take the series hidden through the legend from the plot's memory,
    /// to leave them out of the readout too. On a `reset` frame the plot
    /// rebuilds its memory with nothing hidden, so the stored set no
    /// longer holds.
    fn sync_hidden(&mut self, ctx: &egui::Context, reset: bool) {
        let mut hidden: Vec<Id> = if reset {
            Vec::new()
        } else {
            PlotMemory::load(ctx, Id::new(PLOT_ID))
                .map(|memory| memory.hidden_items.into_iter().collect())
                .unwrap_or_default()
        };
        // sorted, so the same set always compares equal
        hidden.sort_unstable_by_key(Id::value);
        if hidden != self.hidden {
            self.hidden = hidden;
            self.refresh_readout();
        }
    }

    /// Time and value of every series at the cursor, or a hint.
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
            for r in &self.readout {
                ui.colored_label(r.color, "\u{25A0}");
                ui.label(format!(
                    "{} = {}{}",
                    r.title,
                    egui_plot::format_number(r.value, 4),
                    offset_note(r.time, cursor)
                ));
            }
        });
    }
}

/// Every shown series' sample nearest to board time `cursor`; the series
/// whose line ids are in `hidden` are left out.
fn readout_at(selected: &[Selected], cursor: f64, hidden: &[Id]) -> Vec<Readout> {
    selected
        .iter()
        .filter(|s| !hidden.contains(&line_id(&s.key)))
        .filter_map(|s| {
            let (time, value) = nearest(&s.data, cursor)?;
            Some(Readout {
                title: s.title.clone(),
                color: s.color,
                time,
                value,
            })
        })
        .collect()
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
    let unit = log
        .field_of(key)
        .and_then(|f| f.unit.clone())
        .filter(|u| !u.is_empty());
    match unit {
        Some(unit) => format!("{} ({unit})", key.label()),
        None => key.label(),
    }
}

/// The earliest to the latest sample over the series shown, those whose
/// line ids are in `hidden` left out, as the plot leaves them out of the
/// bounds it fits.
fn full_range(selected: &[Selected], hidden: &[Id]) -> Option<RangeInclusive<f64>> {
    let mut range: Option<(f64, f64)> = None;
    for s in selected
        .iter()
        .filter(|s| !hidden.contains(&line_id(&s.key)))
    {
        if let (Some(&first), Some(&last)) = (s.data.xs.first(), s.data.xs.last()) {
            range = Some(range.map_or((first, last), |(a, b)| (a.min(first), b.max(last))));
        }
    }
    range.map(|(a, b)| a..=b)
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
    fn new(selected: &[Selected]) -> Option<AxisMap> {
        let extent = |axis: YAxis| {
            let range = selected
                .iter()
                .filter(|s| s.axis == axis)
                .filter_map(|s| s.extent)
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

/// The points of a series within `range`, with one sample before and one
/// after it so the line runs off both edges, thinned to about two per pixel
/// of `width`: each bucket keeps its lowest and highest sample, in time
/// order, so no spike is lost. `xs` must be non-decreasing.
pub fn decimate(
    xs: &[f64],
    ys: &[f64],
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

    let mut out = Vec::with_capacity(2 * buckets);
    let mut i = lo;
    for bucket in 0..buckets {
        let last = bucket + 1 == buckets;
        let edge = x0 + span * (bucket + 1) as f64 / buckets as f64;
        let start = i;
        let (mut min_i, mut max_i) = (i, i);
        while i < hi && (last || xs[i] <= edge) {
            if ys[i] < ys[min_i] {
                min_i = i;
            }
            if ys[i] > ys[max_i] {
                max_i = i;
            }
            i += 1;
        }
        if i == start {
            continue;
        }
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
    let x_formatter = move |mark: GridMark, _: &RangeInclusive<f64>| match time_base {
        Some(base) => {
            timefmt::utc_time(base.wall_clock_unix_ms(mark.value * 1000.0), mark.step_size)
        }
        None => timefmt::boot_time(mark.value, mark.step_size),
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

/// Each visible band's mode name at its top left, a band that starts off
/// the left edge labeled at the edge, clipped to the plot frame.
fn mode_labels(ui: &egui::Ui, log: &LoadedLog, transform: &egui_plot::PlotTransform) {
    let frame = *transform.frame();
    let painter = ui.painter().with_clip_rect(frame);
    let font = egui::TextStyle::Small.resolve(ui.style());
    for (mode, _) in visible_bands(log, &transform.bounds().range_x()) {
        let x = transform
            .position_from_point_x(mode.time)
            .max(frame.left() + 2.0);
        painter.text(
            egui::pos2(x + 2.0, frame.top() + 2.0),
            Align2::LEFT_TOP,
            &mode.name,
            font.clone(),
            mode_color(mode.number),
        );
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
        let readout = readout_at(&both, 0.21, &[]);
        assert_eq!(readout.len(), 2);
        assert_eq!((readout[0].time, readout[0].value), (0.2, 3.0));
        assert_eq!((readout[1].time, readout[1].value), (0.0, 10.0));
        assert_eq!(offset_note(readout[0].time, 0.21), " (-0.010 s)");
        assert_eq!(offset_note(readout[1].time, 0.21), " (-0.210 s)");
        assert_eq!(offset_note(0.2, 0.2002), "");
        assert_eq!(offset_note(1.5, 1.0), " (+0.500 s)");

        // a series hidden through the legend leaves the readout
        let shown = readout_at(&both, 0.21, &[line_id(&key("b"))]);
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
        assert_eq!((plot.readout[1].time, plot.readout[1].value), (1.0, 20.0));

        // playing moves them too, and leaves the view and the events list
        // to paging and following
        plot.pending_seek = None;
        plot.follow = false;
        plot.play_to(0.21);
        assert_eq!((plot.playhead, plot.cursor), (Some(0.21), Some(0.21)));
        assert_eq!(plot.pending_seek, None);
        assert!(!plot.follow);
        assert_eq!((plot.readout[0].time, plot.readout[0].value), (0.2, 3.0));

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

    #[test]
    fn mode_colors_differ_for_neighbors() {
        assert_ne!(mode_color(0), mode_color(1));
        assert_ne!(mode_color(5), mode_color(6));
        assert_eq!(mode_color(3), mode_color(3));
    }
}
