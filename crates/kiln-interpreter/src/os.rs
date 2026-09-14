//! `node:os` operating system facts and environment queries.
//!
//! Exposes platform, architecture, memory, CPU and network interface descriptors.
//! Compile-time constants are read from `std::env::consts`, keeping values reproducible
//! across runs for a given target.

use crate::{ArrayData, ExecutionError, Native, RealmId, Value, ordinary_object};

/// The fixed facts the OS built-in answers without host delegation.
///
/// Kept minimal on purpose: every fact here is an answer to a question a
/// program can act on wrongly without also being a program that needed a real machine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OsFact {
    Platform,
    Arch,
    Type,
    Endianness,
    Machine,
    TmpDir,
    HomeDir,
    Release,
    Version,
    Parallelism,
    Cpus,
    TotalMem,
    FreeMem,
    Uptime,
    LoadAvg,
    UserInfo,
}

pub(crate) fn os_fact(fact: OsFact) -> Value {
    let text = |value: &str| Value::String(value.to_owned().into());
    match fact {
        // node's spelling, which is not always Rust's: "macos" is "darwin" to every program that
        // branches on this, and answering Rust's name would fail every one of those branches.
        OsFact::Platform => text(match std::env::consts::OS {
            "macos" => "darwin",
            "windows" => "win32",
            other => other,
        }),
        OsFact::Arch => text(match std::env::consts::ARCH {
            "x86_64" => "x64",
            "aarch64" => "arm64",
            other => other,
        }),
        OsFact::Type => text(match std::env::consts::OS {
            "macos" => "Darwin",
            "windows" => "Windows_NT",
            "linux" => "Linux",
            other => other,
        }),
        OsFact::Machine => text(std::env::consts::ARCH),
        OsFact::Endianness => text(if cfg!(target_endian = "big") {
            "BE"
        } else {
            "LE"
        }),
        OsFact::TmpDir => text("/tmp"),
        OsFact::HomeDir => text("/"),
        OsFact::Release => text("0.0.0"),
        OsFact::Version => text("kiln"),
        // One, and truthfully so: the realm has one agent (see `Atomics.wait`), so a library
        // sizing a worker pool by this gets a pool this realm can actually run.
        OsFact::Parallelism => Value::Number(1.0),
        OsFact::Cpus => Value::Array(ArrayData::new(vec![ordinary_object([
            ("model".to_owned(), text("kiln")),
            ("speed".to_owned(), Value::Number(0.0)),
            (
                "times".to_owned(),
                ordinary_object([
                    ("user".to_owned(), Value::Number(0.0)),
                    ("nice".to_owned(), Value::Number(0.0)),
                    ("sys".to_owned(), Value::Number(0.0)),
                    ("idle".to_owned(), Value::Number(0.0)),
                    ("irq".to_owned(), Value::Number(0.0)),
                ]),
            ),
        ])])),
        OsFact::TotalMem | OsFact::FreeMem => Value::Number(0.0),
        // Zero for the same reason `Date.now()` starts at the epoch: nothing has happened yet.
        OsFact::Uptime => Value::Number(0.0),
        OsFact::LoadAvg => Value::Array(ArrayData::new(vec![
            Value::Number(0.0),
            Value::Number(0.0),
            Value::Number(0.0),
        ])),
        OsFact::UserInfo => ordinary_object([
            ("uid".to_owned(), Value::Number(-1.0)),
            ("gid".to_owned(), Value::Number(-1.0)),
            ("username".to_owned(), text("kiln")),
            ("homedir".to_owned(), text("/")),
            ("shell".to_owned(), Value::Null),
        ]),
    }
}

pub(crate) fn network_interfaces() -> Value {
    let loopback = Value::Array(ArrayData::new(vec![ordinary_object([
        ("address".to_owned(), Value::String("127.0.0.1".to_owned().into())),
        ("family".to_owned(), Value::String("IPv4".to_owned().into())),
        ("internal".to_owned(), Value::Boolean(true)),
    ])]));
    ordinary_object([("loopback".to_owned(), loopback)])
}

