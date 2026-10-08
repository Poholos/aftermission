//! The map: the vehicle's track over OpenStreetMap tiles, with the plot's
//! cursor marked on it by an arrow along the vehicle's heading, and a
//! view that can follow it. Hovering the track reads a point's time; a
//! click seeks the plot to it. While a second log, B, is open, its track
//! draws under A's, lighter, with a marker of its own.

use dflog::time::TimeBase;
use egui::{Color32, Painter, Pos2, Rect, Stroke};
use walkers::sources::OpenStreetMap;
use walkers::{HttpOptions, HttpTiles, Map, MapMemory, Position, Projector, Tiles, lat_lon};

use crate::model::{Bounds, LoadedLog, Track, nearest_within};
use crate::settings::TimeAxis;
use crate::timefmt;
use crate::worker::Slot;

/// The map's state between frames.
pub struct MapPanel {
    /// The tile downloader, while tiles are wanted.
    tiles: Option<HttpTiles>,
    memory: MapMemory,
    /// Fit the view to the tracks on the next frame: set when a log opens,
    /// and by the Fit button.
    fit_pending: bool,
    /// A's track on screen for the view it was projected in, so a still
    /// map costs nothing per frame.
    projected: Option<ProjectedTrack>,
    /// B's, the same way.
    projected_b: Option<ProjectedTrack>,
    /// The zoom and center Follow last left the view at: a view that
    /// differs from it by the next frame was moved by hand.
    followed: Option<(f64, Position)>,
}

impl Default for MapPanel {
    fn default() -> Self {
        Self {
            tiles: None,
            memory: MapMemory::default(),
            fit_pending: true,
            projected: None,
            projected_b: None,
            followed: None,
        }
    }
}

/// A track's points on screen, with the log and the view they hold for.
struct ProjectedTrack {
    /// The log whose track it is, by its id: another log's track under
    /// an unchanged view is projected afresh.
    log: u64,
    zoom: f64,
    center: Position,
    rect: Rect,
    /// Track indices and their positions, thinned to the pixels.
    points: Vec<(usize, Pos2)>,
}

impl std::fmt::Debug for MapPanel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MapPanel")
            .field("tiles", &self.tiles.is_some())
            .field("memory", &self.memory)
            .field("fit_pending", &self.fit_pending)
            .field("projected", &self.projected.is_some())
            .field("projected_b", &self.projected_b.is_some())
            .field("followed", &self.followed)
            .finish()
    }
}

/// A's track line.
const TRACK_COLOR: Color32 = Color32::from_rgb(255, 140, 40);
/// B's, a lighter orange, as B's lines on the plot are lighter than A's.
const TRACK_COLOR_B: Color32 = Color32::from_rgb(255, 205, 150);
/// Drawn under a white ring, so it reads on light tiles as on the plain
/// dark background.
const HALO: Stroke = Stroke {
    width: 4.5,
    color: Color32::from_black_alpha(160),
};
/// How close to the track, in points, the pointer counts as on it.
const HIT_DISTANCE: f32 = 12.0;
/// The part of the view, on either side of its middle, the marker may
/// reach before Follow centers the map on it: short of the tenth each
/// side a fitted track keeps clear, so the whole track in view stays.
const FOLLOW_REACH: f32 = 0.45;

/// A track on the map this frame: whose it is, and the clock its hover
/// labels read through.
#[derive(Clone, Copy)]
struct Shown<'a> {
    slot: Slot,
    log: &'a LoadedLog,
    clock: Option<TimeBase>,
}

impl Shown<'_> {
    fn track(&self) -> &Track {
        &self.log.track
    }
}

impl MapPanel {
    /// A new log is on: show the whole of its track.
    pub fn reload(&mut self) {
        self.fit_pending = true;
        self.projected = None;
        self.projected_b = None;
    }

    /// Fit the view to the tracks on the next frame, as B's arrival and
    /// its closing do.
    pub fn refit(&mut self) {
        self.fit_pending = true;
    }

    /// Whether the next frame fits the view to the tracks; for tests.
    #[cfg(test)]
    pub(crate) fn fits(&self) -> bool {
        self.fit_pending
    }

