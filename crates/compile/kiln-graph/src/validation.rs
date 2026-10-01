use kiln_ir::Instruction;
use std::fmt::{Display, Formatter};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StackEffectError {
    pub index: usize,
    pub needed: usize,
    pub actual: usize,
}

impl Display for StackEffectError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "stack needs {} value(s), but has {}",
            self.needed, self.actual
        )
    }
}

pub fn stack_effect(
    instruction: &Instruction,
    height: usize,
    index: usize,
) -> Result<(usize, bool), StackEffectError> {
    let underflow = |needed: usize| StackEffectError {
        index,
        needed,
        actual: height,
    };
    match instruction {
        // Calls are interpreter-only for now. The native backend emits one C function per
        // module with a flat local array and no call stack, so these have no lowering yet --
        // and refusing here is what keeps the two backends from silently disagreeing about
        // what a program means. `kiln run` works on these; `kiln run --native` says why not.
        Instruction::Const(_)
        | Instruction::LoadTemplateObject(_)
        | Instruction::LoadLocal(_)
        | Instruction::LoadCell(_)
        | Instruction::LoadUpvalue(_)
        | Instruction::LoadBuiltin(_)
        | Instruction::LoadIntrinsicBuiltin(_)
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
        Instruction::SetProperty(_)
        | Instruction::SetPropertyStrict { .. }
        | Instruction::DefineOwnProperty(_) => {
            if height < 2 {
                Err(underflow(2))
            } else {
                Ok((height - 1, false))
            }
        }
        Instruction::SetComputed
        | Instruction::SetComputedStrict { .. }
        | Instruction::DefineOwnComputed => {
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
        | Instruction::DefineNamedProperty { .. } => {
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
        // Super write consumes prototype and value, leaving value.
        Instruction::SetSuperPropertyStrict { .. } => {
            if height < 2 {
                Err(underflow(2))
            } else {
                Ok((height - 1, false))
            }
        }
        // Super computed write consumes key, prototype, and value, leaving value.
        Instruction::SetSuperElementStrict { .. } => {
            if height < 3 {
                Err(underflow(3))
            } else {
                Ok((height - 2, false))
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
        // The graph bridge and native tail emitter consume the same call shape. `argc` is `None`
        // when a spread put the arguments in one array, which leaves one argument value below
        // the callee.
        Instruction::DirectEval(site) => {
            // The source reference to `eval` is read before its arguments, so accessors and
            // mutations run in ordinary call-expression order. The callee remains below the
            // fixed arguments, or below the materialized spread list.
            let needed = usize::from(site.argc.unwrap_or(1)) + 1;
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
        Instruction::DynamicImportWithOptions => {
            if height < 2 {
                Err(underflow(2))
            } else {
                Ok((height - 1, false))
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
        Instruction::YieldStar(_) => {
            if height < 3 {
                Err(underflow(3))
            } else {
                Ok((height - 2, false))
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
        // The brand rides on top of the receiver and is consumed with it: the value replaces
        // both, so the height falls by one.
        Instruction::GetPrivate(_) => {
            if height < 2 {
                Err(underflow(2))
            } else {
                Ok((height - 1, false))
            }
        }
        // ...and the value replaces both, exactly as `SetProperty` does: an assignment is an
        // expression and its value is what was assigned.
        Instruction::SetPrivate(_) => {
            if height < 3 {
                Err(underflow(3))
            } else {
                Ok((height - 2, false))
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
        Instruction::JumpIfReturnSignal(_) => {
            if height < 1 {
                Err(underflow(1))
            } else {
                Ok((height, false))
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
        Instruction::Nop => Ok((height, false)),
    }
}
