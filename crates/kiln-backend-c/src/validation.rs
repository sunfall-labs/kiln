use super::*;

pub(crate) fn validate(program: &Program) -> Result<usize, EmitError> {
    Ok(validate_program(program)?.body)
}

pub(crate) fn validate_program(program: &Program) -> Result<StackPlan, EmitError> {
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
    let body = validate_code(&program.code, program, program.local_count, 0)?;
    let functions = program
        .functions
        .iter()
        .map(|proto| {
            validate_code(
                &proto.code,
                program,
                proto.local_count,
                proto.upvalues.len(),
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
) -> Result<usize, EmitError> {
    for (index, instruction) in code.iter().enumerate() {
        validate_supported_instruction(instruction)?;
        match instruction {
            Instruction::Const(constant) if (*constant as usize) >= program.constants.len() => {
                return Err(invalid(
                    index,
                    format!("constant {constant} is out of bounds"),
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
                        if refused_host_hook(text)
                            || text == HOST_CREATE_REALM
                            || text == HOST_EVAL_SCRIPT
                            || text == SHADOW_REALM
                ) =>
            {
                let hook = match program.constants.get(*name as usize) {
                    Some(Constant::String(text)) => text.clone(),
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
            | Instruction::DeleteProperty(name)
            | Instruction::SetPropertyStrict { name, .. }
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
                        Upvalue::ParentLocal(slot) => u64::from(*slot) < u64::from(local_count),
                        Upvalue::ParentUpvalue(slot) => (*slot as usize) < upvalue_count,
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
            | Instruction::JumpIfNotNullish(target) => {
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
            | kiln_ir::Builtin::AggregateError
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
            | kiln_ir::Builtin::Performance,
        ) => Ok(()),
        Instruction::LoadBuiltin(builtin) => Err(EmitError::Unsupported {
            feature: format!("the `{builtin:?}` builtin in the native backend"),
        }),
        // A direct `eval` needs the compiler at run time: it reads the caller's bindings and
        // compiles source that does not exist until the call happens. The native backend ships
        // without a compiler, so this is refused rather than approximated -- an indirect `eval`
        // is a different instruction and is unaffected.
        Instruction::DirectEval(_) => Err(EmitError::Unsupported {
            feature: "a direct `eval` call in the native backend".to_owned(),
        }),
        // `import(specifier)` resolves and loads a module while the program runs. A generated
        // unit is a closed object file: the module graph was settled when it was emitted, and
        // there is nothing at run time to resolve a specifier against or to compile what it
        // names. Refused rather than answered with a rejected promise, which would be a wrong
        // answer wearing the right shape.
        Instruction::DynamicImport => Err(EmitError::Unsupported {
            feature: "a dynamic `import()` in the native backend".to_owned(),
        }),
        // `with (x) { ... }` puts an object in front of the scope chain, so every name in the
        // body is resolved at run time against whatever properties that object turns out to
        // have -- and against `Symbol.unscopables`, which can hide one. The backend resolves
        // names when it emits, into slots; there is no scope left at run time for an object to
        // sit in front of. Both halves are refused together, because half of a `with` is not a
        // smaller feature but a wrong one.
        Instruction::ToObject | Instruction::WithHas(_) => Err(EmitError::Unsupported {
            feature: "a `with` statement in the native backend".to_owned(),
        }),
        Instruction::Const(_)
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
        | Instruction::NewArray { .. }
        | Instruction::ToPropertyKey
        | Instruction::GetComputed
        | Instruction::SetComputed
        | Instruction::SetComputedStrict { .. }
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
        | Instruction::LoadGlobal(_)
        | Instruction::StoreGlobal(_)
        | Instruction::DefineAccessor { .. }
        | Instruction::DefineMethod
        | Instruction::DefineNamedProperty
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
        | Instruction::HostCall { .. }
        | Instruction::Return => Ok(()),
    }
}

pub(crate) fn validate_module(module: &ModuleProgram) -> Result<usize, EmitError> {
    let max_stack = validate(&module.program)?;
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

fn stack_effect(
    instruction: &Instruction,
    height: usize,
    index: usize,
) -> Result<(usize, bool), EmitError> {
    let underflow = |needed: usize| {
        invalid(
            index,
            format!("stack needs {needed} value(s), but has {height}"),
        )
    };
    match instruction {
        // Calls are interpreter-only for now. The native backend emits one C function per
        // module with a flat local array and no call stack, so these have no lowering yet --
        // and refusing here is what keeps the two backends from silently disagreeing about
        // what a program means. `kiln run` works on these; `kiln run --native` says why not.
        Instruction::Const(_)
        | Instruction::LoadLocal(_)
        | Instruction::LoadCell(_)
        | Instruction::LoadUpvalue(_)
        | Instruction::LoadBuiltin(_)
        | Instruction::LoadGlobal(_)
        | Instruction::RestArguments(_)
        | Instruction::PushHole
        | Instruction::LoadArguments
        | Instruction::MakeFunction(_) => Ok((height + 1, false)),
        // A literal consumes what it is built from and leaves the container.
        Instruction::NewObject { count } => {
            let needed = usize::try_from(*count)
                .unwrap_or(usize::MAX)
                .saturating_mul(2);
            if height < needed {
                Err(underflow(needed))
            } else {
                Ok((height - needed + 1, false))
            }
        }
        Instruction::NewArray { count } => {
            let needed = usize::try_from(*count).unwrap_or(usize::MAX);
            if height < needed {
                Err(underflow(needed))
            } else {
                Ok((height - needed + 1, false))
            }
        }
        // The object is replaced by what was read; the key and value are consumed with it.
        Instruction::GetProperty(_) => {
            if height < 1 {
                Err(underflow(1))
            } else {
                Ok((height, false))
            }
        }
        // Height-neutral: the key is replaced by its converted self. It needs the object beneath
        // it as well, which it reads without consuming.
        Instruction::ToPropertyKey => {
            if height < 2 {
                Err(underflow(2))
            } else {
                Ok((height, false))
            }
        }
        Instruction::GetComputed => {
            if height < 2 {
                Err(underflow(2))
            } else {
                Ok((height - 1, false))
            }
        }
        // A write leaves the value it wrote, which is what makes `a = b.c = d` work.
        Instruction::SetProperty(_) | Instruction::SetPropertyStrict { .. } => {
            if height < 2 {
                Err(underflow(2))
            } else {
                Ok((height - 1, false))
            }
        }
        Instruction::SetComputed | Instruction::SetComputedStrict { .. } => {
            if height < 3 {
                Err(underflow(3))
            } else {
                Ok((height - 2, false))
            }
        }
        Instruction::LoadThis => Ok((height + 1, false)),
        // The parent initialises the *existing* instance rather than allocating one, but it is
        // still a call and still leaves what it returned -- which the lowering pops, because
        // `super()` is a statement.
        Instruction::SuperCall { argc } => {
            let needed = usize::from(*argc) + 1;
            if height < needed {
                Err(underflow(needed))
            } else {
                Ok((height - needed + 1, false))
            }
        }
        // The arguments come from the frame rather than the stack, so only the parent is here
        // and the result replaces it.
        Instruction::SuperForward => {
            if height < 1 {
                Err(underflow(1))
            } else {
                Ok((height, false))
            }
        }
        // `new` consumes the callee and its arguments and leaves the instance, exactly as a
        // call does.
        Instruction::New { argc } => {
            let needed = usize::from(*argc) + 1;
            if height < needed {
                Err(underflow(needed))
            } else {
                Ok((height - needed + 1, false))
            }
        }
        // The half and the key are consumed; the target stays, so a class body can define
        // several accessors on one object without reloading it.
        Instruction::DefineAccessor { .. }
        | Instruction::DefineMethod
        | Instruction::DefineNamedProperty => {
            if height < 3 {
                Err(underflow(3))
            } else {
                Ok((height - 2, false))
            }
        }
        // Same shape with the key in the operand rather than on the stack: the function is
        // consumed and the target stays.
        Instruction::DefinePrivate { .. } => {
            if height < 2 {
                Err(underflow(2))
            } else {
                Ok((height - 1, false))
            }
        }
        // Replaces the value with an array of its own enumerable keys.
        Instruction::EnumerateKeys
        | Instruction::DeleteProperty(_)
        | Instruction::DeletePropertyStrict { .. } => {
            if height < 1 {
                Err(underflow(1))
            } else {
                Ok((height, false))
            }
        }
        Instruction::DeleteComputed | Instruction::DeleteComputedStrict { .. } => {
            if height < 2 {
                Err(underflow(2))
            } else {
                Ok((height - 1, false))
            }
        }
        // `[this, callee, arguments] -> [result]`: the arity is the length of an array the
        // program computed, which is exactly what a spread means.
        Instruction::Apply => {
            if height < 3 {
                Err(underflow(3))
            } else {
                Ok((height - 2, false))
            }
        }
        // `super(...xs)`: the parent and the computed argument list come off, its result goes on.
        Instruction::SuperApply => {
            if height < 2 {
                Err(underflow(2))
            } else {
                Ok((height - 1, false))
            }
        }
        // Replaces the value with an array of what it yields.
        Instruction::IterableToArray => {
            if height < 1 {
                Err(underflow(1))
            } else {
                Ok((height, false))
            }
        }
        // The source is consumed and the target stays, the same shape `SetPrototype` uses.
        Instruction::SpreadObject | Instruction::SpreadInto => {
            if height < 2 {
                Err(underflow(2))
            } else {
                Ok((height - 1, false))
            }
        }
        // The prototype is written into the target, which stays.
        Instruction::SetPrototype => {
            if height < 2 {
                Err(underflow(2))
            } else {
                Ok((height - 1, false))
            }
        }
        // The property replaces the prototype it was read from.
        Instruction::LoadSuperProperty(_) => {
            if height < 1 {
                Err(underflow(1))
            } else {
                Ok((height, false))
            }
        }
        // The value replaces both the prototype and the key that were read from.
        Instruction::LoadSuperElement => {
            if height < 2 {
                Err(underflow(2))
            } else {
                Ok((height - 1, false))
            }
        }
        // The link replaces the value it was read from.
        Instruction::LoadPrototype => {
            if height < 1 {
                Err(underflow(1))
            } else {
                Ok((height, false))
            }
        }
        // The receiver sits under the arguments, exactly where a callee would, and the result
        // replaces both -- so a method call and a plain call have the same shape here.
        Instruction::CallMethod { argc, .. } => {
            let needed = usize::from(*argc) + 1;
            if height < needed {
                Err(underflow(needed))
            } else {
                Ok((height - needed + 1, false))
            }
        }
        // The callee sits under its arguments, and the result replaces both.
        Instruction::Call { argc } => {
            let needed = usize::from(*argc) + 1;
            if height < needed {
                Err(underflow(needed))
            } else {
                Ok((height - needed + 1, false))
            }
        }
        // Refused by `validate_supported_instruction`, so nothing is ever emitted for it. The
        // arithmetic is still written out rather than waved away: `stack_effect` is what proves
        // the height claims of a program consistent, and a lie here would be a lie about a
        // program the interpreter runs happily. `argc` is `None` when a spread put the arguments
        // in one array, which leaves exactly one value to consume.
        Instruction::DirectEval(site) => {
            let needed = usize::from(site.argc.unwrap_or(1));
            if height < needed {
                Err(underflow(needed))
            } else {
                Ok((height - needed + 1, false))
            }
        }
        // Refused by `validate_supported_instruction` as well; the specifier becomes the
        // promise, so the height is unchanged. `with`'s two are the same shape: each replaces
        // the value on top with one derived from it.
        Instruction::DynamicImport | Instruction::ToObject | Instruction::WithHas(_) => {
            if height < 1 {
                Err(underflow(1))
            } else {
                Ok((height, false))
            }
        }
        // Both write a slot in place and touch no stack.
        Instruction::NewCell(_) | Instruction::MarkUninitialized(_) => Ok((height, false)),
        // Registering and retiring a handler leave the stack alone; the *target* is reached with
        // one more value on it, which the dataflow walk below accounts for.
        Instruction::PushHandler(_) | Instruction::PopHandler => Ok((height, false)),
        // A yield consumes the value and leaves one behind: what the caller sends back becomes
        // the result of the `yield` expression. `await` is identical in the machine -- the two
        // differ only in what the driver does with the value, which is not the body's business.
        Instruction::Yield | Instruction::Await => {
            if height < 1 {
                Err(underflow(1))
            } else {
                Ok((height, false))
            }
        }
        // The name is a constant rather than a stack value. Control does *not* necessarily leave:
        // the instruction stands in for a value load, and the emitted runtime resolves the name
        // against the realm's global object before throwing -- so on the found path it pushes
        // exactly what `LoadBuiltin` would have, and the instructions after it were compiled
        // expecting that value. Modelling it as terminal would leave those instructions verified
        // at a height one short of the one the machine actually runs them at.
        Instruction::ThrowReference(_) => Ok((height + 1, false)),
        // A sloppy assignment to an unbound name. The value is consumed and the property lands on
        // the global object, so nothing is left behind: the compound and expression-valued forms
        // duplicate the value themselves before the store.
        Instruction::StoreGlobalBinding(_) | Instruction::StoreGlobalBindingStrict(_) => {
            if height < 1 {
                Err(underflow(1))
            } else {
                Ok((height - 1, false))
            }
        }
        // `delete unboundName` takes nothing and leaves the boolean the global object's
        // `[[Delete]]` answered.
        Instruction::DeleteGlobalBinding(_) => Ok((height + 1, false)),
        // A declaration touches the global object and leaves the stack where it found it.
        Instruction::DeclareGlobalBinding { .. } => Ok((height, false)),
        // A check, not a value: it either throws or leaves the stack alone.
        Instruction::RequireGlobalBinding(_) => Ok((height, false)),
        // A read that cannot fail: the value, or `undefined` when the realm has no such name.
        Instruction::LoadGlobalBindingOrUndefined(_) => Ok((height + 1, false)),
        // A literal produces a fresh object, so it only ever grows the stack.
        Instruction::NewRegExp { .. } => Ok((height + 1, false)),
        Instruction::LoadNewTarget => Ok((height + 1, false)),
        Instruction::Dup2 => {
            if height < 2 {
                Err(underflow(2))
            } else {
                Ok((height + 2, false))
            }
        }
        // The receiver, the key and the arguments are all consumed and the result replaces them.
        Instruction::CallComputed { argc } => {
            let needed = usize::from(*argc).saturating_add(2);
            if height < needed {
                Err(underflow(needed))
            } else {
                Ok((height - needed + 1, false))
            }
        }
        // The callee and the argument list are both consumed and the instance replaces them.
        Instruction::ConstructSpread => {
            if height < 2 {
                Err(underflow(2))
            } else {
                Ok((height - 1, false))
            }
        }
        // The object is replaced by what was read, exactly as `GetProperty` is.
        Instruction::GetPrivate(_) => {
            if height < 1 {
                Err(underflow(1))
            } else {
                Ok((height, false))
            }
        }
        // ...and the value replaces both, exactly as `SetProperty` does: an assignment is an
        // expression and its value is what was assigned.
        Instruction::SetPrivate(_) => {
            if height < 2 {
                Err(underflow(2))
            } else {
                Ok((height - 1, false))
            }
        }
        // A throw consumes the value and does not fall through: control leaves for a handler.
        Instruction::Throw => {
            if height < 1 {
                Err(underflow(1))
            } else {
                Ok((height - 1, true))
            }
        }
        Instruction::StoreLocal(_)
        | Instruction::StoreCell(_)
        | Instruction::StoreUpvalue(_)
        | Instruction::StoreGlobal(_)
        | Instruction::Pop => {
            if height < 1 {
                Err(underflow(1))
            } else {
                Ok((height - 1, false))
            }
        }
        Instruction::Dup | Instruction::Unary(_) => {
            if height < 1 {
                Err(underflow(1))
            } else {
                Ok((
                    height + usize::from(matches!(instruction, Instruction::Dup)),
                    false,
                ))
            }
        }
        Instruction::Binary(_) => {
            if height < 2 {
                Err(underflow(2))
            } else {
                Ok((height - 1, false))
            }
        }
        Instruction::Jump(_) => Ok((height, false)),
        Instruction::JumpIfFalse(_)
        | Instruction::JumpIfTruthy(_)
        | Instruction::JumpIfNotNullish(_) => {
            if height < 1 {
                Err(underflow(1))
            } else {
                Ok((height - 1, false))
            }
        }
        Instruction::HostCall { argc, .. } => {
            let argc = usize::from(*argc);
            if height < argc {
                Err(underflow(argc))
            } else {
                Ok((height - argc + 1, false))
            }
        }
        // A return takes the top of the stack and abandons the frame, so anything under it is
        // simply gone. Residue is not a malformed program: `try { return 1 } finally { return 2 }`
        // reaches the second return with the first one's value still pending underneath, and the
        // interpreter has always truncated rather than complained. Requiring an empty stack here
        // rejected that shape outright, which is a refusal the language does not licence.
        Instruction::Return => Ok((height, true)),
    }
}
