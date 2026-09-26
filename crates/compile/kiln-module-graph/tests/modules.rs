use kiln_hir::Module;
use kiln_host_abi::HostOps;
use kiln_host_abi::{HostError, HostOp, HostValue};
use kiln_interpreter::*;
use kiln_ir::{BYTECODE_VERSION, Instruction, Program};
use kiln_ir::{ExportBinding, ModuleProgram, compile_module};
use kiln_module_eval::{GraphModuleLoader, ModuleEvaluationError, ModuleGraphExecution};
use kiln_module_graph::*;
use kiln_module_resolver::{
    NodeModuleFormat, NodePackageResolver, NodeRuntimeResolver, PinnedPackageResolver,
};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

struct TempProject {
    root: PathBuf,
}

impl TempProject {
    fn new() -> Self {
        let id = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        // nextest runs each test in its own process, so a process-local counter alone makes
        // every parallel test claim `kiln-module-graph-0` and race on deletion and writes.
        let root =
            std::env::temp_dir().join(format!("kiln-module-graph-{}-{id}", std::process::id()));
        fs::create_dir_all(&root).expect("create temporary project");
        Self { root }
    }

    fn write(&self, name: &str, source: &str) -> PathBuf {
        let path = self.root.join(name);
        fs::write(&path, source).expect("write source");
        // ModuleGraph canonicalizes its entry and module keys. Returning the same identity from
        // the fixture keeps hand-built LiveDemand values faithful to the production API even on
        // macOS, where temp_dir may expose /var while canonicalize returns /private/var.
        fs::canonicalize(path).expect("canonical fixture path")
    }

    fn graph(&self, entry: &Path) -> Result<ModuleGraph, ModuleGraphError> {
        fn load(path: &Path) -> io::Result<String> {
            fs::read_to_string(path)
        }

        ModuleGraph::build(entry, &self.root, load, kiln_frontend_oxc::parse)
    }
}

struct OxcFrontend;

impl Frontend for OxcFrontend {
    fn parse(&mut self, path: &str, source: &str) -> Module {
        kiln_frontend_oxc::parse(path, source)
    }

    fn parse_commonjs(&mut self, path: &str, source: &str) -> CommonJsParse {
        let (module, static_requires) =
            kiln_frontend_oxc::parse_commonjs_with_requires(path, source);
        CommonJsParse {
            module,
            static_requires,
        }
    }
}

impl Drop for TempProject {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[derive(Default)]
struct TestHost {
    output: Vec<String>,
    errors: Vec<String>,
}

impl HostOps for TestHost {
    fn call(&mut self, operation: HostOp, arguments: &[HostValue]) -> Result<HostValue, HostError> {
        if operation == HostOp::WriteStderr {
            self.errors.push(
                arguments
                    .iter()
                    .map(HostValue::to_js_string)
                    .collect::<Vec<_>>()
                    .join(" "),
            );
            return Ok(HostValue::Undefined);
        }
        assert_eq!(operation, HostOp::WriteStdout);
        self.output.push(
            arguments
                .iter()
                .map(HostValue::to_js_string)
                .collect::<Vec<_>>()
                .join(" "),
        );
        Ok(HostValue::Undefined)
    }
}

struct ServerHostForPort {
    sender: Option<std::sync::mpsc::Sender<u16>>,
    output: Vec<String>,
}

fn http_header<'a>(response: &'a str, name: &str) -> Option<&'a str> {
    response
        .split_once("\r\n\r\n")
        .expect("HTTP response has a header terminator")
        .0
        .lines()
        .skip(1)
        .find_map(|line| {
            let (header_name, value) = line.split_once(':')?;
            header_name
                .eq_ignore_ascii_case(name)
                .then_some(value.trim())
        })
}

fn http_status(response: &str) -> &str {
    response
        .split_once("\r\n")
        .expect("HTTP response has a status line")
        .0
}

fn http_body(response: &str) -> Vec<u8> {
    let (head, body) = response
        .split_once("\r\n\r\n")
        .expect("HTTP response has a header terminator");
    let body = body.as_bytes();
    if http_header(response, "transfer-encoding").is_some_and(|value| {
        value
            .split(',')
            .any(|part| part.trim().eq_ignore_ascii_case("chunked"))
    }) {
        let mut decoded = Vec::new();
        let mut cursor = 0;
        loop {
            let line_end = body[cursor..]
                .windows(2)
                .position(|window| window == b"\r\n")
                .map(|offset| cursor + offset)
                .expect("chunk has a size line");
            let size_text = std::str::from_utf8(&body[cursor..line_end])
                .expect("chunk size is UTF-8")
                .split(';')
                .next()
                .expect("chunk size is present");
            let size =
                usize::from_str_radix(size_text.trim(), 16).expect("chunk size is hexadecimal");
            cursor = line_end + 2;
            if size == 0 {
                assert!(
                    body[cursor..].starts_with(b"\r\n"),
                    "chunked body is terminated"
                );
                break;
            }
            assert!(
                body.len() >= cursor + size + 2,
                "chunk has its full payload"
            );
            decoded.extend_from_slice(&body[cursor..cursor + size]);
            assert_eq!(&body[cursor + size..cursor + size + 2], b"\r\n");
            cursor += size + 2;
        }
        return decoded;
    }

    let length = http_header(response, "content-length")
        .expect("non-chunked response has Content-Length")
        .parse::<usize>()
        .expect("Content-Length is numeric");
    assert_eq!(
        head.len() + 4 + length,
        response.len(),
        "response body has its declared length"
    );
    body[..length].to_vec()
}

impl HostOps for ServerHostForPort {
    fn call(&mut self, operation: HostOp, arguments: &[HostValue]) -> Result<HostValue, HostError> {
        assert_eq!(operation, HostOp::WriteStdout);
        let text = arguments
            .first()
            .map_or_else(String::new, HostValue::to_js_string);
        if let Ok(port) = text.parse::<u16>()
            && let Some(sender) = self.sender.take()
        {
            sender.send(port).expect("publish server port");
        }
        self.output.push(text);
        Ok(HostValue::Undefined)
    }
}

fn reduce_all_live_demand(graph: &ModuleGraph) -> LiveDemand {
    let mut live_imports = BTreeSet::new();
    for node in graph.modules() {
        for (index, _) in node.program.imports.iter().enumerate() {
            live_imports.insert((node.path.clone(), index));
        }
    }
    LiveDemand {
        live: graph.modules().map(|node| node.path.clone()).collect(),
        slots: BTreeMap::new(),
        live_imports,
    }
}

#[path = "modules/execution.rs"]
mod execution;
#[path = "modules/linking.rs"]
mod linking;
#[path = "modules/node.rs"]
mod node;
