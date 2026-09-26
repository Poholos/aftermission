//! The application: menus, the side panel with the log's types, the plot,
//! the map, the bottom panel with the events list and the parameter
//! table, and the ways a log gets opened.

use std::path::{Path, PathBuf};

use egui::{Align2, Color32, Context};

use crate::events::EventsPanel;
use crate::filter::Filter;
use crate::map::MapPanel;
use crate::model::LoadedLog;
use crate::params::ParamsPanel;
use crate::plot::PlotPanel;
use crate::settings::{BottomTab, Settings, TimeAxis};
use crate::tree;
use crate::worker::OpenJob;

const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Default)]
pub struct AftermissionApp {
    settings: Settings,
    log: Option<LoadedLog>,
    job: Option<OpenJob>,
    /// Why the last open failed, until the next one starts.
    error: Option<String>,
    plot: PlotPanel,
    map: MapPanel,
    events: EventsPanel,
    params: ParamsPanel,
    /// The side panel's type and field filter.
    filter: String,
    /// Whether the filter was in use last frame: the headers it unfolded
    /// fold again when it is cleared.
    filter_was_active: bool,
    about_open: bool,
}

impl AftermissionApp {
    pub fn new(cc: &eframe::CreationContext<'_>, initial: Option<PathBuf>) -> Self {
        let mut app = Self {
            settings: Settings::load(cc.storage),
            ..Self::default()
        };
        // Our setting outranks the preference eframe restored with egui's memory.
        cc.egui_ctx.set_theme(app.settings.theme);
        if let Some(path) = initial {
            app.open_path(&path, &cc.egui_ctx);
        }
        app
    }

    /// Start opening `path`. The log on screen stays until the new one is
    /// ready; an open already running is dropped.
    pub fn open_path(&mut self, path: &Path, ctx: &Context) {
        self.error = None;
        self.job = Some(OpenJob::start(path.to_path_buf(), ctx.clone()));
    }

    /// The loaded log, for tests.
    #[cfg(test)]
    pub(crate) fn log(&self) -> Option<&LoadedLog> {
        self.log.as_ref()
    }

    #[cfg(test)]
    pub(crate) fn plot(&self) -> &PlotPanel {
        &self.plot
    }

    #[cfg(test)]
    pub(crate) fn set_filter(&mut self, filter: &str) {
        self.filter = filter.to_string();
    }

    #[cfg(test)]
    pub(crate) fn params_mut(&mut self) -> &mut ParamsPanel {
        &mut self.params
    }

    #[cfg(test)]
    pub(crate) fn events_mut(&mut self) -> &mut EventsPanel {
        &mut self.events
    }

    /// Take the result of a finished open.
    pub fn poll(&mut self, ctx: &Context) {
        let Some(job) = &mut self.job else {
            return;
        };
        let Some(result) = job.poll() else {
            return;
        };
        let path = job.path.clone();
        self.job = None;
        match result {
            Ok(log) => {
                // only a log that opened is worth offering again
                self.settings.remember(&path);
                self.plot.reload(&log);
                self.map.reload();
                self.params.reload();
                ctx.send_viewport_cmd(egui::ViewportCommand::Title(format!(
                    "{} - Aftermission",
                    log.name
                )));
                tracing::info!(name = %log.name, records = log.records(), "log opened");
                self.log = Some(log);
            }
            Err(message) => {
                tracing::warn!(%message, "cannot open the log");
                self.error = Some(message);
            }
        }
    }

