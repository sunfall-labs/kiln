//! Deterministic reference workloads for the timing harness (design.md §13).
//!
//! Generated rather than committed. A few hundred hand-written fixture modules would rot, and
//! benchmark numbers are only comparable across runs if the input is bit-identical - so the
//! generator is a pure function of a fixed seed, and the copy materialised under `examples/`
//! carries a manifest so a run can tell that it no longer matches and rewrite it.

use std::fs;
use std::path::Path;

/// Which of a module's two variants an edit swaps in.
///
/// The gap between these two is the entire thesis of the design: an implementation edit must
/// stop at the module's interface, an interface edit must not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditKind {
    /// Same exports, different implementation expressions.
    Implementation,
    /// Same bodies, different export set.
    Interface,
}

impl EditKind {
    pub fn label(self) -> &'static str {
        match self {
            EditKind::Implementation => "implementation edit",
            EditKind::Interface => "interface edit",
        }
    }

    pub fn slug(self) -> &'static str {
        match self {
            EditKind::Implementation => "warm-impl-edit",
            EditKind::Interface => "warm-interface-edit",
        }
    }
}

pub struct WorkloadSpec {
    pub name: &'static str,
    pub modules: usize,
    /// Depth of the import DAG. Layer 0 has no imports; every later layer imports from earlier
    /// ones, which is what gives the graph fan-in rather than a flat pile of files.
    pub layers: usize,
    pub imports_per_module: usize,
    pub helpers_per_module: usize,
    pub seed: u64,
    /// Provisional warm-query budget from design.md §13. See `report::GOALS_NOTE`.
    pub warm_goal_ms: f64,
}

pub const SMALL: WorkloadSpec = WorkloadSpec {
    name: "small",
    modules: 8,
    layers: 3,
    imports_per_module: 2,
    helpers_per_module: 3,
    seed: 0x5EED_0001,
    warm_goal_ms: 200.0,
};

pub const MEDIUM: WorkloadSpec = WorkloadSpec {
    name: "medium",
    modules: 320,
    layers: 8,
    imports_per_module: 4,
    helpers_per_module: 6,
    seed: 0x5EED_0002,
    warm_goal_ms: 500.0,
};

pub const ALL: [&WorkloadSpec; 2] = [&SMALL, &MEDIUM];

pub fn spec_by_name(name: &str) -> Option<&'static WorkloadSpec> {
    ALL.iter().copied().find(|s| s.name == name)
}

/// Directory names a module can land in, so the generated tree reads like a real project
/// instead of one flat folder. Indexed by layer.
const AREAS: [&str; 8] = [
    "util", "model", "store", "codec", "service", "domain", "view", "app",
];

pub struct Module {
    pub stem: String,
    pub area: &'static str,
    pub layer: usize,
    /// Indices into [`Workload::modules`]. Always strictly less than this module's own index.
    pub imports: Vec<usize>,
}

impl Module {
    pub fn path(&self) -> String {
        format!("src/{}/{}.ts", self.area, self.stem)
    }

    fn export_alpha(&self) -> String {
        format!("{}_alpha", self.stem)
    }

    fn export_value(&self) -> String {
        format!("{}_value", self.stem)
    }
}

pub struct Workload {
    pub spec: &'static WorkloadSpec,
    pub modules: Vec<Module>,
    /// The unedited text of each module, i.e. `variant(i, _, 0)`.
    pub base_texts: Vec<String>,
    /// Executable scalar counterparts used only by linked-module scenarios.
    pub linked_base_texts: Vec<String>,
    /// Module the warm-edit scenarios touch: the one with the most dependents, so the edit is
    /// the worst realistic case rather than a leaf nobody imports.
    pub edit_target: usize,
}

const ENTRY_FILE: &str = "entry.ts";
const LINKED_DIR: &str = "linked";

impl Workload {
    pub fn generate(spec: &'static WorkloadSpec) -> Workload {
        let modules = build_graph(spec);
        let base_texts = (0..modules.len())
            .map(|i| render(spec, &modules, i, 0, 0))
            .collect();
        let linked_base_texts = (0..modules.len())
            .map(|i| render_linked(spec, &modules, i, 0))
            .collect();
        let edit_target = pick_edit_target(&modules);
        Workload {
            spec,
            modules,
            base_texts,
            linked_base_texts,
            edit_target,
        }
    }

