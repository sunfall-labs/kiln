use super::*;

impl<H: HostOps + ?Sized> Vm<'_, H> {
    /// Writes one console line to standard output (`stderr` false) or standard error. A partial
    /// line `write` left pending continues on it, as on a terminal.
    pub(super) fn host_line(
        &mut self,
        stderr: bool,
        mut arguments: Vec<HostValue>,
    ) -> Result<HostValue, kiln_host_abi::HostError> {
        let pending = std::mem::take(&mut self.pending_stdio[usize::from(stderr)]);
        if !pending.is_empty() {
            match arguments.first_mut() {
                Some(first) => {
                    *first = HostValue::String((pending + &first.to_console_string()).into());
                }
                None => arguments.push(HostValue::String(pending.into())),
            }
        }
        let op = if stderr {
            HostOp::WriteStderr
        } else {
            HostOp::WriteStdout
        };
        self.host.call(op, &arguments)
    }

    /// `stream.write(text)` on standard output or error: emits every completed line and keeps
    /// the unterminated rest pending, so `write("a")` prints `a` and no newline.
    pub(super) fn write_stdio(
        &mut self,
        stderr: bool,
        text: &str,
    ) -> Result<(), kiln_host_abi::HostError> {
        let op = if stderr {
            HostOp::WriteStderr
        } else {
            HostOp::WriteStdout
        };
        let index = usize::from(stderr);
        self.pending_stdio[index].push_str(text);
        while let Some(end) = self.pending_stdio[index].find('\n') {
            let line: String = self.pending_stdio[index].drain(..=end).collect();
            let line = line
                .trim_end_matches('\n')
                .trim_end_matches('\r')
                .to_owned();
            self.host.call(op, &[HostValue::String(line.into())])?;
        }
        Ok(())
    }

    /// Emits whatever partial lines are still pending (the run is ending or exiting).
    pub(crate) fn flush_stdio(&mut self) {
        for stderr in [false, true] {
            let pending = std::mem::take(&mut self.pending_stdio[usize::from(stderr)]);
            if !pending.is_empty() {
                let op = if stderr {
                    HostOp::WriteStderr
                } else {
                    HostOp::WriteStdout
                };
                let _ = self.host.call(op, &[HostValue::String(pending.into())]);
            }
        }
    }

    /// `new Console(stdout, [stderr])` or `new Console({ stdout, stderr, ... })`.
    pub(super) fn construct_console(
        &mut self,
        arguments: &[Value],
        instruction: usize,
    ) -> Result<Value, ExecutionError> {
        let mut stdout = Value::Undefined;
        let mut stderr = Value::Undefined;

        if let Some(first) = arguments.first() {
            if let Value::Object(_) = first {
                let opt_stdout = self.read_property(first, "stdout", instruction)?;
                if !matches!(opt_stdout, Value::Undefined) {
                    stdout = opt_stdout;
                    let opt_stderr = self.read_property(first, "stderr", instruction)?;
                    stderr = if matches!(opt_stderr, Value::Undefined) {
                        stdout.clone()
                    } else {
                        opt_stderr
                    };
                } else {
                    stdout = first.clone();
                    stderr = arguments.get(1).cloned().unwrap_or_else(|| stdout.clone());
                }
            } else if !matches!(first, Value::Undefined) {
                stdout = first.clone();
                stderr = arguments.get(1).cloned().unwrap_or_else(|| stdout.clone());
            }
        }

        if matches!(stdout, Value::Undefined) {
            return Err(ExecutionError::TypeErrorMessage {
                message: "Console expects a writable stream instance for stdout".to_owned(),
            });
        }

        let mut data = ObjectData::default();
        data.set_prototype(Value::Native(
            Native::PrototypeOf(PrototypeTable::Console),
            RealmId::PRIMARY,
        ));
        data.set("_stdout", stdout);
        data.set("_stderr", stderr);
        data.set("log", Value::Native(Native::ConsoleLog, RealmId::PRIMARY));
        data.set("info", Value::Native(Native::ConsoleInfo, RealmId::PRIMARY));
        data.set(
            "debug",
            Value::Native(Native::ConsoleDebug, RealmId::PRIMARY),
        );
        data.set("dir", Value::Native(Native::ConsoleDir, RealmId::PRIMARY));
        data.set("warn", Value::Native(Native::ConsoleWarn, RealmId::PRIMARY));
        data.set(
            "error",
            Value::Native(Native::ConsoleError, RealmId::PRIMARY),
        );
        data.set(
            "assert",
            Value::Native(Native::ConsoleAssert, RealmId::PRIMARY),
        );
        data.set(
            "clear",
            Value::Native(Native::ConsoleClear, RealmId::PRIMARY),
        );
        data.set(
            "count",
            Value::Native(Native::ConsoleCount, RealmId::PRIMARY),
        );
        data.set(
            "countReset",
            Value::Native(Native::ConsoleCountReset, RealmId::PRIMARY),
        );
        data.set(
            "group",
            Value::Native(Native::ConsoleGroup, RealmId::PRIMARY),
        );
        data.set(
            "groupEnd",
            Value::Native(Native::ConsoleGroupEnd, RealmId::PRIMARY),
        );
        data.set(
            "groupCollapsed",
            Value::Native(Native::ConsoleGroupCollapsed, RealmId::PRIMARY),
        );
        data.set(
            "table",
            Value::Native(Native::ConsoleTable, RealmId::PRIMARY),
        );
        data.set("time", Value::Native(Native::ConsoleTime, RealmId::PRIMARY));
        data.set(
            "timeEnd",
            Value::Native(Native::ConsoleTimeEnd, RealmId::PRIMARY),
        );
        data.set(
            "timeLog",
            Value::Native(Native::ConsoleTimeLog, RealmId::PRIMARY),
        );
        data.set(
            "trace",
            Value::Native(Native::ConsoleTrace, RealmId::PRIMARY),
        );
        data.set(
            "dirxml",
            Value::Native(Native::ConsoleDirxml, RealmId::PRIMARY),
        );
        data.set(
            "timeStamp",
            Value::Native(Native::ConsoleTimeStamp, RealmId::PRIMARY),
        );
        data.set(
            "profile",
            Value::Native(Native::ConsoleProfile, RealmId::PRIMARY),
        );
        data.set(
            "profileEnd",
            Value::Native(Native::ConsoleProfileEnd, RealmId::PRIMARY),
        );

        Ok(Value::Object(tracked_object(data)))
    }

    /// Node's `util.inspect.custom` for what a console prints: an object with a callable
    /// `[Symbol.for("nodejs.util.inspect.custom")]` shows what it answers for
    /// `(depth, options, inspect)` -- a string as it stands, anything else inspected in its place.
    /// Only at the top level, where the host's formatter cannot call back into the program. A
    /// hook that throws, which one that needs the `inspect` argument this does not have will,
    /// leaves the value to the ordinary formatting.
    pub(super) fn console_arguments(
        &mut self,
        mut arguments: Vec<Value>,
        instruction: usize,
    ) -> Result<Vec<Value>, ExecutionError> {
        // `Symbol.for`'s: until something registers it, nothing can carry the hook.
        let registered = match &self.module_realm {
            Some(realm) => realm.borrow().symbol_registry.clone(),
            None => self.symbol_registry.clone(),
        };
        let Some((_, custom)) = registered
            .into_iter()
            .find(|(name, _)| name.to_lossy_string() == "nodejs.util.inspect.custom")
        else {
            return Ok(arguments);
        };
        for argument in &mut arguments {
            if !is_reflect_target(argument) {
                continue;
            }
            let Ok(hook) = self.catching(|vm| vm.read_computed(argument, &custom, instruction))
            else {
                continue;
            };
            if !is_callable(&hook) {
                continue;
            }
            let options = Value::Object(tracked_object(ObjectData::default()));
            let hook_arguments = vec![Value::Number(2.0), options, Value::Undefined];
            if let Ok(shown) =
                self.call_catching(&hook, argument.clone(), hook_arguments, instruction)?
            {
                *argument = shown;
            }
        }
        Ok(arguments)
    }

    pub(super) fn write_console_output(
        &mut self,
        receiver: &Value,
        is_stderr: bool,
        arguments: &[Value],
        instruction: usize,
    ) -> Result<Value, ExecutionError> {
        let stream = match receiver {
            Value::Object(_) => {
                let prop = if is_stderr { "_stderr" } else { "_stdout" };
                self.read_property(receiver, prop, instruction)?
            }
            _ => Value::Undefined,
        };
        if !matches!(stream, Value::Undefined)
            && let Ok(write_fn) = self.read_property(&stream, "write", instruction)
            && is_callable(&write_fn)
        {
            let formatted = format!("{}\n", node_util_format(arguments));
            self.call_value(
                &write_fn,
                stream.clone(),
                vec![Value::String(formatted.into())],
                instruction,
            )?;
            return Ok(Value::Undefined);
        }
        let host_args = self
            .console_arguments(arguments.to_vec(), instruction)?
            .into_iter()
            .map(HostValue::from)
            .collect::<Vec<_>>();
        self.host_line(is_stderr, host_args)
            .map_err(|error| ExecutionError::Host {
                instruction,
                message: error.to_string(),
            })?;
        Ok(Value::Undefined)
    }
}
