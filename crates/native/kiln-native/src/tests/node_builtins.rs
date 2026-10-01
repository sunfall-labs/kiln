use super::*;

#[test]
fn typed_node_querystring_has_a_native_materializer() {
    use kiln_module_resolver::{NodeBuiltin, NodeRequestKind, PinnedPackageResolver};

    let project = TestDirectory::new("native-node-builtin-gate");
    let entry = project.path.join("main.mjs");
    fs::write(&entry, "export const marker = 1;\n").expect("write entry");
    let builtin_path = NodeBuiltin::QueryString.virtual_path(&project.path);
    let entry_program = ModuleProgram {
        program: Program {
            source: "".into(),
            version: BYTECODE_VERSION,
            source_origin: None,
            constants: vec![Constant::Number(1_f64.to_bits())],
            code: vec![
                Instruction::Const(0),
                Instruction::StoreLocal(0),
                Instruction::Const(0),
                Instruction::Return,
            ],
            local_count: 1,
            functions: Vec::new(),
            template_sites: Vec::new(),
        },
        dependencies: vec!["node:querystring".to_owned()],
        imports: Vec::new(),
        exports: vec![ExportBinding {
            exported: "marker".to_owned(),
            local_slot: 0,
        }],
        strictness: kiln_ir::Strictness::default(),
        ..Default::default()
    };
    let provider_entry = fs::canonicalize(&entry).expect("canonical entry");
    let graph = ModuleGraph::build_with_typed_policy(
        &entry,
        PinnedPackageResolver::new_with_node_builtins_requests_and_formats(
            &project.path,
            [(
                entry.clone(),
                "node:querystring".to_owned(),
                NodeRequestKind::Import,
                builtin_path,
            )],
            [],
        )
        .expect("node resolver"),
        move |path, source| {
            assert_eq!(path, provider_entry);
            assert_eq!(source, ModuleSource::Source);
            Ok(kiln_module_graph::ModulePayload::Program(
                entry_program.clone(),
            ))
        },
    )
    .expect("typed node graph");

    native_graph_plan(&graph).expect("querystring materializes");
    for export in kiln_builtins::QUERYSTRING_EXPORTS {
        assert!(native_builtin_supports_export(
            NodeBuiltin::QueryString,
            export
        ));
    }
}

#[test]
fn native_node_querystring_parses_form_data_and_preserves_namespace_identity() {
    use kiln_module_resolver::NodeRuntimeResolver;

    let root = TestDirectory::new("kiln-native-querystring");
    fs::write(root.path.join("package.json"), r#"{"type":"module"}"#)
        .expect("write package metadata");
    let entry = root.path.join("main.mjs");
    fs::write(
            &entry,
            r#"import querystring, { decode, parse } from "node:querystring";
const query = parse("hello=world&foo=b==ar&repeat=1&repeat=2&a+b=c+d&check=%E2%9C%93&bad=%ZZ&__proto__=safe");
const edges = decode("&&missing&=empty-key&empty-value=&short=%&half=%A&invalid=%C0&broken=%E2%28%A1");
console.log(parse === decode, querystring.parse === parse, query.hello, query.foo, query.repeat.join(","), query["a b"], query.check, query.bad, query.__proto__);
console.log(edges.missing === "", edges[""], edges["empty-value"] === "", edges.short, edges.half, edges.invalid, edges.broken);"#,
        )
        .expect("write querystring consumer");
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&root.path).expect("node resolver"),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("build querystring graph");

    let Some(toolchain) = Toolchain::discover().ok() else {
        eprintln!("skipping native querystring integration: no C compiler found");
        return;
    };
    let builder = NativeBuilder::new(test_cache(root.path.join("cache")), toolchain);
    let run = builder
        .run_graph(
            &graph,
            &NativeBuildOptions {
                output: root.path.join("program"),
                profile: NativeProfile::Dev,
                realm: NativeRealmPolicy::default(),
                march_native: false,
                ..NativeBuildOptions::default()
            },
        )
        .expect("native querystring graph builds and runs");
    assert_eq!(
        run.stdout,
        "true true world b==ar 1,2 c d ✓ %ZZ safe\ntrue empty-key true % %A � �(�\n".as_bytes()
    );
}

#[test]
fn native_create_require_uses_captured_virtual_filename_for_builtin_path() {
    use kiln_module_resolver::NodeRuntimeResolver;

    let root = TestDirectory::new("kiln-native-create-require-virtual");
    fs::write(root.path.join("package.json"), r#"{"type":"module"}"#)
        .expect("write package metadata");
    let entry = root.path.join("main.mjs");
    fs::write(
        &entry,
        r#"import { createRequire } from "node:module";
import * as staticallyLoadedPath from "node:path";
const require = createRequire("/virtual/main.cjs");
const path = require("node:path");
console.log(path.dirname === staticallyLoadedPath.dirname, path.basename("/tmp/file.js"));"#,
    )
    .expect("write createRequire consumer");
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&root.path).expect("node resolver"),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("build createRequire graph");
    assert!(
        graph.modules().any(|node| node.source
            == ModuleSource::NodeBuiltin(kiln_module_resolver::NodeBuiltin::Path)),
        "graph modules: {:?}",
        graph
            .modules()
            .map(|node| node.path.clone())
            .collect::<Vec<_>>()
    );

    let Some(toolchain) = Toolchain::discover().ok() else {
        eprintln!("skipping native createRequire integration: no C compiler found");
        return;
    };
    let run = NativeBuilder::new(test_cache(root.path.join("cache")), toolchain)
        .run_graph(
            &graph,
            &NativeBuildOptions {
                output: root.path.join("program"),
                profile: NativeProfile::Dev,
                realm: NativeRealmPolicy::default(),
                march_native: false,
                ..NativeBuildOptions::default()
            },
        )
        .expect("native createRequire graph builds and runs");
    assert_eq!(run.stdout, b"true file.js\n", "stderr={:?}", run.stderr);
}

#[test]
fn native_create_require_escaped_relative_cjs_and_demand_only_path() {
    use kiln_module_resolver::NodeRuntimeResolver;

    let root = TestDirectory::new("kiln-native-create-require-escaped");
    fs::write(root.path.join("package.json"), r#"{"type":"module"}"#)
        .expect("write package metadata");
    let entry = root.path.join("entry with space.mjs");
    fs::write(
        &entry,
        r#"import { createRequire } from "node:module";
function factory() {
  const require = createRequire(import.meta.url);
  return { load: () => require('./dep.cjs'), require };
}
const { load, require } = factory();
console.log(`before:${globalThis.__loads || 0}`);
const first = load();
const second = load();
console.log(`${first === second}:${first.loads}:${globalThis.__loads}`);
const unknownBase = '/virtual/' + (globalThis.__createRequireBase || 'main.cjs');
const fallbackPath = createRequire(unknownBase)('node:path');
console.log(typeof fallbackPath.basename);
const path = createRequire(import.meta.url)('node:path');
console.log(typeof path.basename);"#,
    )
    .expect("write escaped createRequire entry");
    fs::write(
        root.path.join("dep.cjs"),
        r#"globalThis.__loads = (globalThis.__loads || 0) + 1;
module.exports = { loads: globalThis.__loads };"#,
    )
    .expect("write escaped createRequire dependency");
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&root.path).expect("node resolver"),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("build escaped createRequire graph");
    assert!(
        graph
            .modules()
            .flat_map(|node| &node.runtime_dependencies)
            .any(|dependency| dependency.specifier == "node:path")
    );
    let Some(toolchain) = Toolchain::discover().ok() else {
        eprintln!("skipping escaped createRequire integration: no C compiler found");
        return;
    };
    let run = NativeBuilder::new(test_cache(root.path.join("cache")), toolchain)
        .run_graph(
            &graph,
            &NativeBuildOptions {
                output: root.path.join("program"),
                profile: NativeProfile::Dev,
                realm: NativeRealmPolicy::default(),
                march_native: false,
                ..NativeBuildOptions::default()
            },
        )
        .expect("native escaped createRequire graph builds and runs");
    assert_eq!(
        run.stdout, b"before:0\ntrue:1:1\nfunction\nfunction\n",
        "stderr={:?}",
        run.stderr
    );
}

#[test]
fn node_events_is_a_ts_builtin_and_https_has_a_native_runtime_materializer() {
    use kiln_module_resolver::NodeBuiltin;
    assert!(native_builtin_supports_export(
        NodeBuiltin::Events,
        "EventEmitter"
    ));
    assert!(native_builtin_supports_export(
        NodeBuiltin::Buffer,
        "Buffer"
    ));
    assert!(native_builtin_supports_export(NodeBuiltin::Buffer, "alloc"));
    assert!(native_builtin_supports_export(
        NodeBuiltin::StringDecoder,
        "StringDecoder"
    ));
    assert!(native_builtin_supports_export(
        NodeBuiltin::QueryString,
        "parse"
    ));
    assert!(native_builtin_supports_export(NodeBuiltin::Util, "format"));
    assert!(native_builtin_supports_export(
        NodeBuiltin::Util,
        "promisify"
    ));
    assert!(native_builtin_supports_export(NodeBuiltin::Url, "parse"));
    assert!(native_builtin_supports_export(
        NodeBuiltin::Url,
        "fileURLToPath"
    ));
    assert!(native_builtin_supports_export(
        NodeBuiltin::DiagnosticsChannel,
        "channel"
    ));
    assert!(native_builtin_supports_export(
        NodeBuiltin::DiagnosticsChannel,
        "tracingChannel"
    ));
    assert!(native_builtin_supports_export(
        NodeBuiltin::PerfHooks,
        "performance"
    ));
    assert!(native_builtin_supports_export(
        NodeBuiltin::PerfHooks,
        "PerformanceEntry"
    ));
    assert!(native_builtin_supports_export(
        NodeBuiltin::Console,
        "Console"
    ));
    assert!(native_builtin_supports_export(NodeBuiltin::Console, "log"));
    assert!(native_builtin_supports_export(NodeBuiltin::Tty, "isatty"));
    assert!(native_builtin_supports_export(
        NodeBuiltin::Tty,
        "ReadStream"
    ));
    assert!(native_builtin_supports_export(
        NodeBuiltin::Tty,
        "WriteStream"
    ));
    assert!(native_builtin_supports_export(
        NodeBuiltin::WorkerThreads,
        "Worker"
    ));
    assert!(native_builtin_supports_export(
        NodeBuiltin::WorkerThreads,
        "isMainThread"
    ));
    assert!(native_builtin_supports_export(
        NodeBuiltin::Module,
        "Module"
    ));
    assert!(native_builtin_supports_export(
        NodeBuiltin::Module,
        "createRequire"
    ));
    assert!(native_builtin_supports_export(
        NodeBuiltin::Module,
        "builtinModules"
    ));
    assert!(native_builtin_supports_export(
        NodeBuiltin::Https,
        "createServer"
    ));
    assert!(native_builtin_supports_export(NodeBuiltin::Path, "join"));
    assert!(native_builtin_supports_export(NodeBuiltin::Assert, "ok"));
    assert!(native_builtin_supports_export(
        NodeBuiltin::Zlib,
        "deflateSync"
    ));
}

