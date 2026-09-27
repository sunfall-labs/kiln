//! Realms issue 04 step 5: every mutable static the emitters write has an owner that is not a
//! realm (`kiln_backend_c::state_owners`). Realm state lives in the realm's blocks and graph
//! context, so a new kind of emitted static holding a realm's values fails here.

use super::*;
use kiln_module_resolver::NodeRuntimeResolver;

/// A program touching every emission path that declares statics: module globals read by
/// functions, template sites, classes and typed method calls (receiver rows, typed callees),
/// object literals and spreads (literal and spread shapes), private fields, a BigInt literal, a
/// regular expression, loops (speculation guards), generators, async functions, a dynamic import.
const PROGRAM: &str = r#"
let counter = 0;
const cache = new Map();
export function bump(n) { counter += n; cache.set(n, counter); return counter; }
function fib(n) { return n < 2 ? n : fib(n - 1) + fib(n - 2); }
class Point {
    #secret = 1;
    constructor(x, y) { this.x = x; this.y = y; }
    norm() { return Math.sqrt(this.x * this.x + this.y * this.y) + this.#secret; }
    get sum() { return this.x + this.y; }
}
class Point3 extends Point { constructor(x, y, z) { super(x, y); this.z = z; } norm() { return super.norm() + this.z; } }
const tag = (s, ...v) => s.raw.join("|") + v.join(",");
function* gen(n) { for (let i = 0; i < n; i++) yield i * counter; }
async function later() { await null; return counter + 1n.toString().length; }
let total = 0;
for (let i = 0; i < 100; i++) {
    const p = i % 2 ? new Point(i, 1) : new Point3(i, 2, 3);
    total += p.norm() + p.sum + fib(5);
    const o = { a: i, b: String(i), ...{ c: i } };
    total += o.a + o.b.length + o.c;
}
const big = 123456789012345678901234567890n;
const re = /a(b+)c/g;
console.log(total, tag`x${1}y`, [...gen(3)], big % 7n, "abbc".replace(re, "$1"));
later().then((v) => console.log(v));
if (counter > 1e9) import("./other.mjs").then((m) => console.log(m.value));
export default counter;
"#;

fn graph_for(project: &Path) -> ModuleGraph {
    fs::create_dir_all(project).expect("project");
    fs::write(project.join("main.mjs"), PROGRAM).expect("entry");
    fs::write(project.join("other.mjs"), "export const value = 1;\n").expect("other");
    fs::write(
        project.join("lib.cjs"),
        "let hits = 0; module.exports = { hit() { return ++hits; } };\n",
    )
    .expect("commonjs");
    fs::write(
        project.join("uses.mjs"),
        "import lib from './lib.cjs';\nimport { bump } from './main.mjs';\nconsole.log(lib.hit(), bump(1));\n",
    )
    .expect("importer");
    ModuleGraph::build_node(
        &project.join("uses.mjs"),
        NodeRuntimeResolver::new(project).expect("node resolver"),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("fixture compiles")
}

fn assert_owned(source: &str, symbols: &[&str], what: &str) {
    let problems = kiln_backend_c::state_owners::unowned_emitted_statics(source, symbols);
    assert!(
        problems.is_empty(),
        "{what} declares mutable statics without an owner -- realm state belongs in the realm's \
         block or graph context, not a static (ADR 0070; `kiln_backend_c::state_owners`):\n{}",
        problems.join("\n")
    );
}

#[test]
fn every_mutable_emitted_static_has_an_owner_that_is_not_a_realm() {
    let root = TestDirectory::new("kiln-native-emitted-state-owners");
    let graph = graph_for(&root.path.join("project"));
    let policies = [
        kiln_backend_c::EmissionPolicy {
            decoupled_prelude: true,
            ..kiln_backend_c::EmissionPolicy::default()
        },
        kiln_backend_c::EmissionPolicy {
            decoupled_prelude: true,
            compact: true,
            ..kiln_backend_c::EmissionPolicy::default()
        },
        kiln_backend_c::EmissionPolicy {
            decoupled_prelude: true,
            generic_only: true,
            ..kiln_backend_c::EmissionPolicy::default()
        },
    ];
    let mut checked = 0;
    for policy in policies {
        let modules = graph
            .modules()
            .map(|node| {
                (
                    node,
                    kiln_backend_c::emit_module_with_policy(&node.program, policy.clone())
                        .expect("module emits"),
                )
            })
            .collect::<Vec<_>>();
        for (_, module) in &modules {
            let symbols = [module.symbol.as_str(), "ku"];
            assert_owned(&module.source, &symbols, &format!("unit {}", module.symbol));
            checked += kiln_backend_c::state_owners::mutable_statics(&module.source).len();
        }
        let launcher = graph_launcher_source(&graph, &modules, &NativeRealmPolicy::default())
            .expect("launcher emits");
        assert_owned(&launcher, &[], "the graph launcher");
        for forbidden in ["kiln_namespaces[", "kiln_exports_", "kiln_imports_", "kiln_commonjs_"] {
            assert!(
                !kiln_backend_c::state_owners::mutable_statics(&launcher)
                    .iter()
                    .any(|found| found.declaration.contains(forbidden)),
                "the launcher's module linkage is its realm's graph context, not a static"
            );
        }
        let shared = crate::shared_graph::graph_shared_wrapper_source(&graph, &modules, "kiln_probe")
            .expect("shared wrapper emits");
        assert_owned(&shared, &["kiln_probe"], "the shared-library wrapper");
    }
    assert!(checked > 10, "the probe program exercises the emitters' statics ({checked})");
}
