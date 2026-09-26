//! Export of the whole log as Parquet, one file per message type, into a
//! folder of its own: `flight_parquet` for `flight.bin`, made inside the
//! folder the user chose. A new folder each time, so an export never
//! overwrites or mixes with an earlier one, and one that fails is removed
//! whole, since the exporter leaves its files unfinished.

use std::io;
use std::path::{Path, PathBuf};

use crate::model::LoadedLog;
use crate::worker::{self, Done};

/// The folder an export of `log_name` goes into: `flight_parquet` for
/// `flight.bin`.
#[must_use]
pub fn folder_name(log_name: &str) -> String {
    format!("{}_parquet", worker::file_stem(log_name))
}

/// Make a new folder named `name` in `parent`, or, when that name is
/// taken, the first free of `name (2)`, `name (3)` and so on. Creating
/// fails on a name that exists, so the check and the creation are one
/// step and nothing that was there is touched.
///
/// # Errors
///
/// Any error making the folder other than its name being taken, such as
/// a missing parent or one the user cannot write to, with the path of the
/// folder it was making.
pub fn new_folder(parent: &Path, name: &str) -> Result<PathBuf, String> {
    let mut n = 1u32;
    loop {
        let candidate = if n == 1 {
            parent.join(name)
        } else {
            parent.join(format!("{name} ({n})"))
        };
        match std::fs::create_dir(&candidate) {
            Ok(()) => return Ok(candidate),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists && n < u32::MAX => n += 1,
            Err(e) => return Err(format!("{}: {e}", candidate.display())),
        }
    }
}

/// A folder removed with everything in it when this is dropped, unless
/// [`Self::keep`] was called: a failed export is cleaned up whether it
/// returned an error or its thread panicked.
struct RemoveOnDrop(Option<PathBuf>);

impl RemoveOnDrop {
    fn keep(mut self) {
        self.0 = None;
    }
}

impl Drop for RemoveOnDrop {
    fn drop(&mut self) {
        if let Some(dir) = &self.0
            && let Err(e) = std::fs::remove_dir_all(dir)
        {
            tracing::warn!(error = %e, dir = %dir.display(), "cannot remove the failed export");
        }
    }
}

/// Make a new folder `name` in `parent` and fill it through `write`. When
/// `write` fails, by an error or a panic, the folder goes again with
/// whatever `write` left in it.
///
/// # Errors
///
/// Making the folder, with its path, or the error `write` returned.
pub fn into_new_folder<T>(
    parent: &Path,
    name: &str,
    write: impl FnOnce(&Path) -> Result<T, String>,
) -> Result<(PathBuf, T), String> {
    let dir = new_folder(parent, name)?;
    let guard = RemoveOnDrop(Some(dir.clone()));
    let value = write(&dir)?;
    guard.keep();
    Ok((dir, value))
}

