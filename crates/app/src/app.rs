//! The application: menus, the side panel with the log's types, the plot,
//! the map, the bottom panel with the events list and the parameter
//! table, the transport that plays the log back, the ways a log gets
//! opened, a second log compared with it, and the exports.
//!
//! The log under review is A. A second log, B, opens beside it through
//! Compare with…, and closes when a new A lands, so the two on screen are
//! always a pair chosen together.

#[cfg(not(target_arch = "wasm32"))]
use std::io::BufWriter;
use std::io::Write;
use std::ops::RangeInclusive;
#[cfg(not(target_arch = "wasm32"))]
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

use egui::{Align2, Color32, Context};

use crate::align::{self, Lineup};
use crate::csv::{self, Column, Digits, Export};
#[cfg(any(target_arch = "wasm32", test))]
use crate::demo;
use crate::events::EventsPanel;
use crate::filter::Filter;
use crate::map::MapPanel;
use crate::model::{LoadedLog, Scale};
use crate::paramfile::{self, ParamFile};
use crate::params::ParamsPanel;
#[cfg(feature = "parquet")]
use crate::parquetdir;
#[cfg(any(target_arch = "wasm32", test))]
use crate::picks::{Outcome, Picks};
use crate::playback::{self, Playback, Speed, Step};
use crate::plot::PlotPanel;
use crate::settings::{Align, BottomTab, Settings, TimeAxis};
use crate::timefmt;
use crate::tree;
use crate::worker::{self, Done, Job, Kind, Opened, Slot};

const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The commit CI built from, which the About dialog names beside the
/// version, so a page deployed between releases says which `main` it is.
const COMMIT: Option<&str> = option_env!("AFTERMISSION_COMMIT");

/// The About dialog's first line: the version, and the commit when CI
/// built it.
fn version_line(commit: Option<&str>) -> String {
    match commit {
        Some(commit) => format!("Aftermission {VERSION} ({commit})"),
        None => format!("Aftermission {VERSION}"),
    }
}

/// The export dialogs' button: a save dialog natively, a download in the
/// browser.
#[cfg(not(target_arch = "wasm32"))]
const SAVE_LABEL: &str = "Save as\u{2026}";
#[cfg(target_arch = "wasm32")]
const SAVE_LABEL: &str = "Download";

/// How an export's summary starts. Natively the file is in place when the
/// job ends; in the browser the page has handed it over, and whether the
/// browser saved it, or the user canceled a prompt for where, the page
/// cannot tell.
#[cfg(not(target_arch = "wasm32"))]
const DONE_VERB: &str = "Wrote";
#[cfg(target_arch = "wasm32")]
const DONE_VERB: &str = "Downloaded";

/// A log to open, waiting for its label to be painted before the scan
/// holds the page: the frame that arms it asks for a repaint, the next
/// counts down, the one after starts the job, so the label has been
/// presented when the page holds. It is presented twice when the arming
/// comes before the status line draws, as a menu item's or a pick's
/// does, and once when it comes after, as the empty window's button's
/// does. Frames are counted by egui's frame number, since a frame can
/// run the window several passes over, none of them presented.
#[cfg(any(target_arch = "wasm32", test))]
struct Armed {
    /// Frames still to paint before the scan starts.
    frames_left: u8,
    /// The frame the count last moved in.
    counted_frame: u64,
    pending: Pending,
}

/// What an armed scan opens.
#[cfg(any(target_arch = "wasm32", test))]
enum Pending {
    /// A file the browser read, for its slot.
    Bytes {
        name: String,
        bytes: Vec<u8>,
        slot: Slot,
    },
    /// The demo flight, generated once the scan starts, so that too runs
    /// under the label rather than in the frame that asked for it.
    Demo,
    /// The demo pair, both flights generated once the scan starts.
    Pair,
}

#[cfg(any(target_arch = "wasm32", test))]
impl Pending {
    /// What the status line says while it waits and while it runs.
    fn label(&self) -> String {
        match self {
            Self::Bytes { name, bytes, slot } => {
                worker::indexing_label(name, Some(bytes.len() as u64)) + slot.suffix()
            }
            Self::Demo => demo::LABEL.to_string(),
            Self::Pair => demo::PAIR_LABEL.to_string(),
        }
    }

    /// The slot the scan opens into; a pair opens from A.
    fn slot(&self) -> Slot {
        match self {
            Self::Bytes { slot, .. } => *slot,
            Self::Demo | Self::Pair => Slot::A,
        }
    }
}

/// What the page opens as it starts, as its address asks.
#[cfg(any(target_arch = "wasm32", test))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Startup {
    Nothing,
    /// The demo flight, for `?demo`.
    Demo,
    /// The demo pair, for `?compare`.
    Pair,
}

/// Log B, the one compared with the log under review, A, and how it
/// lines up with A.
#[derive(Debug)]
struct LogB {
    log: Arc<LoadedLog>,
    /// How B lines up under the alignment chosen: the offset worked out.
    lineup: Lineup,
    /// An offset set by hand, in seconds; None follows the lineup. It
    /// lasts until A or B is replaced or closed.
    by_hand: Option<f64>,
}

impl LogB {
    /// Seconds added to B's times to put them on A's clock.
    fn offset(&self) -> f64 {
        self.by_hand.unwrap_or(self.lineup.offset)
    }
}

/// What only B logs, worked out once for a pair, by the ids of its two
/// logs ([`LoadedLog::id`] never repeats), and kept between frames:
/// owned, since the logs are reached through `Arc`s cloned each frame.
#[derive(Debug)]
struct OnlyInB {
    pair: (u64, u64),
    types: Vec<tree::OnlyType>,
}

/// What the user asks of the pair in the row above the plot, applied
/// once the panels have drawn, so no frame draws half with one pair and
/// half with another.
#[derive(Debug, Clone, Copy, PartialEq)]
enum PairAction {
    CloseB,
    /// Line up the worked-out way chosen, which the settings keep.
    Align(Align),
    /// An offset set by hand, in seconds.
    ByHand(f64),
    /// Back from an offset set by hand to the one worked out.
    Reset,
    /// Review B as A and A as B.
    Swap,
}

#[cfg(any(target_arch = "wasm32", test))]
impl std::fmt::Debug for Armed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Armed")
            .field("frames_left", &self.frames_left)
            .field("counted_frame", &self.counted_frame)
            .field("pending", &self.pending.label())
            .finish()
    }
}

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

/// The choices made before a `.param` export.
#[derive(Debug, Default)]
struct ParamDialog {
    /// The values at boot rather than the last ones in the log.
    boot: bool,
}

/// The choices made before a Parquet export.
#[cfg(feature = "parquet")]
#[derive(Debug, Default)]
struct ParquetDialog {
    /// One file per instance value of a type with instances.
    split: bool,
}

#[derive(Debug, Default)]
pub struct AftermissionApp {
    settings: Settings,
    /// A, the log under review. Shared with an export job that reads the
    /// whole log on a worker.
    log: Option<Arc<LoadedLog>>,
    /// B, the log compared with A, while one is open.
    log_b: Option<LogB>,
    /// What only B logs, which the side panel lists under A's types,
    /// kept for the pair it was worked out for.
    only_in_b: Option<OnlyInB>,
    /// The one job running: an open or an export.
    job: Option<Job>,
    /// Why the last open failed, until the next one starts.
    error: Option<String>,
    /// Until the next job starts.
    notice: Option<Notice>,
    /// The CSV export's confirmation window, while it is open.
    csv_dialog: Option<CsvDialog>,
    /// The `.param` export's confirmation window, while it is open.
    param_dialog: Option<ParamDialog>,
    /// The Parquet export's confirmation window, while it is open.
    #[cfg(feature = "parquet")]
    parquet_dialog: Option<ParquetDialog>,
    plot: PlotPanel,
    /// Whether the log plays; the playhead is the plot's.
    playback: Playback,
    map: MapPanel,
    events: EventsPanel,
    params: ParamsPanel,
    /// The side panel's type and field filter.
    filter: String,
    /// Whether the filter was in use last frame: the headers it unfolded
    /// fold again when it is cleared.
    filter_was_active: bool,
    about_open: bool,
    /// A close was held while an export ran; the next one closes.
    close_held: bool,
    /// The files the browser hands the page, picked or dropped.
    #[cfg(any(target_arch = "wasm32", test))]
    picks: Picks,
    /// A log about to be indexed: one the browser read, or the demo.
    #[cfg(any(target_arch = "wasm32", test))]
    armed: Option<Armed>,
}