#[test]
fn native_process_next_tick_forwards_variadic_arguments() {
    use kiln_module_resolver::NodeRuntimeResolver;
    use std::process::Command;

    let Some(toolchain) = Toolchain::discover().ok() else {
        eprintln!("skipping native process.nextTick integration: no C compiler found");
        return;
    };
    let project = TestDirectory::new("kiln-native-process-next-tick");
    let entry = project.path.join("main.mjs");
    fs::write(
        &entry,
        r#"process.nextTick((left, right) => console.log(left + right), 19, 23);
"#,
    )
    .expect("write process.nextTick fixture");
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&project.path).expect("node resolver"),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("source graph compiles");
    let report = NativeBuilder::new(test_cache(project.path.join("cache")), toolchain)
        .build_graph(
            &graph,
            &NativeBuildOptions {
                output: project.path.join("program"),
                profile: NativeProfile::Dev,
                realm: NativeRealmPolicy::default(),
                march_native: false,
                ..NativeBuildOptions::default()
            },
        )
        .expect("native process.nextTick graph builds");
    let output = Command::new(&report.output)
        .output_when_launchable()
        .expect("run native process.nextTick fixture");
    assert!(
        output.status.success(),
        "status={} stdout={:?} stderr={:?}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, b"42\n");
}

#[test]
fn native_node_fs_promises_read_write_stat_and_encoding() {
    use kiln_module_resolver::NodeRuntimeResolver;
    use std::process::Command;

    let Some(toolchain) = Toolchain::discover().ok() else {
        eprintln!("skipping native node:fs/promises integration: no C compiler found");
        return;
    };
    let project = TestDirectory::new("kiln-native-node-fs-promises");
    let entry = project.path.join("main.mjs");
    let data = project.path.join("data.txt");
    let malformed = project.path.join("malformed.bin");
    let missing = project.path.join("missing.txt");
    let path = data.to_string_lossy();
    let malformed_path = malformed.to_string_lossy();
    let missing_path = missing.to_string_lossy();
    fs::write(
        &entry,
        format!(
            r#"import {{ readFile, stat, writeFile }} from 'node:fs/promises';
writeFile({path:?}, 'héllo').then(() => readFile({path:?}, 'utf8')).then((text) => {{
  console.log(text);
  return readFile({path:?});
}}).then((bytes) => {{
  console.log(Buffer.isBuffer(bytes));
  console.log(bytes.length);
  return stat({path:?});
}}).then((info) => {{
  console.log(info.size);
  console.log(info.isFile());
  console.log(info.isDirectory());
  console.log(info['@@kiln:node:is-file'] === undefined);
  // The user-spelled string is an ordinary property, separate from the adapter's reserved key.
  Object.defineProperty(info, '@@kiln:node:is-file', {{ value: false }});
  console.log(info.isFile());
  try {{ delete info['@@kiln:node:is-file']; }} catch (error) {{ console.log(error.constructor.name); }}
  console.log(info.isFile());
  console.log(Object.hasOwn(info, '@@kiln:node:is-file'));
  console.log('@@kiln:node:is-file' in info);
  console.log(Object.getOwnPropertyDescriptor(info, '@@kiln:node:is-file').value);
  return writeFile({malformed_path:?}, new Uint8Array([0xe2, 0x82]));
}}).then(() => readFile({malformed_path:?}, 'utf8')).then((text) => {{
  console.log(text === '\ufffd');
  return readFile({missing_path:?});
}}).then(() => console.log('unexpected'), (error) => {{
  console.log(error.code);
  console.log(error.syscall);
  console.log(error.path === {missing_path:?});
}});
"#,
            path = path,
            malformed_path = malformed_path,
            missing_path = missing_path
        ),
    )
    .expect("write node:fs/promises fixture");
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&project.path).expect("node resolver"),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("source graph compiles");
    let report = NativeBuilder::new(test_cache(project.path.join("cache")), toolchain)
        .build_graph(
            &graph,
            &NativeBuildOptions {
                output: project.path.join("program"),
                profile: NativeProfile::Dev,
                realm: NativeRealmPolicy::default(),
                march_native: false,
                ..NativeBuildOptions::default()
            },
        )
        .expect("native node:fs/promises graph builds");
    let output = Command::new(&report.output)
        .output_when_launchable()
        .expect("run native node:fs/promises fixture");
    assert!(
        output.status.success(),
        "status={} stdout={:?} stderr={:?}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
            output.stdout,
            "héllo\ntrue\n6\n6\ntrue\nfalse\ntrue\ntrue\nTypeError\ntrue\ntrue\ntrue\nfalse\ntrue\nENOENT\nopen\ntrue\n".as_bytes()
        );
}

#[test]
fn node_ffi_is_typescript_builtin_and_supports_exports() {
    use kiln_module_resolver::NodeBuiltin;
    for export in kiln_builtins::FFI_EXPORTS {
        assert!(native_builtin_supports_export(NodeBuiltin::Ffi, export));
    }
}

#[test]
fn node_fs_is_typescript_builtin_and_supports_exports() {
    use kiln_module_resolver::NodeBuiltin;
    for export in kiln_builtins::FS_EXPORTS {
        assert!(native_builtin_supports_export(NodeBuiltin::Fs, export));
    }
    for export in kiln_builtins::FS_PROMISES_EXPORTS {
        assert!(native_builtin_supports_export(
            NodeBuiltin::FsPromises,
            export
        ));
    }
}

#[test]
fn embedded_shared_graph_create_require_uses_runtime_relative_cjs_edge() {
    use kiln_module_resolver::NodeRuntimeResolver;
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        eprintln!("skipping shared createRequire integration on unsupported platform");
        return;
    }
    let Some(toolchain) = Toolchain::discover().ok() else {
        eprintln!("skipping shared createRequire integration: no C compiler found");
        return;
    };
    let root = TestDirectory::new("kiln-native-shared-create-require");
    let project = root.path.join("project");
    fs::create_dir_all(&project).expect("create shared createRequire project");
    let entry = project.join("entry with space.mjs");
    fs::write(
        &entry,
        r#"import { createRequire } from "node:module";
let loads = 0;
function factory() {
  const require = createRequire(import.meta.url);
  return { load: () => require('./dep.cjs'), require };
}
const factoryResult = factory();
const load = factoryResult.load;
console.log(`before:${loads}`);
const first = load();
const second = load();
console.log(`${first === second}:${first.loads}:${loads}`);
try { createRequire(import.meta.url)('./missing.cjs'); } catch (error) { console.log(`${error instanceof TypeError}`); }"#,
    )
    .expect("write shared createRequire entry");
    fs::write(project.join("dep.cjs"), r#"globalThis.__shared_create_require_loads = (globalThis.__shared_create_require_loads || 0) + 1;
module.exports = { loads: globalThis.__shared_create_require_loads };"#)
        .expect("write shared createRequire dependency");
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&project).expect("node resolver"),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("build shared createRequire graph");
    let library = NativeBuilder::new(test_cache(root.path.join("cache")), toolchain.clone())
        .build_shared_graph(
            &graph,
            &NativeBuildOptions {
                output: root.path.join(shared_library_name()),
                profile: NativeProfile::Dev,
                realm: NativeRealmPolicy::default(),
                march_native: false,
                ..NativeBuildOptions::default()
            },
        )
        .expect("build shared createRequire library");
    let header_name = library
        .header
        .file_name()
        .and_then(|name| name.to_str())
        .expect("shared createRequire header");
    let host_source = root.path.join("host.c");
    fs::write(
        &host_source,
        format!(
            r#"#include "{header_name}"
#include <stdio.h>
static int host_call(void *context, uint16_t operation, uint16_t argc,
                     const kiln_value *arguments, kiln_value *result) {{
    (void)context;
    if (operation != KILN_HOST_OP_WRITE_STDOUT || argc != 1 || arguments == NULL ||
        result == NULL || arguments[0].tag != KILN_VALUE_STRING ||
        (arguments[0].len != 0 && arguments[0].ptr == NULL)) return KILN_C99_HOST_ERROR;
    const kiln_string *str = (const kiln_string *)arguments[0].ptr;
    for (size_t i = 0; i < arguments[0].len; ++i) {{
        if (fputc((int)kiln_string_unit_at(str, i), stdout) == EOF) return KILN_C99_HOST_ERROR;
    }}
    if (fputc('\n', stdout) == EOF) return KILN_C99_HOST_ERROR;
    *result = (kiln_value){{ .tag = KILN_VALUE_UNDEFINED, .flags = 0, .len = 0, .bits = 0 }};
    return KILN_C99_OK;
}}
int main(void) {{
    kiln_host host = {{ host_call, NULL }};
    kiln_value result;
    int status = {symbol}(&host, &result);
    fprintf(stderr, "status=%d tag=%u len=%u\\n", status, result.tag, result.len);
    return status == KILN_C99_OK ? 0 : 1;
}}
"#,
            symbol = library.module_symbol,
        ),
    )
    .expect("write shared createRequire host");
    let host_object = root.path.join("host.o");
    let mut compile = CompileOptions::default();
    compile.include_dirs.push(root.path.clone());
    toolchain
        .compile_c(&host_source, &host_object, &compile)
        .expect("compile shared createRequire host");
    let host_executable = root.path.join("host");
    let loader_directory = library.loader.parent().expect("loader directory");
    let mut link = LinkOptions::default();
    link.runtime_libraries.push(library.output.clone());
    link.extra_args
        .push(format!("-Wl,-rpath,{}", loader_directory.display()).into());
    toolchain
        .link_executable(&[host_object], &host_executable, &link)
        .expect("link shared createRequire host");
    let mut command = Command::new(&host_executable);
    #[cfg(target_os = "linux")]
    command.env("LD_LIBRARY_PATH", loader_directory);
    #[cfg(target_os = "macos")]
    command.env("DYLD_LIBRARY_PATH", loader_directory);
    let output = command.output().expect("run shared createRequire host");
    assert!(
        output.status.success(),
        "shared createRequire host failed with {:?}: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, b"before:0\ntrue:1:0\ntrue\n");
}

