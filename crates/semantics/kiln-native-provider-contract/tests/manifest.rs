use kiln_native_provider_contract::{
    ProviderCapabilities, ProviderCapability, ProviderManifest, ProviderVariant, manifest,
};
use serde_json::Value;
use std::process::Command;

fn sample(variant: ProviderVariant) -> ProviderManifest {
    manifest(
        "aarch64-apple-darwin".to_owned(),
        "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_owned(),
        "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".to_owned(),
        "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc".to_owned(),
        "deployment".to_owned(),
        "libc".to_owned(),
        variant,
    )
}

#[test]
fn full_and_common_manifests_round_trip_and_match_source_identity() {
    for variant in ProviderVariant::ALL {
        let source = sample(variant);
        let encoded = serde_json::to_string(&source).expect("serialize manifest");
        let installed: ProviderManifest =
            serde_json::from_str(&encoded).expect("deserialize installed manifest");
        assert_eq!(source.identity(), installed.identity());
        assert_eq!(source.variant, variant);
        assert_eq!(
            ProviderCapabilities::from_capabilities(source.capabilities.iter().copied()),
            variant.capabilities()
        );
        assert_eq!(source.archive, variant.archive());
    }
}

#[test]
fn every_producer_identity_field_changes_identity_but_artifact_metadata_does_not() {
    let base = sample(ProviderVariant::Full);
    let fields = [
        ("schema", Value::from(base.schema + 1)),
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
        ("linkage", Value::from("dynamic")),
        ("pic", Value::from(false)),
        ("profile", Value::from("other")),
        ("cpu_features", Value::from("native")),
        ("deployment", Value::from("other")),
        ("libc_baseline", Value::from("other")),
        ("variant", Value::from("common")),
        (
            "capabilities",
            serde_json::to_value(ProviderCapabilities::COMMON.iter().collect::<Vec<_>>()).unwrap(),
        ),
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
    let mut rebuilt = base.clone();
    rebuilt.archive_sha256 =
        "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee".to_owned();
    assert_eq!(
        baseline,
        rebuilt.identity(),
        "archive content is an artifact input, not producer identity"
    );
}

#[test]
fn export_delimiters_cannot_collide_and_capabilities_are_required() {
    let mut comma = sample(ProviderVariant::Full);
    comma.exports = vec!["a,b".to_owned()];
    let mut split = sample(ProviderVariant::Full);
    split.exports = vec!["a".to_owned(), "b".to_owned()];
    assert_ne!(comma.identity(), split.identity());

    let mut json = serde_json::to_value(sample(ProviderVariant::Full)).expect("manifest value");
    json.as_object_mut().unwrap().remove("capabilities");
    assert!(serde_json::from_value::<ProviderManifest>(json).is_err());
}

#[test]
fn manifest_cli_rejects_invalid_variant_and_argument_counts() {
    let binary = env!("CARGO_BIN_EXE_kiln-native-provider-contract");
    let valid = ["target", "rustc", "source", "archive", "", "", "full"];
    let invalid_variant = Command::new(binary)
        .arg("manifest")
        .args(valid.iter().take(6).chain(std::iter::once(&"maybe")))
        .output()
        .expect("run manifest CLI");
    assert_eq!(invalid_variant.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&invalid_variant.stderr).contains("variant must be one of"));

    let wrong_count = Command::new(binary)
        .args(["manifest", "target"])
        .output()
        .expect("run manifest CLI");
    assert_eq!(wrong_count.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&wrong_count.stderr).contains("requires 7 arguments"));
}

#[test]
fn capability_spelling_is_stable_json() {
    assert_eq!(
        serde_json::to_string(&ProviderCapability::RegExp).unwrap(),
        "\"reg-exp\""
    );
    assert_eq!(
        serde_json::to_string(&ProviderCapability::BigInt).unwrap(),
        "\"big-int\""
    );
    assert_eq!(
        serde_json::to_string(&ProviderCapability::Gui).unwrap(),
        "\"gui\""
    );
}
