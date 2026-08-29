//! `kiln watch` - re-check a file every time it changes.
//!
//! v0 polls. That is a deliberate placeholder: a real watcher means FSEvents/inotify and a
//! dependency, and at a 200ms cadence the poll is invisible next to the edit-to-answer time
//! we actually care about. Swapping in a notify-based watcher is a later, self-contained
//! change - the loop below is the only thing that would move.
//!
//! Change detection compares file *contents*, not mtimes: editors rewrite files on save
//! whether or not anything changed, and a re-check that reports "recomputed nothing" for an
//! edit the user did not make is confusing rather than reassuring.

use std::io::Write;
use std::path::Path;
use std::time::Duration;

use crate::backend::Backend;
use crate::commands;
use crate::error::{Error, Result};

pub fn run(
    backend: &mut dyn Backend,
    path: &Path,
    interval: Duration,
    out: &mut dyn Write,
    notes: &mut dyn Write,
) -> Result<()> {
    let display = path.display().to_string();
    let canonical = commands::canonical_path(path)?;
    let mut text = commands::read_source(path)?;

    writeln!(
        notes,
        "watching {display} (polling every {}ms, ctrl-c to stop)",
        interval.as_millis()
    )
    .map_err(|source| Error::io("cannot write to stderr", source))?;

    check_once(backend, &display, &canonical, text.clone(), out)?;

    let mut missing = false;
    loop {
        std::thread::sleep(interval);
        match commands::read_source(path) {
            Ok(current) => {
                if missing {
                    writeln!(notes, "{display} is back")
                        .map_err(|source| Error::io("cannot write to stderr", source))?;
                    missing = false;
                }
                if current != text {
                    text = current;
                    check_once(backend, &display, &canonical, text.clone(), out)?;
                }
            }
            // Editors save by writing a temporary file and renaming over the target, so the
            // file can be briefly absent. Say so once, then keep waiting for it to return.
            Err(err) if is_missing(&err) => {
                if !missing {
                    writeln!(notes, "{display} disappeared; still watching")
                        .map_err(|source| Error::io("cannot write to stderr", source))?;
                    missing = true;
                }
            }
            Err(err) => return Err(err),
        }
    }
}

fn check_once(
    backend: &mut dyn Backend,
    display: &str,
    canonical: &str,
    text: String,
    out: &mut dyn Write,
) -> Result<()> {
    let output = commands::check_source(backend, display, canonical.to_string(), text)?;
    out.write_all(output.text.as_bytes())
        .and_then(|()| out.flush())
        .map_err(|source| Error::io("cannot write to stdout", source))
}

fn is_missing(error: &Error) -> bool {
    matches!(error, Error::MissingFile(_))
}