#[test]
fn native_node_os_typescript_builtin_builds_and_runs() {
    use kiln_module_resolver::NodeRuntimeResolver;
    let Ok(toolchain) = Toolchain::discover() else {
        return;
    };
    let project = TestDirectory::new("kiln-native-node-os-ts");
    let entry = project.path.join("main.mjs");
    fs::write(
        &entry,
        concat!(
            "import os, { platform, homedir, EOL } from 'node:os';\n",
            "console.log('os-test:', typeof platform(), typeof homedir(), EOL === '\\n', typeof os.tmpdir());\n",
        ),
    )
    .unwrap();
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&project.path).unwrap(),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .unwrap();
    let report = project
        .builder(toolchain)
        .build_graph(
            &graph,
            &NativeBuildOptions::new(project.path.join("os_bin"), NativeProfile::Release),
        )
        .unwrap();
    let result = Command::new(&report.output)
        .output_when_launchable()
        .expect("run os binary");
    assert!(
        result.status.success(),
        "process failed: stderr={}",
        String::from_utf8_lossy(&result.stderr)
    );
    let stdout = String::from_utf8_lossy(&result.stdout);
    assert_eq!(stdout.trim(), "os-test: string string true string");
}

#[test]
fn native_node_os_commonjs_require_builds_and_runs() {
    use kiln_module_resolver::NodeRuntimeResolver;
    let Ok(toolchain) = Toolchain::discover() else {
        return;
    };
    let project = TestDirectory::new("kiln-native-node-os-cjs");
    let entry = project.path.join("main.cjs");
    fs::write(
        &entry,
        concat!(
            "const os = require('node:os');\n",
            "console.log('cjs-os:', typeof os.platform, typeof os.homedir, os.EOL === '\\n', typeof os.tmpdir);\n",
        ),
    )
    .unwrap();
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&project.path).unwrap(),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .unwrap();
    let report = project
        .builder(toolchain)
        .build_graph(
            &graph,
            &NativeBuildOptions::new(project.path.join("os_cjs_bin"), NativeProfile::Release),
        )
        .unwrap();
    let result = Command::new(&report.output)
        .output_when_launchable()
        .expect("run os cjs binary");
    assert!(
        result.status.success(),
        "process failed: stderr={}",
        String::from_utf8_lossy(&result.stderr)
    );
    let stdout = String::from_utf8_lossy(&result.stdout);
    assert_eq!(stdout.trim(), "cjs-os: function function true function");
}

#[test]
fn native_node_path_typescript_builtin_builds_and_runs() {
    use kiln_module_resolver::NodeRuntimeResolver;
    let Ok(toolchain) = Toolchain::discover() else {
        return;
    };
    let project = TestDirectory::new("kiln-native-node-path-ts");
    let entry = project.path.join("main.mjs");
    fs::write(
        &entry,
        concat!(
            "import path, { join, resolve, dirname, basename, extname, parse, format, posix, sep } from 'node:path';\n",
            "console.log('path-test:', join('a', 'b'), resolve('/x', 'y'), dirname('/a/b/c.js'), basename('a/b.txt'), extname('a.txt'), sep === '/');\n",
            "console.log('parse-test:', JSON.stringify(parse('/a/b/c.txt')));\n",
            "console.log('format-test:', format({ dir: '/x/y', base: 'z.txt' }));\n",
            "console.log('posix-test:', path.posix === path);\n",
        ),
    )
    .unwrap();
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&project.path).unwrap(),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .unwrap();
    let report = project
        .builder(toolchain)
        .build_graph(
            &graph,
            &NativeBuildOptions::new(project.path.join("path_bin"), NativeProfile::Release),
        )
        .unwrap();
    let result = Command::new(&report.output)
        .output_when_launchable()
        .expect("run path binary");
    assert!(
        result.status.success(),
        "process failed: stderr={}",
        String::from_utf8_lossy(&result.stderr)
    );
    let stdout = String::from_utf8_lossy(&result.stdout);
    assert_eq!(
        stdout.trim(),
        concat!(
            "path-test: a/b /x/y /a/b b.txt .txt true\n",
            "parse-test: {\"root\":\"/\",\"dir\":\"/a/b\",\"base\":\"c.txt\",\"ext\":\".txt\",\"name\":\"c\"}\n",
            "format-test: /x/y/z.txt\n",
            "posix-test: true"
        )
    );
}

#[test]
fn native_node_path_commonjs_require_builds_and_runs() {
    use kiln_module_resolver::NodeRuntimeResolver;
    let Ok(toolchain) = Toolchain::discover() else {
        return;
    };
    let project = TestDirectory::new("kiln-native-node-path-cjs");
    let entry = project.path.join("main.cjs");
    fs::write(
        &entry,
        concat!(
            "const path = require('node:path');\n",
            "console.log('cjs-path:', path.join('foo', 'bar'), path.posix === path, path.default === path);\n",
        ),
    )
    .unwrap();
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&project.path).unwrap(),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .unwrap();
    let report = project
        .builder(toolchain)
        .build_graph(
            &graph,
            &NativeBuildOptions::new(project.path.join("path_cjs_bin"), NativeProfile::Release),
        )
        .unwrap();
    let result = Command::new(&report.output)
        .output_when_launchable()
        .expect("run path cjs binary");
    assert!(
        result.status.success(),
        "process failed: stderr={}",
        String::from_utf8_lossy(&result.stderr)
    );
    let stdout = String::from_utf8_lossy(&result.stdout);
    assert_eq!(stdout.trim(), "cjs-path: foo/bar true true");
}

#[test]
fn native_node_assert_typescript_builtin_builds_and_runs() {
    use kiln_module_resolver::NodeRuntimeResolver;
    let Ok(toolchain) = Toolchain::discover() else {
        return;
    };
    let project = TestDirectory::new("kiln-native-node-assert-ts");
    let entry = project.path.join("main.mjs");
    fs::write(
        &entry,
        concat!(
            "import assert, { ok, strict, strictEqual, deepStrictEqual, throws, doesNotThrow, AssertionError } from 'node:assert';\n",
            "assert(true);\n",
            "ok(true);\n",
            "strict(true);\n",
            "strictEqual(10 + 20, 30);\n",
            "deepStrictEqual({ x: [1, 2] }, { x: [1, 2] });\n",
            "doesNotThrow(() => 42);\n",
            "let caught = false;\n",
            "try { strictEqual(1, 2); } catch (e) { caught = e instanceof AssertionError; }\n",
            "console.log('assert-test:', caught);\n",
        ),
    )
    .unwrap();
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&project.path).unwrap(),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .unwrap();
    let report = project
        .builder(toolchain)
        .build_graph(
            &graph,
            &NativeBuildOptions::new(project.path.join("assert_bin"), NativeProfile::Release),
        )
        .unwrap();
    let result = Command::new(&report.output)
        .output_when_launchable()
        .expect("run assert binary");
    assert!(
        result.status.success(),
        "process failed: stderr={}",
        String::from_utf8_lossy(&result.stderr)
    );
    let stdout = String::from_utf8_lossy(&result.stdout);
    assert_eq!(stdout.trim(), "assert-test: true");
}

#[test]
fn native_node_assert_commonjs_require_builds_and_runs() {
    use kiln_module_resolver::NodeRuntimeResolver;
    let Ok(toolchain) = Toolchain::discover() else {
        return;
    };
    let project = TestDirectory::new("kiln-native-node-assert-cjs");
    let entry = project.path.join("main.cjs");
    fs::write(
        &entry,
        concat!(
            "const assert = require('node:assert');\n",
            "assert(true);\n",
            "assert.strictEqual(3 * 3, 9);\n",
            "console.log('cjs-assert:', typeof assert, assert.name, assert.default === assert, assert.strict === assert.strict.strict);\n",
        ),
    )
    .unwrap();
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&project.path).unwrap(),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .unwrap();
    let report = project
        .builder(toolchain)
        .build_graph(
            &graph,
            &NativeBuildOptions::new(project.path.join("assert_cjs_bin"), NativeProfile::Release),
        )
        .unwrap();
    let result = Command::new(&report.output)
        .output_when_launchable()
        .expect("run assert cjs binary");
    assert!(
        result.status.success(),
        "process failed: stderr={}",
        String::from_utf8_lossy(&result.stderr)
    );
    let stdout = String::from_utf8_lossy(&result.stdout);
    assert_eq!(stdout.trim(), "cjs-assert: function assert true true");
}

#[test]
fn native_node_events_typescript_builtin_builds_and_runs() {
    use kiln_module_resolver::NodeRuntimeResolver;
    let Ok(toolchain) = Toolchain::discover() else {
        return;
    };
    let project = TestDirectory::new("kiln-native-node-events-ts");
    let entry = project.path.join("main.mjs");
    fs::write(
        &entry,
        concat!(
            "import EventEmitter, { EventEmitter as EE } from 'node:events';\n",
            "const ee = new EventEmitter();\n",
            "let count = 0;\n",
            "ee.on('add', (x) => { count += x; });\n",
            "ee.once('add', (x) => { count += x * 10; });\n",
            "ee.emit('add', 2);\n",
            "ee.emit('add', 3);\n",
            "console.log('events-test:', count, ee.addListener === ee.on, EventEmitter === EE);\n",
        ),
    )
    .unwrap();
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&project.path).unwrap(),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .unwrap();
    let report = project
        .builder(toolchain)
        .build_graph(
            &graph,
            &NativeBuildOptions::new(project.path.join("events_bin"), NativeProfile::Release),
        )
        .unwrap();
    let result = Command::new(&report.output)
        .output_when_launchable()
        .expect("run events binary");
    assert!(
        result.status.success(),
        "process failed: stderr={}",
        String::from_utf8_lossy(&result.stderr)
    );
    let stdout = String::from_utf8_lossy(&result.stdout);
    assert_eq!(stdout.trim(), "events-test: 25 true true");
}

