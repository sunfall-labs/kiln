use super::*;

impl<H: HostOps + ?Sized> Vm<'_, H> {
    /// Prepare the receiver and argument list forwarded by `Function.prototype.call/apply`.
    /// The ordinary instruction dispatcher can then enter a compiled target by pushing a VM
    /// frame, without nesting another Rust interpreter run loop for every `.call` in a chain.
    pub(crate) fn forwarded_function_arguments(
        &mut self,
        native: Native,
        arguments: &[Value],
        instruction: usize,
    ) -> Result<(Value, Vec<Value>), ExecutionError> {
        let this_value = arguments.first().cloned().unwrap_or(Value::Undefined);
        if native == Native::FunctionCall {
            return Ok((this_value, arguments.get(1..).unwrap_or_default().to_vec()));
        }
        debug_assert_eq!(native, Native::FunctionApply);
        let rest = match arguments.get(1) {
            Some(Value::Array(items)) => dense_items(&items.borrow()),
            Some(Value::Undefined | Value::Null) | None => Vec::new(),
            Some(other) => {
                let length = self.read_property(other, "length", instruction)?;
                let length = length.as_number();
                let mut list = Vec::new();
                for index in 0..(if length.is_finite() {
                    length.max(0.0) as usize
                } else {
                    0
                }) {
                    list.push(self.read_property(other, &index.to_string(), instruction)?);
                }
                list
            }
        };
        Ok((this_value, rest))
    }

    pub(crate) fn dispatch_native(
        &mut self,
        native: Native,
        receiver: &Value,
        arguments: &[Value],
        instruction: usize,
    ) -> Result<Value, ExecutionError> {
        if native == Native::ShareRaw {
            return self.share_value(arguments.first().cloned().unwrap_or(Value::Undefined));
        }
        // Ahead of the wrapper and proxy preambles below: a `Temporal` value is neither a boxed
        // primitive nor a shape-inspecting built-in, and its own dispatch decides which of the
        // eight types a method belongs to.
        if let Native::Temporal(op) = native {
            return self.temporal_dispatch(op, receiver, arguments, instruction);
        }
        // Date methods share Temporal's requirement to stay on the machine: setters convert
        // arguments through user-visible `valueOf` calls, and the formatting methods read the
        // realm's deterministic clock/zone policy. `call_native` deliberately has no VM and
        // therefore keeps its Date arm unreachable as a guard against losing this route again.
        if let Native::Date(method) = native {
            return self.date_dispatch(method, receiver, arguments, instruction);
        }
        if native == Native::ErrorCaptureStackTrace {
            return self.capture_stack_trace(arguments, instruction);
        }
        if let Native::HostAgent(op) = native {
            return self.host_agent(op, arguments, instruction);
        }
        if native == Native::HostCreateRealm {
            return self.create_realm();
        }
        if native == Native::ShadowRealmEvaluate {
            return self.shadow_realm_evaluate(receiver, arguments);
        }
        if native == Native::ShadowRealmImportValue {
            return self.shadow_realm_import_value(receiver, arguments, instruction);
        }
        if native == Native::HostEvalScript {
            let source = match arguments.first() {
                Some(Value::String(source)) => source.clone(),
                // Unlike `eval`, which hands a non-string straight back: `evalScript` is given
                // source text or it is given nothing it can run.
                Some(value) => self.as_text_units(value, instruction)?,
                None => StringUnits::default(),
            };
            return self.drive_eval_units(&source, None, true);
        }
        // A write through a view onto an immutable buffer is refused before its arguments are
        // converted, and that order is observable: a `valueOf` handed to a setter on such a view
        // must not run, because there was never a write for it to prepare. Here rather than in
        // the setter itself, because the conversion happens on the way there.
        if matches!(
            native,
            Native::DataViewSet(_) | Native::DataViewSetBig { .. }
        ) && let Value::DataView(view) = receiver
            && view.buffer.immutable
        {
            return Err(ExecutionError::BufferImmutable);
        }
        // An `Array.prototype` method whose receiver is not an array walks it live, through the
        // specification's own `HasProperty`/`Get`/`Set`/`Delete` steps. Ahead of everything
        // below because the arms there answer from a dense `Vec` that this receiver has not got.
        if native.walks_a_live_receiver()
            && !matches!(receiver, Value::TypedArray(_))
            && dense_array(receiver).is_none()
        {
            return self.generic_array_native(native, receiver, arguments, instruction);
        }
        // These four take an index argument that the specification converts through
        // `ToIntegerOrInfinity` -- which may be the program's own `valueOf` -- and then start
        // the walk from it. A pure implementation can do neither, and the ones here did
        // neither: `[0, 1, 1].lastIndexOf(1, "1")` searched the whole array and answered 2. So
        // every receiver takes the generic walk, dense array included, and the pure arms are
        // unreachable rather than a second answer that disagrees.
        if matches!(
            native,
            Native::ArrayIndexOf
                | Native::ArrayLastIndexOf
                | Native::ArrayIncludes
                | Native::ArrayAt
                // `toLocaleString` reduces each element by *calling* its own, so it has no pure
                // form at all: it is the machine's from the first element on.
                | Native::ArrayToLocaleString
                // `join` converts its separator with `ToString`, which may be the program's own
                // -- and a view's elements are read the same way an array's are, so the one
                // walk serves both receivers. The pure arm coerced neither, which made a typed
                // array and an array disagree about `join({toString(){ return "-" }})`.
                | Native::ArrayJoin
                // `Array.prototype.toString` is a *call* to the receiver's own `join`, whatever
                // the receiver. A view reaching the pure arm found no `Vec` to join and answered
                // the empty string (`String(new Uint8Array([1, 2, 3]))` was `""`), and an array
                // reaching it joined without `ToString`ing its elements, so every `String(arr)`,
                // `${arr}` and `arr + ""` answered "[object Object]" for an element with its own
                // `toString` (issue 23). It has no pure form.
                | Native::ArrayJoinDefault
        ) {
            return self.generic_array_native(native, receiver, arguments, instruction);
        }
        // The four that build a *new* array take the machine's walk whatever their receiver is,
        // because `ArraySpeciesCreate` is a pair of property reads and possibly a call, and a
        // pure arm can make none of them. A dense fast path here would answer with a plain array
        // while the generic one honoured the receiver's species -- one method disagreeing with
        // another about what `slice` and `map` return is worse than neither consulting it.
        //
        // A typed array is excluded: `%TypedArray%.prototype.slice` builds through
        // `TypedArraySpeciesCreate`, which is a different operation over a different receiver.
        if !matches!(receiver, Value::TypedArray(_))
            && matches!(
                native,
                Native::ArraySlice | Native::ArraySplice | Native::ArrayConcat | Native::ArrayFlat
            )
        {
            return self.generic_array_native(native, receiver, arguments, instruction);
        }
        // These three answer from the machine because each can run program code -- a set-like's
        // `has`/`keys`, a sort comparator, a getter reached through a generic receiver -- and
        // `call_native` has no VM to run it with. Their arms in `call_native` are `unreachable!`
        // precisely so a missing route here is loud; it was silent instead, because the routes
        // were lost and nothing constructed a call that reached them. `Set.prototype.union`
        // panicked the process rather than answering.
        match native {
            Native::SetOperation(operation) => {
                return self.set_operation(operation, receiver, arguments, instruction);
            }
            Native::ArrayCopy(method) => {
                return self.array_copy(method, receiver, arguments, instruction);
            }
            // An array iterator over anything but a dense array or a view reads each position
            // through the program: the `length` may be a getter and the element may be an
            // accessor, and the pure `next` below can call neither.
            Native::ArrayIteratorNext
                if iterator_state(receiver).is_ok_and(|iterated| {
                    dense_array(&iterated).is_none() && !matches!(iterated, Value::TypedArray(_))
                }) =>
            {
                return self.generic_array_iterator_next(receiver, instruction);
            }
            Native::RegExpAccessor(accessor) => {
                return self.regexp_accessor(accessor, receiver, instruction);
            }
            Native::BuiltinAccessor(accessor) => {
                return self.builtin_accessor(accessor, receiver, instruction);
            }
            // `Object.prototype.toString` is defined to *read* `@@toStringTag` off the receiver
            // and use it when it is a string, falling back to the shape it worked out itself.
            // The read can run a getter, so the answer cannot come from the pure table alone --
            // and without it every value whose tag lives on a prototype, from a `Map` to `Math`,
            // reports the bare `[object Object]` that the tag exists to replace.
            Native::ObjectToStringTag if is_reflect_target(receiver) => {
                let tag = self.read_property(
                    receiver,
                    WellKnown::ToStringTag.property_key(),
                    instruction,
                )?;
                if let Value::String(tag) = tag {
                    let mut units = StringUnits::from("[object ").to_units().into_vec();
                    units.extend(tag.units());
                    units.extend(StringUnits::from("]").units());
                    return Ok(Value::String(StringUnits::from_units(&units)));
                }
            }
            // `ToString(text)` may run the program's own `toString`, so the validation cannot
            // happen in the pure table.
            Native::JsonRawJson => {
                let text = self
                    .as_text_units(arguments.first().unwrap_or(&Value::Undefined), instruction)?;
                return build_raw_json(&text);
            }
            Native::JsonIsRawJson => {
                return Ok(Value::Boolean(matches!(
                    arguments.first(),
                    Some(Value::Object(object)) if object.borrow().get_own(RAW_JSON_MARK).is_some()
                )));
            }
            // Both of these need the machine for `CanBeHeldWeakly`, which asks the realm's
            // symbol registry whether a symbol is one a program could look up again.
            Native::WeakRefDeref => return self.weak_ref_deref(receiver, instruction),
            Native::DisposableStackUse => {
                return self.disposable_use(receiver, arguments, instruction, Disposal::Sync);
            }
            Native::DisposableStackAdopt => {
                return self.disposable_adopt(receiver, arguments, Disposal::Sync);
            }
            Native::DisposableStackDefer => {
                return self.disposable_defer(receiver, arguments, Disposal::Sync);
            }
            Native::DisposableStackMove => return self.disposable_move(receiver, Disposal::Sync),
            Native::DisposableStackDispose => {
                return self.disposable_dispose(receiver, instruction);
            }
            Native::AsyncDisposableStackUse => {
                return self.disposable_use(receiver, arguments, instruction, Disposal::Async);
            }
            Native::AsyncDisposableStackAdopt => {
                return self.disposable_adopt(receiver, arguments, Disposal::Async);
            }
            Native::AsyncDisposableStackDefer => {
                return self.disposable_defer(receiver, arguments, Disposal::Async);
            }
            Native::AsyncDisposableStackMove => {
                return self.disposable_move(receiver, Disposal::Async);
            }
            Native::AsyncDisposableStackDisposeAsync => {
                return Ok(self.async_disposable_dispose(receiver, instruction));
            }
            Native::FinalizationRegistryRegister => {
                return self.finalization_registry_register(receiver, arguments, instruction);
            }
            Native::FinalizationRegistryUnregister => {
                return self.finalization_registry_unregister(receiver, arguments, instruction);
            }
            // Calling any of these without `new` is a `TypeError`: each exists to build a value
            // with reserved state, and there is nothing for a plain call to answer with. The
            // specification says so in the first step of every one of them -- "If NewTarget is
            // undefined, throw a TypeError" -- and a `new` never arrives here, because
            // `construct_as` builds each of these itself rather than routing through a call.
            //
            // `Promise` is in this list for a second reason as well: without it, `Promise(f)`
            // fell through to `call_native`, whose `Promise` arm is the `unreachable!` that says
            // the scheduling built-ins are dispatched by the machine -- so the *process aborted*
            // where the language wanted a catchable TypeError.
            Native::WeakRefConstructor
            | Native::ShadowRealmConstructor
            | Native::FinalizationRegistryConstructor
            | Native::MapConstructor
            | Native::SetConstructor
            | Native::WeakMapConstructor
            | Native::WeakSetConstructor
            | Native::ProxyConstructor
            | Native::DisposableStackConstructor
            | Native::AsyncDisposableStackConstructor
            | Native::ArrayBufferConstructor
            | Native::SharedArrayBufferConstructor
            | Native::DataViewConstructor
            | Native::PromiseConstructor
            | Native::TypedArrayConstructor(_) => {
                return Err(ExecutionError::TypeErrorMessage {
                    message: format!("Constructor {} requires 'new'", native_js_name(&native)),
                });
            }
            // All five read `exec` off the receiver and call it, so none of them can be answered
            // without a machine to run the program's own matcher on.
            Native::RegExpSymbol(method) => {
                return self.regexp_symbol(method, receiver, arguments, instruction);
            }
            Native::RegExpStringIteratorNext => {
                return self.regexp_string_iterator_next(receiver, instruction);
            }
            // These four had an `unreachable!` arm in `call_native` and no route here, which is
            // the pairing that fails silently: nothing refuses to compile, and the first program
            // to call one aborts the process instead of answering. `Iterator.from`,
            // `Object.groupBy`, `Map.groupBy` and `Promise.try` each did.
            Native::IteratorFrom => {
                return self.iterator_from(arguments.first(), instruction);
            }
            Native::IteratorWrapNext => {
                let Value::Object(state) = receiver else {
                    return Err(ExecutionError::NotAnIterator);
                };
                let state = Rc::clone(state);
                let source = helper_slot(&state, HELPER_SOURCE);
                let next = helper_slot(&state, HELPER_NEXT);
                // The wrapped iterator's own result object, unchanged: a wrapper exists to add
                // the helper methods, not to reshape what stepping produces.
                return self.call_value(&next, source, Vec::new(), instruction);
            }
            Native::ObjectGroupBy => return self.group_by(false, arguments, instruction),
            Native::MapGroupBy => return self.group_by(true, arguments, instruction),
            _ => {}
        }
        // And ahead of them for `Intl`, for a sharper reason: every constructor here reads an
        // options bag, and the *order* those reads happen in is observable through getters and
        // proxies. That needs the machine, so there is no machine-free path to fall back to.
        if let Native::Intl(op) = native {
            return self.intl_dispatch(op, receiver, arguments, instruction);
        }
        if matches!(native, Native::WorkerOpRaw | Native::PortWaitRaw) {
            return self.worker_native(native, arguments, instruction);
        }
        if matches!(
            native,
            Native::ProcessStartRaw
                | Native::ProcessCancelRaw
                | Native::NodeFsReadFile
                | Native::NodeFsWriteFile
                | Native::NodeFsAppendFile
                | Native::NodeFsStat
                | Native::NodeFsLstat
                | Native::NodeFsMkdir
                | Native::NodeFsReaddir
                | Native::NodeFsCopyFile
                | Native::NodeFsRename
                | Native::NodeFsRm
                | Native::NodeFsUnlink
                | Native::NodeFsRealpath
                | Native::NodeFsAccess
                | Native::NodeFsChmod
                | Native::NodeFsRmdir
                | Native::NodeFsReadlink
        ) {
            return self.dispatch_node_native(native, arguments, instruction);
        }
        match native {
            Native::NodeDiagnosticsChannelCreate => return Ok(diagnostics_channel::channel()),
            Native::NodeDiagnosticsTracingChannel => {
                return Ok(diagnostics_channel::tracing_channel());
            }
            Native::NodeDiagnosticsPublish => return Ok(Value::Undefined),
            Native::NodeDiagnosticsRunStores => {
                let callback = arguments.get(1).cloned().unwrap_or(Value::Undefined);
                if !is_callable(&callback) {
                    return Err(ExecutionError::NotCallable {
                        instruction,
                        value: callback.type_name(),
                    });
                }
                let this_value = arguments.get(2).cloned().unwrap_or(Value::Undefined);
                let forwarded = arguments.iter().skip(3).cloned().collect();
                return self.call_value(&callback, this_value, forwarded, instruction);
            }
            Native::NodeUtilDebuglog
            | Native::NodeUtilDebugNoop
            | Native::NodeUtilDeprecate
            | Native::NodeUtilInspect
            | Native::NodeUtilFormat
            | Native::NodeUtilInherits => {
                return util::dispatch(native, arguments, |val, prop| {
                    self.read_property(val, prop, instruction)
                });
            }
            Native::ConsoleConstructor => {
                return Err(ExecutionError::TypeErrorMessage {
                    message: "Class constructor Console cannot be invoked without 'new'".to_owned(),
                });
            }
            Native::ConsoleLog
            | Native::ConsoleInfo
            | Native::ConsoleDebug
            | Native::ConsoleDir
            | Native::ConsoleTable => {
                return self.write_console_output(receiver, false, arguments, instruction);
            }
            Native::ConsoleError | Native::ConsoleWarn => {
                return self.write_console_output(receiver, true, arguments, instruction);
            }
            Native::ConsoleAssert => {
                let condition = arguments.first().map(|v| v.is_truthy()).unwrap_or(false);
                if !condition {
                    let message = if arguments.len() > 1 {
                        format!("Assertion failed: {}", node_util_format(&arguments[1..]))
                    } else {
                        "Assertion failed".to_owned()
                    };
                    return self.write_console_output(
                        receiver,
                        true,
                        &[Value::String(message.into())],
                        instruction,
                    );
                }
                return Ok(Value::Undefined);
            }
            Native::ConsoleTrace => {
                let message = if !arguments.is_empty() {
                    format!("Trace: {}", node_util_format(arguments))
                } else {
                    "Trace".to_owned()
                };
                return self.write_console_output(
                    receiver,
                    true,
                    &[Value::String(message.into())],
                    instruction,
                );
            }
            Native::ConsoleClear
            | Native::ConsoleCount
            | Native::ConsoleCountReset
            | Native::ConsoleGroup
            | Native::ConsoleGroupEnd
            | Native::ConsoleGroupCollapsed
            | Native::ConsoleTime
            | Native::ConsoleTimeEnd
            | Native::ConsoleTimeLog
            | Native::ConsoleDirxml
            | Native::ConsoleTimeStamp
            | Native::ConsoleProfile
            | Native::ConsoleProfileEnd
            | Native::NodeConsole => {
                return Ok(Value::Undefined);
            }
            Native::NodeAssertOk => return assert::ok(arguments),
            Native::NodeAsyncResourceBind => {
                let callback = arguments.first().cloned().unwrap_or(Value::Undefined);
                if !is_callable(&callback) {
                    return Err(ExecutionError::NotCallable {
                        instruction,
                        value: callback.type_name(),
                    });
                }
                return Ok(Value::Bound(Rc::new(BoundFunction {
                    target: callback,
                    this_value: arguments.get(1).cloned().unwrap_or(Value::Undefined),
                    arguments: Vec::new(),
                    properties: RefCell::new(ObjectData::default()),
                })));
            }
            Native::NodeAsyncResourceEmitDestroy => return Ok(Value::Undefined),
            Native::NodeCryptoRandomUuid
            | Native::NodeCryptoRandomBytes
            | Native::NodeCryptoRandomFillSync
            | Native::NodeCryptoGetHashes
            | Native::NodeCryptoGetRandomValues
            | Native::NodeCryptoTimingSafeEqual
            | Native::NodeCryptoCreateHash
            | Native::NodeCryptoCreateHmac
            | Native::NodeHashUpdate
            | Native::NodeHashDigest => {
                return self.crypto_dispatch(native, receiver, arguments, instruction);
            }
            Native::NodeStringDecoderWrite | Native::NodeStringDecoderEnd => {
                return string_decoder::dispatch(native, receiver, arguments);
            }
            Native::NodeStreamFinished => {
                let stream = arguments.first().cloned().unwrap_or(Value::Undefined);
                let callback = arguments
                    .iter()
                    .rev()
                    .find(|value| is_callable(value))
                    .cloned()
                    .unwrap_or(Value::Undefined);
                if !is_callable(&callback) {
                    return Err(ExecutionError::NotCallable {
                        instruction,
                        value: callback.type_name(),
                    });
                }
                let event = if self.property(&stream, "writableEnded").is_truthy() {
                    None
                } else {
                    Some(if self.property(&stream, "readableEnded").is_truthy() {
                        "end"
                    } else {
                        "finish"
                    })
                };
                if let Some(event) = event {
                    self.dispatch_node_event(
                        Native::NodeEventOnce,
                        &stream,
                        &[Value::String(event.to_owned().into()), callback],
                        instruction,
                    )?;
                } else {
                    self.call_value(&callback, Value::Undefined, Vec::new(), instruction)?;
                }
                return Ok(Value::Native(Native::NodeUtilDebugNoop, RealmId::PRIMARY));
            }
            Native::NodeStreamAddAbortSignal => {
                let stream = arguments.get(1).cloned().unwrap_or(Value::Undefined);
                // AbortSignal is deliberately semantic rather than a Tokio cancellation token:
                // the signal emits in the language realm and `destroy` decides what cancelling
                // this particular stream means. Providers only see cancellation once an adapter
                // translates the resulting resource action.
                return Ok(stream);
            }
            Native::NodeDnsLookup => {
                let (callback, callback_arguments) = dns::prepare_lookup(arguments, instruction)?;
                self.call_value(&callback, Value::Undefined, callback_arguments, instruction)?;
                return Ok(Value::Undefined);
            }
            Native::NodeOsNetworkInterfaces | Native::NodeOsHostname | Native::NodeOsFact(_) => {
                return os::dispatch(native);
            }
            Native::NodeTtyIsatty => return Ok(tty::isatty()),
            Native::NodeZlibCreateGzip
            | Native::NodeZlibCreateGunzip
            | Native::NodeZlibCreateDeflate
            | Native::NodeZlibCreateInflate
            | Native::NodeZlibCreateUnzip
            | Native::NodeZlibCreateBrotliDecompress
            | Native::NodeZlibCreateDecompress
            | Native::NodeZlibGzipSync
            | Native::NodeZlibGunzipSync
            | Native::NodeZlibDeflateSync
            | Native::NodeZlibInflateSync
            | Native::NodeZlibUnzipSync => {
                return self.zlib_dispatch(native, receiver, arguments, instruction);
            }
            Native::TextDecoderConstructor => {
                let label = arguments
                    .first()
                    .filter(|value| !matches!(value, Value::Undefined))
                    .map_or_else(|| "utf-8".to_owned(), Value::to_display_string);
                let encoding = encoding_rs::Encoding::for_label(label.trim().as_bytes())
                    .ok_or_else(|| ExecutionError::RangeErrorMessage {
                        message: format!("The encoding label provided ('{label}') is invalid"),
                    })?;
                let encoding_name = encoding.name().to_ascii_lowercase();
                let fatal = arguments
                    .get(1)
                    .and_then(|options| own_data_property(options, "fatal"))
                    .is_some_and(|v| v.is_truthy());
                let ignore_bom = arguments
                    .get(1)
                    .and_then(|options| own_data_property(options, "ignoreBOM"))
                    .is_some_and(|v| v.is_truthy());
                return Ok(ordinary_object([
                    ("encoding".to_owned(), Value::String(encoding_name.into())),
                    ("fatal".to_owned(), Value::Boolean(fatal)),
                    ("ignoreBOM".to_owned(), Value::Boolean(ignore_bom)),
                    (
                        "decode".to_owned(),
                        Value::Native(Native::TextDecoderDecode, RealmId::PRIMARY),
                    ),
                ]));
            }
            Native::TextDecoderDecode => {
                let bytes = match arguments.first() {
                    None | Some(Value::Undefined) => Vec::new(),
                    Some(Value::TypedArray(view)) => view.bytes(),
                    Some(Value::ArrayBuffer(buffer)) => buffer.bytes.borrow().clone(),
                    Some(other) => {
                        return Err(ExecutionError::TypeErrorMessage {
                            message: format!(
                                "The provided value is not of type ArrayBuffer or ArrayBufferView: {}",
                                other.type_name()
                            ),
                        });
                    }
                };
                let encoding_label = own_data_property(receiver, "encoding")
                    .and_then(|v| match v {
                        Value::String(s) => Some(s),
                        _ => None,
                    })
                    .unwrap_or_else(|| "utf-8".to_owned().into());
                let fatal = own_data_property(receiver, "fatal").is_some_and(|v| v.is_truthy());
                let ignore_bom =
                    own_data_property(receiver, "ignoreBOM").is_some_and(|v| v.is_truthy());
                let encoding =
                    encoding_rs::Encoding::for_label(encoding_label.to_lossy_string().as_bytes())
                        .unwrap_or(encoding_rs::UTF_8);
                let (decoded, had_errors) = if ignore_bom {
                    encoding.decode_without_bom_handling(&bytes)
                } else {
                    encoding.decode_with_bom_removal(&bytes)
                };
                if fatal && had_errors {
                    return Err(ExecutionError::TypeErrorMessage {
                        message: "The encoded data was not valid for encoding".to_owned(),
                    });
                }
                return Ok(Value::String(decoded.into_owned().into()));
            }
            Native::NodeHttpsCreateServer => {
                return Err(ExecutionError::Host {
                    instruction,
                    message:
                        "node:https requires a TLS host capability that this realm does not provide"
                            .to_owned(),
                });
            }
            Native::NodeHttp2CreateServer | Native::NodeHttp2CreateSecureServer => {
                return Err(ExecutionError::Host {
                    instruction,
                    message: "node:http2 requires an HTTP/2 host capability that this realm does not provide"
                        .to_owned(),
                });
            }
            Native::NodeModuleCreateRequire => {
                if let Some(loader) = self.commonjs_loader {
                    let importer = if let Some(value) = arguments.first() {
                        match value {
                            Value::String(value) => value.clone(),
                            _ => match self.read_property(value, "href", instruction)? {
                                Value::String(value) => value,
                                _ => {
                                    return Err(ExecutionError::TypeErrorMessage {
                                        message:
                                            "The argument must be a string or a file URL object"
                                                .to_owned(),
                                    });
                                }
                            },
                        }
                    } else {
                        self.module_path
                            .as_deref()
                            .map(str::to_owned)
                            .unwrap_or_default()
                            .into()
                    };
                    return unsafe {
                        (&mut *loader).create_require(&importer.to_lossy_string(), instruction)
                    };
                }
                let require = CommonJsRequire::new([]);
                return Ok(Value::CommonJsRequire(Rc::new(require)));
            }
            Native::AbortControllerAbort | Native::AbortSignalAbort => {
                let reason = arguments.first().cloned();
                let signal = if matches!(native, Native::AbortSignalAbort) {
                    let signal = abort_signal();
                    self.run_abort(&signal, reason, instruction)?;
                    signal
                } else {
                    let signal = self.read_property(receiver, "signal", instruction)?;
                    self.run_abort(&signal, reason, instruction)?;
                    return Ok(Value::Undefined);
                };
                return Ok(signal);
            }
            // A signal that aborts itself after a delay. The timer carries the signal as the
            // receiver of a bound `abort`, which is what lets a `Native` with no state of its
            // own settle a particular signal later.
            Native::AbortSignalTimeout => {
                let signal = abort_signal();
                let milliseconds = self.as_number_argument(arguments.first(), instruction)?;
                let reason = self.construct_error(
                    ErrorKind::Error,
                    &[Value::String(
                        "The operation was aborted due to timeout".to_owned().into(),
                    )],
                );
                if let Value::Object(object) = &reason {
                    object.borrow_mut().set_non_enumerable(
                        "name",
                        Value::String("TimeoutError".to_owned().into()),
                    );
                }
                let timer = Timer {
                    callback: Value::Bound(Rc::new(BoundFunction {
                        target: Value::Native(Native::AbortSignalSettle, RealmId::PRIMARY),
                        this_value: signal.clone(),
                        arguments: vec![reason],
                        properties: RefCell::new(ObjectData::default()),
                    })),
                    arguments: Vec::new(),
                    context: Rc::clone(&self.async_context),
                };
                // A timer like `setTimeout`'s, so 0 is node's 1 ms here too.
                self.with_realm_mut(|realm| {
                    let (delay, _) = realm.node_timer_delay(Some(milliseconds.max(0.0)));
                    realm.set_timeout(delay, timer)
                });
                return Ok(signal);
            }
            // `AbortSignal.any(signals)`: one signal that follows whichever of them settles
            // first, and is already aborted if any of them already is.
            Native::AbortSignalAny => {
                let signal = abort_signal();
                let sources = self
                    .drain_iterable(arguments.first().unwrap_or(&Value::Undefined), instruction)?;
                for source in sources {
                    if self
                        .read_property(&source, "aborted", instruction)?
                        .is_truthy()
                    {
                        let reason = self.read_property(&source, "reason", instruction)?;
                        self.run_abort(&signal, Some(reason), instruction)?;
                        return Ok(signal);
                    }
                    let follower = Value::Bound(Rc::new(BoundFunction {
                        target: Value::Native(Native::AbortSignalFollow, RealmId::PRIMARY),
                        this_value: signal.clone(),
                        arguments: vec![source.clone()],
                        properties: RefCell::new(ObjectData::default()),
                    }));
                    self.dispatch_native(
                        Native::AbortSignalListen { remove: false },
                        &source,
                        &[Value::String("abort".to_owned().into()), follower],
                        instruction,
                    )?;
                }
                return Ok(signal);
            }
            Native::AbortSignalSettle => {
                let reason = arguments.first().cloned();
                self.run_abort(receiver, reason, instruction)?;
                return Ok(Value::Undefined);
            }
            Native::AbortSignalFollow => {
                let source = arguments.first().cloned().unwrap_or(Value::Undefined);
                let reason = self.read_property(&source, "reason", instruction)?;
                self.run_abort(receiver, Some(reason), instruction)?;
                return Ok(Value::Undefined);
            }
            Native::AbortSignalListen { remove } => {
                // Only the "abort" type exists on a signal, so a listener for anything else is
                // registered nowhere rather than never called: there is no second event.
                if !matches!(arguments.first(), Some(Value::String(kind)) if kind == "abort") {
                    return Ok(Value::Undefined);
                }
                let listener = arguments.get(1).cloned().unwrap_or(Value::Undefined);
                let Value::Object(object) = receiver else {
                    return Ok(Value::Undefined);
                };
                let held = object.borrow().get_own(ABORT_LISTENERS).cloned();
                let Some(Value::Array(listeners)) = held else {
                    return Ok(Value::Undefined);
                };
                if remove {
                    listeners
                        .borrow_mut()
                        .retain(|held| !same_value(held, &listener));
                } else if is_callable(&listener)
                    && !listeners
                        .borrow()
                        .iter()
                        .any(|held| same_value(held, &listener))
                {
                    listeners.borrow_mut().push(listener);
                }
                return Ok(Value::Undefined);
            }
            Native::AbortSignalThrowIfAborted => {
                if self
                    .read_property(receiver, "aborted", instruction)?
                    .is_truthy()
                {
                    self.caught = Some(self.read_property(receiver, "reason", instruction)?);
                    return Err(ExecutionError::ThrownAcrossNative);
                }
                return Ok(Value::Undefined);
            }
            // Rust strings are already UTF-8, so the encoder is the identity on the bytes --
            // which is the whole of what `TextEncoder` is defined to do.
            Native::TextEncoderEncode => {
                let text = arguments
                    .first()
                    .map_or_else(String::new, Value::to_display_string);
                return Ok(buffer::plain_bytes_value(text.into_bytes()));
            }
            Native::NodeFsExistsSync
            | Native::NodeFsReadFileSync
            | Native::NodeFsMkdirSync
            | Native::NodeFsWriteFileSync
            | Native::NodeFsStatSync
            | Native::NodeFsLstatSync
            | Native::NodeFsFstatSync
            | Native::NodeFsReaddirSync
            | Native::NodeFsRmSync
            | Native::NodeFsUnlinkSync
            | Native::NodeFsRealpathSync
            | Native::NodeFsRenameSync
            | Native::NodeFsCopyFileSync
            | Native::NodeFsAccessSync
            | Native::NodeFsAppendFileSync
            | Native::NodeFsChmodSync
            | Native::NodeFsFchmodSync
            | Native::NodeFsOpenSync
            | Native::NodeFsCloseSync
            | Native::NodeFsReadlinkSync
            | Native::NodeFsRmdirSync
            | Native::NodeFsUtimesSync
            | Native::NodeFsFutimesSync
            | Native::NodeFsSync
            | Native::NodeFsWriteSync
            | Native::NodeFsFtruncateSync
            | Native::NodeFsFsyncSync => {
                return fs::dispatch(self, native, receiver, arguments, instruction);
            }
            Native::NodePathJoin
            | Native::NodePathResolve
            | Native::NodePathPart(_)
            | Native::NodePathRelative
            | Native::NodePathParse
            | Native::NodePathFormat
            | Native::NodePathIsAbsolute => {
                return path::dispatch(self, native, arguments, instruction);
            }
            Native::NodeWorkerConstructor => {
                return Err(ExecutionError::Host {
                    instruction,
                    message: "node:worker_threads requires an isolated realm worker capability"
                        .to_owned(),
                });
            }
            _ => {}
        }
        if matches!(
            native,
            Native::NodeEventOn
                | Native::NodeEventOnce
                | Native::NodeEventEmit
                | Native::NodeEventListeners
                | Native::NodeEventListenerCount
                | Native::NodeEventNames
                | Native::NodeEventRemoveListener
                | Native::NodeEventRemoveAllListeners
                | Native::NodeEventSetMaxListeners
        ) {
            return self.dispatch_node_event(native, receiver, arguments, instruction);
        }
        if matches!(
            native,
            Native::NodeNetCreateServer
                | Native::NodeNetConnect
                | Native::NodeSocketWrite
                | Native::NodeSocketEnd
                | Native::NodeSocketDestroy
        ) {
            return self.dispatch_node_net(native, receiver, arguments, instruction);
        }
        if matches!(
            native,
            Native::NodeWritableWrite | Native::NodeWritableEnd | Native::NodeWritableDestroy
        ) {
            return self.dispatch_memory_writable(native, receiver, arguments, instruction);
        }
        if matches!(
            native,
            Native::NodeReadablePush
                | Native::NodeReadableRead
                | Native::NodeReadableDestroy
                | Native::NodeReadablePause
                | Native::NodeReadableResume
                | Native::NodeReadableSetEncoding
        ) {
            return self.dispatch_node_readable(native, receiver, arguments, instruction);
        }
        if matches!(
            native,
            Native::NodeHttpServer
                | Native::NodeHttpCreateServer
                | Native::NodeHttpRequest
                | Native::NodeHttpGet
                | Native::NodeHttpCreateFetchServer
                | Native::NodeHttpFetchRespond
                | Native::NodeClientRequestWrite
                | Native::NodeClientRequestEnd
                | Native::NodeServerListen
                | Native::NodeServerClose
                | Native::NodeServerAddress
                | Native::NodeServerSetTimeout
                | Native::NodeResponseSetHeader
                | Native::NodeResponseGetHeader
                | Native::NodeResponseGetHeaders
                | Native::NodeResponseHasHeader
                | Native::NodeResponseRemoveHeader
                | Native::NodeResponseWriteHead
                | Native::NodeResponseWrite
                | Native::NodeResponseEnd
        ) {
            return self.dispatch_node_http(native, receiver, arguments, instruction);
        }
        // The same wrapper opening `call_native` does, applied before the machine's own arms so
        // that the two halves of a built-in cannot disagree about what its receiver is.
        if let Some(boxed) = receiver_for(native, receiver) {
            return self.dispatch_native(native, &boxed, arguments, instruction);
        }
        // A built-in that inspects a value's *shape* rather than its properties has no trap to
        // consult, so it looks through a proxy to what is being proxied. This is the honest
        // reading of "the trap is absent": the operation falls through to the target, which is
        // what every trapped operation does too.
        if matches!(arguments.first(), Some(Value::Proxy(_)))
            && matches!(native, Native::JsonStringify)
        {
            let mut unwrapped = arguments.to_vec();
            unwrapped[0] = unwrap_proxy(&arguments[0]).clone();
            return self.dispatch_native(native, receiver, &unwrapped, instruction);
        }
        // ToNumber and ToString on a reference *call* one of the value's own methods, and the
        // pure implementations below cannot: `argument_number` takes a `&[Value]` and has no
        // machine to run `valueOf` on, so it answered NaN for every object that defines one.
        // Reducing here, once, is what makes `Math.abs({valueOf: () => 42})` 42 -- and recursing
        // the way the proxy case above does keeps the machine out of `call_native`'s thirty-five
        // numeric call sites. The recursion terminates because a reduced argument is a primitive,
        // so the second pass finds nothing left to reduce.
        // A symbol has neither a number nor a string, and every built-in in that list asks for
        // one or the other -- so a symbol argument is a `TypeError` before any of them runs.
        // Similarly, BigInt refuses ToNumber, throwing TypeError unless the native is NumberConstructor
        // (which uses ToNumeric).
        if let Some(hint) = native.primitive_argument_hint(arguments.len())
            && arguments.iter().any(|argument| {
                matches!(
                    argument,
                    Value::Object(_) | Value::Function(_) | Value::Array(_)
                    // A `Temporal` value reduces by *calling*, and every one of its types
                    // refuses: `Number(date)` is a `TypeError`, where reading the value as it
                    // arrived answered NaN.
                    | Value::Temporal(_)
                )
            })
        {
            let mut reduced = Vec::with_capacity(arguments.len());
            for argument in arguments {
                // Left to right, because each reduction can run user code and the order in which
                // those calls happen is observable.
                let primitive = self.as_primitive(argument, hint, instruction)?;
                if hint == PrimitiveHint::Number
                    && native != Native::NumberConstructor
                    && matches!(primitive, Value::BigInt(_))
                {
                    return Err(ExecutionError::BigIntIsNotANumber);
                }
                if matches!(primitive, Value::Symbol(_)) {
                    return Err(ExecutionError::SymbolIsNotANumber);
                }
                reduced.push(primitive);
            }
            return self.dispatch_native(native, receiver, &reduced, instruction);
        }
        if let Some(hint) = native.primitive_argument_hint(arguments.len()) {
            for argument in arguments {
                if hint == PrimitiveHint::Number
                    && native != Native::NumberConstructor
                    && matches!(argument, Value::BigInt(_))
                {
                    return Err(ExecutionError::BigIntIsNotANumber);
                }
                if matches!(argument, Value::Symbol(_)) {
                    return Err(ExecutionError::SymbolIsNotANumber);
                }
            }
        }
        // `CanBeHeldWeakly`: an object or an *unregistered* symbol, and nothing else. A weak
        // collection that took a number held something it could never observe being collected,
        // so `new WeakSet([1])` built a set whose member no reference could ever match. The
        // predicate reads the symbol registry, which the machine-free table has no access to,
        // so the refusal is made here.
        if matches!(native, Native::WeakSetAdd | Native::WeakMapSet)
            && !self.can_be_held_weakly(arguments.first().unwrap_or(&Value::Undefined))
        {
            return Err(ExecutionError::TypeErrorMessage {
                message: format!(
                    "{} cannot be held weakly: a weak collection takes an object or an \
                     unregistered symbol",
                    arguments
                        .first()
                        .unwrap_or(&Value::Undefined)
                        .to_display_string()
                ),
            });
        }
        // Each of these begins with `ToObject` of its first argument, and that step throws for
        // `null` and `undefined`. The rest of the coercion is invisible here: a primitive has
        // exactly the own properties its wrapper would, and the pure implementations already
        // read them off the primitive itself.
        if matches!(
            arguments.first(),
            None | Some(Value::Undefined | Value::Null)
        ) && matches!(
            native,
            Native::ObjectKeys
                | Native::ObjectValues
                | Native::ObjectEntries
                | Native::ObjectOwnNames
                | Native::ObjectGetOwnPropertySymbols
                | Native::ObjectGetOwnPropertyDescriptor
                | Native::ObjectGetOwnPropertyDescriptors
                | Native::ObjectGetPrototypeOf
                | Native::ObjectSetPrototypeOf
                | Native::ObjectAssign
        ) {
            return Err(ExecutionError::TypeErrorMessage {
                message: "Cannot convert undefined or null to object".to_owned(),
            });
        }
        // Each of the `Promise` statics reads `this` as the constructor to build its result
        // with -- `NewPromiseCapability(C)` begins `If IsConstructor(C) is false, throw a
        // TypeError` -- so one that is not a constructor is refused before any of them looks at
        // its argument. This realm always builds an ordinary promise, but a program that
        // redirected one of these onto something it cannot `new` has to be told so, and it has
        // to be told *synchronously*: the refusal happens before the capability exists, so it is
        // the one failure in a combinator that is not a rejection.
        //
        // `IsConstructor` rather than "is an object": `Promise.all.call(eval)` is a call on a
        // built-in function, which is an object and is not something `new` can reach.
        if matches!(
            native,
            Native::PromiseResolve
                | Native::PromiseReject
                | Native::PromiseAll
                | Native::PromiseAllSettled
                | Native::PromiseRace
                | Native::PromiseAny
        ) && !is_constructor(receiver)
        {
            return Err(ExecutionError::TypeErrorMessage {
                message: "PromiseCapability constructor is not a constructor".to_owned(),
            });
        }
        // Five `String.prototype` methods hand the whole call to their argument when the
        // argument knows how to answer it. That is not a shortcut -- it is how the specification
        // defines them, and it is the entire reason `RegExp.prototype[@@split]` and its four
        // siblings exist: a regexp performs the split, and the string method is the front door.
        // A program that puts a `@@replace` on an object of its own gets the same treatment,
        // which is what makes the protocol worth having.
        //
        // The lookup happens after `RequireObjectCoercible(this)` and *before* `ToString(this)`,
        // an order a test can see through a `toString` that counts its calls.
        // Held alive for the borrow below: the coerced receiver has to outlive the block that
        // builds it, because the rest of this function reads through the reference.
        let coerced_receiver;
        let receiver = if let Some(symbol) = string_delegating_symbol(native) {
            if receiver.is_nullish() {
                return Err(ExecutionError::TypeErrorMessage {
                    message: format!(
                        "String.prototype method called on {}",
                        receiver.to_display_string()
                    ),
                });
            }
            let searched = arguments.first().cloned().unwrap_or(Value::Undefined);
            // Only an Object is asked. The specification looks the symbol up on `regexp` after
            // establishing it is one, and the difference is observable through an accessor: a
            // getter on `Number.prototype[@@match]` must not run for `"a1b".match(1)`.
            //
            // This read as correct for as long as a primitive answered nothing off its chain,
            // which was itself a bug -- so fixing that one exposed this one. A `nullish` test
            // was never the same question: it lets every other primitive through.
            if is_object_type(&searched) {
                // `replaceAll` and `matchAll` refuse a regexp that is not global, because both
                // are defined to visit every match and a non-global regexp would visit the
                // first one forever. The check is on the *flags property*, not on an internal
                // slot, so an object pretending to be a regexp is held to it too.
                if matches!(native, Native::StringReplaceAll | Native::StringMatchAll)
                    && self.is_regexp(&searched, instruction)?
                {
                    let flags = self.read_property(&searched, "flags", instruction)?;
                    if flags.is_nullish() {
                        return Err(ExecutionError::TypeErrorMessage {
                            message: "Cannot convert undefined or null to object".to_owned(),
                        });
                    }
                    if !self.as_text(&flags, instruction)?.contains('g') {
                        return Err(ExecutionError::TypeErrorMessage {
                            message: format!(
                                "{} must be called with a global RegExp",
                                if matches!(native, Native::StringMatchAll) {
                                    "matchAll"
                                } else {
                                    "replaceAll"
                                }
                            ),
                        });
                    }
                }
                let method = self.read_property(&searched, symbol.property_key(), instruction)?;
                if !method.is_nullish() {
                    if !is_callable(&method) {
                        return Err(ExecutionError::NotCallable {
                            instruction,
                            value: method.type_name(),
                        });
                    }
                    let mut forwarded = vec![receiver.clone()];
                    forwarded.extend(arguments.iter().skip(1).cloned());
                    return self.call_value(&method, searched, forwarded, instruction);
                }
            }
            // Nothing answered, so this method does the work itself -- and the receiver is
            // coerced *here* rather than by the generic string preamble below, which would
            // re-enter this function and consult the symbol a second time. A test that counts
            // the getter's reads can see that.
            if matches!(receiver, Value::String(_)) {
                receiver
            } else {
                let primitive = self.as_primitive(receiver, PrimitiveHint::String, instruction)?;
                if matches!(primitive, Value::Symbol(_)) {
                    return Err(ExecutionError::SymbolIsNotAString);
                }
                coerced_receiver = Value::String(primitive.to_display_string().into());
                &coerced_receiver
            }
        } else {
            receiver
        };
        // `String.prototype.toUpperCase.call(1)` is `"1".toUpperCase()`, and
        // `.call(null)` is a `TypeError` -- both of which happen before the method looks at its
        // receiver at all. Reaching this with something that is not a string is the ordinary way
        // these are used, not a mistake: every one of them is generic.
        if native.walks_a_string() && !matches!(receiver, Value::String(_)) {
            if receiver.is_nullish() {
                return Err(ExecutionError::TypeErrorMessage {
                    message: format!(
                        "String.prototype method called on {}",
                        receiver.to_display_string()
                    ),
                });
            }
            let primitive = self.as_primitive(receiver, PrimitiveHint::String, instruction)?;
            // A symbol is the one primitive `ToString` refuses: it has a description, not a
            // spelling, and reading one as text is the mistake `String(symbol)` exists for.
            if matches!(primitive, Value::Symbol(_)) {
                return Err(ExecutionError::SymbolIsNotAString);
            }
            let coerced = Value::String(primitive.to_display_string().into());
            return self.dispatch_native(native, &coerced, arguments, instruction);
        }
        // `ToPropertyKey` for every built-in that names a property with an argument. The
        // conversion is the program's own `Symbol.toPrimitive`/`valueOf`/`toString`, so the pure
        // table cannot run it: it read every object as `"[object Object]"` and asked about the
        // wrong property, silently. Done once here rather than in each built-in, and the
        // re-dispatch cannot loop because what it puts back is a primitive.
        let keyed_argument = match native {
            Native::ObjectDefineProperty
            | Native::ReflectDefineProperty
            | Native::ObjectGetOwnPropertyDescriptor
            | Native::ReflectGetOwnPropertyDescriptor
            | Native::ObjectHasOwn
            | Native::ReflectHas
            | Native::ReflectGet
            | Native::ReflectSet
            | Native::ReflectDeleteProperty => Some(1),
            // These two take the property on the *receiver*, so the key is their first argument.
            Native::ObjectHasOwnProperty | Native::ObjectPropertyIsEnumerable => Some(0),
            _ => None,
        };
        if let Some(index) = keyed_argument
            && let Some(key) = arguments.get(index)
            && is_reflect_target(key)
        {
            let coerced = self.computed_key(key, instruction)?;
            let mut converted = arguments.to_vec();
            converted[index] = coerced;
            return self.dispatch_native(native, receiver, &converted, instruction);
        }
        // `ToString` for every argument a built-in reads as text, on the same terms and in the
        // same place as the numeric pass above: after the receiver, because that is the order a
        // `toString` counting its calls can see.
        let textual = textual_arguments(native);
        if !textual.is_empty()
            && textual
                .iter()
                .any(|&index| arguments.get(index).is_some_and(needs_text_conversion))
        {
            let mut converted = arguments.to_vec();
            for &index in textual {
                let Some(argument) = converted.get(index) else {
                    continue;
                };
                if !needs_text_conversion(argument) {
                    continue;
                }
                let text = self.as_text(&argument.clone(), instruction)?;
                converted[index] = Value::String(text.into());
            }
            return self.dispatch_native(native, receiver, &converted, instruction);
        }
        // `ToNumber` for every argument a built-in reads as a number. Done once here, after the
        // receiver has been validated and coerced -- which is the order the specification uses,
        // and which a `valueOf` that counts its calls can see -- rather than inside fifty
        // built-ins that have no machine to run the conversion with. The re-dispatch cannot
        // loop, because what it puts back is a Number.
        let numeric = numeric_arguments(native);
        if !numeric.is_empty()
            && numeric
                .iter()
                .any(|&index| arguments.get(index).is_some_and(needs_number_conversion))
        {
            let mut converted = arguments.to_vec();
            for &index in numeric {
                let Some(argument) = converted.get(index) else {
                    continue;
                };
                if !needs_number_conversion(argument) {
                    continue;
                }
                let number = self.as_number_argument(Some(&argument.clone()), instruction)?;
                converted[index] = Value::Number(number);
            }
            return self.dispatch_native(native, receiver, &converted, instruction);
        }
        // A prototype is stored rather than read, so the only two things it may be are an object
        // and the explicit absence of one.
        if matches!(native, Native::ObjectSetPrototypeOf) {
            let prototype = arguments.get(1).unwrap_or(&Value::Undefined);
            if !matches!(prototype, Value::Null) && !is_an_object(prototype) {
                return Err(ExecutionError::TypeErrorMessage {
                    message: "Object prototype may only be an Object or null".to_owned(),
                });
            }
        }
        match native {
            Native::ProcessGetBuiltinModule => match arguments.first() {
                Some(Value::String(name))
                    if kiln_builtins::is_builtin_specifier(&name.to_lossy_string()) =>
                {
                    if let Some(loader) = self.commonjs_loader {
                        return unsafe {
                            (&mut *loader).get_builtin(&name.to_lossy_string(), instruction)
                        }
                        .map(|value| value.unwrap_or(Value::Undefined));
                    }
                    Err(ExecutionError::TypeErrorMessage {
                        message: format!(
                            "node:{} is graph-materialized; process.getBuiltinModule requires a graph loader",
                            name.to_lossy_string()
                        ),
                    })
                }
                Some(Value::String(_)) => Ok(Value::Undefined),
                _ => Ok(Value::Undefined),
            },
            Native::NodeStatIsFile
            | Native::NodeStatIsDirectory
            | Native::NodeStatIsSymbolicLink => {
                let Value::Object(stats) = receiver else {
                    return Err(ExecutionError::TypeErrorMessage {
                        message: "Stats method called on an incompatible receiver".to_owned(),
                    });
                };
                let key = match native {
                    Native::NodeStatIsFile => "@@kiln:node:is-file",
                    Native::NodeStatIsDirectory => "@@kiln:node:is-directory",
                    _ => "@@kiln:node:is-symlink",
                };
                Ok(stats
                    .borrow()
                    .get_own(key)
                    .cloned()
                    .unwrap_or(Value::Boolean(false)))
            }
            Native::NodeBufferFrom
            | Native::NodeBufferAlloc
            | Native::NodeBufferIsBuffer
            | Native::NodeBufferByteLength
            | Native::NodeBufferConcat
            | Native::NodeBufferToString => buffer::dispatch(native, receiver, arguments),
            Native::NodeQueryStringParse => querystring::dispatch(native, arguments),
            // The bridges the shared TypeScript `URL` is built on (`kiln-builtins/src/url.ts`),
            // over `kiln-url`, the record the native provider spills too. There is no request
            // fast path here and nothing to adopt: the class lives in the realm's globals.
            Native::UrlParseRaw => {
                let input = arguments
                    .first()
                    .map_or_else(String::new, Value::to_display_string);
                let base = match arguments.get(1) {
                    None | Some(Value::Undefined) => None,
                    Some(value) => Some(value.to_display_string()),
                };
                match kiln_url::record(&input, base.as_deref()) {
                    Ok(record) => Ok(Value::String(record.into())),
                    Err(_) => Err(ExecutionError::TypeErrorMessage {
                        message: "Invalid URL".to_owned(),
                    }),
                }
            }
            Native::UrlSetRaw => {
                let href = arguments
                    .first()
                    .map_or_else(String::new, Value::to_display_string);
                let field = arguments.get(1).map_or(f64::NAN, Value::as_number);
                let value = arguments
                    .get(2)
                    .map_or_else(String::new, Value::to_display_string);
                if !(0.0..=9.0).contains(&field) || field.fract() != 0.0 {
                    return Err(ExecutionError::TypeErrorMessage {
                        message: "Invalid URL setter".to_owned(),
                    });
                }
                match kiln_url::set(&href, field as u32, &value) {
                    Ok(record) => Ok(Value::String(record.into())),
                    Err(_) => Err(ExecutionError::TypeErrorMessage {
                        message: "Invalid URL".to_owned(),
                    }),
                }
            }
            Native::UrlFastRaw | Native::UrlAdoptRaw => Ok(Value::Undefined),
            Native::UrlConstructor
            | Native::NodeUrlParse
            | Native::NodeUrlFileUrlToPath
            | Native::NodeUrlPathToFileUrl
            | Native::UrlToString
            | Native::UrlSearchParamsGet
            | Native::UrlSearchParamsHas
            | Native::UrlSearchParamsGetAll
            | Native::UrlSearchParamsToString => url::dispatch(native, receiver, arguments),
            Native::ArrayFrom => self.array_from(receiver, arguments, instruction),
            Native::ArrayBufferConstructor => {
                self.construct_array_buffer(false, arguments, instruction)
            }
            Native::SharedArrayBufferConstructor => {
                self.construct_array_buffer(true, arguments, instruction)
            }
            Native::DataViewConstructor => self.construct_data_view(arguments, instruction),
            Native::DataViewGet(kind) => self.data_view_get(kind, receiver, arguments, instruction),
            Native::DataViewSet(kind) => self.data_view_set(kind, receiver, arguments, instruction),
            Native::DataViewGetBig { signed } => {
                self.data_view_get_big(signed, receiver, arguments, instruction)
            }
            Native::DataViewSetBig { .. } => {
                self.data_view_set_big(receiver, arguments, instruction)
            }
            Native::IteratorLazy(kind) => {
                self.iterator_lazy(kind, receiver, arguments, instruction)
            }
            Native::IteratorTerminal(kind) => {
                self.iterator_terminal(kind, receiver, arguments, instruction)
            }
            Native::IteratorHelperNext(kind) => {
                self.iterator_helper_next(kind, receiver, instruction)
            }
            Native::IteratorHelperReturn => self.iterator_helper_return(receiver, instruction),
            // Built on `Array.from`, so an iterable, an array-like and a mapping function all
            // behave the same way they do there.
            // `%TypedArray%.from` and `.of` build through `this`, not through the kind whose
            // name they were reached by: `Int8Array.from.call(Uint8Array, [1])` is a Uint8Array,
            // and `Int8Array.of.call({}, 1)` is a `TypeError` because a plain object is not a
            // constructor. Both baked the kind in and ignored the receiver, so the first answered
            // an Int8Array and the second answered an ordinary object.
            Native::TypedArrayFrom(kind) => {
                // The mapper is checked before the source is read, and then held back: the view
                // is built first and each value passes through the mapper as it is written. The
                // collection below therefore asks for the values *unmapped*, which is why the
                // source is handed on alone.
                let mapper = match arguments.get(1) {
                    None | Some(Value::Undefined) => None,
                    Some(value) if is_callable(value) => Some((
                        value.clone(),
                        arguments.get(2).cloned().unwrap_or(Value::Undefined),
                    )),
                    Some(value) => {
                        return Err(ExecutionError::NotCallable {
                            instruction,
                            value: value.type_name(),
                        });
                    }
                };
                let source = [arguments.first().cloned().unwrap_or(Value::Undefined)];
                let collected = self.array_from(&Value::Undefined, &source, instruction)?;
                let Value::Array(items) = &collected else {
                    return construct_typed_array(kind, &[collected]);
                };
                let elements = dense_items(&items.borrow());
                self.typed_array_through(receiver, kind, elements, mapper, instruction)
            }
            Native::TypedArrayOf(kind) => {
                self.typed_array_through(receiver, kind, arguments.to_vec(), None, instruction)
            }
            // `Math.random` advances state that lives on the machine, so it cannot be a pure
            // function of its arguments either.
            Native::MathRandom => Ok(Value::Number(self.random_double(instruction)?)),
            // The scheduling built-ins are the event loop's only entry points, and the loop lives
            // on the machine, so they cannot be pure functions of their arguments.
            Native::SetTimeout
            | Native::SetInterval
            | Native::SetImmediate
            | Native::QueueIoRaw => {
                // An immediate and an I/O callback have no delay, so their arguments start one
                // earlier: `setImmediate(fn, x)` calls `fn(x)`.
                let undelayed = matches!(native, Native::SetImmediate | Native::QueueIoRaw);
                let (delay, warning) = if undelayed {
                    (std::time::Duration::ZERO, None)
                } else {
                    let requested = match arguments.get(1) {
                        None | Some(Value::Undefined) => None,
                        Some(delay) => Some(delay.as_number()),
                    };
                    self.with_realm_mut(|realm| realm.node_timer_delay(requested))
                };
                if let Some(warning) = warning {
                    let text = Value::String(warning.trim_end_matches('\n').to_owned().into());
                    self.host
                        .call(HostOp::WriteStderr, &[HostValue::from(text)])
                        .map_err(|error| ExecutionError::Host {
                            instruction,
                            message: error.to_string(),
                        })?;
                }
                let timer = Timer {
                    callback: arguments.first().cloned().unwrap_or(Value::Undefined),
                    // Everything after the delay is handed to the callback, which is how a
                    // timer carries state without a closure.
                    arguments: arguments
                        .get(if undelayed { 1.. } else { 2.. })
                        .unwrap_or_default()
                        .to_vec(),
                    // Captured where the timer was set, not where it fires.
                    context: Rc::clone(&self.async_context),
                };
                let id = self.with_realm_mut(|realm| match native {
                    Native::SetInterval => realm.set_interval(delay, timer),
                    Native::SetImmediate => realm.set_immediate(timer),
                    Native::QueueIoRaw => realm.queue_io(timer),
                    _ => realm.set_timeout(delay, timer),
                });
                Ok(Value::Number(id.get() as f64))
            }
            Native::ClearTimeout | Native::ClearInterval | Native::ClearImmediate => {
                let id = arguments.first().map_or(f64::NAN, Value::as_number);
                if id.is_finite() && id >= 1.0 && id.fract() == 0.0 && id <= u64::MAX as f64 {
                    self.with_realm_mut(|realm| realm.clear_timeout(TimerId::new(id as u64)));
                }
                Ok(Value::Undefined)
            }
            Native::PromiseResolve => {
                // Already a promise *of this constructor*? Hand it straight back; wrapping would
                // add a tick for nothing and break `Promise.resolve(p) === p`. A promise of a
                // different constructor is wrapped, which is what makes `P.resolve(plain)` a `P`.
                let value = arguments.first().cloned().unwrap_or(Value::Undefined);
                if !is_an_object(receiver) {
                    return Err(ExecutionError::TypeErrorMessage {
                        message: "Promise.resolve called on a non-object".to_owned(),
                    });
                }
                if matches!(value, Value::Promise(_)) {
                    let constructor = self.read_property(&value, "constructor", instruction)?;
                    if same_value(&constructor, receiver) {
                        return Ok(value);
                    }
                }
                if !Self::reached_through_intrinsic_promise(receiver) {
                    let (promise, resolve, _) =
                        self.new_promise_capability(receiver, instruction)?;
                    self.call_value(&resolve, Value::Undefined, vec![value], instruction)?;
                    return Ok(promise);
                }
                let promise = PromiseState::pending();
                self.resolve_promise(&promise, value);
                Ok(Value::Promise(promise))
            }
            Native::PromiseReject => {
                let reason = arguments.first().cloned().unwrap_or(Value::Undefined);
                if !is_an_object(receiver) {
                    return Err(ExecutionError::TypeErrorMessage {
                        message: "Promise.reject called on a non-object".to_owned(),
                    });
                }
                if !Self::reached_through_intrinsic_promise(receiver) {
                    let (promise, _, reject) =
                        self.new_promise_capability(receiver, instruction)?;
                    self.call_value(&reject, Value::Undefined, vec![reason], instruction)?;
                    return Ok(promise);
                }
                let promise = PromiseState::pending();
                self.settle(&promise, &Settled::Rejected(reason));
                Ok(Value::Promise(promise))
            }
            // The pure spelling builds the pair; the constructor it was reached through decides
            // what kind of promise is in it, and only the machine can see the receiver's chain.
            Native::PromiseWithResolvers => {
                if !is_an_object(receiver) {
                    return Err(ExecutionError::TypeErrorMessage {
                        message: "Promise.withResolvers called on a non-object".to_owned(),
                    });
                }
                if !Self::reached_through_intrinsic_promise(receiver) {
                    let (promise, resolve, reject) =
                        self.new_promise_capability(receiver, instruction)?;
                    let mut result = ObjectData::default();
                    result.set("promise", promise);
                    result.set("resolve", resolve);
                    result.set("reject", reject);
                    return Ok(Value::Object(Rc::new(RefCell::new(result))));
                }
                call_native(
                    &self.intrinsics,
                    Native::PromiseWithResolvers,
                    receiver,
                    arguments,
                )
            }
            Native::PromiseThen => {
                let Value::Promise(promise) = receiver else {
                    return Err(ExecutionError::NotCallable {
                        instruction,
                        value: receiver.type_name(),
                    });
                };
                let promise = Rc::clone(promise);
                let species = self.promise_species_prototype(receiver, instruction)?;
                let derived = PromiseState::pending();
                self.react(
                    &promise,
                    arguments.first().cloned().unwrap_or(Value::Undefined),
                    arguments.get(1).cloned().unwrap_or(Value::Undefined),
                    Sink::Promise(Rc::clone(&derived)),
                    false,
                );
                Ok(Self::as_promise_of(Value::Promise(derived), species))
            }
            // `catch(r)` is `Invoke(this, "then", [undefined, r])` -- literally, including the
            // read. Calling the intrinsic `then` directly meant a subclass that overrides `then`,
            // and every thenable a library hands round in place of a promise, had its own `then`
            // skipped: `Promise.prototype.catch.call(thenable, f)` refused the receiver outright.
            Native::PromiseCatch => {
                let method = self.read_property(receiver, "then", instruction)?;
                if !is_callable(&method) {
                    return Err(ExecutionError::TypeErrorMessage {
                        message: format!("{} is not a function", method.to_display_string()),
                    });
                }
                let on_rejected = arguments.first().cloned().unwrap_or(Value::Undefined);
                self.call_value(
                    &method,
                    receiver.clone(),
                    vec![Value::Undefined, on_rejected],
                    instruction,
                )
            }
            // `finally(f)` runs `f` either way and passes the outcome through untouched, which
            // is what separates it from `then(f, f)`.
            Native::PromiseFinally => {
                let Value::Promise(promise) = receiver else {
                    return Err(ExecutionError::NotCallable {
                        instruction,
                        value: receiver.type_name(),
                    });
                };
                let promise = Rc::clone(promise);
                let handler = arguments.first().cloned().unwrap_or(Value::Undefined);
                let species = self.promise_species_prototype(receiver, instruction)?;
                let derived = PromiseState::pending();
                self.react(
                    &promise,
                    handler.clone(),
                    handler,
                    Sink::Promise(Rc::clone(&derived)),
                    true,
                );
                Ok(Self::as_promise_of(Value::Promise(derived), species))
            }
            // `all`, `allSettled`, `race` and `any` differ only in when they settle and what
            // they collect, so they share one walk over the inputs.
            Native::PromiseAll
            | Native::PromiseAllSettled
            | Native::PromiseRace
            | Native::PromiseAny => {
                let source = arguments.first().cloned().unwrap_or(Value::Undefined);
                // `NewPromiseCapability(C)` is step 2, and it is deliberately *outside* the
                // `IfAbruptRejectPromise` below: with no capability there is no promise to
                // reject with, so `Promise.all.call(null, [])` throws where
                // `Promise.all(null)` rejects.
                let (promise, resolve, reject) =
                    self.new_promise_capability(receiver, instruction)?;
                // `IfAbruptRejectPromise` for everything after it. A combinator reports an
                // argument that is not iterable, a `Symbol.iterator` that throws, a `next` that
                // does, a `resolve` that is not callable -- all of it -- by rejecting the
                // promise it already built. Throwing instead is a different shape of answer:
                // `Promise.all(null).catch(f)` would never reach `f`.
                //
                // The handler is installed by hand rather than through `catching`, which turns
                // *every* failure into a value: a step limit would become a rejected promise
                // holding its own message, and a runaway walk would look like a program that
                // merely failed. Machine failures still propagate; only what a program could
                // have caught becomes a rejection.
                let installed = self.handlers.len();
                self.handlers.push(Handler {
                    target: HOST_HANDLER,
                    frame_depth: self.frames.len(),
                    stack_height: self.stack.len(),
                    native_depth: self.native_depth,
                });
                let walked = self.perform_promise_combinator(
                    native,
                    receiver,
                    &source,
                    &resolve,
                    &reject,
                    instruction,
                );
                self.handlers.truncate(installed);
                let reason = match walked {
                    Ok(()) => return Ok(promise),
                    Err(ExecutionError::CaughtByHost) => {
                        self.caught.take().unwrap_or(Value::Undefined)
                    }
                    // Not something a program threw: `thrown_value` hands a machine failure back.
                    Err(error) => self.thrown_value(error)?,
                };
                self.call_value(&reject, Value::Undefined, vec![reason], instruction)?;
                Ok(promise)
            }
            // `Object.getOwnPropertySymbols`: the symbol-keyed own properties, and only those.
            //
            // It answered `[]` for everything. The properties were there and read back
            // correctly -- a symbol key is a reserved *string* here -- but nothing could
            // recover the symbol from the key, so every faithful-copy routine (deep clone, deep
            // merge, a decorator re-wrapping an object, `Object.assign` over `Reflect.ownKeys`)
            // silently dropped symbol-keyed data instead of failing.
            Native::ObjectGetOwnPropertySymbols => {
                let source = arguments.first().cloned().unwrap_or(Value::Undefined);
                if !is_an_object(&source) {
                    return Ok(Value::Array(ArrayData::new(Vec::new())));
                }
                let symbols = self
                    .own_keys_reflected(&source, instruction)?
                    .into_iter()
                    .filter(|key| matches!(key, Value::Symbol(_)))
                    .collect();
                Ok(Value::Array(ArrayData::new(symbols)))
            }
            // `Error("x")` is `new Error("x")`: every error constructor answers the same way
            // called or constructed, which is why `throw Error(m)` is as common in real code as
            // `throw new Error(m)`. Reaching the pure table instead answered `undefined`, and
            // the `TypeError` a program then got was about reading `.message` of nothing.
            Native::Error(kind) => self.construct_error_reached(kind, arguments, instruction),
            // `Error.prototype.toString` reads `name` and `message` off the receiver, and a
            // read is a *call* when either is an accessor -- `class G extends Error { get
            // name() { return "G"; } }` is the ordinary way a library names its error kind
            // without an own property. The pure spelling walks the store data-only and answered
            // "Error" for exactly that class.
            Native::ErrorToString => {
                if !is_an_object(receiver) {
                    return Err(ExecutionError::TypeErrorMessage {
                        message: "Error.prototype.toString called on non-object".to_owned(),
                    });
                }
                let read = |vm: &mut Self, key: &str| -> Result<String, ExecutionError> {
                    Ok(match vm.read_property(receiver, key, instruction)? {
                        Value::Undefined => String::new(),
                        other => other.to_display_string(),
                    })
                };
                let named = self.read_property(receiver, "name", instruction)?;
                let name = match named {
                    Value::Undefined => "Error".to_owned(),
                    other => other.to_display_string(),
                };
                let message = read(self, "message")?;
                // Either half being empty drops the separator with it, so a message-less error
                // is "Error" rather than "Error: ".
                Ok(Value::String(if name.is_empty() {
                    message.into()
                } else if message.is_empty() {
                    name.into()
                } else {
                    format!("{name}: {message}").into()
                }))
            }
            Native::StringConstructor => {
                let Some(value) = arguments.first() else {
                    return Ok(Value::String(String::new().into()));
                };
                // A string hint, because that is what `String(x)` asks for -- so an object with
                // both `valueOf` and `toString` uses the second.
                let primitive = self.as_primitive(value, PrimitiveHint::String, instruction)?;
                Ok(Value::String(primitive.to_display_string().into()))
            }
            // `JSON.parse(text, reviver)`. The reviver is called on every value the text
            // produced, deepest first, with the holder as its `this` -- which is how a program
            // turns `{"at":"2020-01-01"}` into an object holding a `Date`. Without it the
            // second argument was read and discarded, and the parse looked like it had worked.
            Native::JsonParse => {
                let text = self
                    .as_text_units(arguments.first().unwrap_or(&Value::Undefined), instruction)?;
                let parsed = parse_json_text(&text)?;
                let reviver = arguments.get(1).cloned().unwrap_or(Value::Undefined);
                if !is_callable(&reviver) {
                    return Ok(parsed);
                }
                // The specification's root holder: an ordinary object whose one property is
                // named by the empty string. The reviver is called for it too, last of all,
                // which is what lets a reviver replace the whole document.
                let mut holder = ObjectData::default();
                holder.set("", parsed);
                let root = Value::Object(Rc::new(RefCell::new(holder)));
                self.internalize_json_property(
                    &root,
                    &StringUnits::default(),
                    &reviver,
                    instruction,
                )
            }
            Native::JsonStringify => {
                let indent = json_indent(arguments.get(2));
                let value = arguments.first().cloned().unwrap_or(Value::Undefined);
                let mut seen = Vec::new();
                // `SerializeJSONProperty` applies the replacer to every property, including the
                // root, which it reaches through a wrapper holder keyed by the empty string.
                // Two forms: a function, called as `(holder, key, value)`, and an array, which
                // is a list of the keys to keep.
                let replacer = arguments.get(1).cloned().unwrap_or(Value::Undefined);
                let allowed: Option<Vec<StringUnits>> = match &replacer {
                    Value::Array(items) => {
                        let mut keys: Vec<StringUnits> = Vec::new();
                        for entry in items.borrow().iter() {
                            // A `String` or `Number` *wrapper* counts as its primitive, which the
                            // specification says explicitly and which matching the primitive
                            // forms alone missed: `[new String("a")]` selected nothing.
                            let name = match entry {
                                Value::String(text) => Some(text.clone()),
                                Value::Number(number) => {
                                    Some(StringUnits::from(number_string(*number)))
                                }
                                Value::Object(object) => {
                                    let inner = object.borrow().primitive().cloned();
                                    match inner {
                                        Some(Value::String(text)) => Some(text),
                                        Some(Value::Number(number)) => {
                                            Some(StringUnits::from(number_string(number)))
                                        }
                                        _ => None,
                                    }
                                }
                                _ => None,
                            };
                            // The list is a set in list order: a key repeated in the replacer
                            // appears once, where it first appeared.
                            if let Some(name) = name
                                && !keys.contains(&name)
                            {
                                keys.push(name);
                            }
                        }
                        Some(keys)
                    }
                    _ => None,
                };
                let function = is_callable(&replacer).then_some(replacer);
                let mut resolve = |call: JsonCall<'_>| match call {
                    JsonCall::Getter(getter, receiver) => {
                        self.call_value(getter, receiver.clone(), Vec::new(), instruction)
                    }
                    JsonCall::Replace(holder, key, property) => match &function {
                        Some(replacer) => self.call_value(
                            replacer,
                            holder.clone(),
                            Vec::from([Value::String(key.clone()), property]),
                            instruction,
                        ),
                        None => Ok(property),
                    },
                    JsonCall::ToJson(receiver, key) => {
                        // Only a reference value can carry one, and asking a primitive would
                        // reach the string and number method tables for nothing. A `Temporal`
                        // value carries a `toJSON` and it is the whole reason the step exists:
                        // without it `JSON.stringify(plainDate)` answered `{}`.
                        // `SerializeJSONProperty` asks for `toJSON` when the value is an Object
                        // *or a BigInt* -- the one primitive in the list, and the reason a
                        // program can make its BigInts serialisable at all. Leaving it out meant
                        // `JSON.stringify(1n)` refused before the hook it was told to call.
                        // A typed array is an object too; the one that carries a `toJSON`
                        // is a Buffer, through `Buffer.prototype`.
                        if !matches!(
                            receiver,
                            Value::Object(_)
                                | Value::Array(_)
                                | Value::Date(_)
                                | Value::Temporal(_)
                                | Value::Proxy(_)
                                | Value::BigInt(_)
                                | Value::TypedArray(_)
                        ) {
                            return Ok(receiver.clone());
                        }
                        let method = self.read_property(receiver, "toJSON", instruction)?;
                        if !is_callable(&method) {
                            return Ok(receiver.clone());
                        }
                        self.call_value(
                            &method,
                            receiver.clone(),
                            Vec::from([Value::String(key.clone())]),
                            instruction,
                        )
                    }
                };
                let wrapper = {
                    let mut data = ObjectData::default();
                    data.set("", value.clone());
                    Value::Object(Rc::new(RefCell::new(data)))
                };
                let empty = StringUnits::default();
                Ok(
                    match json_stringify(
                        &value,
                        &indent,
                        &empty,
                        &mut seen,
                        &mut resolve,
                        allowed.as_deref(),
                        &empty,
                        &wrapper,
                    )? {
                        Some(text) => Value::String(text),
                        // `JSON.stringify(undefined)` is `undefined`, not the string.
                        None => Value::Undefined,
                    },
                )
            }
            Native::DateNow => Ok(Value::Number(
                self.wall_clock_milliseconds(self.clock_capability, instruction)?,
            )),
            // Monotonic measurement stays on the Tokio-owned virtual clock. A host-backed Date
            // must not make elapsed-time APIs jump when the operating-system wall clock changes.
            Native::PerformanceNow => Ok(Value::Number(
                self.with_realm(kiln_runtime::Realm::now_milliseconds),
            )),
            Native::ProcessHrtimeBigInt => {
                let nanoseconds = self.with_realm(|realm| realm.now()).as_nanos();
                Ok(Value::BigInt(Rc::new(num_bigint::BigInt::from(
                    nanoseconds,
                ))))
            }
            Native::ProcessHrtime => {
                let mut nanoseconds = self.with_realm(|realm| realm.now()).as_nanos();
                if let Some(Value::Array(previous)) = arguments.first() {
                    let previous = previous.borrow();
                    let seconds = previous.first().map(Value::as_number).unwrap_or(0.0);
                    let nanos = previous.get(1).map(Value::as_number).unwrap_or(0.0);
                    let prior = (seconds.max(0.0) as u128)
                        .saturating_mul(1_000_000_000)
                        .saturating_add(nanos.max(0.0) as u128);
                    nanoseconds = nanoseconds.saturating_sub(prior);
                }
                Ok(Value::Array(ArrayData::new(vec![
                    Value::Number((nanoseconds / 1_000_000_000) as f64),
                    Value::Number((nanoseconds % 1_000_000_000) as f64),
                ])))
            }
            Native::ProcessCwd => {
                let cwd = std::env::current_dir()
                    .map(|p| p.to_string_lossy().into_owned())
                    .unwrap_or_else(|_| "/".to_owned());
                Ok(Value::String(cwd.into()))
            }
            Native::ProcessKill => Ok(Value::Boolean(true)),
            Native::ProcessExit => {
                let code = match arguments.first() {
                    Some(Value::Number(n)) => *n as i32,
                    Some(v) => v.as_number() as i32,
                    None => 0,
                };
                return Err(ExecutionError::ProcessExited(code));
            }
            Native::ProcessStdinSetRawMode => {
                let mode = arguments.first().map(Value::is_truthy).unwrap_or(false);
                if let Value::Object(obj) = receiver {
                    obj.borrow_mut().set("isRaw", Value::Boolean(mode));
                }
                Ok(receiver.clone())
            }
            // Copying reads each property, and a read is a call when the property is an
            // accessor -- which is why this cannot be a pure transformation of its arguments.
            Native::ObjectAssign => {
                // `ToObject(target)`: `Object.assign(1, { a: 2 })` copies onto a boxed number
                // and hands it back. Null and undefined throw TypeError.
                let given = arguments.first().cloned().unwrap_or(Value::Undefined);
                let Some(target) = to_object(&given) else {
                    return Err(ExecutionError::TypeErrorMessage {
                        message: "Cannot convert undefined or null to object".to_owned(),
                    });
                };
                if arguments.len() <= 1 {
                    return Ok(target);
                }
                for source in arguments.iter().skip(1) {
                    if source.is_nullish() {
                        continue;
                    }
                    let Some(from) = to_object(source) else {
                        continue;
                    };
                    for key in self.own_keys_reflected(&from, instruction)? {
                        let Some(descriptor) =
                            self.own_descriptor_value(&from, &key, instruction)?
                        else {
                            continue;
                        };
                        let enumerable =
                            self.read_property(&descriptor, "enumerable", instruction)?;
                        if !enumerable.is_truthy() {
                            continue;
                        }
                        let value = self.read_computed(&from, &key, instruction)?;
                        self.set_or_throw(&target, &key, value, instruction)?;
                    }
                }
                Ok(target)
            }
            Native::ObjectEntries | Native::ObjectValues | Native::ReflectOwnKeys => {
                let source = arguments.first().cloned().unwrap_or(Value::Undefined);
                if matches!(native, Native::ReflectOwnKeys) {
                    let source = self.reflect_target(Some(&source), "ownKeys")?;
                    let items = self.own_keys_reflected(&source, instruction)?;
                    return Ok(Value::Array(ArrayData::new(items)));
                }
                // A proxy answers "what keys do you have" with a trap, not with a property
                // store, so the walk below has nothing to read.
                if matches!(source, Value::Proxy(_)) {
                    let mut items = Vec::new();
                    for key in self.own_keys(&source, instruction)? {
                        if is_reserved_units(&key) {
                            continue;
                        }
                        items.push(match native {
                            Native::ObjectValues => self.read_property_key(
                                &source,
                                PropertyKeyRef::Units(&key),
                                instruction,
                            )?,
                            _ => Value::Array(ArrayData::new(vec![
                                Value::String(key.clone()),
                                self.read_property_key(
                                    &source,
                                    PropertyKeyRef::Units(&key),
                                    instruction,
                                )?,
                            ])),
                        });
                    }
                    return Ok(Value::Array(ArrayData::new(items)));
                }
                let mut items = Vec::new();
                for (key, property) in own_string_properties_units(&self.intrinsics, &source) {
                    let value = match property {
                        Property::Value(value) => value,
                        Property::Accessor { get, .. } => match get {
                            Some(getter) => {
                                self.call_value(&getter, source.clone(), Vec::new(), instruction)?
                            }
                            None => Value::Undefined,
                        },
                    };
                    items.push(if matches!(native, Native::ObjectValues) {
                        value
                    } else {
                        Value::Array(ArrayData::new(vec![Value::String(key), value]))
                    });
                }
                Ok(Value::Array(ArrayData::new(items)))
            }
            Native::ObjectFromEntries => {
                let source = arguments.first().cloned().unwrap_or(Value::Undefined);
                let mut data = ObjectData::default();
                // Stepped and consumed one entry at a time, not drained first: a source that
                // never reports `done` was walked for ever, and the entry read that would have
                // ended it -- an accessor at `0` that throws -- was never reached. The same
                // hang `new Map(source)` had, in the same shape.
                let method =
                    self.read_property(&source, WellKnown::Iterator.property_key(), instruction)?;
                if !is_callable(&method) {
                    return Err(ExecutionError::TypeErrorMessage {
                        message: format!("{} is not iterable", source.to_display_string()),
                    });
                }
                let iterator = self.call_value(&method, source.clone(), Vec::new(), instruction)?;
                let next = self.iterator_direct(&iterator, instruction)?;
                loop {
                    let Some(entry) = self.iterator_step(&iterator, &next, instruction)? else {
                        break;
                    };
                    match self.from_entries_pair(&entry, instruction) {
                        Ok((key, value)) => {
                            let key_units = key.property_key_units_checked()?;
                            data.set_units(&key_units, value);
                        }
                        Err(error) => {
                            self.close_discarding(&iterator, instruction);
                            return Err(error);
                        }
                    }
                }
                Ok(Value::Object(Rc::new(RefCell::new(data))))
            }
            // `String.raw` is the one built-in written to be used as a template tag: it reads
            // the `raw` half of the template object, which is the text before escapes were
            // interpreted.
            Native::StringRaw => {
                let strings = arguments.first().cloned().unwrap_or(Value::Undefined);
                // `ToObject` of the template, and again of whatever its `raw` answered. Both
                // refuse a nullish value, and skipping the check did not merely lose an error:
                // `String.raw()` answered the empty string, which tells a caller its template
                // had no segments rather than that it passed no template at all.
                if strings.is_nullish() {
                    return Err(ExecutionError::TypeErrorMessage {
                        message: "Cannot convert undefined or null to object".to_owned(),
                    });
                }
                let raw = self.read_property(&strings, "raw", instruction)?;
                if raw.is_nullish() {
                    return Err(ExecutionError::TypeErrorMessage {
                        message: "Cannot convert undefined or null to object".to_owned(),
                    });
                }
                // An array's elements are not named properties, so an index read has to go
                // through the array itself; anything else is treated as array-like.
                let segments = match &raw {
                    Value::Array(items) => dense_items(&items.borrow()),
                    other => {
                        let length = bounded_length(
                            self.read_property(other, "length", instruction)?
                                .as_number(),
                        )?;
                        let mut segments = Vec::with_capacity(length);
                        for index in 0..length {
                            segments.push(self.read_property(
                                other,
                                &index.to_string(),
                                instruction,
                            )?);
                        }
                        segments
                    }
                };
                let mut out = String::new();
                for (index, segment) in segments.iter().enumerate() {
                    out.push_str(&segment.to_display_string());
                    // One substitution between each pair of segments, and none after the last.
                    if index + 1 < segments.len()
                        && let Some(value) = arguments.get(index + 1)
                    {
                        out.push_str(&value.to_display_string());
                    }
                }
                Ok(Value::String(out.into()))
            }
            // The operations that reach a proxy's remaining traps. Each has an ordinary pure
            // implementation that reads a property store; a proxy has none, and answering from
            // its target would skip the handler entirely.
            Native::ObjectGetPrototypeOf if matches!(arguments.first(), Some(Value::Proxy(_))) => {
                let target = arguments.first().cloned().unwrap_or(Value::Undefined);
                self.proxy_get_prototype_of(&target, instruction)
            }
            Native::ObjectSetPrototypeOf if matches!(arguments.first(), Some(Value::Proxy(_))) => {
                let target = arguments.first().cloned().unwrap_or(Value::Undefined);
                let prototype = arguments.get(1).cloned().unwrap_or(Value::Undefined);
                if !matches!(prototype, Value::Null) && !is_reflect_target(&prototype) {
                    return Err(ExecutionError::TypeErrorMessage {
                        message: "Object prototype may only be an Object or null".to_owned(),
                    });
                }
                // `Object.setPrototypeOf` raises where `Reflect.setPrototypeOf` reports: the
                // refusal is the same one, and only the spelling of the answer differs.
                if !self.proxy_set_prototype_of(&target, &prototype, instruction)? {
                    return Err(prototype_change_refused());
                }
                Ok(target)
            }
            Native::ObjectIsExtensible if matches!(arguments.first(), Some(Value::Proxy(_))) => {
                let target = arguments.first().cloned().unwrap_or(Value::Undefined);
                Ok(Value::Boolean(
                    self.proxy_is_extensible(&target, instruction)?,
                ))
            }
            Native::ObjectPreventExtensions
                if matches!(arguments.first(), Some(Value::Proxy(_))) =>
            {
                let target = arguments.first().cloned().unwrap_or(Value::Undefined);
                if !self.proxy_prevent_extensions(&target, instruction)? {
                    return Err(ExecutionError::TypeErrorMessage {
                        message: "Object.preventExtensions could not close the object".to_owned(),
                    });
                }
                Ok(target)
            }
            Native::ObjectFreeze if matches!(arguments.first(), Some(Value::Proxy(_))) => {
                let target = arguments.first().cloned().unwrap_or(Value::Undefined);
                self.proxy_integrity_level(&target, true, instruction)
            }
            Native::ObjectIsFrozen if matches!(arguments.first(), Some(Value::Proxy(_))) => {
                let target = arguments.first().cloned().unwrap_or(Value::Undefined);
                self.proxy_is_integrity_level(&target, true, instruction)
            }
            // `Object.prototype.toLocaleString()` is `Invoke(this, "toString")`: it calls the
            // receiver's own `toString`, wherever on the chain that is, rather than producing the
            // tag itself. A class that overrides `toString` for display is the reason the method
            // exists at all.
            Native::ObjectToLocaleString => {
                if matches!(receiver, Value::Undefined | Value::Null) {
                    return Err(ExecutionError::TypeErrorMessage {
                        message: "Cannot convert undefined or null to object".to_owned(),
                    });
                }
                let method = self.read_property(receiver, "toString", instruction)?;
                if !is_callable(&method) {
                    return Err(ExecutionError::TypeErrorMessage {
                        message: "toString is not a function".to_owned(),
                    });
                }
                self.call_value(&method, receiver.clone(), Vec::new(), instruction)
            }
            Native::ObjectIsPrototypeOf => {
                // `ToObject(this)` before the argument is even looked at, so `undefined` is a
                // `TypeError` rather than the `false` that reads like a real answer.
                if matches!(receiver, Value::Undefined | Value::Null) {
                    return Err(ExecutionError::TypeErrorMessage {
                        message: "Cannot convert undefined or null to object".to_owned(),
                    });
                }
                let target = arguments.first().cloned().unwrap_or(Value::Undefined);
                self.is_prototype_of(&target, receiver, instruction)
            }
            // `Object.getOwnPropertyNames` is the `ownKeys` trap too. It lists what is not
            // enumerated as well, so it cannot be answered from `Object.keys`.
            Native::ObjectOwnNames if matches!(arguments.first(), Some(Value::Proxy(_))) => {
                let target = arguments.first().cloned().unwrap_or(Value::Undefined);
                let keys = self.own_keys(&target, instruction)?;
                Ok(Value::Array(ArrayData::new(
                    keys.into_iter()
                        .filter(|key| !is_reserved_units(key))
                        .map(Value::String)
                        .collect(),
                )))
            }
            // `Object.prototype.hasOwnProperty.call(proxy, k)` is how a library asks a value
            // whether it really holds a key, and on a proxy the answer is a trap: the pure
            // implementation reads a property store and would say no to everything.
            Native::ObjectHasOwnProperty if matches!(receiver, Value::Proxy(_)) => {
                let key = arguments.first().cloned().unwrap_or(Value::Undefined);
                let described = self.proxy_descriptor(receiver, &key, instruction)?;
                Ok(Value::Boolean(!matches!(
                    described,
                    Value::Undefined | Value::Null
                )))
            }
            // `Object.prototype.propertyIsEnumerable.call(proxy, key)` is the same own-property
            // query as `hasOwnProperty`, with one flag read from the descriptor. Going through
            // `own_attributes` here would bypass a proxy's `getOwnPropertyDescriptor` trap.
            Native::ObjectPropertyIsEnumerable if matches!(receiver, Value::Proxy(_)) => {
                let key = arguments.first().cloned().unwrap_or(Value::Undefined);
                let described = self.proxy_descriptor(receiver, &key, instruction)?;
                if !is_an_object(&described) {
                    return Ok(Value::Boolean(false));
                }
                let descriptor = self.read_descriptor(&described, instruction)?;
                Ok(Value::Boolean(descriptor.enumerable.unwrap_or(false)))
            }
            Native::ObjectHasOwn if matches!(arguments.first(), Some(Value::Proxy(_))) => {
                let target = arguments.first().cloned().unwrap_or(Value::Undefined);
                let key = arguments.get(1).cloned().unwrap_or(Value::Undefined);
                let described = self.proxy_descriptor(&target, &key, instruction)?;
                Ok(Value::Boolean(!matches!(
                    described,
                    Value::Undefined | Value::Null
                )))
            }
            // Describing and defining a property are trapped operations of their own. A library
            // that copies a proxy into a plain object -- which is what a copy-on-write layer
            // does when it finalises -- goes through exactly this pair.
            // `Function.prototype[Symbol.hasInstance]` called directly, and -- since it is a
            // real property now -- also what `instanceof` reaches for every ordinary function.
            // It performs `OrdinaryHasInstance`, deliberately not the operator, which would read
            // this same property back off the receiver and recur without end.
            Native::FunctionHasInstance => {
                let value = arguments.first().cloned().unwrap_or(Value::Undefined);
                let receiver = receiver.clone();
                let answered = self.ordinary_has_instance(&value, &receiver, None, instruction)?;
                Ok(Value::Boolean(answered))
            }
            // The detached `Object.prototype.__proto__` getter, called on a proxy: it is
            // `[[GetPrototypeOf]]` like the property read is, and the trap can throw.
            Native::ProtoGetter if matches!(receiver, Value::Proxy(_)) => {
                let target = receiver.clone();
                self.proxy_get_prototype_of(&target, instruction)
            }
            Native::ObjectGetOwnPropertyDescriptor
                if matches!(arguments.first(), Some(Value::Proxy(_))) =>
            {
                let target = arguments.first().cloned().unwrap_or(Value::Undefined);
                let key = arguments.get(1).cloned().unwrap_or(Value::Undefined);
                self.proxy_descriptor(&target, &key, instruction)
            }
            Native::ObjectGetOwnPropertyDescriptors
                if matches!(arguments.first(), Some(Value::Proxy(_))) =>
            {
                let target = arguments.first().cloned().unwrap_or(Value::Undefined);
                let mut data = ObjectData::default();
                for key in self.own_keys(&target, instruction)? {
                    let descriptor =
                        self.proxy_descriptor(&target, &Value::String(key.clone()), instruction)?;
                    if !matches!(descriptor, Value::Undefined) {
                        data.set_units(&key, descriptor);
                    }
                }
                Ok(Value::Object(Rc::new(RefCell::new(data))))
            }
            Native::ObjectDefineProperty | Native::ReflectDefineProperty
                if matches!(arguments.first(), Some(Value::Proxy(_))) =>
            {
                let target = arguments.first().cloned().unwrap_or(Value::Undefined);
                let Value::Proxy(proxy) = &target else {
                    unreachable!("checked by the guard");
                };
                let proxy = Rc::clone(proxy);
                let inner = proxy.target.clone();
                let key = arguments.get(1).cloned().unwrap_or(Value::Undefined);
                let descriptor = arguments.get(2).cloned().unwrap_or(Value::Undefined);
                match self.proxy_trap(&proxy, "defineProperty", instruction)? {
                    Some(trap) => {
                        // Read before the trap runs, which is the order the specification
                        // gives: a handler that mutates the descriptor object cannot change
                        // what it is checked against afterwards.
                        let described = self.read_descriptor(&descriptor, instruction)?;
                        let handler = proxy.handler.clone();
                        let list = vec![inner.clone(), self.trap_key(&key), descriptor];
                        let answered = self.call_value(&trap, handler, list, instruction)?;
                        if !answered.is_truthy() {
                            if matches!(native, Native::ReflectDefineProperty) {
                                return Ok(Value::Boolean(false));
                            }
                            return Err(ExecutionError::TypeErrorMessage {
                                message: "proxy defineProperty refused the definition".to_owned(),
                            });
                        }
                        self.check_define_invariant(&inner, &key, &described, instruction)?;
                    }
                    None => {
                        self.dispatch_native(
                            native,
                            receiver,
                            &[inner, key, descriptor],
                            instruction,
                        )?;
                    }
                }
                if matches!(native, Native::ReflectDefineProperty) {
                    return Ok(Value::Boolean(true));
                }
                Ok(target)
            }
            // `Object.keys` on a proxy is the `ownKeys` trap filtered through `getOwnPropertyDescriptor`
            // for enumerability; the pure implementation reads a property store the proxy does not have.
            Native::ObjectKeys if matches!(arguments.first(), Some(Value::Proxy(_))) => {
                let target = arguments.first().cloned().unwrap_or(Value::Undefined);
                let mut kept = Vec::new();
                for key in self.own_keys(&target, instruction)? {
                    if is_reserved_units(&key) {
                        continue;
                    }
                    let described =
                        self.proxy_descriptor(&target, &Value::String(key.clone()), instruction)?;
                    if !is_an_object(&described) {
                        continue;
                    }
                    let descriptor = self.read_descriptor(&described, instruction)?;
                    if !descriptor.enumerable.unwrap_or(false) {
                        continue;
                    }
                    kept.push(Value::String(key));
                }
                Ok(Value::Array(ArrayData::new(kept)))
            }
            // `map.forEach(cb)` calls back into the program, so it belongs here rather than in
            // the pure table. A Set passes its value as both arguments, which is the same shape
            // its `entries` takes and the reason one callback signature serves both.
            Native::CollectionForEach(_) => {
                let (entries, is_map) = require_collection(receiver, native)?;
                let callback = arguments.first().cloned().unwrap_or(Value::Undefined);
                let this_value = arguments.get(1).cloned().unwrap_or(Value::Undefined);
                let mut index = 0;
                // The length is re-read each step: entries added while iterating are visited,
                // which is what a Set does and is observable.
                loop {
                    let entry = entries.borrow().get(index).cloned();
                    let Some(entry) = entry else { break };
                    index += 1;
                    if matches!(entry, Value::Native(Native::Tombstone, _)) {
                        continue;
                    }
                    let (key, value) = match (is_map, &entry) {
                        (true, Value::Array(pair)) => {
                            let pair = pair.borrow();
                            (
                                pair.first().cloned().unwrap_or(Value::Undefined),
                                pair.get(1).cloned().unwrap_or(Value::Undefined),
                            )
                        }
                        _ => (entry.clone(), entry.clone()),
                    };
                    self.call_value(
                        &callback,
                        this_value.clone(),
                        vec![value, key, receiver.clone()],
                        instruction,
                    )?;
                }
                Ok(Value::Undefined)
            }
            Native::FunctionCall | Native::FunctionApply => {
                let (this_value, rest) =
                    self.forwarded_function_arguments(native, arguments, instruction)?;
                let callee = receiver.clone();
                self.call_value(&callee, this_value, rest, instruction)
            }
            // `bind` is generic in its receiver but not *that* generic: the specification's
            // first step refuses one that is not callable, because the result would be a bound
            // function wrapping nothing -- an object that says `typeof` is "function" and
            // throws only later, wherever it is finally called.
            Native::FunctionBind if !is_callable(receiver) => {
                Err(ExecutionError::TypeErrorMessage {
                    message: format!(
                        "Bind must be called on a function, got {}",
                        receiver.type_name()
                    ),
                })
            }
            Native::FunctionBind => {
                let prototype = self.prototype_for(receiver, instruction)?;
                let bound = Value::Bound(Rc::new(BoundFunction {
                    target: receiver.clone(),
                    this_value: arguments.first().cloned().unwrap_or(Value::Undefined),
                    arguments: arguments.get(1..).unwrap_or_default().to_vec(),
                    properties: RefCell::new(ObjectData::default()),
                }));
                record_prototype(&bound, prototype);
                Ok(bound)
            }
            // `Object.create(proto, descriptors)` is `Object.create(proto)` followed by
            // `Object.defineProperties`. The descriptors may carry accessors, and an accessor
            // is a function -- so the second argument cannot be handled beside the first one,
            // which needs no machine at all.
            // `Object.create(proto, descriptors)` is `Object.create(proto)` followed by
            // `Object.defineProperties`. The descriptors may carry accessors, and an accessor is
            // a function -- so the second argument cannot be handled beside the first one, which
            // needs no machine at all.
            Native::ObjectCreate => {
                let prototype = arguments.first().cloned().unwrap_or(Value::Undefined);
                // The prototype is stored, not read: a primitive there would make every miss on
                // the new object walk into something that has no chain to walk.
                if !matches!(prototype, Value::Null) && !is_an_object(&prototype) {
                    return Err(ExecutionError::TypeErrorMessage {
                        message: "Object prototype may only be an Object or null".to_owned(),
                    });
                }
                let object = call_native(
                    &self.intrinsics,
                    Native::ObjectCreate,
                    receiver,
                    &[prototype],
                )?;
                // `undefined` is "no second argument"; `null` is a second argument that cannot
                // be converted to an object, which is a `TypeError` rather than a silent skip.
                if let Some(descriptors) = arguments.get(1)
                    && !matches!(descriptors, Value::Undefined)
                {
                    let descriptors = descriptors.clone();
                    self.define_properties(&object, &descriptors, instruction)?;
                }
                Ok(object)
            }
            Native::ObjectDefineProperties => {
                let target = arguments.first().cloned().unwrap_or(Value::Undefined);
                if !is_an_object(&target) {
                    return Err(ExecutionError::TypeErrorMessage {
                        message: "Object.defineProperties called on non-object".to_owned(),
                    });
                }
                let source = arguments.get(1).cloned().unwrap_or(Value::Undefined);
                self.define_properties(&target, &source, instruction)?;
                Ok(target)
            }
            // `Object.defineProperty` and `Reflect.defineProperty` are one operation reported
            // two ways: the definition either happens or is refused, and only what a refusal
            // means to the caller differs.
            Native::ObjectDefineProperty | Native::ReflectDefineProperty => {
                let target = arguments.first().cloned().unwrap_or(Value::Undefined);
                if !is_an_object(&target) {
                    return Err(ExecutionError::TypeErrorMessage {
                        message: format!(
                            "Object.defineProperty called on non-object: {}",
                            target.to_display_string()
                        ),
                    });
                }
                let key =
                    self.computed_key(arguments.get(1).unwrap_or(&Value::Undefined), instruction)?;
                let key_units = key.property_key_units_checked()?;
                let source = arguments.get(2).cloned().unwrap_or(Value::Undefined);
                let descriptor = self.read_descriptor(&source, instruction)?;
                let defined = self.define_own_key(
                    &target,
                    PropertyKeyRef::from_value(&key, &key_units),
                    &descriptor,
                    instruction,
                )?;
                if matches!(native, Native::ReflectDefineProperty) {
                    return Ok(Value::Boolean(defined));
                }
                if !defined {
                    return Err(ExecutionError::TypeErrorMessage {
                        message: refusal(&self.intrinsics, &target, &key_units.to_lossy_string()),
                    });
                }
                Ok(target)
            }
            Native::Uint8Text(conversion) => {
                self.byte_text(conversion, receiver, arguments, instruction)
            }
            Native::ReflectGet => {
                let target = arguments.first().cloned().unwrap_or(Value::Undefined);
                if !is_reflect_target(&target) {
                    return Err(ExecutionError::NotAnObject {
                        instruction,
                        value: target.type_name(),
                    });
                }
                let key = arguments.get(1).cloned().unwrap_or(Value::Undefined);
                let receiver = arguments.get(2).cloned().unwrap_or_else(|| target.clone());
                self.read_computed_with_receiver(&target, &key, &receiver, instruction)
            }
            Native::AtomicsOp(op) => self.atomics(op, arguments, instruction),
            Native::CollectionGetOrInsert { weak, computed } => {
                self.collection_get_or_insert(weak, computed, receiver, arguments, instruction)
            }
            Native::ReflectSet => {
                let target = self.reflect_target(arguments.first(), "set")?;
                let key = arguments.get(1).cloned().unwrap_or(Value::Undefined);
                let value = arguments.get(2).cloned().unwrap_or(Value::Undefined);
                let receiver = arguments.get(3).cloned().unwrap_or_else(|| target.clone());
                let accepted = self.set_reflect(&target, &key, value, &receiver, instruction)?;
                Ok(Value::Boolean(accepted))
            }
            Native::ReflectApply => {
                let callee = arguments.first().cloned().unwrap_or(Value::Undefined);
                let this_value = arguments.get(1).cloned().unwrap_or(Value::Undefined);
                let list = self.argument_list(arguments.get(2), instruction)?;
                self.call_value(&callee, this_value, list, instruction)
            }
            // `Reflect.construct(target, args, newTarget)`: `new target(...args)` with the
            // `new.target` the body sees named separately. Both functions are checked for a
            // `[[Construct]]` *before* the argument list is read, which is the order a program
            // watching the list's `length` getter can see.
            Native::ReflectConstruct => {
                let target = arguments.first().cloned().unwrap_or(Value::Undefined);
                if !is_constructor(&target) {
                    return Err(ExecutionError::TypeErrorMessage {
                        message: "Reflect.construct target is not a constructor".to_owned(),
                    });
                }
                let new_target = match arguments.get(2) {
                    Some(named) => named.clone(),
                    None => target.clone(),
                };
                if !is_constructor(&new_target) {
                    return Err(ExecutionError::TypeErrorMessage {
                        message: "Reflect.construct newTarget is not a constructor".to_owned(),
                    });
                }
                let list = self.argument_list(arguments.get(1), instruction)?;
                self.construct_value(target, list, new_target, instruction)
            }
            Native::ReflectHas => {
                let target = self.reflect_target(arguments.first(), "has")?;
                let key = arguments.get(1).cloned().unwrap_or(Value::Undefined);
                // `in`, not `hasOwnProperty`: the prototype chain counts, which is the whole
                // difference between this and `Object.hasOwn`.
                Ok(Value::Boolean(self.has(&target, &key, instruction)?))
            }
            Native::ReflectGetPrototypeOf => {
                let target = self.reflect_target(arguments.first(), "getPrototypeOf")?;
                if matches!(target, Value::Proxy(_)) {
                    return self.proxy_get_prototype_of(&target, instruction);
                }
                Ok(prototype_of(&target))
            }
            Native::ReflectSetPrototypeOf => {
                let target = self.reflect_target(arguments.first(), "setPrototypeOf")?;
                let prototype = arguments.get(1).cloned().unwrap_or(Value::Undefined);
                if matches!(target, Value::Proxy(_)) {
                    if !matches!(prototype, Value::Null) && !is_reflect_target(&prototype) {
                        return Err(ExecutionError::TypeErrorMessage {
                            message: "Object prototype may only be an Object or null".to_owned(),
                        });
                    }
                    return Ok(Value::Boolean(self.proxy_set_prototype_of(
                        &target,
                        &prototype,
                        instruction,
                    )?));
                }
                // A prototype must be an object or null; anything else is refused rather than
                // silently ignored, and the refusal is this operation's `false`.
                if !matches!(prototype, Value::Null) && !is_reflect_target(&prototype) {
                    return Err(ExecutionError::TypeErrorMessage {
                        message: "Object prototype may only be an Object or null".to_owned(),
                    });
                }
                // The same two refusals, reported rather than thrown: answering whether the
                // change happened is what this spelling of the operation exists for.
                if !prototype_may_change(&self.intrinsics, &target, &prototype) {
                    return Ok(Value::Boolean(false));
                }
                // `OrdinarySetPrototypeOf` step 3: the prototype it already has is a change that
                // succeeds -- `Reflect.setPrototypeOf(Object.prototype, null)` is true, although
                // that object's chain lives in its kind and could not take a different one.
                if same_value(&prototype_of(&target), &prototype) {
                    return Ok(Value::Boolean(true));
                }
                // A value with no store keeps its chain in its kind, so there is nowhere to put
                // a new one. Reported as a refusal, which is what this operation exists to say.
                Ok(Value::Boolean(record_prototype(&target, prototype)))
            }
            Native::ReflectGetOwnPropertyDescriptor => {
                let target = self.reflect_target(arguments.first(), "getOwnPropertyDescriptor")?;
                let key =
                    self.computed_key(arguments.get(1).unwrap_or(&Value::Undefined), instruction)?;
                if matches!(target, Value::Proxy(_)) {
                    return self.proxy_descriptor(&target, &key, instruction);
                }
                let key_units = key.property_key_units_checked()?;
                Ok(own_descriptor_key(
                    &self.intrinsics,
                    &target,
                    PropertyKeyRef::from_value(&key, &key_units),
                )
                .unwrap_or(Value::Undefined))
            }
            Native::ReflectDeleteProperty => {
                let target = self.reflect_target(arguments.first(), "deleteProperty")?;
                let key = arguments.get(1).cloned().unwrap_or(Value::Undefined);
                if matches!(target, Value::Proxy(_)) {
                    return Ok(Value::Boolean(self.delete(&target, &key, instruction)?));
                }
                Ok(Value::Boolean(delete_property_checked(
                    &self.intrinsics,
                    &target,
                    &key,
                )?))
            }
            Native::ReflectPreventExtensions => {
                let target = self.reflect_target(arguments.first(), "preventExtensions")?;
                if matches!(target, Value::Proxy(_)) {
                    return Ok(Value::Boolean(
                        self.proxy_prevent_extensions(&target, instruction)?,
                    ));
                }
                let closed = match &target {
                    Value::Object(object) => {
                        object.borrow_mut().prevent_extensions();
                        true
                    }
                    Value::Function(closure) => {
                        closure.properties.borrow_mut().prevent_extensions();
                        true
                    }
                    Value::Array(items) => {
                        items.properties.borrow_mut().prevent_extensions();
                        true
                    }
                    Value::Native(_, _)
                    | Value::Callback(_, _)
                    | Value::Bound(_)
                    | Value::Settler(..)
                    | Value::Capability(..)
                    | Value::Element(_) => {
                        self.intrinsics.close(&target, false);
                        true
                    }
                    other if exotic_properties(other).is_some() => {
                        if let Some(store) = exotic_properties(other) {
                            store.borrow_mut().prevent_extensions();
                        }
                        true
                    }
                    // Nothing to close: what is left has no property store of its own, so this
                    // reports the refusal rather than claiming a shape it cannot keep.
                    _ => false,
                };
                Ok(Value::Boolean(closed))
            }
            Native::SymbolFor => {
                let key = self
                    .as_text_units(arguments.first().unwrap_or(&Value::Undefined), instruction)?;
                let registered = match &self.module_realm {
                    Some(realm) => realm.borrow().symbol_registry.clone(),
                    None => self.symbol_registry.clone(),
                };
                if let Some((_, symbol)) = registered.iter().find(|(name, _)| *name == key) {
                    return Ok(symbol.clone());
                }
                let symbol = self.new_symbol(Some(key.clone()));
                match &self.module_realm {
                    Some(realm) => realm
                        .borrow_mut()
                        .symbol_registry
                        .push((key, symbol.clone())),
                    None => self.symbol_registry.push((key, symbol.clone())),
                }
                Ok(symbol)
            }
            Native::SymbolKeyFor => {
                let Some(Value::Symbol(symbol)) = arguments.first() else {
                    return Err(ExecutionError::TypeErrorMessage {
                        message: "Symbol.keyFor requires a symbol".to_owned(),
                    });
                };
                if let Some(realm) = &self.module_realm {
                    return Ok(realm
                        .borrow()
                        .symbol_registry
                        .iter()
                        .find(|(_, registered)| {
                            matches!(registered, Value::Symbol(other) if other == symbol)
                        })
                        .map_or(Value::Undefined, |(key, _)| Value::String(key.clone())));
                }
                // Only a registered symbol has a key; one from `Symbol()` does not, which is how
                // a program tells the two apart.
                Ok(self
                    .symbol_registry
                    .iter()
                    .find(|(_, registered)| {
                        matches!(registered, Value::Symbol(other) if other == symbol)
                    })
                    .map_or(Value::Undefined, |(key, _)| Value::String(key.clone())))
            }
            Native::DateConstructor => {
                let time = match arguments.len() {
                    0 => self.wall_clock_milliseconds(self.clock_capability, instruction)?,
                    1 => match arguments.first() {
                        // `new Date(otherDate)` copies the instant rather than stringifying it.
                        Some(Value::Date(existing)) => existing.time.get(),
                        Some(Value::String(text)) => parse_date(&text.to_lossy_string()),
                        Some(value) => {
                            // `ToPrimitive(value)` with no preferred type at all, which is the
                            // default hint rather than the number one: `new Date(x)` decides
                            // between parsing a date string and reading an instant only *after*
                            // the value has reduced, so it cannot have asked for a number.
                            let primitive =
                                self.as_primitive(value, PrimitiveHint::Default, instruction)?;
                            match primitive {
                                Value::String(text) => parse_date(&text.to_lossy_string()),
                                // Anything that is not a string is `ToNumber`d, and the two
                                // values that refuses are refusals here too rather than the
                                // plausible instants `as_number` would invent for them.
                                Value::Symbol(_) => {
                                    return Err(ExecutionError::SymbolIsNotANumber);
                                }
                                Value::BigInt(_) => {
                                    return Err(ExecutionError::BigIntIsNotANumber);
                                }
                                other => other.as_number(),
                            }
                        }
                        None => unreachable!("length checked above"),
                    },
                    _ => date_from_fields(arguments),
                };
                Ok(new_date(time_clip(time)))
            }
            Native::RegExpConstructor => {
                let pattern = match arguments.first() {
                    Some(Value::RegExp(existing)) => existing.source.clone(),
                    None | Some(Value::Undefined) => StringUnits::default(),
                    Some(value) => value.to_string_units_checked()?,
                };
                let flags = match arguments.get(1) {
                    None | Some(Value::Undefined) => match arguments.first() {
                        // `new RegExp(re)` keeps the original's flags.
                        Some(Value::RegExp(existing)) => existing.flags.clone(),
                        _ => String::new(),
                    },
                    Some(value) => value.to_display_string(),
                };
                // An error rather than `throw_error` followed by a value: this function returns
                // one, and throwing *and* returning undefined pushed the undefined on top of the
                // handler's stack -- so `catch (e)` caught nothing rather than a SyntaxError.
                build_regexp(&pattern, &flags).ok_or(ExecutionError::InvalidRegExp)
            }
            Native::RegExpTest | Native::RegExpExec => {
                let Value::RegExp(regexp) = receiver else {
                    return Err(ExecutionError::NotCallable {
                        instruction,
                        value: receiver.type_name(),
                    });
                };
                // `ToString(S)`, which for an object *calls* its `toString`. The display
                // spelling is the machine's: `/\.14/.exec({ toString: () => Math.PI })` matched
                // against the literal text "[object Object]" and answered null. Units travel
                // losslessly: a lone surrogate in the subject stays the unit the language
                // counts rather than becoming U+FFFD before the matcher sees it.
                let subject_units: StringUnits = match arguments.first() {
                    Some(argument) if is_an_object(argument) && !is_callable(argument) => {
                        self.as_text_units(argument, instruction)?
                    }
                    Some(argument) => argument.to_string_units_checked()?,
                    None => StringUnits::default(),
                };
                let subject: Vec<u16> = subject_units.to_units().into_vec();
                // Only `g` and `y` carry a cursor; a plain pattern always searches from the
                // start, however many times it is used.
                // `ToLength(Get(R, "lastIndex"))` happens for every regexp, global or not: the
                // read is the program's own property and a `valueOf` on it runs and may throw.
                // Only a `g` or `y` pattern then *uses* the cursor; a plain one searches from
                // the start however many times it is used.
                let regexp = Rc::clone(regexp);
                let cursor = self.read_length(receiver, "lastIndex", instruction)?;
                let start = if regexp.global || regexp.sticky {
                    cursor
                } else {
                    0
                };
                let found = regexp_search(&regexp, &subject, start);
                if (regexp.global || regexp.sticky) && found.is_none() {
                    // A failed search resets the cursor, which is what stops a `g` regexp from
                    // being permanently stuck past the end.
                    self.set_last_index(receiver, 0.0, instruction)?;
                }
                if let Some((range, _, _)) = &found
                    && (regexp.global || regexp.sticky)
                {
                    self.set_last_index(receiver, range.end as f64, instruction)?;
                }
                let regexp = &regexp;
                if matches!(native, Native::RegExpTest) {
                    return Ok(Value::Boolean(found.is_some()));
                }
                let Some((range, groups, names)) = found else {
                    return Ok(Value::Null);
                };
                Ok(match_result(
                    &subject,
                    &range,
                    &groups,
                    &names,
                    regexp.has_indices,
                ))
            }
            Native::StringReplace | Native::StringReplaceAll
                if matches!(arguments.first(), Some(Value::RegExp(_)))
                    || matches!(
                        arguments.get(1),
                        Some(Value::Function(_) | Value::Native(_, _) | Value::Callback(_, _))
                    ) =>
            {
                self.replace_with_pattern(receiver, arguments, native, instruction)
            }
            // A string method whose argument the specification passes through `ToString`, given
            // an object to convert.
            //
            // `ToString` on an object *calls* its `toString`, and the implementations of these
            // ten live in the machine-free table, which has no machine to call it with -- it
            // reads arguments through a helper that can only ask for the display spelling. So
            // `"abc".indexOf({ toString: () => "b" })` answered -1, `split` did not split, and
            // `concat` appended the literal text "[object Object]". Every one of them a plausible
            // wrong answer in ordinary code rather than a failure.
            //
            // The conversion happens here, where there is a machine, and the pure implementation
            // is then handed the string it always assumed it had. Which arguments convert is per
            // method and not guessable: `split`'s second argument is a *length*, and `replace`'s
            // second is a replacement that may be a function and must reach it uncalled.
            Native::StringSplit
            | Native::StringReplace
            | Native::StringReplaceAll
            | Native::StringIndexOf
            | Native::StringLastIndexOf
            | Native::StringIncludes
            | Native::StringStartsWith
            | Native::StringEndsWith
            | Native::StringConcat
            | Native::StringPadStart
            | Native::StringPadEnd
                if arguments.iter().enumerate().any(|(index, argument)| {
                    (string_coerced_argument(native, index)
                        || (matches!(native, Native::StringSplit) && index == 1))
                        && is_an_object(argument)
                        && !is_callable(argument)
                        && !matches!(argument, Value::RegExp(_))
                }) =>
            {
                let mut converted = Vec::with_capacity(arguments.len());
                for (index, argument) in arguments.iter().enumerate() {
                    let convertible = is_an_object(argument)
                        && !is_callable(argument)
                        && !matches!(argument, Value::RegExp(_));
                    // `split`'s limit is the one argument in this set converted with `ToUint32`
                    // rather than `ToString`, and it was not converted at all: a limit object
                    // read as `NaN` truncated the answer to nothing.
                    let numeric =
                        convertible && matches!(native, Native::StringSplit) && index == 1;
                    converted.push(if numeric {
                        Value::Number(self.as_number_argument(Some(argument), instruction)?)
                    } else if convertible && string_coerced_argument(native, index) {
                        Value::String(self.as_text(argument, instruction)?.into())
                    } else {
                        argument.clone()
                    });
                }
                self.dispatch_native(native, receiver, &converted, instruction)
            }
            Native::StringSplit if matches!(arguments.first(), Some(Value::RegExp(_))) => {
                let Value::String(text) = receiver else {
                    return Ok(Value::Undefined);
                };
                let Some(Value::RegExp(regexp)) = arguments.first() else {
                    unreachable!("checked by the guard");
                };
                let subject: Vec<u16> = text.encode_utf16().collect();
                let mut parts = Vec::new();
                let mut copied = 0;
                if subject.is_empty() {
                    // An empty subject is the one case with no trailing piece: a separator that
                    // matches it splits it into nothing, and one that does not leaves it whole.
                    if regexp_search(regexp, &subject, 0).is_none() {
                        parts.push(Value::String(String::new().into()));
                    }
                    return Ok(Value::Array(ArrayData::new(parts)));
                }
                for (range, groups, _) in regexp_search_all(regexp, &subject) {
                    // A separator that matches nothing would split between every character
                    // *and* produce an empty piece for each; the specification skips an empty
                    // match at the position the last piece started.
                    if range.start == range.end && range.start == copied {
                        continue;
                    }
                    // A match *at* the end is never a separator: the walk stops before the last
                    // position, and the piece after the final separator is pushed below. Without
                    // this, `"abc".split(/x*/)` grew a fourth, empty part.
                    if range.start == subject.len() {
                        continue;
                    }
                    parts.push(Value::String(
                        utf16_slice(&subject, copied..range.start).into(),
                    ));
                    // A capturing separator contributes its groups to the result, which is what
                    // makes `"a1b".split(/(\d)/)` three elements rather than two.
                    for group in groups.iter().skip(1) {
                        parts.push(match group {
                            Some(range) => {
                                Value::String(utf16_slice(&subject, range.clone()).into())
                            }
                            None => Value::Undefined,
                        });
                    }
                    copied = range.end;
                }
                parts.push(Value::String(
                    utf16_slice(&subject, copied..subject.len()).into(),
                ));
                Ok(Value::Array(ArrayData::new(parts)))
            }
            // `ArrayBuffer.prototype.slice` builds its result with `SpeciesConstructor`, which
            // is why it is here and not in the machine-free table: reading `constructor` and
            // `@@species` runs the program's own getters, and validating what they answered is
            // most of the work. Without it a subclass sliced to a plain `ArrayBuffer` and a
            // species that was not a constructor was accepted in silence.
            Native::ArrayBufferSlice => {
                let Value::ArrayBuffer(buffer) = receiver else {
                    return Err(ExecutionError::NotABuffer);
                };
                let buffer = Rc::clone(buffer);
                if buffer.detached() {
                    return Err(ExecutionError::BufferDetached);
                }
                let (start, end) = slice_bounds(arguments, buffer.bytes.borrow().len());
                let length = end.saturating_sub(start);
                let Some(species) = self.typed_array_species(receiver, instruction)? else {
                    let bytes = buffer.bytes.borrow();
                    return Ok(Value::ArrayBuffer(Rc::new(BufferData {
                        bytes: Bytes::new(buffer.shared, bytes[start..end].to_vec()),
                        max_byte_length: None,
                        shared: buffer.shared,
                        detached: std::cell::Cell::new(false),
                        immutable: false,
                        properties: RefCell::new(ObjectData::default()),
                    })));
                };
                let out = self.construct_value(
                    species.clone(),
                    vec![Value::Number(length as f64)],
                    species,
                    instruction,
                )?;
                let Value::ArrayBuffer(target) = &out else {
                    return Err(ExecutionError::TypeErrorMessage {
                        message: "an ArrayBuffer species must answer an ArrayBuffer".to_owned(),
                    });
                };
                // The species ran the program's code, and that code may have detached either
                // buffer or handed back the very buffer being sliced -- in which case the copy
                // below would read and write the same bytes.
                if target.detached() {
                    return Err(ExecutionError::BufferDetached);
                }
                if Rc::ptr_eq(target, &buffer) {
                    return Err(ExecutionError::TypeErrorMessage {
                        message: "an ArrayBuffer species answered the buffer it was slicing"
                            .to_owned(),
                    });
                }
                if target.bytes.borrow().len() < length {
                    return Err(ExecutionError::TypeErrorMessage {
                        message: "an ArrayBuffer species answered a buffer that is too short"
                            .to_owned(),
                    });
                }
                if buffer.detached() {
                    return Err(ExecutionError::BufferDetached);
                }
                {
                    let source = buffer.bytes.borrow();
                    let mut destination = target.bytes.borrow_mut();
                    destination[..length].copy_from_slice(&source[start..end]);
                }
                Ok(out)
            }
            Native::StringSearch => {
                let Value::String(text) = receiver else {
                    return Ok(Value::Number(-1.0));
                };
                let subject: Vec<u16> = text.encode_utf16().collect();
                let Some(regexp) = self.pattern_argument(arguments, "", instruction)? else {
                    return Ok(Value::Number(-1.0));
                };
                let Value::RegExp(regexp) = &regexp else {
                    return Ok(Value::Number(-1.0));
                };
                // `search` ignores `lastIndex` entirely and always scans from the start.
                Ok(Value::Number(
                    regexp_search(regexp, &subject, 0)
                        .map_or(-1.0, |(range, _, _)| range.start as f64),
                ))
            }
            Native::StringMatch | Native::StringMatchAll => {
                let Value::String(text) = receiver else {
                    return Ok(Value::Null);
                };
                let subject: Vec<u16> = text.encode_utf16().collect();
                // `matchAll` requires `g`; `match` takes the flags the pattern was given, which
                // for a string pattern is none -- so it reports the first match, not every one.
                let built_flags = if matches!(native, Native::StringMatchAll) {
                    "g"
                } else {
                    ""
                };
                let Some(pattern) = self.pattern_argument(arguments, built_flags, instruction)?
                else {
                    return Ok(Value::Null);
                };
                let Value::RegExp(regexp) = &pattern else {
                    return Ok(Value::Null);
                };
                let all = matches!(native, Native::StringMatchAll) || regexp.global;
                if !all {
                    // Without `g`, `match` is `exec`: one result with its groups.
                    return Ok(match regexp_search(regexp, &subject, 0) {
                        Some((range, groups, names)) => {
                            match_result(&subject, &range, &groups, &names, regexp.has_indices)
                        }
                        None => Value::Null,
                    });
                }
                let mut results = Vec::new();
                for (range, groups, names) in regexp_search_all(regexp, &subject) {
                    results.push(if matches!(native, Native::StringMatchAll) {
                        // `matchAll` yields full results; `match` with `g` yields only the text.
                        match_result(&subject, &range, &groups, &names, regexp.has_indices)
                    } else {
                        Value::String(utf16_slice(&subject, range).into())
                    });
                }
                if results.is_empty() && matches!(native, Native::StringMatch) {
                    return Ok(Value::Null);
                }
                self.set_last_index(&pattern, 0.0, instruction)?;
                Ok(Value::Array(ArrayData::new(results)))
            }
            Native::QueueMicrotask => {
                let scheduled = self.scheduled(Job::Callback(
                    arguments.first().cloned().unwrap_or(Value::Undefined),
                    Vec::new(),
                ));
                self.with_realm_mut(|realm| realm.queue_microtask(scheduled));
                Ok(Value::Undefined)
            }
            // 4: a rejection to report (node's `hasRejectionToWarn`), which the end of this
            // checkpoint does -- so a delivery ends its batch there, as node reports one before
            // the next message.
            Native::JobsPendingRaw => {
                let rejections = self.with_rejections_mut(|rejections| {
                    !rejections.rejected.is_empty() || !rejections.handled.is_empty()
                });
                Ok(Value::Number(
                    self.with_realm(|realm| {
                        f64::from(u8::from(realm.has_queued_next_ticks()))
                            + 2.0 * f64::from(u8::from(realm.has_queued_microtasks()))
                    }) + 4.0 * f64::from(u8::from(rejections)),
                ))
            }
            // The first registration is the realm's own bootstrap; a later realm's (a
            // `ShadowRealm`) reports through it.
            Native::PromiseRejectionsRaw => {
                let process = arguments.first().cloned().unwrap_or(Value::Undefined);
                if is_callable(&process) {
                    self.with_rejections_mut(|rejections| {
                        rejections.process.get_or_insert(process);
                    });
                }
                Ok(Value::Undefined)
            }
            Native::ProcessNextTick => {
                let callback = arguments.first().cloned().unwrap_or(Value::Undefined);
                if !is_callable(&callback) {
                    return Err(ExecutionError::NotCallable {
                        instruction,
                        value: callback.type_name(),
                    });
                }
                let scheduled = self.scheduled(Job::NextTick(
                    callback,
                    arguments.get(1..).unwrap_or_default().to_vec(),
                ));
                self.with_realm_mut(|realm| realm.queue_next_tick(scheduled));
                Ok(Value::Undefined)
            }
            // Every call has to produce a symbol distinct from every other, so this needs the
            // counter rather than only its arguments.
            Native::SymbolConstructor => {
                let description = match arguments.first() {
                    None | Some(Value::Undefined) => None,
                    Some(value) => Some(self.as_text_units(value, instruction)?),
                };
                Ok(self.new_symbol(description))
            }
            // The two built-ins that compile. Both need the machine -- a compiled program is
            // entered as a frame and run -- so neither can live in `call_native`, which is a
            // free function with no VM to enter one on.
            Native::Eval => self.indirect_eval(arguments),
            Native::FunctionConstructor => {
                self.function_from_source(arguments, instruction, "function")
            }
            Native::GeneratorFunctionConstructor => {
                self.function_from_source(arguments, instruction, "function*")
            }
            Native::AsyncGeneratorFunctionConstructor => {
                self.function_from_source(arguments, instruction, "async function*")
            }
            // `run` and `exit` call the program's own function, so they need the machine for the
            // same reason those two do. The other three are here to keep one storage's five
            // methods in one place rather than splitting them across two dispatchers.
            Native::NodeAsyncLocalStorage
            | Native::NodeAsyncLocalStorageRun
            | Native::NodeAsyncLocalStorageGetStore
            | Native::NodeAsyncLocalStorageEnterWith
            | Native::NodeAsyncLocalStorageExit
            | Native::NodeAsyncLocalStorageDisable => {
                self.async_local_storage(native, receiver, arguments, instruction)
            }
            _ => call_native(&self.intrinsics, native, receiver, arguments),
        }
    }
}
