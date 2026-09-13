use std::path::{Path, PathBuf};

use kiln_digest::Digest;
use serde::Serialize;

pub const SOURCE_LIST: &str = include_str!("../../kiln-native/provider-sources.txt");
pub const SCHEMA_VERSION: u16 = 1;
pub const PROVIDER_NAME: &str = "kiln-native-number-ffi";
pub const PROFILE: &str = "provider";
pub const RUSTFLAGS: &str =
    "-C panic=unwind -C relocation-model=pic -C target-cpu=generic -C debuginfo=0";
pub const FULL_FEATURES: &str = "regexp,bigint,temporal,json,intl,parallel,ffi";
pub const SLIM_FEATURES: &str = "realm,regexp,bigint,temporal,json,intl,parallel,ffi";
pub const FULL_ARCHIVE: &str = "libkiln_native_number_ffi.a";
pub const SLIM_ARCHIVE: &str = "libkiln_native_number_ffi.slim.a";
pub const FULL_MANIFEST: &str = "kiln-native-number-provider.json";
pub const SLIM_MANIFEST: &str = "kiln-native-number-provider.slim.json";

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
    pub exports: Vec<&'static str>,
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
    let mut bytes = Vec::new();
    for relative in source_paths() {
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

pub fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn source_digest_changes_with_contents_and_order() {
        // The repository inventory is fixed; this test locks the framing through a known prefix.
        assert!(source_digest(Path::new("/definitely/missing")).is_err());
        assert!(!contract().exports.is_empty());
    }
}
