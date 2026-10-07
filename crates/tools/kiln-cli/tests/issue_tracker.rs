//! The issue tracker under `docs/issues/` is part of the repository, and this is the backstop
//! for a deletion the hooks never saw.
//!
//! On 2026-10-02 a commit about something else deleted all 634 tracked filings -- they had been
//! cleared from disk, then `git add -A` recorded it -- and nothing noticed for hours. The hooks
//! (`scripts/check-issue-deletions.sh`) refuse that at commit and push time, but a hook is
//! per-checkout and skippable. The gate runs this test, so the tracker vanishing, or shrinking
//! below the count `docs/issues/.floor` records, fails it by name.
//!
//! Raise the floor when the tracker grows; lowering it is the same deliberate act as
//! `KILN_ALLOW_ISSUE_DELETION=1`, and the diff is the record.

use std::path::{Path, PathBuf};

fn issues_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../docs/issues")
        .canonicalize()
        .expect("docs/issues/ exists: the issue tracker is missing from this checkout")
}

fn markdown_files(directory: &Path, found: &mut usize) {
    for entry in std::fs::read_dir(directory).expect("docs/issues/ is readable") {
        let path = entry.expect("docs/issues/ entry is readable").path();
        if path.is_dir() {
            markdown_files(&path, found);
        } else if path.extension().is_some_and(|extension| extension == "md") {
            *found += 1;
        }
    }
}

#[test]
fn the_issue_tracker_has_not_lost_filings() {
    let root = issues_root();
    let floor: usize = std::fs::read_to_string(root.join(".floor"))
        .expect("docs/issues/.floor records the tracker's minimum size")
        .trim()
        .parse()
        .expect("docs/issues/.floor holds one number");
    let mut found = 0;
    markdown_files(&root, &mut found);
    assert!(
        found >= floor,
        "docs/issues/ holds {found} Markdown filings, below the floor of {floor} in \
         docs/issues/.floor. Filings were deleted: restore them (`git log --diff-filter=D -- \
         docs/issues` names the commit), or, if removing them was the decision, lower the floor \
         in the same commit."
    );
}