    /// The text of module `idx` under edit `kind`, revision `n`.
    ///
    /// `n == 0` is the base text for both kinds. Callers alternate between `n == 1` and
    /// `n == 2` so that every iteration of a warm loop is a genuine edit in both directions,
    /// rather than a first edit followed by a series of no-ops.
    pub fn variant(&self, idx: usize, kind: EditKind, n: u32) -> String {
        match kind {
            EditKind::Implementation => render(self.spec, &self.modules, idx, n, 0),
            EditKind::Interface => render(self.spec, &self.modules, idx, 0, n),
        }
    }

    pub fn linked_variant(&self, idx: usize, n: u32) -> String {
        render_linked(self.spec, &self.modules, idx, n)
    }

    pub fn import_edges(&self) -> usize {
        self.modules.iter().map(|m| m.imports.len()).sum()
    }

    pub fn total_lines(&self) -> usize {
        self.base_texts.iter().map(|t| t.lines().count()).sum()
    }

    /// The synthetic entry imports every generated module so linked benchmarks cover the whole
    /// generated workload, not an accidental entry-rooted subset of the random DAG.
    pub fn entry_path(&self) -> &'static str {
        ENTRY_FILE
    }

    pub fn linked_dir(&self) -> &'static str {
        LINKED_DIR
    }

    pub fn entry_text(&self) -> String {
        render_entry(self.spec, &self.modules)
    }

    /// Number of modules that import [`Workload::edit_target`], directly.
    pub fn edit_target_dependents(&self) -> usize {
        self.modules
            .iter()
            .filter(|m| m.imports.contains(&self.edit_target))
            .count()
    }

    /// Materialise the workload under `root/<name>/`, skipping the write when what is already
    /// there was produced by this exact generator.
    ///
    /// Returns whether anything was written.
    pub fn materialise(&self, root: &Path, force: bool) -> Result<bool, String> {
        let dir = root.join(self.spec.name);
        let manifest = dir.join(MANIFEST_FILE);
        let want = self.fingerprint();

        if !force && fs::read_to_string(&manifest).is_ok_and(|got| got.trim() == want) {
            return Ok(false);
        }

        if dir.exists() {
            // Only ever delete a directory this generator owns. A directory that exists but
            // has no manifest is somebody else's, and blowing it away would be rude at best.
            if !manifest.exists() && dir.read_dir().is_ok_and(|mut d| d.next().is_some()) {
                return Err(format!(
                    "{} exists, is not empty, and has no {MANIFEST_FILE}; refusing to overwrite it",
                    dir.display()
                ));
            }
            fs::remove_dir_all(&dir).map_err(|e| format!("removing {}: {e}", dir.display()))?;
        }

        for (module, text) in self.modules.iter().zip(&self.base_texts) {
            let file = dir.join(module.path());
            let parent = file
                .parent()
                .ok_or_else(|| format!("{} has no parent", file.display()))?;
            fs::create_dir_all(parent)
                .map_err(|e| format!("creating {}: {e}", parent.display()))?;
            fs::write(&file, text).map_err(|e| format!("writing {}: {e}", file.display()))?;
        }
        for (module, text) in self.modules.iter().zip(&self.linked_base_texts) {
            let file = dir.join(self.linked_dir()).join(module.path());
            let parent = file
                .parent()
                .ok_or_else(|| format!("{} has no parent", file.display()))?;
            fs::create_dir_all(parent)
                .map_err(|e| format!("creating {}: {e}", parent.display()))?;
            fs::write(&file, text).map_err(|e| format!("writing {}: {e}", file.display()))?;
        }
        let entry = dir.join(self.linked_dir()).join(self.entry_path());
        fs::write(&entry, self.entry_text())
            .map_err(|e| format!("writing {}: {e}", entry.display()))?;
        fs::write(&manifest, format!("{want}\n"))
            .map_err(|e| format!("writing {}: {e}", manifest.display()))?;
        Ok(true)
    }

    fn fingerprint(&self) -> String {
        let mut h = Fnv::new();
        h.write(self.spec.name.as_bytes());
        h.write(&self.spec.seed.to_le_bytes());
        for (module, text) in self.modules.iter().zip(&self.base_texts) {
            h.write(module.path().as_bytes());
            h.write(text.as_bytes());
        }
        h.write(self.linked_dir().as_bytes());
        for (module, text) in self.modules.iter().zip(&self.linked_base_texts) {
            h.write(module.path().as_bytes());
            h.write(text.as_bytes());
        }
        h.write(self.entry_path().as_bytes());
        h.write(self.entry_text().as_bytes());
        format!(
            "kiln-bench-workload-v2 {:016x} {}",
            h.finish(),
            self.modules.len()
        )
    }
}