/// Export `log` into a new folder in `parent`: the body of the export job.
/// The summary names the folder, the files and the rows written.
///
/// # Errors
///
/// Making the folder, or the exporter's error; the folder is gone then.
pub fn export(log: &LoadedLog, parent: &Path, split_instances: bool) -> Result<Done, String> {
    let (dir, summary) = into_new_folder(parent, &folder_name(&log.name), |dir| {
        log.export_parquet(dir, split_instances)
            .map_err(|e| e.to_string())
    })?;
    let rows: u64 = summary.files.iter().map(|f| f.rows).sum();
    let summary = format!(
        "Wrote {}: {} files, {rows} rows",
        worker::file_name(&dir),
        summary.files.len()
    );
    Ok(Done::Exported { path: dir, summary })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folders_are_named_after_the_log() {
        assert_eq!(folder_name("flight.bin"), "flight_parquet");
        assert_eq!(folder_name("00000012.BIN"), "00000012_parquet");
    }

    #[test]
    fn a_taken_name_gets_the_next_number_and_keeps_its_contents() {
        let parent = tempfile::tempdir().unwrap();
        let first = new_folder(parent.path(), "flight_parquet").unwrap();
        assert_eq!(first, parent.path().join("flight_parquet"));
        std::fs::write(first.join("ATT.parquet"), "earlier").unwrap();

        let second = new_folder(parent.path(), "flight_parquet").unwrap();
        assert_eq!(second, parent.path().join("flight_parquet (2)"));
        let third = new_folder(parent.path(), "flight_parquet").unwrap();
        assert_eq!(third, parent.path().join("flight_parquet (3)"));
        assert_eq!(
            std::fs::read_to_string(first.join("ATT.parquet")).unwrap(),
            "earlier"
        );
        assert_eq!(std::fs::read_dir(&second).unwrap().count(), 0);
    }

    #[cfg(unix)]
    #[test]
    fn an_error_names_the_folder_it_was_making() {
        use std::os::unix::fs::PermissionsExt;

        // the first name taken by a folder of its own, then the parent made
        // read-only: the second name is the one that fails
        let parent = tempfile::tempdir().unwrap();
        std::fs::create_dir(parent.path().join("flight_parquet")).unwrap();
        std::fs::set_permissions(parent.path(), std::fs::Permissions::from_mode(0o555)).unwrap();
        let err = new_folder(parent.path(), "flight_parquet");
        std::fs::set_permissions(parent.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
        // root may write anyway, and then there is no error to check
        if let Err(err) = err {
            let tried = parent.path().join("flight_parquet (2)");
            assert!(err.starts_with(&tried.display().to_string()), "{err}");
        }
    }

    #[test]
    fn a_file_in_the_way_counts_as_taken() {
        let parent = tempfile::tempdir().unwrap();
        std::fs::write(parent.path().join("flight_parquet"), "a file").unwrap();
        let dir = new_folder(parent.path(), "flight_parquet").unwrap();
        assert_eq!(dir, parent.path().join("flight_parquet (2)"));
        assert!(dir.is_dir());
    }

    #[test]
    fn a_missing_parent_is_an_error_with_the_path() {
        let parent = tempfile::tempdir().unwrap();
        let missing = parent.path().join("no-such-folder");
        let err = into_new_folder(&missing, "flight_parquet", |_| Ok(())).unwrap_err();
        assert!(
            err.starts_with(&missing.join("flight_parquet").display().to_string()),
            "{err}"
        );
        assert!(!missing.exists());
    }

    #[test]
    fn a_failed_write_takes_its_folder_with_it() {
        let parent = tempfile::tempdir().unwrap();
        let err = into_new_folder(parent.path(), "flight_parquet", |dir| {
            std::fs::write(dir.join("ATT.parquet"), "unfinished").unwrap();
            Err::<(), _>("disk full".to_string())
        })
        .unwrap_err();
        assert_eq!(err, "disk full");
        assert_eq!(std::fs::read_dir(parent.path()).unwrap().count(), 0);

        let (dir, value) = into_new_folder(parent.path(), "flight_parquet", |dir| {
            std::fs::write(dir.join("ATT.parquet"), "done").unwrap();
            Ok(7)
        })
        .unwrap();
        assert_eq!(value, 7);
        assert_eq!(dir, parent.path().join("flight_parquet"));
        assert!(dir.join("ATT.parquet").is_file());
    }

    #[test]
    fn a_write_that_panics_takes_its_folder_with_it() {
        let parent = tempfile::tempdir().unwrap();
        let unwound = std::panic::catch_unwind(|| {
            into_new_folder(
                parent.path(),
                "flight_parquet",
                |dir| -> Result<(), String> {
                    std::fs::write(dir.join("ATT.parquet"), "unfinished").unwrap();
                    panic!("a writer that dies halfway")
                },
            )
        });
        unwound.unwrap_err();
        assert_eq!(std::fs::read_dir(parent.path()).unwrap().count(), 0);
    }
}
