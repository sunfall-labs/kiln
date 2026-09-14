use std::path::Path;

use kiln_digest::Digest;
use serde::{Deserialize, Serialize};

pub const SOURCE_LIST: &str = include_str!("../../kiln-native/provider-sources.txt");
// Schema 3 replaces the overloaded `http` variant bit with an explicit, canonical capability
// list. Old compilers must reject it rather than guessing which engines an archive contains.
pub const SCHEMA_VERSION: u16 = 3;
pub const PROVIDER_NAME: &str = "kiln-native-number-ffi";
pub const PROFILE: &str = "provider";
pub const RUSTFLAGS: &str =
    "-C panic=unwind -C relocation-model=pic -C target-cpu=generic -C debuginfo=0";
pub const FULL_FEATURES: &str = "regexp,bigint,temporal,json,intl,parallel,ffi";
pub const SLIM_FEATURES: &str = "realm,regexp,bigint,json";
pub const FULL_ARCHIVE: &str = "libkiln_native_number_ffi.a";
pub const SLIM_ARCHIVE: &str = "libkiln_native_number_ffi.slim.a";
pub const FULL_MANIFEST: &str = "kiln-native-number-provider.json";
pub const SLIM_MANIFEST: &str = "kiln-native-number-provider.slim.json";

/// One independently demandable engine behind the private provider ABI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProviderCapability {
    Realm,
    RegExp,
    BigInt,
    Json,
    Temporal,
    Intl,
    Parallel,
    Http,
    Ffi,
}

impl ProviderCapability {
    pub const ALL: [Self; 9] = [
        Self::Realm,
        Self::RegExp,
        Self::BigInt,
        Self::Json,
        Self::Temporal,
        Self::Intl,
        Self::Parallel,
        Self::Http,
        Self::Ffi,
    ];

    const fn bit(self) -> u16 {
        1 << self as u16
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Realm => "realm",
            Self::RegExp => "reg-exp",
            Self::BigInt => "big-int",
            Self::Json => "json",
            Self::Temporal => "temporal",
            Self::Intl => "intl",
            Self::Parallel => "parallel",
            Self::Http => "http",
            Self::Ffi => "ffi",
        }
    }
}

/// A canonical set of provider engines required by a retained program or supplied by an archive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct ProviderCapabilities(u16);

impl ProviderCapabilities {
    pub const NONE: Self = Self(0);
    pub const COMMON: Self = Self(
        ProviderCapability::Realm.bit()
            | ProviderCapability::RegExp.bit()
            | ProviderCapability::BigInt.bit()
            | ProviderCapability::Json.bit(),
    );
    pub const FULL: Self = Self(
        Self::COMMON.0
            | ProviderCapability::Temporal.bit()
            | ProviderCapability::Intl.bit()
            | ProviderCapability::Parallel.bit()
            | ProviderCapability::Http.bit()
            | ProviderCapability::Ffi.bit(),
    );

    pub const fn with(self, capability: ProviderCapability) -> Self {
        Self(self.0 | capability.bit())
    }

    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    pub const fn contains(self, capability: ProviderCapability) -> bool {
        self.0 & capability.bit() != 0
    }

    pub const fn contains_all(self, required: Self) -> bool {
        self.0 & required.0 == required.0
    }

    pub fn iter(self) -> impl Iterator<Item = ProviderCapability> {
        ProviderCapability::ALL
            .into_iter()
            .filter(move |capability| self.contains(*capability))
    }

    pub fn from_capabilities(capabilities: impl IntoIterator<Item = ProviderCapability>) -> Self {
        capabilities.into_iter().fold(Self::NONE, Self::with)
    }

    pub fn names(self) -> Vec<&'static str> {
        self.iter().map(ProviderCapability::as_str).collect()
    }
}

/// The two provider packages currently shipped. Selection is by capability coverage, so adding
/// finer-grained variants later does not change the planner or manifest semantics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProviderVariant {
    Common,
    Full,
}

impl ProviderVariant {
    pub const ALL: [Self; 2] = [Self::Common, Self::Full];

    pub const fn capabilities(self) -> ProviderCapabilities {
        match self {
            Self::Common => ProviderCapabilities::COMMON,
            Self::Full => ProviderCapabilities::FULL,
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Common => "common",
            Self::Full => "full",
        }
    }