    fn handle_drop(&mut self, ctx: &Context) {
        let dropped: Vec<PathBuf> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .map(|f| f.path().to_path_buf())
                .collect()
        });
        // Several files dropped at once: the first opens, as one log is shown.
        if let Some(path) = dropped.first() {
            self.open_path(path, ctx);
        }
    }

    fn shortcuts(&mut self, ctx: &Context) {
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::O)) {
            self.open_dialog(ctx);
        }
    }

    fn open_dialog(&mut self, ctx: &Context) {
        let picked = rfd::FileDialog::new()
            .add_filter("Dataflash log", &["bin", "BIN"])
            .pick_file();
        if let Some(path) = picked {
            self.open_path(&path, ctx);
        }
    }

    /// The whole window.
    pub fn show(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        self.poll(&ctx);
        self.handle_drop(&ctx);
        self.shortcuts(&ctx);

        egui::Panel::top("menu_bar").show(ui, |ui| {
            egui::MenuBar::new().ui(ui, |ui| {
                self.file_menu(ui);
                self.view_menu(ui);
                self.help_menu(ui);
                ui.separator();
                self.status(ui);
            });
        });
        if self.settings.side_panel {
            egui::Panel::left("side_panel")
                .default_size(320.0)
                .show(ui, |ui| {
                    egui::ScrollArea::vertical().show(ui, |ui| self.side_ui(ui));
                });
        }
        egui::CentralPanel::default().show(ui, |ui| self.central_ui(ui));
        self.about_ui(&ctx);
        Self::drop_overlay(&ctx);
    }

    fn file_menu(&mut self, ui: &mut egui::Ui) {
        ui.menu_button("File", |ui| {
            if ui.button("Open\u{2026}").clicked() {
                ui.close();
                self.open_dialog(ui.ctx());
            }
            let recent = self.settings.recent.clone();
            ui.add_enabled_ui(!recent.is_empty(), |ui| {
                ui.menu_button("Open recent", |ui| {
                    for path in &recent {
                        if ui.button(path.display().to_string()).clicked() {
                            ui.close();
                            self.open_path(path, ui.ctx());
                        }
                    }
                    ui.separator();
                    if ui.button("Clear list").clicked() {
                        ui.close();
                        self.settings.recent.clear();
                    }
                });
            });
            ui.separator();
            if ui.button("Quit").clicked() {
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
            }
        });
    }

    fn view_menu(&mut self, ui: &mut egui::Ui) {
        ui.menu_button("View", |ui| {
            ui.menu_button("Theme", |ui| {
                use egui::ThemePreference;
                for (theme, name) in [
                    (ThemePreference::Dark, "Dark"),
                    (ThemePreference::Light, "Light"),
                    (ThemePreference::System, "System"),
                ] {
                    if ui
                        .radio_value(&mut self.settings.theme, theme, name)
                        .changed()
                    {
                        ui.ctx().set_theme(theme);
                    }
                }
            });
            ui.menu_button("Time axis", |ui| {
                ui.radio_value(&mut self.settings.time_axis, TimeAxis::Boot, "Since boot");
                ui.radio_value(&mut self.settings.time_axis, TimeAxis::Utc, "UTC")
                    .on_hover_text("Through the first GPS fix; since boot when the log has none");
            });
            ui.checkbox(&mut self.settings.show_modes, "Mode bands");
            ui.checkbox(&mut self.settings.side_panel, "Side panel");
            ui.checkbox(&mut self.settings.show_map, "Map");
            ui.checkbox(&mut self.settings.show_bottom, "Events and parameters");
            ui.checkbox(
                &mut self.settings.online_tiles,
                "Map tiles from OpenStreetMap",
            )
            .on_hover_text("Off, the track draws on a plain background and nothing is downloaded");
            ui.separator();
            if ui.button("Clear plot").clicked() {
                ui.close();
                self.plot.clear();
            }
        });
    }

    fn help_menu(&mut self, ui: &mut egui::Ui) {
        ui.menu_button("Help", |ui| {
            if ui.button("About Aftermission").clicked() {
                ui.close();
                self.about_open = true;
            }
        });
    }

    /// A spinner while a log is being opened.
    fn status(&self, ui: &mut egui::Ui) {
        if let Some(job) = &self.job {
            ui.spinner();
            let size = job
                .bytes
                .map(|b| format!(" ({:.1} MB)", b as f64 / 1e6))
                .unwrap_or_default();
            ui.label(format!("Indexing {}{size}\u{2026}", job.name()));
        }
    }

    fn side_ui(&mut self, ui: &mut egui::Ui) {
        if let Some(error) = &self.error {
            ui.colored_label(ui.visuals().error_fg_color, error);
            ui.separator();
        }
        let Some(log) = &self.log else {
            ui.label("No log open.");
            ui.weak("File > Open\u{2026}, or drop a .bin file on the window.");
            return;
        };
        tree::summary(ui, log, &self.settings);
        ui.separator();
        ui.add(
            egui::TextEdit::singleline(&mut self.filter)
                .hint_text("Filter types and fields")
                .desired_width(f32::INFINITY),
        );
        if let Some(error) = &self.plot.error {
            ui.colored_label(ui.visuals().error_fg_color, error);
        }
        let filter = Filter::new(&self.filter);
        let filtering = !filter.is_blank();
        let open = if filtering {
            Some(true)
        } else if self.filter_was_active {
            Some(false)
        } else {
            None
        };
        self.filter_was_active = filtering;
        tree::types(ui, log, &mut self.plot, &filter, open);
    }

    fn central_ui(&mut self, ui: &mut egui::Ui) {
        let Some(log) = &self.log else {
            ui.vertical_centered(|ui| {
                ui.add_space(ui.available_height() * 0.3);
                ui.heading("Open a log to review");
                ui.label("Drop an ArduPilot .bin file here, or press Ctrl+O.");
                if ui.button("Open\u{2026}").clicked() {
                    self.open_dialog(ui.ctx());
                }
            });
            return;
        };
        // The map to the right and the events or parameters below share
        // the width and height with the plot, which takes what is left.
        if self.settings.show_map {
            egui::Panel::right("map_panel")
                .default_size(420.0)
                .show(ui, |ui| {
                    let cursor = self.plot.cursor;
                    let clock = log.wall_clock(self.settings.time_axis);
                    let online = self.settings.online_tiles;
                    if let Some(time) = self.map.show(ui, log, cursor, clock, online) {
                        self.plot.seek(time);
                    }
                });
        }
        if self.settings.show_bottom {
            egui::Panel::bottom("bottom_panel")
                .default_size(180.0)
                .resizable(true)
                .show(ui, |ui| {
                    let tab = &mut self.settings.bottom_tab;
                    ui.horizontal(|ui| {
                        ui.selectable_value(tab, BottomTab::Events, "Events");
                        ui.selectable_value(tab, BottomTab::Parameters, "Parameters");
                    });
                    let seek = match *tab {
                        BottomTab::Events => {
                            self.events.show(ui, log, &self.settings, self.plot.cursor)
                        }
                        BottomTab::Parameters => self.params.show(ui, log, &self.settings),
                    };
                    if let Some(time) = seek {
                        self.plot.seek(time);
                    }
                });
        }
        self.plot.show(ui, log, &self.settings);
    }

    fn about_ui(&mut self, ctx: &Context) {
        egui::Window::new("About Aftermission")
            .open(&mut self.about_open)
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                ui.label(format!("Aftermission {VERSION}"));
                ui.label("Post-mission review of ArduPilot dataflash logs.");
                ui.label("Copyright 2026 Poholos. Licensed under the AGPL-3.0-only; commercial licenses available.");
                ui.hyperlink("https://github.com/Poholos/aftermission");
            });
    }

    /// A shade over the window while a file is being dragged over it.
    fn drop_overlay(ctx: &Context) {
        if ctx.input(|i| i.raw.hovered_files.is_empty()) {
            return;
        }
        let rect = ctx.content_rect();
        let painter = ctx.layer_painter(egui::LayerId::new(
            egui::Order::Foreground,
            egui::Id::new("drop_overlay"),
        ));
        painter.rect_filled(rect, 0.0, Color32::from_black_alpha(140));
        painter.text(
            rect.center(),
            Align2::CENTER_CENTER,
            "Drop the log to open it",
            egui::FontId::proportional(24.0),
            Color32::WHITE,
        );
    }
}

