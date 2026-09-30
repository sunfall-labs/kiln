//! The program every interpreter realm runs once its global object exists.
//!
//! The shared TypeScript built-ins (`kiln-builtins`, ADR 0060) are installed by running them, and
//! the interpreter owns no frontend, so the embedder compiles them and hands the machine a
//! [`RealmBootstrap`]. This is that embedder side, shared by every host that runs the interpreter
//! -- the module evaluator `kiln run` uses, the conformance runner's script path, the daemon --
//! so there is one answer to "what does a realm start with" and it is compiled once per process.
//!
//! Compiled once, and only code is cached: a [`Program`] is immutable bytecode, and every object
//! the bootstrap builds is built afresh by running it in each realm (ADR 0038, ADR 0071). The
//! process-wide copy is what is compiled; each thread holds one `Rc` of it, because the machine
//! shares programs by `Rc` and a closure keeps its program alive past the realm that made it.

use kiln_interpreter::{REALM_BOOTSTRAP_PATH, RealmBootstrap};
use kiln_ir::{Program, compile_module};
use std::cell::OnceCell;
use std::rc::Rc;
use std::sync::LazyLock;

/// The bootstrap for a realm of the given mode.
///
/// Which sources are included is the module evaluator's long-standing choice, and the native
/// graph makes the same one: the common built-ins and `URL` in every mode, the web platform and
/// `Buffer` only under `--node` (whose `Buffer` captures `__kilnBufferToString`; a plain realm
/// deletes that bridge instead, so neither mode leaves it behind).
pub fn realm_bootstrap(node: bool) -> RealmBootstrap {
    if node {
        node_realm_bootstrap
    } else {
        plain_realm_bootstrap
    }
}

fn source(node: bool) -> String {
    let common = kiln_builtins::common_bootstrap_source();
    // WHATWG web standards (fetch, Headers, Request, Response, etc.) are global,
    // not gated behind the node adapter: the HTTP host capability
    // (process.__kilnNativeModules.http.request) exists in both modes.
    let web = kiln_builtins::WEB_BOOTSTRAP_SOURCE;
    let url = kiln_builtins::URL_BOOTSTRAP_SOURCE;
    if node {
        format!("{common}\n{url}\n{web}")
    } else {
        format!("{common}\n{url}\n{web}\ndelete globalThis.__kilnBufferToString;\n")
    }
}

fn compile(node: bool) -> Result<Program, String> {
    let source = source(node);
    let hir = kiln_frontend_oxc::parse(REALM_BOOTSTRAP_PATH, &source);
    compile_module(&hir)
        .map(|module| module.program)
        .map_err(|diagnostics| {
            diagnostics
                .iter()
                .map(|diagnostic| diagnostic.message.as_str())
                .collect::<Vec<_>>()
                .join("; ")
        })
}

static PLAIN: LazyLock<Result<Program, String>> = LazyLock::new(|| compile(false));
static NODE: LazyLock<Result<Program, String>> = LazyLock::new(|| compile(true));

thread_local! {
    static PLAIN_HERE: OnceCell<Rc<Program>> = const { OnceCell::new() };
    static NODE_HERE: OnceCell<Rc<Program>> = const { OnceCell::new() };
}

fn here(
    cell: &'static std::thread::LocalKey<OnceCell<Rc<Program>>>,
    compiled: &'static LazyLock<Result<Program, String>>,
) -> Result<Rc<Program>, String> {
    cell.with(|cell| {
        if let Some(program) = cell.get() {
            return Ok(Rc::clone(program));
        }
        let program = Rc::new(compiled.as_ref().map_err(Clone::clone)?.clone());
        Ok(Rc::clone(cell.get_or_init(|| program)))
    })
}

fn plain_realm_bootstrap() -> Result<Rc<Program>, String> {
    here(&PLAIN_HERE, &PLAIN)
}

fn node_realm_bootstrap() -> Result<Rc<Program>, String> {
    here(&NODE_HERE, &NODE)
}
