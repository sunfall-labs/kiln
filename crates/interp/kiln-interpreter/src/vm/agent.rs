use super::*;

impl<H: HostOps + ?Sized> Vm<'_, H> {
    /// One operation of Test262's agent protocol.
    ///
    /// Published only when the host asked for agents, so reaching here means it did. Every arm
    /// that can block goes through the scheduler in [`agent`] rather than through the operating
    /// system, which is what makes a run of the agent tests reproducible.
    pub(super) fn host_agent(
        &mut self,
        op: AgentOp,
        arguments: &[Value],
        instruction: usize,
    ) -> Result<Value, ExecutionError> {
        let Some(group) = self.agents.clone() else {
            return Err(ExecutionError::TypeErrorMessage {
                message: "kiln: this execution has no agents".to_owned(),
            });
        };
        let first = arguments.first().cloned().unwrap_or(Value::Undefined);
        match op {
            AgentOp::Start => {
                let source = self.as_text_units(&first, instruction)?;
                let source = self.checked_dynamic_source(&source)?;
                // The host's prelude first: a new realm has never heard of `$262`, and the
                // source it was given assumes it has.
                let prelude = self.host_agents.unwrap_or_default();
                group.start(format!(
                    "{prelude}
{source}"
                ))?;
                self.checkpoint = self.next_checkpoint();
                Ok(Value::Undefined)
            }
            AgentOp::Broadcast => {
                let Value::ArrayBuffer(buffer) = &first else {
                    return Err(ExecutionError::TypeErrorMessage {
                        message: "$262.agent.broadcast requires a SharedArrayBuffer".to_owned(),
                    });
                };
                let Some(store) = buffer.bytes.store().map(Arc::clone) else {
                    return Err(ExecutionError::TypeErrorMessage {
                        message: "$262.agent.broadcast requires a SharedArrayBuffer".to_owned(),
                    });
                };
                group
                    .broadcast(self.agent_id, &store, buffer.max_byte_length)
                    .map_err(|_| ExecutionError::AgentTerminated)?;
                self.checkpoint = self.next_checkpoint();
                Ok(Value::Undefined)
            }
            // `null` rather than `undefined` when there is nothing: Test262's own wrapper loops
            // `while ((r = getReport()) == null)`, and an agent that reported the empty string
            // has still reported.
            AgentOp::GetReport => Ok(group
                .take_report()
                .map_or(Value::Null, |value| Value::String(value.into()))),
            AgentOp::Sleep => {
                let milliseconds = self.as_number_argument(Some(&first), instruction)?;
                group
                    .sleep(self.agent_id, milliseconds)
                    .map_err(|_| ExecutionError::AgentTerminated)?;
                self.checkpoint = self.next_checkpoint();
                Ok(Value::Undefined)
            }
            AgentOp::MonotonicNow => Ok(Value::Number(group.now_ms())),
            AgentOp::ReceiveBroadcast => {
                let payload = group
                    .receive_broadcast(self.agent_id)
                    .map_err(|_| ExecutionError::AgentTerminated)?;
                self.checkpoint = self.next_checkpoint();
                // The same bytes, seen through this realm's own `SharedArrayBuffer`. Not a copy:
                // a copy would make every `Atomics` operation in the agent test suite agree with
                // itself and with nobody else.
                let buffer = agent::adopt_broadcast(&payload);
                self.call_value(&first, Value::Undefined, vec![buffer], instruction)?;
                Ok(Value::Undefined)
            }
            AgentOp::Report => {
                let text = self.as_string(&first, instruction)?;
                group.report(text);
                Ok(Value::Undefined)
            }
            // Nothing to do: an agent's script ending is what leaving *is*, and the scheduler
            // learns of it when the thread finishes. The hook exists because Test262 calls it.
            AgentOp::Leaving => Ok(Value::Undefined),
        }
    }

    /// One of the six `Uint8Array` conversions between bytes and text.
    ///
    /// All six in one place because they are one algorithm read six ways: two directions, two
    /// encodings, and -- for the decoding direction -- a choice between building a view and
    /// filling one. Splitting them would mean six copies of the options reading, whose order and
    /// refusals are exactly what half the tests here are about.
    pub(super) fn byte_text(
        &mut self,
        conversion: ByteText,
        receiver: &Value,
        arguments: &[Value],
        instruction: usize,
    ) -> Result<Value, ExecutionError> {
        let base64 = matches!(
            conversion,
            ByteText::FromBase64 | ByteText::ToBase64 | ByteText::SetFromBase64
        );
        // `ValidateUint8Array`. A wider view is refused rather than reinterpreted: these are
        // defined on the byte view alone, and answering for an `Int32Array` would have to invent
        // a byte order to do it.
        let view = match conversion {
            ByteText::FromBase64 | ByteText::FromHex => None,
            _ => match receiver {
                Value::TypedArray(view) if view.kind == TypedKind::Uint8 => Some(Rc::clone(view)),
                other => {
                    return Err(ExecutionError::TypeErrorMessage {
                        message: format!(
                            "{} called on {}",
                            conversion.key(),
                            other.to_display_string()
                        ),
                    });
                }
            },
        };
        // Which argument carries the options is the one thing the six do not share: a conversion
        // that reads a string takes it first, and one that writes a string takes none at all.
        let (text, options) = match conversion {
            ByteText::ToBase64 | ByteText::ToHex => (None, arguments.first()),
            _ => (Some(arguments.first()), arguments.get(1)),
        };
        // `ToString` is deliberately *not* applied: a decode is given a string or it is refused,
        // because a number that stringifies to something base64 accepts is far more likely to be
        // a mistake than an intention.
        let text = match text {
            None => None,
            Some(Some(Value::String(text))) => Some(text.clone()),
            Some(_) => {
                return Err(ExecutionError::TypeErrorMessage {
                    message: format!("{} requires a string", conversion.key()),
                });
            }
        };
        let mut url = false;
        let mut handling = LastChunk::Loose;
        let mut omit_padding = false;
        // `GetOptionsObject`: absent means the defaults, an object is read, and anything else is
        // refused before a single field is looked at.
        match options {
            None | Some(Value::Undefined) => {}
            Some(bag) if is_reflect_target(bag) => {
                if base64 {
                    // Each field is read once, and the value has to *be* a string: `Object("base64")`
                    // is a `TypeError` and its `toString` is never called, which is what stops a
                    // program from smuggling an option in through a coercion.
                    let named = self.read_property(bag, "alphabet", instruction)?;
                    url = match &named {
                        Value::Undefined => false,
                        Value::String(name) if name == "base64" => false,
                        Value::String(name) if name == "base64url" => true,
                        _ => {
                            return Err(ExecutionError::TypeErrorMessage {
                                message: "alphabet must be \"base64\" or \"base64url\"".to_owned(),
                            });
                        }
                    };
                }
                match conversion {
                    ByteText::ToBase64 => {
                        // The one option that *is* coerced, because it is a flag rather than a
                        // name: the specification says `ToBoolean`, so `{ omitPadding: 1 }` omits.
                        let named = self.read_property(bag, "omitPadding", instruction)?;
                        omit_padding = named.is_truthy();
                    }
                    ByteText::FromBase64 | ByteText::SetFromBase64 => {
                        let named = self.read_property(bag, "lastChunkHandling", instruction)?;
                        handling = match &named {
                            Value::Undefined => LastChunk::Loose,
                            Value::String(name) if name == "loose" => LastChunk::Loose,
                            Value::String(name) if name == "strict" => LastChunk::Strict,
                            Value::String(name) if name == "stop-before-partial" => {
                                LastChunk::StopBeforePartial
                            }
                            _ => {
                                return Err(ExecutionError::TypeErrorMessage {
                                    message: "lastChunkHandling must be \"loose\", \"strict\", or \"stop-before-partial\""
                                        .to_owned(),
                                });
                            }
                        };
                    }
                    _ => {}
                }
            }
            Some(_) => {
                return Err(ExecutionError::TypeErrorMessage {
                    message: "options must be an object".to_owned(),
                });
            }
        }
        // Reading the bytes out comes after the options, and is where a detached buffer stops
        // the whole thing -- `ValidateTypedArray` again, at the point the specification puts it.
        if let Some(view) = &view
            && view.out_of_bounds()
        {
            return Err(ExecutionError::BufferDetached);
        }
        match conversion {
            ByteText::ToBase64 | ByteText::ToHex => {
                let bytes = view.map(|view| view.bytes()).unwrap_or_default();
                Ok(Value::String(if base64 {
                    to_base64_text(&bytes, url, omit_padding).into()
                } else {
                    to_hex_text(&bytes).into()
                }))
            }
            ByteText::FromBase64 | ByteText::FromHex => {
                let text = text.unwrap_or_default();
                let decoded = if base64 {
                    from_base64(&text.to_lossy_string(), url, handling, usize::MAX)
                } else {
                    from_hex(&text.to_lossy_string(), usize::MAX)
                };
                if decoded.refused {
                    return Err(byte_text_refused(base64));
                }
                let items = decoded
                    .bytes
                    .into_iter()
                    .map(|byte| Value::Number(f64::from(byte)))
                    .collect();
                construct_typed_array(TypedKind::Uint8, &[Value::Array(ArrayData::new(items))])
            }
            ByteText::SetFromBase64 | ByteText::SetFromHex => {
                let Some(view) = view else {
                    unreachable!("a set conversion validated its receiver above");
                };
                let text = text.unwrap_or_default();
                // Bounded by the target rather than decoded whole and truncated: the bound is
                // observable, because a chunk that would not fit is left *unread* and reported
                // as such rather than partly written.
                let decoded = if base64 {
                    from_base64(&text.to_lossy_string(), url, handling, view.len())
                } else {
                    from_hex(&text.to_lossy_string(), view.len())
                };
                let written = decoded.bytes.len();
                for (index, byte) in decoded.bytes.into_iter().enumerate() {
                    view.set(index, &Value::Number(f64::from(byte)))?;
                }
                // The write happens first and the refusal second, so a program that catches the
                // refusal still sees the bytes that were good -- which is the whole point of
                // reporting how far a decode got.
                if decoded.refused {
                    return Err(byte_text_refused(base64));
                }
                let mut record = ObjectData::default();
                record.set("read", Value::Number(decoded.read as f64));
                record.set("written", Value::Number(written as f64));
                Ok(Value::Object(Rc::new(RefCell::new(record))))
            }
        }
    }
}