impl eframe::App for AftermissionApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.show(ui);
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        self.settings.save(storage);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::{Harness, kittest::Queryable};

    /// Write the synthetic log, open it and wait for the model.
    fn opened(dir: &Path) -> AftermissionApp {
        let path = dir.join("flight.bin");
        std::fs::write(&path, crate::model::testlog::bytes()).unwrap();
        let mut app = AftermissionApp::default();
        // tests draw the map without downloading anything
        app.settings.online_tiles = false;
        reopen(&mut app, &path);
        assert_eq!(app.settings.recent, [path]);
        app
    }

    /// Open `path` in `app` and wait until the job is done.
    fn reopen(app: &mut AftermissionApp, path: &Path) {
        let ctx = Context::default();
        app.open_path(path, &ctx);
        assert!(app.job.is_some(), "the open runs in the background");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while app.job.is_some() {
            assert!(std::time::Instant::now() < deadline, "{:?}", app.error);
            app.poll(&ctx);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        // a failed open keeps the log before, so the error is what tells
        assert!(app.error.is_none(), "{:?}", app.error);
        assert!(app.log().is_some());
    }

    /// Run a frame of the app in a window with room for every panel, and
    /// look at it.
    fn with_ui(app: &mut AftermissionApp, check: impl FnOnce(&mut Harness<'_>)) {
        let mut harness = Harness::builder()
            .with_size(egui::vec2(1400.0, 900.0))
            .build_ui(|ui| app.show(ui));
        harness.run();
        check(&mut harness);
    }

    #[test]
    fn without_a_log_the_window_says_how_to_open_one() {
        let mut app = AftermissionApp::default();
        with_ui(&mut app, |harness| {
            harness.get_by_label("Open a log to review");
            harness.get_by_label("No log open.");
        });
    }

    #[test]
    fn a_field_checked_in_the_tree_goes_on_the_plot() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = opened(dir.path());
        // the filter unfolds the types it matches
        app.set_filter("roll");
        with_ui(&mut app, |harness| {
            harness.get_by_label("flight.bin");
            harness.get_by_label_contains("Copter, ");
            harness.get_by_label("Roll (deg)").click();
            harness.run();
            // the series chip, and the legend once the plot has drawn
            assert!(harness.get_all_by_label("ATT.Roll (deg)").count() >= 1);
        });
        assert_eq!(app.plot().selected.len(), 1);
        assert_eq!(app.plot().selected[0].key.label(), "ATT.Roll");
        assert!(app.plot().error.is_none());

        // with the filter gone the types fold again, ATT included, and the
        // instances show as nodes
        app.set_filter("");
        with_ui(&mut app, |harness| {
            harness.get_by_label("IMU (8)");
            assert!(harness.query_by_label("IMU[1]").is_none(), "folded");
            assert!(
                harness.query_by_label("Roll (deg)").is_none(),
                "folded again"
            );
            harness.get_by_label("IMU (8)").click();
            harness.run();
            harness.get_by_label("IMU[1]").click();
            harness.run();
            harness.get_by_label_contains("GyrX").click();
            harness.run();
        });
        assert_eq!(app.plot().selected.len(), 2);
        assert_eq!(app.plot().selected[1].key.label(), "IMU[1].GyrX");
    }

    #[test]
    fn the_map_shows_the_track_and_a_clicked_event_seeks_the_plot() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = opened(dir.path());
        with_ui(&mut app, |harness| {
            harness.get_by_label("Track from POS (3 points)");
            harness.get_by_label_contains("MSG  ArduCopter V4.7.0");
            harness.get_by_label_contains("ERR  Compass: resolved");
            harness.get_by_label_contains("MODE Loiter").click();
            harness.run();
        });
        assert_eq!(app.plot().cursor, Some(2.25));
        assert!(app.plot().readout.is_empty(), "nothing plotted yet");

        // the panels can be turned off
        app.settings.show_map = false;
        app.settings.show_bottom = false;
        with_ui(&mut app, |harness| {
            assert!(
                harness
                    .query_by_label("Track from POS (3 points)")
                    .is_none()
            );
            assert!(harness.query_by_label_contains("MODE Loiter").is_none());
        });
    }

    #[test]
    fn the_parameters_tab_lists_filters_and_a_clicked_change_seeks_the_plot() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = opened(dir.path());
        with_ui(&mut app, |harness| {
            harness.get_by_label("Parameters").click();
            harness.run();
            harness.get_by_label_contains("WPNAV_SPEED");
            harness.get_by_label_contains("ATC_RAT_RLL_P");
            // the changed parameter unfolds to its history
            harness.get_by_label_contains("MIS_TOTAL").click();
            harness.run();
            harness.get_by_label_contains("at boot");
            harness.get_by_label_contains("0:02.320").click();
            harness.run();
        });
        assert_eq!(app.settings.bottom_tab, BottomTab::Parameters);
        assert_eq!(app.plot().cursor, Some(2.32));

        // the toggles narrow the table
        with_ui(&mut app, |harness| {
            harness.get_by_label("Not default").click();
            harness.run();
            harness.get_by_label_contains("WPNAV_SPEED");
            harness.get_by_label_contains("MIS_TOTAL");
            assert!(harness.query_by_label_contains("ATC_RAT_RLL_P").is_none());
            assert!(harness.query_by_label_contains("SIM_RATE_HZ").is_none());
            harness.get_by_label("Changed after boot").click();
            harness.run();
            harness.get_by_label_contains("MIS_TOTAL");
            assert!(harness.query_by_label_contains("WPNAV_SPEED").is_none());
        });

        // the filter narrows by name, and to nothing
        with_ui(&mut app, |harness| {
            harness.get_by_label("Not default").click();
            harness.run();
            harness.get_by_label("Changed after boot").click();
            harness.run();
        });
        app.params_mut().set_filter("wpnav");
        with_ui(&mut app, |harness| {
            harness.get_by_label_contains("WPNAV_SPEED");
            assert!(harness.query_by_label_contains("MIS_TOTAL").is_none());
            assert!(harness.query_by_label_contains("ATC_RAT_RLL_P").is_none());
        });
        app.params_mut().set_filter("rtl");
        with_ui(&mut app, |harness| {
            harness.get_by_label("Nothing matches.");
        });

        // a log opened afresh starts with every history folded
        app.params_mut().set_filter("");
        let path = app.settings.recent[0].clone();
        reopen(&mut app, &path);
        with_ui(&mut app, |harness| {
            harness.get_by_label_contains("MIS_TOTAL");
            assert!(harness.query_by_label_contains("at boot").is_none());
        });
    }

    #[test]
    fn the_bottom_panel_keeps_its_height_on_either_tab_and_after_a_drag() {
        use dflog::access::Value;
        use dflog::write::LogWriter;

        // enough parameters and events that both lists have to scroll
        let mut w = LogWriter::new();
        w.define(1, "PARM", "QNff", &["TimeUS", "Name", "Value", "Default"])
            .unwrap();
        w.define(2, "EV", "QB", &["TimeUS", "Id"]).unwrap();
        for i in 0..40u64 {
            w.record("EV", &[Value::U64(2_000_000 + i * 1000), Value::U64(10)])
                .unwrap();
        }
        for i in 0..120u64 {
            w.record(
                "PARM",
                &[
                    Value::U64(1_000_000),
                    Value::Str(format!("TEST_P{i:03}")),
                    Value::F64(i as f64),
                    Value::F64(0.0),
                ],
            )
            .unwrap();
        }
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("many.bin");
        std::fs::write(&path, w.into_bytes()).unwrap();
        let mut app = AftermissionApp::default();
        app.settings.online_tiles = false;
        app.settings.bottom_tab = BottomTab::Parameters;
        reopen(&mut app, &path);

        let mut harness = Harness::builder()
            .with_size(egui::vec2(1400.0, 900.0))
            .build_ui_state(|ui, app: &mut AftermissionApp| app.show(ui), app);
        let height = |harness: &Harness<'_, AftermissionApp>| {
            egui::containers::panel::PanelState::load(&harness.ctx, egui::Id::new("bottom_panel"))
                .expect("the bottom panel has drawn")
                .size()
                .y
        };
        // a table that scrolls does not grow the panel past its default
        harness.run();
        harness.get_by_label_contains("TEST_P000");
        let before = height(&harness);
        assert!(before < 200.0, "the panel grew to {before}");

        // nor does it shrink to one row or the empty message, and stay
        // there
        for filter in ["no-such-name", "TEST_P000", ""] {
            harness.state_mut().params_mut().set_filter(filter);
            harness.run();
            let now = height(&harness);
            assert!(
                (now - before).abs() < 1.0,
                "filtered to {filter:?}, the panel went from {before} to {now}"
            );
        }
        harness.get_by_label_contains("TEST_P000");

        // the events list holds it too
        harness.state_mut().settings.bottom_tab = BottomTab::Events;
        for filter in ["no-such-event", ""] {
            harness.state_mut().events_mut().set_filter(filter);
            harness.run();
            let now = height(&harness);
            assert!(
                (now - before).abs() < 1.0,
                "events filtered to {filter:?}, the panel went from {before} to {now}"
            );
        }
        // rows on screen, so the list itself held the height
        assert!(harness.get_all_by_label_contains("Armed").count() > 1);

        // dragged small on one tab, the panel stays small on the other
        harness.state_mut().settings.bottom_tab = BottomTab::Parameters;
        harness.run();
        // the edge from the stored rect: the central panel's margin keeps
        // the panel off the window's bottom
        let rect =
            egui::containers::panel::PanelState::load(&harness.ctx, egui::Id::new("bottom_panel"))
                .expect("the bottom panel has drawn")
                .outer_rect;
        let at = |y: f32| egui::pos2(rect.center().x, y);
        let (edge, target) = (rect.min.y, rect.min.y + 100.0);
        harness.hover_at(at(edge));
        harness.run();
        harness.drag_at(at(edge));
        harness.run();
        for step in 1..=5u8 {
            harness.hover_at(at(edge + (target - edge) * f32::from(step) / 5.0));
            harness.run();
        }
        harness.drop_at(at(target));
        harness.run();
        let small = height(&harness);
        assert!(
            (small - (before - 100.0)).abs() < 1.0,
            "the drag left the panel at {small}"
        );
        harness.state_mut().settings.bottom_tab = BottomTab::Events;
        harness.run();
        let now = height(&harness);
        assert!(
            (now - small).abs() < 1.0,
            "the events tab took the panel from {small} to {now}"
        );
        assert!(harness.get_all_by_label_contains("Armed").count() > 1);
    }

    #[test]
    fn a_failed_open_is_reported_in_the_side_panel() {
        let mut app = AftermissionApp::default();
        let ctx = Context::default();
        app.open_path(Path::new("no-such-folder/none.bin"), &ctx);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while app.job.is_some() {
            assert!(std::time::Instant::now() < deadline);
            app.poll(&ctx);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let error = app.error.clone().unwrap();
        assert!(error.starts_with("no-such-folder"), "{error}");
        assert!(
            app.settings.recent.is_empty(),
            "a failed path is not remembered"
        );
        with_ui(&mut app, |harness| {
            harness.get_by_label(error.as_str());
        });
    }
}