    /// The map of `log`'s track with the point nearest `cursor` marked,
    /// by an arrow along the vehicle's heading where the log gives one,
    /// and with `b` open, B's track under it, moved onto A's clock by its
    /// offset, with a ring at B's point at the cursor, where B's track
    /// reaches. Hover labels read each log's time through its own clock,
    /// as `axis` chooses. With `online`, tiles are downloaded from
    /// OpenStreetMap; without, the tracks draw on a plain background. With
    /// `follow`, the map centers on the marker, A's or B's when only B has
    /// a track, when it nears the view's edge, and a drag, scroll or zoom
    /// of the map by hand turns `follow` off. Returns the time of a track
    /// point clicked, on its own log's clock, with whose it is, to seek to.
    #[expect(
        clippy::too_many_arguments,
        reason = "what the map shows each frame, each from another owner"
    )]
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        log: &LoadedLog,
        b: Option<(&LoadedLog, f64)>,
        cursor: Option<f64>,
        axis: TimeAxis,
        online: bool,
        follow: &mut bool,
    ) -> Option<(Slot, f64)> {
        let a = Shown {
            slot: Slot::A,
            log,
            clock: log.wall_clock(axis),
        };
        let shown_b = b.map(|(b, _)| Shown {
            slot: Slot::B,
            log: b,
            clock: b.wall_clock(axis),
        });
        let track = a.track();
        let track_b = b.map(|(b, _)| &b.track).filter(|t| !t.is_empty());
        let bounds = self.header(ui, track, track_b, b.is_some())?;
        self.sync_tiles(ui.ctx(), online);

        let center = center(bounds);
        let size = ui.available_rect_before_wrap().size();
        if std::mem::take(&mut self.fit_pending) {
            self.memory.center_at(center);
            // fit_zoom stays within the levels MapMemory accepts, so this
            // cannot fail
            let _ = self.memory.set_zoom(fit_zoom(bounds, size));
            // the app's move, not the hand's
            self.followed = self.followed.map(|_| self.view(center));
        }

        let marker = cursor.and_then(|t| track.nearest(t)).map(|i| Marker {
            index: i,
            heading: cursor.and_then(|t| log.heading_at(t)),
        });
        // B's point at the cursor, on B's clock, only where B's track is
        let marker_b = b
            .zip(track_b)
            .and_then(|((_, offset), t)| within(t, cursor? - offset));
        let attribution = self.tiles.as_ref().map(Tiles::attribution);
        let tiles = self.tiles.as_mut().map(|t| t as &mut dyn Tiles);
        let (projected, projected_b) = (&mut self.projected, &mut self.projected_b);
        let response = Map::new(tiles, &mut self.memory, center).show(
            ui,
            |ui, response, projector, memory| {
                let view = (
                    memory.zoom(),
                    memory.detached().unwrap_or(center),
                    response.rect,
                );
                let painter = ui.painter().with_clip_rect(response.rect);
                let points_b = shown_b.filter(|_| track_b.is_some()).map(|b| {
                    (
                        b,
                        project(projected_b, view, b.log.id, b.track(), projector),
                    )
                });
                let points = project(projected, view, log.id, track, projector);
                if let Some((b, points)) = &points_b {
                    draw_line(&painter, points, TRACK_COLOR_B, false);
                    if let Some(i) = marker_b {
                        ring(&painter, screen(projector, b.track(), i), TRACK_COLOR_B);
                    }
                }
                draw_line(&painter, points, TRACK_COLOR, true);
                if let Some(m) = marker {
                    draw_marker(&painter, screen(projector, track, m.index), m.heading);
                }
                // A's track first under the pointer, then B's
                let mut candidates = vec![(a, points)];
                candidates.extend(points_b);
                let seek = hover(ui, response, &painter, &candidates);
                // Follow keeps A's marker in view, or B's without A's track
                let followed = match (marker, marker_b) {
                    (Some(m), _) => Some((track, m.index)),
                    (None, Some(i)) if track.is_empty() => track_b.map(|t| (t, i)),
                    _ => None,
                };
                let marker_at = followed.map(|(t, i)| (position(t, i), screen(projector, t, i)));
                (seek, marker_at)
            },
        );
        let (seek, marker_at) = response.inner;
        let rect = response.response.rect;
        if *follow {
            if self.follow(center, rect, marker_at, follow) {
                // drawn where it was: the next frame shows it centered
                ui.ctx().request_repaint();
            }
        } else {
            self.followed = None;
        }
        if let Some(attribution) = attribution {
            credit(ui, rect, &attribution);
        }
        seek
    }

    /// The line above the map, what each track came from and the Fit
    /// button, and the bounds that take in the tracks; or, with none to
    /// draw, the reason why, and None.
    fn header(
        &mut self,
        ui: &mut egui::Ui,
        track: &Track,
        track_b: Option<&Track>,
        b_open: bool,
    ) -> Option<Bounds> {
        let Some(bounds) = union(track.bounds(), track_b.and_then(Track::bounds)) else {
            let text = if b_open {
                "No position in either log: no POS records, and no GPS records with a fix."
            } else {
                "No position in this log: no POS records, and no GPS records with a fix."
            };
            ui.centered_and_justified(|ui| {
                ui.weak(text);
            });
            return None;
        };
        ui.horizontal(|ui| {
            if !track.is_empty() {
                ui.weak(format!(
                    "Track from {} ({} points)",
                    track.source,
                    track.len()
                ));
            }
            if let Some(t) = track_b {
                ui.weak(format!("B from {} ({} points)", t.source, t.len()));
            }
            if ui
                .small_button("Fit")
                .on_hover_text("Show the whole of the tracks")
                .clicked()
            {
                self.fit_pending = true;
            }
        });
        Some(bounds)
    }

    /// The zoom and center the map shows.
    fn view(&self, center: Position) -> (f64, Position) {
        (self.memory.zoom(), self.memory.detached().unwrap_or(center))
    }

    /// Keep the marker in view: center the map on it once it is past
    /// [`FOLLOW_REACH`] of the view in `rect` from the middle, keeping the
    /// zoom. A view that moved since Follow last left it was moved by
    /// hand, and turns `follow` off instead. `marker` is where the marker
    /// is, on the map and on screen. Whether the map moved.
    fn follow(
        &mut self,
        center: Position,
        rect: Rect,
        marker: Option<(Position, Pos2)>,
        follow: &mut bool,
    ) -> bool {
        if self
            .followed
            .is_some_and(|last| !same_view(last, self.view(center)))
        {
            *follow = false;
            self.followed = None;
            return false;
        }
        let reach = rect.shrink2(rect.size() * (0.5 - FOLLOW_REACH));
        let away = marker.filter(|(_, at)| !reach.contains(*at));
        if let Some((position, _)) = away {
            self.memory.center_at(position);
        }
        self.followed = Some(self.view(center));
        away.is_some()
    }

    /// How many tiles are downloading, or None with tiles off; for tests.
    #[cfg(test)]
    pub(crate) fn tiles_in_progress(&self) -> Option<usize> {
        self.tiles.as_ref().map(|tiles| tiles.stats().in_progress)
    }

    /// Start or stop downloading tiles, as `online` asks.
    fn sync_tiles(&mut self, ctx: &egui::Context, online: bool) {
        match (&self.tiles, online) {
            (None, true) => {
                self.tiles = Some(HttpTiles::with_options(
                    OpenStreetMap,
                    http_options(),
                    ctx.clone(),
                ));
            }
            (Some(_), false) => self.tiles = None,
            _ => {}
        }
    }
}

