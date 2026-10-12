//! Does this program need the Node adapter?
//!
//! `--node` switches on the Node host adapter (`fs`, `http`, `process`, ...) and the Node package
//! resolver. A program that says `import http from "node:http"` cannot run without it, and making
//! the person add a flag the program has already asked for is a papercut on the first thing they
//! try. So the commands that read a program (`run`, `build`, `check`, `watch`) look at the module
//! graph they are about to load: starting at the entry file and following relative imports through
//! project-local files, the adapter turns on as soon as one of them names a Node built-in --
//! `node:fs`, or a bare `fs` / `path` / `http`, either as an `import` or as `require(...)`.
//!
//! Packages under `node_modules` are not followed: a project that has any already turns the
//! adapter on (see `auto_node` in `main.rs`). The scan is deliberately generous -- one stray
//! built-in name in a dead branch turns the adapter on, and the adapter is a superset of the plain
//! mode -- and bounded, so a pathological import fan-out cannot make a `run` slow.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use kiln_module_resolver::NodeBuiltin;

/// The most files one scan reads before it gives up and answers `false`.
const MAX_FILES: usize = 512;

const EXTENSIONS: [&str; 9] = ["ts", "tsx", "js", "jsx", "mjs", "mts", "cjs", "cts", "json"];

/// Whether `specifier` names a Node built-in the plain resolver cannot provide.
pub(crate) fn is_node_builtin_specifier(specifier: &str) -> bool {
    if specifier.starts_with("kiln:") || specifier.starts_with("node:kiln:") {
        return false;
    }
    if specifier.starts_with("node:") {
        return true;
    }
    NodeBuiltin::parse(specifier).is_some()
}

/// Whether the project-local module graph reachable from `entry` imports a Node built-in.
pub(crate) fn graph_imports_node_builtin(entry: &Path) -> bool {
    let mut seen: HashSet<PathBuf> = HashSet::new();
    let mut pending = vec![entry.to_path_buf()];
    while let Some(file) = pending.pop() {
        if seen.len() >= MAX_FILES || !seen.insert(file.clone()) {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&file) else {
            continue;
        };
        let Some(name) = file.to_str() else {
            continue;
        };
        let mut specifiers = kiln_frontend_oxc::discover_module_specifiers(name, &text);
        // `require("fs")` is not an ECMAScript module request, so the module discovery above does
        // not see it; the CommonJS parse reports the literal ones.
        if text.contains("require") {
            specifiers.extend(kiln_frontend_oxc::parse_commonjs_with_requires(name, &text).1);
        }
        for specifier in specifiers {
            if is_node_builtin_specifier(&specifier) {
                return true;
            }
            if specifier.starts_with("./") || specifier.starts_with("../") {
                let base = file.parent().unwrap_or_else(|| Path::new("."));
                if let Some(target) = resolve_local(&base.join(&specifier)) {
                    pending.push(target);
                }
            }
        }
    }
    false
}

/// A relative specifier's file, probing the extensions and index files a bundler would.
fn resolve_local(base: &Path) -> Option<PathBuf> {
    if base.is_file() {
        return Some(base.to_path_buf());
    }
    let text = base.to_str()?;
    for extension in EXTENSIONS {
        let candidate = PathBuf::from(format!("{text}.{extension}"));
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    // `./util.js` written for a `util.ts` on disk.
    if let Some(stem) = base.file_stem().and_then(|stem| stem.to_str())
        && matches!(
            base.extension().and_then(|extension| extension.to_str()),
            Some("js" | "jsx" | "mjs" | "cjs")
        )
    {
        let parent = base.parent()?;
        for extension in ["ts", "tsx", "mts", "cts"] {
            let candidate = parent.join(format!("{stem}.{extension}"));
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    for extension in EXTENSIONS {
        let candidate = base.join(format!("index.{extension}"));
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(files: &[(&str, &str)]) -> tempdir::Dir {
        tempdir::Dir::new(files)
    }

    /// A throwaway directory, removed on drop.
    mod tempdir {
        use std::path::{Path, PathBuf};

        pub struct Dir(PathBuf);

        impl Dir {
            pub fn new(files: &[(&str, &str)]) -> Self {
                static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
                let path = std::env::temp_dir().join(format!(
                    "kiln-node-detect-{}-{}",
                    std::process::id(),
                    NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                ));
                for (name, text) in files {
                    let file = path.join(name);
                    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
                    std::fs::write(file, text).unwrap();
                }
                Self(path)
            }

            pub fn path(&self) -> &Path {
                &self.0
            }
        }

        impl Drop for Dir {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
    }

    #[test]
    fn prefixed_and_bare_builtins_are_recognised() {
        assert!(is_node_builtin_specifier("node:http"));
        assert!(is_node_builtin_specifier("fs"));
        assert!(is_node_builtin_specifier("path"));
        assert!(!is_node_builtin_specifier("left-pad"));
        assert!(!is_node_builtin_specifier("./fs"));
        assert!(!is_node_builtin_specifier("kiln:gui"));
    }

    #[test]
    fn entry_import_turns_the_adapter_on() {
        let dir = project(&[(
            "main.ts",
            "import http from \"node:http\";\nconsole.log(http);\n",
        )]);
        assert!(graph_imports_node_builtin(&dir.path().join("main.ts")));
    }

    #[test]
    fn plain_program_leaves_it_off() {
        let dir = project(&[("main.ts", "console.log(new Date().getFullYear());\n")]);
        assert!(!graph_imports_node_builtin(&dir.path().join("main.ts")));
    }

    #[test]
    fn require_and_bare_names_count() {
        let dir = project(&[("main.cjs", "const fs = require(\"fs\");\n")]);
        assert!(graph_imports_node_builtin(&dir.path().join("main.cjs")));
        let dir = project(&[("main.mjs", "import { join } from \"path\";\n")]);
        assert!(graph_imports_node_builtin(&dir.path().join("main.mjs")));
    }

    #[test]
    fn relative_imports_are_followed_including_cycles() {
        let dir = project(&[
            (
                "main.ts",
                "import { a } from \"./lib/a\";\nconsole.log(a);\n",
            ),
            (
                "lib/a.ts",
                "import { b } from \"./b.js\";\nexport const a = b;\n",
            ),
            (
                "lib/b.ts",
                "import \"./a\";\nimport os from \"node:os\";\nexport const b = os;\n",
            ),
        ]);
        assert!(graph_imports_node_builtin(&dir.path().join("main.ts")));
    }
}
