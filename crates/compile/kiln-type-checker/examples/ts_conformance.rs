//! TypeScript's compiler and conformance tests through `kiln check`'s frontend, adapter and
//! driver, compared with typescript-go's baselines: see `kiln_type_checker::conformance`.
//!
//! `cargo run --release --features baselines --example ts_conformance -- --bundle=<file> ..`

/// The frontend `kiln check` uses: a declaration file is parsed as authored, without the
/// transform executable TypeScript goes through.
fn parse(path: &str, source: &str) -> kiln_hir::Module {
    let lower = path.to_ascii_lowercase();
    if lower.ends_with(".d.ts") || lower.ends_with(".d.mts") || lower.ends_with(".d.cts") {
        kiln_frontend_oxc::parse_authored(path, source)
    } else {
        kiln_frontend_oxc::parse(path, source)
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<&[u8]> = args.iter().map(|arg| arg.as_bytes()).collect();
    let all_match = kiln_type_checker::conformance::run_from_command_line(&args, parse);
    std::process::exit(if all_match { 0 } else { 1 });
}