/// OpenStreetMap's tile usage policy asks for an identifying user agent
/// and a cache that honors the tiles' expiry; the cache sits beside the
/// app's settings.
#[cfg(not(target_arch = "wasm32"))]
fn http_options() -> HttpOptions {
    HttpOptions {
        cache: eframe::storage_dir(crate::APP_ID).map(|dir| dir.join("tiles")),
        user_agent: Some(walkers::HeaderValue::from_static(concat!(
            "aftermission/",
            env!("CARGO_PKG_VERSION"),
            " (+https://github.com/Poholos/aftermission)"
        ))),
        ..HttpOptions::default()
    }
}

/// In the browser, the browser does both: it caches the tiles by their
/// expiry, and it sends its own user agent, which walkers asks us not to
/// replace there.
#[cfg(target_arch = "wasm32")]
fn http_options() -> HttpOptions {
    HttpOptions::default()
}

/// The tiles' attribution in the bottom right corner of the map in
/// `rect`, a link to its page.
fn credit(ui: &mut egui::Ui, rect: Rect, attribution: &walkers::sources::Attribution) {
    let text = format!("\u{a9} {}", attribution.text);
    let size = egui::vec2(11.0 + 6.0 * text.len() as f32, 16.0);
    let rect = egui::Rect::from_min_size(rect.right_bottom() - size - egui::vec2(4.0, 4.0), size);
    ui.painter()
        .rect_filled(rect, 3.0, ui.visuals().window_fill.gamma_multiply(0.85));
    ui.put(
        rect,
        egui::Hyperlink::from_label_and_url(egui::RichText::new(text).small(), attribution.url),
    );
}

/// The middle of `bounds`.
fn center(bounds: Bounds) -> Position {
    lat_lon(
        f64::midpoint(bounds.south, bounds.north),
        f64::midpoint(bounds.west, bounds.east),
    )
}

