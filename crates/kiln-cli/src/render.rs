//! Turning responses into text.
//!
//! These are pure `-> String` functions so the output format is testable without a daemon,
//! and so the command layer stays about sequencing requests rather than formatting.
//!
//! The bias throughout: report the *work*, not just the answer. A developer's only evidence
//! that incrementality is doing its job is a report that says what was recomputed and what
//! was reused, so that report is part of the normal output rather than a verbose mode.

use std::fmt::Write as _;
use std::time::Duration;

use kiln_proto::{
    ModuleSummary, NativeBuildSummary, NativeSharedLibrarySummary, ProjectSummary, Stats,
};

/// Width of the indented label column, sized to the longest label ("implementation").
const LABEL: usize = 16;
/// Beyond this, an export list is noise; the count above it is the useful part.
const MAX_EXPORTS_SHOWN: usize = 6;

/// The difference between two [`Stats`] snapshots: what one operation cost.
///
/// Counters are cumulative and daemon-wide, so a single check is only legible as a delta.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Work {
    pub parse: u64,
    pub ir: u64,
    pub interface: u64,
    pub project: u64,
    pub validated: u64,
    pub early_cutoff: u64,
}

impl Work {
    pub fn between(before: &Stats, after: &Stats) -> Work {
        Work {
            parse: after.parse_computed.saturating_sub(before.parse_computed),
            ir: after.ir_computed.saturating_sub(before.ir_computed),
            interface: after
                .interface_computed
                .saturating_sub(before.interface_computed),
            project: after
                .project_computed
                .saturating_sub(before.project_computed),
            validated: after.validated.saturating_sub(before.validated),
            early_cutoff: after.early_cutoff.saturating_sub(before.early_cutoff),
        }
    }

    fn recomputed(&self) -> String {
        let mut parts = Vec::new();
        for (name, count) in [
            ("parse", self.parse),
            ("ir", self.ir),
            ("interface", self.interface),
            ("project", self.project),
        ] {
            if count > 0 {
                parts.push(format!("{name} {count}"));
            }
        }
        if parts.is_empty() {
            // The headline result of the whole design: an edit that changed nothing
            // observable costs nothing.
            "nothing".to_string()
        } else {
            parts.join(", ")
        }
    }

    fn reused(&self) -> Option<String> {
        let mut parts = Vec::new();
        if self.validated > 0 {
            parts.push(format!("{} validated", self.validated));
        }
        if self.early_cutoff > 0 {
            parts.push(format!("{} cut off early", self.early_cutoff));
        }
        (!parts.is_empty()).then(|| parts.join(", "))
    }
}

/// `kiln check` output: the module's identity, then what it cost to learn it.
pub fn check(
    display_path: &str,
    summary: &ModuleSummary,
    work: &Work,
    revision: u64,
    elapsed: Duration,
) -> String {
    let mut out = format!("{display_path}  r{revision}  {}\n", duration(elapsed));
    row(&mut out, "interface", &hash(summary.interface_hash));
    row(
        &mut out,
        "implementation",
        &hash(summary.implementation_hash),
    );
    if let Some(resolved) = &summary.resolved_exports {
        if resolved != &summary.exports {
            row(&mut out, "declared", &exports(&summary.exports));
        }
        row(&mut out, "exports", &exports(resolved));
    } else {
        row(&mut out, "exports", &exports(&summary.exports));
    }
    if !summary.diagnostics.is_empty() {
        row(
            &mut out,
            "diagnostics",
            &summary.diagnostics.len().to_string(),
        );
    }
    work_rows(&mut out, work);
    out
}

/// `kiln project` output. The project rollup depends on every module interface, so the
/// work report is the evidence that an unrelated edit did not disturb it.
pub fn project(summary: &ProjectSummary, work: &Work, revision: u64, elapsed: Duration) -> String {
    let mut out = format!("project  r{revision}  {}\n", duration(elapsed));
    row(&mut out, "modules", &summary.modules.to_string());
    row(&mut out, "exports", &summary.exports.to_string());
    row(&mut out, "interface", &hash(summary.interface_hash));
    work_rows(&mut out, work);
    out
}