#[test]
fn native_node_events_commonjs_require_builds_and_runs() {
    use kiln_module_resolver::NodeRuntimeResolver;
    let Ok(toolchain) = Toolchain::discover() else {
        return;
    };
    let project = TestDirectory::new("kiln-native-node-events-cjs");
    let entry = project.path.join("main.cjs");
    fs::write(
        &entry,
        concat!(
            "const EventEmitter = require('node:events');\n",
            "const ee = new EventEmitter();\n",
            "let received = '';\n",
            "ee.on('data', (d) => { received = d; });\n",
            "ee.emit('data', 'payload');\n",
            "console.log('cjs-events:', typeof EventEmitter, EventEmitter.name, EventEmitter.default === EventEmitter, received);\n",
        ),
    )
    .unwrap();
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&project.path).unwrap(),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .unwrap();
    let report = project
        .builder(toolchain)
        .build_graph(
            &graph,
            &NativeBuildOptions::new(project.path.join("events_cjs_bin"), NativeProfile::Release),
        )
        .unwrap();
    let result = Command::new(&report.output)
        .output_when_launchable()
        .expect("run events cjs binary");
    assert!(
        result.status.success(),
        "process failed: stderr={}",
        String::from_utf8_lossy(&result.stderr)
    );
    let stdout = String::from_utf8_lossy(&result.stdout);
    assert_eq!(
        stdout.trim(),
        "cjs-events: function EventEmitter true payload"
    );
}

#[test]
fn native_node_buffer_typescript_builtin_builds_and_runs() {
    use kiln_module_resolver::NodeRuntimeResolver;
    let Ok(toolchain) = Toolchain::discover() else {
        return;
    };
    let project = TestDirectory::new("kiln-native-node-buffer-ts");
    let entry = project.path.join("main.mjs");
    fs::write(
        &entry,
        concat!(
            "import buffer, { Buffer, alloc, from, isBuffer, byteLength, concat, constants } from 'node:buffer';\n",
            "const b1 = alloc(3);\n",
            "const b2 = from('world');\n",
            "const b3 = concat([b1, b2]);\n",
            "console.log('buffer-test:', buffer.Buffer === Buffer, isBuffer(b1), b2.toString(), b3.length, byteLength('hello'), typeof constants.MAX_STRING_LENGTH);\n",
        ),
    )
    .unwrap();
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&project.path).unwrap(),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .unwrap();
    let report = project
        .builder(toolchain)
        .build_graph(
            &graph,
            &NativeBuildOptions::new(project.path.join("buffer_bin"), NativeProfile::Release),
        )
        .unwrap();
    let result = Command::new(&report.output)
        .output_when_launchable()
        .expect("run buffer binary");
    assert!(
        result.status.success(),
        "process failed: stderr={}",
        String::from_utf8_lossy(&result.stderr)
    );
    let stdout = String::from_utf8_lossy(&result.stdout);
    assert_eq!(stdout.trim(), "buffer-test: true true world 8 5 number");
}

#[test]
fn native_node_buffer_commonjs_require_builds_and_runs() {
    use kiln_module_resolver::NodeRuntimeResolver;
    let Ok(toolchain) = Toolchain::discover() else {
        return;
    };
    let project = TestDirectory::new("kiln-native-node-buffer-cjs");
    let entry = project.path.join("main.cjs");
    fs::write(
        &entry,
        concat!(
            "const buffer = require('node:buffer');\n",
            "const b = buffer.from('abc');\n",
            "console.log('cjs-buffer:', typeof buffer, buffer.Buffer === Buffer, buffer.isBuffer(b), b.toString(), buffer.default.Buffer === buffer.Buffer);\n",
        ),
    )
    .unwrap();
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&project.path).unwrap(),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .unwrap();
    let report = project
        .builder(toolchain)
        .build_graph(
            &graph,
            &NativeBuildOptions::new(project.path.join("buffer_cjs_bin"), NativeProfile::Release),
        )
        .unwrap();
    let result = Command::new(&report.output)
        .output_when_launchable()
        .expect("run buffer cjs binary");
    assert!(
        result.status.success(),
        "process failed: stderr={}",
        String::from_utf8_lossy(&result.stderr)
    );
    let stdout = String::from_utf8_lossy(&result.stdout);
    assert_eq!(stdout.trim(), "cjs-buffer: object true true abc true");
}

#[test]
fn native_node_string_decoder_typescript_builtin_builds_and_runs() {
    use kiln_module_resolver::NodeRuntimeResolver;
    let Ok(toolchain) = Toolchain::discover() else {
        return;
    };
    let project = TestDirectory::new("kiln-native-node-string-decoder-ts");
    let entry = project.path.join("main.mjs");
    fs::write(
        &entry,
        concat!(
            "import { StringDecoder } from 'node:string_decoder';\n",
            "const decoder = new StringDecoder('utf8');\n",
            "const p1 = decoder.write(new Uint8Array([0xf0, 0x9f]));\n",
            "const p2 = decoder.write(new Uint8Array([0x92, 0xa9, 0x21]));\n",
            "const p3 = decoder.end(new Uint8Array([0xe2]));\n",
            "console.log('sd-test:', typeof StringDecoder, p1 === '', p2 === '💩!', p3 === '\\uFFFD', decoder.encoding);\n",
        ),
    )
    .unwrap();
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&project.path).unwrap(),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .unwrap();
    let report = project
        .builder(toolchain)
        .build_graph(
            &graph,
            &NativeBuildOptions::new(project.path.join("sd_bin"), NativeProfile::Release),
        )
        .unwrap();
    let result = Command::new(&report.output)
        .output_when_launchable()
        .expect("run string_decoder binary");
    assert!(
        result.status.success(),
        "process failed: stderr={}",
        String::from_utf8_lossy(&result.stderr)
    );
    let stdout = String::from_utf8_lossy(&result.stdout);
    assert_eq!(stdout.trim(), "sd-test: function true true true utf8");
}

#[test]
fn native_node_string_decoder_commonjs_require_builds_and_runs() {
    use kiln_module_resolver::NodeRuntimeResolver;
    let Ok(toolchain) = Toolchain::discover() else {
        return;
    };
    let project = TestDirectory::new("kiln-native-node-string-decoder-cjs");
    let entry = project.path.join("main.cjs");
    fs::write(
        &entry,
        concat!(
            "const sd = require('node:string_decoder');\n",
            "const SD = sd.StringDecoder;\n",
            "const dec = new SD();\n",
            "const res = dec.write(new Uint8Array([119, 111, 114, 108, 100]));\n",
            "console.log('cjs-sd:', typeof sd, typeof SD, SD.StringDecoder === SD, res);\n",
        ),
    )
    .unwrap();
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&project.path).unwrap(),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .unwrap();
    let report = project
        .builder(toolchain)
        .build_graph(
            &graph,
            &NativeBuildOptions::new(project.path.join("sd_cjs_bin"), NativeProfile::Release),
        )
        .unwrap();
    let result = Command::new(&report.output)
        .output_when_launchable()
        .expect("run string_decoder cjs binary");
    assert!(
        result.status.success(),
        "process failed: stderr={}",
        String::from_utf8_lossy(&result.stderr)
    );
    let stdout = String::from_utf8_lossy(&result.stdout);
    assert_eq!(stdout.trim(), "cjs-sd: object function true world");
}

#[test]
fn native_node_querystring_typescript_builtin_builds_and_runs() {
    use kiln_module_resolver::NodeRuntimeResolver;
    let Ok(toolchain) = Toolchain::discover() else {
        return;
    };
    let project = TestDirectory::new("kiln-native-node-querystring-ts");
    let entry = project.path.join("main.mjs");
    fs::write(
        &entry,
        concat!(
            "import qs, { parse, decode, stringify, encode, escape, unescape } from 'node:querystring';\n",
            "const parsed = parse('alpha=1&beta=2&alpha=3');\n",
            "const str = stringify({ foo: 'bar', baz: ['a', 'b'] });\n",
            "const esc = escape('a/b?c=d');\n",
            "const unesc = unescape('a%2Fb%3Fc%3Dd');\n",
            "console.log('qs-ts:', typeof parse, parse === decode, stringify === encode, qs.parse === parse, qs.default === qs, parsed.alpha.join(','), parsed.beta, str, esc, unesc);\n",
        ),
    )
    .unwrap();
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&project.path).unwrap(),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .unwrap();
    let report = project
        .builder(toolchain)
        .build_graph(
            &graph,
            &NativeBuildOptions::new(project.path.join("qs_bin"), NativeProfile::Release),
        )
        .unwrap();
    let result = Command::new(&report.output)
        .output_when_launchable()
        .expect("run querystring binary");
    assert!(
        result.status.success(),
        "process failed: stderr={}",
        String::from_utf8_lossy(&result.stderr)
    );
    let stdout = String::from_utf8_lossy(&result.stdout);
    assert_eq!(
        stdout.trim(),
        "qs-ts: function true true true true 1,3 2 foo=bar&baz=a&baz=b a%2Fb%3Fc%3Dd a/b?c=d"
    );
}

#[test]
fn native_node_querystring_commonjs_require_builds_and_runs() {
    use kiln_module_resolver::NodeRuntimeResolver;
    let Ok(toolchain) = Toolchain::discover() else {
        return;
    };
    let project = TestDirectory::new("kiln-native-node-querystring-cjs");
    let entry = project.path.join("main.cjs");
    fs::write(
        &entry,
        concat!(
            "const qs = require('node:querystring');\n",
            "const parsed = qs.parse('user=alice&role=admin');\n",
            "const res = qs.stringify({ ok: true });\n",
            "console.log('cjs-qs:', typeof qs, typeof qs.parse, qs.default === qs, parsed.user, parsed.role, res);\n",
        ),
    )
    .unwrap();
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&project.path).unwrap(),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .unwrap();
    let report = project
        .builder(toolchain)
        .build_graph(
            &graph,
            &NativeBuildOptions::new(project.path.join("qs_cjs_bin"), NativeProfile::Release),
        )
        .unwrap();
    let result = Command::new(&report.output)
        .output_when_launchable()
        .expect("run querystring cjs binary");
    assert!(
        result.status.success(),
        "process failed: stderr={}",
        String::from_utf8_lossy(&result.stderr)
    );
    let stdout = String::from_utf8_lossy(&result.stdout);
    assert_eq!(
        stdout.trim(),
        "cjs-qs: object function true alice admin ok=true"
    );
}

