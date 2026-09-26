//! The application: menus, the side panel with the log's types, the plot,
//! the map, the bottom panel with the events list and the parameter
//! table, the ways a log gets opened, and the exports.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::ops::RangeInclusive;
use std::path::{Path, PathBuf};

use egui::{Align2, Color32, Context};

use crate::csv::{self, Column, Export};
use crate::events::EventsPanel;
use crate::filter::Filter;
use crate::map::MapPanel;
use crate::model::LoadedLog;
use crate::params::ParamsPanel;
use crate::plot::PlotPanel;
use crate::settings::{BottomTab, Settings, TimeAxis};
use crate::tree;
use crate::worker::{self, Done, Job, Kind};

const VERSION: &str = env!("CARGO_PKG_VERSION");

/// A line in the menu bar: the last job's outcome, or why one could not
/// start.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Notice {
    text: String,
    error: bool,
}

/// The choices made before a CSV export.
#[derive(Debug, Default)]
struct CsvDialog {
    /// Only the time range the plot shows.
    visible_only: bool,
}

#[derive(Debug, Default)]
pub struct AftermissionApp {
    settings: Settings,
    log: Option<LoadedLog>,
    /// The one job running: an open or an export.
    job: Option<Job>,
    /// Why the last open failed, until the next one starts.
    error: Option<String>,
    /// Until the next job starts.
    notice: Option<Notice>,
    /// The CSV export's confirmation window, while it is open.
    csv_dialog: Option<CsvDialog>,
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
    /// ready. Refused while a job runs.
    pub fn open_path(&mut self, path: &Path, ctx: &Context) {
        if self.busy() {
            return;
        }
        self.error = None;
        self.job = Some(Job::open(path.to_path_buf(), ctx.clone()));
    }

