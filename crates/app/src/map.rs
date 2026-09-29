//! The map: the vehicle's track over OpenStreetMap tiles, with the plot's
//! cursor marked on it by an arrow along the vehicle's heading, and a
//! view that can follow it. Hovering the track reads a point's time; a
//! click seeks the plot to it.

use dflog::time::TimeBase;
use egui::{Color32, Pos2, Rect, Stroke};
use walkers::sources::OpenStreetMap;
use walkers::{HttpOptions, HttpTiles, Map, MapMemory, Position, Projector, Tiles, lat_lon};

use crate::model::{Bounds, LoadedLog, Track};
use crate::timefmt;

/// The map's state between frames.
pub struct MapPanel {
    /// The tile downloader, while tiles are wanted.
    tiles: Option<HttpTiles>,
    memory: MapMemory,
    /// Fit the view to the track on the next frame: set when a log opens,
    /// and by the Fit button.
    fit_pending: bool,
    /// The track on screen for the view it was projected in, so a still
    /// map costs nothing per frame.
    projected: Option<ProjectedTrack>,
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
            followed: None,
        }
    }
}

/// The track's points on screen, with the view they hold for.
struct ProjectedTrack {
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
            .field("followed", &self.followed)
            .finish()
    }
}

/// The track's line.
const TRACK_COLOR: Color32 = Color32::from_rgb(255, 140, 40);
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

impl MapPanel {
    /// A new log is on: show the whole of its track.
    pub fn reload(&mut self) {
        self.fit_pending = true;
        self.projected = None;
    }

