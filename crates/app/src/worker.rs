//! Work off the UI thread, so the window keeps painting: opening a log,
//! which maps, scans and models the file, and writing an export. One job
//! runs at a time. The browser has no thread to give, so there a job
//! runs to its end inside `run` and its result waits at the first poll.
//!
//! A log opens as A, the log under review, or as B, the one compared
//! with it; a pair opens both in one job.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, TryRecvError};

use dflog::Log;

use crate::demo::{self, Flight};
use crate::model::LoadedLog;

/// Which log an open fills: A, the log under review, or B, the one
/// compared with it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Slot {
    A,
    B,
}

impl Slot {
    /// What a line about the log ends with: nothing for A, ` as B` for B,
    /// as in `Indexing flight.bin as B`.
    #[must_use]
    pub fn suffix(self) -> &'static str {
        match self {
            Slot::A => "",
            Slot::B => " as B",
        }
    }

    /// What a row about one of the log's items starts with: nothing for
    /// A, `B ` for B, as in `B 0:12.30, 40.0 m`.
    #[must_use]
    pub fn prefix(self) -> &'static str {
        match self {
            Slot::A => "",
            Slot::B => "B ",
        }
    }

    /// Why an open failed, naming B: `B: flight.bin: ...`.
    #[must_use]
    pub fn error(self, message: &str) -> String {
        match self {
            Slot::A => message.to_string(),
            Slot::B => format!("B: {message}"),
        }
    }
}

/// What a job is doing, for where its outcome goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Opening a log into the slot, or a pair from A.
    Open(Slot),
    Export,
}

/// A log modeled, and the file it came from.
#[derive(Debug)]
pub struct Opened {
    /// None for a log with no path to open again: the bytes the browser
    /// read, or the demo.
    pub path: Option<PathBuf>,
    /// Boxed, so a job's outcome stays small while it waits.
    pub log: Box<LoadedLog>,
}

/// What a finished job hands back.
#[derive(Debug)]
pub enum Done {
    /// A log modeled for the job's slot: the file at `path`, or one with
    /// no path to open again. For a pair, opened in one job as A and B,
    /// `b` holds B's own outcome, so A opens while B fails.
    Opened {
        path: Option<PathBuf>,
        log: Box<LoadedLog>,
        b: Option<Result<Opened, String>>,
    },
    /// A file written at `path`, or in the browser downloaded under that
    /// name; `summary` is what the status line says.
    Exported { path: PathBuf, summary: String },
}

/// Work running on a worker thread.
pub struct Job {
    pub kind: Kind,
    /// The file the job reads or writes, which names it in an error.
    pub path: PathBuf,
    /// What the status line says while it runs: `Indexing flight.bin
    /// (12.3 MB)`, `Indexing baseline.bin (8.1 MB) as B`, `Writing
    /// flight.csv`.
    pub label: String,
    rx: Receiver<Result<Done, String>>,
}

impl std::fmt::Debug for Job {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Job")
            .field("kind", &self.kind)
            .field("path", &self.path)
            .field("label", &self.label)
            .finish_non_exhaustive()
    }
}

impl Job {
    /// Open and model the log at `path` in the background, as `slot`.
    #[cfg(not(target_arch = "wasm32"))]
    #[must_use]
    pub fn open(path: PathBuf, slot: Slot, ctx: egui::Context) -> Self {
        let label = indexing_label(
            &file_name(&path),
            std::fs::metadata(&path).ok().map(|m| m.len()),
        ) + slot.suffix();
        Self::run(Kind::Open(slot), path.clone(), label, ctx, move || {
            let Opened { path, log } = open_file(path)?;
            Ok(Done::Opened { path, log, b: None })
        })
    }

    /// Open and model the logs at `a` and `b` in the background, as A and
    /// B, in one job: A's failure fails the job, and B's rides beside A,
    /// so A opens while B fails.
    #[cfg(not(target_arch = "wasm32"))]
    #[must_use]
    pub fn open_pair(a: PathBuf, b: PathBuf, ctx: egui::Context) -> Self {
        let size = |path: &Path| std::fs::metadata(path).ok().map(|m| m.len());
        let label = format!(
            "Indexing {}, then {}{}",
            sized(&file_name(&a), size(&a)),
            sized(&file_name(&b), size(&b)),
            Slot::B.suffix()
        );
        Self::run(Kind::Open(Slot::A), a.clone(), label, ctx, move || {
            let Opened { path, log } = open_file(a)?;
            let b = Some(open_file(b));
            Ok(Done::Opened { path, log, b })
        })
    }