    pub fn smallest_covering(required: ProviderCapabilities) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|variant| variant.capabilities().contains_all(required))
    }
}

#[derive(Debug, Serialize)]
pub struct Contract {
    pub schema: u16,
    pub provider: &'static str,
    pub provider_abi: u32,
    pub format_version: u32,
    pub parse_version: u32,
    pub profile: &'static str,
    pub rustflags: &'static str,
    pub full_features: &'static str,
    pub slim_features: &'static str,
    pub full_archive: &'static str,
    pub slim_archive: &'static str,
    pub full_manifest: &'static str,
    pub slim_manifest: &'static str,
    pub full_capabilities: Vec<ProviderCapability>,
    pub common_capabilities: Vec<ProviderCapability>,
    pub exports: Vec<&'static str>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderManifest {
    pub schema: u16,
    pub provider: String,
    pub provider_abi: u32,
    pub format_version: u32,
    pub parse_version: u32,
    pub rust_target: String,
    pub rustc_identity: String,
    pub source_sha256: String,
    pub archive_sha256: String,
    pub archive: String,
    pub linkage: String,
    pub pic: bool,
    pub profile: String,
    pub cpu_features: String,
    pub deployment: String,
    pub libc_baseline: String,
    pub exports: Vec<String>,
    pub variant: ProviderVariant,
    pub capabilities: Vec<ProviderCapability>,
}

impl ProviderManifest {
    pub fn identity(&self) -> String {
        #[derive(Serialize)]
        struct Identity<'a> {
            schema: u16,
            provider: &'a str,
            abi: u32,
            format: u32,
            parse: u32,
            target: &'a str,
            rustc: &'a str,
            source: &'a str,
            archive: &'a str,
            linkage: &'a str,
            pic: bool,
            profile: &'a str,
            cpu: &'a str,
            variant: ProviderVariant,
            capabilities: &'a [ProviderCapability],
            deployment: &'a str,
            libc: &'a str,
            exports: &'a [String],
        }
        serde_json::to_string(&Identity {
            schema: self.schema,
            provider: &self.provider,
            abi: self.provider_abi,
            format: self.format_version,
            parse: self.parse_version,
            target: &self.rust_target,
            rustc: &self.rustc_identity,
            source: &self.source_sha256,
            archive: &self.archive_sha256,
            linkage: &self.linkage,
            pic: self.pic,
            profile: &self.profile,
            cpu: &self.cpu_features,
            variant: self.variant,
            capabilities: &self.capabilities,
            deployment: &self.deployment,
            libc: &self.libc_baseline,
            exports: &self.exports,
        })
        .expect("provider identity serialization is infallible")
    }
}

pub fn manifest(
    target: String,
    rustc: String,
    source: String,
    archive: String,
    deployment: String,
    libc: String,
    variant: ProviderVariant,
) -> ProviderManifest {
    let c = contract();
    ProviderManifest {
        schema: c.schema,
        provider: c.provider.to_owned(),
        provider_abi: c.provider_abi,
        format_version: c.format_version,
        parse_version: c.parse_version,
        rust_target: target,
        rustc_identity: rustc,
        source_sha256: source,
        archive_sha256: archive,
        archive: match variant {
            ProviderVariant::Full => c.full_archive,
            ProviderVariant::Common => c.slim_archive,
        }
        .to_owned(),
        linkage: "static".to_owned(),
        pic: true,
        profile: c.profile.to_owned(),
        cpu_features: "generic".to_owned(),
        deployment,
        libc_baseline: libc,
        exports: c.exports.into_iter().map(str::to_owned).collect(),
        variant,
        capabilities: variant.capabilities().iter().collect(),
    }
}

macro_rules! export_names {
    ($(($feature:ident, $name:ident, $sig:ident);)*) => { vec![$(stringify!($name)),*] };
}

pub fn exports() -> Vec<&'static str> {
    kiln_native_number_abi::provider_abi_registry!(export_names)
}

pub fn contract() -> Contract {
    Contract {
        schema: SCHEMA_VERSION,
        provider: PROVIDER_NAME,
        provider_abi: kiln_native_number_abi::PROVIDER_ABI_VERSION,
        format_version: kiln_native_number::NUMBER_FORMAT_VERSION,
        parse_version: kiln_native_number::NUMBER_PARSE_VERSION,
        profile: PROFILE,
        rustflags: RUSTFLAGS,
        full_features: FULL_FEATURES,
        slim_features: SLIM_FEATURES,
        full_archive: FULL_ARCHIVE,
        slim_archive: SLIM_ARCHIVE,
        full_manifest: FULL_MANIFEST,
        slim_manifest: SLIM_MANIFEST,
        full_capabilities: ProviderCapabilities::FULL.iter().collect(),
        common_capabilities: ProviderCapabilities::COMMON.iter().collect(),
        exports: exports(),
    }
}