/// The bounds that take in both, or either alone.
fn union(a: Option<Bounds>, b: Option<Bounds>) -> Option<Bounds> {
    match (a, b) {
        (Some(a), Some(b)) => Some(Bounds {
            south: a.south.min(b.south),
            west: a.west.min(b.west),
            north: a.north.max(b.north),
            east: a.east.max(b.east),
        }),
        (one, other) => one.or(other),
    }
}

/// The point of `track` nearest `time`, when `time` lies within the
/// track's times.
fn within(track: &Track, time: f64) -> Option<usize> {
    nearest_within(&track.times, time)
}

/// The zoom level that fits `bounds` into a view of `size` points with a
/// margin around it, between a continent's and a backyard's; a track of
/// one point gets the close look.
fn fit_zoom(bounds: Bounds, size: egui::Vec2) -> f64 {
    // at zoom 0 the world is one 256 px tile, and each level doubles it
    let corner = |lat: f64, lon: f64| walkers::mercator::project(lat_lon(lat, lon), 0.0);
    let a = corner(bounds.north, bounds.west);
    let b = corner(bounds.south, bounds.east);
    let fit = |px: f32, span: f64| {
        if span > 0.0 {
            (f64::from(px) * 0.8 / span).log2()
        } else {
            f64::INFINITY
        }
    };
    fit(size.x, (b.x() - a.x()).abs())
        .min(fit(size.y, (b.y() - a.y()).abs()))
        .clamp(2.0, 18.0)
}

/// `track`'s points on screen for the view `(zoom, center, rect)`,
/// reprojected only when the view or the log, `log`, has changed since
/// the last frame.
fn project<'a>(
    cache: &'a mut Option<ProjectedTrack>,
    (zoom, center, rect): (f64, Position, Rect),
    log: u64,
    track: &Track,
    projector: &Projector,
) -> &'a [(usize, Pos2)] {
    let stale = cache.as_ref().is_none_or(|c| {
        c.log != log || c.zoom.to_bits() != zoom.to_bits() || c.center != center || c.rect != rect
    });
    if stale {
        *cache = None;
    }
    let held = cache.get_or_insert_with(|| ProjectedTrack {
        log,
        zoom,
        center,
        rect,
        points: thin(track, |i| screen(projector, track, i)),
    });
    &held.points
}

/// Track point `i` on the map.
fn position(track: &Track, i: usize) -> Position {
    lat_lon(track.lats[i], track.lons[i])
}

/// Track point `i` on screen.
fn screen(projector: &Projector, track: &Track, i: usize) -> Pos2 {
    projector.project(position(track, i)).to_pos2()
}

/// The track point the cursor is at, and the vehicle's heading there.
#[derive(Debug, Clone, Copy)]
struct Marker {
    index: usize,
    /// Degrees clockwise from north.
    heading: Option<f64>,
}

/// A track as a line through `points` in `color`, with its start and end
/// marked when `ends`.
fn draw_line(painter: &Painter, points: &[(usize, Pos2)], color: Color32, ends: bool) {
    painter.add(egui::Shape::line(
        points.iter().map(|&(_, p)| p).collect(),
        Stroke::new(2.5, color),
    ));
    if ends && let (Some(&(_, first)), Some(&(_, last))) = (points.first(), points.last()) {
        painter.circle_filled(first, 4.0, Color32::from_rgb(70, 200, 90));
        painter.circle_filled(last, 4.0, Color32::from_rgb(235, 70, 70));
    }
}

/// The marker at `at`: an arrow along `heading`, or a white ring without
/// one.
fn draw_marker(painter: &Painter, at: Pos2, heading: Option<f64>) {
    let Some(heading) = heading else {
        ring(painter, at, Color32::WHITE);
        return;
    };
    // the notch makes it concave: a halo around the outline, and the two
    // halves filled
    let [tip, left, notch, right] = arrow(at, heading);
    painter.add(egui::Shape::closed_line(
        vec![tip, left, notch, right],
        HALO,
    ));
    for half in [[tip, left, notch], [tip, notch, right]] {
        painter.add(egui::Shape::convex_polygon(
            half.to_vec(),
            Color32::WHITE,
            Stroke::NONE,
        ));
    }
}

/// A ring at `at` in `color`, over a halo.
fn ring(painter: &Painter, at: Pos2, color: Color32) {
    painter.circle_stroke(at, 6.0, HALO);
    painter.circle(at, 6.0, color.gamma_multiply(0.25), Stroke::new(2.0, color));
}

