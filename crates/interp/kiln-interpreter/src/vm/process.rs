use super::*;

impl<H: HostOps + ?Sized> Vm<'_, H> {
    /// Lazily build this execution's restricted `process` object.
    pub(super) fn process_object(&mut self) -> Value {
        if let Some(existing) = &self.process_object {
            return existing.clone();
        }
        // A realm shared by several machines (a module graph's) has one `process`: the one its
        // global object was given when the first machine built it. A second machine reaching
        // `process` must find that one, or a change made through one -- a worker's `env`,
        // set by the bootstrap before the program runs -- is invisible through the other.
        if let Some(Some(Value::Object(global))) =
            self.global_objects.get(self.current_realm.0 as usize)
            && let Some(existing @ Value::Object(_)) = global.borrow().get_own("process").cloned()
        {
            self.process_object = Some(existing.clone());
            return existing;
        }
        let mut process = ObjectData::default();
        process.set_prototype(Value::Native(
            Native::PrototypeOf(PrototypeTable::EventEmitter),
            RealmId::PRIMARY,
        ));
        process.node_events = Some(Vec::new());
        let mut env_obj = ObjectData::default();
        // Appended rather than `set`, with the duplicate check a hash lookup: `set` searches the
        // store for each name, which made building `process` -- every realm does -- quadratic in
        // the size of the environment (225 names here, a quarter of a bare realm's cost). An
        // environment can hold a name twice; the first one is kept, as `getenv` does.
        let mut seen = std::collections::HashSet::new();
        for (key, val) in std::env::vars() {
            if !seen.insert(key.clone()) {
                continue;
            }
            env_obj.insert_key(
                PropertyKeyRef::Utf8(&key),
                Property::Value(Value::String(val.into())),
                Attributes::DEFAULT,
            );
        }
        process.set("env", Value::Object(Rc::new(RefCell::new(env_obj))));
        // Whatever the host said, verbatim -- including the two leading entries, which node
        // guarantees are there and which `const [, , first] = process.argv` depends on. A host
        // that stated no command line leaves this empty rather than inventing an executable
        // path; nothing was launched, so there is nothing to report.
        process.set(
            "argv",
            Value::Array(ArrayData::new(
                self.argv
                    .iter()
                    .cloned()
                    .map(|value| Value::String(value.into()))
                    .collect(),
            )),
        );
        // The runtime does not forward its own command-line flags to a user's worker. Node
        // packages such as pino still inspect this array before creating one.
        process.set("execArgv", Value::Array(ArrayData::new(Vec::new())));
        if let Some(executable) = self.argv.first() {
            process.set("execPath", Value::String(executable.clone().into()));
        }
        // The host's platform and the node this runtime targets (ADR 0063), spelled the way
        // node spells them. This answered "kiln" for both until 2026-09-19 -- on the theory that
        // claiming to be darwin invites a path nothing here implements -- and every library
        // that reads `process.platform` for a path separator or `process.version.slice(1)` for
        // a feature took its oldest branch instead. The native backend answered "darwin".
        let platform = match std::env::consts::OS {
            "macos" => "darwin",
            "windows" => "win32",
            other => other,
        };
        process.set("platform", Value::String(platform.to_owned().into()));
        // The program's pid, which is not the process's when the daemon hosts it (realms issue 26).
        process.set(
            "pid",
            Value::Number(f64::from(kiln_runtime::signals::pid())),
        );
        process.set("version", Value::String("v22.0.0".to_owned().into()));
        process.set(
            "versions",
            ordinary_object([("node".to_owned(), Value::String("22.0.0".to_owned().into()))]),
        );
        process.set(
            "release",
            ordinary_object([("name".to_owned(), Value::String("node".to_owned().into()))]),
        );
        let arch = match std::env::consts::ARCH {
            "aarch64" => "arm64",
            "x86_64" => "x64",
            other => other,
        };
        process.set("arch", Value::String(arch.to_owned().into()));
        process.set(
            "features",
            ordinary_object([
                ("cached_builtins".to_owned(), Value::Boolean(true)),
                ("debug".to_owned(), Value::Boolean(false)),
                ("inspector".to_owned(), Value::Boolean(false)),
                ("ipv6".to_owned(), Value::Boolean(true)),
                ("tls".to_owned(), Value::Boolean(true)),
                ("tls_alpn".to_owned(), Value::Boolean(true)),
                ("tls_ocsp".to_owned(), Value::Boolean(true)),
                ("tls_sni".to_owned(), Value::Boolean(true)),
                ("uv".to_owned(), Value::Boolean(true)),
            ]),
        );
        let stdin_is_tty = {
            use std::io::IsTerminal;
            std::io::stdin().is_terminal() || std::env::var("PI_STARTUP_BENCHMARK").is_ok()
        };
        let stdout_is_tty = {
            use std::io::IsTerminal;
            std::io::stdout().is_terminal() || std::env::var("PI_STARTUP_BENCHMARK").is_ok()
        };
        let stderr_is_tty = {
            use std::io::IsTerminal;
            std::io::stderr().is_terminal() || std::env::var("PI_STARTUP_BENCHMARK").is_ok()
        };

        let stdin = node_readable(&[], None);
        if let Value::Object(stdin_obj) = &stdin {
            let mut data = stdin_obj.borrow_mut();
            data.set("fd", Value::Number(0.0));
            data.set("isTTY", Value::Boolean(stdin_is_tty));
            data.set("isRaw", Value::Boolean(false));
            data.set_non_enumerable(
                "setRawMode",
                Value::Native(Native::ProcessStdinSetRawMode, RealmId::PRIMARY),
            );
        }
        process.set("stdin", stdin);

        let stdout = node_writable(&[], None);
        if let Value::Object(stdout_obj) = &stdout {
            let mut data = stdout_obj.borrow_mut();
            data.set("fd", Value::Number(1.0));
            data.set("isTTY", Value::Boolean(stdout_is_tty));
            data.set("columns", Value::Number(80.0));
            data.set("rows", Value::Number(24.0));
        }
        process.set("stdout", stdout);

        let stderr = node_writable(&[], None);
        if let Value::Object(stderr_obj) = &stderr {
            let mut data = stderr_obj.borrow_mut();
            data.set("fd", Value::Number(2.0));
            data.set("isTTY", Value::Boolean(stderr_is_tty));
        }
        process.set("stderr", stderr);
        process.set_non_enumerable(
            "hrtime",
            Value::Native(Native::ProcessHrtime, RealmId::PRIMARY),
        );
        process.set_non_enumerable("cwd", Value::Native(Native::ProcessCwd, RealmId::PRIMARY));
        process.set_non_enumerable("kill", Value::Native(Native::ProcessKill, RealmId::PRIMARY));
        process.set_non_enumerable("exit", Value::Native(Native::ProcessExit, RealmId::PRIMARY));
        process.set_non_enumerable(
            "nextTick",
            Value::Native(Native::ProcessNextTick, RealmId::PRIMARY),
        );
        process.set_non_enumerable(
            "getBuiltinModule",
            Value::Native(Native::ProcessGetBuiltinModule, RealmId::PRIMARY),
        );
        // TypeScript Node builtins use this private capability table while they are
        // materializing themselves.  It is deliberately separate from the public
        // getBuiltinModule projection: asking for the builtin currently being evaluated
        // must not fall back to a second legacy namespace with different identity.
        let native_modules = ordinary_object([
            (
                "async_hooks".to_owned(),
                Value::Native(Native::NodeAsyncHooks, RealmId::PRIMARY),
            ),
            (
                "fs".to_owned(),
                Value::Native(Native::NodeFs, RealmId::PRIMARY),
            ),
            (
                "fs_promises".to_owned(),
                Value::Native(Native::NodeFsPromises, RealmId::PRIMARY),
            ),
            ("http".to_owned(), crate::http::native_capabilities()),
            ("net".to_owned(), crate::net::native_capabilities()),
        ]);
        process.set_non_enumerable("__kilnNativeModules", native_modules);
        let value = Value::Object(Rc::new(RefCell::new(process)));
        self.process_object = Some(value.clone());
        value
    }
}