/// `kiln stats` output: the raw cumulative counters, unaggregated.
pub fn stats(stats: &Stats) -> String {
    let mut out = String::new();
    let width = "interface computed".len();
    for (label, value) in [
        ("revision", stats.revision),
        ("parse computed", stats.parse_computed),
        ("ir computed", stats.ir_computed),
        ("interface computed", stats.interface_computed),
        ("project computed", stats.project_computed),
        ("validated", stats.validated),
        ("early cutoff", stats.early_cutoff),
    ] {
        let _ = writeln!(out, "{label:<width$}  {value}");
    }
    out
}

/// `kiln build` output. Cache decisions are deliberately visible so a build can be diagnosed
/// without turning on tracing or inspecting the cache directory.
pub fn native_build(source: &str, summary: &NativeBuildSummary) -> String {
    let hit = |value: bool| if value { "hit" } else { "built" };
    let mut out = format!("native build {source}  {}ms\n", summary.total_millis);
    row(&mut out, "module", &summary.module_symbol);
    row(&mut out, "output", &summary.output_path);
    row(&mut out, "compiler", &summary.compiler_identity);
    row(
        &mut out,
        "generated C",
        &format!(
            "{} bytes ({})",
            summary.generated_c_bytes,
            hit(summary.generated_c_cache_hit)
        ),
    );
    row(
        &mut out,
        "app object",
        &format!(
            "{} bytes ({})",
            summary.app_object_bytes,
            hit(summary.app_object_cache_hit)
        ),
    );
    row(
        &mut out,
        "runtime object",
        &format!(
            "{} bytes ({})",
            summary.runtime_object_bytes,
            hit(summary.runtime_object_cache_hit)
        ),
    );
    row(
        &mut out,
        "launcher object",
        &format!(
            "{} bytes ({})",
            summary.launcher_object_bytes,
            hit(summary.launcher_object_cache_hit)
        ),
    );
    row(
        &mut out,
        "executable",
        &format!(
            "{} bytes ({})",
            summary.executable_bytes,
            hit(summary.executable_cache_hit)
        ),
    );
    row(
        &mut out,
        "timing",
        &format!(
            "emit {}ms, compile {}ms, link {}ms",
            summary.emit_millis, summary.compile_millis, summary.link_millis
        ),
    );
    out
}

/// `kiln build --shared` output. Shared-library reports intentionally contain only the
/// generated source, PIC object, and library layers involved in an embedding build.
pub fn native_shared_build(source: &str, summary: &NativeSharedLibrarySummary) -> String {
    let hit = |value: bool| if value { "hit" } else { "built" };
    let mut out = format!("native shared build {source}  {}ms\n", summary.total_millis);
    row(&mut out, "entry", &summary.module_symbol);
    row(&mut out, "API symbol", &summary.module_api_symbol);
    row(&mut out, "output", &summary.output_path);
    row(&mut out, "loader", &summary.loader_path);
    row(&mut out, "header", &summary.header_path);
    row(&mut out, "compiler", &summary.compiler_identity);
    row(
        &mut out,
        "generated C",
        &format!(
            "{} bytes ({})",
            summary.generated_c_bytes,
            hit(summary.generated_c_cache_hit)
        ),
    );
    row(
        &mut out,
        "PIC object",
        &format!(
            "{} bytes ({})",
            summary.object_bytes,
            hit(summary.object_cache_hit)
        ),
    );
    row(
        &mut out,
        "shared library",
        &format!(
            "{} bytes ({})",
            summary.library_bytes,
            hit(summary.library_cache_hit)
        ),
    );
    row(
        &mut out,
        "timing",
        &format!(
            "emit {}ms, compile {}ms, link {}ms",
            summary.emit_millis, summary.compile_millis, summary.link_millis
        ),
    );
    out
}