/// The point under the pointer, of the first of `tracks` with one within
/// reach, marked and labeled with its time on its own log's clock and its
/// altitude, a point of B's named B's. Returns whose it is and its time,
/// on its log's clock, when it is clicked.
fn hover(
    ui: &egui::Ui,
    response: &egui::Response,
    painter: &Painter,
    tracks: &[(Shown<'_>, &[(usize, Pos2)])],
) -> Option<(Slot, f64)> {
    let hovered = response.hover_pos()?;
    let (shown, i, at) = tracks.iter().find_map(|(shown, points)| {
        points
            .iter()
            .map(|&(i, p)| (i, p, p.distance(hovered)))
            .min_by(|a, b| a.2.total_cmp(&b.2))
            .filter(|hit| hit.2 <= HIT_DISTANCE)
            .map(|(i, p, _)| (*shown, i, p))
    })?;
    let track = shown.track();
    painter.circle_stroke(at, 5.0, HALO);
    painter.circle_stroke(at, 5.0, Stroke::new(1.5, Color32::WHITE));
    let text = point_label(shown, i);
    let font = egui::TextStyle::Small.resolve(ui.style());
    let galley = painter.layout_no_wrap(text, font, ui.visuals().text_color());
    let pos = at + egui::vec2(10.0, -10.0 - galley.size().y);
    painter.rect_filled(
        egui::Rect::from_min_size(pos, galley.size()).expand(3.0),
        3.0,
        ui.visuals().window_fill.gamma_multiply(0.9),
    );
    painter.galley(pos, galley, ui.visuals().text_color());
    response.clicked().then(|| (shown.slot, track.times[i]))
}

/// What a hovered point of `shown`'s track says: its time on its own
/// log's clock and its altitude, a point of B's named B's.
fn point_label(shown: Shown<'_>, i: usize) -> String {
    let track = shown.track();
    let time = timefmt::stamp(shown.clock, track.times[i]);
    let whose = shown.slot.prefix();
    match track.alts[i] {
        alt if alt.is_finite() => format!("{whose}{time}, {alt:.1} m"),
        _ => format!("{whose}{time}"),
    }
}

/// Whether two views, zoom and center, are the same but for the rounding
/// walkers' projection leaves in a center: a hand's move shifts one by a
/// pixel at least, some 1e-6 of a degree at the closest zoom.
fn same_view((zoom_a, a): (f64, Position), (zoom_b, b): (f64, Position)) -> bool {
    const DEGREES: f64 = 1e-9;
    (zoom_a - zoom_b).abs() < 1e-9
        && (a.y() - b.y()).abs() < DEGREES
        && (a.x() - b.x()).abs() < DEGREES
}

/// An arrow at `at` pointing `heading` degrees clockwise from north, up
/// the screen: its tip ahead, its two back corners behind and to the
/// sides, and a notch between them, so it reads as a direction rather
/// than a triangle. North is up on the map's projection.
fn arrow(at: Pos2, heading: f64) -> [Pos2; 4] {
    let radians = heading.to_radians() as f32;
    let ahead = egui::vec2(radians.sin(), -radians.cos());
    let side = egui::vec2(-ahead.y, ahead.x);
    [
        at + ahead * 11.0,
        at - ahead * 7.0 + side * 7.0,
        at - ahead * 3.0,
        at - ahead * 7.0 - side * 7.0,
    ]
}

/// The track's points on screen, with their indices, consecutive points
/// closer than a pixel dropped so a long track costs what it shows; the
/// last point is always kept.
fn thin(track: &Track, screen: impl Fn(usize) -> Pos2) -> Vec<(usize, Pos2)> {
    let mut out = Vec::new();
    let mut last: Option<Pos2> = None;
    for i in 0..track.len() {
        let p = screen(i);
        let apart = last.is_none_or(|l| l.distance_sq(p) >= 1.0);
        if apart || i + 1 == track.len() {
            out.push((i, p));
            last = Some(p);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::kittest::Queryable as _;

    #[test]
    fn the_view_fits_the_track_and_thins_it_to_pixels() {
        let size = egui::vec2(800.0, 600.0);
        let point = Bounds {
            south: 47.0,
            west: 8.0,
            north: 47.0,
            east: 8.0,
        };
        assert!((fit_zoom(point, size) - 18.0).abs() < 1e-12);
        let field = Bounds {
            south: 47.0,
            west: 8.0,
            north: 47.005,
            east: 8.007,
        };
        let region = Bounds {
            south: 46.0,
            west: 7.0,
            north: 48.0,
            east: 10.0,
        };
        let world = Bounds {
            south: -80.0,
            west: -180.0,
            north: 80.0,
            east: 180.0,
        };
        let (f, r, w) = (
            fit_zoom(field, size),
            fit_zoom(region, size),
            fit_zoom(world, size),
        );
        assert!(f > r && r > w, "{f} {r} {w}");
        assert!((w - 2.0).abs() < 1e-12);
        assert!((14.0..=17.0).contains(&f), "{f}");
        // the smaller view zooms out
        assert!(fit_zoom(field, egui::vec2(200.0, 150.0)) < f);

        let c = center(field);
        assert!((c.y() - 47.0025).abs() < 1e-9 && (c.x() - 8.0035).abs() < 1e-9);

        let track = Track {
            times: (0..100).map(f64::from).collect(),
            lats: vec![0.0; 100],
            lons: vec![0.0; 100],
            alts: vec![0.0; 100],
            source: "POS",
            bounds: None,
        };
        // ten points to a pixel: one of each ten is kept, and the last
        let thinned = thin(&track, |i| Pos2::new(i as f32 * 0.1, 0.0));
        assert_eq!(thinned.len(), 11);
        assert_eq!(thinned[0].0, 0);
        assert_eq!(thinned[1].0, 10);
        assert_eq!(thinned[10].0, 99);
        let spread = thin(&track, |i| Pos2::new(i as f32 * 5.0, 0.0));
        assert_eq!(spread.len(), 100);
    }

    #[test]
    fn the_arrow_points_along_the_heading() {
        let at = Pos2::new(100.0, 100.0);
        let close = |a: Pos2, b: Pos2| a.distance(b) < 1e-3;
        // north is up the screen, east to the right
        let [tip, left, notch, right] = arrow(at, 0.0);
        assert!(close(tip, Pos2::new(100.0, 89.0)), "{tip:?}");
        assert!(left.y > at.y && right.y > at.y && notch.y > at.y);
        let [tip, ..] = arrow(at, 90.0);
        assert!(close(tip, Pos2::new(111.0, 100.0)), "{tip:?}");
        let [tip, ..] = arrow(at, 225.0);
        assert!(tip.x < at.x && tip.y > at.y, "south-west: {tip:?}");
    }

    /// The map panel with what it is given each frame.
    struct Bench {
        map: MapPanel,
        cursor: Option<f64>,
        follow: bool,
    }

    #[test]
    fn follow_keeps_the_marker_in_view_until_the_map_is_moved_by_hand() {
        let log = LoadedLog::build(
            dflog::Log::from_bytes(&crate::model::testlog::bytes()),
            "test.bin".into(),
        );
        let track = &log.track;
        let point = |i: usize| lat_lon(track.lats[i], track.lons[i]);
        let at = |harness: &egui_kittest::Harness<'_, Bench>, i: usize| {
            let map = &harness.state().map;
            let center = map.memory.detached().expect("the map has been placed");
            same_view((map.memory.zoom(), center), (map.memory.zoom(), point(i)))
        };
        let bench = Bench {
            map: MapPanel::default(),
            cursor: None,
            follow: true,
        };
        let mut harness = egui_kittest::Harness::builder()
            .with_size(egui::vec2(600.0, 400.0))
            .build_ui_state(
                |ui, bench: &mut Bench| {
                    bench.map.show(
                        ui,
                        &log,
                        None,
                        bench.cursor,
                        TimeAxis::Boot,
                        false,
                        &mut bench.follow,
                    );
                },
                bench,
            );
        harness.run();
        // the whole track fitted in view, its ends inside Follow's reach:
        // the cursor on either end moves nothing
        let fitted = harness.state().map.memory.detached();
        for end in [2.05, 2.25] {
            harness.state_mut().cursor = Some(end);
            harness.run();
            assert_eq!(harness.state().map.memory.detached(), fitted);
            assert!(harness.state().follow);
        }

        // zoomed in on the first point, as by hand: Follow lets go
        let memory = &mut harness.state_mut().map.memory;
        memory.center_at(point(0));
        memory.set_zoom(21.0).unwrap();
        harness.run();
        assert!(!harness.state().follow);
        assert!(at(&harness, 0));

        // on again, with the cursor on the last point, now off screen: the
        // map centers on it and keeps the zoom
        harness.state_mut().follow = true;
        harness.run();
        assert!(at(&harness, 2));
        assert!((harness.state().map.memory.zoom() - 21.0).abs() < 1e-9);
        assert!(harness.state().follow);
        // the marker still in view: nothing moves, and Follow holds
        harness.run();
        assert!(at(&harness, 2));
        assert!(harness.state().follow);

        // a drag of the map, as a changed center, turns it off
        harness.state_mut().map.memory.center_at(point(1));
        harness.run();
        assert!(!harness.state().follow);
        harness.state_mut().cursor = Some(2.05);
        harness.run();
        assert!(at(&harness, 1));

        // and Fit, the app's own move, leaves it on
        harness.state_mut().follow = true;
        harness.run();
        harness.state_mut().map.fit_pending = true;
        harness.run();
        assert!(harness.state().follow);
    }

    /// A log whose track is `POS` points at `(seconds, latitude,
    /// longitude)`.
    fn track_log(points: &[(f64, f64, f64)]) -> LoadedLog {
        use dflog::access::Value;
        let mut w = dflog::write::LogWriter::new();
        w.define(
            9,
            "POS",
            "QLLfff",
            &["TimeUS", "Lat", "Lng", "Alt", "RelHomeAlt", "RelOriginAlt"],
        )
        .unwrap();
        w.define(2, "EV", "QB", &["TimeUS", "Id"]).unwrap();
        w.record("EV", &[Value::U64(500_000), Value::U64(25)])
            .unwrap();
        for &(t, lat, lon) in points {
            w.record(
                "POS",
                &[
                    Value::U64((t * 1e6).round() as u64),
                    Value::F64(lat),
                    Value::F64(lon),
                    Value::F64(500.0),
                    Value::F64(10.0),
                    Value::F64(10.0),
                ],
            )
            .unwrap();
        }
        LoadedLog::build(dflog::Log::from_bytes(&w.into_bytes()), "t.bin".into())
    }

    /// A's three points going north-east from 47 N 8 E, a second apart
    /// from 1 s.
    fn a_log() -> LoadedLog {
        track_log(&[(1.0, 47.0, 8.0), (2.0, 47.001, 8.001), (3.0, 47.002, 8.002)])
    }

    /// B's three points 0.01 degrees north of A's, from 10 s; its first
    /// on A's last.
    fn b_log() -> LoadedLog {
        track_log(&[
            (10.0, 47.002, 8.002),
            (11.0, 47.011, 8.001),
            (12.0, 47.012, 8.002),
        ])
    }

    /// The map with a pair on it, and what it answered.
    struct Pair {
        map: MapPanel,
        a: LoadedLog,
        b: Option<(LoadedLog, f64)>,
        cursor: Option<f64>,
        follow: bool,
        seek: Option<(Slot, f64)>,
    }

    fn pair_harness(pair: Pair) -> egui_kittest::Harness<'static, Pair> {
        let mut harness = egui_kittest::Harness::builder()
            .with_size(egui::vec2(600.0, 400.0))
            .build_ui_state(
                |ui, pair: &mut Pair| {
                    let b = pair.b.as_ref().map(|(b, offset)| (b, *offset));
                    let seek = pair.map.show(
                        ui,
                        &pair.a,
                        b,
                        pair.cursor,
                        TimeAxis::Boot,
                        false,
                        &mut pair.follow,
                    );
                    if seek.is_some() {
                        pair.seek = seek;
                    }
                },
                pair,
            );
        harness.run();
        harness
    }

    /// Press and let go on track point `i` of the kept points, A's or
    /// B's, as it was last drawn.
    fn click_point(harness: &mut egui_kittest::Harness<'_, Pair>, slot: Slot, i: usize) {
        let map = &harness.state().map;
        let kept = match slot {
            Slot::A => &map.projected,
            Slot::B => &map.projected_b,
        };
        let at = kept
            .as_ref()
            .expect("the track has been drawn")
            .points
            .iter()
            .find(|&&(index, _)| index == i)
            .expect("the point is among those kept")
            .1;
        harness.hover_at(at);
        harness.run();
        for pressed in [true, false] {
            harness.event(egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            });
        }
        harness.run();
    }

    #[test]
    fn two_tracks_share_the_view_and_either_draws_alone() {
        let pair = Pair {
            map: MapPanel::default(),
            a: a_log(),
            b: Some((b_log(), -8.0)),
            cursor: None,
            follow: false,
            seek: None,
        };
        let mut harness = pair_harness(pair);
        harness.get_by_label("Track from POS (3 points)");
        harness.get_by_label("B from POS (3 points)");
        // Fit, and the first view, take in both
        let fitted = harness.state().map.memory.detached().unwrap();
        assert!((fitted.y() - 47.006).abs() < 1e-9, "{fitted:?}");
        let zoom = harness.state().map.memory.zoom();
        assert!(
            zoom < fit_zoom(a_log().track.bounds().unwrap(), egui::vec2(600.0, 380.0)),
            "{zoom}"
        );

        // B's alone, where A has no track
        harness.state_mut().a = track_log(&[]);
        harness.state_mut().map.reload();
        harness.run();
        assert!(
            harness
                .query_by_label("Track from POS (3 points)")
                .is_none()
        );
        harness.get_by_label("B from POS (3 points)");
        // and neither
        harness.state_mut().b = Some((track_log(&[]), 0.0));
        harness.run();
        harness.get_by_label_contains("No position in either log");
        harness.state_mut().b = None;
        harness.run();
        harness.get_by_label_contains("No position in this log");
    }

    #[test]
    fn a_click_on_either_track_seeks_by_its_own_clock_and_a_first() {
        let pair = Pair {
            map: MapPanel::default(),
            a: a_log(),
            b: Some((b_log(), -8.0)),
            cursor: None,
            follow: false,
            seek: None,
        };
        let mut harness = pair_harness(pair);
        // a point of B's alone: its time on B's clock, B's
        click_point(&mut harness, Slot::B, 2);
        assert_eq!(harness.state().seek, Some((Slot::B, 12.0)));
        // A's last point, where B's first lies too: A's comes first
        click_point(&mut harness, Slot::A, 2);
        assert_eq!(harness.state().seek, Some((Slot::A, 3.0)));

        // a second B inside A's bounds, under the same view: its points
        // are projected afresh and a click reads its own times
        let other = track_log(&[(20.0, 47.0005, 8.0015), (21.0, 47.0015, 8.0005)]);
        let id = other.id;
        harness.state_mut().b = Some((other, 0.0));
        harness.run();
        assert_eq!(harness.state().map.projected_b.as_ref().unwrap().log, id);
        click_point(&mut harness, Slot::B, 1);
        assert_eq!(harness.state().seek, Some((Slot::B, 21.0)));
    }

    #[test]
    fn a_hovered_point_reads_on_its_own_log_clock() {
        use crate::demo::{self, Flight};

        fn shown(slot: Slot, log: &LoadedLog) -> Shown<'_> {
            Shown {
                slot,
                log,
                clock: log.wall_clock(TimeAxis::Utc),
            }
        }

        let flight = |f: Flight| {
            LoadedLog::build(
                dflog::Log::from_source(demo::bytes(f).into()),
                f.name().into(),
            )
        };
        let (a, b) = (flight(Flight::A), flight(Flight::B));
        // each log's first point after its origin, at 22 s since its boot:
        // A's at 14:30:22, B's two hours later on B's own clock
        assert!(point_label(shown(Slot::A, &a), 0).starts_with("2026-05-14 14:30:22.0"));
        assert!(point_label(shown(Slot::B, &b), 0).starts_with("B 2026-05-14 16:30:22.0"));
        assert!(point_label(shown(Slot::B, &b), 0).ends_with(", 306.0 m"));
    }

    #[test]
    fn b_marker_is_where_b_track_reaches_and_follow_takes_it_alone() {
        let track = b_log().track;
        // on B's clock: inside its track, a point; outside, none
        assert_eq!(within(&track, 11.2), Some(1));
        assert_eq!(within(&track, 9.9), None);
        assert_eq!(within(&track, 12.1), None);

        // with no track of A's, Follow keeps B's marker in view
        let pair = Pair {
            map: MapPanel::default(),
            a: track_log(&[]),
            b: Some((b_log(), -8.0)),
            cursor: Some(2.0),
            follow: true,
            seek: None,
        };
        let mut harness = pair_harness(pair);
        let point = |i: usize| lat_lon(track.lats[i], track.lons[i]);
        // zoomed in on B's first point, as by hand: Follow lets go
        let memory = &mut harness.state_mut().map.memory;
        memory.center_at(point(0));
        memory.set_zoom(21.0).unwrap();
        harness.run();
        assert!(!harness.state().follow);
        // on again, the cursor at 4 s is B's 12 s, its last point
        harness.state_mut().follow = true;
        harness.state_mut().cursor = Some(4.0);
        harness.run();
        let map = &harness.state().map;
        let center = map.memory.detached().unwrap();
        assert!(same_view(
            (map.memory.zoom(), center),
            (map.memory.zoom(), point(2))
        ));
    }
}