#[test]
fn native_node_util_typescript_builtin_builds_and_runs() {
    use kiln_module_resolver::NodeRuntimeResolver;
    let Ok(toolchain) = Toolchain::discover() else {
        return;
    };
    let project = TestDirectory::new("kiln-native-node-util-ts");
    let entry = project.path.join("main.mjs");
    fs::write(
        &entry,
        concat!(
            "import util, { format, inherits, inspect, promisify, types, isArray } from 'node:util';\n",
            "const fmt = format('%s is %d years old', 'Bob', 30);\n",
            "function Animal() {}\n",
            "Animal.prototype.speak = function() { return 'noise'; };\n",
            "function Dog() {}\n",
            "inherits(Dog, Animal);\n",
            "const d = new Dog();\n",
            "const isArr = isArray([10, 20]);\n",
            "const isPromise = types.isPromise(Promise.resolve(1));\n",
            "console.log('util-ts:', typeof format, util.default === util, fmt, d.speak(), Dog.super_ === Animal, isArr, isPromise);\n",
        ),
    )
    .unwrap();
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&project.path).unwrap(),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .unwrap();
    let report = project
        .builder(toolchain)
        .build_graph(
            &graph,
            &NativeBuildOptions::new(project.path.join("util_bin"), NativeProfile::Release),
        )
        .unwrap();
    let result = Command::new(&report.output)
        .output_when_launchable()
        .expect("run util binary");
    assert!(
        result.status.success(),
        "process failed: stderr={}",
        String::from_utf8_lossy(&result.stderr)
    );
    let stdout = String::from_utf8_lossy(&result.stdout);
    assert_eq!(
        stdout.trim(),
        "util-ts: function true Bob is 30 years old noise true true true"
    );
}

#[test]
fn native_node_util_commonjs_require_builds_and_runs() {
    use kiln_module_resolver::NodeRuntimeResolver;
    let Ok(toolchain) = Toolchain::discover() else {
        return;
    };
    let project = TestDirectory::new("kiln-native-node-util-cjs");
    let entry = project.path.join("main.cjs");
    fs::write(
        &entry,
        concat!(
            "const util = require('node:util');\n",
            "const formatted = util.format('status: %s', 'running');\n",
            "console.log('cjs-util:', typeof util, typeof util.format, typeof util.promisify, util.default === util, formatted);\n",
        ),
    )
    .unwrap();
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&project.path).unwrap(),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .unwrap();
    let report = project
        .builder(toolchain)
        .build_graph(
            &graph,
            &NativeBuildOptions::new(project.path.join("util_cjs_bin"), NativeProfile::Release),
        )
        .unwrap();
    let result = Command::new(&report.output)
        .output_when_launchable()
        .expect("run util cjs binary");
    assert!(
        result.status.success(),
        "process failed: stderr={}",
        String::from_utf8_lossy(&result.stderr)
    );
    let stdout = String::from_utf8_lossy(&result.stdout);
    assert_eq!(
        stdout.trim(),
        "cjs-util: object function function true status: running"
    );
}

#[test]
fn native_node_url_typescript_builtin_builds_and_runs() {
    use kiln_module_resolver::NodeRuntimeResolver;
    let Ok(toolchain) = Toolchain::discover() else {
        return;
    };
    let project = TestDirectory::new("kiln-native-node-url-ts");
    let entry = project.path.join("main.mjs");
    fs::write(
        &entry,
        concat!(
            "import url, { URL, fileURLToPath, pathToFileURL, parse, format, urlToHttpOptions } from 'node:url';\n",
            "const p = fileURLToPath('file:///etc/hosts');\n",
            "const u = pathToFileURL('/etc/passwd');\n",
            "const parsed = parse('http://user:pass@example.com:8080/p/a/t/h?q=1#hash');\n",
            "const opts = urlToHttpOptions(new URL('http://example.com:3000/api'));\n",
            "console.log('url-ts:', typeof URL, url.default === url, p, u.protocol, parsed.hostname, parsed.port, opts.port);\n",
        ),
    )
    .unwrap();
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&project.path).unwrap(),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .unwrap();
    let report = project
        .builder(toolchain)
        .build_graph(
            &graph,
            &NativeBuildOptions::new(project.path.join("url_bin"), NativeProfile::Release),
        )
        .unwrap();
    let result = Command::new(&report.output)
        .output_when_launchable()
        .expect("run url binary");
    assert!(
        result.status.success(),
        "process failed: stderr={}",
        String::from_utf8_lossy(&result.stderr)
    );
    let stdout = String::from_utf8_lossy(&result.stdout);
    assert_eq!(
        stdout.trim(),
        "url-ts: function true /etc/hosts file: example.com 8080 3000"
    );
}

#[test]
fn native_node_url_commonjs_require_builds_and_runs() {
    use kiln_module_resolver::NodeRuntimeResolver;
    let Ok(toolchain) = Toolchain::discover() else {
        return;
    };
    let project = TestDirectory::new("kiln-native-node-url-cjs");
    let entry = project.path.join("main.cjs");
    fs::write(
        &entry,
        concat!(
            "const url = require('node:url');\n",
            "const p = url.fileURLToPath('file:///var/log');\n",
            "console.log('cjs-url:', typeof url, typeof url.URL, typeof url.parse, url.default === url, p);\n",
        ),
    )
    .unwrap();
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&project.path).unwrap(),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .unwrap();
    let report = project
        .builder(toolchain)
        .build_graph(
            &graph,
            &NativeBuildOptions::new(project.path.join("url_cjs_bin"), NativeProfile::Release),
        )
        .unwrap();
    let result = Command::new(&report.output)
        .output_when_launchable()
        .expect("run url cjs binary");
    assert!(
        result.status.success(),
        "process failed: stderr={}",
        String::from_utf8_lossy(&result.stderr)
    );
    let stdout = String::from_utf8_lossy(&result.stdout);
    assert_eq!(
        stdout.trim(),
        "cjs-url: object function function true /var/log"
    );
}

#[test]
fn native_node_diagnostics_channel_typescript_builtin_builds_and_runs() {
    use kiln_module_resolver::NodeRuntimeResolver;
    let Ok(toolchain) = Toolchain::discover() else {
        return;
    };
    let project = TestDirectory::new("kiln-native-node-dc-ts");
    let entry = project.path.join("main.mjs");
    fs::write(
        &entry,
        concat!(
            "import dc, { Channel, TracingChannel, channel, hasSubscribers, subscribe, unsubscribe, tracingChannel } from 'node:diagnostics_channel';\n",
            "const ch = channel('test.chan');\n",
            "let msgReceived = '';\n",
            "const sub = (data) => { msgReceived = data.value; };\n",
            "subscribe('test.chan', sub);\n",
            "const hasSubBefore = hasSubscribers('test.chan');\n",
            "ch.publish({ value: 'hello-dc' });\n",
            "unsubscribe('test.chan', sub);\n",
            "const hasSubAfter = hasSubscribers('test.chan');\n",
            "const tc = tracingChannel('test.trace');\n",
            "let ran = false;\n",
            "tc.start.runStores({}, function(x) { ran = this.ok && x === 99; }, { ok: true }, 99);\n",
            "console.log('dc-ts:', typeof channel, dc.default === dc, msgReceived, hasSubBefore, hasSubAfter, typeof tc.start, ran);\n",
        ),
    )
    .unwrap();
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&project.path).unwrap(),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .unwrap();
    let report = project
        .builder(toolchain)
        .build_graph(
            &graph,
            &NativeBuildOptions::new(project.path.join("dc_bin"), NativeProfile::Release),
        )
        .unwrap();
    let result = Command::new(&report.output)
        .output_when_launchable()
        .expect("run dc binary");
    assert!(
        result.status.success(),
        "process failed: stderr={}",
        String::from_utf8_lossy(&result.stderr)
    );
    let stdout = String::from_utf8_lossy(&result.stdout);
    assert_eq!(
        stdout.trim(),
        "dc-ts: function true hello-dc true false object true"
    );
}

#[test]
fn native_node_diagnostics_channel_commonjs_require_builds_and_runs() {
    use kiln_module_resolver::NodeRuntimeResolver;
    let Ok(toolchain) = Toolchain::discover() else {
        return;
    };
    let project = TestDirectory::new("kiln-native-node-dc-cjs");
    let entry = project.path.join("main.cjs");
    fs::write(
        &entry,
        concat!(
            "const dc = require('node:diagnostics_channel');\n",
            "const ch = dc.channel('test.cjs');\n",
            "console.log('cjs-dc:', typeof dc, typeof dc.channel, typeof dc.tracingChannel, dc.default === dc, typeof ch.publish);\n",
        ),
    )
    .unwrap();
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&project.path).unwrap(),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .unwrap();
    let report = project
        .builder(toolchain)
        .build_graph(
            &graph,
            &NativeBuildOptions::new(project.path.join("dc_cjs_bin"), NativeProfile::Release),
        )
        .unwrap();
    let result = Command::new(&report.output)
        .output_when_launchable()
        .expect("run dc cjs binary");
    assert!(
        result.status.success(),
        "process failed: stderr={}",
        String::from_utf8_lossy(&result.stderr)
    );
    let stdout = String::from_utf8_lossy(&result.stdout);
    assert_eq!(
        stdout.trim(),
        "cjs-dc: object function function true function"
    );
}

#[test]
fn native_node_perf_hooks_typescript_builtin_builds_and_runs() {
    use kiln_module_resolver::NodeRuntimeResolver;
    let Ok(toolchain) = Toolchain::discover() else {
        return;
    };
    let project = TestDirectory::new("kiln-native-node-perf-hooks-ts");
    let entry = project.path.join("main.mjs");
    fs::write(
        &entry,
        concat!(
            "import perfHooks, { Performance, PerformanceEntry, PerformanceMark, PerformanceMeasure, PerformanceObserver, performance, constants, createHistogram, monitorEventLoopDelay } from 'node:perf_hooks';\n",
            "const globEq = performance === globalThis.performance;\n",
            "console.log('perf-ts:', typeof performance.now, typeof Performance, typeof PerformanceEntry, typeof PerformanceMark, typeof createHistogram, typeof monitorEventLoopDelay, perfHooks.default === perfHooks, globEq, constants.NODE_PERFORMANCE_GC_MAJOR);\n",
        ),
    )
    .unwrap();
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&project.path).unwrap(),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .unwrap();
    let report = project
        .builder(toolchain)
        .build_graph(
            &graph,
            &NativeBuildOptions::new(project.path.join("perf_bin"), NativeProfile::Release),
        )
        .unwrap();
    let result = Command::new(&report.output)
        .output_when_launchable()
        .expect("run perf binary");
    assert!(
        result.status.success(),
        "process failed: stderr={}",
        String::from_utf8_lossy(&result.stderr)
    );
    let stdout = String::from_utf8_lossy(&result.stdout);
    assert_eq!(
        stdout.trim(),
        "perf-ts: function function function function function function true true 4"
    );
}