const MANIFEST_FILE: &str = ".kiln-bench-manifest";

fn build_graph(spec: &WorkloadSpec) -> Vec<Module> {
    assert!(
        spec.layers > 0 && spec.modules >= spec.layers,
        "spec {} is degenerate",
        spec.name
    );
    let mut rng = Rng::new(spec.seed);
    let mut modules: Vec<Module> = Vec::with_capacity(spec.modules);
    // Start index of each layer, so a module can be pointed at a specific earlier layer.
    let mut layer_starts = Vec::with_capacity(spec.layers);

    for layer in 0..spec.layers {
        layer_starts.push(modules.len());
        let end = (layer + 1) * spec.modules / spec.layers;
        while modules.len() < end {
            let idx = modules.len();
            let imports = if layer == 0 {
                Vec::new()
            } else {
                pick_imports(&mut rng, spec, layer, &layer_starts, idx)
            };
            modules.push(Module {
                stem: format!("m{idx:04}"),
                area: AREAS[layer % AREAS.len()],
                layer,
                imports,
            });
        }
    }
    modules
}

fn pick_imports(
    rng: &mut Rng,
    spec: &WorkloadSpec,
    layer: usize,
    layer_starts: &[usize],
    idx: usize,
) -> Vec<usize> {
    let prev_start = layer_starts[layer - 1];
    let mut imports = Vec::with_capacity(spec.imports_per_module);
    for _ in 0..spec.imports_per_module {
        // Mostly depend on the layer immediately below, occasionally reach further down. Real
        // import graphs look like this, and it gives deep modules a long dependent chain.
        let dep = if rng.below(10) < 7 {
            prev_start + rng.below(idx - prev_start)
        } else {
            rng.below(idx)
        };
        if !imports.contains(&dep) {
            imports.push(dep);
        }
    }
    imports.sort_unstable();
    imports
}

fn pick_edit_target(modules: &[Module]) -> usize {
    let mut dependents = vec![0usize; modules.len()];
    for m in modules {
        for &d in &m.imports {
            dependents[d] += 1;
        }
    }
    // max_by_key returns the *last* maximum; reverse the scan so ties resolve to the lowest
    // index, which keeps the choice stable if the graph generator ever changes shape slightly.
    dependents
        .iter()
        .enumerate()
        .rev()
        .max_by_key(|&(_, &n)| n)
        .map(|(i, _)| i)
        .unwrap_or(0)
}

/// Render one analysis workload module.
///
/// `body` perturbs a numeric literal inside the exported function without touching any name or
/// the line count; `extra` appends one additional exported binding. That split is what makes
/// the two warm scenarios comparable: identical work for the parser, different answers for the
/// interface query.
fn render(spec: &WorkloadSpec, modules: &[Module], idx: usize, body: u32, extra: u32) -> String {
    let m = &modules[idx];
    let mut s = String::with_capacity(1536);

    s.push_str(&format!(
        "// Generated by kiln-bench for the \"{}\" workload (seed {:#x}). Do not edit by hand.\n",
        spec.name, spec.seed
    ));
    for &dep in &m.imports {
        let d = &modules[dep];
        s.push_str(&format!(
            "import {{ {} }} from \"../{}/{}\";\n",
            d.export_alpha(),
            d.area,
            d.stem
        ));
    }
    s.push_str(&format!("\nconst SEED = {};\n\n", 17 + idx % 97));
    for h in 0..spec.helpers_per_module {
        s.push_str(&format!(
            "function {stem}_step{h}(x: number): number {{\n  \
             return (x * {mul} + SEED) % 1009;\n}}\n\n",
            stem = m.stem,
            mul = 3 + h,
        ));
    }

    s.push_str(&format!(
        "export function {}(x: number): number {{\n  let acc = x + {};\n",
        m.export_alpha(),
        7 + body,
    ));
    for h in 0..spec.helpers_per_module {
        s.push_str(&format!("  acc = {}_step{h}(acc);\n", m.stem));
    }
    for &dep in &m.imports {
        s.push_str(&format!("  acc = {}(acc);\n", modules[dep].export_alpha()));
    }
    s.push_str("  return acc;\n}\n\n");

    s.push_str(&format!(
        "export const {stem}_beta: number = {n};\n\n\
         export interface {stem}Shape {{\n  id: number;\n  label: string;\n}}\n",
        stem = m.stem,
        n = 13 + m.layer,
    ));

    if extra > 0 {
        s.push_str(&format!(
            "\nexport const {}_ext{extra}: number = {extra};\n",
            m.stem
        ));
    }
    s
}