impl AftermissionApp {
    /// The app with its saved settings, opening `initial` when the
    /// command line named a log.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn new(cc: &eframe::CreationContext<'_>, initial: Option<PathBuf>) -> Self {
        let mut app = Self::with_settings(cc);
        if let Some(path) = initial {
            app.open_path(&path, &cc.egui_ctx);
        }
        app
    }

    /// The app with its saved settings; the page has no command line,
    /// but its address can ask for the demo with `?demo`, or the demo
    /// pair with `?compare`.
    #[cfg(target_arch = "wasm32")]
    pub fn new(cc: &eframe::CreationContext<'_>, startup: Startup) -> Self {
        let mut app = Self::with_settings(cc);
        match startup {
            Startup::Nothing => {}
            Startup::Demo => app.arm_demo(&cc.egui_ctx),
            Startup::Pair => app.arm(Pending::Pair, &cc.egui_ctx),
        }
        app
    }

    fn with_settings(cc: &eframe::CreationContext<'_>) -> Self {
        let app = Self {
            settings: Settings::load(cc.storage),
            ..Self::default()
        };
        // Our setting outranks the preference eframe restored with egui's memory.
        cc.egui_ctx.set_theme(app.settings.theme);
        app
    }

    /// Start opening `path` as A. The logs on screen stay until the new
    /// one is ready, and B closes when it lands. Refused while a job runs.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn open_path(&mut self, path: &Path, ctx: &Context) {
        if self.may_start() {
            self.job = Some(Job::open(path.to_path_buf(), Slot::A, ctx.clone()));
        }
    }

    /// Start opening `a` and `b` as a pair, A and B, in one job, as two
    /// dropped files do: B closes and opens anew with the new A, and a B
    /// that fails leaves A open alone. Refused while a job runs.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn open_pair(&mut self, a: &Path, b: &Path, ctx: &Context) {
        if self.may_start() {
            self.job = Some(Job::open_pair(
                a.to_path_buf(),
                b.to_path_buf(),
                ctx.clone(),
            ));
        }
    }

    /// Start opening `path` as B, beside A, replacing any B before it;
    /// A stays as it is. Refused while a job runs, and without an A to
    /// compare it with.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn compare_path(&mut self, path: &Path, ctx: &Context) {
        if self.log.is_some() && self.may_start() {
            self.job = Some(Job::open(path.to_path_buf(), Slot::B, ctx.clone()));
        }
    }

    /// Whether an open may start: not while a job runs or a scan is
    /// armed, which [`Self::busy`] says with a notice. One that may
    /// clears the last open's error.
    fn may_start(&mut self) -> bool {
        if self.busy() {
            return false;
        }
        self.error = None;
        true
    }

    /// Start opening the demo flight: natively as a job, so the window
    /// never waits for it; in the browser armed like a picked file, so
    /// its label paints before the page holds. Refused while a job runs.
    fn open_demo(&mut self, ctx: &Context) {
        #[cfg(not(target_arch = "wasm32"))]
        if self.may_start() {
            self.job = Some(Job::open_demo(ctx.clone()));
        }
        #[cfg(target_arch = "wasm32")]
        self.arm_demo(ctx);
    }

    /// Start opening the demo pair, the demo flight as A and the second
    /// sortie as B, as the demo is opened.
    fn open_demo_pair(&mut self, ctx: &Context) {
        #[cfg(not(target_arch = "wasm32"))]
        if self.may_start() {
            self.job = Some(Job::open_demo_pair(ctx.clone()));
        }
        #[cfg(target_arch = "wasm32")]
        self.arm(Pending::Pair, ctx);
    }

    /// Close B, as the button above the plot and File > Close B do. Not
    /// while a job runs or a file is read, as B does not open then either.
    fn close_b(&mut self, ctx: &Context) {
        if self.pair_busy().is_none() && self.log_b.take().is_some() {
            tracing::info!("log B closed");
            self.b_changed();
            self.retitle(ctx);
        }
    }

    /// What follows B's arrival or departure: each series takes B's side
    /// or loses it, the events list forgets its highlight, which would
    /// name another event of this B, the map's view takes in both tracks
    /// or A's alone again, and the list of what only B logs is worked out
    /// anew for the pair, or goes.
    fn b_changed(&mut self) {
        self.sync_b();
        self.events.reload();
        self.map.refit();
        self.only_in_b = None;
    }

    /// Seek to a time of A's, or to one of B's moved onto A's clock by
    /// the offset and kept within A's range, so the playhead never leaves
    /// A's log: what a click on B's track or on a B event asks.
    fn seek_to(&mut self, slot: Slot, time: f64) {
        let time = match slot {
            Slot::A => time,
            Slot::B => {
                let (Some(b), Some(range)) = (&self.log_b, self.time_range()) else {
                    return;
                };
                (time + b.offset()).clamp(*range.start(), *range.end())
            }
        };
        self.plot.seek(time);
    }

    /// Give the plot B and its offset as they are now, so the readout it
    /// refreshes before it draws reads them.
    fn sync_b(&mut self) {
        let b = self
            .log_b
            .as_ref()
            .map(|b| (Arc::clone(&b.log), b.offset()));
        self.plot.set_b(b);
    }

    /// Do what the row above the plot asked, once the panels have drawn.
    fn apply(&mut self, action: PairAction, ctx: &Context) {
        match action {
            PairAction::CloseB => self.close_b(ctx),
            PairAction::Swap => self.swap(ctx),
            PairAction::Align(chosen) => {
                self.settings.align = chosen;
                if let (Some(a), Some(b)) = (&self.log, &mut self.log_b) {
                    b.lineup = align::line_up(a, &b.log, chosen);
                    b.by_hand = None;
                }
                self.sync_b();
            }
            // a typed inf or NaN is no offset: the field shows the last one
            PairAction::ByHand(offset) if !offset.is_finite() => {}
            PairAction::ByHand(offset) => {
                if let Some(b) = &mut self.log_b {
                    b.by_hand = Some(offset);
                }
                self.sync_b();
            }
            PairAction::Reset => {
                if let Some(b) = &mut self.log_b {
                    b.by_hand = None;
                }
                self.sync_b();
            }
        }
    }

    /// Why B may not open or close now, for the items that wait: while a
    /// job runs or a file is being read, a B could land in a slot that
    /// changed meaning under it. None when it may.
    fn pair_busy(&self) -> Option<&'static str> {
        #[cfg(any(target_arch = "wasm32", test))]
        let reading = self.picks.reading().is_some();
        #[cfg(not(any(target_arch = "wasm32", test)))]
        let reading = false;
        if self.running().is_some() {
            Some(WAIT_JOB)
        } else if reading {
            Some("Wait for the file being read")
        } else {
            None
        }
    }

    /// Name the window after the logs open.
    fn retitle(&self, ctx: &Context) {
        if let Some(log) = &self.log {
            let b = self.log_b.as_ref().map(|b| b.log.name.as_str());
            set_title(ctx, &window_title(&log.name, b));
        }
    }

    /// Arm a scan: the job starts two frames on, its label shown from
    /// this frame or the next; see [`Armed`]. Refused while a job runs or
    /// a scan is armed, as an open by path is. A scan for A drops any
    /// file the browser is still reading: it is the newer choice, as a
    /// newer pick is, and a B would land beside the wrong A.
    #[cfg(any(target_arch = "wasm32", test))]
    fn arm(&mut self, pending: Pending, ctx: &Context) {
        if !self.may_start() {
            return;
        }
        if pending.slot() == Slot::A {
            self.picks.drop_reading();
        }
        self.armed = Some(Armed {
            frames_left: 2,
            counted_frame: ctx.cumulative_frame_nr(),
            pending,
        });
        ctx.request_repaint();
    }

    /// Arm the demo flight, as the browser's menu item and `?demo` do.
    #[cfg(any(target_arch = "wasm32", test))]
    fn arm_demo(&mut self, ctx: &Context) {
        self.arm(Pending::Demo, ctx);
    }

    /// One job at a time: while one runs, the next is refused with a
    /// notice beside the spinner, not queued; the notice goes when the job
    /// does. A job that may start clears the last notice.
    fn busy(&mut self) -> bool {
        if let Some(label) = self.running() {
            // a held close keeps its warning, which the refusal would hide
            if !self.close_held {
                self.notice = Some(Notice {
                    text: format!("Still busy: {label}\u{2026}"),
                    error: false,
                });
            }
            return true;
        }
        self.notice = None;
        false
    }

    /// The label of the work under way, a job's or an armed scan's, which
    /// the status line shows and the next job waits on.
    fn running(&self) -> Option<String> {
        if let Some(job) = &self.job {
            return Some(job.label.clone());
        }
        #[cfg(any(target_arch = "wasm32", test))]
        if let Some(armed) = &self.armed {
            return Some(armed.pending.label());
        }
        None
    }

    /// Act on what a pick or a drop came to: arm the scan of a file
    /// read, or say why there is none.
    #[cfg(any(target_arch = "wasm32", test))]
    fn poll_picks(&mut self, ctx: &Context) {
        let Some(outcome) = self.picks.poll() else {
            return;
        };
        // one outcome a frame: another message may wait behind this one,
        // and the repaint its sender asked for was spent on this frame. A
        // message shown usually asks egui for a frame anyway; this does not
        // rely on the screen changing.
        ctx.request_repaint();
        match outcome {
            Outcome::File { name, bytes, slot } => {
                self.arm(Pending::Bytes { name, bytes, slot }, ctx);
            }
            Outcome::Unreadable {
                name,
                dropped,
                slot,
            } => {
                // a dropped folder arrives as an entry with no file behind
                // it; the chooser takes files only
                let hint = if dropped {
                    " A folder cannot be opened; drop the log file itself."
                } else {
                    ""
                };
                self.error =
                    Some(slot.error(&format!("{name}: the browser could not read it.{hint}")));
            }
            Outcome::Refused { slot } => {
                let what = match slot {
                    Slot::A => "",
                    Slot::B => " for B",
                };
                self.error = Some(format!(
                    "The browser did not open the file chooser{what}. Try again."
                ));
            }
        }
    }

    /// Count the armed scan down and start it once its label has been
    /// painted; see [`Armed`].
    #[cfg(any(target_arch = "wasm32", test))]
    fn tick_armed(&mut self, ctx: &Context) {
        let Some(armed) = &mut self.armed else {
            return;
        };
        let frame = ctx.cumulative_frame_nr();
        if frame == armed.counted_frame {
            // another pass of the frame already counted
            ctx.request_repaint();
            return;
        }
        armed.counted_frame = frame;
        if armed.frames_left > 1 {
            armed.frames_left -= 1;
            ctx.request_repaint();
            return;
        }
        let Some(Armed { pending, .. }) = self.armed.take() else {
            return;
        };
        self.job = Some(match pending {
            Pending::Bytes { name, bytes, slot } => Job::open_bytes(name, bytes, slot, ctx.clone()),
            Pending::Demo => Job::open_demo(ctx.clone()),
            Pending::Pair => Job::open_demo_pair(ctx.clone()),
        });
    }

    /// Whether to hold a request to close the window. Ending the process
    /// while an export writes would leave its Parquet folder unfinished, or
    /// a CSV or `.param` export's `.tmp` file beside its target and the
    /// target not written, so the first request is held with a notice; a
    /// second closes anyway, so an export that never ends cannot keep the
    /// window open. Opening a log writes nothing and never holds it.
    fn hold_close(&mut self) -> bool {
        let Some(job) = &self.job else {
            return false;
        };
        if job.kind != Kind::Export || self.close_held {
            return false;
        }
        self.close_held = true;
        self.notice = Some(Notice {
            text: format!("{}\u{2026} Close again to quit anyway", job.label),
            error: true,
        });
        true
    }

    /// The loaded log, for tests.
    #[cfg(test)]
    pub(crate) fn log(&self) -> Option<&LoadedLog> {
        self.log.as_deref()
    }

    /// Log B, for tests.
    #[cfg(test)]
    pub(crate) fn log_b(&self) -> Option<&LoadedLog> {
        self.log_b.as_ref().map(|b| &*b.log)
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
        // the next close during another export is held again
        self.close_held = false;
        // a refusal made while the job ran is over with it
        self.notice = None;
        match result {
            Ok(Done::Opened { path, log, b }) => {
                if kind == Kind::Open(Slot::B) {
                    self.land_b(Opened { path, log });
                } else {
                    self.land_a(path, log, b);
                }
                self.retitle(ctx);
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
            Err(message) => match kind {
                // the logs on screen stay as they were
                Kind::Open(slot) => {
                    tracing::warn!(%message, ?slot, "cannot open the log");
                    self.error = Some(slot.error(&message));
                }
                Kind::Export => {
                    tracing::warn!(%message, "the export failed");
                    self.notice = Some(Notice {
                        text: message,
                        error: true,
                    });
                }
            },
        }
    }

    /// A new A, opened: every panel starts over on it, and B closes, so
    /// the two on screen are a pair chosen together. For a pair, its B
    /// lands beside it, or its error names B.
    fn land_a(
        &mut self,
        path: Option<PathBuf>,
        log: Box<LoadedLog>,
        b: Option<Result<Opened, String>>,
    ) {
        tracing::info!(name = %log.name, records = log.records(), "log opened");
        self.start_over();
        self.plot.reload(&log);
        self.log = Some(Arc::from(log));
        self.log_b = None;
        self.only_in_b = None;
        match b {
            Some(Ok(b)) => self.land_b(b),
            Some(Err(message)) => {
                tracing::warn!(%message, "cannot open the pair's B");
                self.error = Some(Slot::B.error(&message));
            }
            None => {}
        }
        // only a log that opened from a file is worth offering again: the
        // browser's have no path, and the demo needs none. A's goes last,
        // at the top of the list.
        if let Some(path) = path {
            self.settings.remember(&path);
        }
    }

    /// Every panel but the plot starts over on a new A: playback pauses,
    /// the map fits the tracks again, the events and the parameters forget
    /// what they held, and an export set up before goes.
    fn start_over(&mut self) {
        // the log that played is gone
        self.playback.pause();
        self.map.reload();
        self.params.reload();
        self.events.reload();
        // an export set up on the log before would write the wrong one
        self.csv_dialog = None;
        self.param_dialog = None;
        #[cfg(feature = "parquet")]
        self.parquet_dialog.take();
    }

    /// B, opened beside A, replacing any B before it, and lined up with A
    /// the way the settings keep. Nothing else moves: the playhead, the
    /// view and the series stay as they were, and each series takes B's
    /// side.
    fn land_b(&mut self, Opened { path, log }: Opened) {
        let Some(a) = &self.log else {
            // nothing opens B without an A to compare it with
            tracing::warn!(name = %log.name, "a log B with no A; dropped");
            return;
        };
        let lineup = align::line_up(a, &log, self.settings.align);
        // offered again as any log is, and opened from there as A
        if let Some(path) = path {
            self.settings.remember(&path);
        }
        tracing::info!(name = %log.name, records = log.records(), "log B opened");
        self.log_b = Some(LogB {
            log: Arc::from(log),
            lineup,
            by_hand: None,
        });
        self.b_changed();
    }

    /// Review B as A and A as B, as the row's Swap does for a pair that
    /// came the other way round, as two dropped files can. The other
    /// panels start over on the new A, as on any new A; the plot keeps its
    /// series and its moment in view, and an offset set by hand stays with
    /// the pair, turned round, so a second swap undoes the first. Not
    /// while a job runs or a file is read, as B does not open or close
    /// then either.
    fn swap(&mut self, ctx: &Context) {
        if self.pair_busy().is_some() {
            return;
        }
        let (Some(a), Some(b)) = (&self.log, &self.log_b) else {
            return;
        };
        let (a, b, by_hand) = (Arc::clone(a), Arc::clone(&b.log), b.by_hand);
        tracing::info!(a = %b.name, b = %a.name, "logs A and B swapped");
        let pair = LogB {
            lineup: align::line_up(&b, &a, self.settings.align),
            log: a,
            by_hand: by_hand.map(|offset| -offset),
        };
        self.start_over();
        self.plot.swap(&b, Arc::clone(&pair.log), pair.offset());
        self.log = Some(b);
        self.log_b = Some(pair);
        self.retitle(ctx);
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn handle_drop(&mut self, ctx: &Context) {
        let dropped: Vec<PathBuf> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .map(|f| f.path().to_path_buf())
                .collect()
        });
        // two files dropped at once open as a pair, the first as A and the
        // second as B; more than two, the first two
        match dropped.as_slice() {
            [] => {}
            [a] => self.open_path(a, ctx),
            [a, b, ..] => self.open_pair(a, b, ctx),
        }
    }

    /// A drop in the browser is a file to read, not a path: the first
    /// dropped file becomes a numbered pick for A, read in the background.
    #[cfg(target_arch = "wasm32")]
    fn handle_drop(&mut self, ctx: &Context) {
        let first = ctx.input(|i| i.raw.dropped_files.first().cloned());
        if let Some(handle) = first {
            let (pick, tx) = self.picks.start(Slot::A);
            crate::picks::read_dropped(pick, handle, tx, ctx.clone());
        }
    }

    fn shortcuts(&mut self, ctx: &Context) {
        if self.open_key(ctx) {
            self.open_dialog(ctx);
        }
        self.transport_keys(ctx);
    }

    /// Whether Ctrl+O asks for the open dialog: pressed, and no job runs,
    /// as File > Open waits, since a job on a large log runs for minutes
    /// and a choice refused after its dialog is lost. The key is taken
    /// either way.
    fn open_key(&self, ctx: &Context) -> bool {
        ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::O))
            && self.running().is_none()
    }

    /// Space plays and pauses, Left and Right step a second back and
    /// forward, ten with Shift, and `[` and `]` step the speed down and
    /// up, while no widget has the keyboard. egui gives it only to a text
    /// field that is clicked and to a widget reached with Tab, which take
    /// Space and the arrows as their own: a focused Play button plays by
    /// Space as its click. Not under an export's window either, where
    /// playback holds still.
    fn transport_keys(&mut self, ctx: &Context) {
        use egui::{Key, Modifiers};

        if self.time_range().is_none() || self.exporting() || ctx.egui_wants_keyboard_input() {
            return;
        }
        // Shift first: a key asked for without it matches with it too
        let step = |i: &mut egui::InputState, key| {
            if i.consume_key(Modifiers::SHIFT, key) {
                10.0
            } else if i.consume_key(Modifiers::NONE, key) {
                1.0
            } else {
                0.0
            }
        };
        let (toggle, back, on, slower, faster) = ctx.input_mut(|i| {
            (
                i.consume_key(Modifiers::NONE, Key::Space),
                step(i, Key::ArrowLeft),
                step(i, Key::ArrowRight),
                i.consume_key(Modifiers::NONE, Key::OpenBracket),
                i.consume_key(Modifiers::NONE, Key::CloseBracket),
            )
        });
        if toggle {
            self.toggle_playback();
        }
        if back + on != 0.0 {
            self.step_playhead(on - back);
        }
        if slower {
            self.settings.speed = self.settings.speed.slower();
        }
        if faster {
            self.settings.speed = self.settings.speed.faster();
        }
    }

    /// Whether an export's window is open. Playback holds still under
    /// it, so the time range in view that the CSV export offers stays
    /// the one the user saw.
    fn exporting(&self) -> bool {
        #[cfg(feature = "parquet")]
        let parquet = self.parquet_dialog.is_some();
        #[cfg(not(feature = "parquet"))]
        let parquet = false;
        self.csv_dialog.is_some() || self.param_dialog.is_some() || parquet
    }

    /// The time the open log plays through; None without a log or a
    /// timed record in it.
    fn time_range(&self) -> Option<RangeInclusive<f64>> {
        self.log.as_ref().and_then(|log| log.time_range())
    }

    /// Play or pause. Play goes on from the playhead, or starts at the
    /// log's first time when nothing has been played or sought yet, or
    /// when playing last ran to the end.
    fn toggle_playback(&mut self) {
        let Some(range) = self.time_range() else {
            return;
        };
        if self.playback.is_playing() {
            self.playback.pause();
            return;
        }
        let from = playback::start_from(self.plot.playhead(), &range);
        if self.plot.playhead() != Some(from) {
            self.plot.seek(from);
        }
        self.playback.play();
    }

    /// Move the playhead `seconds` forward, or back when negative, within
    /// the log's time, playing or paused; from the log's first time when
    /// it has not moved yet.
    fn step_playhead(&mut self, seconds: f64) {
        let Some(range) = self.time_range() else {
            return;
        };
        let (start, end) = (*range.start(), *range.end());
        let from = self.plot.playhead().unwrap_or(start);
        self.plot.seek((from + seconds).clamp(start, end));
    }

    /// While playing, move the playhead by the time since the last frame
    /// and ask for the next one; under an export's window, pause.
    fn advance_playback(&mut self, ctx: &Context) {
        if !self.playback.is_playing() {
            return;
        }
        let range = self.time_range().filter(|_| !self.exporting());
        let (Some(range), Some(playhead)) = (range, self.plot.playhead()) else {
            self.playback.pause();
            return;
        };
        let now = ctx.input(|i| i.time);
        let frame = ctx.cumulative_frame_nr();
        if let Some(Step::Moved(time) | Step::Ended(time)) =
            self.playback
                .advance(self.settings.speed, now, frame, playhead, &range)
        {
            self.plot.play_to(time);
        }
        if self.playback.is_playing() {
            ctx.request_repaint();
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn open_dialog(&mut self, ctx: &Context) {
        let picked = rfd::FileDialog::new()
            .add_filter("Dataflash log", &["bin", "BIN"])
            .pick_file();
        if let Some(path) = picked {
            self.open_path(&path, ctx);
        }
    }

    /// The browser's file chooser, as a numbered pick; what it yields
    /// arrives through [`Self::poll_picks`].
    #[cfg(target_arch = "wasm32")]
    fn open_dialog(&mut self, ctx: &Context) {
        let (pick, tx) = self.picks.start(Slot::A);
        crate::picks::pick_file(pick, &tx, ctx);
    }

    /// Ask for the log to open as B.
    #[cfg(not(target_arch = "wasm32"))]
    fn compare_dialog(&mut self, ctx: &Context) {
        let picked = rfd::FileDialog::new()
            .set_title("Compare with")
            .add_filter("Dataflash log", &["bin", "BIN"])
            .pick_file();
        if let Some(path) = picked {
            self.compare_path(&path, ctx);
        }
    }

    /// The browser's file chooser for B, as a numbered pick.
    #[cfg(target_arch = "wasm32")]
    fn compare_dialog(&mut self, ctx: &Context) {
        let (pick, tx) = self.picks.start(Slot::B);
        crate::picks::pick_file(pick, &tx, ctx);
    }

    /// The whole window.
    pub fn show(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        self.poll(&ctx);
        #[cfg(any(target_arch = "wasm32", test))]
        self.tick_armed(&ctx);
        #[cfg(any(target_arch = "wasm32", test))]
        self.poll_picks(&ctx);
        if ctx.input(|i| i.viewport().close_requested()) && self.hold_close() {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
        }
        self.handle_drop(&ctx);
        self.shortcuts(&ctx);
        self.advance_playback(&ctx);

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
        self.param_dialog_ui(&ctx);
        #[cfg(feature = "parquet")]
        self.parquet_dialog_ui(&ctx);
        Self::drop_overlay(&ctx);
    }

    fn file_menu(&mut self, ui: &mut egui::Ui) {
        // Nothing opens while a job runs: the job would refuse it after the
        // dialog, with the choice lost. B waits for a file being read too.
        let idle = self.running().is_none();
        let pair_busy = self.pair_busy();
        let has_log = self.log.is_some();
        ui.menu_button("File", |ui| {
            if ui
                .add_enabled(idle, egui::Button::new("Open\u{2026}"))
                .on_disabled_hover_text(WAIT_JOB)
                .clicked()
            {
                ui.close();
                self.open_dialog(ui.ctx());
            }
            if ui
                .add_enabled(
                    has_log && pair_busy.is_none(),
                    egui::Button::new("Compare with\u{2026}"),
                )
                .on_hover_text("Open a second log as B, beside this one")
                .on_disabled_hover_text(pair_busy.unwrap_or("Open a log first"))
                .clicked()
            {
                ui.close();
                self.compare_dialog(ui.ctx());
            }
            if ui
                .add_enabled(idle, egui::Button::new("Open demo log"))
                .on_disabled_hover_text(WAIT_JOB)
                .clicked()
            {
                ui.close();
                self.open_demo(ui.ctx());
            }
            if ui
                .add_enabled(idle, egui::Button::new("Open demo pair"))
                .on_hover_text("The demo flight as A, and a second sortie as B to compare it with")
                .on_disabled_hover_text(WAIT_JOB)
                .clicked()
            {
                ui.close();
                self.open_demo_pair(ui.ctx());
            }
            // the browser has no path to offer again
            #[cfg(not(target_arch = "wasm32"))]
            let recent = self.settings.recent.clone();
            #[cfg(not(target_arch = "wasm32"))]
            ui.add_enabled_ui(!recent.is_empty() && idle, |ui| {
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
            if self.log_b.is_some()
                && ui
                    .add_enabled(pair_busy.is_none(), egui::Button::new("Close B"))
                    .on_disabled_hover_text(pair_busy.unwrap_or_default())
                    .clicked()
            {
                ui.close();
                self.close_b(ui.ctx());
            }
            ui.separator();
            self.export_menu(ui, idle);
            // a page has no window to close
            #[cfg(not(target_arch = "wasm32"))]
            {
                ui.separator();
                if ui.button("Quit").clicked() {
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
        });
    }

    /// Why the CSV export has nothing to write: B's lines alone are
    /// plotted, which no export writes, or no line of A's shows.
    fn unplotted(&self) -> &'static str {
        if self.plot.selected.is_empty() || self.plot.has_a_series() {
            "Plot a series first"
        } else {
            "B's lines are not exported"
        }
    }

    /// The exports of A, none while a job runs: the export would be
    /// refused after the save dialog, with the chosen path lost.
    fn export_menu(&mut self, ui: &mut egui::Ui, idle: bool) {
        let plotted = self.log.is_some() && self.plot.showing().next().is_some();
        let unplotted = self.unplotted();
        let has_params = self.log.as_ref().is_some_and(|l| !l.params.is_empty());
        #[cfg(feature = "parquet")]
        let has_records = self.log.as_ref().is_some_and(|l| l.parquet_counts().1 > 0);
        let has_log = self.log.is_some();
        let why = |reason| disabled_reason(idle, has_log, reason);
        ui.menu_button("Export", |ui| {
            if ui
                .add_enabled(
                    plotted && idle,
                    egui::Button::new("Plotted series as CSV\u{2026}"),
                )
                .on_disabled_hover_text(why(unplotted))
                .clicked()
            {
                ui.close();
                self.csv_dialog = Some(CsvDialog::default());
            }
            if ui
                .add_enabled(
                    has_params && idle,
                    egui::Button::new("Parameters as .param\u{2026}"),
                )
                .on_disabled_hover_text(why("The log has no parameters"))
                .clicked()
            {
                ui.close();
                self.param_dialog = Some(ParamDialog::default());
            }
            #[cfg(feature = "parquet")]
            if ui
                .add_enabled(
                    has_records && idle,
                    egui::Button::new("Whole log as Parquet\u{2026}"),
                )
                .on_disabled_hover_text(why("The log has no records"))
                .clicked()
            {
                ui.close();
                self.parquet_dialog = Some(ParquetDialog::default());
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
        if let Some(label) = self.running() {
            ui.spinner();
            ui.label(format!("{label}\u{2026}"));
        }
        #[cfg(any(target_arch = "wasm32", test))]
        if let Some((name, slot)) = self.picks.reading() {
            ui.spinner();
            ui.label(format!("Reading {name}{}\u{2026}", slot.suffix()));
        }
        if let Some(notice) = &self.notice {
            if notice.error {
                ui.colored_label(ui.visuals().error_fg_color, &notice.text);
            } else {
                ui.label(&notice.text);
            }
        }
    }

    /// Keep the list of what only B logs for the pair on screen: worked
    /// out when the pair's ids are not those it was worked out for, as
    /// after a swap, a new B or a new A, and gone with B.
    fn keep_only_in_b(&mut self) {
        let (Some(a), Some(b)) = (&self.log, &self.log_b) else {
            self.only_in_b = None;
            return;
        };
        let pair = (a.id, b.log.id);
        if self.only_in_b.as_ref().is_none_or(|kept| kept.pair != pair) {
            self.only_in_b = Some(OnlyInB {
                pair,
                types: tree::only_in(&b.log, a),
            });
        }
    }

    fn side_ui(&mut self, ui: &mut egui::Ui) {
        self.keep_only_in_b();
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
        let shown = tree::types(ui, log, &mut self.plot, &filter, open);
        let shown_b = self.only_in_b.as_ref().map_or(0, |only| {
            tree::b_types(ui, &only.types, &mut self.plot, &filter, open)
        });
        if shown + shown_b == 0 {
            ui.weak("Nothing matches the filter.");
        }
    }

    fn central_ui(&mut self, ui: &mut egui::Ui) {
        // a handle of its own, so the transport can play and seek while
        // the panels read the log
        let Some(log) = self.log.clone() else {
            let idle = self.running().is_none();
            ui.vertical_centered(|ui| {
                ui.add_space(ui.available_height() * 0.3);
                ui.heading("Open a log to review");
                ui.label("Drop an ArduPilot .bin file here, press Ctrl+O, or try the demo.");
                if ui
                    .add_enabled(idle, egui::Button::new("Open\u{2026}"))
                    .on_disabled_hover_text(WAIT_JOB)
                    .clicked()
                {
                    self.open_dialog(ui.ctx());
                }
                if ui
                    .add_enabled(idle, egui::Button::new("Demo"))
                    .on_hover_text("An invented survey flight, generated here")
                    .on_disabled_hover_text(WAIT_JOB)
                    .clicked()
                {
                    self.open_demo(ui.ctx());
                }
            });
            return;
        };
        let log = &log;
        let b = self.log_b.as_ref().map(|b| Arc::clone(&b.log));
        let b = b.as_deref();
        // B with its offset onto A's clock, for the map and the events
        let b_moved = b.zip(self.log_b.as_ref().map(LogB::offset));
        let pair_busy = self.pair_busy();
        let axis = self.settings.time_axis;
        let playing = self.playback.is_playing();
        // The map to the right and the events or parameters below share
        // the width and height with the plot, which takes what is left;
        // the transport sits between the plot and the events.
        if self.settings.show_map {
            egui::Panel::right("map_panel")
                .default_size(420.0)
                .show(ui, |ui| {
                    let cursor = self.plot.cursor;
                    let online = self.settings.online_tiles;
                    let follow = &mut self.settings.follow_map;
                    if let Some((slot, time)) = self
                        .map
                        .show(ui, log, b_moved, cursor, axis, online, follow)
                    {
                        self.seek_to(slot, time);
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
                            let cursor = self.plot.cursor;
                            let follow = self.plot.follow_mut();
                            let seek = self.events.show(
                                ui,
                                log,
                                b_moved,
                                &self.settings,
                                cursor,
                                follow,
                                playing,
                            );
                            if let Some((slot, time)) = seek {
                                // the clicked row is in view: the list does
                                // not follow its own click, which would scroll
                                // to the last of rows sharing its time
                                self.seek_to(slot, time);
                                *self.plot.follow_mut() = false;
                            }
                            None
                        }
                        BottomTab::Parameters => self.params.show(ui, log, b, &self.settings),
                    };
                    if let Some(time) = seek {
                        self.plot.seek(time);
                    }
                });
        }
        egui::Panel::bottom("transport")
            .resizable(false)
            .show(ui, |ui| self.transport_ui(ui, log));
        let action = self
            .log_b
            .as_ref()
            .and_then(|b| pair_row(ui, log, b, pair_busy, axis));
        self.plot.show(ui, log, &self.settings, playing);
        if let Some(action) = action {
            self.apply(action, ui.ctx());
        }
    }

    /// The transport: play and pause, a step back and one forward, the
    /// playhead's time since the log's first and the log's length, the
    /// time of day on the UTC axis, a slider to scrub, and the speed.
    /// Disabled for a log without a timed record, and under an export's
    /// window.
    fn transport_ui(&mut self, ui: &mut egui::Ui, log: &LoadedLog) {
        let range = log.time_range();
        ui.add_enabled_ui(range.is_some() && !self.exporting(), |ui| {
            ui.horizontal(|ui| {
                let (icon, tip) = if self.playback.is_playing() {
                    ("\u{23F8}", "Pause (Space)")
                } else {
                    ("\u{23F5}", "Play (Space)")
                };
                if ui.button(icon).on_hover_text(tip).clicked() {
                    self.toggle_playback();
                }
                let by = if ui.input(|i| i.modifiers.shift) {
                    10.0
                } else {
                    1.0
                };
                if ui
                    .button("\u{23EA}")
                    .on_hover_text("Back 1 s, with Shift 10 s (Left)")
                    .clicked()
                {
                    self.step_playhead(-by);
                }
                if ui
                    .button("\u{23E9}")
                    .on_hover_text("Forward 1 s, with Shift 10 s (Right)")
                    .clicked()
                {
                    self.step_playhead(by);
                }
                let Some(range) = range else {
                    ui.weak("No timed records");
                    return;
                };
                let (start, end) = (*range.start(), *range.end());
                let at = self.plot.playhead().unwrap_or(start);
                ui.monospace(format!(
                    "{} / {}",
                    timefmt::boot_time(at - start, 0.1),
                    timefmt::boot_time(end - start, 0.1)
                ));
                if let Some(base) = log.wall_clock(self.settings.time_axis) {
                    ui.monospace(timefmt::utc_time(base.wall_clock_unix_ms(at * 1000.0), 1.0));
                }
                // Follow and the speed at the right end, and the slider
                // across the rest
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.checkbox(&mut self.settings.follow_map, "Follow")
                        .on_hover_text(
                            "Keep the vehicle in view on the map; moving the map by hand turns it off",
                        );
                    egui::ComboBox::from_id_salt("playback_speed")
                        .width(60.0)
                        .selected_text(self.settings.speed.label())
                        .show_ui(ui, |ui| {
                            for speed in Speed::all() {
                                ui.selectable_value(&mut self.settings.speed, speed, speed.label());
                            }
                        })
                        .response
                        .on_hover_text("Speed ([ and ])");
                    ui.spacing_mut().slider_width = ui.available_width();
                    let mut time = at;
                    let slider = ui.add(
                        egui::Slider::new(&mut time, start..=end)
                            .show_value(false)
                            .trailing_fill(true),
                    );
                    // a drag pauses while it lasts; a click seeks
                    if slider.drag_started() {
                        self.playback.begin_scrub();
                    }
                    if slider.changed() {
                        self.plot.seek(time);
                    }
                    if slider.drag_stopped() {
                        self.playback.end_scrub();
                    }
                });
            });
        });
    }

    fn about_ui(&mut self, ctx: &Context) {
        egui::Window::new("About Aftermission")
            .open(&mut self.about_open)
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                ui.label(version_line(COMMIT));
                ui.label("Post-mission review of ArduPilot dataflash logs.");
                ui.label("Copyright 2026 Poholos. Licensed under the AGPL-3.0-only; commercial licenses available.");
                ui.hyperlink("https://github.com/Poholos/aftermission");
            });
    }

    /// The choices before a CSV export, then the save dialog.
    fn csv_dialog_ui(&mut self, ctx: &Context) {
        // a job started underneath, by a dropped file, would refuse the export
        if self.running().is_some() {
            self.csv_dialog = None;
        }
        let b_open = self.log_b.is_some();
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
                from_a(ui, b_open, log);
                ui.label("Columns:");
                for name in &header {
                    ui.monospace(name);
                }
                if b_open {
                    ui.weak("B's lines are left out: the file holds A's series");
                }
                ui.add_enabled(
                    zoomed.is_some(),
                    egui::Checkbox::new(&mut dialog.visible_only, "Only the time range in view"),
                )
                .on_disabled_hover_text("Zoom the plot to narrow the range");
                ui.horizontal(|ui| {
                    save = ui
                        .add_enabled(showing, egui::Button::new(SAVE_LABEL))
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
            self.csv_dialog = None;
            if let Some(path) = self.save_dialog("CSV", "csv", name) {
                self.export_csv_to(export, path, ctx);
            }
        }
    }

    /// The choices before a `.param` export, then the save dialog.
    fn param_dialog_ui(&mut self, ctx: &Context) {
        // a job started underneath, by a dropped file, would refuse the export
        if self.running().is_some() {
            self.param_dialog = None;
        }
        let b_open = self.log_b.is_some();
        let (Some(dialog), Some(log)) = (&mut self.param_dialog, &self.log) else {
            return;
        };
        // counted as the file will have them
        let (mut written, mut changed) = (0, 0);
        for p in paramfile::exportable(&log.params) {
            written += 1;
            changed += usize::from(!p.changes.is_empty());
        }
        let left_out = log.params.len() - written;
        if changed == 0 {
            dialog.boot = false;
        }
        let mut open = true;
        let mut save = false;
        let mut cancel = false;
        egui::Window::new("Export parameters")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                from_a(ui, b_open, log);
                ui.label(format!(
                    "{written} parameters, {changed} changed after boot"
                ));
                if left_out > 0 {
                    ui.colored_label(
                        ui.visuals().warn_fg_color,
                        format!("Left out, with a name no loader can read: {left_out}"),
                    );
                }
                ui.add_enabled(
                    changed > 0,
                    egui::Checkbox::new(&mut dialog.boot, "Boot values instead of last values"),
                )
                .on_disabled_hover_text("No parameter changed after boot");
                ui.horizontal(|ui| {
                    save = ui
                        .add_enabled(written > 0, egui::Button::new(SAVE_LABEL))
                        .on_disabled_hover_text("No parameter has a name a loader can read")
                        .clicked();
                    cancel = ui.button("Cancel").clicked();
                });
            });
        let boot = dialog.boot;
        if !open || cancel {
            self.param_dialog = None;
        }
        if save {
            let file = ParamFile::new(&log.name, &log.params, boot);
            let name = paramfile::file_name(&log.name, boot);
            self.param_dialog = None;
            if let Some(path) = self.save_dialog("Parameter file", "param", name) {
                self.export_params_to(file, path, ctx);
            }
        }
    }

    /// The choices before a Parquet export, then the folder picker.
    #[cfg(feature = "parquet")]
    fn parquet_dialog_ui(&mut self, ctx: &Context) {
        // a job started underneath, by a dropped file, would refuse the export
        if self.running().is_some() {
            self.parquet_dialog = None;
        }
        let b_open = self.log_b.is_some();
        let (Some(dialog), Some(log)) = (&mut self.parquet_dialog, &self.log) else {
            return;
        };
        // with no type logged as two instances, splitting would only rename
        // the files of types with an instance field, `GPS` to `GPS_0`
        let instanced = log.types.iter().any(|t| !t.instances.is_empty());
        if !instanced {
            dialog.split = false;
        }
        let mut open = true;
        let mut save = false;
        let mut cancel = false;
        egui::Window::new("Export Parquet")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                from_a(ui, b_open, log);
                let (types, records) = log.parquet_counts();
                ui.label(format!("{types} message types, {records} records"));
                ui.label(format!(
                    "One file per type, in a new folder {} in the folder you choose, numbered when the name is taken",
                    parquetdir::folder_name(&log.name)
                ));
                ui.add_enabled(
                    instanced,
                    egui::Checkbox::new(&mut dialog.split, "One file per instance"),
                )
                .on_disabled_hover_text("No message type has more than one instance");
                ui.horizontal(|ui| {
                    save = ui.button("Save to folder\u{2026}").clicked();
                    cancel = ui.button("Cancel").clicked();
                });
            });
        let split = dialog.split;
        if !open || cancel {
            self.parquet_dialog = None;
        }
        if save {
            self.parquet_dialog = None;
            if let Some(parent) = self.folder_dialog() {
                self.export_parquet_to(parent, split, ctx);
            }
        }
    }

    /// Ask for the folder a Parquet export makes its own folder in,
    /// starting in the last export folder.
    #[cfg(feature = "parquet")]
    fn folder_dialog(&self) -> Option<PathBuf> {
        let mut picker = rfd::FileDialog::new().set_title("Choose where the Parquet folder goes");
        if let Some(dir) = &self.settings.export_dir {
            picker = picker.set_directory(dir);
        }
        picker.pick_folder()
    }

    /// Write every message type of the log into a new folder in `parent`,
    /// on a worker, which shares the log rather than copying it. Refused
    /// while a job runs.
    #[cfg(feature = "parquet")]
    pub(crate) fn export_parquet_to(&mut self, parent: PathBuf, split: bool, ctx: &Context) {
        let Some(log) = &self.log else {
            return;
        };
        let log = Arc::clone(log);
        if self.busy() {
            return;
        }
        let label = format!("Writing Parquet files for {}", log.name);
        self.job = Some(Job::run(
            Kind::Export,
            parent.clone(),
            label,
            ctx.clone(),
            move || parquetdir::export(&log, &parent, split),
        ));
    }

    /// Ask where to save a file named `name`, starting in the last export
    /// folder.
    #[cfg(not(target_arch = "wasm32"))]
    fn save_dialog(&self, kind: &str, extension: &str, name: String) -> Option<PathBuf> {
        let mut picker = rfd::FileDialog::new()
            .add_filter(kind, &[extension])
            .set_file_name(name);
        if let Some(dir) = &self.settings.export_dir {
            picker = picker.set_directory(dir);
        }
        picker.save_file()
    }

    /// In the browser the file is a download: the name is the whole of
    /// the answer, and the browser asks where it goes if its settings say
    /// to.
    #[cfg(target_arch = "wasm32")]
    #[expect(
        clippy::unused_self,
        clippy::unnecessary_wraps,
        reason = "the same call as the native dialog, which starts in the last export folder and can be canceled"
    )]
    fn save_dialog(&self, _kind: &str, _extension: &str, name: String) -> Option<PathBuf> {
        Some(PathBuf::from(name))
    }

    /// Write `export` to `path` on a worker.
    pub(crate) fn export_csv_to(&mut self, export: Export, path: PathBuf, ctx: &Context) {
        self.write_file(path, ctx, move |mut out, name| {
            let rows = export.write(&mut out)?;
            Ok(format!(
                "{DONE_VERB} {name}: {rows} rows, {} series",
                export.columns.len()
            ))
        });
    }

    /// Write `file` to `path` on a worker.
    pub(crate) fn export_params_to(&mut self, file: ParamFile, path: PathBuf, ctx: &Context) {
        self.write_file(path, ctx, move |mut out, name| {
            file.write(&mut out)?;
            Ok(format!(
                "{DONE_VERB} {name}: {} parameters",
                file.values.len()
            ))
        });
    }

    /// Write `path` through `write` on a worker: into a new file beside it,
    /// `flight.csv.k3Yx0a.tmp`, synced to disk and moved over `path` once
    /// `write` is done. A failure part way leaves the file that was there,
    /// and the new one goes. A process ended mid-write, by a second close,
    /// a kill or a power loss, leaves the file that was there too, with
    /// the `.tmp` file beside it. `write` is given the file's name and
    /// returns the summary the menu bar shows. Refused while a job runs.
    ///
    /// The move replaces what is at `path` rather than writing into it: a
    /// symbolic link there is replaced, not written through; on Unix a
    /// read-only file is replaced as well, while on Windows the move fails
    /// on one, as writing in place would; and a folder the user cannot make
    /// files in fails, even when the file in it is writable. On Windows the
    /// move takes no long-path prefix, so a target path near 260 characters
    /// fails where writing it in place would not. The sync is a hard
    /// failure on a mount without one, and the move has no retry against
    /// a program holding the just-closed file for a moment.
    #[cfg(not(target_arch = "wasm32"))]
    fn write_file(
        &mut self,
        path: PathBuf,
        ctx: &Context,
        write: impl FnOnce(&mut dyn Write, &str) -> std::io::Result<String> + Send + 'static,
    ) {
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
                let dir = path
                    .parent()
                    .filter(|d| !d.as_os_str().is_empty())
                    .unwrap_or(Path::new("."));
                let prefix = format!("{name}.");
                let mut builder = tempfile::Builder::new();
                builder.prefix(&prefix).suffix(".tmp");
                // a temporary file is owner-only by default; an export is an
                // ordinary file, so it asks for everything and the umask
                // takes its usual share
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    builder.permissions(std::fs::Permissions::from_mode(0o666))
                };
                // `temp` removes the file on any return before the move
                let (file, temp) = builder.tempfile_in(dir).map_err(describe)?.into_parts();
                let mut out = BufWriter::new(file);
                let summary = write(&mut out, &name).map_err(describe)?;
                let file = out.into_inner().map_err(|e| describe(e.into_error()))?;
                // on disk before it takes the place of the file that was
                // there, and a late write error reported, not lost at close
                file.sync_all().map_err(describe)?;
                drop(file);
                temp.persist(&path).map_err(|e| describe(e.error))?;
                Ok(Done::Exported { path, summary })
            },
        ));
    }

    /// In the browser, write through `write` into memory and hand the
    /// bytes to the browser as a download named after `path`, as a job
    /// that runs to its end at once, so the outcome reaches the menu bar
    /// the way a native export's does. Refused while a job runs or a scan
    /// is armed. Nothing is replaced: the browser names a second download
    /// of the same name itself, `flight (1).csv`.
    #[cfg(target_arch = "wasm32")]
    fn write_file(
        &mut self,
        path: PathBuf,
        ctx: &Context,
        write: impl FnOnce(&mut dyn Write, &str) -> std::io::Result<String> + Send + 'static,
    ) {
        if self.busy() {
            return;
        }
        let name = worker::file_name(&path);
        let label = format!("Downloading {name}");
        self.job = Some(Job::run(
            Kind::Export,
            path.clone(),
            label,
            ctx.clone(),
            move || {
                let mut bytes = Vec::new();
                let summary = write(&mut bytes, &name).map_err(|e| format!("{name}: {e}"))?;
                crate::download::download(&name, &bytes).map_err(|e| format!("{name}: {e}"))?;
                Ok(Done::Exported { path, summary })
            },
        ));
    }

    /// A shade over the window while a file is being dragged over it, and
    /// what dropping it does.
    fn drop_overlay(ctx: &Context) {
        let files = ctx.input(|i| i.raw.hovered_files.len());
        if files == 0 {
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
            drop_text(files),
            egui::FontId::proportional(24.0),
            Color32::WHITE,
        );
    }
}

