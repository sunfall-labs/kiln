use super::*;

impl<H: HostOps + ?Sized> Vm<'_, H> {
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

        Ok(Value::Object(Rc::new(RefCell::new(data))))
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
        let host_args = arguments
            .iter()
            .cloned()
            .map(HostValue::from)
            .collect::<Vec<_>>();
        let operation = if is_stderr {
            HostOp::WriteStderr
        } else {
            HostOp::WriteStdout
        };
        self.host
            .call(operation, &host_args)
            .map_err(|error| ExecutionError::Host {
                instruction,
                message: error.to_string(),
            })?;
        Ok(Value::Undefined)
    }
}