/// Render the scalar counterpart used by linked execution. Explicit extensions are intentional:
/// the exact resolver does not perform Node-style extension guessing.
fn render_linked(spec: &WorkloadSpec, modules: &[Module], idx: usize, body: u32) -> String {
    let m = &modules[idx];
    let mut s = String::with_capacity(1024);
    s.push_str(&format!(
        "// Generated linked fixture for the \"{}\" workload (seed {:#x}).\n",
        spec.name, spec.seed
    ));
    for &dep in &m.imports {
        let d = &modules[dep];
        s.push_str(&format!(
            "import {{ {} }} from \"../{}/{}.ts\";\n",
            d.export_value(),
            d.area,
            d.stem
        ));
    }
    for h in 0..spec.helpers_per_module {
        s.push_str(&format!(
            "const {stem}_step{h}: number = {value};\n",
            stem = m.stem,
            value = 3 + h,
        ));
    }
    let dependency_sum = m
        .imports
        .iter()
        .map(|&dep| modules[dep].export_value())
        .collect::<Vec<_>>()
        .join(" + ");
    let dependency_sum = if dependency_sum.is_empty() {
        "0".to_owned()
    } else {
        dependency_sum
    };
    let helper_sum = (0..spec.helpers_per_module)
        .map(|h| format!("{}_step{h}", m.stem))
        .collect::<Vec<_>>()
        .join(" + ");
    let helper_sum = if helper_sum.is_empty() {
        "0".to_owned()
    } else {
        helper_sum
    };
    s.push_str(&format!(
        "export const {value}: number = ({seed} + {body} + {helper_sum} + {dependency_sum}) % 1009;\n",
        value = m.export_value(),
        seed = 17 + idx % 97,
    ));
    s
}

/// Render the synthetic root used by linked-module benchmarks. It imports every module directly,
/// making the entry-rooted graph include every generated node even when the random DAG has
/// multiple branches. All imported values participate in the calculation, while subtracting the
/// total from itself keeps expected stdout stable across implementation edits.
fn render_entry(spec: &WorkloadSpec, modules: &[Module]) -> String {
    let mut s = String::with_capacity(modules.len() * 96);
    s.push_str(&format!(
        "// Generated by kiln-bench for the \"{}\" workload (seed {:#x}). Do not edit by hand.\n",
        spec.name, spec.seed
    ));
    for module in modules {
        s.push_str(&format!(
            "import {{ {} }} from \"./{}\";\n",
            module.export_value(),
            module.path()
        ));
    }
    let values = modules
        .iter()
        .map(Module::export_value)
        .collect::<Vec<_>>()
        .join(" + ");
    s.push_str(&format!(
        "\nconst linked_total = {values};\nconsole.log(\"linked\", linked_total - linked_total);\n"
    ));
    s
}

/// splitmix64. Enough for shaping a fixture graph, and stable across platforms and Rust
/// versions - which `HashMap`'s hasher and the `rand` crate's generators are not.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Rng {
        Rng(seed)
    }

    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform-ish in `0..n`. The modulo bias is irrelevant here: the seed is fixed, so what
    /// matters is that the sequence is identical on every run, not that it is unbiased.
    fn below(&mut self, n: usize) -> usize {
        (self.next_u64() % n as u64) as usize
    }
}

/// FNV-1a, used only for the workload manifest.
struct Fnv(u64);

