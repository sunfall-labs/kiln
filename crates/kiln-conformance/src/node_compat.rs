//! Explicit inventory for pinned upstream Node compatibility tests.
//!
//! This follows Deno's useful split: upstream files are a separately pinned suite, while this
//! repository owns a small configuration that says which tests are gated, skipped, flaky, or
//! expected to fail and why. The inventory never changes a test result; it decides what CI claims.

use std::collections::BTreeMap;
use std::error::Error;
use std::fmt::{self, Display, Formatter};
use std::fs;
use std::path::{Path, PathBuf};

use kiln_digest::Digest;
use serde::Deserialize;

pub const NODE_COMPAT_CONFIG: &str = "node-compat.json";

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NodeCompatInventory {
    pub schema: u32,
    pub upstream: UpstreamPin,
    pub tests: BTreeMap<String, NodeTestExpectation>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpstreamPin {
    pub repository: String,
    pub tag: String,
    pub commit: String,
    pub suite_path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NodeTestExpectation {
    pub expectation: Expectation,
    pub sha256: String,
    #[serde(default)]
    pub reason: Option<String>,
    #[serde(default = "enabled")]
    pub linux: bool,
    #[serde(default = "enabled")]
    pub darwin: bool,
    #[serde(default = "enabled")]
    pub windows: bool,
    #[serde(default)]
    pub features: Vec<String>,
    /// Executable repository-owned cases derived from this exact upstream source.
    #[serde(default)]
    pub adapted: Vec<String>,
    /// Whether those cases preserve every relevant assertion in the selected upstream file.
    #[serde(default)]
    pub adapted_complete: bool,
}

const fn enabled() -> bool {
    true
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Expectation {
    Pass,
    Skip,
    ExpectedFailure,
    Flaky,
}

impl NodeCompatInventory {
    pub fn read(path: &Path) -> Result<Self, NodeCompatError> {
        let text = fs::read_to_string(path).map_err(|source| NodeCompatError::Io {
            path: path.to_owned(),
            source,
        })?;
        let inventory: Self =
            serde_json::from_str(&text).map_err(|source| NodeCompatError::Json {
                path: path.to_owned(),
                source,
            })?;
        inventory.validate(path)?;
        Ok(inventory)
    }

    fn validate(&self, path: &Path) -> Result<(), NodeCompatError> {
        if self.schema != 1 {
            return Err(NodeCompatError::Schema {
                path: path.to_owned(),
                message: format!("unsupported node compatibility schema {}", self.schema),
            });
        }
        if self.upstream.commit.len() != 40
            || !self
                .upstream
                .commit
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(NodeCompatError::Schema {
                path: path.to_owned(),
                message: "upstream commit must be a full 40-digit hexadecimal identity".into(),
            });
        }
        for (test, expectation) in &self.tests {
            if Path::new(test).is_absolute() || test.split('/').any(|component| component == "..") {
                return Err(NodeCompatError::Schema {
                    path: path.to_owned(),
                    message: format!("test path `{test}` must be suite-relative"),
                });
            }
            Digest::from_hex(&expectation.sha256).map_err(|source| NodeCompatError::Schema {
                path: path.to_owned(),
                message: format!("test `{test}` has an invalid sha256: {source}"),
            })?;
            let needs_reason = expectation.expectation != Expectation::Pass
                || !expectation.linux
                || !expectation.darwin
                || !expectation.windows;
            if needs_reason
                && expectation
                    .reason
                    .as_deref()
                    .is_none_or(|reason| reason.trim().is_empty())
            {
                return Err(NodeCompatError::Schema {
                    path: path.to_owned(),
                    message: format!("test `{test}` needs a non-empty reason"),
                });
            }
            if expectation.adapted_complete && expectation.adapted.is_empty() {
                return Err(NodeCompatError::Schema {
                    path: path.to_owned(),
                    message: format!(
                        "test `{test}` cannot declare complete adaptation without adapted cases"
                    ),
                });
            }
            if expectation.expectation == Expectation::Pass && !expectation.adapted_complete {
                return Err(NodeCompatError::Schema {
                    path: path.to_owned(),
                    message: format!(
                        "passing test `{test}` must declare a complete executable adaptation"
                    ),
                });
            }
        }
        Ok(())
    }

    pub fn passing(&self) -> impl Iterator<Item = (&str, &NodeTestExpectation)> {
        self.tests.iter().filter_map(|(path, expectation)| {
            (expectation.expectation == Expectation::Pass).then_some((path.as_str(), expectation))
        })
    }

    /// Fail closed when a configured upstream file is absent from the pinned suite checkout.
    pub fn verify_suite(&self, config_path: &Path) -> Result<(), NodeCompatError> {
        let base = config_path.parent().unwrap_or(Path::new("."));
        let suite = base.join(&self.upstream.suite_path);
        for (test, expectation) in &self.tests {
            let path = suite.join(test);
            let bytes = fs::read(&path).map_err(|source| NodeCompatError::Io {
                path: path.clone(),
                source,
            })?;
            let actual = Digest::hash(&bytes).to_hex();
            if actual != expectation.sha256 {
                return Err(NodeCompatError::Schema {
                    path: config_path.to_owned(),
                    message: format!(
                        "configured Node test differs from the pinned bytes: {} (expected {}, got {actual})",
                        path.display(),
                        expectation.sha256
                    ),
                });
            }
            for adapted in &expectation.adapted {
                let adapted_path = base.join(adapted);
                if !adapted_path.is_file() {
                    return Err(NodeCompatError::Schema {
                        path: config_path.to_owned(),
                        message: format!(
                            "configured adaptation is missing: {}",
                            adapted_path.display()
                        ),
                    });
                }
            }
        }
        Ok(())
    }
}

#[derive(Debug)]
pub enum NodeCompatError {
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    Json {
        path: PathBuf,
        source: serde_json::Error,
    },
    Schema {
        path: PathBuf,
        message: String,
    },
}

impl Display for NodeCompatError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, source } => write!(formatter, "{}: {source}", path.display()),
            Self::Json { path, source } => write!(formatter, "{}: {source}", path.display()),
            Self::Schema { path, message } => write!(formatter, "{}: {message}", path.display()),
        }
    }
}

impl Error for NodeCompatError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Json { source, .. } => Some(source),
            Self::Schema { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repository_inventory_is_pinned_reasoned_and_complete() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../conformance/node")
            .join(NODE_COMPAT_CONFIG);
        let inventory = NodeCompatInventory::read(&path).expect("inventory parses");
        inventory
            .verify_suite(&path)
            .expect("suite matches inventory");
        assert!(inventory.passing().count() >= 2);
        assert!(inventory.tests.values().any(|test| {
            matches!(
                test.expectation,
                Expectation::ExpectedFailure | Expectation::Skip
            )
        }));
    }

    #[test]
    fn exclusions_require_reasons() {
        let inventory: NodeCompatInventory = serde_json::from_str(
            r#"{"schema":1,"upstream":{"repository":"x","tag":"v1","commit":"0000000000000000000000000000000000000000","suite_path":"suite"},"tests":{"parallel/a.js":{"expectation":"skip","sha256":"0000000000000000000000000000000000000000000000000000000000000000"}}}"#,
        )
        .expect("shape parses");
        assert!(inventory.validate(Path::new("config")).is_err());
    }
}
