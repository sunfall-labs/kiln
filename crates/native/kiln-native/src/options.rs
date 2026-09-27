use kiln_artifact_cache::BuildProfile as CacheProfile;
use kiln_backend_c::{EmissionPolicy, EmissionStringPolicy};
use kiln_native_toolchain::BuildProfile as ToolchainProfile;
use std::path::PathBuf;

/// Native optimization and checking policy.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum NativeProfile {
    #[default]
    Dev,
    Check,
    Release,
    /// Size-optimized release profile.
    ReleaseSize,
    /// Release, instrumented for `gprof`. See `scripts/profile-native.sh`.
    Profile,
}

impl NativeProfile {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Dev => "native-dev",
            Self::Check => "native-check",
            Self::Release => "native-release",
            Self::ReleaseSize => "native-release-size",
            Self::Profile => "native-profile",
        }
    }

    pub(crate) const fn cache(self) -> CacheProfile {
        match self {
            Self::Dev => CacheProfile::Debug,
            Self::Check => CacheProfile::Check,
            Self::Release => CacheProfile::Release,
            Self::ReleaseSize => CacheProfile::ReleaseSize,
            // Its own cache bucket: an instrumented object must never satisfy a release build.
            Self::Profile => CacheProfile::Profile,
        }
    }

    pub(crate) const fn toolchain(self) -> ToolchainProfile {
        match self {
            Self::Dev => ToolchainProfile::NativeDev,
            Self::Check => ToolchainProfile::NativeCheck,
            Self::Release => ToolchainProfile::NativeRelease,
            Self::ReleaseSize => ToolchainProfile::NativeReleaseSize,
            Self::Profile => ToolchainProfile::NativeProfile,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeBuildOptions {
    pub output: PathBuf,
    pub profile: NativeProfile,
    pub realm: NativeRealmPolicy,
    pub march_native: bool,
    pub static_link: bool,
    pub pgo_generate: Option<PathBuf>,
    pub pgo_use: Option<PathBuf>,
    pub node: bool,
}

impl Default for NativeBuildOptions {
    fn default() -> Self {
        Self {
            output: PathBuf::new(),
            profile: NativeProfile::default(),
            realm: NativeRealmPolicy::default(),
            march_native: false,
            static_link: false,
            pgo_generate: None,
            pgo_use: None,
            node: false,
        }
    }
}

impl NativeBuildOptions {
    pub fn new(output: impl Into<PathBuf>, profile: NativeProfile) -> Self {
        Self {
            output: output.into(),
            profile,
            realm: NativeRealmPolicy::default(),
            march_native: false,
            static_link: false,
            pgo_generate: None,
            pgo_use: None,
            node: false,
        }
    }

    pub fn with_realm_policy(mut self, realm: NativeRealmPolicy) -> Self {
        self.realm = realm;
        self
    }

    pub fn with_march_native(mut self, march_native: bool) -> Self {
        self.march_native = march_native;
        self
    }

    pub fn with_static_link(mut self, static_link: bool) -> Self {
        self.static_link = static_link;
        self
    }

    pub fn with_pgo_generate(mut self, pgo_generate: Option<PathBuf>) -> Self {
        self.pgo_generate = pgo_generate;
        self
    }

    pub fn with_pgo_use(mut self, pgo_use: Option<PathBuf>) -> Self {
        self.pgo_use = pgo_use;
        self
    }

    pub fn with_type_profile_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.realm.type_profile_path = Some(path.into());
        self
    }

    pub fn with_node(mut self, node: bool) -> Self {
        self.node = node;
        self
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NativeRealmPolicy {
    pub host_clock: bool,
    pub entropy: NativeEntropyPolicy,
    pub timezone: NativeStringPolicy,
    pub locale: NativeStringPolicy,
    pub type_profile_path: Option<PathBuf>,
    /// Never install kiln's own members (ADR 0068), whatever `KILN_EXTENSIONS` says when the
    /// binary runs: `kiln build --no-kiln-extensions`.
    pub no_kiln_extensions: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum NativeEntropyPolicy {
    #[default]
    Deterministic,
    Seeded(u64),
    Host,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum NativeStringPolicy {
    #[default]
    Deterministic,
    Named(String),
    Host,
}

impl NativeRealmPolicy {
    pub const fn host_clock() -> Self {
        Self {
            host_clock: true,
            entropy: NativeEntropyPolicy::Deterministic,
            timezone: NativeStringPolicy::Deterministic,
            locale: NativeStringPolicy::Deterministic,
            type_profile_path: None,
            no_kiln_extensions: false,
        }
    }

    /// The same policy for a binary that never installs kiln's own members (ADR 0068).
    pub const fn without_kiln_extensions(mut self, without: bool) -> Self {
        self.no_kiln_extensions = without;
        self
    }

    pub const fn with_entropy(mut self, entropy: NativeEntropyPolicy) -> Self {
        self.entropy = entropy;
        self
    }

    pub fn with_timezone(mut self, timezone: NativeStringPolicy) -> Self {
        self.timezone = timezone;
        self
    }

    pub fn with_locale(mut self, locale: NativeStringPolicy) -> Self {
        self.locale = locale;
        self
    }
}

impl NativeRealmPolicy {
    pub(crate) fn emission(&self) -> EmissionPolicy {
        EmissionPolicy {
            host_clock: self.host_clock,
            entropy: match self.entropy {
                NativeEntropyPolicy::Deterministic => {
                    kiln_backend_c::EmissionEntropyPolicy::Deterministic
                }
                NativeEntropyPolicy::Seeded(seed) => {
                    kiln_backend_c::EmissionEntropyPolicy::Seeded(seed)
                }
                NativeEntropyPolicy::Host => kiln_backend_c::EmissionEntropyPolicy::Host,
            },
            timezone: match &self.timezone {
                NativeStringPolicy::Deterministic => EmissionStringPolicy::Deterministic,
                NativeStringPolicy::Named(value) => EmissionStringPolicy::Named(value.clone()),
                NativeStringPolicy::Host => EmissionStringPolicy::Host,
            },
            locale: match &self.locale {
                NativeStringPolicy::Deterministic => EmissionStringPolicy::Deterministic,
                NativeStringPolicy::Named(value) => EmissionStringPolicy::Named(value.clone()),
                NativeStringPolicy::Host => EmissionStringPolicy::Host,
            },
            decoupled_prelude: true,
            compact: false,
            generic_only: false,
            parallel_in_generic: false,
            speculate: Some(std::env::var("KILN_SPECULATE").as_deref() != Ok("0")),
            type_profile_record: false,
            type_profile: None,
            live_module_slots: None,
            compile_regex: None,
            omit_source: false,
            no_kiln_extensions: self.no_kiln_extensions,
        }
    }

    pub(crate) fn emission_executable(&self) -> EmissionPolicy {
        let mut policy = self.emission();
        policy.decoupled_prelude = true;
        policy
    }
}