/// The series showing on `plot`, as the CSV writes them, each column's
/// digits chosen by its field's type and scaling; see [`Digits::of`].
fn csv_export(log: &LoadedLog, plot: &PlotPanel, range: Option<RangeInclusive<f64>>) -> Export {
    Export {
        columns: plot
            .showing()
            .map(|s| {
                let field = log.field_of(&s.key);
                Column {
                    title: s.title.clone(),
                    data: s.data.clone(),
                    digits: Digits::of(
                        field.is_some_and(|f| f.code == 'f'),
                        Scale::of(field.and_then(|f| f.multiplier)),
                    ),
                }
            })
            .collect(),
        time_base: log.time_base,
        range,
    }
}

/// The row above the plot while B is open: the names of A and B, a
/// button that closes B and one that swaps the two, which wait while
/// `busy` says why, and how B
/// lines up: the alignment, the offset, which a drag sets by hand and a
/// reset takes back, the two times put together on each log's clock,
/// what stood in for what was asked, and when the two share no time. What
/// the user asks is returned, to apply once the panels have drawn.
fn pair_row(
    ui: &mut egui::Ui,
    a: &LoadedLog,
    b: &LogB,
    busy: Option<&'static str>,
    axis: TimeAxis,
) -> Option<PairAction> {
    let mut action = None;
    // wrapped, so long names leave the controls on screen
    ui.horizontal_wrapped(|ui| {
        ui.strong("A");
        ui.label(&a.name);
        ui.add_space(12.0);
        ui.strong("B");
        ui.label(&b.log.name);
        if ui
            .add_enabled(busy.is_none(), egui::Button::new("\u{d7}").small())
            .on_hover_text("Close B")
            .on_disabled_hover_text(busy.unwrap_or_default())
            .clicked()
        {
            action = Some(PairAction::CloseB);
        }
        if ui
            .add_enabled(busy.is_none(), egui::Button::new("Swap").small())
            .on_hover_text("Review B as A, and A as B")
            .on_disabled_hover_text(busy.unwrap_or_default())
            .clicked()
        {
            action = Some(PairAction::Swap);
        }
        ui.separator();
        let by_hand = b.by_hand.is_some();
        let shown = if by_hand {
            "By hand"
        } else {
            b.lineup.took.label()
        };
        egui::ComboBox::from_id_salt("pair_align")
            .selected_text(shown)
            .show_ui(ui, |ui| {
                for align in Align::ALL {
                    let current = !by_hand && b.lineup.took == align;
                    if ui.selectable_label(current, align.label()).clicked() {
                        action = Some(PairAction::Align(align));
                    }
                }
            })
            .response
            .on_hover_text("How B lines up with A on the time axis");
        let mut offset = b.offset();
        if ui
            .add(
                egui::DragValue::new(&mut offset)
                    .speed(0.1)
                    .max_decimals(3)
                    .suffix(" s"),
            )
            .on_hover_text("Seconds added to B's times; drag or type to set by hand")
            .changed()
        {
            action = Some(PairAction::ByHand(offset));
        }
        if ui
            .add_enabled(by_hand, egui::Button::new("Reset").small())
            .on_hover_text("Back to the offset worked out")
            .clicked()
        {
            action = Some(PairAction::Reset);
        }
        if let Some((at_a, at_b)) = b.lineup.at.filter(|_| !by_hand) {
            ui.weak(format!(
                "A {} with B {}",
                timefmt::clock(a.wall_clock(axis), at_a, 0.01),
                timefmt::clock(b.log.wall_clock(axis), at_b, 0.01)
            ));
        }
        if let Some(note) = b.lineup.note.as_deref().filter(|_| !by_hand) {
            ui.weak(note);
        }
        if let Some(apart) = align::apart(a.time_range(), b.log.time_range(), b.offset()) {
            ui.colored_label(ui.visuals().warn_fg_color, apart);
        }
    });
    action
}

/// The line an export's window starts with while B is open: the log it
/// writes is A.
fn from_a(ui: &mut egui::Ui, b_open: bool, log: &LoadedLog) {
    if b_open {
        ui.label(format!("From A: {}", log.name));
    }
}

/// What the shade over the window says while `files` are dragged over
/// it: natively two or more open as a pair; the browser opens the first.
fn drop_text(files: usize) -> &'static str {
    if files > 1 && cfg!(not(target_arch = "wasm32")) {
        "Drop the logs to compare them: the first as A, the second as B"
    } else {
        "Drop the log to open it"
    }
}

/// The window's title: `flight.bin - Aftermission`, and with B open
/// `flight.bin vs baseline.bin - Aftermission`.
fn window_title(a: &str, b: Option<&str>) -> String {
    match b {
        Some(b) => format!("{a} vs {b} - Aftermission"),
        None => format!("{a} - Aftermission"),
    }
}

/// Name the window after the logs.
#[cfg(not(target_arch = "wasm32"))]
fn set_title(ctx: &Context, title: &str) {
    ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.to_string()));
}

/// Name the tab after the logs. eframe's web runner acts on no viewport
/// command but the screenshot, and warns about each other one, so the
/// title goes to the document itself.
#[cfg(target_arch = "wasm32")]
fn set_title(_ctx: &Context, title: &str) {
    if let Some(document) = web_sys::window().and_then(|w| w.document()) {
        document.set_title(title);
    }
}

