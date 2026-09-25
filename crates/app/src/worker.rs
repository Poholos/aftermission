//! Opening a log on a worker thread, so the UI keeps painting while the
//! file is mapped, scanned and modeled.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, TryRecvError};

use dflog::Log;

use crate::model::LoadedLog;

/// A log being opened.
pub struct OpenJob {
    pub path: PathBuf,
    /// The file's size when the job started, for the progress line.
    pub bytes: Option<u64>,
    rx: Receiver<Result<LoadedLog, String>>,
}

impl std::fmt::Debug for OpenJob {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenJob")
            .field("path", &self.path)
            .field("bytes", &self.bytes)
            .finish_non_exhaustive()
    }
}

impl OpenJob {
    /// Open and model `path` in the background. `ctx` is asked to repaint
    /// when the result is in.
    #[must_use]
    pub fn start(path: PathBuf, ctx: egui::Context) -> Self {
        let (tx, rx) = mpsc::channel();
        let bytes = std::fs::metadata(&path).ok().map(|m| m.len());
        let job_path = path.clone();
        spawn("aftermission-open", move || {
            let result = Log::open(&job_path)
                .map_err(|e| format!("{}: {e}", job_path.display()))
                .map(|log| LoadedLog::build(log, file_name(&job_path)));
            // a closed receiver only means the app moved on
            let _ = tx.send(result);
            ctx.request_repaint();
        });
        Self { path, bytes, rx }
    }

    /// The outcome, once the log is modeled.
    pub fn poll(&mut self) -> Option<Result<LoadedLog, String>> {
        match self.rx.try_recv() {
            Ok(result) => Some(result),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => Some(Err(format!(
                "{}: the worker ended without a result",
                self.path.display()
            ))),
        }
    }

    /// The file's name, for the progress line.
    #[must_use]
    pub fn name(&self) -> String {
        file_name(&self.path)
    }
}

/// The last component of a path, or the whole path when it has none.
#[must_use]
pub fn file_name(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    )
}

fn spawn(name: &str, body: impl FnOnce() + Send + 'static) {
    if let Err(err) = std::thread::Builder::new()
        .name(name.to_string())
        .spawn(body)
    {
        tracing::error!(error = %err, name, "cannot spawn worker thread");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    /// Poll until the job reports, for tests.
    pub(crate) fn wait(job: &mut OpenJob) -> Result<LoadedLog, String> {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(result) = job.poll() {
                return result;
            }
            if Instant::now() >= deadline {
                return Err("the open job did not finish in time".into());
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn a_log_opens_in_the_background() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("flight.bin");
        std::fs::write(&path, crate::model::testlog::bytes()).unwrap();
        let mut job = OpenJob::start(path.clone(), egui::Context::default());
        assert_eq!(job.name(), "flight.bin");
        assert_eq!(job.bytes, Some(std::fs::metadata(&path).unwrap().len()));
        let log = wait(&mut job).unwrap();
        assert_eq!(log.name, "flight.bin");
        assert_eq!(log.type_named("ATT").unwrap().count, 4);
    }

    #[test]
    fn a_missing_file_is_an_error_with_its_path() {
        let path = PathBuf::from("no-such-folder/no-such-log.bin");
        let mut job = OpenJob::start(path.clone(), egui::Context::default());
        assert_eq!(job.bytes, None);
        let err = wait(&mut job).unwrap_err();
        assert!(err.starts_with(&path.display().to_string()), "{err}");
    }

    #[test]
    fn file_names_fall_back_to_the_path() {
        assert_eq!(file_name(Path::new("a/b/c.bin")), "c.bin");
        assert_eq!(file_name(Path::new("..")), "..");
    }
}