pub fn source_paths() -> impl Iterator<Item = &'static str> {
    SOURCE_LIST
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
}

pub fn source_digest(root: &Path) -> std::io::Result<Digest> {
    digest_paths(root, source_paths())
}

fn digest_paths<'a>(
    root: &Path,
    paths: impl IntoIterator<Item = &'a str>,
) -> std::io::Result<Digest> {
    let mut bytes = Vec::new();
    for relative in paths {
        let path = root.join(relative);
        let contents = std::fs::read(&path)?;
        bytes.extend_from_slice(relative.as_bytes());
        bytes.push(0);
        bytes.extend_from_slice(contents.len().to_string().as_bytes());
        bytes.push(0);
        bytes.extend_from_slice(&contents);
    }
    Ok(Digest::hash(&bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn source_digest_changes_with_contents_and_order() {
        let root =
            std::env::temp_dir().join(format!("kiln-provider-contract-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("a"), b"one").unwrap();
        std::fs::write(root.join("b"), b"two").unwrap();
        let first = digest_paths(&root, ["a", "b"]).unwrap();
        assert_eq!(
            first,
            Digest::from_hex("d4fcad726c5fe31fb48171feca00869c5522b7f88db0b0fe379309ca927fab7f")
                .unwrap()
        );
        assert_ne!(first, digest_paths(&root, ["b", "a"]).unwrap());
        std::fs::write(root.join("a"), b"changed").unwrap();
        assert_ne!(first, digest_paths(&root, ["a", "b"]).unwrap());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn contract_keeps_full_and_common_variants_distinct() {
        let value = contract();
        assert_ne!(value.full_archive, value.slim_archive);
        assert_ne!(value.full_manifest, value.slim_manifest);
        assert_ne!(value.full_features, value.slim_features);
        assert!(
            ProviderCapabilities::from_capabilities(value.full_capabilities)
                .contains_all(ProviderCapabilities::FULL)
        );
        assert_eq!(
            ProviderCapabilities::from_capabilities(value.common_capabilities),
            ProviderCapabilities::COMMON
        );
        assert!(!value.exports.is_empty());
    }

    #[test]
    fn capability_sets_have_stable_canonical_order_and_coverage() {
        assert_eq!(
            ProviderCapabilities::COMMON.names(),
            ["realm", "reg-exp", "big-int", "json"]
        );
        assert!(ProviderCapabilities::FULL.contains_all(ProviderCapabilities::COMMON));
        assert!(!ProviderCapabilities::COMMON.contains(ProviderCapability::Http));
        assert!(ProviderCapabilities::FULL.contains(ProviderCapability::Http));
        assert_eq!(
            ProviderVariant::smallest_covering(ProviderCapabilities::COMMON),
            Some(ProviderVariant::Common)
        );
        assert_eq!(
            ProviderVariant::smallest_covering(
                ProviderCapabilities::COMMON.with(ProviderCapability::Intl)
            ),
            Some(ProviderVariant::Full)
        );
    }

    #[test]
    fn identity_changes_for_semantic_fields_but_ignores_archive_filename() {
        let mut m = manifest(
            "target".into(),
            "rustc".into(),
            "source".into(),
            "archive".into(),
            "deployment".into(),
            "libc".into(),
            ProviderVariant::Full,
        );
        let base = m.identity();
        m.profile.push('x');
        assert_ne!(base, m.identity());
        m.profile.pop();
        m.variant = ProviderVariant::Common;
        assert_ne!(base, m.identity());
        m.variant = ProviderVariant::Full;
        m.capabilities = ProviderCapabilities::COMMON.iter().collect();
        assert_ne!(base, m.identity());
        m.capabilities = ProviderCapabilities::FULL.iter().collect();
        m.exports.push("extra".into());
        assert_ne!(base, m.identity());
        m.exports.pop();
        m.archive.push('x');
        assert_eq!(base, m.identity());
    }
}
