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
    /// Same exports, different function bodies.
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
}

pub struct Workload {
    pub spec: &'static WorkloadSpec,
    pub modules: Vec<Module>,
    /// The unedited text of each module, i.e. `variant(i, _, 0)`.
    pub base_texts: Vec<String>,
    /// Module the warm-edit scenarios touch: the one with the most dependents, so the edit is
    /// the worst realistic case rather than a leaf nobody imports.
    pub edit_target: usize,
}

impl Workload {
    pub fn generate(spec: &'static WorkloadSpec) -> Workload {
        let modules = build_graph(spec);
        let base_texts = (0..modules.len())
            .map(|i| render(spec, &modules, i, 0, 0))
            .collect();
        let edit_target = pick_edit_target(&modules);
        Workload {
            spec,
            modules,
            base_texts,
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

    pub fn import_edges(&self) -> usize {
        self.modules.iter().map(|m| m.imports.len()).sum()
    }

    pub fn total_lines(&self) -> usize {
        self.base_texts.iter().map(|t| t.lines().count()).sum()
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
        format!(
            "kiln-bench-workload-v1 {:016x} {}",
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

/// Render one module.
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

    /// Approximates whatever the frontend will do, well enough to police the fixtures.
    fn exports(text: &str) -> Vec<&str> {
        text.lines().filter(|l| l.starts_with("export ")).collect()
    }
}