/// What an item that waits for a job says while it waits.
const WAIT_JOB: &str = "Wait for the running job";

/// Why an export item is disabled: a running job comes first, then no log
/// to export from, then what the item itself lacks.
fn disabled_reason(idle: bool, has_log: bool, reason: &'static str) -> &'static str {
    if !idle {
        WAIT_JOB
    } else if !has_log {
        "Open a log first"
    } else {
        reason
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
    use std::sync::mpsc;

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

    /// The app in a window with room for every panel.
    fn harness(app: &mut AftermissionApp) -> Harness<'_> {
        Harness::builder()
            .with_size(egui::vec2(1400.0, 900.0))
            .build_ui(|ui| app.show(ui))
    }

    /// Run a frame of the app and look at it.
    fn with_ui(app: &mut AftermissionApp, check: impl FnOnce(&mut Harness<'_>)) {
        let mut harness = harness(app);
        harness.run();
        check(&mut harness);
    }

    /// [`with_ui`] while a job runs: the spinner asks for frames without
    /// end, so the run stops at the step limit rather than when the window
    /// is still.
    fn with_busy_ui(app: &mut AftermissionApp, check: impl FnOnce(&mut Harness<'_>)) {
        let mut harness = harness(app);
        harness.run_ok();
        check(&mut harness);
    }

    /// Start an export of `path` that runs until the returned sender fires,
    /// so what the window shows while a job runs can be checked with no
    /// race against the job's end. It ends with the summary "Released".
    fn hold_job(app: &mut AftermissionApp, ctx: &Context, path: PathBuf) -> mpsc::Sender<()> {
        let (release, held) = mpsc::channel::<()>();
        let label = format!("Writing {}", worker::file_name(&path));
        let job_path = path.clone();
        app.job = Some(Job::run(
            Kind::Export,
            path,
            label,
            ctx.clone(),
            move || {
                let _ = held.recv();
                Ok(Done::Exported {
                    path: job_path,
                    summary: "Released".into(),
                })
            },
        ));
        release
    }

    #[test]
    fn the_about_dialog_names_the_commit_ci_built() {
        assert_eq!(
            version_line(Some("49d52b1")),
            format!("Aftermission {VERSION} (49d52b1)")
        );
        assert_eq!(version_line(None), format!("Aftermission {VERSION}"));
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
    fn a_click_in_the_events_list_is_not_followed() {
        let dir = tempfile::tempdir().unwrap();
        let app = opened(dir.path());
        let mut harness = Harness::builder()
            .with_size(egui::vec2(1400.0, 900.0))
            .build_ui_state(|ui, app: &mut AftermissionApp| app.show(ui), app);
        harness.run();
        harness.get_by_label_contains("MODE Loiter").click();
        // the click's own frame: the seek is made, and the list is not
        // asked to follow it, which would scroll to the last of any rows
        // sharing its time
        harness.step();
        let app = harness.state_mut();
        assert_eq!(app.plot().cursor, Some(2.25));
        assert!(!*app.plot.follow_mut());
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
        drag_bottom_panel(&mut harness, 100.0);
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
        let release = hold_job(&mut app, &ctx, dir.path().join("held.csv"));
        with_busy_ui(&mut app, |harness| {
            assert!(harness.query_by_label("Export CSV").is_none());
        });
        assert!(app.csv_dialog.is_none());
        release.send(()).unwrap();
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
            export.columns.iter().map(|c| c.digits).collect::<Vec<_>>(),
            [Digits::Double, Digits::Single]
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
    fn the_parameters_export_as_a_param_file_with_last_or_boot_values() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = opened(dir.path());
        let ctx = Context::default();

        // the window counts what it is about to write
        app.param_dialog = Some(ParamDialog::default());
        with_ui(&mut app, |harness| {
            let window =
                harness.get_by_role_and_label(egui::accesskit::Role::Window, "Export parameters");
            window.get_by_label("4 parameters, 1 changed after boot");
            window
                .get_by_label("Boot values instead of last values")
                .click();
            harness.run();
        });
        assert!(app.param_dialog.as_ref().unwrap().boot);
        with_ui(&mut app, |harness| {
            harness.get_by_label("Cancel").click();
            harness.run();
        });
        assert!(app.param_dialog.is_none());

        let log = app.log().unwrap();
        let (last, boot) = (
            ParamFile::new(&log.name, &log.params, false),
            ParamFile::new(&log.name, &log.params, true),
        );
        let out = dir.path().join("flight.param");
        app.export_params_to(last, out.clone(), &ctx);
        assert_eq!(app.job.as_ref().unwrap().label, "Writing flight.param");
        wait(&mut app, &ctx);
        assert_eq!(
            app.notice,
            Some(Notice {
                text: "Wrote flight.param: 4 parameters".into(),
                error: false,
            })
        );
        assert_eq!(app.settings.export_dir.as_deref(), Some(dir.path()));
        assert_eq!(
            std::fs::read_to_string(&out).unwrap(),
            "# flight.bin: each parameter's last value in the log\n\
             ATC_RAT_RLL_P,0.137\n\
             MIS_TOTAL,7\n\
             SIM_RATE_HZ,380\n\
             WPNAV_SPEED,1500\n"
        );
        // the boot values differ in the one changed after boot
        let out = dir.path().join("flight_boot.param");
        app.export_params_to(boot, out.clone(), &ctx);
        wait(&mut app, &ctx);
        assert_eq!(
            std::fs::read_to_string(&out).unwrap(),
            "# flight.bin: each parameter's value at boot in the log\n\
             ATC_RAT_RLL_P,0.137\n\
             MIS_TOTAL,4\n\
             SIM_RATE_HZ,380\n\
             WPNAV_SPEED,1500\n"
        );

        // a job underneath the window closes it
        app.param_dialog = Some(ParamDialog::default());
        let release = hold_job(&mut app, &ctx, dir.path().join("held.csv"));
        with_busy_ui(&mut app, |harness| {
            assert!(harness.query_by_label("Export parameters").is_none());
        });
        assert!(app.param_dialog.is_none());
        release.send(()).unwrap();
        wait(&mut app, &ctx);
    }

    /// The names of the files in a Parquet export's folder, sorted, each
    /// checked to be finished: a file the exporter did not close lacks the
    /// footer's closing magic.
    #[cfg(feature = "parquet")]
    fn parquet_files(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|entry| {
                let path = entry.unwrap().path();
                let bytes = std::fs::read(&path).unwrap();
                assert!(
                    bytes.starts_with(b"PAR1") && bytes.ends_with(b"PAR1"),
                    "{}",
                    path.display()
                );
                worker::file_name(&path)
            })
            .collect();
        names.sort();
        names
    }

    #[cfg(feature = "parquet")]
    #[test]
    fn the_whole_log_exports_as_parquet_into_a_folder_of_its_own() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = opened(dir.path());
        let ctx = Context::default();
        let log = app.log().unwrap();
        let (types, records) = log.parquet_counts();
        assert_eq!((types, records), (log.types.len(), log.records()));
        let mut expected: Vec<String> = log
            .types
            .iter()
            .map(|t| format!("{}.parquet", t.name))
            .collect();
        expected.sort();

        // the window counts the log and offers the split for its two IMUs
        app.parquet_dialog = Some(ParquetDialog::default());
        with_ui(&mut app, |harness| {
            let window =
                harness.get_by_role_and_label(egui::accesskit::Role::Window, "Export Parquet");
            window.get_by_label(&format!("{types} message types, {records} records"));
            window.get_by_label(
                "One file per type, in a new folder flight_parquet in the folder you \
                 choose, numbered when the name is taken",
            );
            window.get_by_label("One file per instance").click();
            harness.run();
        });
        assert!(app.parquet_dialog.as_ref().unwrap().split);
        with_ui(&mut app, |harness| {
            harness.get_by_label("Cancel").click();
            harness.run();
        });
        assert!(app.parquet_dialog.is_none());

        // one file per type, in a folder named after the log
        app.export_parquet_to(dir.path().to_path_buf(), false, &ctx);
        assert_eq!(
            app.job.as_ref().unwrap().label,
            "Writing Parquet files for flight.bin"
        );
        wait(&mut app, &ctx);
        assert_eq!(
            app.notice,
            Some(Notice {
                text: format!("Wrote flight_parquet: {types} files, {records} rows"),
                error: false,
            })
        );
        // the next export opens in the folder chosen, not the one made
        assert_eq!(app.settings.export_dir.as_deref(), Some(dir.path()));
        let first = dir.path().join("flight_parquet");
        assert_eq!(parquet_files(&first), expected);

        // again, split: a new folder beside the first, the IMU in two files
        app.export_parquet_to(dir.path().to_path_buf(), true, &ctx);
        wait(&mut app, &ctx);
        let split = parquet_files(&dir.path().join("flight_parquet (2)"));
        assert!(split.contains(&"IMU_0.parquet".to_string()), "{split:?}");
        assert!(split.contains(&"IMU_1.parquet".to_string()), "{split:?}");
        assert!(!split.contains(&"IMU.parquet".to_string()), "{split:?}");
        assert_eq!(parquet_files(&first), expected, "the first is untouched");

        // a folder that cannot be made is an error, and nothing is left
        let missing = dir.path().join("gone");
        app.export_parquet_to(missing.clone(), false, &ctx);
        wait(&mut app, &ctx);
        let notice = app.notice.clone().unwrap();
        assert!(notice.error);
        assert!(
            notice
                .text
                .starts_with(&missing.join("flight_parquet").display().to_string()),
            "{}",
            notice.text
        );
        assert!(!missing.exists());

        // a job underneath the window closes it
        app.parquet_dialog = Some(ParquetDialog::default());
        let release = hold_job(&mut app, &ctx, dir.path().join("held.csv"));
        with_busy_ui(&mut app, |harness| {
            assert!(harness.query_by_label("Export Parquet").is_none());
        });
        assert!(app.parquet_dialog.is_none());
        release.send(()).unwrap();
        wait(&mut app, &ctx);
    }

    #[test]
    fn a_close_during_an_export_is_held_once() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = opened(dir.path());
        let ctx = Context::default();

        // idle, or opening a log: nothing to lose
        assert!(!app.hold_close());
        let path = app.settings.recent[0].clone();
        app.open_path(&path, &ctx);
        assert!(!app.hold_close());
        wait(&mut app, &ctx);

        // an export that has not finished: held once, with a notice
        let (tx, rx) = std::sync::mpsc::channel::<()>();
        app.job = Some(Job::run(
            Kind::Export,
            dir.path().join("flight.csv"),
            "Writing flight.csv".into(),
            ctx.clone(),
            move || {
                // held until the test lets it finish
                let _ = rx.recv();
                Ok(Done::Exported {
                    path: "flight.csv".into(),
                    summary: "Wrote flight.csv: 3 rows, 1 series".into(),
                })
            },
        ));
        assert!(app.hold_close());
        let warning = Some(Notice {
            text: "Writing flight.csv\u{2026} Close again to quit anyway".into(),
            error: true,
        });
        assert_eq!(app.notice, warning);
        // a refusal while the close is held leaves the warning showing
        app.open_path(&path, &ctx);
        assert_eq!(app.notice, warning);
        assert!(!app.hold_close(), "the second close closes");

        // a later export is held again
        tx.send(()).unwrap();
        wait(&mut app, &ctx);
        assert!(!app.close_held);
        let (tx, rx) = std::sync::mpsc::channel::<()>();
        app.job = Some(Job::run(
            Kind::Export,
            dir.path().join("flight.param"),
            "Writing flight.param".into(),
            ctx.clone(),
            move || {
                let _ = rx.recv();
                Err("disk full".into())
            },
        ));
        assert!(app.hold_close());
        tx.send(()).unwrap();
        wait(&mut app, &ctx);
    }

    #[cfg(feature = "parquet")]
    #[test]
    fn the_parquet_export_needs_a_log_and_splits_only_what_has_instances() {
        use dflog::access::Value;
        use dflog::write::LogWriter;

        let menu_item_disabled = |app: &mut AftermissionApp| {
            let mut disabled = false;
            with_ui(app, |harness| {
                harness.get_by_label("File").click();
                harness.run();
                harness.get_by_label("Export \u{23F5}").hover();
                harness.run();
                disabled = harness
                    .get_by_label("Whole log as Parquet\u{2026}")
                    .accesskit_node()
                    .is_disabled();
            });
            disabled
        };
        let dir = tempfile::tempdir().unwrap();

        // no log, or one without records: nothing to export
        let mut app = AftermissionApp::default();
        app.settings.online_tiles = false;
        assert!(menu_item_disabled(&mut app));
        let path = dir.path().join("empty.bin");
        std::fs::write(&path, b"not a log").unwrap();
        reopen(&mut app, &path);
        assert_eq!(app.log().unwrap().records(), 0);
        assert!(menu_item_disabled(&mut app));

        // a log whose one type has no instances: the split changes nothing
        let mut w = LogWriter::new();
        w.define(1, "BAT", "Qf", &["TimeUS", "Volt"]).unwrap();
        for (us, volt) in [(1_000_000, 15.2), (2_000_000, 15.1)] {
            w.record("BAT", &[Value::U64(us), Value::F64(volt)])
                .unwrap();
        }
        let path = dir.path().join("bench.bin");
        std::fs::write(&path, w.into_bytes()).unwrap();
        reopen(&mut app, &path);
        assert!(!menu_item_disabled(&mut app));
        app.parquet_dialog = Some(ParquetDialog { split: true });
        with_ui(&mut app, |harness| {
            let window =
                harness.get_by_role_and_label(egui::accesskit::Role::Window, "Export Parquet");
            window.get_by_label(
                "One file per type, in a new folder bench_parquet in the folder you \
                 choose, numbered when the name is taken",
            );
            assert!(
                window
                    .get_by_label("One file per instance")
                    .accesskit_node()
                    .is_disabled()
            );
        });
        assert!(!app.parquet_dialog.as_ref().unwrap().split);
    }

    #[cfg(not(feature = "parquet"))]
    #[test]
    fn without_the_parquet_feature_the_menu_has_no_parquet_export() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = opened(dir.path());
        with_ui(&mut app, |harness| {
            harness.get_by_label("File").click();
            harness.run();
            harness.get_by_label("Export \u{23F5}").hover();
            harness.run();
            harness.get_by_label("Parameters as .param\u{2026}");
            assert!(
                harness
                    .query_by_label("Whole log as Parquet\u{2026}")
                    .is_none()
            );
        });
    }

    #[test]
    fn the_param_export_holds_back_what_the_log_cannot_give() {
        use dflog::access::Value;
        use dflog::write::LogWriter;

        // a log with only `records` as `PARM` rows, each at boot, and an
        // event so that it has something without them
        let log = |records: &[(&str, f64)]| {
            let mut w = LogWriter::new();
            w.define(1, "PARM", "QNf", &["TimeUS", "Name", "Value"])
                .unwrap();
            w.define(2, "EV", "QB", &["TimeUS", "Id"]).unwrap();
            w.record("EV", &[Value::U64(1_000_000), Value::U64(10)])
                .unwrap();
            for &(name, value) in records {
                w.record(
                    "PARM",
                    &[
                        Value::U64(1_000_000),
                        Value::Str(name.into()),
                        Value::F64(value),
                    ],
                )
                .unwrap();
            }
            w.into_bytes()
        };
        let dir = tempfile::tempdir().unwrap();
        let mut app = AftermissionApp::default();
        app.settings.online_tiles = false;

        // no parameters: nothing to export
        let path = dir.path().join("none.bin");
        std::fs::write(&path, log(&[])).unwrap();
        reopen(&mut app, &path);
        with_ui(&mut app, |harness| {
            harness.get_by_label("File").click();
            harness.run();
            harness.get_by_label("Export \u{23F5}").hover();
            harness.run();
            assert!(
                harness
                    .get_by_label("Parameters as .param\u{2026}")
                    .accesskit_node()
                    .is_disabled()
            );
        });

        // none changed after boot: the boot values are the last ones, and
        // a damaged name is counted out
        let path = dir.path().join("still.bin");
        std::fs::write(
            &path,
            log(&[("FLTMODE1", 5.0), ("BAD NAME", 1.0), ("RTL_ALT", 1500.0)]),
        )
        .unwrap();
        reopen(&mut app, &path);
        app.param_dialog = Some(ParamDialog { boot: true });
        with_ui(&mut app, |harness| {
            let window =
                harness.get_by_role_and_label(egui::accesskit::Role::Window, "Export parameters");
            window.get_by_label("2 parameters, 0 changed after boot");
            window.get_by_label("Left out, with a name no loader can read: 1");
            assert!(
                window
                    .get_by_label("Boot values instead of last values")
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
        assert!(!app.param_dialog.as_ref().unwrap().boot);

        // every name damaged: the window says why and offers no save
        let path = dir.path().join("damaged.bin");
        std::fs::write(&path, log(&[("BAD NAME", 1.0)])).unwrap();
        reopen(&mut app, &path);
        app.param_dialog = Some(ParamDialog::default());
        with_ui(&mut app, |harness| {
            let window =
                harness.get_by_role_and_label(egui::accesskit::Role::Window, "Export parameters");
            window.get_by_label("0 parameters, 0 changed after boot");
            window.get_by_label("Left out, with a name no loader can read: 1");
            assert!(
                window
                    .get_by_label("Save as\u{2026}")
                    .accesskit_node()
                    .is_disabled()
            );
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
    fn a_running_job_shows_a_refusal_and_holds_back_every_export() {
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
        let release = hold_job(&mut app, &ctx, dir.path().join("held.csv"));
        app.open_path(&dir.path().join("other.bin"), &ctx);

        // the spinner asks for frames without end, so each run stops at the
        // step limit rather than when the window is still
        let export_items = |harness: &mut Harness<'_>| -> Vec<bool> {
            harness.get_by_label("File").click();
            harness.run_ok();
            harness.get_by_label("Export \u{23F5}").hover();
            harness.run_ok();
            let mut items = vec![
                "Plotted series as CSV\u{2026}",
                "Parameters as .param\u{2026}",
            ];
            if cfg!(feature = "parquet") {
                items.push("Whole log as Parquet\u{2026}");
            }
            items
                .iter()
                .map(|item| !harness.get_by_label(item).accesskit_node().is_disabled())
                .collect()
        };
        let mut harness = harness(&mut app);
        harness.run_ok();
        // the refusal shows beside the job it waits for
        harness.get_by_label("Writing held.csv\u{2026}");
        harness.get_by_label("Still busy: Writing held.csv\u{2026}");
        let busy = export_items(&mut harness);
        assert!(
            busy.iter().all(|&on| !on),
            "none while a job runs: {busy:?}"
        );
        drop(harness);

        release.send(()).unwrap();
        wait(&mut app, &ctx);
        with_ui(&mut app, |harness| {
            harness.get_by_label("Released");
            let idle = export_items(harness);
            assert!(
                idle.iter().all(|&on| on),
                "all once the job is done: {idle:?}"
            );
        });
    }

    #[test]
    fn a_failed_write_leaves_the_file_that_was_there() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = opened(dir.path());
        let ctx = Context::default();
        let out = dir.path().join("flight.csv");
        std::fs::write(&out, "the export before").unwrap();

        // a write that fails part way: the file before stays, and the
        // half-written one goes
        app.write_file(out.clone(), &ctx, |file, _| {
            file.write_all(b"half a row")?;
            Err(std::io::Error::other("disk full"))
        });
        wait(&mut app, &ctx);
        assert_eq!(
            app.notice,
            Some(Notice {
                text: format!("{}: disk full", out.display()),
                error: true,
            })
        );
        assert_eq!(std::fs::read_to_string(&out).unwrap(), "the export before");
        let mut names: Vec<String> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|entry| worker::file_name(&entry.unwrap().path()))
            .collect();
        names.sort();
        assert_eq!(names, ["flight.bin", "flight.csv"]);

        // a write that finishes replaces it
        app.write_file(out.clone(), &ctx, |file, name| {
            file.write_all(b"the export after")?;
            Ok(format!("Wrote {name}"))
        });
        wait(&mut app, &ctx);
        assert_eq!(
            app.notice,
            Some(Notice {
                text: "Wrote flight.csv".into(),
                error: false,
            })
        );
        assert_eq!(std::fs::read_to_string(&out).unwrap(), "the export after");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 2);
        // an ordinary file, not an owner-only temporary one; the usual
        // umask leaves it readable to others
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&out).unwrap().permissions().mode();
            assert_eq!(mode & 0o044, 0o044, "{mode:o}");
        }
    }

    #[test]
    fn a_disabled_export_says_what_comes_first() {
        assert_eq!(
            disabled_reason(false, false, "Plot a series first"),
            "Wait for the running job"
        );
        assert_eq!(
            disabled_reason(true, false, "Plot a series first"),
            "Open a log first"
        );
        assert_eq!(
            disabled_reason(true, true, "Plot a series first"),
            "Plot a series first"
        );
    }

    #[test]
    fn a_log_opens_from_bytes() {
        let ctx = Context::default();
        let mut app = AftermissionApp::default();
        app.settings.online_tiles = false;
        app.job = Some(Job::open_bytes(
            "flight.bin".into(),
            crate::model::testlog::bytes(),
            Slot::A,
            ctx.clone(),
        ));
        assert_eq!(
            app.running().unwrap(),
            worker::indexing_label(
                "flight.bin",
                Some(crate::model::testlog::bytes().len() as u64)
            )
        );
        wait(&mut app, &ctx);
        assert!(app.error.is_none(), "{:?}", app.error);
        let log = app.log().unwrap();
        assert_eq!(log.name, "flight.bin");
        assert!(log.records() > 0);
    }

    #[test]
    fn only_a_log_with_a_path_is_remembered() {
        let ctx = Context::default();
        let mut app = AftermissionApp::default();
        app.settings.online_tiles = false;
        let file = PathBuf::from("logs/flight.bin");
        for (path, expected) in [(None, vec![]), (Some(file.clone()), vec![file])] {
            let log = Box::new(LoadedLog::build(
                dflog::Log::from_source(crate::model::testlog::bytes().into()),
                "flight.bin".into(),
            ));
            app.job = Some(Job::run(
                Kind::Open(Slot::A),
                "flight.bin".into(),
                "Indexing flight.bin".into(),
                ctx.clone(),
                move || Ok(Done::Opened { path, log, b: None }),
            ));
            wait(&mut app, &ctx);
            assert_eq!(app.settings.recent, expected);
            assert_eq!(app.log().unwrap().name, "flight.bin");
        }
    }

    #[test]
    fn the_demo_opens_from_the_menu_and_the_empty_window() {
        let ctx = Context::default();
        let mut app = AftermissionApp::default();
        app.settings.online_tiles = false;
        with_ui(&mut app, |harness| {
            harness
                .get_by_label("Drop an ArduPilot .bin file here, press Ctrl+O, or try the demo.");
            harness.get_by_label("File").click();
            harness.run();
            harness.get_by_label("Open demo log").click();
            // the spinner asks for frames while the job runs
            harness.run_ok();
        });
        assert!(
            app.job.is_some() || app.log().is_some(),
            "the demo is on its way"
        );
        wait(&mut app, &ctx);
        assert!(app.error.is_none(), "{:?}", app.error);
        let log = app.log().unwrap();
        assert_eq!(log.name, "demo.bin");
        assert!(log.records() > 50_000, "{}", log.records());
        assert!(app.settings.recent.is_empty(), "no path to offer again");
        with_ui(&mut app, |harness| {
            harness.get_by_label("demo.bin");
            harness.get_by_label_contains("Copter, ");
        });

        // the button on the empty window opens it too
        let mut app = AftermissionApp::default();
        app.settings.online_tiles = false;
        with_ui(&mut app, |harness| {
            harness.get_by_label("Demo").click();
            harness.run_ok();
        });
        wait(&mut app, &ctx);
        assert!(app.error.is_none(), "{:?}", app.error);
        assert_eq!(app.log().unwrap().name, "demo.bin");
        assert!(app.settings.recent.is_empty());
    }

    /// Open `path` as B in `app` and wait until the job is done.
    fn compare(app: &mut AftermissionApp, path: &Path) {
        let ctx = Context::default();
        app.compare_path(path, &ctx);
        assert!(app.job.is_some(), "B opens in the background");
        wait(app, &ctx);
    }

    #[test]
    fn the_title_names_a_and_b() {
        assert_eq!(
            window_title("flight.bin", None),
            "flight.bin - Aftermission"
        );
        assert_eq!(
            window_title("flight.bin", Some("baseline.bin")),
            "flight.bin vs baseline.bin - Aftermission"
        );
    }

    #[test]
    fn compare_with_opens_b_beside_a_and_a_new_a_closes_it() {
        use crate::model::SeriesKey;

        let dir = tempfile::tempdir().unwrap();
        let mut app = opened(dir.path());
        let ctx = Context::default();
        let a_path = app.settings.recent[0].clone();
        app.toggle_series(SeriesKey {
            type_name: "ATT".into(),
            field: "Roll".into(),
            instance: None,
        });
        // a frame shows the new log whole; a seek sets the playhead
        with_ui(&mut app, |_| {});
        app.plot.seek(2.1);
        assert!(!app.plot().resets_view());

        let baseline = dir.path().join("baseline.bin");
        std::fs::write(&baseline, crate::model::testlog::bytes()).unwrap();
        compare(&mut app, &baseline);
        assert!(app.error.is_none(), "{:?}", app.error);
        assert_eq!(app.log().unwrap().name, "flight.bin");
        assert_eq!(app.log_b().unwrap().name, "baseline.bin");
        // A stays as it was: its series, its playhead and its view
        assert_eq!(app.plot().selected.len(), 1);
        assert_eq!(app.plot().playhead(), Some(2.1));
        assert!(!app.plot().resets_view());
        // B's path is offered again, as any log's is
        assert_eq!(app.settings.recent, [baseline.clone(), a_path.clone()]);
        // the row above the plot names B, and the export windows say they
        // write A
        app.param_dialog = Some(ParamDialog::default());
        with_ui(&mut app, |harness| {
            harness.get_by_label("baseline.bin");
            let window =
                harness.get_by_role_and_label(egui::accesskit::Role::Window, "Export parameters");
            window.get_by_label("From A: flight.bin");
        });
        app.param_dialog = None;

        // a second one replaces B, and A stays
        let other = dir.path().join("other.bin");
        std::fs::write(&other, crate::model::testlog::bytes()).unwrap();
        compare(&mut app, &other);
        assert_eq!(app.log().unwrap().name, "flight.bin");
        assert_eq!(app.log_b().unwrap().name, "other.bin");
        assert_eq!(app.plot().playhead(), Some(2.1));

        // a B that fails names B, and leaves A and the B before it
        let gone = dir.path().join("gone.bin");
        app.compare_path(&gone, &ctx);
        wait(&mut app, &ctx);
        let error = app.error.clone().unwrap();
        assert!(
            error.starts_with(&format!("B: {}", gone.display())),
            "{error}"
        );
        assert_eq!(app.log().unwrap().name, "flight.bin");
        assert_eq!(app.log_b().unwrap().name, "other.bin");
        with_ui(&mut app, |harness| {
            harness.get_by_label(error.as_str());
        });

        // an A that fails leaves both
        app.open_path(&gone, &ctx);
        wait(&mut app, &ctx);
        let error = app.error.clone().unwrap();
        assert!(error.starts_with(&gone.display().to_string()), "{error}");
        assert_eq!(app.log().unwrap().name, "flight.bin");
        assert_eq!(app.log_b().unwrap().name, "other.bin");

        // an A that lands closes B
        reopen(&mut app, &a_path);
        assert!(app.log_b().is_none());
        with_ui(&mut app, |harness| {
            assert!(harness.query_by_label("other.bin").is_none());
        });
    }

    /// The row's alignment box, showing `shown`: a combo box carries its
    /// text as its value, not its label.
    /// The row's offset field, the one spin button on screen; being
    /// edited, a text box, the one with text in it.
    fn offset_field<'h>(harness: &'h Harness<'_>) -> egui_kittest::Node<'h> {
        harness
            .query_by_role(egui::accesskit::Role::SpinButton)
            .or_else(|| {
                harness
                    .get_all_by_role(egui::accesskit::Role::TextInput)
                    .find(|node| {
                        node.accesskit_node()
                            .value()
                            .is_some_and(|value| !value.is_empty())
                    })
            })
            .expect("the row's offset field")
    }

    fn align_box<'h>(harness: &'h Harness<'_>, shown: &str) -> egui_kittest::Node<'h> {
        harness
            .get_all_by_role(egui::accesskit::Role::ComboBox)
            .find(|node| node.accesskit_node().value().as_deref() == Some(shown))
            .unwrap_or_else(|| panic!("no alignment box showing {shown:?}"))
    }

    #[test]
    fn the_row_lines_b_up_and_a_hand_offset_goes_with_the_pair() {
        use crate::model::SeriesKey;

        let dir = tempfile::tempdir().unwrap();
        let mut app = opened(dir.path());
        let ctx = Context::default();
        let b_path = dir.path().join("b.bin");
        std::fs::write(&b_path, crate::model::testlog::b_bytes()).unwrap();
        app.toggle_series(SeriesKey {
            type_name: "ATT".into(),
            field: "Roll".into(),
            instance: None,
        });
        compare(&mut app, &b_path);
        let offset = |app: &AftermissionApp| app.log_b.as_ref().unwrap().offset();

        // at arming, the default: A armed at 2.03 s, and B, with no
        // arming, lines up at its start, 10 s, which the row names
        let lineup = &app.log_b.as_ref().unwrap().lineup;
        assert_eq!(lineup.took, Align::Arming);
        assert!((offset(&app) + 7.97).abs() < 1e-9, "{}", offset(&app));
        with_ui(&mut app, |harness| {
            align_box(harness, "At arming");
            harness.get_by_label("A 0:02.03 with B 0:10.00");
            harness.get_by_label("No arming in B; its start stands in");
        });
        // the plot reads B on A's clock
        app.plot.seek(2.1);
        assert_eq!(app.plot().readout[0].b, Some((10.1, 6.0)));

        // an offset set by hand, which the plot reads at once, and back
        app.apply(PairAction::ByHand(-8.0), &ctx);
        assert!((offset(&app) + 8.0).abs() < 1e-9);
        assert_eq!(app.plot().readout[0].b, Some((10.1, 6.0)));
        app.apply(PairAction::ByHand(-7.9), &ctx);
        assert_eq!(app.plot().readout[0].b, Some((10.0, 5.0)));
        with_ui(&mut app, |harness| {
            align_box(harness, "By hand");
            assert!(harness.query_by_label("A 0:02.03 with B 0:10.00").is_none());
        });
        // typed into the row's field, the same; a value that is no number
        // leaves the offset as it was
        let typed = |app: &mut AftermissionApp, text: &str| {
            with_ui(app, |harness| {
                offset_field(harness).click();
                harness.run();
                offset_field(harness).type_text(text);
                harness.run();
                harness.key_press(egui::Key::Enter);
                harness.run();
            });
        };
        typed(&mut app, "-8.5");
        assert!((offset(&app) + 8.5).abs() < 1e-9, "{}", offset(&app));
        typed(&mut app, "inf");
        assert!((offset(&app) + 8.5).abs() < 1e-9, "{}", offset(&app));
        app.apply(PairAction::Reset, &ctx);
        assert!((offset(&app) + 7.97).abs() < 1e-9);

        // at the start, picked in the box: A's 1 s with B's 10 s, kept in
        // the settings
        with_ui(&mut app, |harness| {
            align_box(harness, "At arming").click();
            harness.run();
            harness.get_by_label("At the start").click();
            harness.run();
        });
        assert_eq!(app.settings.align, Align::Start);
        assert!((offset(&app) + 9.0).abs() < 1e-9, "{}", offset(&app));

        // by UTC cannot hold without GPS time: at arming takes effect, the
        // row says why, and the choice made stays for the next pair
        app.apply(PairAction::Align(Align::Utc), &ctx);
        assert_eq!(app.settings.align, Align::Utc);
        let lineup = &app.log_b.as_ref().unwrap().lineup;
        assert_eq!(lineup.took, Align::Arming);
        assert_eq!(
            lineup.note.as_deref(),
            Some(
                "No GPS time in either log; lined up at arming. No arming in B; its start stands in"
            )
        );

        // a typed inf or NaN leaves the offset as it was
        let before = offset(&app);
        for typed in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            app.apply(PairAction::ByHand(typed), &ctx);
            assert_eq!(offset(&app).to_bits(), before.to_bits(), "{typed}");
        }

        // a hand offset goes when B is replaced
        app.apply(PairAction::ByHand(3.0), &ctx);
        compare(&mut app, &b_path);
        assert_eq!(app.log_b.as_ref().unwrap().by_hand, None);
        assert!((offset(&app) + 7.97).abs() < 1e-9);

        // the CSV window says B's lines stay out of the file
        app.csv_dialog = Some(CsvDialog::default());
        with_ui(&mut app, |harness| {
            harness.get_by_label("B's lines are left out: the file holds A's series");
        });
        app.csv_dialog = None;

        // closing B fits the map to A's track again
        with_ui(&mut app, |_| {});
        assert!(!app.map.fits());
        app.close_b(&ctx);
        assert!(app.map.fits());
    }

    #[test]
    fn swap_reviews_b_as_a_and_a_second_swap_undoes_it() {
        use crate::model::SeriesKey;

        let dir = tempfile::tempdir().unwrap();
        let mut app = opened(dir.path());
        let ctx = Context::default();
        let b_path = dir.path().join("b.bin");
        std::fs::write(&b_path, crate::model::testlog::b_bytes()).unwrap();
        let att = |field: &str| SeriesKey {
            type_name: "ATT".into(),
            field: field.into(),
            instance: None,
        };
        // B logs no pitch
        app.toggle_series(att("Roll"));
        app.toggle_series(att("Pitch"));
        compare(&mut app, &b_path);
        app.apply(PairAction::ByHand(-8.0), &ctx);

        // not while a job runs: B could land in a slot that changed. The
        // alignment and a hand offset apply meanwhile, and the B that
        // lands takes the alignment chosen, with no hand offset
        app.compare_path(&b_path, &ctx);
        app.apply(PairAction::Swap, &ctx);
        assert_eq!(app.log().unwrap().name, "flight.bin");
        app.apply(PairAction::Align(Align::Start), &ctx);
        app.apply(PairAction::ByHand(-3.0), &ctx);
        wait(&mut app, &ctx);
        let b = app.log_b.as_ref().unwrap();
        assert_eq!((b.lineup.took, b.by_hand), (Align::Start, None));
        app.apply(PairAction::Align(Align::Arming), &ctx);

        app.apply(PairAction::ByHand(-8.0), &ctx);
        app.plot.seek(2.1);
        with_ui(&mut app, |harness| {
            harness.get_by_label("Swap").click();
            harness.run();
        });
        assert_eq!(app.log().unwrap().name, "b.bin");
        assert_eq!(app.log_b().unwrap().name, "flight.bin");
        let b = app.log_b.as_ref().unwrap();
        assert_eq!(b.by_hand, Some(8.0));
        // worked out afresh the other way round: A's start, 10 s, with
        // B's arming at 2.03 s
        assert!((b.lineup.offset - 7.97).abs() < 1e-9, "{}", b.lineup.offset);
        assert_eq!(
            b.lineup.note.as_deref(),
            Some("No arming in A; its start stands in")
        );
        // the plot keeps both series, pitch for B's side alone, and the
        // playhead stays at the same moment, on the new A's clock
        assert_eq!(app.plot().selected.len(), 2);
        assert!(app.plot().selected[1].data.xs.is_empty());
        assert!(app.plot().selected[1].data_b.is_some());
        let playhead = app.plot().playhead().unwrap();
        assert!((playhead - 10.1).abs() < 1e-9, "{playhead}");

        // and back, as it was
        app.apply(PairAction::Swap, &ctx);
        assert_eq!(app.log().unwrap().name, "flight.bin");
        assert_eq!(app.log_b.as_ref().unwrap().by_hand, Some(-8.0));
        assert_eq!(app.plot().selected.len(), 2);
        assert!(!app.plot().selected[1].data.xs.is_empty());
        let playhead = app.plot().playhead().unwrap();
        assert!((playhead - 2.1).abs() < 1e-9, "{playhead}");

        // closing B takes the series only B had along
        app.apply(PairAction::Swap, &ctx);
        app.close_b(&ctx);
        assert_eq!(app.plot().selected.len(), 1);
    }

    #[test]
    fn what_only_b_logs_is_listed_under_a_s_types_to_plot_from_b() {
        use crate::model::SeriesKey;

        let dir = tempfile::tempdir().unwrap();
        let mut app = opened(dir.path());
        let ctx = Context::default();
        let b_path = dir.path().join("extra.bin");
        std::fs::write(&b_path, crate::model::testlog::b_extra_bytes()).unwrap();
        compare(&mut app, &b_path);
        let vibe = SeriesKey {
            type_name: "VIBE".into(),
            field: "VibeX".into(),
            instance: None,
        };

        // the section, closed, counts ATT, IMU and VIBE; opened, a tick
        // plots VIBE.VibeX from B, its chip marked B's
        with_ui(&mut app, |harness| {
            assert!(harness.query_by_label("VIBE (4)").is_none(), "closed");
            harness.get_by_label("Only in B (3)").click();
            harness.run();
            harness.get_by_label("VIBE (4)").click();
            harness.run();
            harness.get_by_label("VibeX").click();
            harness.run();
            harness.get_by_label("B VIBE.VibeX");
        });
        assert!(app.plot().is_selected_b(&vibe));
        assert_eq!(app.plot().showing().count(), 0, "B's, no A series");
        assert_eq!(app.unplotted(), "B's lines are not exported");

        // the list is worked out once for the pair and kept: emptied by
        // hand, it stays empty over a frame, as nothing works it out again
        let ids = (app.log().unwrap().id, app.log_b().unwrap().id);
        let types = |app: &AftermissionApp| app.only_in_b.as_ref().unwrap().types.len();
        assert_eq!(app.only_in_b.as_ref().unwrap().pair, ids);
        app.only_in_b.as_mut().unwrap().types.clear();
        with_ui(&mut app, |_| {});
        assert_eq!(types(&app), 0, "kept, not worked out again");

        // a swap works it out anew for the pair the other way round, and
        // swapping back, anew again
        app.apply(PairAction::Swap, &ctx);
        with_ui(&mut app, |_| {});
        assert_eq!(app.only_in_b.as_ref().unwrap().pair, (ids.1, ids.0));
        assert_ne!(types(&app), 0);
        app.apply(PairAction::Swap, &ctx);
        with_ui(&mut app, |_| {});
        assert_eq!(app.only_in_b.as_ref().unwrap().pair, ids);
        assert_eq!(types(&app), 3);

        // with a series of A's, the hint is A's (hidden through the
        // legend, still A's: see the plot's has_a_series)
        app.toggle_series(SeriesKey {
            type_name: "ATT".into(),
            field: "Roll".into(),
            instance: None,
        });
        assert_eq!(app.unplotted(), "Plot a series first");

        // the filter matches B's type names and field names, not units,
        // and the header counts the types it shows
        app.filter = "Vibe".into();
        with_ui(&mut app, |harness| {
            harness.get_by_label("Only in B (1)");
            harness.get_by_label("VibeX");
        });
        app.filter = "rad".into();
        with_ui(&mut app, |harness| {
            assert!(harness.query_by_label("Only in B (3)").is_none());
        });
        app.filter.clear();

        // a new B, the same file opened again, is another log: only B's
        // id changes, and the list, emptied by hand, is worked out anew
        app.only_in_b.as_mut().unwrap().types.clear();
        compare(&mut app, &b_path);
        with_ui(&mut app, |harness| {
            harness.get_by_label("Only in B (3)");
        });
        let new_b = app.log_b().unwrap().id;
        assert_ne!(new_b, ids.1);
        assert_eq!(app.only_in_b.as_ref().unwrap().pair, (ids.0, new_b));
        assert_eq!(types(&app), 3);

        // Close B takes the section and B's series away
        app.close_b(&ctx);
        with_ui(&mut app, |harness| {
            assert!(harness.query_by_label("Only in B (3)").is_none());
        });
        assert!(app.only_in_b.is_none());
        assert!(!app.plot().is_selected_b(&vibe));
        assert_eq!(app.plot().selected.len(), 1);
    }

    #[test]
    fn on_the_utc_axis_each_log_reads_through_its_own_clock() {
        let ctx = Context::default();
        let mut app = AftermissionApp::default();
        app.settings.online_tiles = false;
        app.settings.time_axis = TimeAxis::Utc;
        app.open_demo_pair(&ctx);
        wait(&mut app, &ctx);
        // the armings put together: A's at 14:30:45 and B's two hours on
        with_ui(&mut app, |harness| {
            harness.get_by_label("A 14:30:45.25 with B 16:30:33.25");
        });
    }

    #[test]
    fn a_click_on_a_b_event_seeks_to_its_moved_time() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = opened(dir.path());
        let ctx = Context::default();
        let baseline = dir.path().join("baseline.bin");
        std::fs::write(&baseline, crate::model::testlog::bytes()).unwrap();
        compare(&mut app, &baseline);
        // the same flight half a second earlier, by hand
        app.apply(PairAction::ByHand(-0.5), &ctx);
        with_ui(&mut app, |harness| {
            harness.get_by_label("B's events").click();
            harness.run();
            // B's Armed at 2.03 s on its clock, 1.53 s on A's
            harness
                .get_by_label_contains("0:01.530  EV   B Armed")
                .click();
            harness.run();
        });
        let playhead = app.plot().playhead().unwrap();
        assert!((playhead - 1.53).abs() < 1e-9, "{playhead}");
        assert!(!*app.plot.follow_mut(), "the list took the seek");
    }

    #[test]
    fn a_seek_to_a_time_of_b_moves_by_the_offset_and_stays_in_a() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = opened(dir.path());
        let b_path = dir.path().join("b.bin");
        std::fs::write(&b_path, crate::model::testlog::b_bytes()).unwrap();
        compare(&mut app, &b_path);
        // at arming: B's 10 s start stands in for its arming, A armed at
        // 2.03 s, so B's times draw 7.97 s earlier; A's run 1 s to 2.33 s
        let seeks = [(10.1, 2.13), (20.0, 2.33), (0.0, 1.0)];
        for (b_time, a_time) in seeks {
            app.seek_to(Slot::B, b_time);
            let playhead = app.plot().playhead().unwrap();
            assert!((playhead - a_time).abs() < 1e-9, "{b_time}: {playhead}");
        }
        // A's own times go as they are, as before
        app.seek_to(Slot::A, 5.0);
        assert_eq!(app.plot().playhead(), Some(5.0));
    }

    /// A file dropped on the window, as the desktop hands one over: a
    /// path.
    #[derive(Debug)]
    struct DroppedPath(PathBuf);

    impl egui::DroppedFile for DroppedPath {
        fn path(&self) -> &Path {
            &self.0
        }

        fn bytes(&self) -> Result<Vec<u8>, String> {
            std::fs::read(&self.0).map_err(|e| e.to_string())
        }
    }

    #[test]
    fn two_dropped_files_open_as_a_and_b_and_three_as_the_first_two() {
        let dir = tempfile::tempdir().unwrap();
        let file = |name: &str| {
            let path = dir.path().join(name);
            std::fs::write(&path, crate::model::testlog::bytes()).unwrap();
            path
        };
        let (a, b, c) = (file("a.bin"), file("b.bin"), file("c.bin"));
        let mut app = AftermissionApp::default();
        app.settings.online_tiles = false;
        let drop = |app: &mut AftermissionApp, paths: &[&PathBuf]| {
            let ctx = Context::default();
            let input = egui::RawInput {
                dropped_files: paths
                    .iter()
                    .map(|path| Arc::new(DroppedPath((*path).clone())) as egui::DroppedFileHandle)
                    .collect(),
                ..egui::RawInput::default()
            };
            let mut output = ctx.run_ui(input, |ui| app.show(ui));
            output.textures_delta.clear();
            wait(app, &ctx);
        };
        drop(&mut app, &[&a, &b]);
        assert!(app.error.is_none(), "{:?}", app.error);
        assert_eq!(app.log().unwrap().name, "a.bin");
        assert_eq!(app.log_b().unwrap().name, "b.bin");
        drop(&mut app, &[&c, &a, &b]);
        assert_eq!(app.log().unwrap().name, "c.bin");
        assert_eq!(app.log_b().unwrap().name, "a.bin");
        // one file opens as A and closes B
        drop(&mut app, &[&b]);
        assert_eq!(app.log().unwrap().name, "b.bin");
        assert!(app.log_b().is_none());

        // by the pair's rule: B gone, A opens alone and the error names B;
        // A gone, nothing changes
        let gone = dir.path().join("gone.bin");
        drop(&mut app, &[&a, &gone]);
        assert_eq!(app.log().unwrap().name, "a.bin");
        assert!(app.log_b().is_none());
        assert!(app.error.as_deref().unwrap().starts_with("B: "));
        drop(&mut app, &[&c, &b]);
        drop(&mut app, &[&gone, &a]);
        assert_eq!(app.log().unwrap().name, "c.bin");
        assert_eq!(app.log_b().unwrap().name, "b.bin");
        let error = app.error.clone().unwrap();
        assert!(error.starts_with(&gone.display().to_string()), "{error}");

        // the shade says what a drop of two will do
        assert_eq!(drop_text(1), "Drop the log to open it");
        assert_eq!(
            drop_text(2),
            "Drop the logs to compare them: the first as A, the second as B"
        );
    }

    #[test]
    fn a_pair_opens_a_and_b_and_one_whose_b_fails_opens_a_alone() {
        let ctx = Context::default();
        let mut app = AftermissionApp::default();
        app.settings.online_tiles = false;
        let model = |name: &str| {
            Box::new(LoadedLog::build(
                dflog::Log::from_source(crate::model::testlog::bytes().into()),
                name.into(),
            ))
        };
        let pair = |b: Result<Opened, String>| {
            let log = model("a.bin");
            Job::run(
                Kind::Open(Slot::A),
                "a.bin".into(),
                "Indexing a.bin".into(),
                ctx.clone(),
                move || {
                    Ok(Done::Opened {
                        path: Some("logs/a.bin".into()),
                        log,
                        b: Some(b),
                    })
                },
            )
        };
        app.job = Some(pair(Ok(Opened {
            path: Some("logs/b.bin".into()),
            log: model("b.bin"),
        })));
        wait(&mut app, &ctx);
        assert!(app.error.is_none(), "{:?}", app.error);
        assert_eq!(app.log().unwrap().name, "a.bin");
        assert_eq!(app.log_b().unwrap().name, "b.bin");
        // A tops Open recent, B after it
        assert_eq!(
            app.settings.recent,
            [PathBuf::from("logs/a.bin"), PathBuf::from("logs/b.bin")]
        );

        // B fails: A opens alone, the B before it goes with the A it was
        // beside, and the error names B
        app.job = Some(pair(Err("c.bin: not a log".into())));
        wait(&mut app, &ctx);
        assert_eq!(app.log().unwrap().name, "a.bin");
        assert!(app.log_b().is_none());
        assert_eq!(app.error.as_deref(), Some("B: c.bin: not a log"));
    }

    #[test]
    fn b_and_the_items_that_open_a_wait_while_a_job_runs() {
        use crate::picks::Picked;

        let dir = tempfile::tempdir().unwrap();
        let mut app = opened(dir.path());
        let ctx = Context::default();
        let baseline = dir.path().join("baseline.bin");
        std::fs::write(&baseline, crate::model::testlog::bytes()).unwrap();
        compare(&mut app, &baseline);

        // while a job runs, B neither opens nor closes
        let release = hold_job(&mut app, &ctx, dir.path().join("held.csv"));
        app.compare_path(&baseline, &ctx);
        assert_eq!(app.job.as_ref().unwrap().label, "Writing held.csv");
        assert_eq!(
            app.notice.as_ref().map(|n| n.text.as_str()),
            Some("Still busy: Writing held.csv\u{2026}")
        );
        app.close_b(&ctx);
        assert!(app.log_b().is_some());
        // and the menu's items that open a log, or close B, wait with it
        let items = [
            "Open\u{2026}",
            "Compare with\u{2026}",
            "Open demo log",
            "Open demo pair",
            "Open recent \u{23F5}",
            "Close B",
        ];
        let enabled = |harness: &mut Harness<'_>| -> Vec<bool> {
            harness.get_by_label("File").click();
            harness.run_ok();
            items
                .iter()
                .map(|item| !harness.get_by_label(item).accesskit_node().is_disabled())
                .collect()
        };
        let mut harness = harness(&mut app);
        harness.run_ok();
        assert!(
            harness
                .get_by_label("\u{d7}")
                .accesskit_node()
                .is_disabled(),
            "the row's close button waits"
        );
        let busy = enabled(&mut harness);
        assert!(
            busy.iter().all(|&on| !on),
            "none while a job runs: {busy:?}"
        );
        drop(harness);
        release.send(()).unwrap();
        wait(&mut app, &ctx);
        with_ui(&mut app, |harness| {
            let idle = enabled(harness);
            assert!(idle.iter().all(|&on| on), "all once it is done: {idle:?}");
        });

        // the row's button closes B, and so does the menu's item, which is
        // there only while B is open
        with_ui(&mut app, |harness| {
            harness.get_by_label("\u{d7}").click();
            harness.run();
        });
        assert!(app.log_b().is_none());
        compare(&mut app, &baseline);
        with_ui(&mut app, |harness| {
            harness.get_by_label("File").click();
            harness.run();
            harness.get_by_label("Close B").click();
            harness.run();
        });
        assert!(app.log_b().is_none());
        with_ui(&mut app, |harness| {
            harness.get_by_label("File").click();
            harness.run();
            assert!(harness.query_by_label("Close B").is_none());
        });

        // a file being read holds B too, and says why
        compare(&mut app, &baseline);
        let (pick, tx) = app.picks.start(Slot::A);
        tx.send(Picked::Chosen {
            pick,
            name: "next.bin".into(),
        })
        .unwrap();
        frame(&mut app, &ctx);
        assert_eq!(app.pair_busy(), Some("Wait for the file being read"));
        app.close_b(&ctx);
        assert!(app.log_b().is_some());
        // and so does Swap
        let a_name = app.log().unwrap().name.clone();
        app.apply(PairAction::Swap, &ctx);
        assert_eq!(app.log().unwrap().name, a_name);
        assert_eq!(app.log_b().unwrap().name, "baseline.bin");
    }

    #[test]
    fn b_arriving_while_playing_leaves_it_playing() {
        let dir = tempfile::tempdir().unwrap();
        let baseline = dir.path().join("baseline.bin");
        std::fs::write(&baseline, crate::model::testlog::bytes()).unwrap();
        let mut harness = player(long_flight(dir.path()));
        harness.key_press(egui::Key::Space);
        harness.step();
        assert!(playing(&harness));
        let ctx = harness.ctx.clone();
        harness.state_mut().compare_path(&baseline, &ctx);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while harness.state().job.is_some() {
            assert!(std::time::Instant::now() < deadline, "B did not open");
            harness.step();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(harness.state().log_b().unwrap().name, "baseline.bin");
        let at = playhead(&harness);
        harness.step();
        assert!(playing(&harness), "B's arrival pauses nothing");
        assert!(playhead(&harness) > at, "and the playhead goes on");
    }

    #[test]
    fn ctrl_o_waits_while_a_job_runs() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = opened(dir.path());
        let ctx = Context::default();
        // whether a frame with Ctrl+O pressed asks for the dialog
        let pressed = |app: &mut AftermissionApp| {
            let input = egui::RawInput {
                events: vec![egui::Event::Key {
                    key: egui::Key::O,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::COMMAND,
                }],
                ..egui::RawInput::default()
            };
            let mut asks = false;
            let mut output = ctx.run_ui(input, |ui| asks = app.open_key(ui.ctx()));
            output.textures_delta.clear();
            asks
        };
        let release = hold_job(&mut app, &ctx, dir.path().join("held.csv"));
        assert!(!pressed(&mut app), "not while a job runs");
        release.send(()).unwrap();
        wait(&mut app, &ctx);
        assert!(pressed(&mut app), "once it is done");
    }

    #[test]
    fn a_picked_b_opens_beside_a_and_a_new_a_drops_one_being_read() {
        use crate::picks::Picked;

        let dir = tempfile::tempdir().unwrap();
        let mut app = opened(dir.path());
        let ctx = Context::default();
        let (pick, tx) = app.picks.start(Slot::B);
        tx.send(Picked::Chosen {
            pick,
            name: "baseline.bin".into(),
        })
        .unwrap();
        with_busy_ui(&mut app, |harness| {
            harness.get_by_label("Reading baseline.bin as B\u{2026}");
        });
        let bytes = crate::model::testlog::bytes();
        tx.send(Picked::File {
            pick,
            name: "baseline.bin".into(),
            bytes: bytes.clone(),
        })
        .unwrap();
        frame(&mut app, &ctx);
        let label = worker::indexing_label("baseline.bin", Some(bytes.len() as u64)) + " as B";
        assert_eq!(app.running(), Some(label));
        frames_until_idle(&mut app, &ctx);
        assert!(app.error.is_none(), "{:?}", app.error);
        assert_eq!(app.log().unwrap().name, "flight.bin");
        assert_eq!(app.log_b().unwrap().name, "baseline.bin");
        assert_eq!(app.settings.recent.len(), 1, "no path to offer again");

        // a B that cannot be read, or a chooser refused, says it was B's
        let (pick, tx) = app.picks.start(Slot::B);
        tx.send(Picked::Chosen {
            pick,
            name: "photos".into(),
        })
        .unwrap();
        pick_frame(
            &mut app,
            Picked::Unreadable {
                pick,
                name: "photos".into(),
                dropped: false,
            },
        );
        assert_eq!(
            app.error.as_deref(),
            Some("B: photos: the browser could not read it.")
        );
        let (pick, _) = app.picks.start(Slot::B);
        pick_frame(&mut app, Picked::Refused { pick });
        assert_eq!(
            app.error.as_deref(),
            Some("The browser did not open the file chooser for B. Try again.")
        );
        assert_eq!(app.log_b().unwrap().name, "baseline.bin");

        // an A armed while a B is read drops the read, which lands nowhere
        let (pick, tx) = app.picks.start(Slot::B);
        tx.send(Picked::Chosen {
            pick,
            name: "other.bin".into(),
        })
        .unwrap();
        frame(&mut app, &ctx);
        assert_eq!(app.picks.reading(), Some(("other.bin", Slot::B)));
        app.arm_demo(&ctx);
        assert!(app.picks.reading().is_none());
        tx.send(Picked::File {
            pick,
            name: "other.bin".into(),
            bytes,
        })
        .unwrap();
        frames_until_idle(&mut app, &ctx);
        assert_eq!(app.log().unwrap().name, "demo.bin");
        assert!(app.log_b().is_none());
        assert!(app.running().is_none());

        // and an A still being read, which the newer demo pair outdates
        let (pick, tx) = app.picks.start(Slot::A);
        tx.send(Picked::Chosen {
            pick,
            name: "flight.bin".into(),
        })
        .unwrap();
        frame(&mut app, &ctx);
        app.arm(Pending::Pair, &ctx);
        assert!(app.picks.reading().is_none());
        tx.send(Picked::File {
            pick,
            name: "flight.bin".into(),
            bytes: crate::model::testlog::bytes(),
        })
        .unwrap();
        frames_until_idle(&mut app, &ctx);
        frame(&mut app, &ctx);
        assert!(app.running().is_none(), "the read opens nothing");
        assert_eq!(app.log().unwrap().name, "demo.bin");
        assert_eq!(app.log_b().unwrap().name, "demo2.bin");
    }

    #[test]
    fn the_demo_pair_opens_from_the_menu_with_its_differences() {
        let ctx = Context::default();
        let mut app = AftermissionApp::default();
        app.settings.online_tiles = false;
        app.settings.bottom_tab = BottomTab::Parameters;
        with_ui(&mut app, |harness| {
            harness.get_by_label("File").click();
            harness.run();
            harness.get_by_label("Open demo pair").click();
            harness.run_ok();
        });
        wait(&mut app, &ctx);
        assert!(app.error.is_none(), "{:?}", app.error);
        assert_eq!(app.log().unwrap().name, "demo.bin");
        assert_eq!(app.log_b().unwrap().name, "demo2.bin");
        assert!(app.settings.recent.is_empty(), "no path to offer again");
        with_ui(&mut app, |harness| {
            harness.get_by_label("demo2.bin");
            // the table narrows to the settings apart: the rate gains, the
            // mission's length, and the survey speed B starts at and A
            // ends at
            harness.get_by_label("4 differ");
            harness.get_by_label("Differences").click();
            harness.run();
            for name in ["ATC_RAT_PIT_P", "ATC_RAT_RLL_P", "MIS_TOTAL", "WPNAV_SPEED"] {
                harness.get_by_label_contains(name);
            }
            assert!(harness.query_by_label_contains("FENCE_RADIUS").is_none());
            // the speed ends at 1000 in both, marked as apart at boot
            harness.get_by_label("1000 *");
            // the learned values wait for their toggle
            harness.get_by_label("In one log only (0)");
            assert!(harness.query_by_label_contains("MOT_THST_HOVER").is_none());
            assert!(harness.query_by_label_contains("BARO1_GND_PRESS").is_none());
            harness.get_by_label("Learned values (2)").click();
            harness.run();
            harness.get_by_label_contains("MOT_THST_HOVER");
            harness.get_by_label_contains("BARO1_GND_PRESS");
            harness.get_by_label("6 differ");
            // Differences off takes its additions with it, so the count
            // is what the toggle shows when turned on again
            harness.get_by_label("Differences").click();
            harness.run();
            assert!(harness.query_by_label("Learned values (2)").is_none());
            harness.get_by_label("4 differ");
            harness.get_by_label("Differences").click();
            harness.run();
            harness.get_by_label("Learned values (2)");
            harness.get_by_label("4 differ");
            assert!(harness.query_by_label_contains("MOT_THST_HOVER").is_none());
        });
    }

    #[test]
    fn a_seek_from_the_parameters_tab_scrolls_the_events_list_on_return() {
        let ctx = Context::default();
        let mut app = AftermissionApp::default();
        app.settings.online_tiles = false;
        app.job = Some(Job::open_demo(ctx.clone()));
        wait(&mut app, &ctx);
        // the scroll animates over a few frames
        let mut harness = Harness::builder()
            .with_size(egui::vec2(1400.0, 900.0))
            .with_max_steps(60)
            .build_ui(|ui| app.show(ui));
        harness.run();
        harness.get_by_label_contains("ArduCopter V4.7.0");
        harness.get_by_label("Parameters").click();
        harness.run();
        harness.get_by_label("Changed after boot").click();
        harness.run();
        harness.get_by_label_contains("WPNAV_SPEED").click();
        harness.run();
        harness.get_by_label_contains("4:10.000").click();
        harness.run();
        // the events list was not on screen for the seek; it follows it
        // when it shows again
        harness.get_by_label("Events").click();
        harness.run();
        assert!(
            harness
                .query_by_label_contains("ArduCopter V4.7.0")
                .is_none(),
            "the list scrolled to the change"
        );
        // the rows around 250 s are the survey's mission lines
        assert!(harness.get_all_by_label_contains("Mission: ").count() > 0);
        drop(harness);
        let cursor = app.plot().cursor.unwrap();
        assert!((cursor - 250.0).abs() < 1e-6, "{cursor}");
        assert!(!*app.plot.follow_mut(), "the list took the seek");
    }

    #[test]
    fn the_armed_demo_shows_its_label_then_opens() {
        let ctx = Context::default();
        let mut app = AftermissionApp::default();
        app.settings.online_tiles = false;
        app.arm_demo(&ctx);
        assert!(app.armed.is_some() && app.job.is_none());
        assert_eq!(app.running().as_deref(), Some(demo::LABEL));
        // a second request while the scan is armed is refused, with the
        // notice an open by path gets while a job runs
        app.arm_demo(&ctx);
        assert!(app.armed.is_some() && app.job.is_none());
        assert_eq!(
            app.notice.as_ref().map(|n| n.text.clone()),
            Some(format!("Still busy: {}\u{2026}", demo::LABEL))
        );
        // armed between frames, the label is up for the next two, as a
        // pick's is: egui counts a frame at its end, so the first frame
        // has the number the arming saw; the third starts the job under it
        for painted in 1..=2 {
            frame(&mut app, &ctx);
            assert!(
                app.armed.is_some() && app.job.is_none(),
                "frame {painted} paints the label"
            );
            assert_eq!(app.running().as_deref(), Some(demo::LABEL));
        }
        frame(&mut app, &ctx);
        assert!(app.armed.is_none() && app.job.is_some(), "the third starts");
        assert_eq!(app.running().as_deref(), Some(demo::LABEL));
        frames_until_idle(&mut app, &ctx);
        assert!(app.error.is_none(), "{:?}", app.error);
        assert_eq!(app.log().unwrap().name, "demo.bin");
        assert!(app.settings.recent.is_empty());
    }

    #[test]
    fn the_armed_demo_pair_shows_its_label_then_opens_both() {
        let ctx = Context::default();
        let mut app = AftermissionApp::default();
        app.settings.online_tiles = false;
        // as the page's `?compare` asks
        app.arm(Pending::Pair, &ctx);
        assert_eq!(app.running().as_deref(), Some(demo::PAIR_LABEL));
        frame(&mut app, &ctx);
        assert!(app.armed.is_some() && app.job.is_none());
        frames_until_idle(&mut app, &ctx);
        assert!(app.error.is_none(), "{:?}", app.error);
        assert_eq!(app.log().unwrap().name, demo::NAME);
        assert_eq!(app.log_b().unwrap().name, demo::NAME_SECOND);
    }

    /// Run one frame of the app on `ctx`, as the browser would between
    /// presented frames. A kittest harness runs frames until the window is
    /// still when it is built, so a count of frames goes through this
    /// instead. Nothing paints, so the texture updates are dropped.
    fn frame(app: &mut AftermissionApp, ctx: &Context) {
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| app.show(ui));
        output.textures_delta.clear();
    }

    /// Run frames, one at least, until nothing is armed or running,
    /// polling the picks as the window does, unlike [`wait`].
    fn frames_until_idle(app: &mut AftermissionApp, ctx: &Context) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            frame(app, ctx);
            if app.job.is_none() && app.armed.is_none() {
                return;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the job did not finish"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    /// Send `message` as a browser callback would and run one frame.
    fn pick_frame(app: &mut AftermissionApp, message: crate::picks::Picked) {
        let (_, tx) = app.picks.start(Slot::A);
        tx.send(message).unwrap();
        frame(app, &Context::default());
    }

    #[test]
    fn a_picked_log_shows_its_label_then_opens() {
        use crate::picks::Picked;
        let mut app = AftermissionApp::default();
        app.settings.online_tiles = false;
        let (pick, tx) = app.picks.start(Slot::A);
        tx.send(Picked::Chosen {
            pick,
            name: "flight.bin".into(),
        })
        .unwrap();
        with_busy_ui(&mut app, |harness| {
            harness.get_by_label("Reading flight.bin\u{2026}");
        });
        let bytes = crate::model::testlog::bytes();
        tx.send(Picked::File {
            pick,
            name: "flight.bin".into(),
            bytes: bytes.clone(),
        })
        .unwrap();
        // the frame that takes the bytes shows the label and no log yet,
        // and so does the next; the one after starts the scan
        let label = worker::indexing_label("flight.bin", Some(bytes.len() as u64));
        let ctx = Context::default();
        frame(&mut app, &ctx);
        assert_eq!(app.running().as_deref(), Some(label.as_str()));
        assert!(app.armed.is_some() && app.job.is_none() && app.log.is_none());
        // a second choice while the scan is armed is refused, as an open by
        // path is while a job runs; nothing runs on a thread yet, so the
        // refusal cannot race the scan
        let (late, tx) = app.picks.start(Slot::A);
        tx.send(Picked::Chosen {
            pick: late,
            name: "other.bin".into(),
        })
        .unwrap();
        tx.send(Picked::File {
            pick: late,
            name: "other.bin".into(),
            bytes: Vec::new(),
        })
        .unwrap();
        frame(&mut app, &ctx);
        assert_eq!(app.running().as_deref(), Some(label.as_str()));
        assert!(app.armed.is_some() && app.job.is_none() && app.log.is_none());
        assert_eq!(
            app.notice.as_ref().map(|n| n.text.clone()),
            Some(format!("Still busy: {label}\u{2026}"))
        );
        assert!(app.picks.reading().is_none());
        frame(&mut app, &ctx);
        assert!(app.armed.is_none() && app.job.is_some(), "the scan started");
        assert_eq!(app.running().as_deref(), Some(label.as_str()));
        frames_until_idle(&mut app, &ctx);
        assert_eq!(app.log().unwrap().name, "flight.bin");
        assert!(app.error.is_none(), "{:?}", app.error);
    }

    #[test]
    fn a_pick_that_fails_says_so() {
        use crate::picks::Picked;
        let mut app = AftermissionApp::default();
        app.settings.online_tiles = false;
        let (pick, tx) = app.picks.start(Slot::A);
        tx.send(Picked::Chosen {
            pick,
            name: "photos".into(),
        })
        .unwrap();
        pick_frame(
            &mut app,
            Picked::Unreadable {
                pick,
                name: "photos".into(),
                dropped: true,
            },
        );
        let error = app.error.clone().unwrap();
        assert_eq!(
            error,
            "photos: the browser could not read it. A folder cannot be opened; drop the log file itself."
        );
        with_ui(&mut app, |harness| {
            harness.get_by_label(error.as_str());
        });

        // a chosen file that fails gets no advice about folders
        let (pick, tx) = app.picks.start(Slot::A);
        tx.send(Picked::Chosen {
            pick,
            name: "flight.bin".into(),
        })
        .unwrap();
        pick_frame(
            &mut app,
            Picked::Unreadable {
                pick,
                name: "flight.bin".into(),
                dropped: false,
            },
        );
        assert_eq!(
            app.error.as_deref(),
            Some("flight.bin: the browser could not read it.")
        );

        // a refusal and a read that land together: the refusal shows, and
        // the next frame arms the read
        let ctx = Context::default();
        let (reading, tx) = app.picks.start(Slot::A);
        let (refused, _) = app.picks.start(Slot::A);
        tx.send(Picked::Chosen {
            pick: reading,
            name: "flight.bin".into(),
        })
        .unwrap();
        tx.send(Picked::Refused { pick: refused }).unwrap();
        tx.send(Picked::File {
            pick: reading,
            name: "flight.bin".into(),
            bytes: crate::model::testlog::bytes(),
        })
        .unwrap();
        frame(&mut app, &ctx);
        assert!(
            app.error
                .as_deref()
                .unwrap()
                .starts_with("The browser did not open")
        );
        assert!(app.armed.is_none(), "one outcome a frame");
        frame(&mut app, &ctx);
        assert!(app.armed.is_some(), "the next frame arms the read");
        app.armed = None;

        app.error = None;
        let (pick, _) = app.picks.start(Slot::A);
        pick_frame(&mut app, Picked::Canceled { pick });
        assert!(app.error.is_none() && app.armed.is_none() && app.job.is_none());
    }

    #[test]
    fn a_newer_choice_wins_over_an_older_read() {
        use crate::picks::Picked;
        let mut app = AftermissionApp::default();
        app.settings.online_tiles = false;
        let (first, tx) = app.picks.start(Slot::A);
        let (second, _) = app.picks.start(Slot::A);
        let (third, _) = app.picks.start(Slot::A);
        for (pick, name) in [(first, "a.bin"), (second, "b.bin")] {
            tx.send(Picked::Chosen {
                pick,
                name: name.into(),
            })
            .unwrap();
        }
        tx.send(Picked::Canceled { pick: third }).unwrap();
        // the older read lands first: dropped, and the newer is still waited on
        tx.send(Picked::File {
            pick: first,
            name: "a.bin".into(),
            bytes: crate::model::testlog::bytes(),
        })
        .unwrap();
        with_busy_ui(&mut app, |harness| {
            harness.get_by_label("Reading b.bin\u{2026}");
        });
        assert!(app.armed.is_none() && app.job.is_none() && app.log.is_none());
        tx.send(Picked::File {
            pick: second,
            name: "b.bin".into(),
            bytes: crate::model::testlog::bytes(),
        })
        .unwrap();
        let ctx = Context::default();
        frames_until_idle(&mut app, &ctx);
        assert_eq!(app.log().unwrap().name, "b.bin");
    }

    /// A log of a hundred seconds, an attitude record a second from 1 s,
    /// written to `dir` and opened.
    fn long_flight(dir: &Path) -> AftermissionApp {
        use dflog::access::Value;
        use dflog::write::LogWriter;

        let mut w = LogWriter::new();
        w.define(1, "ATT", "Qf", &["TimeUS", "Roll"]).unwrap();
        for second in 1..=100u64 {
            w.record(
                "ATT",
                &[Value::U64(second * 1_000_000), Value::F64(second as f64)],
            )
            .unwrap();
        }
        let path = dir.join("long.bin");
        std::fs::write(&path, w.into_bytes()).unwrap();
        let mut app = AftermissionApp::default();
        app.settings.online_tiles = false;
        reopen(&mut app, &path);
        assert_eq!(app.log().unwrap().time_range(), Some(1.0..=100.0));
        app
    }

    /// `app` in a window with room for every panel, a twentieth of a
    /// second a frame. Playing asks for frames without end, so the tests
    /// step rather than run to a still window.
    fn player(app: AftermissionApp) -> Harness<'static, AftermissionApp> {
        Harness::builder()
            .with_size(egui::vec2(1400.0, 900.0))
            .with_step_dt(0.05)
            .build_ui_state(|ui, app: &mut AftermissionApp| app.show(ui), app)
    }

    fn playhead(harness: &Harness<'_, AftermissionApp>) -> f64 {
        harness
            .state()
            .plot
            .playhead()
            .expect("the playhead has moved")
    }

    fn playing(harness: &Harness<'_, AftermissionApp>) -> bool {
        harness.state().playback.is_playing()
    }

    #[test]
    fn keys_play_step_and_change_the_speed() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = long_flight(dir.path());
        app.settings.speed = Speed::from(5);
        assert_eq!(app.settings.speed.label(), "10\u{d7}");
        let mut harness = player(app);
        harness.get_by_label("0:00.0 / 1:39.0");

        // Space plays from the log's first time; the frame it lands in
        // counts no time, the next one does
        harness.key_press(egui::Key::Space);
        harness.step();
        assert!(playing(&harness));
        let from = playhead(&harness);
        assert!((from - 1.5).abs() < 1e-5, "{from}");
        // ten frames of a twentieth of a second at 10x: five seconds
        for _ in 0..10 {
            harness.step();
        }
        let at = playhead(&harness);
        assert!((at - from - 5.0).abs() < 1e-5, "{from} to {at}");
        assert_eq!(
            harness.state().plot.cursor,
            Some(at),
            "the cursor goes along"
        );

        // a seek while playing, as the map's or the events list's, is
        // where playing goes on from
        harness.state_mut().plot.seek(50.0);
        harness.step();
        let at = playhead(&harness);
        assert!((at - 50.5).abs() < 1e-5, "{at}");

        // Space pauses; Right steps a second forward, ten with Shift, and
        // Left back, within the log's time
        harness.key_press(egui::Key::Space);
        harness.step();
        assert!(!playing(&harness));
        let paused = playhead(&harness);
        harness.key_press(egui::Key::ArrowRight);
        harness.step();
        assert!((playhead(&harness) - paused - 1.0).abs() < 1e-9);
        harness.key_press_modifiers(egui::Modifiers::SHIFT, egui::Key::ArrowRight);
        harness.step();
        assert!((playhead(&harness) - paused - 11.0).abs() < 1e-9);
        harness.key_press(egui::Key::ArrowLeft);
        harness.step();
        assert!((playhead(&harness) - paused - 10.0).abs() < 1e-9);
        for _ in 0..8 {
            harness.key_press_modifiers(egui::Modifiers::SHIFT, egui::Key::ArrowLeft);
        }
        harness.step();
        assert_eq!(harness.state().plot.playhead(), Some(1.0));
        assert!(!playing(&harness), "stepping does not play");

        // ] and [ step the speed, which the settings keep
        harness.key_press(egui::Key::CloseBracket);
        harness.step();
        assert_eq!(harness.state().settings.speed.label(), "30\u{d7}");
        harness.key_press(egui::Key::OpenBracket);
        harness.key_press(egui::Key::OpenBracket);
        harness.step();
        assert_eq!(harness.state().settings.speed.label(), "5\u{d7}");
    }

    #[test]
    fn playing_stops_at_the_end_and_play_there_starts_over() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = long_flight(dir.path());
        app.settings.speed = Speed::from(7);
        let mut harness = player(app);
        harness.state_mut().plot.seek(99.0);
        harness.key_press(egui::Key::Space);
        harness.step();
        assert!(!playing(&harness), "60x runs out within a frame");
        assert_eq!(harness.state().plot.playhead(), Some(100.0));
        harness.get_by_label("1:39.0 / 1:39.0");
        harness.key_press(egui::Key::Space);
        harness.step();
        assert!(playing(&harness));
        let at = playhead(&harness);
        assert!(at < 5.0, "from the start again: {at}");
    }

    #[test]
    fn a_widget_with_the_keyboard_keeps_space_and_the_arrows() {
        let dir = tempfile::tempdir().unwrap();
        let mut harness = player(long_flight(dir.path()));

        // with the side panel's filter focused, Space types a space there
        let side = panel_rect(&harness, "side_panel");
        let filter = harness
            .get_all_by_role(egui::accesskit::Role::TextInput)
            .find(|node| side.contains(node.rect().center()))
            .expect("the side panel has its filter");
        filter.focus();
        harness.step();
        harness.key_press(egui::Key::Space);
        harness
            .get_all_by_role(egui::accesskit::Role::TextInput)
            .find(|node| side.contains(node.rect().center()))
            .unwrap()
            .type_text(" ");
        harness.step();
        assert_eq!(harness.state().filter, " ");
        assert!(!playing(&harness));

        // a widget reached with Tab takes Space as its click, and the
        // arrows as its own, and the transport leaves them to it
        harness.get_by_label("ATT (100)").focus();
        harness.step();
        harness.key_press(egui::Key::Space);
        harness.step();
        harness.get_by_label("Roll");
        harness.key_press(egui::Key::ArrowRight);
        harness.step();
        assert!(!playing(&harness));
        assert_eq!(harness.state().plot.playhead(), None);

        // the Play button reached so plays once by Space, as its click
        harness.get_by_label("\u{23F5}").focus();
        harness.step();
        harness.key_press(egui::Key::Space);
        harness.step();
        assert!(playing(&harness));
        // and with no widget focused, Space is the transport's again
        harness.key_press(egui::Key::Escape);
        harness.step();
        harness.key_press(egui::Key::Space);
        harness.step();
        assert!(!playing(&harness));
    }

    #[test]
    fn opening_a_log_or_an_export_dialog_stops_playing() {
        let dir = tempfile::tempdir().unwrap();
        let mut harness = player(long_flight(dir.path()));
        let path = dir.path().join("long.bin");

        // a new log stops playing when it arrives, and starts with no
        // playhead
        harness.key_press(egui::Key::Space);
        harness.step();
        assert!(playing(&harness));
        let ctx = harness.ctx.clone();
        harness.state_mut().open_path(&path, &ctx);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while harness.state().job.is_some() {
            assert!(
                std::time::Instant::now() < deadline,
                "the open did not finish"
            );
            harness.step();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(!playing(&harness));
        assert_eq!(harness.state().plot.playhead(), None);

        // an export dialog pauses, so the range in view holds still under
        // it, and Space leaves it paused
        harness.state_mut().toggle_series(crate::model::SeriesKey {
            type_name: "ATT".into(),
            field: "Roll".into(),
            instance: None,
        });
        harness.key_press(egui::Key::Space);
        harness.step();
        assert!(playing(&harness));
        harness.get_by_label("File").click();
        harness.step();
        harness.get_by_label("Export \u{23F5}").hover();
        harness.step();
        harness
            .get_by_label("Plotted series as CSV\u{2026}")
            .click();
        harness.step();
        assert!(harness.state().csv_dialog.is_some());
        harness.step();
        assert!(!playing(&harness));
        // and holds it still while it is open: Space, the Play button and
        // the steps do nothing
        harness.key_press(egui::Key::Space);
        harness.step();
        let play = harness.get_by_label("\u{23F5}");
        assert!(play.accesskit_node().is_disabled());
        play.click();
        harness.step();
        assert!(!playing(&harness));
        let at = harness.state().plot.playhead();
        harness.key_press(egui::Key::ArrowRight);
        harness.step();
        assert_eq!(harness.state().plot.playhead(), at);

        // the About window, which moves nothing, leaves the keys be
        harness.get_by_label("Cancel").click();
        harness.step();
        assert!(harness.state().csv_dialog.is_none());
        harness.state_mut().about_open = true;
        harness.step();
        harness.key_press(egui::Key::Space);
        harness.step();
        assert!(playing(&harness));
    }

    #[test]
    fn a_drag_of_the_slider_pauses_until_it_ends() {
        let dir = tempfile::tempdir().unwrap();
        let mut harness = player(long_flight(dir.path()));
        harness.key_press(egui::Key::Space);
        harness.step();
        assert!(playing(&harness));

        let rail = harness.get_by_role(egui::accesskit::Role::Slider).rect();
        let from = egui::pos2(rail.left() + rail.width() * 0.5, rail.center().y);
        harness.hover_at(from);
        harness.step();
        harness.drag_at(from);
        harness.step();
        for step in 1..=5u8 {
            let x = rail.width() * 0.05 * f32::from(step);
            harness.hover_at(from + egui::vec2(x, 0.0));
            harness.step();
            assert!(!playing(&harness), "paused while the hand moves it");
        }
        // three quarters along, give or take the handle's margins
        let at = playhead(&harness);
        assert!((65.0..85.0).contains(&at), "{at}");
        harness.drop_at(from + egui::vec2(rail.width() * 0.25, 0.0));
        harness.step();
        assert!(playing(&harness), "playing again once it ends");
    }

    #[test]
    fn a_log_without_a_timed_record_disables_the_transport() {
        use dflog::access::Value;
        use dflog::write::LogWriter;

        let mut w = LogWriter::new();
        w.define(8, "NOTM", "Bf", &["Idx", "Value"]).unwrap();
        w.record("NOTM", &[Value::U64(7), Value::F64(42.0)])
            .unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("untimed.bin");
        std::fs::write(&path, w.into_bytes()).unwrap();
        let mut app = AftermissionApp::default();
        app.settings.online_tiles = false;
        reopen(&mut app, &path);
        with_ui(&mut app, |harness| {
            harness.get_by_label("No timed records");
            assert!(
                harness
                    .get_by_label("\u{23F5}")
                    .accesskit_node()
                    .is_disabled()
            );
            harness.key_press(egui::Key::Space);
            harness.run();
        });
        assert!(!app.playback.is_playing());
        assert_eq!(app.plot.playhead(), None);
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

    /// The README's pictures, rendered from the demo log by the real UI.
    /// Ignored in the ordinary run since it needs a GPU and writes files:
    /// `cargo test --release -p aftermission -- --ignored hero_shots`
    /// writes them to `assets/hero/`, or to `$HERO_DIR`. One harness serves
    /// every shot: a texture belongs to the context that made it. Unlike
    /// every other test, this one has the map's tiles on, so it needs the
    /// network; the few dozen it takes go through the app's tile cache,
    /// with its user agent, as the app's own would.
    #[test]
    #[ignore = "renders the README pictures; run on demand"]
    fn hero_shots() {
        let out = hero_dir();
        let ctx = Context::default();
        let mut app = AftermissionApp::default();
        app.settings.time_axis = TimeAxis::Utc;
        app.open_demo(&ctx);
        wait(&mut app, &ctx);
        let fault = event_time(&app, "GPS: unhealthy");
        let speed_change = event_time(&app, "WPNAV_SPEED 800 -> 1000");
        let turn = event_time(&app, "Mission: 4 WP");
        // the events list's scroll animates over a few frames
        let mut harness = Harness::builder()
            .with_pixels_per_point(1.0)
            .with_max_steps(60)
            .wgpu()
            .build_ui_state(|ui, app: &mut AftermissionApp| app.show(ui), app);
        window(&mut harness, 1440.0, 900.0);
        // the side panel dragged a third narrower, for the plot's sake
        let wide = panel_rect(&harness, "side_panel").width();
        drag_side_panel(&mut harness, -wide / 3.0);
        let narrow = panel_rect(&harness, "side_panel").width();
        assert!(
            (narrow - wide * 2.0 / 3.0).abs() < 1.0,
            "{wide} to {narrow}"
        );

        // The whole window at the GPS fault: roll against desired roll on
        // the left axis, altitude on the right, picked in the tree as a
        // user does.
        harness.get_by_label_contains("ATT (").click();
        harness.run();
        harness.get_by_label("Roll (deg)").click();
        harness.get_by_label("DesRoll (deg)").click();
        harness.run();
        harness.get_by_label_contains("CTUN (").click();
        harness.run();
        harness.get_by_label("Alt (m)").click();
        harness.run();
        harness.get_all_by_label("L").nth(2).unwrap().click();
        harness.run();
        // The seeks of two clicks on the map: past the fault, which scrolls
        // the events list down to its resolution, then on the fault, which
        // the list already shows.
        harness.state_mut().plot.seek(fault + 2.5);
        harness.run();
        harness.state_mut().plot.seek(fault + 0.5);
        harness.run();
        harness.get_by_label_contains("GPS: resolved");
        shoot(&mut harness, &out, "overview");

        // The speed over the flight above the parameter table on WPNAV,
        // the speed parameter unfolded to its history and its change
        // clicked, the side panel folded away and the table dragged taller.
        harness.get_by_label_contains("ATT (").click();
        harness.get_by_label_contains("CTUN (").click();
        harness.run();
        harness.state_mut().plot.clear();
        harness.get_by_label_contains("GPS (").click();
        harness.run();
        harness.get_by_label("Spd (m/s)").click();
        harness.run();
        harness.state_mut().settings.side_panel = false;
        harness.run();
        harness.get_by_label("Parameters").click();
        harness.run();
        harness.state_mut().params_mut().set_filter("WPNAV");
        harness.run();
        harness.get_by_label("\u{25b8} WPNAV_SPEED").click();
        harness.run();
        drag_bottom_panel(&mut harness, -120.0);
        harness.get_by_label_contains("14:34:10").click();
        harness.run();
        let cursor = harness.state().plot().cursor.unwrap();
        assert!((cursor - speed_change).abs() < 1e-6, "{cursor}");
        shoot(&mut harness, &out, "parameters");

        // The plot alone, zoomed on a turn of the survey: roll against
        // desired roll with the legend and the readout.
        harness.state_mut().plot.clear();
        harness.state_mut().settings.side_panel = true;
        harness.run();
        // GPS folds again and ATT unfolds
        harness.get_by_label_contains("GPS (").click();
        harness.get_by_label_contains("ATT (").click();
        harness.run();
        harness.get_by_label("Roll (deg)").click();
        harness.get_by_label("DesRoll (deg)").click();
        harness.run();
        let settings = &mut harness.state_mut().settings;
        settings.side_panel = false;
        settings.show_map = false;
        settings.show_bottom = false;
        window(&mut harness, 1440.0, 720.0);
        PlotPanel::zoom_to(&harness.ctx, turn - 8.0..=turn + 16.0);
        // the pointer, last on the panel's edge, would move the cursor
        harness.event(egui::Event::PointerGone);
        harness.state_mut().plot.seek(turn + 2.0);
        harness.run();
        shoot(&mut harness, &out, "survey-turn");
    }

    /// The README's picture of two logs compared, rendered from the demo
    /// pair by the real UI, as [`hero_shots`] renders the others and into
    /// the same folder: the pitch of both flights lined up at arming, B's
    /// stronger wind leaning it further on the legs, over the parameters
    /// that differ. The map stays closed, so nothing is downloaded:
    /// `cargo test --release -p aftermission -- --ignored hero_shot_compare`
    #[test]
    #[ignore = "renders a README picture; run on demand"]
    fn hero_shot_compare() {
        use crate::model::{SeriesKey, nearest_within};

        let out = hero_dir();
        let ctx = Context::default();
        let mut app = AftermissionApp::default();
        let settings = &mut app.settings;
        settings.time_axis = TimeAxis::Utc;
        settings.online_tiles = false;
        settings.side_panel = false;
        settings.show_map = false;
        settings.bottom_tab = BottomTab::Parameters;
        app.open_demo_pair(&ctx);
        wait(&mut app, &ctx);
        let pitch = SeriesKey {
            type_name: "ATT".into(),
            field: "Pitch".into(),
            instance: None,
        };
        app.toggle_series(pitch.clone());
        app.params_mut().show_differences();
        // a moment on a leg flown east by both, each leaning into the wind
        let (a, b) = (app.log().unwrap(), app.log_b().unwrap());
        let (a_pitch, b_pitch) = (a.series(&pitch).unwrap(), b.series(&pitch).unwrap());
        let offset = app.log_b.as_ref().unwrap().offset();
        let moment = a_pitch
            .xs
            .iter()
            .zip(&a_pitch.ys)
            .find(|&(&x, &y)| {
                x > 150.0
                    && y > 6.0
                    && nearest_within(&b_pitch.xs, x - offset).is_some_and(|i| b_pitch.ys[i] > 9.0)
            })
            .map(|(&x, _)| x)
            .expect("the flights share an eastbound leg");
        let mut harness = Harness::builder()
            .with_pixels_per_point(1.0)
            .with_max_steps(60)
            .wgpu()
            .build_ui_state(|ui, app: &mut AftermissionApp| app.show(ui), app);
        window(&mut harness, 1440.0, 900.0);
        drag_bottom_panel(&mut harness, -60.0);
        PlotPanel::zoom_to(&harness.ctx, moment - 90.0..=moment + 90.0);
        harness.event(egui::Event::PointerGone);
        harness.state_mut().plot.seek(moment);
        harness.run();
        harness.get_by_label("4 differ");
        shoot(&mut harness, &out, "compare");
    }

    /// A pair of logs for a check by hand of what only B logs: the test
    /// log as `a.bin` and [`testlog::b_extra_bytes`] as `b.bin`, written
    /// to `target/only-in-b` (or `$PAIR_DIR`), to open as A and compare
    /// with as B, in the app or the page. Run on demand:
    /// `cargo test -p aftermission -- --ignored write_only_in_b_pair`
    ///
    /// [`testlog::b_extra_bytes`]: crate::model::testlog::b_extra_bytes
    #[test]
    #[ignore = "writes a pair of logs for a check by hand; run on demand"]
    fn write_only_in_b_pair() {
        let out = std::env::var_os("PAIR_DIR").map_or_else(
            || PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/only-in-b"),
            PathBuf::from,
        );
        std::fs::create_dir_all(&out).unwrap();
        std::fs::write(out.join("a.bin"), crate::model::testlog::bytes()).unwrap();
        std::fs::write(out.join("b.bin"), crate::model::testlog::b_extra_bytes()).unwrap();
        eprintln!("wrote a.bin and b.bin to {}", out.display());
    }

    /// The plot's frame cost, the bench the frame-cost work is judged by:
    /// ten series played at 5x, each frame's `run_ui` and tessellation
    /// timed over 300 frames, the median and p95 printed, and what the
    /// thinning of the lines takes of it measured apart, as ten thinnings
    /// of every line afresh. The cases: the demo alone and as a pair, a
    /// synthetic 30-minute 400 Hz log alone and against a second, each
    /// with the whole flight in view and with a minute. Run on demand, in
    /// release, which is what counts:
    /// `cargo test --release -p aftermission -- --ignored frame_cost --nocapture`
    /// With `FRAME_COST_DIR` set, the synthetic pair is written there as
    /// `long.bin` and `long2.bin`, to load in the page.
    #[test]
    #[ignore = "measures the frame cost; run on demand"]
    fn frame_cost() {
        let demo_keys = [
            ("ATT", "Roll", None),
            ("ATT", "DesRoll", None),
            ("ATT", "Pitch", None),
            ("ATT", "Yaw", None),
            ("IMU", "GyrX", Some(0)),
            ("IMU", "GyrY", Some(0)),
            ("IMU", "AccZ", Some(0)),
            ("IMU", "AccZ", Some(1)),
            ("CTUN", "Alt", None),
            ("GPS", "Spd", None),
        ];
        let ctx = Context::default();
        let mut app = AftermissionApp::default();
        app.settings.online_tiles = false;
        app.open_demo(&ctx);
        wait(&mut app, &ctx);
        ten_series(&mut app, &demo_keys);
        measure_frames(&mut app, "demo, whole flight", None);
        // the series carry over to the pair
        app.open_demo_pair(&ctx);
        wait(&mut app, &ctx);
        assert!(app.log_b().is_some(), "{:?}", app.error);
        assert_eq!(app.plot.selected.len(), 10);
        measure_frames(&mut app, "demo pair, whole flight", None);

        let dir = std::env::var_os("FRAME_COST_DIR").map_or_else(
            || std::env::temp_dir().join("aftermission-frame-cost"),
            PathBuf::from,
        );
        std::fs::create_dir_all(&dir).unwrap();
        let (long, long2) = (dir.join("long.bin"), dir.join("long2.bin"));
        std::fs::write(&long, synthetic_log(0.0, 1.0)).unwrap();
        std::fs::write(&long2, synthetic_log(0.3, 1.1)).unwrap();
        println!(
            "synthetic pair in {}: {} MB each",
            dir.display(),
            std::fs::metadata(&long).unwrap().len() / 1_000_000
        );
        let fields = ["A", "B", "C", "D", "E", "F", "G", "H", "I", "J"];
        let syn_keys: Vec<(&str, &str, Option<i64>)> =
            fields.iter().map(|f| ("SYN", *f, None)).collect();
        let mut app = AftermissionApp::default();
        app.settings.online_tiles = false;
        reopen(&mut app, &long);
        ten_series(&mut app, &syn_keys);
        measure_frames(&mut app, "synthetic, whole flight", None);
        measure_frames(&mut app, "synthetic, a minute in view", Some(600.0..=660.0));
        app.open_pair(&long, &long2, &ctx);
        wait(&mut app, &ctx);
        assert!(app.log_b().is_some(), "{:?}", app.error);
        assert_eq!(app.plot.selected.len(), 10);
        measure_frames(&mut app, "synthetic pair, whole flight", None);
        measure_frames(
            &mut app,
            "synthetic pair, a minute in view",
            Some(600.0..=660.0),
        );
    }

    /// One frame of the app at 1400 by 900 points, the clock at `i`
    /// sixtieths of a second, timed in milliseconds through tessellation.
    fn timed_frame(ctx: &Context, app: &mut AftermissionApp, i: usize) -> f64 {
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1400.0, 900.0));
        let input = egui::RawInput {
            screen_rect: Some(rect),
            time: Some(i as f64 / 60.0),
            ..Default::default()
        };
        let start = std::time::Instant::now();
        let mut output = ctx.run_ui(input, |ui| app.show(ui));
        output.textures_delta.clear();
        let _ = ctx.tessellate(output.shapes, output.pixels_per_point);
        start.elapsed().as_secs_f64() * 1000.0
    }

    /// Play 300 frames of `app` after 30 to settle, the view zoomed to
    /// `zoom` when given and the playhead put at its start, with ten more
    /// frames for the zoom itself to settle (the values fit the view anew
    /// and their labels resize the plot); at 5x the playhead stays in a
    /// minute's view, so a zoomed case times a still view, no page turn.
    /// Prints the frame times and what one thinning of every line takes.
    fn measure_frames(app: &mut AftermissionApp, label: &str, zoom: Option<RangeInclusive<f64>>) {
        let ctx = Context::default();
        for i in 0..30 {
            timed_frame(&ctx, app, i);
        }
        if let Some(range) = zoom {
            PlotPanel::zoom_to(&ctx, range.clone());
            app.plot.seek(*range.start() + 1.0);
            for i in 30..40 {
                timed_frame(&ctx, app, i);
            }
        }
        app.toggle_playback();
        assert!(app.playback.is_playing());
        let mut times: Vec<f64> = (40..340).map(|i| timed_frame(&ctx, app, i)).collect();
        app.playback.pause();
        times.sort_by(f64::total_cmp);
        let start = std::time::Instant::now();
        for _ in 0..10 {
            std::hint::black_box(app.plot.thin_all(&ctx));
        }
        let thinning = start.elapsed().as_secs_f64() * 100.0;
        println!(
            "{label}: median {:.2} ms, p95 {:.2} ms, max {:.2} ms; thinning {thinning:.2} ms",
            times[times.len() / 2],
            times[times.len() * 95 / 100],
            times[times.len() - 1]
        );
    }

    /// Thirty minutes of ten fields at 400 Hz as the bytes of a log, slow
    /// waves, each shifted by `shift` and scaled by `scale` so that two
    /// such logs differ.
    fn synthetic_log(shift: f64, scale: f64) -> Vec<u8> {
        use dflog::access::Value;
        use dflog::write::LogWriter;

        let mut w = LogWriter::new();
        let labels = ["TimeUS", "A", "B", "C", "D", "E", "F", "G", "H", "I", "J"];
        w.define(1, "SYN", "Qffffffffff", &labels).unwrap();
        for n in 0..(400u64 * 1800) {
            let t = n as f64 / 400.0;
            let mut values = vec![Value::U64(1_000_000 + n * 2500)];
            for k in 0..10 {
                let value = (t * (0.1 + f64::from(k) * 0.05) + shift).sin() * scale;
                values.push(Value::F64(value));
            }
            w.record("SYN", &values).unwrap();
        }
        w.into_bytes()
    }

    /// Put the ten series `keys` name on the plot.
    fn ten_series(app: &mut AftermissionApp, keys: &[(&str, &str, Option<i64>)]) {
        for (type_name, field, instance) in keys {
            app.toggle_series(crate::model::SeriesKey {
                type_name: (*type_name).into(),
                field: (*field).into(),
                instance: *instance,
            });
        }
        assert_eq!(app.plot.selected.len(), 10, "{:?}", app.plot.error);
    }

    /// Where the README's pictures go: `assets/hero/`, or `$HERO_DIR`,
    /// made when missing.
    fn hero_dir() -> PathBuf {
        let out = std::env::var_os("HERO_DIR").map_or_else(
            || PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/hero"),
            PathBuf::from,
        );
        std::fs::create_dir_all(&out).unwrap();
        out
    }

    /// The time of the demo's event reading `text`.
    fn event_time(app: &AftermissionApp, text: &str) -> f64 {
        app.log()
            .unwrap()
            .events
            .iter()
            .find(|e| e.text == text)
            .unwrap_or_else(|| panic!("the demo has {text:?}"))
            .time
    }

    /// The frame kittest draws around the app, cropped off each shot: a
    /// window has none.
    const KITTEST_MARGIN: f32 = 8.0;

    /// Size the app's window, inside kittest's frame, and run it.
    fn window(harness: &mut Harness<'_, AftermissionApp>, width: f32, height: f32) {
        let margin = 2.0 * KITTEST_MARGIN;
        harness.set_size(egui::vec2(width + margin, height + margin));
        harness.run();
    }

    /// Where the panel `id` was drawn, as it stores it: the edges come from
    /// here, since the central panel's margin keeps a panel off the
    /// window's edge.
    fn panel_rect(harness: &Harness<'_, AftermissionApp>, id: &str) -> egui::Rect {
        egui::containers::panel::PanelState::load(&harness.ctx, egui::Id::new(id))
            .expect("the panel has drawn")
            .outer_rect
    }

    /// Drag the bottom panel's top edge by `dy` points, up when negative.
    fn drag_bottom_panel(harness: &mut Harness<'_, AftermissionApp>, dy: f32) {
        let rect = panel_rect(harness, "bottom_panel");
        drag(
            harness,
            egui::pos2(rect.center().x, rect.min.y),
            egui::vec2(0.0, dy),
        );
    }

    /// Drag the side panel's right edge by `dx` points, left when negative.
    fn drag_side_panel(harness: &mut Harness<'_, AftermissionApp>, dx: f32) {
        let rect = panel_rect(harness, "side_panel");
        drag(
            harness,
            egui::pos2(rect.max.x, rect.center().y),
            egui::vec2(dx, 0.0),
        );
    }

    /// Press at `from`, move by `by` in five steps and let go, as a hand
    /// drags.
    fn drag(harness: &mut Harness<'_, AftermissionApp>, from: egui::Pos2, by: egui::Vec2) {
        harness.hover_at(from);
        harness.run();
        harness.drag_at(from);
        harness.run();
        for step in 1..=5u8 {
            harness.hover_at(from + by * f32::from(step) / 5.0);
            harness.run();
        }
        harness.drop_at(from + by);
        harness.run();
    }

    /// Run frames until no map tile has been downloading for a second, so
    /// the tiles the view asks for are in the shot.
    fn wait_for_tiles(harness: &mut Harness<'_, AftermissionApp>) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        let mut quiet = 0;
        while quiet < 20 {
            assert!(
                std::time::Instant::now() < deadline,
                "the tiles did not load"
            );
            harness.step();
            let loading = harness.state().map.tiles_in_progress().unwrap_or(0);
            quiet = if loading == 0 { quiet + 1 } else { 0 };
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }

    /// Fail unless the map's tiles are in `image`: a download that failed
    /// ends the wait as one that finished does, and would leave the map
    /// pane its plain dark ground. The tiles' ground is light, so most of
    /// the pane's middle is, where the track covers little of it. `map`
    /// is the pane in the image's pixels, one per point.
    fn assert_tiles_drawn(image: &image::RgbaImage, map: egui::Rect) {
        let middle = map.shrink2(map.size() / 4.0);
        let (mut light, mut all) = (0u32, 0u32);
        for y in middle.min.y as u32..middle.max.y as u32 {
            for x in middle.min.x as u32..middle.max.x as u32 {
                let [r, g, b, _] = image.get_pixel(x, y).0;
                all += 1;
                if r.min(g).min(b) > 160 {
                    light += 1;
                }
            }
        }
        assert!(
            light * 2 > all,
            "the map shows no tiles: {light} of {all} pixels light; is the network there?"
        );
    }

    /// Render the window after a few more frames, so the scroll areas
    /// settle, the fonts upload and the map's tiles arrive, and save it as
    /// `<name>.png` without kittest's frame. The pointer leaves first: a
    /// tooltip or a hover has no place in it.
    fn shoot(harness: &mut Harness<'_, AftermissionApp>, out: &Path, name: &str) {
        harness.event(egui::Event::PointerGone);
        wait_for_tiles(harness);
        harness.run_steps(8);
        let image = harness.render().expect("a GPU adapter renders the frame");
        let settings = &harness.state().settings;
        if settings.show_map && settings.online_tiles {
            assert_tiles_drawn(&image, panel_rect(harness, "map_panel"));
        }
        // one pixel per point
        let margin = KITTEST_MARGIN as u32;
        let (width, height) = (image.width() - 2 * margin, image.height() - 2 * margin);
        image::imageops::crop_imm(&image, margin, margin, width, height)
            .to_image()
            .save(out.join(format!("{name}.png")))
            .unwrap();
    }
}