#[test]
fn native_node_perf_hooks_commonjs_require_builds_and_runs() {
    use kiln_module_resolver::NodeRuntimeResolver;
    let Ok(toolchain) = Toolchain::discover() else {
        return;
    };
    let project = TestDirectory::new("kiln-native-node-perf-hooks-cjs");
    let entry = project.path.join("main.cjs");
    fs::write(
        &entry,
        concat!(
            "const perfHooks = require('node:perf_hooks');\n",
            "const { performance } = perfHooks;\n",
            "console.log('cjs-perf:', typeof perfHooks, typeof performance.now, typeof performance.now(), perfHooks.default === perfHooks, performance === globalThis.performance);\n",
        ),
    )
    .unwrap();
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&project.path).unwrap(),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .unwrap();
    let report = project
        .builder(toolchain)
        .build_graph(
            &graph,
            &NativeBuildOptions::new(project.path.join("perf_cjs_bin"), NativeProfile::Release),
        )
        .unwrap();
    let result = Command::new(&report.output)
        .output_when_launchable()
        .expect("run perf cjs binary");
    assert!(
        result.status.success(),
        "process failed: stderr={}",
        String::from_utf8_lossy(&result.stderr)
    );
    let stdout = String::from_utf8_lossy(&result.stdout);
    assert_eq!(stdout.trim(), "cjs-perf: object function number true true");
}

#[test]
fn native_node_console_typescript_builtin_builds_and_runs() {
    use kiln_module_resolver::NodeRuntimeResolver;
    let Ok(toolchain) = Toolchain::discover() else {
        return;
    };
    let project = TestDirectory::new("kiln-native-node-console-ts");
    let entry = project.path.join("main.mjs");
    fs::write(
        &entry,
        concat!(
            "import nodeConsole, { Console, log, error, warn, info, debug, assert } from 'node:console';\n",
            "log('console-log-works');\n",
            "console.log('console-ts:', typeof nodeConsole, typeof Console, typeof log, typeof error, typeof warn, typeof info, typeof debug, typeof assert, nodeConsole.default === nodeConsole);\n",
        ),
    )
    .unwrap();
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&project.path).unwrap(),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .unwrap();
    let report = project
        .builder(toolchain)
        .build_graph(
            &graph,
            &NativeBuildOptions::new(project.path.join("console_bin"), NativeProfile::Release),
        )
        .unwrap();
    let result = Command::new(&report.output)
        .output_when_launchable()
        .expect("run console binary");
    assert!(
        result.status.success(),
        "process failed: stderr={}",
        String::from_utf8_lossy(&result.stderr)
    );
    let stdout = String::from_utf8_lossy(&result.stdout);
    assert!(stdout.contains("console-log-works"));
    assert!(stdout.contains(
        "console-ts: object function function function function function function function true"
    ));
}

#[test]
fn native_node_console_commonjs_require_builds_and_runs() {
    use kiln_module_resolver::NodeRuntimeResolver;
    let Ok(toolchain) = Toolchain::discover() else {
        return;
    };
    let project = TestDirectory::new("kiln-native-node-console-cjs");
    let entry = project.path.join("main.cjs");
    fs::write(
        &entry,
        concat!(
            "const c = require('node:console');\n",
            "c.log('cjs-console-works');\n",
            "console.log('cjs-console:', typeof c, typeof c.Console, typeof c.log, typeof c.warn, c.default === c);\n",
        ),
    )
    .unwrap();
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&project.path).unwrap(),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .unwrap();
    let report = project
        .builder(toolchain)
        .build_graph(
            &graph,
            &NativeBuildOptions::new(project.path.join("console_cjs_bin"), NativeProfile::Release),
        )
        .unwrap();
    let result = Command::new(&report.output)
        .output_when_launchable()
        .expect("run console cjs binary");
    assert!(
        result.status.success(),
        "process failed: stderr={}",
        String::from_utf8_lossy(&result.stderr)
    );
    let stdout = String::from_utf8_lossy(&result.stdout);
    assert!(stdout.contains("cjs-console-works"));
    assert!(stdout.contains("cjs-console: object function function function true"));
}

#[test]
fn native_node_tty_typescript_builtin_builds_and_runs() {
    use kiln_module_resolver::NodeRuntimeResolver;
    let Ok(toolchain) = Toolchain::discover() else {
        return;
    };
    let project = TestDirectory::new("kiln-native-node-tty-ts");
    let entry = project.path.join("main.mjs");
    fs::write(
        &entry,
        concat!(
            "import nodeTty, { isatty, ReadStream, WriteStream } from 'node:tty';\n",
            "console.log('tty-ts:', typeof nodeTty, typeof isatty, typeof ReadStream, typeof WriteStream, nodeTty.default === nodeTty, isatty(-1));\n",
        ),
    )
    .unwrap();
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&project.path).unwrap(),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .unwrap();
    let report = project
        .builder(toolchain)
        .build_graph(
            &graph,
            &NativeBuildOptions::new(project.path.join("tty_bin"), NativeProfile::Release),
        )
        .unwrap();
    let result = Command::new(&report.output)
        .output_when_launchable()
        .expect("run tty binary");
    assert!(
        result.status.success(),
        "process failed: stderr={}",
        String::from_utf8_lossy(&result.stderr)
    );
    let stdout = String::from_utf8_lossy(&result.stdout);
    assert!(stdout.contains("tty-ts: object function function function true false"));
}

#[test]
fn native_node_tty_commonjs_require_builds_and_runs() {
    use kiln_module_resolver::NodeRuntimeResolver;
    let Ok(toolchain) = Toolchain::discover() else {
        return;
    };
    let project = TestDirectory::new("kiln-native-node-tty-cjs");
    let entry = project.path.join("main.cjs");
    fs::write(
        &entry,
        concat!(
            "const t = require('node:tty');\n",
            "console.log('cjs-tty:', typeof t, typeof t.isatty, typeof t.ReadStream, typeof t.WriteStream, t.default === t, t.isatty(-1));\n",
        ),
    )
    .unwrap();
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&project.path).unwrap(),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .unwrap();
    let report = project
        .builder(toolchain)
        .build_graph(
            &graph,
            &NativeBuildOptions::new(project.path.join("tty_cjs_bin"), NativeProfile::Release),
        )
        .unwrap();
    let result = Command::new(&report.output)
        .output_when_launchable()
        .expect("run tty cjs binary");
    assert!(
        result.status.success(),
        "process failed: stderr={}",
        String::from_utf8_lossy(&result.stderr)
    );
    let stdout = String::from_utf8_lossy(&result.stdout);
    assert!(stdout.contains("cjs-tty: object function function function true false"));
}

#[test]
fn native_node_worker_threads_typescript_builtin_builds_and_runs() {
    use kiln_module_resolver::NodeRuntimeResolver;
    let Ok(toolchain) = Toolchain::discover() else {
        return;
    };
    let project = TestDirectory::new("kiln-native-node-worker-threads-ts");
    let entry = project.path.join("main.mjs");
    fs::write(
        &entry,
        concat!(
            "import nodeWt, { Worker, isMainThread, parentPort, threadId, setEnvironmentData, getEnvironmentData } from 'node:worker_threads';\n",
            "setEnvironmentData('num', 42);\n",
            "console.log('wt-ts:', typeof nodeWt, typeof Worker, isMainThread, parentPort === null, threadId, getEnvironmentData('num'), nodeWt.default === nodeWt);\n",
        ),
    )
    .unwrap();
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&project.path).unwrap(),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .unwrap();
    let report = project
        .builder(toolchain)
        .build_graph(
            &graph,
            &NativeBuildOptions::new(project.path.join("wt_bin"), NativeProfile::Release),
        )
        .unwrap();
    let result = Command::new(&report.output)
        .output_when_launchable()
        .expect("run wt binary");
    assert!(
        result.status.success(),
        "process failed: stderr={}",
        String::from_utf8_lossy(&result.stderr)
    );
    let stdout = String::from_utf8_lossy(&result.stdout);
    assert!(stdout.contains("wt-ts: object function true true 0 42 true"));
}

#[test]
fn native_node_worker_threads_commonjs_require_builds_and_runs() {
    use kiln_module_resolver::NodeRuntimeResolver;
    let Ok(toolchain) = Toolchain::discover() else {
        return;
    };
    let project = TestDirectory::new("kiln-native-node-worker-threads-cjs");
    let entry = project.path.join("main.cjs");
    fs::write(
        &entry,
        concat!(
            "const wt = require('node:worker_threads');\n",
            "console.log('cjs-wt:', typeof wt, typeof wt.Worker, wt.isMainThread, wt.default === wt);\n",
        ),
    )
    .unwrap();
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&project.path).unwrap(),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .unwrap();
    let report = project
        .builder(toolchain)
        .build_graph(
            &graph,
            &NativeBuildOptions::new(project.path.join("wt_cjs_bin"), NativeProfile::Release),
        )
        .unwrap();
    let result = Command::new(&report.output)
        .output_when_launchable()
        .expect("run wt cjs binary");
    assert!(
        result.status.success(),
        "process failed: stderr={}",
        String::from_utf8_lossy(&result.stderr)
    );
    let stdout = String::from_utf8_lossy(&result.stdout);
    assert!(stdout.contains("cjs-wt: object function true true"));
}