    /// One job at a time: while one runs, the next is refused with a
    /// notice beside the spinner, not queued; the notice goes when the job
    /// does. A job that may start clears the last notice.
    fn busy(&mut self) -> bool {
        if let Some(job) = &self.job {
            self.notice = Some(Notice {
                text: format!("Still busy: {}\u{2026}", job.label),
                error: false,
            });
            return true;
        }
        self.notice = None;
        false
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

    /// Put `key` on the plot, as a click in the tree does; for tests.
    #[cfg(test)]
    pub(crate) fn toggle_series(&mut self, key: crate::model::SeriesKey) {
        if let Some(log) = &self.log {
            self.plot.toggle(key, log);
        }
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

    /// Take the result of a finished job.
    pub fn poll(&mut self, ctx: &Context) {
        let Some(job) = &mut self.job else {
            return;
        };
        let Some(result) = job.poll() else {
            return;
        };
        let kind = job.kind;
        self.job = None;
        // a refusal made while the job ran is over with it
        self.notice = None;
        match result {
            Ok(Done::Opened { path, log }) => {
                // only a log that opened is worth offering again
                self.settings.remember(&path);
                self.plot.reload(&log);
                self.map.reload();
                self.params.reload();
                // an export set up on the log before would write the wrong one
                self.csv_dialog = None;
                ctx.send_viewport_cmd(egui::ViewportCommand::Title(format!(
                    "{} - Aftermission",
                    log.name
                )));
                tracing::info!(name = %log.name, records = log.records(), "log opened");
                self.log = Some(*log);
            }
            Ok(Done::Exported { path, summary }) => {
                tracing::info!(%summary, "export done");
                // a folder that took a file is where the next dialog opens
                if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
                    self.settings.export_dir = Some(dir.to_path_buf());
                }
                self.notice = Some(Notice {
                    text: summary,
                    error: false,
                });
            }
            Err(message) if kind == Kind::Open => {
                tracing::warn!(%message, "cannot open the log");
                self.error = Some(message);
            }
            Err(message) => {
                tracing::warn!(%message, "the export failed");
                self.notice = Some(Notice {
                    text: message,
                    error: true,
                });
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
        self.csv_dialog_ui(&ctx);
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
            // not while a job runs: the export would be refused after the
            // save dialog, with the chosen path lost
            let plotted = self.log.is_some() && self.plot.showing().next().is_some();
            let idle = self.job.is_none();
            ui.menu_button("Export", |ui| {
                if ui
                    .add_enabled(
                        plotted && idle,
                        egui::Button::new("Plotted series as CSV\u{2026}"),
                    )
                    .on_disabled_hover_text(if idle {
                        "Plot a series first"
                    } else {
                        "Wait for the running job"
                    })
                    .clicked()
                {
                    ui.close();
                    self.csv_dialog = Some(CsvDialog::default());
                }
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

    /// A spinner while a job runs, and the notice: a refusal beside the
    /// spinner, or the last job's outcome.
    fn status(&self, ui: &mut egui::Ui) {
        if let Some(job) = &self.job {
            ui.spinner();
            ui.label(format!("{}\u{2026}", job.label));
        }
        if let Some(notice) = &self.notice {
            if notice.error {
                ui.colored_label(ui.visuals().error_fg_color, &notice.text);
            } else {
                ui.label(&notice.text);
            }
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

    /// The choices before a CSV export, then the save dialog.
    fn csv_dialog_ui(&mut self, ctx: &Context) {
        // a job started underneath, by a dropped file, would refuse the export
        if self.job.is_some() {
            self.csv_dialog = None;
        }
        let (Some(dialog), Some(log)) = (&mut self.csv_dialog, &self.log) else {
            return;
        };
        let zoomed = self.plot.zoomed_range(ctx);
        if zoomed.is_none() {
            // the view shows everything again: the choice no longer applies
            dialog.visible_only = false;
        }
        let range = zoomed.clone().filter(|_| dialog.visible_only);
        // the plot may have lost its series since the window opened
        let showing = self.plot.showing().next().is_some();
        let header = csv::header(
            log.time_base.is_some(),
            self.plot.showing().map(|s| s.title.as_str()),
        );
        let mut open = true;
        let mut save = false;
        let mut cancel = false;
        egui::Window::new("Export CSV")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                ui.label("Columns:");
                for name in &header {
                    ui.monospace(name);
                }
                ui.add_enabled(
                    zoomed.is_some(),
                    egui::Checkbox::new(&mut dialog.visible_only, "Only the time range in view"),
                )
                .on_disabled_hover_text("Zoom the plot to narrow the range");
                ui.horizontal(|ui| {
                    save = ui
                        .add_enabled(showing, egui::Button::new("Save as\u{2026}"))
                        .on_disabled_hover_text("Nothing is showing on the plot")
                        .clicked();
                    cancel = ui.button("Cancel").clicked();
                });
            });
        if !open || cancel {
            self.csv_dialog = None;
        }
        if save {
            let export = csv_export(log, &self.plot, range.clone());
            let name = csv::file_name(&log.name, range.as_ref());
            let mut picker = rfd::FileDialog::new()
                .add_filter("CSV", &["csv"])
                .set_file_name(name);
            if let Some(dir) = &self.settings.export_dir {
                picker = picker.set_directory(dir);
            }
            self.csv_dialog = None;
            if let Some(path) = picker.save_file() {
                self.export_csv_to(export, path, ctx);
            }
        }
    }

    /// Write `export` to `path` on a worker.
    pub(crate) fn export_csv_to(&mut self, export: Export, path: PathBuf, ctx: &Context) {
        if self.busy() {
            return;
        }
        let name = worker::file_name(&path);
        let label = format!("Writing {name}");
        let subject = path.clone();
        self.job = Some(Job::run(
            Kind::Export,
            subject,
            label,
            ctx.clone(),
            move || {
                let describe = |e: std::io::Error| format!("{}: {e}", path.display());
                let mut out = BufWriter::new(File::create(&path).map_err(describe)?);
                let rows = export.write(&mut out).map_err(describe)?;
                out.flush().map_err(describe)?;
                let summary = format!("Wrote {name}: {rows} rows, {} series", export.columns.len());
                Ok(Done::Exported { path, summary })
            },
        ));
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

/// The series showing on `plot`, as the CSV writes them: a value from an
/// `f` field prints as the float32 the log stored.
fn csv_export(log: &LoadedLog, plot: &PlotPanel, range: Option<RangeInclusive<f64>>) -> Export {
    Export {
        columns: plot
            .showing()
            .map(|s| Column {
                title: s.title.clone(),
                data: s.data.clone(),
                single: log.field_of(&s.key).is_some_and(|f| f.code == 'f'),
            })
            .collect(),
        time_base: log.time_base,
        range,
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
    use egui_kittest::{
        Harness,
        kittest::{NodeT, Queryable},
    };

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

        // parameter changes are listed only once their toggle is on
        with_ui(&mut app, |harness| {
            assert!(harness.query_by_label_contains("4 -> 7").is_none());
            harness.get_by_label("Parameter changes").click();
            harness.run();
            harness
                .get_by_label_contains("PARM MIS_TOTAL 4 -> 7")
                .click();
            harness.run();
        });
        assert_eq!(app.plot().cursor, Some(2.32));

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
    fn a_log_of_only_parameter_changes_says_they_are_toggled_off() {
        use dflog::access::Value;
        use dflog::write::LogWriter;

        let mut w = LogWriter::new();
        w.define(1, "PARM", "QNf", &["TimeUS", "Name", "Value"])
            .unwrap();
        for (time, value) in [(1_000_000, 35.0), (4_000_000, 42.5)] {
            w.record(
                "PARM",
                &[
                    Value::U64(time),
                    Value::Str("FENCE_RADIUS".into()),
                    Value::F64(value),
                ],
            )
            .unwrap();
        }
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("parm.bin");
        std::fs::write(&path, w.into_bytes()).unwrap();
        let mut app = AftermissionApp::default();
        app.settings.online_tiles = false;
        reopen(&mut app, &path);
        with_ui(&mut app, |harness| {
            harness.get_by_label("Every kind this log has is toggled off.");
            harness.get_by_label("Parameter changes").click();
            harness.run();
            harness.get_by_label_contains("PARM FENCE_RADIUS 35 -> 42.5");
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

    /// Poll until the running job is done.
    fn wait(app: &mut AftermissionApp, ctx: &Context) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while app.job.is_some() {
            assert!(
                std::time::Instant::now() < deadline,
                "the job did not finish"
            );
            app.poll(ctx);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    #[test]
    fn the_csv_window_lists_the_columns_and_follows_the_plot() {
        use crate::model::SeriesKey;

        let dir = tempfile::tempdir().unwrap();
        let mut app = opened(dir.path());
        let ctx = Context::default();
        let key = |type_name: &str, field: &str, instance| SeriesKey {
            type_name: type_name.into(),
            field: field.into(),
            instance,
        };
        // nothing plotted: nothing to export, and the menu says so
        with_ui(&mut app, |harness| {
            harness.get_by_label("File").click();
            harness.run();
            harness.get_by_label("Export \u{23F5}").hover();
            harness.run();
            assert!(
                harness
                    .get_by_label("Plotted series as CSV\u{2026}")
                    .accesskit_node()
                    .is_disabled()
            );
        });

        // Roll is a scaled integer field, GyrX a float32 one
        app.toggle_series(key("ATT", "Roll", None));
        app.toggle_series(key("IMU", "GyrX", Some(1)));
        with_ui(&mut app, |harness| {
            harness.get_by_label("File").click();
            harness.run();
            harness.get_by_label("Export \u{23F5}").hover();
            harness.run();
            harness
                .get_by_label("Plotted series as CSV\u{2026}")
                .click();
            harness.run();
            // the window lists the columns about to be written
            let window = harness.get_by_role_and_label(egui::accesskit::Role::Window, "Export CSV");
            window.get_by_label("time_s");
            window.get_by_label("ATT.Roll (deg)");
            window.get_by_label("IMU[1].GyrX (rad/s)");
            assert!(window.query_by_label("utc").is_none(), "no GPS clock");
            // the view shows the whole log, so there is no range to cut to
            assert!(
                window
                    .get_by_label("Only the time range in view")
                    .accesskit_node()
                    .is_disabled()
            );
            assert!(
                !window
                    .get_by_label("Save as\u{2026}")
                    .accesskit_node()
                    .is_disabled()
            );
        });
        // with nothing left showing, the window offers no save
        let roll = key("ATT", "Roll", None);
        let gyr = key("IMU", "GyrX", Some(1));
        app.toggle_series(roll.clone());
        app.toggle_series(gyr.clone());
        with_ui(&mut app, |harness| {
            assert!(
                harness
                    .get_by_label("Save as\u{2026}")
                    .accesskit_node()
                    .is_disabled()
            );
            harness.get_by_label("Cancel").click();
            harness.run();
            assert!(harness.query_by_label("Export CSV").is_none());
        });
        assert!(app.csv_dialog.is_none());

        // a job underneath the window closes it
        app.csv_dialog = Some(CsvDialog::default());
        let path = app.settings.recent[0].clone();
        app.open_path(&path, &ctx);
        with_ui(&mut app, |harness| {
            assert!(harness.query_by_label("Export CSV").is_none());
        });
        assert!(app.csv_dialog.is_none());
        wait(&mut app, &ctx);
    }

    #[test]
    fn the_showing_series_export_to_csv_as_the_plot_has_them() {
        use crate::model::SeriesKey;

        let dir = tempfile::tempdir().unwrap();
        let mut app = opened(dir.path());
        let ctx = Context::default();
        let key = |type_name: &str, field: &str, instance| SeriesKey {
            type_name: type_name.into(),
            field: field.into(),
            instance,
        };
        // Roll is a scaled integer field, GyrX a float32 one
        app.toggle_series(key("ATT", "Roll", None));
        app.toggle_series(key("IMU", "GyrX", Some(1)));
        with_ui(&mut app, |_| {});

        let export = csv_export(app.log().unwrap(), app.plot(), None);
        assert_eq!(
            export.header(),
            ["time_s", "ATT.Roll (deg)", "IMU[1].GyrX (rad/s)"]
        );
        assert_eq!(
            export.columns.iter().map(|c| c.single).collect::<Vec<_>>(),
            [false, true]
        );
        let out = dir.path().join("out").join("flight.csv");
        std::fs::create_dir(out.parent().unwrap()).unwrap();
        app.export_csv_to(export.clone(), out.clone(), &ctx);
        assert_eq!(app.job.as_ref().unwrap().label, "Writing flight.csv");
        wait(&mut app, &ctx);
        assert_eq!(
            app.notice,
            Some(Notice {
                text: "Wrote flight.csv: 8 rows, 2 series".into(),
                error: false,
            })
        );
        assert_eq!(app.settings.export_dir.as_deref(), out.parent());
        // the gyro samples 10 us after each attitude sample
        assert_eq!(
            std::fs::read_to_string(&out).unwrap(),
            "time_s,ATT.Roll (deg),IMU[1].GyrX (rad/s)\n\
             2.000000,0,\n\
             2.000010,,0.5\n\
             2.100000,1.5,\n\
             2.100010,,0.5\n\
             2.200000,3,\n\
             2.200010,,0.5\n\
             2.300000,4.5,\n\
             2.300010,,0.5\n"
        );
        with_ui(&mut app, |harness| {
            harness.get_by_label("Wrote flight.csv: 8 rows, 2 series");
        });

        // a file that cannot be written reports why, in the menu bar, and
        // its folder is not where the next dialog opens
        let bad = dir.path().join("no-such-folder").join("flight.csv");
        app.export_csv_to(export, bad.clone(), &ctx);
        wait(&mut app, &ctx);
        let notice = app.notice.clone().unwrap();
        assert!(notice.error);
        assert!(
            notice.text.starts_with(&bad.display().to_string()),
            "{}",
            notice.text
        );
        assert_eq!(app.settings.export_dir.as_deref(), out.parent());
        with_ui(&mut app, |harness| {
            harness.get_by_label(notice.text.as_str());
        });
    }

    #[test]
    fn one_job_runs_at_a_time() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("flight.bin");
        std::fs::write(&path, crate::model::testlog::bytes()).unwrap();
        let mut app = AftermissionApp::default();
        app.settings.online_tiles = false;
        let ctx = Context::default();
        app.open_path(&path, &ctx);
        let label = app.job.as_ref().unwrap().label.clone();

        // a second open while the first runs is refused, not queued
        let other = dir.path().join("other.bin");
        app.open_path(&other, &ctx);
        assert_eq!(
            app.job.as_ref().unwrap().label,
            label,
            "the first job runs on"
        );
        assert_eq!(
            app.notice,
            Some(Notice {
                text: format!("Still busy: {label}\u{2026}"),
                error: false,
            })
        );
        wait(&mut app, &ctx);
        assert_eq!(app.log().unwrap().name, "flight.bin");
        assert!(app.error.is_none());
        assert!(app.notice.is_none(), "the refusal goes with the job");
        with_ui(&mut app, |harness| {
            assert!(harness.query_by_label_contains("Still busy").is_none());
        });
    }

    #[test]
    fn a_running_job_shows_a_refusal_and_holds_back_the_export() {
        use crate::model::SeriesKey;

        let dir = tempfile::tempdir().unwrap();
        let mut app = opened(dir.path());
        let ctx = Context::default();
        app.toggle_series(SeriesKey {
            type_name: "ATT".into(),
            field: "Roll".into(),
            instance: None,
        });
        // a job that runs until the test lets it go
        let (release, held) = std::sync::mpsc::channel::<()>();
        let out = dir.path().join("held.csv");
        let job_out = out.clone();
        app.job = Some(Job::run(
            Kind::Export,
            out,
            "Writing held.csv".into(),
            ctx.clone(),
            move || {
                let _ = held.recv();
                Ok(Done::Exported {
                    path: job_out,
                    summary: "Released".into(),
                })
            },
        ));
        app.open_path(&dir.path().join("other.bin"), &ctx);

        // the spinner asks for frames without end, so each run stops at the
        // step limit rather than when the window is still
        let export_enabled = |harness: &mut Harness<'_>| {
            harness.get_by_label("File").click();
            harness.run_ok();
            harness.get_by_label("Export \u{23F5}").hover();
            harness.run_ok();
            !harness
                .get_by_label("Plotted series as CSV\u{2026}")
                .accesskit_node()
                .is_disabled()
        };
        let mut harness = Harness::builder()
            .with_size(egui::vec2(1400.0, 900.0))
            .build_ui(|ui| app.show(ui));
        harness.run_ok();
        // the refusal shows beside the job it waits for
        harness.get_by_label("Writing held.csv\u{2026}");
        harness.get_by_label("Still busy: Writing held.csv\u{2026}");
        assert!(!export_enabled(&mut harness), "not while a job runs");
        drop(harness);

        release.send(()).unwrap();
        wait(&mut app, &ctx);
        with_ui(&mut app, |harness| {
            harness.get_by_label("Released");
            assert!(export_enabled(harness), "once the job is done");
        });
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
