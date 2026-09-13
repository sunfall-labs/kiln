use kiln_native_provider_contract::{ProviderManifest, manifest};
use serde_json::Value;
use std::process::Command;

fn sample(http: bool) -> ProviderManifest {
    manifest(
        "aarch64-apple-darwin".to_owned(),
        "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_owned(),
        "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".to_owned(),
        "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc".to_owned(),
        "deployment".to_owned(),
        "libc".to_owned(),
        http,
    )
}

#[test]
fn full_and_slim_manifests_round_trip_and_match_source_identity() {
    for http in [true, false] {
        let source = sample(http);
        let encoded = serde_json::to_string(&source).expect("serialize manifest");
        let installed: ProviderManifest =
            serde_json::from_str(&encoded).expect("deserialize installed manifest");
        assert_eq!(source.identity(), installed.identity());
        assert_eq!(source.http, http);
        assert_eq!(
            source.archive,
            if http {
                "libkiln_native_number_ffi.a"
            } else {
                "libkiln_native_number_ffi.slim.a"
            }
        );
    }
}

#[test]
fn every_identity_field_changes_identity_but_archive_filename_does_not() {
    let base = sample(true);
    let fields = [
        ("schema", Value::from(2)),
        ("provider", Value::from("other")),
        ("provider_abi", Value::from(2)),
        ("format_version", Value::from(2)),
        ("parse_version", Value::from(2)),
        ("rust_target", Value::from("other")),
        (
            "rustc_identity",
            Value::from("sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"),
        ),
        (
            "source_sha256",
            Value::from("dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd"),
        ),
        (
            "archive_sha256",
            Value::from("eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee"),
        ),
        ("linkage", Value::from("dynamic")),
        ("pic", Value::from(false)),
        ("profile", Value::from("other")),
        ("cpu_features", Value::from("native")),
        ("deployment", Value::from("other")),
        ("libc_baseline", Value::from("other")),
        ("http", Value::from(false)),
    ];
    let baseline = base.identity();
    for (field, value) in fields {
        let mut json = serde_json::to_value(&base).expect("manifest value");
        json[field] = value;
        let changed: ProviderManifest = serde_json::from_value(json).expect("changed manifest");
        assert_ne!(baseline, changed.identity(), "{field} must affect identity");
    }

    let mut renamed = base.clone();
    renamed.archive = "different-name.a".to_owned();
    assert_eq!(
        baseline,
        renamed.identity(),
        "archive filename is packaging metadata"
    );
}

#[test]
fn export_delimiters_cannot_collide_and_http_is_required() {
    let mut comma = sample(true);
    comma.exports = vec!["a,b".to_owned()];
    let mut split = sample(true);
    split.exports = vec!["a".to_owned(), "b".to_owned()];
    assert_ne!(comma.identity(), split.identity());

    let mut json = serde_json::to_value(sample(true)).expect("manifest value");
    json.as_object_mut().unwrap().remove("http");
    assert!(serde_json::from_value::<ProviderManifest>(json).is_err());
}

#[test]
fn manifest_cli_rejects_invalid_http_and_argument_counts() {
    let binary = env!("CARGO_BIN_EXE_kiln-native-provider-contract");
    let valid = ["target", "rustc", "source", "archive", "", "", "true"];
    let invalid_http = Command::new(binary)
        .arg("manifest")
        .args(valid.iter().take(6).chain(std::iter::once(&"maybe")))
        .output()
        .expect("run manifest CLI");
    assert_eq!(invalid_http.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&invalid_http.stderr).contains("http must be true or false"));

    let wrong_count = Command::new(binary)
        .args(["manifest", "target"])
        .output()
        .expect("run manifest CLI");
    assert_eq!(wrong_count.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&wrong_count.stderr).contains("requires 7 arguments"));
}