    /// The map of `log`'s track with the point nearest `cursor` marked,
    /// by an arrow along the vehicle's heading where the log gives one;
    /// hover labels read through `clock` as the readout does. With
    /// `online`, tiles are downloaded from OpenStreetMap; without, the
    /// track draws on a plain background. With `follow`, the map centers
    /// on the marker when it nears the view's edge, and a drag, scroll or
    /// zoom of the map by hand turns `follow` off. Returns the time of a
    /// track point clicked, to seek to.
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        log: &LoadedLog,
        cursor: Option<f64>,
        clock: Option<TimeBase>,
        online: bool,
        follow: &mut bool,
    ) -> Option<f64> {
        let track = &log.track;
        let Some(bounds) = track.bounds() else {
            ui.centered_and_justified(|ui| {
                ui.weak("No position in this log: no POS records, and no GPS records with a fix.");
            });
            return None;
        };
        ui.horizontal(|ui| {
            ui.weak(format!(
                "Track from {} ({} points)",
                track.source,
                track.len()
            ));
            if ui
                .small_button("Fit")
                .on_hover_text("Show the whole track")
                .clicked()
            {
                self.fit_pending = true;
            }
        });
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
        let attribution = self.tiles.as_ref().map(Tiles::attribution);
        let tiles = self.tiles.as_mut().map(|t| t as &mut dyn Tiles);
        let projected = &mut self.projected;
        let response = Map::new(tiles, &mut self.memory, center).show(
            ui,
            |ui, response, projector, memory| {
                let view = (
                    memory.zoom(),
                    memory.detached().unwrap_or(center),
                    response.rect,
                );
                let points = project(projected, view, track, projector);
                let seek = draw_track(ui, response, projector, track, points, marker, clock);
                let marker_at = marker.map(|m| screen(projector, track, m.index));
                (seek, marker_at)
            },
        );
        let (seek, marker_at) = response.inner;
        let rect = response.response.rect;
        if *follow {
            if self.follow(center, rect, marker.zip(marker_at), track, follow) {
                // drawn where it was: the next frame shows it centered
                ui.ctx().request_repaint();
            }
        } else {
            self.followed = None;
        }
        if let Some(attribution) = attribution {
            let text = format!("\u{a9} {}", attribution.text);
            let size = egui::vec2(11.0 + 6.0 * text.len() as f32, 16.0);
            let rect = egui::Rect::from_min_size(
                response.response.rect.right_bottom() - size - egui::vec2(4.0, 4.0),
                size,
            );
            ui.painter()
                .rect_filled(rect, 3.0, ui.visuals().window_fill.gamma_multiply(0.85));
            ui.put(
                rect,
                egui::Hyperlink::from_label_and_url(
                    egui::RichText::new(text).small(),
                    attribution.url,
                ),
            );
        }
        seek
    }

    /// The zoom and center the map shows.
    fn view(&self, center: Position) -> (f64, Position) {
        (self.memory.zoom(), self.memory.detached().unwrap_or(center))
    }

    /// Keep the marker in view: center the map on it once it is past
    /// [`FOLLOW_REACH`] of the view in `rect` from the middle, keeping the
    /// zoom. A view that moved since Follow last left it was moved by
    /// hand, and turns `follow` off instead. Whether the map moved.
    fn follow(
        &mut self,
        center: Position,
        rect: Rect,
        marker: Option<(Marker, Pos2)>,
        track: &Track,
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
        if let Some((m, _)) = away {
            self.memory
                .center_at(lat_lon(track.lats[m.index], track.lons[m.index]));
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

/// The middle of `bounds`.
fn center(bounds: Bounds) -> Position {
    lat_lon(
        f64::midpoint(bounds.south, bounds.north),
        f64::midpoint(bounds.west, bounds.east),
    )
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

/// The track's points on screen for the view `(zoom, center, rect)`,
/// reprojected only when the view has changed since the last frame.
fn project<'a>(
    cache: &'a mut Option<ProjectedTrack>,
    (zoom, center, rect): (f64, Position, Rect),
    track: &Track,
    projector: &Projector,
) -> &'a [(usize, Pos2)] {
    let stale = cache
        .as_ref()
        .is_none_or(|c| c.zoom.to_bits() != zoom.to_bits() || c.center != center || c.rect != rect);
    if stale {
        *cache = None;
    }
    let held = cache.get_or_insert_with(|| ProjectedTrack {
        zoom,
        center,
        rect,
        points: thin(track, |i| screen(projector, track, i)),
    });
    &held.points
}

/// Track point `i` on screen.
fn screen(projector: &Projector, track: &Track, i: usize) -> Pos2 {
    projector
        .project(lat_lon(track.lats[i], track.lons[i]))
        .to_pos2()
}

/// The track point the cursor is at, and the vehicle's heading there.
#[derive(Debug, Clone, Copy)]
struct Marker {
    index: usize,
    /// Degrees clockwise from north.
    heading: Option<f64>,
}

/// The track as a line through `points`, its start and end, the
/// `marker`'s point marked, and the point under the pointer labeled with
/// its time. Returns the time of a point clicked.
fn draw_track(
    ui: &egui::Ui,
    response: &egui::Response,
    projector: &Projector,
    track: &Track,
    points: &[(usize, Pos2)],
    marker: Option<Marker>,
    clock: Option<TimeBase>,
) -> Option<f64> {
    let rect = response.rect;
    let painter = ui.painter().with_clip_rect(rect);

    painter.add(egui::Shape::line(
        points.iter().map(|&(_, p)| p).collect(),
        Stroke::new(2.5, TRACK_COLOR),
    ));
    if let (Some(&(_, first)), Some(&(_, last))) = (points.first(), points.last()) {
        painter.circle_filled(first, 4.0, Color32::from_rgb(70, 200, 90));
        painter.circle_filled(last, 4.0, Color32::from_rgb(235, 70, 70));
    }
    if let Some(m) = marker {
        let at = screen(projector, track, m.index);
        if let Some(heading) = m.heading {
            // the notch makes it concave: a halo around the outline, and
            // the two halves filled
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
        } else {
            painter.circle_stroke(at, 6.0, HALO);
            painter.circle(
                at,
                6.0,
                Color32::from_white_alpha(60),
                Stroke::new(2.0, Color32::WHITE),
            );
        }
    }

    let hover = response.hover_pos()?;
    let (i, at, _) = points
        .iter()
        .map(|&(i, p)| (i, p, p.distance(hover)))
        .min_by(|a, b| a.2.total_cmp(&b.2))
        .filter(|hit| hit.2 <= HIT_DISTANCE)?;
    painter.circle_stroke(at, 5.0, HALO);
    painter.circle_stroke(at, 5.0, Stroke::new(1.5, Color32::WHITE));
    let time = timefmt::stamp(clock, track.times[i]);
    let text = match track.alts[i] {
        alt if alt.is_finite() => format!("{time}, {alt:.1} m"),
        _ => time,
    };
    let font = egui::TextStyle::Small.resolve(ui.style());
    let galley = painter.layout_no_wrap(text, font, ui.visuals().text_color());
    let pos = at + egui::vec2(10.0, -10.0 - galley.size().y);
    painter.rect_filled(
        egui::Rect::from_min_size(pos, galley.size()).expand(3.0),
        3.0,
        ui.visuals().window_fill.gamma_multiply(0.9),
    );
    painter.galley(pos, galley, ui.visuals().text_color());
    response.clicked().then(|| track.times[i])
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
                    bench
                        .map
                        .show(ui, &log, bench.cursor, None, false, &mut bench.follow);
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
}
