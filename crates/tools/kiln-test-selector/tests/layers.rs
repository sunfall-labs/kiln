use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Layer {
    name: &'static str,
    rank: u8,
}

fn layer(directory: &str) -> Option<Layer> {
    let (name, rank) = match directory {
        "foundation" => ("foundation", 0),
        "semantics" => ("semantics", 1),
        "syntax" => ("syntax", 2),
        "host" => ("host", 2),
        "compile" => ("compile", 3),
        "interp" => ("interp", 4),
        "native" => ("native", 4),
        "tools" => ("tools", 5),
        _ => return None,
    };
    Some(Layer { name, rank })
}

fn permitted(source: Layer, target: Layer) -> bool {
    target.rank <= source.rank
        && !matches!(
            (source.name, target.name),
            ("interp", "native") | ("native", "interp")
        )
}

#[test]
fn layers_allow_downward_and_peer_edges_but_separate_engines() {
    assert!(permitted(layer("tools").unwrap(), layer("native").unwrap()));
    assert!(permitted(
        layer("compile").unwrap(),
        layer("compile").unwrap()
    ));
    assert!(permitted(layer("host").unwrap(), layer("syntax").unwrap()));
    assert!(!permitted(
        layer("compile").unwrap(),
        layer("native").unwrap()
    ));
    assert!(!permitted(
        layer("interp").unwrap(),
        layer("native").unwrap()
    ));
    assert!(!permitted(
        layer("native").unwrap(),
        layer("interp").unwrap()
    ));
}

#[test]
fn workspace_dependencies_follow_adr_0066() {
    let output = Command::new("mbx")
        .args([
            "metadata",
            "--offline",
            "--no-deps",
            "--format-version",
            "1",
        ])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("run workspace metadata through mbx");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let metadata: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("Cargo metadata");
    let root = Path::new(metadata["workspace_root"].as_str().unwrap());
    let members = metadata["workspace_members"].as_array().unwrap();
    let packages: Vec<_> = metadata["packages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|package| members.contains(&package["id"]))
        .collect();
    let mut directories = HashMap::new();
    for package in &packages {
        let name = package["name"].as_str().unwrap();
        let directory = Path::new(package["manifest_path"].as_str().unwrap())
            .parent()
            .unwrap();
        let relative = directory
            .strip_prefix(root.join("crates"))
            .unwrap_or_else(|_| panic!("ADR 0066: {name} must declare its layer under crates/"));
        let layer_name = relative
            .components()
            .next()
            .unwrap()
            .as_os_str()
            .to_str()
            .unwrap();
        let declared = layer(layer_name)
            .unwrap_or_else(|| panic!("ADR 0066: {name} has unknown layer {layer_name}"));
        directories.insert(directory.to_path_buf(), (name, declared));
    }
    let mut violations = Vec::new();
    for package in packages {
        let source_path = Path::new(package["manifest_path"].as_str().unwrap())
            .parent()
            .unwrap();
        let (source_name, source_layer) = directories[source_path];
        for dependency in package["dependencies"].as_array().unwrap() {
            if !dependency["kind"].is_null() {
                continue;
            }
            let Some(path) = dependency["path"].as_str() else {
                continue;
            };
            let target_path = PathBuf::from(path)
                .canonicalize()
                .expect("dependency directory");
            let Some(&(target_name, target_layer)) = directories.get(&target_path) else {
                continue;
            };
            if !permitted(source_layer, target_layer) {
                violations.push(format!(
                    "{source_name} ({}) -> {target_name} ({})",
                    source_layer.name, target_layer.name
                ));
            }
        }
    }
    violations.sort();
    assert!(
        violations.is_empty(),
        "ADR 0066 dependency violations:\n{}",
        violations.join("\n")
    );
}