pub(crate) fn property(prop: &str) -> Option<Value> {
    match prop {
        "networkInterfaces" => Some(Value::Native(
            Native::NodeOsNetworkInterfaces,
            RealmId::PRIMARY,
        )),
        "hostname" => Some(Value::Native(Native::NodeOsHostname, RealmId::PRIMARY)),
        "platform" => Some(Value::Native(
            Native::NodeOsFact(OsFact::Platform),
            RealmId::PRIMARY,
        )),
        "arch" => Some(Value::Native(
            Native::NodeOsFact(OsFact::Arch),
            RealmId::PRIMARY,
        )),
        "type" => Some(Value::Native(
            Native::NodeOsFact(OsFact::Type),
            RealmId::PRIMARY,
        )),
        "endianness" => Some(Value::Native(
            Native::NodeOsFact(OsFact::Endianness),
            RealmId::PRIMARY,
        )),
        "tmpdir" => Some(Value::Native(
            Native::NodeOsFact(OsFact::TmpDir),
            RealmId::PRIMARY,
        )),
        "homedir" => Some(Value::Native(
            Native::NodeOsFact(OsFact::HomeDir),
            RealmId::PRIMARY,
        )),
        "release" => Some(Value::Native(
            Native::NodeOsFact(OsFact::Release),
            RealmId::PRIMARY,
        )),
        "version" => Some(Value::Native(
            Native::NodeOsFact(OsFact::Version),
            RealmId::PRIMARY,
        )),
        "machine" => Some(Value::Native(
            Native::NodeOsFact(OsFact::Machine),
            RealmId::PRIMARY,
        )),
        "availableParallelism" => Some(Value::Native(
            Native::NodeOsFact(OsFact::Parallelism),
            RealmId::PRIMARY,
        )),
        "cpus" => Some(Value::Native(
            Native::NodeOsFact(OsFact::Cpus),
            RealmId::PRIMARY,
        )),
        "totalmem" => Some(Value::Native(
            Native::NodeOsFact(OsFact::TotalMem),
            RealmId::PRIMARY,
        )),
        "freemem" => Some(Value::Native(
            Native::NodeOsFact(OsFact::FreeMem),
            RealmId::PRIMARY,
        )),
        "uptime" => Some(Value::Native(
            Native::NodeOsFact(OsFact::Uptime),
            RealmId::PRIMARY,
        )),
        "loadavg" => Some(Value::Native(
            Native::NodeOsFact(OsFact::LoadAvg),
            RealmId::PRIMARY,
        )),
        "userInfo" => Some(Value::Native(
            Native::NodeOsFact(OsFact::UserInfo),
            RealmId::PRIMARY,
        )),
        "EOL" => Some(Value::String("\n".to_owned().into())),
        "devNull" => Some(Value::String("/dev/null".to_owned().into())),
        _ => None,
    }
}

pub(crate) const fn js_name(native: Native) -> Option<&'static str> {
    match native {
        Native::NodeOsNetworkInterfaces => Some("networkInterfaces"),
        Native::NodeOsHostname => Some("hostname"),
        Native::NodeOsFact(fact) => match fact {
            OsFact::Platform => Some("platform"),
            OsFact::Arch => Some("arch"),
            OsFact::Type => Some("type"),
            OsFact::Endianness => Some("endianness"),
            OsFact::TmpDir => Some("tmpdir"),
            OsFact::HomeDir => Some("homedir"),
            OsFact::Release => Some("release"),
            OsFact::Version => Some("version"),
            OsFact::Machine => Some("machine"),
            OsFact::Parallelism => Some("availableParallelism"),
            OsFact::Cpus => Some("cpus"),
            OsFact::TotalMem => Some("totalmem"),
            OsFact::FreeMem => Some("freemem"),
            OsFact::Uptime => Some("uptime"),
            OsFact::LoadAvg => Some("loadavg"),
            OsFact::UserInfo => Some("userInfo"),
        },
        _ => None,
    }
}

pub(crate) fn dispatch(native: Native) -> Result<Value, ExecutionError> {
    match native {
        Native::NodeOsNetworkInterfaces => Ok(network_interfaces()),
        Native::NodeOsHostname => Ok(Value::String("kiln".to_owned().into())),
        Native::NodeOsFact(fact) => Ok(os_fact(fact)),
        _ => Err(ExecutionError::TypeErrorMessage {
            message: "Invalid native os operation".into(),
        }),
    }
}