fn work_rows(out: &mut String, work: &Work) {
    row(out, "recomputed", &work.recomputed());
    if let Some(reused) = work.reused() {
        row(out, "reused", &reused);
    }
}

fn row(out: &mut String, label: &str, value: &str) {
    let _ = writeln!(out, "  {label:<LABEL$}{value}");
}

fn hash(value: u64) -> String {
    format!("{value:#018x}")
}

fn exports(names: &[String]) -> String {
    if names.is_empty() {
        return "0".to_string();
    }
    let shown = names
        .iter()
        .take(MAX_EXPORTS_SHOWN)
        .cloned()
        .collect::<Vec<_>>()
        .join(", ");
    match names.len().checked_sub(MAX_EXPORTS_SHOWN) {
        Some(hidden) if hidden > 0 => format!("{}  {shown}, +{hidden} more", names.len()),
        _ => format!("{}  {shown}", names.len()),
    }
}

/// Latency is the thesis, so it is always on screen. Two significant decimals is enough to
/// tell a warm incremental check from a cold one without pretending to nanosecond accuracy.
pub fn duration(elapsed: Duration) -> String {
    let ms = elapsed.as_secs_f64() * 1000.0;
    if ms >= 1000.0 {
        format!("{:.2}s", ms / 1000.0)
    } else {
        format!("{ms:.2}ms")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary() -> ModuleSummary {
        ModuleSummary {
            path: "/src/a.js".to_string(),
            file: kiln_proto::FileId(1),
            implementation_hash: 0x1122_3344_5566_7788,
            interface_hash: 0x9f3c_1d2e_4a5b_6c7d,
            exports: vec!["greet".to_string(), "main".to_string()],
            resolved_exports: None,
            diagnostics: Vec::new(),
        }
    }

    fn stats_at(
        revision: u64,
        parse: u64,
        interface: u64,
        project: u64,
        validated: u64,
        cutoff: u64,
    ) -> Stats {
        Stats {
            revision,
            parse_computed: parse,
            ir_computed: 0,
            interface_computed: interface,
            project_computed: project,
            validated,
            early_cutoff: cutoff,
            ..Stats::default()
        }
    }

    #[test]
    fn work_is_the_delta_between_snapshots() {
        let before = stats_at(1, 5, 4, 1, 10, 2);
        let after = stats_at(2, 6, 5, 1, 12, 3);
        assert_eq!(
            Work::between(&before, &after),
            Work {
                parse: 1,
                ir: 0,
                interface: 1,
                project: 0,
                validated: 2,
                early_cutoff: 1
            }
        );
    }

    #[test]
    fn a_body_only_edit_reads_as_early_cutoff() {
        let work = Work {
            parse: 1,
            ir: 0,
            interface: 1,
            project: 0,
            validated: 2,
            early_cutoff: 1,
        };
        let out = check("a.js", &summary(), &work, 2, Duration::from_micros(420));
        assert!(out.starts_with("a.js  r2  0.42ms\n"), "{out}");
        assert!(
            out.contains("recomputed      parse 1, interface 1\n"),
            "{out}"
        );
        assert!(
            out.contains("reused          2 validated, 1 cut off early\n"),
            "{out}"
        );
        // The project rollup was untouched, so it must not appear as recomputed work.
        assert!(!out.contains("project 1"), "{out}");
    }

    #[test]
    fn a_no_op_check_says_so_loudly() {
        let work = Work {
            parse: 0,
            ir: 0,
            interface: 0,
            project: 0,
            validated: 3,
            early_cutoff: 0,
        };
        let out = check("a.js", &summary(), &work, 2, Duration::ZERO);
        assert!(out.contains("recomputed      nothing\n"), "{out}");
    }

    #[test]
    fn check_distinguishes_a_declared_surface_from_star_resolved_exports() {
        let mut summary = summary();
        summary.exports.clear();
        summary.resolved_exports = Some(vec!["answer".to_owned()]);
        let out = check(
            "barrel.js",
            &summary,
            &Work {
                parse: 0,
                ir: 0,
                interface: 0,
                project: 0,
                validated: 0,
                early_cutoff: 0,
            },
            1,
            Duration::ZERO,
        );
        assert!(out.contains("declared        0\n"), "{out}");
        assert!(out.contains("exports         1  answer\n"), "{out}");
    }

    #[test]
    fn reused_row_is_omitted_when_there_was_no_reuse() {
        let work = Work {
            parse: 1,
            ir: 0,
            interface: 1,
            project: 1,
            validated: 0,
            early_cutoff: 0,
        };
        let out = check("a.js", &summary(), &work, 1, Duration::ZERO);
        assert!(!out.contains("reused"), "{out}");
        assert!(
            out.contains("recomputed      parse 1, interface 1, project 1\n"),
            "{out}"
        );
    }

    #[test]
    fn hashes_are_full_width_so_they_line_up_between_runs() {
        let mut small = summary();
        small.interface_hash = 0xff;
        let out = check(
            "a.js",
            &small,
            &Work {
                parse: 0,
                ir: 0,
                interface: 0,
                project: 0,
                validated: 0,
                early_cutoff: 0,
            },
            1,
            Duration::ZERO,
        );
        assert!(
            out.contains("interface       0x00000000000000ff\n"),
            "{out}"
        );
    }

    #[test]
    fn long_export_lists_are_summarised() {
        let names: Vec<String> = (0..9).map(|i| format!("e{i}")).collect();
        assert_eq!(exports(&names), "9  e0, e1, e2, e3, e4, e5, +3 more");
        assert_eq!(exports(&names[..2]), "2  e0, e1");
        assert_eq!(exports(&[]), "0");
    }

    #[test]
    fn durations_switch_units_at_a_second() {
        assert_eq!(duration(Duration::from_micros(1)), "0.00ms");
        assert_eq!(duration(Duration::from_millis(12)), "12.00ms");
        assert_eq!(duration(Duration::from_millis(1500)), "1.50s");
    }

    #[test]
    fn stats_renders_every_counter() {
        let out = stats(&stats_at(7, 3, 2, 1, 9, 2));
        assert!(out.contains("revision            7\n"), "{out}");
        assert!(out.contains("interface computed  2\n"), "{out}");
        assert!(out.contains("early cutoff        2\n"), "{out}");
    }

    #[test]
    fn shared_build_renders_only_embedding_artifacts() {
        let out = native_shared_build(
            "src/main.ts",
            &NativeSharedLibrarySummary {
                module_symbol: "kiln_module_main".to_owned(),
                module_api_symbol: "kiln_module_main_api_v2".to_owned(),
                output_path: ".kiln/lib/main.so".to_owned(),
                loader_path: ".kiln/lib/.kiln-loaders/kiln-content-id.so".to_owned(),
                header_path: ".kiln/lib/main.h".to_owned(),
                compiler_identity: "cc".to_owned(),
                generated_c_cache_hit: true,
                object_cache_hit: false,
                library_cache_hit: true,
                generated_c_bytes: 10,
                object_bytes: 20,
                library_bytes: 30,
                emit_millis: 1,
                compile_millis: 2,
                link_millis: 3,
                total_millis: 6,
            },
        );
        assert!(out.contains("native shared build src/main.ts"), "{out}");
        assert!(out.contains("entry           kiln_module_main"), "{out}");
        assert!(
            out.contains("API symbol      kiln_module_main_api_v2"),
            "{out}"
        );
        assert!(
            out.contains(".kiln/lib/.kiln-loaders/kiln-content-id.so"),
            "{out}"
        );
        assert!(out.contains("header          .kiln/lib/main.h"), "{out}");
        assert!(out.contains("PIC object"), "{out}");
        assert!(out.contains("shared library"), "{out}");
        assert!(!out.contains("runtime object"), "{out}");
        assert!(!out.contains("launcher object"), "{out}");
        assert!(!out.contains("executable"), "{out}");
    }
}
