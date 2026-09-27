//! Work off the UI thread, so the window keeps painting: opening a log,
//! which maps, scans and models the file, and writing an export. One job
//! runs at a time. The browser has no thread to give, so there a job
//! runs to its end inside `run` and its result waits at the first poll.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, TryRecvError};

use dflog::Log;

use crate::model::LoadedLog;

/// What a job is doing, for where its outcome goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Open,
    Export,
}

/// What a finished job hands back.
#[derive(Debug)]
pub enum Done {
    /// The log at `path`, modeled; boxed, so the enum stays the size of
    /// its other variant.
    Opened { path: PathBuf, log: Box<LoadedLog> },
    /// A file written at `path`; `summary` is what the status line says.
    #[cfg_attr(
        target_arch = "wasm32",
        expect(dead_code, reason = "the browser build writes no export yet")
    )]
    Exported { path: PathBuf, summary: String },
}

/// Work running on a worker thread.
pub struct Job {
    pub kind: Kind,
    /// The file the job reads or writes, which names it in an error.
    pub path: PathBuf,
    /// What the status line says while it runs: `Indexing flight.bin
    /// (12.3 MB)`, `Writing flight.csv`.
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
    /// Open and model the log at `path` in the background.
    #[cfg(not(target_arch = "wasm32"))]
    #[must_use]
    pub fn open(path: PathBuf, ctx: egui::Context) -> Self {
        let label = indexing_label(
            &file_name(&path),
            std::fs::metadata(&path).ok().map(|m| m.len()),
        );
        Self::run(Kind::Open, path.clone(), label, ctx, move || {
            let log = Log::open(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            let log = Box::new(LoadedLog::build(log, file_name(&path)));
            Ok(Done::Opened { path, log })
        })
    }

    /// Model a log the browser read into memory as `name`. Natively only
    /// tests take this way in; the path the result carries is the name.
    #[cfg(any(target_arch = "wasm32", test))]
    #[must_use]
    pub fn open_bytes(name: String, bytes: Vec<u8>, ctx: egui::Context) -> Self {
        let label = indexing_label(&name, Some(bytes.len() as u64));
        let path = PathBuf::from(&name);
        Self::run(Kind::Open, path.clone(), label, ctx, move || {
            let log = Box::new(LoadedLog::build(Log::from_source(bytes.into()), name));
            Ok(Done::Opened { path, log })
        })
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
            Kind::Open => "aftermission-open",
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

/// What the status line says while a log is indexed: `Indexing flight.bin
/// (12.3 MB)`, the size left out when it is not known.
#[must_use]
pub fn indexing_label(name: &str, bytes: Option<u64>) -> String {
    let size = bytes
        .map(|len| format!(" ({:.1} MB)", len as f64 / 1e6))
        .unwrap_or_default();
    format!("Indexing {name}{size}")
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
        let mut job = Job::open(path.clone(), egui::Context::default());
        assert_eq!(job.kind, Kind::Open);
        let kb = std::fs::metadata(&path).unwrap().len() as f64 / 1e6;
        assert_eq!(job.label, format!("Indexing flight.bin ({kb:.1} MB)"));
        let Done::Opened { path: opened, log } = wait(&mut job).unwrap() else {
            panic!("an open job opens");
        };
        assert_eq!(opened, path);
        assert_eq!(log.name, "flight.bin");
        assert_eq!(log.type_named("ATT").unwrap().count, 4);
    }

    #[test]
    fn a_missing_file_is_an_error_with_its_path() {
        let path = PathBuf::from("no-such-folder/no-such-log.bin");
        let mut job = Job::open(path.clone(), egui::Context::default());
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
            Kind::Open,
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
