//! Checks each project directory given on the command line as `tsc --pretty false` would, and
//! writes the lines `tsc` would print to `<out>/<directory name>`: the half of a differential test
//! against TypeScript whose other half is `tsc` itself.
//!
//! `cargo run --release --example tsc_differential -- <out> <project>...`

use rayon::prelude::*;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use kiln_type_checker::{CheckOptions, Libraries, SourceSnapshot, check, project_roots};

const SOURCE_EXTENSIONS: &[&str] = &["ts", "tsx", "mts", "cts", "js", "jsx", "mjs", "cjs"];

fn parse(path: &str, source: &str) -> kiln_hir::Module {
    let lower = path.to_ascii_lowercase();
    if lower.ends_with(".d.ts") || lower.ends_with(".d.mts") || lower.ends_with(".d.cts") {
        kiln_frontend_oxc::parse_authored(path, source)
    } else {
        kiln_frontend_oxc::parse(path, source)
    }
}

fn walk(dir: &Path, files: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk(&path, files);
        } else {
            files.push(path);
        }
    }
}

/// TypeScript's line and column, one-based, the column in UTF-16 units.
fn position(text: &str, offset: u32) -> (usize, usize) {
    let offset = (offset as usize).min(text.len());
    let before = &text[..text.floor_char_boundary(offset)];
    let line_start = before.rfind('\n').map_or(0, |at| at + 1);
    let line = before.matches('\n').count() + 1;
    let column = before[line_start..].encode_utf16().count() + 1;
    (line, column)
}

fn check_project(root: &Path, libraries: &Arc<Libraries>) -> Vec<String> {
    let mut files = Vec::new();
    walk(root, &mut files);
    let mut snapshot = SourceSnapshot {
        libraries: Some(libraries.clone()),
        ..Default::default()
    };
    let mut texts = std::collections::HashMap::new();
    for file in files {
        let Ok(bytes) = std::fs::read(&file) else {
            continue;
        };
        let path = file.to_string_lossy().into_owned();
        let extension = file
            .extension()
            .map(|e| e.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();
        match std::str::from_utf8(&bytes) {
            Ok(text) if SOURCE_EXTENSIONS.contains(&extension.as_str()) => {
                snapshot
                    .modules
                    .insert(path.clone(), Arc::new(parse(&path, text)));
                texts.insert(path, text.to_owned());
            }
            _ => {
                if let Ok(text) = std::str::from_utf8(&bytes) {
                    texts.insert(path.clone(), text.to_owned());
                }
                snapshot.auxiliary_files.insert(path, bytes.into());
            }
        }
    }
    let config = root.join("tsconfig.json").to_string_lossy().into_owned();
    let has_config = snapshot.auxiliary_files.contains_key(&config);
    let roots = if has_config {
        project_roots(&snapshot, &config)
    } else {
        Vec::new()
    };
    let options = CheckOptions {
        config_path: has_config.then_some(config),
        current_directory: root.to_string_lossy().into_owned(),
        ..Default::default()
    };
    let result = check(&snapshot, &roots, &options);
    let prefix = format!("{}/", root.to_string_lossy());
    let mut lines = Vec::new();
    for diagnostic in &result.diagnostics {
        let code = diagnostic
            .code
            .map_or_else(|| "kiln".to_owned(), |code| format!("TS{code}"));
        let mut message = diagnostic.message.lines();
        let first = message.next().unwrap_or_default();
        let at = match (&diagnostic.path, &diagnostic.span) {
            (Some(path), Some(span)) => {
                let (line, column) = texts
                    .get(path)
                    .map_or((0, 0), |text| position(text, span.start));
                let path = path.strip_prefix(&prefix).unwrap_or(path);
                format!("{path}({line},{column}): ")
            }
            (Some(path), None) => format!("{}: ", path.strip_prefix(&prefix).unwrap_or(path)),
            _ => String::new(),
        };
        lines.push(format!("{at}error {code}: {first}"));
        lines.extend(
            message
                .filter(|line| !line.trim().is_empty())
                .map(str::to_owned),
        );
    }
    if !result.complete {
        lines.push("kiln: incomplete".to_owned());
    }
    lines
}

fn main() {
    let mut args = std::env::args().skip(1);
    let out = PathBuf::from(
        args.next()
            .expect("usage: tsc_differential <out> <project>..."),
    );
    std::fs::create_dir_all(&out).unwrap();
    let projects: Vec<PathBuf> = args.map(PathBuf::from).collect();
    let libraries = Arc::new(Libraries::standard(kiln_frontend_oxc::parse_authored));
    // One project per worker, each checked as `kiln check` checks it; a deep case needs a deep
    // stack.
    let pool = kiln_jobs::pool(
        "kiln-tsc-differential",
        kiln_jobs::budget(usize::MAX),
        Some(1 << 30),
    )
    .expect("differential thread pool");
    pool.install(|| {
        projects.par_iter().for_each(|project| {
            let name = project.file_name().unwrap();
            let root = std::fs::canonicalize(project).unwrap();
            let lines = std::panic::catch_unwind(|| check_project(&root, &libraries))
                .unwrap_or_else(|_| vec!["kiln: panic".to_owned()]);
            let mut text = lines.join("\n");
            if !text.is_empty() {
                text.push('\n');
            }
            std::fs::write(out.join(name), text).unwrap();
        });
    });
}