#[test]
fn native_node_module_typescript_builtin_builds_and_runs() {
    use kiln_module_resolver::NodeRuntimeResolver;
    let Ok(toolchain) = Toolchain::discover() else {
        return;
    };
    let project = TestDirectory::new("kiln-native-node-module-ts");
    let entry = project.path.join("main.mjs");
    fs::write(
        &entry,
        concat!(
            "import defaultModule, { Module, createRequire, isBuiltin } from 'node:module';\n",
            "console.log('module-ts:', typeof defaultModule, defaultModule === Module, typeof createRequire, isBuiltin('fs'));\n",
        ),
    )
    .unwrap();
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&project.path).unwrap(),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .unwrap();
    let report = project
        .builder(toolchain)
        .build_graph(
            &graph,
            &NativeBuildOptions::new(project.path.join("module_bin"), NativeProfile::Release),
        )
        .unwrap();
    let result = Command::new(&report.output)
        .output_when_launchable()
        .expect("run module binary");
    assert!(
        result.status.success(),
        "process failed: stderr={}",
        String::from_utf8_lossy(&result.stderr)
    );
    let stdout = String::from_utf8_lossy(&result.stdout);
    assert!(stdout.contains("module-ts: function true function true"));
}

#[test]
fn native_node_module_commonjs_require_builds_and_runs() {
    use kiln_module_resolver::NodeRuntimeResolver;
    let Ok(toolchain) = Toolchain::discover() else {
        return;
    };
    let project = TestDirectory::new("kiln-native-node-module-cjs");
    let entry = project.path.join("main.cjs");
    fs::write(
        &entry,
        concat!(
            "const m = require('node:module');\n",
            "console.log('cjs-module:', typeof m, typeof m.createRequire, m.isBuiltin('path'), m.default === m);\n",
        ),
    )
    .unwrap();
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&project.path).unwrap(),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .unwrap();
    let report = project
        .builder(toolchain)
        .build_graph(
            &graph,
            &NativeBuildOptions::new(project.path.join("module_cjs_bin"), NativeProfile::Release),
        )
        .unwrap();
    let result = Command::new(&report.output)
        .output_when_launchable()
        .expect("run module cjs binary");
    assert!(
        result.status.success(),
        "process failed: stderr={}",
        String::from_utf8_lossy(&result.stderr)
    );
    let stdout = String::from_utf8_lossy(&result.stdout);
    assert!(stdout.contains("cjs-module: function function true true"));
}

#[test]
fn native_node_dns_typescript_builtin_builds_and_runs() {
    use kiln_module_resolver::NodeRuntimeResolver;
    let Ok(toolchain) = Toolchain::discover() else {
        return;
    };
    let project = TestDirectory::new("kiln-native-node-dns-ts");
    let entry = project.path.join("main.mjs");
    fs::write(
        &entry,
        concat!(
            "import dns, { lookup, ADDRCONFIG } from 'node:dns';\n",
            "let lookedUp = false;\n",
            "lookup('localhost', (err, address, family) => {\n",
            "  lookedUp = address === '127.0.0.1' && family === 4;\n",
            "});\n",
            "console.log('dns-ts:', typeof dns, typeof lookup, ADDRCONFIG === 32, lookedUp);\n",
        ),
    )
    .unwrap();
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&project.path).unwrap(),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .unwrap();
    let report = project
        .builder(toolchain)
        .build_graph(
            &graph,
            &NativeBuildOptions::new(project.path.join("dns_bin"), NativeProfile::Release),
        )
        .unwrap();
    let result = Command::new(&report.output)
        .output_when_launchable()
        .expect("run dns binary");
    assert!(
        result.status.success(),
        "process failed: stderr={}",
        String::from_utf8_lossy(&result.stderr)
    );
    let stdout = String::from_utf8_lossy(&result.stdout);
    assert!(stdout.contains("dns-ts: object function true true"));
}

#[test]
fn native_node_dns_commonjs_require_builds_and_runs() {
    use kiln_module_resolver::NodeRuntimeResolver;
    let Ok(toolchain) = Toolchain::discover() else {
        return;
    };
    let project = TestDirectory::new("kiln-native-node-dns-cjs");
    let entry = project.path.join("main.cjs");
    fs::write(
        &entry,
        concat!(
            "const dns = require('node:dns');\n",
            "let lookedUp = false;\n",
            "dns.lookup('localhost', (err, address, family) => {\n",
            "  lookedUp = address === '127.0.0.1' && family === 4;\n",
            "});\n",
            "console.log('cjs-dns:', typeof dns, typeof dns.lookup, dns.ADDRCONFIG === 32, lookedUp);\n",
        ),
    )
    .unwrap();
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&project.path).unwrap(),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .unwrap();
    let report = project
        .builder(toolchain)
        .build_graph(
            &graph,
            &NativeBuildOptions::new(project.path.join("dns_cjs_bin"), NativeProfile::Release),
        )
        .unwrap();
    let result = Command::new(&report.output)
        .output_when_launchable()
        .expect("run dns cjs binary");
    assert!(
        result.status.success(),
        "process failed: stderr={}",
        String::from_utf8_lossy(&result.stderr)
    );
    let stdout = String::from_utf8_lossy(&result.stdout);
    assert!(stdout.contains("cjs-dns: object function true true"));
}

#[test]
fn native_node_crypto_typescript_builtin_builds_and_runs() {
    use kiln_module_resolver::NodeRuntimeResolver;
    let Ok(toolchain) = Toolchain::discover() else {
        return;
    };
    let project = TestDirectory::new("kiln-native-node-crypto-ts");
    let entry = project.path.join("main.mjs");
    fs::write(
        &entry,
        concat!(
            "import crypto, { createHash, randomUUID } from 'node:crypto';\n",
            "const hash = createHash('sha256').update('hello world').digest('hex');\n",
            "const uuid = randomUUID();\n",
            "console.log('crypto-ts:', typeof crypto, hash === 'b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9', typeof uuid === 'string' && uuid.length === 36);\n",
        ),
    )
    .unwrap();
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&project.path).unwrap(),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .unwrap();
    let report = project
        .builder(toolchain)
        .build_graph(
            &graph,
            &NativeBuildOptions::new(project.path.join("crypto_bin"), NativeProfile::Release),
        )
        .unwrap();
    let result = Command::new(&report.output)
        .output_when_launchable()
        .expect("run crypto binary");
    assert!(
        result.status.success(),
        "process failed: stderr={}",
        String::from_utf8_lossy(&result.stderr)
    );
    let stdout = String::from_utf8_lossy(&result.stdout);
    assert!(stdout.contains("crypto-ts: object true true"));
}

#[test]
fn native_node_crypto_commonjs_require_builds_and_runs() {
    use kiln_module_resolver::NodeRuntimeResolver;
    let Ok(toolchain) = Toolchain::discover() else {
        return;
    };
    let project = TestDirectory::new("kiln-native-node-crypto-cjs");
    let entry = project.path.join("main.cjs");
    fs::write(
        &entry,
        concat!(
            "const crypto = require('node:crypto');\n",
            "const hash = crypto.createHash('sha256').update('hello world').digest('hex');\n",
            "console.log('cjs-crypto:', typeof crypto, typeof crypto.createHash, hash === 'b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9');\n",
        ),
    )
    .unwrap();
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&project.path).unwrap(),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .unwrap();
    let report = project
        .builder(toolchain)
        .build_graph(
            &graph,
            &NativeBuildOptions::new(project.path.join("crypto_cjs_bin"), NativeProfile::Release),
        )
        .unwrap();
    let result = Command::new(&report.output)
        .output_when_launchable()
        .expect("run crypto cjs binary");
    assert!(
        result.status.success(),
        "process failed: stderr={}",
        String::from_utf8_lossy(&result.stderr)
    );
    let stdout = String::from_utf8_lossy(&result.stdout);
    assert!(stdout.contains("cjs-crypto: object function true"));
}

#[test]
fn native_node_zlib_typescript_builtin_builds_and_runs() {
    use kiln_module_resolver::NodeRuntimeResolver;
    let Ok(toolchain) = Toolchain::discover() else {
        return;
    };
    let project = TestDirectory::new("kiln-native-node-zlib-ts");
    let entry = project.path.join("main.mjs");
    fs::write(
        &entry,
        concat!(
            "import zlib, { constants, deflateSync, inflateSync, gzipSync, gunzipSync } from 'node:zlib';\n",
            "const def = deflateSync('native zlib string');\n",
            "const inf = inflateSync(def);\n",
            "const gz = gzipSync('native gzip string');\n",
            "const ungz = gunzipSync(gz);\n",
            "console.log('zlib-ts:', typeof zlib, constants.Z_OK === 0, inf.toString() === 'native zlib string', ungz.toString() === 'native gzip string');\n",
        ),
    )
    .unwrap();
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&project.path).unwrap(),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .unwrap();
    let report = project
        .builder(toolchain)
        .build_graph(
            &graph,
            &NativeBuildOptions::new(project.path.join("zlib_bin"), NativeProfile::Release),
        )
        .unwrap();
    let result = Command::new(&report.output)
        .output_when_launchable()
        .expect("run zlib binary");
    assert!(
        result.status.success(),
        "process failed: stderr={}",
        String::from_utf8_lossy(&result.stderr)
    );
    let stdout = String::from_utf8_lossy(&result.stdout);
    assert!(stdout.contains("zlib-ts: object true true true"));
}

#[test]
fn native_node_zlib_commonjs_require_builds_and_runs() {
    use kiln_module_resolver::NodeRuntimeResolver;
    let Ok(toolchain) = Toolchain::discover() else {
        return;
    };
    let project = TestDirectory::new("kiln-native-node-zlib-cjs");
    let entry = project.path.join("main.cjs");
    fs::write(
        &entry,
        concat!(
            "const zlib = require('node:zlib');\n",
            "const def = zlib.deflateSync('native zlib cjs');\n",
            "const inf = zlib.inflateSync(def);\n",
            "console.log('cjs-zlib:', typeof zlib, zlib.constants.Z_OK === 0, inf.toString() === 'native zlib cjs');\n",
        ),
    )
    .unwrap();
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&project.path).unwrap(),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .unwrap();
    let report = project
        .builder(toolchain)
        .build_graph(
            &graph,
            &NativeBuildOptions::new(project.path.join("zlib_cjs_bin"), NativeProfile::Release),
        )
        .unwrap();
    let result = Command::new(&report.output)
        .output_when_launchable()
        .expect("run zlib cjs binary");
    assert!(
        result.status.success(),
        "process failed: stderr={}",
        String::from_utf8_lossy(&result.stderr)
    );
    let stdout = String::from_utf8_lossy(&result.stdout);
    assert!(stdout.contains("cjs-zlib: object true true"));
}

