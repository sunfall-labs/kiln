//! The tail of the instruction set: what the graph lowers through one correct body each, on the
//! small local operand stack its `Bridge` operation sets up. Every arm here is the whole of that
//! instruction's lowering. The graph emitter owns everything else; there is no other emitter.
//!
//! An arm reads its operands from `stack[]` below `sp` and leaves its results there, which is
//! the contract `bridge_effect` in `graph.rs` states for it. A throw goes to `unwind`, the
//! block's handler or the body's exit; a refusal is attributed to `pc` and routed through the
//! body's `kiln_refusal` label, which materialises the diagnostic while this translation unit's
//! private state is still visible.

use std::fmt::Write as _;

use kiln_ir::{Constant, Instruction};

use crate::{c_string, host_op_code, instruction_diagnostic, private_brand_key_of};

pub(crate) struct TailContext<'a> {
    pub(crate) constants: &'a [Constant],
    pub(crate) symbol: &'a str,
    pub(crate) diagnostic_site: &'a str,
    /// Whether a value reaching the embedding boundary could be a callable and so has to be
    /// converted first. False for a scalar unit, whose emitted bytes must not change.
    pub(crate) callables: bool,
}

pub(crate) fn emit_tail_instruction(
    output: &mut String,
    instruction: &Instruction,
    pc: u32,
    context: &TailContext<'_>,
    unwind: &str,
) {
    let TailContext {
        constants,
        symbol,
        diagnostic_site,
        callables,
    } = context;
    let index = pc as usize;
    let before = output.len();
    let instruction_site = c_string(diagnostic_site);
    let instruction_operation = c_string(&instruction_diagnostic(instruction, constants));
    let direct_refusal = format!(
        "kiln_c99_note_refusal({instruction_site}, (size_t){index}, {instruction_operation}, (size_t)kiln_c99_diagnostic_active_depth); return KILN_C99_CONTROL_ERROR;"
    );
    let control_refusal = format!(
        "if (status == KILN_C99_CONTROL_ERROR || status == KILN_C99_THROWN) {{ kiln_c99_note_refusal({instruction_site}, (size_t){index}, {instruction_operation}, (size_t)kiln_c99_diagnostic_active_depth);"
    );
    match instruction {
        Instruction::NewRegExp { pattern, flags } => {
            let (Constant::String(source), Constant::String(letters)) =
                (&constants[*pattern as usize], &constants[*flags as usize])
            else {
                unreachable!("verification rejects a non-string pattern before emission")
            };
            // A fresh object every time the literal is evaluated, because `lastIndex` is mutable
            // and program-visible: two evaluations of one literal must not share a cursor.
            writeln!(
                output,
                "    {{ kiln_value built; int status = kiln_c99_new_regexp(&built, {symbol}_constant_{pattern}, {}, {symbol}_constant_{flags}, {}, thrown); if (status == KILN_C99_THROWN) {unwind} if (status != KILN_C99_OK) return status; stack[sp++] = kiln_val_from_boundary(built); }}",
                source.len(),
                letters.len()
            )
            .unwrap();
        }
        Instruction::CallComputed { argc } => {
            // Not a `GetComputed` followed by a `Call`: that would discard the receiver, and
            // `obj[name]()` has to run the method against `obj`. The same reason `CallMethod`
            // is one instruction rather than two.
            let argc = usize::from(*argc);
            writeln!(
                output,
                "    {{ kiln_value method; kiln_value call_result; int status; kiln_value key = kiln_val_to_boundary(stack[sp - {argc} - 1]); kiln_value self_value = kiln_val_to_boundary(stack[sp - {argc} - 2]); kiln_value call_argbuf[({argc}) + 1]; for (size_t i = 0; i < (size_t){argc}; i++) {{ call_argbuf[i] = kiln_val_to_boundary(stack[sp - {argc} + i]); }} const kiln_value *call_args = {argc} == 0 ? NULL : call_argbuf; status = kiln_c99_get_computed(host, self_value, key, &method, depth, thrown); if (status == KILN_C99_THROWN) {unwind} if (status != KILN_C99_OK) return status; status = kiln_c99_call(host, &call_result, method, self_value, call_args, (size_t){argc}, depth, thrown); if (status == KILN_C99_THROWN) {unwind} if (status != KILN_C99_OK) return status; sp -= (size_t){}; stack[sp++] = kiln_val_from_boundary(call_result); }}",
                argc + 2
            )
            .unwrap();
        }
        Instruction::ConstructSpread => {
            // `new f(...args)` is the same construction the fixed-arity `New` performs, so it
            // goes through the same path rather than repeating the allocate-run-override
            // sequence.
            writeln!(
                output,
                "    {{ kiln_value list = kiln_val_to_boundary(stack[--sp]); kiln_val callee = stack[--sp]; kiln_val built = KILN_VAL_UNDEFINED; kiln_array *given; int status; if (list.tag != KILN_VALUE_ARRAY) {{ {direct_refusal} }} given = kiln_c99_array_of(list); status = kiln_c99_construct_val(host, &built, callee, given->count == 0 ? NULL : given->elements, given->count, depth, thrown); if (status == KILN_C99_THROWN) {unwind} if (status != KILN_C99_OK) return status; stack[sp++] = built; }}"
            )
            .unwrap();
        }
        Instruction::GetPrivate(name) => {
            let Constant::String(text) = &constants[*name as usize] else {
                unreachable!("verification rejects a non-string private name before emission")
            };
            writeln!(
                output,
                "    {{ kiln_value brand = kiln_val_to_boundary(stack[--sp]); kiln_value receiver = kiln_val_to_boundary(stack[sp - 1]); if (brand.tag != KILN_VALUE_UNDEFINED) {{ kiln_property *brand_slot = kiln_c99_own_private(receiver, \"{brand_key}\", {brand_len}); if (brand_slot == NULL || brand_slot->accessor || !kiln_c99_strict_equal(brand_slot->value, brand)) {{ int status = kiln_c99_private_error(thrown, {symbol}_constant_{name}, {len}, 0); {unwind} }} }} kiln_property *slot = kiln_c99_own_private(receiver, {symbol}_constant_{name}, {len}); if (slot == NULL) {{ int status = kiln_c99_private_error(thrown, {symbol}_constant_{name}, {len}, 0); {unwind} }} if (slot->accessor) {{ kiln_value getter = slot->getter; kiln_value produced; int status; if (getter.tag == KILN_VALUE_UNDEFINED) {{ status = kiln_c99_private_error(thrown, {symbol}_constant_{name}, {len}, 2); {unwind} }} status = kiln_c99_call(host, &produced, getter, receiver, NULL, 0, depth, thrown); if (status == KILN_C99_THROWN) {unwind} if (status != KILN_C99_OK) return status; stack[sp - 1] = kiln_val_from_boundary(produced); }} else {{ stack[sp - 1] = kiln_val_from_boundary(slot->value); }} }}",
                len = text.len(),
                brand_key = private_brand_key_of(text),
                brand_len = private_brand_key_of(text).len()
            )
            .unwrap();
        }
        Instruction::SetPrivate(name) => {
            let Constant::String(text) = &constants[*name as usize] else {
                unreachable!("verification rejects a non-string private name before emission")
            };
            writeln!(
                output,
                "    {{ kiln_value brand = kiln_val_to_boundary(stack[--sp]); kiln_value member = kiln_val_to_boundary(stack[--sp]); kiln_value receiver = kiln_val_to_boundary(stack[sp - 1]); if (brand.tag != KILN_VALUE_UNDEFINED) {{ kiln_property *brand_slot = kiln_c99_own_private(receiver, \"{brand_key}\", {brand_len}); if (brand_slot == NULL || brand_slot->accessor || !kiln_c99_strict_equal(brand_slot->value, brand)) {{ int status = kiln_c99_private_error(thrown, {symbol}_constant_{name}, {len}, 1); {unwind} }} }} kiln_property *slot = kiln_c99_own_private(receiver, {symbol}_constant_{name}, {len}); if (slot == NULL) {{ int status = kiln_c99_private_error(thrown, {symbol}_constant_{name}, {len}, 1); {unwind} }} if (slot->accessor) {{ kiln_value setter = slot->setter; kiln_value produced; int status; if (setter.tag == KILN_VALUE_UNDEFINED) {{ status = kiln_c99_private_error(thrown, {symbol}_constant_{name}, {len}, 3); {unwind} }} status = kiln_c99_call(host, &produced, setter, receiver, &member, 1, depth, thrown); if (status == KILN_C99_THROWN) {unwind} if (status != KILN_C99_OK) return status; }} else if (!slot->writable) {{ int status = kiln_c99_private_error(thrown, {symbol}_constant_{name}, {len}, 4); {unwind} }} else {{ slot->value = member; }} stack[sp - 1] = kiln_val_from_boundary(member); }}",
                len = text.len(),
                brand_key = private_brand_key_of(text),
                brand_len = private_brand_key_of(text).len()
            )
            .unwrap();
        }
        Instruction::ThrowReference(name) => {
            // A name that is not there. Emitted rather than refused, because a reference in a
            // branch that never runs is not an error in JavaScript and real bundles are full of
            // them -- feature detection mentions globals the host may not have.
            //
            // "Not there" is decided at *run* time, though, not here. The compiler reaches this
            // instruction for every name outside `kiln_ir::Builtin`, and the realm's global object
            // holds intrinsics that table does not name -- `Temporal` is one. The interpreter has
            // always looked there before throwing (see its `ThrowReference` arm); this backend did
            // not, which is why a fully implemented `Temporal` was still a ReferenceError under
            // `--native`. The lookup is a no-op for every name the table *does* carry, because
            // those never reach this instruction at all.
            let Constant::String(text) = &constants[*name as usize] else {
                unreachable!("verification rejects a non-string reference name before emission")
            };
            writeln!(
                output,
                "    {{ static const char kiln_missing_name_{index}[] = {}; static const char kiln_missing_{index}[] = {}; int found = 0; kiln_value val; int status = kiln_c99_global_binding(&val, kiln_missing_name_{index}, {}, &found); if (status != KILN_C99_OK) return status; if (found) {{ stack[sp++] = kiln_val_from_boundary(val); }} else {{ status = kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_REFERENCE_ERROR, kiln_missing_{index}); {} }} }}",
                c_string(text),
                c_string(&format!("{text} is not defined")),
                text.len(),
                unwind
            )
            .unwrap();
        }
        Instruction::DeclareGlobalBinding { name, configurable } => {
            let Constant::String(text) = &constants[*name as usize] else {
                unreachable!("verification rejects a non-string binding name before emission")
            };
            writeln!(
                output,
                "    {{ int status = kiln_c99_declare_global({symbol}_constant_{name}, {}, {}); if (status != KILN_C99_OK) return status; }}",
                text.len(),
                i32::from(*configurable)
            )
            .unwrap();
        }
        Instruction::DeleteGlobalBinding(name) => {
            // The mirror of `StoreGlobalBinding`: an unqualified `delete` in sloppy code is the
            // global object environment record's `DeleteBinding`, which is that object's
            // `[[Delete]]`. Both engines have to answer it the same way, because
            // `x = 1; delete x; x` throws in one and reads `1` in the other otherwise.
            let Constant::String(text) = &constants[*name as usize] else {
                unreachable!("verification rejects a non-string binding name before emission")
            };
            writeln!(
                output,
                "    {{ kiln_value globals; int status = kiln_c99_global_object(&globals); if (status != KILN_C99_OK) return status; int answer; status = kiln_c99_delete(host, globals, {symbol}_constant_{name}, {}, &answer, depth, thrown); if (status == KILN_C99_THROWN) {unwind} if (status != KILN_C99_OK) return status; stack[sp++] = answer ? KILN_VAL_TRUE : KILN_VAL_FALSE; }}",
                text.len()
            )
            .unwrap();
        }
        Instruction::LoadGlobalBindingOrUndefined(name) => {
            let Constant::String(text) = &constants[*name as usize] else {
                unreachable!("verification rejects a non-string binding name before emission")
            };
            writeln!(
                output,
                "    {{ kiln_value existing; int found = 0; int status = kiln_c99_global_binding(&existing, {symbol}_constant_{name}, {}, &found); if (status != KILN_C99_OK) return status; stack[sp++] = found ? kiln_val_from_boundary(existing) : KILN_VAL_UNDEFINED; }}",
                text.len()
            )
            .unwrap();
        }
        Instruction::RequireGlobalBinding(name) => {
            let Constant::String(text) = &constants[*name as usize] else {
                unreachable!("verification rejects a non-string binding name before emission")
            };
            writeln!(
                output,
                "    {{ kiln_value existing; int found = 0; int status = kiln_c99_global_binding(&existing, {symbol}_constant_{name}, {}, &found); if (status != KILN_C99_OK) return status; if (!found) {{ static const char kiln_unbound_{index}[] = {}; status = kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_REFERENCE_ERROR, kiln_unbound_{index}); {unwind} }} }}",
                text.len(),
                c_string(&format!("{text} is not defined"))
            )
            .unwrap();
        }
        Instruction::StoreGlobalBindingStrict(name) => {
            // The strict form: an existing global is written, and a name the realm does not have
            // is the unresolvable reference strict mode refuses. Both engines must decide it the
            // same way and at the same moment, which is run time.
            let Constant::String(text) = &constants[*name as usize] else {
                unreachable!("verification rejects a non-string binding name before emission")
            };
            writeln!(
                output,
                "    {{ kiln_value member = kiln_val_to_boundary(stack[--sp]); kiln_value existing; int found = 0; int status = kiln_c99_global_binding(&existing, {symbol}_constant_{name}, {len}, &found); if (status != KILN_C99_OK) return status; if (!found) {{ static const char kiln_unbound_{index}[] = {}; status = kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_REFERENCE_ERROR, kiln_unbound_{index}); {unwind} }} {{ kiln_value globals; status = kiln_c99_global_object(&globals); if (status != KILN_C99_OK) return status; status = kiln_c99_put_mode(host, globals, {symbol}_constant_{name}, {len}, member, 1, depth, thrown); if (status == KILN_C99_THROWN) {unwind} if (status != KILN_C99_OK) return status; }} }}",
                c_string(&format!("{text} is not defined")),
                len = text.len()
            )
            .unwrap();
        }
        Instruction::StoreGlobalBinding(name) => {
            // Sloppy code settles an unresolvable assignment against the global object, which
            // creates the property. The interpreter does the same thing through its own global
            // object; both engines have to, or a UMD header installs itself under one and throws
            // under the other -- a divergence worse than the shared refusal it replaced.
            let Constant::String(text) = &constants[*name as usize] else {
                unreachable!("verification rejects a non-string binding name before emission")
            };
            writeln!(
                output,
                "    {{ kiln_value member = kiln_val_to_boundary(stack[--sp]); kiln_value globals; int status = kiln_c99_global_object(&globals); if (status != KILN_C99_OK) return status; status = kiln_c99_put_mode(host, globals, {symbol}_constant_{name}, {}, member, 0, depth, thrown); if (status == KILN_C99_THROWN) {unwind} if (status != KILN_C99_OK) return status; }}",
                text.len()
            )
            .unwrap();
        }
        Instruction::ToPropertyKey => {
            writeln!(
                output,
                "    {{ int status = kiln_c99_val_to_property_key(host, stack[sp - 2], stack[sp - 1], depth, thrown, &stack[sp - 1]); if (status == KILN_C99_THROWN) {unwind} if (status != KILN_C99_OK) return status; }}"
            )
            .unwrap();
        }
        // Both halves of `super` are an ordinary call with `this` threaded through: the parent
        // initialises the instance that already exists rather than building one, which is the
        // whole difference between `super()` and `new Parent()`.
        Instruction::SuperCall { argc } => {
            let argc = usize::from(*argc);
            writeln!(
                output,
                "    {{ kiln_val produced = KILN_VAL_UNDEFINED; const kiln_val *call_args = {argc} == 0 ? NULL : &stack[sp - {argc}]; int status = kiln_c99_super_call_val(host, &produced, stack[sp - {argc} - 1], receiver, call_args, (size_t){argc}, depth, thrown); if (status == KILN_C99_THROWN) {unwind} if (status != KILN_C99_OK) return status; status = kiln_c99_adopt_super_val(&receiver, produced); if (status != KILN_C99_OK) return status; sp -= (size_t){}; stack[sp++] = produced; }}",
                argc + 1
            )
            .unwrap();
        }
        // A derived class with no constructor behaves as `constructor(...args) { super(...args) }`.
        // Its arity is not known where it is synthesized, so the forwarding happens here from
        // whatever the frame was actually given.
        Instruction::SuperForward => {
            writeln!(
                output,
                "    {{ kiln_val produced = KILN_VAL_UNDEFINED; int status = kiln_c99_super_call_val(host, &produced, stack[sp - 1], receiver, argc == 0 ? NULL : args, argc, depth, thrown); if (status == KILN_C99_THROWN) {unwind} if (status != KILN_C99_OK) return status; status = kiln_c99_adopt_super_val(&receiver, produced); if (status != KILN_C99_OK) return status; stack[sp - 1] = produced; }}"
            )
            .unwrap();
        }
        Instruction::DefineAccessor { setter, enumerable } => {
            let half = if *setter { "setter" } else { "getter" };
            let other = if *setter { "getter" } else { "setter" };
            let prefix = if *setter { "set" } else { "get" };
            let enumerable = i32::from(*enumerable);
            writeln!(
                output,
                "    {{ kiln_value {half} = kiln_val_to_boundary(stack[--sp]); kiln_value key = kiln_val_to_boundary(stack[--sp]); kiln_value {other} = kiln_c99_undefined(); kiln_value name = kiln_c99_property_key(key); int status; if (name.tag != KILN_VALUE_STRING) {{ {direct_refusal} }} status = kiln_c99_set_function_name({half}, key, \"{prefix}\"); if (status != KILN_C99_OK) return status; status = kiln_c99_define_accessor(kiln_val_to_boundary(stack[sp - 1]), name.ptr, name.len, getter, setter, {enumerable}); if (status != KILN_C99_OK) return status; }}"
            )
            .unwrap();
        }
        // A private element is installed with its kind rather than as a slot holding a function,
        // and on the *object* rather than on a prototype: the presence of the name is the brand,
        // so an instance element belongs to each instance.
        Instruction::DefinePrivate { name, kind } => {
            let Constant::String(text) = &constants[*name as usize] else {
                unreachable!("verification rejects a non-string private name before emission")
            };
            let (tag, prefix) = match kind {
                kiln_ir::PrivateKind::Method => (0, "NULL"),
                kiln_ir::PrivateKind::Getter => (1, "\"get\""),
                kiln_ir::PrivateKind::Setter => (2, "\"set\""),
            };
            writeln!(
                output,
                "    {{ kiln_value function = kiln_val_to_boundary(stack[--sp]); kiln_value key = kiln_c99_value(KILN_VALUE_STRING, 0, {symbol}_constant_{name}, {len}); if (kiln_c99_set_function_name(function, key, {prefix}) != KILN_C99_OK) {{ {direct_refusal} }} if (kiln_c99_define_private(kiln_val_to_boundary(stack[sp - 1]), {symbol}_constant_{name}, {len}, function, {tag}) != KILN_C99_OK) {{ {direct_refusal} }} }}",
                len = text.len()
            )
            .unwrap();
        }
        // A class's methods are not enumerated, which is the whole difference from the
        // `SetProperty` an object literal's methods lower to.
        // The enumerated sibling of `DefineMethod`: an object literal's members are listed,
        // a class's methods are not.
        Instruction::DefineNamedProperty => {
            writeln!(
                output,
                "    {{ kiln_value method = kiln_val_to_boundary(stack[--sp]); kiln_value key = kiln_val_to_boundary(stack[--sp]); kiln_value name = kiln_c99_property_key(key); if (name.tag != KILN_VALUE_STRING) {{ {direct_refusal} }} if (kiln_c99_set_function_name(method, key, NULL) != KILN_C99_OK) {{ {direct_refusal} }} if (kiln_c99_put(host, kiln_val_to_boundary(stack[sp - 1]), name.ptr, name.len, method, depth, thrown) != KILN_C99_OK) {{ {direct_refusal} }} }}"
            )
            .unwrap();
        }
        Instruction::DefineMethod => {
            writeln!(
                output,
                "    {{ kiln_value method = kiln_val_to_boundary(stack[--sp]); kiln_value key = kiln_val_to_boundary(stack[--sp]); kiln_value name = kiln_c99_property_key(key); if (name.tag != KILN_VALUE_STRING) {{ {direct_refusal} }} if (kiln_c99_set_function_name(method, key, NULL) != KILN_C99_OK) {{ {direct_refusal} }} if (kiln_c99_define_method(kiln_val_to_boundary(stack[sp - 1]), name.ptr, name.len, method) != KILN_C99_OK) {{ {direct_refusal} }} }}"
            )
            .unwrap();
        }
        Instruction::DeleteProperty(name) => {
            let Constant::String(text) = &constants[*name as usize] else {
                unreachable!("verification rejects a non-string property name before emission")
            };
            writeln!(
                output,
                "    {{ int answer; int status = kiln_c99_delete(host, kiln_val_to_boundary(stack[sp - 1]), {symbol}_key_{name}, {}, &answer, depth, thrown); if (status == KILN_C99_THROWN) {unwind} if (status != KILN_C99_OK) return status; stack[sp - 1] = answer ? KILN_VAL_TRUE : KILN_VAL_FALSE; }}",
                text.len()
            )
            .unwrap();
        }
        Instruction::DeletePropertyStrict { name, strict } => {
            let Constant::String(text) = &constants[*name as usize] else {
                unreachable!("verification rejects a non-string property name before emission")
            };
            writeln!(
                output,
                "    {{ int answer; int status = kiln_c99_delete_or_throw(host, kiln_val_to_boundary(stack[sp - 1]), {symbol}_key_{name}, {}, {}, &answer, depth, thrown); if (status == KILN_C99_THROWN) {unwind} if (status != KILN_C99_OK) return status; stack[sp - 1] = answer ? KILN_VAL_TRUE : KILN_VAL_FALSE; }}",
                text.len(),
                i32::from(*strict)
            )
            .unwrap();
        }
        Instruction::DeleteComputed => {
            writeln!(
                output,
                "    {{ kiln_value key = kiln_val_to_boundary(stack[--sp]); int answer; int status = kiln_c99_delete_computed(host, kiln_val_to_boundary(stack[sp - 1]), key, &answer, depth, thrown); if (status == KILN_C99_THROWN) {unwind} if (status != KILN_C99_OK) return status; stack[sp - 1] = answer ? KILN_VAL_TRUE : KILN_VAL_FALSE; }}"
            )
            .unwrap();
        }
        Instruction::DeleteComputedStrict { strict } => {
            writeln!(
                output,
                "    {{ kiln_value key = kiln_val_to_boundary(stack[--sp]); int answer; int status = kiln_c99_delete_computed_or_throw(host, kiln_val_to_boundary(stack[sp - 1]), key, {}, &answer, depth, thrown); if (status == KILN_C99_THROWN) {unwind} if (status != KILN_C99_OK) return status; stack[sp - 1] = answer ? KILN_VAL_TRUE : KILN_VAL_FALSE; }}",
                i32::from(*strict)
            )
            .unwrap();
        }
        // `arguments` is every argument this frame was given, as an array. A real one is an
        // exotic object that aliases the parameters; a copy is what almost every use of it wants,
        // and the aliasing was removed from the language in strict mode anyway.
        // Not `args` and `argc`: inside an arrow those are the arrow's own, and `arguments`
        // there is the enclosing function's -- a different list entirely.
        Instruction::LoadArguments => {
            output.push_str(
                "    { kiln_value given; int status = kiln_c99_visible_arguments(&given); if (status != KILN_C99_OK) return status; stack[sp++] = kiln_val_from_boundary(given); }\n",
            );
        }
        // `...rest` is every argument past the named parameters, as an array. A generator's
        // arguments were copied into its frame when it was built, so there are none here.
        Instruction::RestArguments(from) => {
            writeln!(
                output,
                "    {{ kiln_value rest; size_t extra; int status = kiln_c99_new_array(&rest); if (status != KILN_C99_OK) return status; for (extra = (size_t){from}; extra < argc; ++extra) {{ status = kiln_c99_array_push(rest, kiln_val_to_boundary(args[extra])); if (status != KILN_C99_OK) return status; }} stack[sp++] = kiln_val_from_boundary(rest); }}"
            )
            .unwrap();
        }
        // The marker for an element a literal left out. It never escapes into an expression:
        // `NewArray` is the only thing that consumes it.
        Instruction::PushHole => output.push_str("    stack[sp++] = KILN_VAL_HOLE;\n"),
        Instruction::EnumerateKeys => {
            writeln!(
                output,
                "    {{ kiln_value keys; int status = kiln_c99_enumerate_keys(host, kiln_val_to_boundary(stack[sp - 1]), &keys, depth, thrown); if (status == KILN_C99_THROWN) {unwind} if (status != KILN_C99_OK) return status; stack[sp - 1] = kiln_val_from_boundary(keys); }}"
            )
            .unwrap();
        }
        // The arity comes from an array the program built, so the arguments are read out of it
        // rather than off the stack.
        Instruction::Apply => {
            writeln!(
                output,
                "    {{ kiln_value list = kiln_val_to_boundary(stack[--sp]); kiln_val callee = stack[--sp]; kiln_val self_value = stack[sp - 1]; kiln_val call_result = KILN_VAL_UNDEFINED; kiln_array *given; int status; if (list.tag != KILN_VALUE_ARRAY) {{ {direct_refusal} }} given = kiln_c99_array_of(list); status = kiln_c99_call_val(host, &call_result, callee, self_value, given->count == 0 ? NULL : given->elements, given->count, depth, thrown); if (status == KILN_C99_THROWN) {{ {unwind} }} if (status != KILN_C99_OK) return status; stack[sp - 1] = call_result; }}"
            )
            .unwrap();
        }
        // `Apply`'s runtime argument list, but against the instance being built and keeping the
        // `new.target` of the call that is building it -- the two things `super()` adds.
        Instruction::SuperApply => {
            writeln!(
                output,
                "    {{ kiln_value list = kiln_val_to_boundary(stack[--sp]); kiln_val parent = stack[--sp]; kiln_val call_result = KILN_VAL_UNDEFINED; kiln_array *given; int status; if (list.tag != KILN_VALUE_ARRAY) {{ {direct_refusal} }} given = kiln_c99_array_of(list); status = kiln_c99_super_call_val(host, &call_result, parent, receiver, given->count == 0 ? NULL : given->elements, given->count, depth, thrown); if (status == KILN_C99_THROWN) {{ {unwind} }} if (status != KILN_C99_OK) return status; status = kiln_c99_adopt_super_val(&receiver, call_result); if (status != KILN_C99_OK) return status; stack[sp++] = call_result; }}"
            )
            .unwrap();
        }
        Instruction::SpreadInto => {
            writeln!(
                output,
                "    {{ kiln_value source = kiln_val_to_boundary(stack[--sp]); int status = kiln_c99_iterate(host, kiln_val_to_boundary(stack[sp - 1]), source, depth, thrown); if (status == KILN_C99_THROWN) {unwind} if (status != KILN_C99_OK) return status; }}"
            )
            .unwrap();
        }
        Instruction::IterableToArray => {
            writeln!(
                output,
                "    {{ kiln_value built; int status = kiln_c99_new_array(&built); if (status != KILN_C99_OK) return status; status = kiln_c99_iterate(host, built, kiln_val_to_boundary(stack[sp - 1]), depth, thrown); if (status == KILN_C99_THROWN) {unwind} if (status != KILN_C99_OK) return status; stack[sp - 1] = kiln_val_from_boundary(built); }}"
            )
            .unwrap();
        }
        Instruction::SpreadObject => {
            writeln!(
                output,
                "    {{ kiln_value source = kiln_val_to_boundary(stack[--sp]); int status = kiln_c99_spread_object(host, kiln_val_to_boundary(stack[sp - 1]), source, depth, thrown); if (status == KILN_C99_THROWN) {unwind} if (status != KILN_C99_OK) return status; }}"
            )
            .unwrap();
        }
        // `class D extends C` links the two prototype objects, which is what makes an instance
        // of `D` find a method of `C`.
        Instruction::SetPrototype => {
            output.push_str(
                "    { kiln_value prototype = kiln_val_to_boundary(stack[--sp]); int status = kiln_c99_set_prototype(kiln_val_to_boundary(stack[sp - 1]), prototype); if (status != KILN_C99_OK) return status; }\n",
            );
        }
        // `super.x` reads the link the method's home object carries *now*, which is the same
        // question `Object.getPrototypeOf` answers -- so it is asked through the same
        // implementation rather than a second one that could drift from it.
        // The receiver is the frame's own, not the prototype the name was found on -- which is
        // the one thing that separates `super.x` from an ordinary read of the same name.
        Instruction::LoadSuperProperty(name) => {
            let Constant::String(text) = &constants[*name as usize] else {
                unreachable!("verification rejects a non-string property name before emission")
            };
            writeln!(
                output,
                "    {{ kiln_value read; int status = kiln_c99_get_with_receiver(host, kiln_val_to_boundary(stack[sp - 1]), kiln_c99_value(KILN_VALUE_STRING, 0, {symbol}_key_{name}, {}), kiln_val_to_boundary(receiver), &read, depth, thrown); if (status == KILN_C99_THROWN) {unwind} if (status != KILN_C99_OK) return status; stack[sp - 1] = kiln_val_from_boundary(read); }}",
                text.len()
            )
            .unwrap();
        }
        // The computed spelling. The key was pushed first, so it is the value the read leaves
        // its answer in place of.
        Instruction::LoadSuperElement => {
            writeln!(
                output,
                "    {{ kiln_value prototype = kiln_val_to_boundary(stack[--sp]); kiln_value read; int status = kiln_c99_get_with_receiver(host, prototype, kiln_val_to_boundary(stack[sp - 1]), kiln_val_to_boundary(receiver), &read, depth, thrown); if (status == KILN_C99_THROWN) {unwind} if (status != KILN_C99_OK) return status; stack[sp - 1] = kiln_val_from_boundary(read); }}"
            )
            .unwrap();
        }
        Instruction::LoadPrototype => {
            writeln!(
                output,
                "    {{ kiln_value subject = kiln_val_to_boundary(stack[sp - 1]); kiln_value proto_out; int status = kiln_c99_invoke_method(host, &proto_out, KILN_C99_METHOD_OBJECT_GET_PROTOTYPE_OF, kiln_c99_undefined(), &subject, 1, depth, thrown); if (status == KILN_C99_THROWN) {unwind} if (status != KILN_C99_OK) return status; stack[sp - 1] = kiln_val_from_boundary(proto_out); }}"
            )
            .unwrap();
        }
        Instruction::HostCall { op, argc } => {
            // The embedding boundary carries only the five scalar tags, so a function argument
            // is reduced to its text before the host sees it -- which is also what the
            // interpreter prints for one.
            // The arguments are copied into a fresh buffer rather than passed as `&stack[sp - n]`.
            // Pointing into `stack[]` and handing that pointer to the host makes the stack array
            // address-escaped for the entire function, which defeats the promotion the numeric
            // fast paths exist to enable: a single `console.log` at the end of a program was
            // enough to keep a fifty-million-iteration loop's values in memory. The buffer is a
            // local whose address escaping costs nothing, because nothing else reads it.
            if *argc == 0 {
                writeln!(output, "    {{ kiln_value host_result; if (host == NULL || host->call == NULL || host->call(host->context, UINT16_C({}), UINT16_C(0), NULL, &host_result) != 0) return KILN_C99_HOST_ERROR; stack[sp++] = kiln_val_from_boundary(host_result); }}", host_op_code(*op)).unwrap()
            } else if *callables {
                writeln!(output, "    {{ kiln_value host_result; kiln_value host_args[{argc}]; for (size_t i = 0; i < (size_t){argc}; i++) {{ kiln_value arg_val = kiln_val_to_boundary(stack[sp - {argc} + i]); if (kiln_c99_crosses_boundary(arg_val)) arg_val = kiln_c99_inspect(arg_val); host_args[i] = arg_val; }} if (host == NULL || host->call == NULL || host->call(host->context, UINT16_C({}), UINT16_C({argc}), host_args, &host_result) != 0) return KILN_C99_HOST_ERROR; sp -= {argc}; stack[sp++] = kiln_val_from_boundary(host_result); }}", host_op_code(*op)).unwrap()
            } else {
                writeln!(output, "    {{ kiln_value host_result; kiln_value host_args[{argc}]; for (size_t i = 0; i < (size_t){argc}; i++) {{ host_args[i] = kiln_val_to_boundary(stack[sp - {argc} + i]); }} if (host == NULL || host->call == NULL || host->call(host->context, UINT16_C({}), UINT16_C({argc}), host_args, &host_result) != 0) return KILN_C99_HOST_ERROR; sp -= {argc}; stack[sp++] = kiln_val_from_boundary(host_result); }}", host_op_code(*op)).unwrap()
            }
        }
        other => unreachable!("the graph lowers {other:?} itself; only the tail comes here"),
    }
    // A refusal from a helper is noted against this instruction and routed through the body's
    // refusal label rather than returned directly, which would lose this unit's private state
    // when an imported function crosses back to its caller.
    let refusal = format!(
        "{control_refusal} goto kiln_refusal; }} if (status != KILN_C99_OK) return status;"
    );
    let routed = output[before..].replace("if (status != KILN_C99_OK) return status;", &refusal);
    output.truncate(before);
    output.push_str(&routed);
}