impl Fnv {
    fn new() -> Fnv {
        Fnv(0xcbf2_9ce4_8422_2325)
    }

    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.0 = (self.0 ^ b as u64).wrapping_mul(0x1000_0000_01b3);
        }
    }

    fn finish(&self) -> u64 {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generation_is_deterministic() {
        let a = Workload::generate(&MEDIUM);
        let b = Workload::generate(&MEDIUM);
        assert_eq!(a.base_texts, b.base_texts);
        assert_eq!(a.edit_target, b.edit_target);
        assert_eq!(a.fingerprint(), b.fingerprint());
    }

    #[test]
    fn analysis_workload_shape_stays_baseline_compatible() {
        assert_eq!(Workload::generate(&SMALL).total_lines(), 262);
        assert_eq!(Workload::generate(&MEDIUM).total_lines(), 16_600);
    }

    #[test]
    fn import_graph_is_acyclic_and_connected_downward() {
        for spec in ALL {
            let w = Workload::generate(spec);
            assert_eq!(w.modules.len(), spec.modules, "{}", spec.name);
            for (i, m) in w.modules.iter().enumerate() {
                for &dep in &m.imports {
                    // Imports only ever point at strictly earlier modules, so the graph cannot
                    // contain a cycle no matter how the layer weighting is tuned.
                    assert!(dep < i, "{} module {i} imports {dep}", spec.name);
                }
                assert_eq!(
                    m.imports.is_empty(),
                    m.layer == 0,
                    "{} module {i}",
                    spec.name
                );
            }
            assert!(
                w.import_edges() > spec.modules,
                "{} graph is too sparse",
                spec.name
            );
        }
    }

    #[test]
    fn edit_target_has_dependents() {
        let w = Workload::generate(&MEDIUM);
        assert!(
            w.edit_target_dependents() > 1,
            "warm-edit target should be a module others import"
        );
    }

    #[test]
    fn generated_runtime_imports_use_explicit_extensions() {
        let w = Workload::generate(&SMALL);
        for text in &w.linked_base_texts {
            assert!(
                !text.contains("function "),
                "linked workload must stay within the scalar execution subset"
            );
            for line in text.lines().filter(|line| line.starts_with("import ")) {
                assert!(
                    line.contains(".ts\";"),
                    "resolver requires explicit source extensions: {line}"
                );
            }
        }
    }

    #[test]
    fn synthetic_entry_reaches_every_generated_module() {
        let w = Workload::generate(&SMALL);
        let entry = w.entry_text();
        assert_eq!(
            entry
                .lines()
                .filter(|line| line.starts_with("import "))
                .count(),
            w.modules.len()
        );
        for module in &w.modules {
            assert!(
                entry.contains(&format!("./{}", module.path())),
                "entry is missing {}",
                module.path()
            );
        }
        assert!(entry.contains("linked_total - linked_total"));
    }

    #[test]
    fn implementation_variants_preserve_the_export_set() {
        let w = Workload::generate(&SMALL);
        let idx = w.edit_target;
        let a = w.variant(idx, EditKind::Implementation, 1);
        let b = w.variant(idx, EditKind::Implementation, 2);
        assert_ne!(a, b, "an implementation edit must change the text");
        assert_eq!(exports(&a), exports(&b), "it must not change the exports");
        assert_eq!(a.lines().count(), b.lines().count(), "nor the line count");
    }

    #[test]
    fn linked_implementation_variants_preserve_the_export_set() {
        let w = Workload::generate(&SMALL);
        let idx = w.edit_target;
        let a = w.linked_variant(idx, 1);
        let b = w.linked_variant(idx, 2);
        assert_ne!(a, b);
        assert_eq!(exports(&a), exports(&b));
        assert_eq!(a.lines().count(), b.lines().count());
    }

    #[test]
    fn interface_variants_change_the_export_set() {
        let w = Workload::generate(&SMALL);
        let idx = w.edit_target;
        let a = w.variant(idx, EditKind::Interface, 1);
        let b = w.variant(idx, EditKind::Interface, 2);
        assert_ne!(exports(&a), exports(&b));
        // Both variants add exactly one export, so the two directions of the alternating warm
        // loop are the same size of change.
        let base = exports(&w.base_texts[idx]).len();
        assert_eq!(exports(&a).len(), base + 1);
        assert_eq!(exports(&b).len(), base + 1);
    }

    /// Approximates exported declaration identities, ignoring initializer/body text.
    fn exports(text: &str) -> Vec<String> {
        text.lines()
            .filter(|line| line.starts_with("export "))
            .map(|line| {
                line.split_whitespace()
                    .take(3)
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .collect()
    }
}