#[test]
fn native_node_async_hooks_typescript_builtin_builds_and_runs() {
    use kiln_module_resolver::NodeRuntimeResolver;
    let Ok(toolchain) = Toolchain::discover() else {
        return;
    };
    let project = TestDirectory::new("kiln-native-node-async-hooks-ts");
    let entry = project.path.join("main.mjs");
    fs::write(
        &entry,
        concat!(
            "import hooks, { AsyncLocalStorage, AsyncResource } from 'node:async_hooks';\n",
            "const als = new AsyncLocalStorage();\n",
            "const sync = als.run(999, () => als.getStore());\n",
            "const res = new AsyncResource('probe');\n",
            "const bound = res.bind((x) => x + 1);\n",
            "console.log('ah-ts:', typeof hooks, typeof AsyncLocalStorage, typeof AsyncResource, sync, bound(10));\n",
        ),
    )
    .unwrap();
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&project.path).unwrap(),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .unwrap();
    let report = project
        .builder(toolchain)
        .build_graph(
            &graph,
            &NativeBuildOptions::new(project.path.join("async_hooks_bin"), NativeProfile::Release),
        )
        .unwrap();
    let result = Command::new(&report.output)
        .output_when_launchable()
        .expect("run async_hooks binary");
    assert!(
        result.status.success(),
        "process failed: stderr={}",
        String::from_utf8_lossy(&result.stderr)
    );
    let stdout = String::from_utf8_lossy(&result.stdout);
    assert!(stdout.contains("ah-ts: object function function 999 11"));
}

#[test]
fn native_node_async_hooks_commonjs_require_builds_and_runs() {
    use kiln_module_resolver::NodeRuntimeResolver;
    let Ok(toolchain) = Toolchain::discover() else {
        return;
    };
    let project = TestDirectory::new("kiln-native-node-async-hooks-cjs");
    let entry = project.path.join("main.cjs");
    fs::write(
        &entry,
        concat!(
            "const hooks = require('node:async_hooks');\n",
            "const als = new hooks.AsyncLocalStorage();\n",
            "const sync = als.run('cjs-store', () => als.getStore());\n",
            "const res = new hooks.AsyncResource('probe');\n",
            "const bound = res.bind((x) => x * 2);\n",
            "console.log('cjs-ah:', typeof hooks, typeof hooks.AsyncLocalStorage, typeof hooks.AsyncResource, sync, bound(5));\n",
        ),
    )
    .unwrap();
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&project.path).unwrap(),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .unwrap();
    let report = project
        .builder(toolchain)
        .build_graph(
            &graph,
            &NativeBuildOptions::new(
                project.path.join("async_hooks_cjs_bin"),
                NativeProfile::Release,
            ),
        )
        .unwrap();
    let result = Command::new(&report.output)
        .output_when_launchable()
        .expect("run async_hooks cjs binary");
    assert!(
        result.status.success(),
        "process failed: stderr={}",
        String::from_utf8_lossy(&result.stderr)
    );
    let stdout = String::from_utf8_lossy(&result.stdout);
    assert!(stdout.contains("cjs-ah: object function function cjs-store 10"));
}

#[test]
fn native_node_fs_typescript_builtin_builds_and_runs() {
    use kiln_module_resolver::NodeRuntimeResolver;
    let Ok(toolchain) = Toolchain::discover() else {
        return;
    };
    let project = TestDirectory::new("kiln-native-node-fs-ts");
    let entry = project.path.join("main.mjs");
    fs::write(
        &entry,
        format!(
            "import fs, {{ existsSync, promises, readFileSync }} from 'node:fs';\nimport {{ readFile }} from 'node:fs/promises';\nconsole.log('fs-ts:', typeof fs, typeof existsSync, typeof promises, typeof readFileSync, typeof readFile, fs.default === fs, existsSync({:?}));\n",
            entry.to_string_lossy(),
        ),
    )
    .unwrap();
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&project.path).unwrap(),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .unwrap();
    let report = project
        .builder(toolchain)
        .build_graph(
            &graph,
            &NativeBuildOptions::new(project.path.join("fs_bin"), NativeProfile::Release),
        )
        .unwrap();
    let result = Command::new(&report.output)
        .output_when_launchable()
        .expect("run fs binary");
    assert!(
        result.status.success(),
        "process failed: stderr={}",
        String::from_utf8_lossy(&result.stderr)
    );
    let stdout = String::from_utf8_lossy(&result.stdout);
    assert!(stdout.contains("fs-ts: object function object function function true true"));
}

#[test]
fn native_node_fs_commonjs_require_builds_and_runs() {
    use kiln_module_resolver::NodeRuntimeResolver;
    let Ok(toolchain) = Toolchain::discover() else {
        return;
    };
    let project = TestDirectory::new("kiln-native-node-fs-cjs");
    let entry = project.path.join("main.cjs");
    fs::write(
        &entry,
        concat!(
            "const fs = require('node:fs');\n",
            "console.log('cjs-fs:', typeof fs, typeof fs.existsSync, typeof fs.promises, typeof fs.readFileSync, typeof fs.promises.readFile, fs.default === fs);\n",
        ),
    )
    .unwrap();
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&project.path).unwrap(),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .unwrap();
    let report = project
        .builder(toolchain)
        .build_graph(
            &graph,
            &NativeBuildOptions::new(project.path.join("fs_cjs_bin"), NativeProfile::Release),
        )
        .unwrap();
    let result = Command::new(&report.output)
        .output_when_launchable()
        .expect("run fs cjs binary");
    assert!(
        result.status.success(),
        "process failed: stderr={}",
        String::from_utf8_lossy(&result.stderr)
    );
    let stdout = String::from_utf8_lossy(&result.stdout);
    assert!(stdout.contains("cjs-fs: object function object function function true"));
}

#[test]
fn native_node_ffi_typescript_builtin_builds_and_runs() {
    use kiln_module_resolver::NodeRuntimeResolver;
    let Ok(toolchain) = Toolchain::discover() else {
        return;
    };
    let project = TestDirectory::new("kiln-native-node-ffi-ts");
    let entry = project.path.join("main.mjs");
    fs::write(
        &entry,
        concat!(
            "import ffi, { dlopen, getRawPointer, suffix, toArrayBuffer } from 'node:ffi';\n",
            "console.log('ffi-ts:', typeof ffi, typeof dlopen, typeof getRawPointer, typeof suffix, typeof toArrayBuffer, ffi.default === ffi);\n",
        ),
    )
    .unwrap();
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&project.path).unwrap(),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .unwrap();
    let report = project
        .builder(toolchain)
        .build_graph(
            &graph,
            &NativeBuildOptions::new(project.path.join("ffi_bin"), NativeProfile::Release),
        )
        .unwrap();
    let result = Command::new(&report.output)
        .output_when_launchable()
        .expect("run ffi binary");
    assert!(
        result.status.success(),
        "process failed: stderr={}",
        String::from_utf8_lossy(&result.stderr)
    );
    let stdout = String::from_utf8_lossy(&result.stdout);
    assert!(stdout.contains("ffi-ts: object function function string function true"));
}

#[test]
fn native_node_ffi_commonjs_require_builds_and_runs() {
    use kiln_module_resolver::NodeRuntimeResolver;
    let Ok(toolchain) = Toolchain::discover() else {
        return;
    };
    let project = TestDirectory::new("kiln-native-node-ffi-cjs");
    let entry = project.path.join("main.cjs");
    fs::write(
        &entry,
        concat!(
            "const ffi = require('node:ffi');\n",
            "console.log('cjs-ffi:', typeof ffi, typeof ffi.dlopen, typeof ffi.getRawPointer, typeof ffi.suffix, typeof ffi.toArrayBuffer, ffi.default === ffi);\n",
        ),
    )
    .unwrap();
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&project.path).unwrap(),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .unwrap();
    let report = project
        .builder(toolchain)
        .build_graph(
            &graph,
            &NativeBuildOptions::new(project.path.join("ffi_cjs_bin"), NativeProfile::Release),
        )
        .unwrap();
    let result = Command::new(&report.output)
        .output_when_launchable()
        .expect("run ffi cjs binary");
    assert!(
        result.status.success(),
        "process failed: stderr={}",
        String::from_utf8_lossy(&result.stderr)
    );
    let stdout = String::from_utf8_lossy(&result.stdout);
    assert!(stdout.contains("cjs-ffi: object function function string function true"));
}

#[test]
fn native_node_ffi_dlopen_calls_compiled_function() {
    use kiln_module_resolver::NodeRuntimeResolver;
    let Ok(toolchain) = Toolchain::discover() else {
        return;
    };
    let project = TestDirectory::new("kiln-native-node-ffi-operation");
    let source = project.path.join("add.c");
    fs::write(
        &source,
        "int kiln_test_add(int left, int right) { return left + right; }\n",
    )
    .expect("write ffi fixture");
    let object = project.path.join("add.o");
    toolchain
        .compile_c(&source, &object, &CompileOptions::default())
        .expect("compile ffi fixture");
    let library = project.path.join(if cfg!(target_os = "macos") {
        "libadd.dylib"
    } else if cfg!(target_os = "windows") {
        "add.dll"
    } else {
        "libadd.so"
    });
    toolchain
        .link_shared_library(&[object], &library, &LinkOptions::default())
        .expect("link ffi fixture");
    let entry = project.path.join("main.mjs");
    fs::write(
        &entry,
        format!(
            "import {{ createRequire }} from 'node:module';\nconst require = createRequire(import.meta.url);\nconst ffi = require('node:ffi');\nconst opened = ffi.dlopen({:?}, {{ kiln_test_add: {{ arguments: ['i32', 'i32'], return: 'i32' }} }});\nconsole.log(opened.functions.kiln_test_add(2, 40));\nopened.lib.close();\n",
            library.to_string_lossy()
        ),
    )
    .expect("write ffi operation entry");
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&project.path).expect("node resolver"),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("build ffi operation graph");
    let report = project
        .builder(toolchain)
        .build_graph(
            &graph,
            &NativeBuildOptions::new(
                project.path.join("ffi_operation_bin"),
                NativeProfile::Release,
            ),
        )
        .expect("build ffi operation binary");
    let result = Command::new(&report.output)
        .output_when_launchable()
        .expect("run ffi operation binary");
    assert!(
        result.status.success(),
        "ffi operation failed: stdout={} stderr={}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(result.stdout, b"42\n");
}
