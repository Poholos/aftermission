//! The plot: the chosen series against time, each on the left or the right
//! axis, over the flight mode bands, with a readout of every series at the
//! cursor.

use std::ops::RangeInclusive;

use egui::{Align2, Color32, Id};
use egui_plot::{
    AxisHints, Corner, GridMark, HPlacement, Legend, Line, Plot, PlotMemory, PlotPoint, PlotPoints,
    Span, VLine,
};

use crate::model::{LoadedLog, ModeChange, Series, SeriesKey};
use crate::settings::{Settings, TimeAxis};
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
            extent: extent(&data.ys),
            data,
        }
    }

    fn replace_data(&mut self, data: Series) {
        self.extent = extent(&data.ys);
        self.data = data;
    }
}

/// The lowest and highest of `values`, which are finite.
fn extent(values: &[f64]) -> Option<(f64, f64)> {
    values.iter().fold(None, |range: Option<(f64, f64)>, &y| {
        Some(range.map_or((y, y), |(a, b)| (a.min(y), b.max(y))))
    })
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

/// The plot's state: what is drawn and what the cursor is over.
#[derive(Debug, Default)]
pub struct PlotPanel {
    pub selected: Vec<Selected>,
    next_color: usize,
    /// The samples at the cursor, one per series. Kept when the pointer
    /// leaves the plot, so the values stay readable and the row keeps its
    /// height; None until the plot has been pointed at.
    pub readout: Vec<Readout>,
    /// Board time the pointer was last over.
    pub cursor: Option<f64>,
    /// Why the last series could not be added.
    pub error: Option<String>,
    /// Show the whole of the data on the next frame: set when the log
    /// changes under the plot, whose zoom would otherwise carry over.
    reset_view: bool,
    /// The series hidden through the legend, by the id their line carries,
    /// as of the last frame; they stay out of the readout.
    hidden: Vec<Id>,
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

    pub fn clear(&mut self) {
        self.selected.clear();
        self.readout.clear();
        self.cursor = None;
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
        self.cursor = None;
        self.reset_view = true;
    }

    /// Whether the next frame shows the whole of the data, for tests.
    #[cfg(test)]
    pub(crate) fn resets_view(&self) -> bool {
        self.reset_view
    }

    /// The series chips, the plot and the cursor readout. The readout takes
    /// a panel at the bottom sized to its lines, and the plot the rest.
    pub fn show(&mut self, ui: &mut egui::Ui, log: &LoadedLog, settings: &Settings) {
        self.series_bar(ui);
        egui::Panel::bottom("plot_readout")
            .resizable(false)
            .show_separator_line(false)
            .show(ui, |ui| self.readout_bar(ui, log, settings));
        let plot_height = ui.available_height().max(120.0);
        self.plot(ui, log, settings, plot_height);
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

    fn plot(&mut self, ui: &mut egui::Ui, log: &LoadedLog, settings: &Settings, height: f32) {
        let map = AxisMap::new(&self.selected);
        let time_base = match settings.time_axis {
            TimeAxis::Utc => log.time_base,
            TimeAxis::Boot => None,
        };
        let x_formatter = move |mark: GridMark, _: &RangeInclusive<f64>| match time_base {
            Some(base) => {
                timefmt::utc_time(base.wall_clock_unix_ms(mark.value * 1000.0), mark.step_size)
            }
            None => timefmt::boot_time(mark.value, mark.step_size),
        };

        // A series hidden through the legend leaves the readout too.
        let mut hidden: Vec<Id> = PlotMemory::load(ui.ctx(), Id::new(PLOT_ID))
            .map(|memory| memory.hidden_items.into_iter().collect())
            .unwrap_or_default();
        // sorted, so the same set always compares equal
        hidden.sort_unstable_by_key(Id::value);
        if hidden != self.hidden {
            self.hidden = hidden;
            self.refresh_readout();
        }

        // No label formatter: the readout under the plot stands in for the
        // hover tooltip, which egui_plot only shows when one is set.
        let mut plot = Plot::new(PLOT_ID)
            .id(Id::new(PLOT_ID))
            .height(height)
            .legend(Legend::default().position(Corner::LeftTop))
            .x_axis_formatter(x_formatter)
            .allow_boxed_zoom(true);
        if std::mem::take(&mut self.reset_view) {
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

        let selected = &self.selected;
        let hidden = &self.hidden;
        let last_cursor = self.cursor;
        let show_modes = settings.show_modes;
        let response = plot.show(ui, |plot_ui| {
            let bounds = plot_ui.plot_bounds();
            // With the bounds following the data, every point counts, so a
            // reset zooms back out to the whole flight.
            let x_range = if plot_ui.auto_bounds().x {
                full_range(selected).unwrap_or_else(|| bounds.range_x())
            } else {
                bounds.range_x()
            };
            let width = plot_ui.transform().frame().width();

            if show_modes {
                mode_bands(plot_ui, log, &bounds.range_x());
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

            // The cursor line marks the pointer, or where it last was, so it
            // agrees with the readout kept underneath.
            let pointer = plot_ui
                .pointer_coordinate()
                .filter(|_| plot_ui.response().hovered())
                .map(|p| p.x);
            if let Some(x) = pointer.or(last_cursor) {
                plot_ui.vline(VLine::new("", x).color(Color32::from_gray(160)).width(1.0));
            }
            pointer.map(|x| (x, readout_at(selected, x, hidden)))
        });

        // The mode names are painted over the frame rather than as plot
        // items, so they do not enter the bounds the plot fits its data in.
        if show_modes {
            mode_labels(ui, log, &response.transform);
        }
        // The last readout stays while the pointer is elsewhere.
        if let Some((cursor, readout)) = response.inner {
            self.cursor = Some(cursor);
            self.readout = readout;
        }
    }

    /// Time and value of every series at the cursor, or a hint.
    fn readout_bar(&self, ui: &mut egui::Ui, log: &LoadedLog, settings: &Settings) {
        ui.horizontal_wrapped(|ui| {
            let Some(cursor) = self.cursor else {
                ui.weak(
                    "Drag or scroll to pan, Ctrl+scroll or pinch to zoom, right-drag a box to \
                     zoom into it, double-click to see the whole log.",
                );
                return;
            };
            let time = match (settings.time_axis, log.time_base) {
                (TimeAxis::Utc, Some(base)) => {
                    let ms = base.wall_clock_unix_ms(cursor * 1000.0);
                    format!("{} {}", timefmt::utc_date(ms), timefmt::utc_time(ms, 0.001))
                }
                _ => timefmt::boot_time(cursor, 0.001),
            };
            ui.monospace(time);
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
        .type_named(&key.type_name)
        .and_then(|t| t.field(&key.field))
        .and_then(|f| f.unit.clone())
        .filter(|u| !u.is_empty());
    match unit {
        Some(unit) => format!("{} ({unit})", key.label()),
        None => key.label(),
    }
}

/// The earliest to the latest sample over every series.
fn full_range(selected: &[Selected]) -> Option<RangeInclusive<f64>> {
    let mut range: Option<(f64, f64)> = None;
    for s in selected {
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
    if series.xs.is_empty() {
        return None;
    }
    let after = series.xs.partition_point(|t| *t < x);
    let candidates = [
        after.checked_sub(1),
        (after < series.xs.len()).then_some(after),
    ];
    candidates
        .into_iter()
        .flatten()
        .min_by(|&a, &b| {
            let da = (series.xs[a] - x).abs();
            let db = (series.xs[b] - x).abs();
            da.total_cmp(&db)
        })
        .map(|i| (series.xs[i], series.ys[i]))
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
        assert_eq!(extent(&[]), None);
        assert_eq!(extent(&[3.0, -1.0, 2.0]), Some((-1.0, 3.0)));
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
        plot.clear();
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
    }

    #[test]
    fn mode_colors_differ_for_neighbors() {
        assert_ne!(mode_color(0), mode_color(1));
        assert_ne!(mode_color(5), mode_color(6));
        assert_eq!(mode_color(3), mode_color(3));
    }
}
