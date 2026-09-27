//! The map: the vehicle's track over OpenStreetMap tiles, with the plot's
//! cursor marked on it. Hovering the track reads a point's time; a click
//! seeks the plot to it.

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
}

impl Default for MapPanel {
    fn default() -> Self {
        Self {
            tiles: None,
            memory: MapMemory::default(),
            fit_pending: true,
            projected: None,
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
            .finish()
    }
}

/// The track's line.
const TRACK_COLOR: Color32 = Color32::from_rgb(255, 140, 40);
/// How close to the track, in points, the pointer counts as on it.
const HIT_DISTANCE: f32 = 12.0;

impl MapPanel {
    /// A new log is on: show the whole of its track.
    pub fn reload(&mut self) {
        self.fit_pending = true;
        self.projected = None;
    }

    /// The map of `log`'s track with the point nearest `cursor` marked;
    /// hover labels read through `clock` as the readout does. With
    /// `online`, tiles are downloaded from OpenStreetMap; without, the
    /// track draws on a plain background. Returns the time of a track
    /// point clicked, to seek to.
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        log: &LoadedLog,
        cursor: Option<f64>,
        clock: Option<TimeBase>,
        online: bool,
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
        }

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
                draw_track(ui, response, projector, track, points, cursor, clock)
            },
        );
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
        response.inner
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

/// The track as a line through `points`, its start and end, the cursor's
/// point ringed, and the point under the pointer labeled with its time.
/// Returns the time of a point clicked.
fn draw_track(
    ui: &egui::Ui,
    response: &egui::Response,
    projector: &Projector,
    track: &Track,
    points: &[(usize, Pos2)],
    cursor: Option<f64>,
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
    if let Some(i) = cursor.and_then(|t| track.nearest(t)) {
        painter.circle(
            screen(projector, track, i),
            6.0,
            Color32::from_white_alpha(60),
            Stroke::new(2.0, Color32::WHITE),
        );
    }

    let hover = response.hover_pos()?;
    let (i, at, _) = points
        .iter()
        .map(|&(i, p)| (i, p, p.distance(hover)))
        .min_by(|a, b| a.2.total_cmp(&b.2))
        .filter(|hit| hit.2 <= HIT_DISTANCE)?;
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
}