    /// Model a log read into memory as `name`, as `slot`: one the browser
    /// read, or a test's. It comes back with no path, since the browser
    /// has none to open again; the name stands for one in an error.
    #[cfg(any(target_arch = "wasm32", test))]
    #[must_use]
    pub fn open_bytes(name: String, bytes: Vec<u8>, slot: Slot, ctx: egui::Context) -> Self {
        let label = indexing_label(&name, Some(bytes.len() as u64)) + slot.suffix();
        Self::run(
            Kind::Open(slot),
            PathBuf::from(&name),
            label,
            ctx,
            move || {
                let log = Box::new(LoadedLog::build(Log::from_source(bytes.into()), name));
                Ok(Done::Opened {
                    path: None,
                    log,
                    b: None,
                })
            },
        )
    }

    /// Generate the demo flight and model it, both under the one label,
    /// since its size is not known before the work runs.
    #[must_use]
    pub fn open_demo(ctx: egui::Context) -> Self {
        let label = demo::LABEL.to_string();
        Self::run(
            Kind::Open(Slot::A),
            PathBuf::from(demo::NAME),
            label,
            ctx,
            || {
                let Opened { path, log } = demo_flight(Flight::A);
                Ok(Done::Opened { path, log, b: None })
            },
        )
    }

    /// Generate and model the demo pair: the demo flight as A and the
    /// second sortie as B.
    #[must_use]
    pub fn open_demo_pair(ctx: egui::Context) -> Self {
        let label = demo::PAIR_LABEL.to_string();
        Self::run(
            Kind::Open(Slot::A),
            PathBuf::from(demo::NAME),
            label,
            ctx,
            || {
                let Opened { path, log } = demo_flight(Flight::A);
                let b = Some(Ok(demo_flight(Flight::B)));
                Ok(Done::Opened { path, log, b })
            },
        )
    }

    /// Run `work`, which reads or writes `path`, on a worker thread.
    /// `ctx` is asked to repaint when the result is in.
    #[must_use]
    pub fn run(
        kind: Kind,
        path: PathBuf,
        label: String,
        ctx: egui::Context,
        work: impl FnOnce() -> Result<Done, String> + Send + 'static,
    ) -> Self {
        let (tx, rx) = mpsc::channel();
        let name = match kind {
            Kind::Open(_) => "aftermission-open",
            Kind::Export => "aftermission-export",
        };
        spawn(name, move || {
            // a closed receiver only means the app moved on
            let _ = tx.send(work());
            ctx.request_repaint();
        });
        Self {
            kind,
            path,
            label,
            rx,
        }
    }

    /// The outcome, once the work is done.
    pub fn poll(&mut self) -> Option<Result<Done, String>> {
        match self.rx.try_recv() {
            Ok(result) => Some(result),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => Some(Err(format!(
                "{}: the worker ended without a result",
                self.path.display()
            ))),
        }
    }
}

/// Open and model the log at `path`; an error names the path.
#[cfg(not(target_arch = "wasm32"))]
fn open_file(path: PathBuf) -> Result<Opened, String> {
    let log = Log::open(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let log = Box::new(LoadedLog::build(log, file_name(&path)));
    Ok(Opened {
        path: Some(path),
        log,
    })
}

/// Generate one of the demo's flights and model it.
fn demo_flight(flight: Flight) -> Opened {
    let log = Log::from_source(demo::bytes(flight).into());
    Opened {
        path: None,
        log: Box::new(LoadedLog::build(log, flight.name().to_string())),
    }
}

/// What the status line says while a log is indexed: `Indexing flight.bin
/// (12.3 MB)`, the size left out when it is not known.
#[must_use]
pub fn indexing_label(name: &str, bytes: Option<u64>) -> String {
    format!("Indexing {}", sized(name, bytes))
}

/// A file's name with its size when it is known: `flight.bin (12.3 MB)`.
fn sized(name: &str, bytes: Option<u64>) -> String {
    let size = bytes
        .map(|len| format!(" ({:.1} MB)", len as f64 / 1e6))
        .unwrap_or_default();
    format!("{name}{size}")
}

/// A file name without its extension, which an export's file name starts
/// with: `flight` for `flight.bin`.
#[must_use]
pub fn file_stem(name: &str) -> String {
    Path::new(name)
        .file_stem()
        .map_or_else(|| name.to_string(), |s| s.to_string_lossy().into_owned())
}

/// The last component of a path, or the whole path when it has none.
#[must_use]
pub fn file_name(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    )
}

