use super::*;

pub(crate) fn validate(program: &Program) -> Result<usize, EmitError> {
    Ok(validate_program(program)?.body)
}

pub(crate) fn validate_program(program: &Program) -> Result<StackPlan, EmitError> {
    validate_program_with_eval_policy(program, false)
}

/// Module emission can prune unreachable function bodies before deciding whether a direct eval
/// is a live unsupported operation. All other instruction checks remain identical.
pub(crate) fn validate_module_program(program: &Program) -> Result<StackPlan, EmitError> {
    validate_program_with_eval_policy(program, true)
}

fn validate_program_with_eval_policy(
    program: &Program,
    allow_direct_eval: bool,
) -> Result<StackPlan, EmitError> {
    if program.version != BYTECODE_VERSION {
        return Err(EmitError::Unsupported {
            feature: format!(
                "bytecode version {} (backend supports {})",
                program.version, BYTECODE_VERSION
            ),
        });
    }
    if program.code.is_empty() {
        return Err(EmitError::InvalidProgram {
            message: "program has no instructions".into(),
        });
    }
    for proto in &program.functions {
        // A generator does not run when it is called: it builds an object that runs in pieces,
        // each resumption picking up where the last left off. That is a suspended frame, which
        // an automatic C frame cannot be. An `async` function is compiled as one for the same
        // reason -- `await` suspends exactly as `yield` does.
        // An async *generator* suspends for two different reasons -- a value for its consumer
        // and a promise it is waiting on -- and the driver has to tell them apart to know whether
        // the consumer's step is finished. `frame->awaiting`, written at each suspension, is what
        // says which; without it the two emit identically and nothing downstream could tell.
        if proto.parameter_count > proto.local_count {
            return Err(EmitError::InvalidProgram {
                message: format!(
                    "function takes {} parameter(s) but has only {} local slot(s)",
                    proto.parameter_count, proto.local_count
                ),
            });
        }
        if proto.code.is_empty() {
            return Err(EmitError::InvalidProgram {
                message: "function body has no instructions".into(),
            });
        }
    }
    let body = validate_code(
        &program.code,
        program,
        program.local_count,
        0,
        allow_direct_eval,
    )?;
    let functions = program
        .functions
        .iter()
        .map(|proto| {
            validate_code(
                &proto.code,
                program,
                proto.local_count,
                proto.upvalues.len(),
                allow_direct_eval,
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(StackPlan { body, functions })
}

/// Validate one body -- the module's or a function's -- and report the stack depth it reaches.
///
/// `local_count` and `upvalue_count` describe the frame this body runs in, which is why this
/// cannot simply read them off the program: every function has its own.
pub(crate) fn validate_code(
    code: &[Instruction],
    program: &Program,
    local_count: u32,
    upvalue_count: usize,
    allow_direct_eval: bool,
) -> Result<usize, EmitError> {
    for (index, instruction) in code.iter().enumerate() {
        if !allow_direct_eval || !matches!(instruction, Instruction::DirectEval(_)) {
            validate_supported_instruction(instruction)?;
        }
        match instruction {
            Instruction::Const(constant) if (*constant as usize) >= program.constants.len() => {
                return Err(invalid(
                    index,
                    format!("constant {constant} is out of bounds"),
                ));
            }
            Instruction::LoadTemplateObject(site)
                if (*site as usize) >= program.template_sites.len() =>
            {
                return Err(invalid(
                    index,
                    format!("template site {site} is out of bounds"),
                ));
            }
            // The interpreter can make a second realm; this backend cannot. Its global object is
            // built once in C, in one copy, so a compiled program that reached the hook would get
            // a plausible-looking realm whose intrinsics were secretly this one's -- and every
            // cross-realm identity question would then be answered confidently and wrongly.
            // Refused by name instead, which is what the interpreter does when its host has not
            // granted the capability.
            Instruction::RequireGlobalBinding(name)
            | Instruction::LoadGlobalBindingOrUndefined(name)
            // A name this backend has no global for never reaches the two above: the compiler
            // emits the lookup and then `ThrowReference` for the miss, so refusing only the
            // lookup let `new ShadowRealm()` compile and answer `ReferenceError` at run time --
            // which reads to a program as "this engine does not have it" rather than "this
            // backend cannot do it", and is the difference between a refusal and a wrong answer
            // about what the language provides.
            | Instruction::ThrowReference(name)
            | Instruction::StoreGlobalBinding(name)
            | Instruction::StoreGlobalBindingStrict(name)
            | Instruction::DeleteGlobalBinding(name)
            | Instruction::DeclareGlobalBinding { name, .. }
            | Instruction::GetProperty(name)
            | Instruction::CallMethod { name, .. }
                if matches!(
                    program.constants.get(*name as usize),
                    Some(Constant::String(text))
                        if text.to_scalar_string().ok().is_some_and(|text| {
                            refused_host_hook(&text)
                                || text == HOST_CREATE_REALM
                                || text == HOST_EVAL_SCRIPT
                                || text == SHADOW_REALM
                        })
                ) =>
            {
                let hook = match program.constants.get(*name as usize) {
                    Some(Constant::String(text)) => text
                        .to_scalar_string()
                        .unwrap_or_else(|_| "<non-scalar host hook>".into()),
                    _ => HOST_CREATE_REALM.to_owned(),
                };
                return Err(EmitError::Unsupported {
                    feature: format!("host realm facility {hook} at instruction {index}"),
                });
            }
            // A property name is emitted by naming the constant's own C symbol, and only a
            // string constant has one.
            Instruction::GetProperty(name)
            | Instruction::SetProperty(name)
            | Instruction::DefineOwnProperty(name)
            | Instruction::DeleteProperty(name)
            | Instruction::SetPropertyStrict { name, .. }
            | Instruction::SetSuperPropertyStrict { name, .. }
            | Instruction::DeletePropertyStrict { name, .. }
            | Instruction::DeleteGlobalBinding(name)
            | Instruction::StoreGlobalBindingStrict(name)
            | Instruction::RequireGlobalBinding(name)
            | Instruction::LoadGlobalBindingOrUndefined(name)
            | Instruction::DeclareGlobalBinding { name, .. }
            | Instruction::CallMethod { name, .. } => match program.constants.get(*name as usize) {
                Some(Constant::String(_)) => {}
                Some(_) => {
                    return Err(invalid(
                        index,
                        format!("property name constant {name} is not a string"),
                    ));
                }
                None => {
                    return Err(invalid(
                        index,
                        format!("property name constant {name} is out of bounds"),
                    ));
                }
            },
            Instruction::LoadLocal(local)
            | Instruction::StoreLocal(local)
            | Instruction::NewCell(local)
            | Instruction::LoadCell(local)
            | Instruction::StoreCell(local)
            | Instruction::MarkUninitialized(local)
                if (*local as u64) >= u64::from(local_count) =>
            {
                return Err(invalid(index, format!("local {local} is out of bounds")));
            }
            Instruction::LoadGlobal(slot) | Instruction::StoreGlobal(slot)
                if u64::from(*slot) >= u64::from(program.local_count) =>
            {
                return Err(invalid(
                    index,
                    format!("module slot {slot} is out of bounds"),
                ));
            }
            Instruction::LoadUpvalue(upvalue) | Instruction::StoreUpvalue(upvalue)
                if (*upvalue as usize) >= upvalue_count =>
            {
                return Err(invalid(
                    index,
                    format!("upvalue {upvalue} is out of bounds"),
                ));
            }
            Instruction::Jump(target)
            | Instruction::JumpIfFalse(target)
            | Instruction::JumpIfTruthy(target)
            | Instruction::JumpIfNotNullish(target)
            | Instruction::PushHandler(target)
            | Instruction::YieldStar(target)
            | Instruction::JumpIfReturnSignal(target)
                if (*target as usize) >= code.len() =>
            {
                return Err(invalid(
                    index,
                    format!("jump target {target} is out of bounds"),
                ));
            }
            Instruction::HostCall { op, .. } if !matches!(op, HostOp::WriteStdout) => {
                return Err(EmitError::Unsupported {
                    feature: format!("host operation {op:?} at instruction {index}"),
                });
            }
            // Where a closure's captures are checked: the descriptors name slots in *this*
            // frame, so this is the only place that knows whether they exist.
            Instruction::MakeFunction(proto) => {
                let Some(target) = program.functions.get(*proto as usize) else {
                    return Err(invalid(index, format!("function {proto} is out of bounds")));
                };
                for upvalue in &target.upvalues {
                    let ok = match upvalue {
                        Upvalue::ParentLocal(slot)
                        | Upvalue::NamedParentLocal { slot, .. } => {
                            u64::from(*slot) < u64::from(local_count)
                        }
                        Upvalue::ParentUpvalue(slot)
                        | Upvalue::NamedParentUpvalue { index: slot, .. } => {
                            (*slot as usize) < upvalue_count
                        }
                    };
                    if !ok {
                        return Err(invalid(
                            index,
                            format!(
                                "function {proto} captures {upvalue:?}, which this frame does not have"
                            ),
                        ));
                    }
                }
            }
            _ => {}
        }
    }

    // Dataflow validation keeps generated stack indexing defined on every reachable branch and
    // calculates a compact allocation for the emitted function.
    let mut heights: Vec<Option<usize>> = vec![None; code.len()];
    let mut work = vec![(0usize, 0usize)];
    let mut max_height = 0usize;
    while let Some((index, height)) = work.pop() {
        if let Some(previous) = heights[index] {
            if previous != height {
                return Err(invalid(
                    index,
                    format!("inconsistent stack height: reached with {previous} and {height}"),
                ));
            }
            continue;
        }
        heights[index] = Some(height);
        max_height = max_height.max(height);
        let instruction = &code[index];
        let (next_height, terminal) = stack_effect(instruction, height, index)?;
        max_height = max_height.max(next_height);
        if terminal {
            continue;
        }
        match instruction {
            // A handler is entered with the thrown value pushed onto the stack as the enclosing
            // `try` left it, which is why the edge carries one more than the fall-through.
            Instruction::PushHandler(target) => {
                work.push((*target as usize, next_height + 1));
                if index + 1 < code.len() {
                    work.push((index + 1, next_height));
                } else {
                    return Err(invalid(index, "control falls off the end of the program"));
                }
            }
            Instruction::Jump(target) => work.push((*target as usize, next_height)),
            Instruction::JumpIfFalse(target)
            | Instruction::JumpIfTruthy(target)
            | Instruction::JumpIfNotNullish(target)
            | Instruction::YieldStar(target)
            | Instruction::JumpIfReturnSignal(target) => {
                work.push((*target as usize, next_height));
                if index + 1 < code.len() {
                    work.push((index + 1, next_height));
                }
            }
            _ if index + 1 < code.len() => work.push((index + 1, next_height)),
            _ => {
                return Err(invalid(index, "control falls off the end of the program"));
            }
        }
    }
    Ok(max_height.max(1))
}

/// Whether the backend can emit this instruction.
///
/// There is no catch-all arm any more: **every instruction in the set is emitted**, and the match
/// is exhaustive so that adding one to `kiln-ir` stops this compiling until somebody decides what
/// the backend does about it. A refusal that has to be written down beats one that happens by
/// default -- and the default was the whole reason the tally in NOTES.md kept finding things.
///
/// A *builtin* can still be refused: `LoadBuiltin` names one value out of a large enum and the
/// arm below lists the ones that exist.
fn validate_supported_instruction(instruction: &Instruction) -> Result<(), EmitError> {
    match instruction {
        // A template's `${x}` lowers to a call to `String`, so this is not an optional extra:
        // without it the backend loses every program containing a template literal.
        Instruction::LoadBuiltin(
            kiln_ir::Builtin::String
            | kiln_ir::Builtin::Math
            | kiln_ir::Builtin::Number
            | kiln_ir::Builtin::Object
            | kiln_ir::Builtin::Array
            | kiln_ir::Builtin::Json
            | kiln_ir::Builtin::Date
            | kiln_ir::Builtin::Map
            | kiln_ir::Builtin::Set
            | kiln_ir::Builtin::WeakMap
            | kiln_ir::Builtin::WeakSet
            | kiln_ir::Builtin::Promise
            | kiln_ir::Builtin::Error
            | kiln_ir::Builtin::TypeError
            | kiln_ir::Builtin::RangeError
            | kiln_ir::Builtin::ReferenceError
            | kiln_ir::Builtin::SyntaxError
            | kiln_ir::Builtin::UriError
            | kiln_ir::Builtin::EvalError
            | kiln_ir::Builtin::Eval
            | kiln_ir::Builtin::AggregateError
            | kiln_ir::Builtin::SuppressedError
            | kiln_ir::Builtin::DisposableStack
            | kiln_ir::Builtin::AsyncDisposableStack
            | kiln_ir::Builtin::NotANumber
            | kiln_ir::Builtin::Infinity
            | kiln_ir::Builtin::Boolean
            | kiln_ir::Builtin::ParseInt
            | kiln_ir::Builtin::ParseFloat
            | kiln_ir::Builtin::IsNaN
            | kiln_ir::Builtin::IsFinite
            | kiln_ir::Builtin::Escape
            | kiln_ir::Builtin::Unescape
            | kiln_ir::Builtin::EncodeUri
            | kiln_ir::Builtin::EncodeUriComponent
            | kiln_ir::Builtin::DecodeUri
            | kiln_ir::Builtin::DecodeUriComponent
            | kiln_ir::Builtin::Reflect
            | kiln_ir::Builtin::Proxy
            | kiln_ir::Builtin::QueueMicrotask
            | kiln_ir::Builtin::SetTimeout
            | kiln_ir::Builtin::SetInterval
            | kiln_ir::Builtin::ClearTimeout
            | kiln_ir::Builtin::ClearInterval
            | kiln_ir::Builtin::Symbol
            | kiln_ir::Builtin::RegExp
            | kiln_ir::Builtin::GlobalThis
            | kiln_ir::Builtin::BigInt
            | kiln_ir::Builtin::Function
            | kiln_ir::Builtin::ArrayBuffer
            | kiln_ir::Builtin::Int8Array
            | kiln_ir::Builtin::Uint8Array
            | kiln_ir::Builtin::Uint8ClampedArray
            | kiln_ir::Builtin::Int16Array
            | kiln_ir::Builtin::Uint16Array
            | kiln_ir::Builtin::Int32Array
            | kiln_ir::Builtin::Uint32Array
            | kiln_ir::Builtin::Float32Array
            | kiln_ir::Builtin::Float64Array
            | kiln_ir::Builtin::Url
            | kiln_ir::Builtin::Buffer
            | kiln_ir::Builtin::Process
            | kiln_ir::Builtin::Atomics
            | kiln_ir::Builtin::SetImmediate
            | kiln_ir::Builtin::ClearImmediate
            | kiln_ir::Builtin::AbortController
            | kiln_ir::Builtin::AbortSignal
            | kiln_ir::Builtin::TextEncoder
            | kiln_ir::Builtin::TextDecoder
            | kiln_ir::Builtin::Performance
            | kiln_ir::Builtin::StructuredClone,
        ) => Ok(()),
        Instruction::LoadIntrinsicBuiltin(kiln_ir::Builtin::Eval) => Ok(()),
        Instruction::LoadIntrinsicBuiltin(builtin) => Err(EmitError::Unsupported {
            feature: format!("the intrinsic `{builtin:?}` builtin in the native backend"),
        }),
        // Source admission is checked by the dynamic expression seam at execution time.
        Instruction::DirectEval(_) => Ok(()),
        Instruction::ToObject | Instruction::WithHas(_) => Err(EmitError::Unsupported {
            feature: "a `with` statement in the native backend".to_owned(),
        }),
        Instruction::DynamicImport
        | Instruction::DynamicImportWithOptions
        | Instruction::Const(_)
        | Instruction::LoadTemplateObject(_)
        | Instruction::LoadLocal(_)
        | Instruction::StoreLocal(_)
        | Instruction::NewCell(_)
        | Instruction::LoadCell(_)
        | Instruction::StoreCell(_)
        | Instruction::MakeFunction(_)
        | Instruction::Call { .. }
        | Instruction::LoadUpvalue(_)
        | Instruction::StoreUpvalue(_)
        | Instruction::MarkUninitialized(_)
        | Instruction::NewObject { .. }
        | Instruction::GetProperty(_)
        | Instruction::SetProperty(_)
        | Instruction::SetPropertyStrict { .. }
        | Instruction::DefineOwnProperty(_)
        | Instruction::NewArray { .. }
        | Instruction::ToPropertyKey
        | Instruction::GetComputed
        | Instruction::SetComputed
        | Instruction::SetComputedStrict { .. }
        | Instruction::DefineOwnComputed
        | Instruction::CallMethod { .. }
        | Instruction::LoadThis
        | Instruction::PushHandler(_)
        | Instruction::PopHandler
        | Instruction::Throw
        | Instruction::ThrowReference(_)
        | Instruction::StoreGlobalBinding(_)
        | Instruction::StoreGlobalBindingStrict(_)
        | Instruction::RequireGlobalBinding(_)
        | Instruction::LoadGlobalBindingOrUndefined(_)
        | Instruction::DeleteGlobalBinding(_)
        | Instruction::DeclareGlobalBinding { .. }
        | Instruction::GetPrivate(_)
        | Instruction::SetPrivate(_)
        | Instruction::Dup2
        | Instruction::CallComputed { .. }
        | Instruction::NewRegExp { .. }
        | Instruction::New { .. }
        | Instruction::SetPrototype
        | Instruction::LoadPrototype
        | Instruction::LoadSuperProperty(_)
        | Instruction::LoadSuperElement
        | Instruction::SetSuperPropertyStrict { .. }
        | Instruction::SetSuperElementStrict { .. }
        | Instruction::LoadGlobal(_)
        | Instruction::StoreGlobal(_)
        | Instruction::DefineAccessor { .. }
        | Instruction::DefineMethod
        | Instruction::DefineNamedProperty { .. }
        | Instruction::DefinePrivate { .. }
        | Instruction::SpreadObject
        | Instruction::SpreadInto
        | Instruction::ConstructSpread
        | Instruction::IterableToArray
        | Instruction::EnumerateKeys
        | Instruction::DeleteProperty(_)
        | Instruction::DeletePropertyStrict { .. }
        | Instruction::DeleteComputed
        | Instruction::DeleteComputedStrict { .. }
        | Instruction::Apply
        | Instruction::RestArguments(_)
        | Instruction::PushHole
        | Instruction::Yield
        | Instruction::YieldStar(_)
        | Instruction::SuperCall { .. }
        | Instruction::SuperApply
        | Instruction::SuperForward
        | Instruction::LoadNewTarget
        | Instruction::LoadArguments
        | Instruction::Await
        | Instruction::Pop
        | Instruction::Dup
        | Instruction::Unary(_)
        | Instruction::Binary(_)
        | Instruction::Jump(_)
        | Instruction::JumpIfFalse(_)
        | Instruction::JumpIfTruthy(_)
        | Instruction::JumpIfNotNullish(_)
        | Instruction::JumpIfReturnSignal(_)
        | Instruction::HostCall { .. }
        | Instruction::Nop
        | Instruction::Return => Ok(()),
    }
}

pub(crate) fn validate_module(module: &ModuleProgram) -> Result<usize, EmitError> {
    let max_stack = validate_module_program(&module.program)?.body;
    let local_count = u64::from(module.program.local_count);
    let mut initialized_slots = HashSet::with_capacity(module.imports.len());
    for (index, binding) in module.imports.iter().enumerate() {
        if u64::from(binding.local_slot) >= local_count {
            return Err(EmitError::InvalidProgram {
                message: format!(
                    "import {index} local slot {} is out of bounds (local count {})",
                    binding.local_slot, module.program.local_count
                ),
            });
        }
        if !initialized_slots.insert(binding.local_slot) {
            return Err(EmitError::InvalidProgram {
                message: format!(
                    "import {index} local slot {} is initialized more than once",
                    binding.local_slot
                ),
            });
        }
    }
    for (index, binding) in module.exports.iter().enumerate() {
        if u64::from(binding.local_slot) >= local_count {
            return Err(EmitError::InvalidProgram {
                message: format!(
                    "export {index} local slot {} is out of bounds (local count {})",
                    binding.local_slot, module.program.local_count
                ),
            });
        }
    }
    Ok(max_stack)
}

pub(crate) fn invalid(index: usize, message: impl Into<String>) -> EmitError {
    EmitError::InvalidProgram {
        message: format!("instruction {index}: {}", message.into()),
    }
}

pub(crate) fn stack_effect(
    instruction: &Instruction,
    height: usize,
    index: usize,
) -> Result<(usize, bool), EmitError> {
    kiln_graph::validation::stack_effect(instruction, height, index)
        .map_err(|error| invalid(error.index, error.to_string()))
}