#[cfg(not(target_arch = "wasm32"))]
fn spawn(name: &str, body: impl FnOnce() + Send + 'static) {
    if let Err(err) = std::thread::Builder::new()
        .name(name.to_string())
        .spawn(body)
    {
        tracing::error!(error = %err, name, "cannot spawn worker thread");
    }
}

/// No threads in the browser: run the job now, holding the frame. The
/// app paints the job's label before it starts one that can take long.
#[cfg(target_arch = "wasm32")]
fn spawn(name: &str, body: impl FnOnce() + Send + 'static) {
    tracing::debug!(name, "running the job inline");
    body();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    /// Poll until the job reports, for tests.
    pub(crate) fn wait(job: &mut Job) -> Result<Done, String> {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(result) = job.poll() {
                return result;
            }
            if Instant::now() >= deadline {
                return Err("the job did not finish in time".into());
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn a_log_opens_in_the_background() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("flight.bin");
        std::fs::write(&path, crate::model::testlog::bytes()).unwrap();
        let mut job = Job::open(path.clone(), Slot::A, egui::Context::default());
        assert_eq!(job.kind, Kind::Open(Slot::A));
        let kb = std::fs::metadata(&path).unwrap().len() as f64 / 1e6;
        assert_eq!(job.label, format!("Indexing flight.bin ({kb:.1} MB)"));
        let Done::Opened {
            path: opened,
            log,
            b: None,
        } = wait(&mut job).unwrap()
        else {
            panic!("an open job opens one log");
        };
        assert_eq!(opened, Some(path.clone()));
        assert_eq!(log.name, "flight.bin");
        assert_eq!(log.type_named("ATT").unwrap().count, 4);

        // as B, the job says so while it runs
        let job = Job::open(path, Slot::B, egui::Context::default());
        assert_eq!(job.kind, Kind::Open(Slot::B));
        assert_eq!(job.label, format!("Indexing flight.bin ({kb:.1} MB) as B"));
    }

    #[test]
    fn a_slot_names_b_and_leaves_a_plain() {
        assert_eq!(Slot::A.suffix(), "");
        assert_eq!(Slot::B.suffix(), " as B");
        assert_eq!(Slot::A.error("a.bin: gone"), "a.bin: gone");
        assert_eq!(Slot::B.error("b.bin: gone"), "B: b.bin: gone");
    }

    #[test]
    fn the_demo_opens_without_a_path() {
        let mut job = Job::open_demo(egui::Context::default());
        assert_eq!(job.kind, Kind::Open(Slot::A));
        assert_eq!(job.label, "Generating the demo log");
        assert_eq!(job.path, PathBuf::from("demo.bin"));
        let Done::Opened { path, log, b: None } = wait(&mut job).unwrap() else {
            panic!("an open job opens one log");
        };
        assert_eq!(path, None);
        assert_eq!(log.name, "demo.bin");
        assert!(log.records() > 50_000, "{}", log.records());
    }

    #[test]
    fn the_demo_pair_opens_both_flights_in_one_job() {
        let mut job = Job::open_demo_pair(egui::Context::default());
        assert_eq!(job.kind, Kind::Open(Slot::A));
        assert_eq!(job.label, "Generating the demo pair");
        let Done::Opened {
            path: None,
            log,
            b: Some(Ok(b)),
        } = wait(&mut job).unwrap()
        else {
            panic!("the pair opens A and B");
        };
        assert_eq!(log.name, "demo.bin");
        assert_eq!(b.path, None);
        assert_eq!(b.log.name, "demo2.bin");
    }

    #[test]
    fn a_pair_of_files_opens_in_one_job_by_the_pair_rule() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = (
            dir.path().join("flight.bin"),
            dir.path().join("baseline.bin"),
        );
        for path in [&a, &b] {
            std::fs::write(path, crate::model::testlog::bytes()).unwrap();
        }
        let mut job = Job::open_pair(a.clone(), b.clone(), egui::Context::default());
        assert_eq!(job.kind, Kind::Open(Slot::A));
        let mb = std::fs::metadata(&a).unwrap().len() as f64 / 1e6;
        assert_eq!(
            job.label,
            format!("Indexing flight.bin ({mb:.1} MB), then baseline.bin ({mb:.1} MB) as B")
        );
        let Done::Opened {
            path,
            log,
            b: Some(Ok(opened)),
        } = wait(&mut job).unwrap()
        else {
            panic!("the pair opens A and B");
        };
        assert_eq!((path, log.name.as_str()), (Some(a.clone()), "flight.bin"));
        assert_eq!(opened.path, Some(b.clone()));

        // B missing: A opens, and B's error rides beside it
        let gone = dir.path().join("gone.bin");
        let mut job = Job::open_pair(a.clone(), gone.clone(), egui::Context::default());
        let Done::Opened {
            b: Some(Err(err)), ..
        } = wait(&mut job).unwrap()
        else {
            panic!("A opens while B fails");
        };
        assert!(err.starts_with(&gone.display().to_string()), "{err}");
        // A missing: the job fails, B unread
        let mut job = Job::open_pair(gone.clone(), b, egui::Context::default());
        let err = wait(&mut job).unwrap_err();
        assert!(err.starts_with(&gone.display().to_string()), "{err}");
    }

    #[test]
    fn a_missing_file_is_an_error_with_its_path() {
        let path = PathBuf::from("no-such-folder/no-such-log.bin");
        let mut job = Job::open(path.clone(), Slot::A, egui::Context::default());
        assert_eq!(job.label, "Indexing no-such-log.bin", "no size to show");
        let err = wait(&mut job).unwrap_err();
        assert!(err.starts_with(&path.display().to_string()), "{err}");
    }

    #[test]
    fn any_work_runs_as_a_job() {
        let mut job = Job::run(
            Kind::Export,
            "out.csv".into(),
            "Writing out.csv".into(),
            egui::Context::default(),
            || {
                Ok(Done::Exported {
                    path: "out.csv".into(),
                    summary: "Wrote out.csv: 3 rows".into(),
                })
            },
        );
        assert_eq!(job.kind, Kind::Export);
        let Done::Exported { path, summary } = wait(&mut job).unwrap() else {
            panic!("an export job exports");
        };
        assert_eq!(path, PathBuf::from("out.csv"));
        assert_eq!(summary, "Wrote out.csv: 3 rows");

        let mut failed = Job::run(
            Kind::Export,
            "out.csv".into(),
            "Writing out.csv".into(),
            egui::Context::default(),
            || Err("disk full".into()),
        );
        assert_eq!(wait(&mut failed).unwrap_err(), "disk full");
    }

    #[test]
    fn a_worker_that_dies_is_reported_by_its_path() {
        let path = PathBuf::from("logs/a/flight.bin");
        let mut job = Job::run(
            Kind::Open(Slot::A),
            path.clone(),
            "Indexing flight.bin (1.0 MB)".into(),
            egui::Context::default(),
            || panic!("a worker that dies before it sends"),
        );
        assert_eq!(
            wait(&mut job).unwrap_err(),
            format!("{}: the worker ended without a result", path.display())
        );
    }

    #[test]
    fn file_names_fall_back_to_the_path() {
        assert_eq!(file_name(Path::new("a/b/c.bin")), "c.bin");
        assert_eq!(file_name(Path::new("..")), "..");
        assert_eq!(file_stem("flight.bin"), "flight");
        assert_eq!(file_stem("flight.bin.gz"), "flight.bin");
        assert_eq!(file_stem(".."), "..");
    }
}
