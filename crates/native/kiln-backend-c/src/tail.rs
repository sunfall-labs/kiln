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

use kiln_ir::{Constant, EvalAccess, Instruction};

use crate::{
    c_string, diagnostic_source, host_argument_inspection, host_op_code, instruction_diagnostic,
    private_brand_key_of,
};

pub(crate) struct TailContext<'a> {
    pub(crate) constants: &'a [Constant],
    pub(crate) symbol: &'a str,
    pub(crate) diagnostic_site: &'a str,
    /// Whether a value reaching the embedding boundary could be a callable and so has to be
    /// converted first. False for a scalar unit, whose emitted bytes must not change.
    pub(crate) callables: bool,
    /// Prefer the provider-backed matcher over emitting one C function per regex state.
    pub(crate) compact: bool,
    /// Whether to compile static regex literals into direct C matchers.
    pub(crate) compile_regex: bool,
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
        compact,
        compile_regex,
    } = context;
    let index = pc as usize;
    let before = output.len();
    let instruction_site = diagnostic_source(diagnostic_site);
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
            // The constants are pointers to headed strings now, so no byte lengths travel.
            if *compile_regex
                && let (Ok(source), Ok(letters)) =
                    (source.to_scalar_string(), letters.to_scalar_string())
                && crate::regex::try_compile_regex(&source, &letters).is_some()
            {
                writeln!(
                    output,
                    "    {{ kiln_value built; int status = kiln_c99_new_regexp_matcher(&built, {symbol}_constant_{pattern}, {symbol}_constant_{flags}, {symbol}_re_{pattern}_{flags}, {symbol}_re_{pattern}_{flags}_ascii, thrown); if (status == KILN_C99_THROWN) {unwind} if (status != KILN_C99_OK) return status; stack[sp++] = kiln_val_from_boundary(built); }}"
                )
                .unwrap();
            } else {
                writeln!(
                    output,
                    "    {{ kiln_value built; int status = kiln_c99_new_regexp(&built, {symbol}_constant_{pattern}, {symbol}_constant_{flags}, thrown); if (status == KILN_C99_THROWN) {unwind} if (status != KILN_C99_OK) return status; stack[sp++] = kiln_val_from_boundary(built); }}"
                )
                .unwrap();
            }
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
            let member_key = text.to_scalar_string().unwrap_or_default();
            let member_len = member_key.len();
            let brand_key = private_brand_key_of(&member_key);
            let brand_len = brand_key.len();
            writeln!(
                output,
                "    {{ static kiln_c99_private_site site_{pc} = {{0}}; \
                 kiln_val brand_v = stack[--sp]; \
                 kiln_val receiver_v = stack[sp - 1]; \
                 if (kiln_c99_val_get_private_cached(&stack[sp - 1], receiver_v, brand_v, &site_{pc}) || \
                     kiln_c99_val_get_private_fast(&stack[sp - 1], receiver_v, brand_v, \"{brand_key}\", {brand_len}, \"{member_key}\", {member_len}, &site_{pc})) {{ \
                 }} else {{ \
                     kiln_value brand = kiln_val_to_boundary(brand_v); \
                     kiln_value receiver = kiln_val_to_boundary(receiver_v); \
                     if (brand.tag != KILN_VALUE_UNDEFINED) {{ \
                         kiln_property *brand_slot = kiln_c99_own_private(receiver, \"{brand_key}\", {brand_len}); \
                         if (brand_slot == NULL || brand_slot->accessor || !kiln_c99_strict_equal(brand_slot->value, brand)) {{ \
                             int status = kiln_c99_private_error_key(thrown, {symbol}_constant_{name}, 0); \
                             {unwind} \
                         }} \
                     }} \
                     kiln_string private_key = *{symbol}_constant_{name}; \
                     private_key.flags |= KILN_STRING_PRIVATE_KEY; \
                     kiln_property *slot = kiln_c99_own_private_key(receiver, &private_key); \
                     if (slot == NULL) {{ \
                         int status = kiln_c99_private_error_key(thrown, {symbol}_constant_{name}, 0); \
                         {unwind} \
                     }} \
                     if (slot->accessor) {{ \
                         kiln_value getter = slot->getter; \
                         kiln_value produced; \
                         int status; \
                         if (getter.tag == KILN_VALUE_UNDEFINED) {{ \
                             status = kiln_c99_private_error_key(thrown, {symbol}_constant_{name}, 2); \
                             {unwind} \
                         }} \
                         status = kiln_c99_call(host, &produced, getter, receiver, NULL, 0, depth, thrown); \
                         if (status == KILN_C99_THROWN) {unwind} \
                         if (status != KILN_C99_OK) return status; \
                         stack[sp - 1] = kiln_val_from_boundary(produced); \
                     }} else {{ \
                         stack[sp - 1] = kiln_val_from_boundary(slot->value); \
                     }} \
                 }} }}"
            )
            .unwrap();
        }
        Instruction::SetPrivate(name) => {
            let Constant::String(text) = &constants[*name as usize] else {
                unreachable!("verification rejects a non-string private name before emission")
            };
            let member_key = text.to_scalar_string().unwrap_or_default();
            let member_len = member_key.len();
            let brand_key = private_brand_key_of(&member_key);
            let brand_len = brand_key.len();
            writeln!(
                output,
                "    {{ static kiln_c99_private_site site_{pc} = {{0}}; \
                 kiln_val brand_v = stack[--sp]; \
                 kiln_val member_v = stack[--sp]; \
                 kiln_val receiver_v = stack[sp - 1]; \
                 if (kiln_c99_val_set_private_cached(receiver_v, brand_v, member_v, &site_{pc}) || \
                     kiln_c99_val_set_private_fast(receiver_v, brand_v, member_v, \"{brand_key}\", {brand_len}, \"{member_key}\", {member_len}, &site_{pc})) {{ \
                     stack[sp - 1] = member_v; \
                 }} else {{ \
                     kiln_value brand = kiln_val_to_boundary(brand_v); \
                     kiln_value member = kiln_val_to_boundary(member_v); \
                     kiln_value receiver = kiln_val_to_boundary(receiver_v); \
                     if (brand.tag != KILN_VALUE_UNDEFINED) {{ \
                         kiln_property *brand_slot = kiln_c99_own_private(receiver, \"{brand_key}\", {brand_len}); \
                         if (brand_slot == NULL || brand_slot->accessor || !kiln_c99_strict_equal(brand_slot->value, brand)) {{ \
                             int status = kiln_c99_private_error_key(thrown, {symbol}_constant_{name}, 1); \
                             {unwind} \
                         }} \
                     }} \
                     kiln_string private_key = *{symbol}_constant_{name}; \
                     private_key.flags |= KILN_STRING_PRIVATE_KEY; \
                     kiln_property *slot = kiln_c99_own_private_key(receiver, &private_key); \
                     if (slot == NULL) {{ \
                         int status = kiln_c99_private_error_key(thrown, {symbol}_constant_{name}, 1); \
                         {unwind} \
                     }} \
                     if (slot->accessor) {{ \
                         kiln_value setter = slot->setter; \
                         kiln_value produced; \
                         int status; \
                         if (setter.tag == KILN_VALUE_UNDEFINED) {{ \
                             status = kiln_c99_private_error_key(thrown, {symbol}_constant_{name}, 3); \
                             {unwind} \
                         }} \
                         status = kiln_c99_call(host, &produced, setter, receiver, &member, 1, depth, thrown); \
                         if (status == KILN_C99_THROWN) {unwind} \
                         if (status != KILN_C99_OK) return status; \
                     }} else if (!slot->writable) {{ \
                         int status = kiln_c99_private_error_key(thrown, {symbol}_constant_{name}, 4); \
                         {unwind} \
                     }} else {{ \
                         slot->value = member; \
                     }} \
                     stack[sp - 1] = member_v; \
                 }} }}"
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
                "    {{ static const char kiln_missing_name_{index}[] = {}; static const char kiln_missing_{index}[] = {}; int found = 0; kiln_value val; int status = kiln_c99_global_binding(host, &val, kiln_missing_name_{index}, {}, &found, depth, thrown); if (status != KILN_C99_OK) return status; if (found) {{ stack[sp++] = kiln_val_from_boundary(val); }} else {{ status = kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_REFERENCE_ERROR, kiln_missing_{index}); {} }} }}",
                c_string(&text.to_scalar_string().unwrap_or_default()),
                c_string(&format!("{} is not defined", text.to_scalar_string().unwrap_or_default())),
                text.len(),
                unwind
            )
            .unwrap();
        }
        Instruction::DeclareGlobalBinding { name, configurable } => {
            let Constant::String(_) = &constants[*name as usize] else {
                unreachable!("verification rejects a non-string binding name before emission")
            };
            writeln!(
                output,
                "    {{ int status = kiln_c99_declare_global_key({symbol}_constant_{name}, {}); if (status != KILN_C99_OK) return status; }}",
                i32::from(*configurable)
            )
            .unwrap();
        }
        Instruction::DeleteGlobalBinding(name) => {
            // The mirror of `StoreGlobalBinding`: an unqualified `delete` in sloppy code is the
            // global object environment record's `DeleteBinding`, which is that object's
            // `[[Delete]]`. Both engines have to answer it the same way, because
            // `x = 1; delete x; x` throws in one and reads `1` in the other otherwise.
            let Constant::String(_) = &constants[*name as usize] else {
                unreachable!("verification rejects a non-string binding name before emission")
            };
            writeln!(
                output,
                "    {{ kiln_value globals; int status = kiln_c99_global_object(&globals); if (status != KILN_C99_OK) return status; int answer; status = kiln_c99_delete_key(host, globals, {symbol}_constant_{name}, NULL, &answer, depth, thrown); if (status == KILN_C99_THROWN) {unwind} if (status != KILN_C99_OK) return status; stack[sp++] = answer ? KILN_VAL_TRUE : KILN_VAL_FALSE; }}",
            )
            .unwrap();
        }
        Instruction::LoadGlobalBindingOrUndefined(name) => {
            let Constant::String(_) = &constants[*name as usize] else {
                unreachable!("verification rejects a non-string binding name before emission")
            };
            writeln!(
                output,
                "    {{ kiln_value existing; int found = 0; int status = kiln_c99_global_binding_key(host, &existing, {symbol}_constant_{name}, &found, depth, thrown); if (status != KILN_C99_OK) return status; stack[sp++] = found ? kiln_val_from_boundary(existing) : KILN_VAL_UNDEFINED; }}",
            )
            .unwrap();
        }
        Instruction::RequireGlobalBinding(name) => {
            let Constant::String(text) = &constants[*name as usize] else {
                unreachable!("verification rejects a non-string binding name before emission")
            };
            writeln!(
                output,
                "    {{ kiln_value existing; int found = 0; int status = kiln_c99_global_binding_key(host, &existing, {symbol}_constant_{name}, &found, depth, thrown); if (status != KILN_C99_OK) return status; if (!found) {{ static const char kiln_unbound_{index}[] = {}; status = kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_REFERENCE_ERROR, kiln_unbound_{index}); {unwind} }} }}",
                c_string(&format!("{} is not defined", text.to_scalar_string().unwrap_or_default()))
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
                "    {{ kiln_value member = kiln_val_to_boundary(stack[--sp]); kiln_value existing; int found = 0; int status = kiln_c99_global_binding_key(host, &existing, {symbol}_constant_{name}, &found, depth, thrown); if (status != KILN_C99_OK) return status; if (!found) {{ static const char kiln_unbound_{index}[] = {}; status = kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_REFERENCE_ERROR, kiln_unbound_{index}); {unwind} }} {{ kiln_value globals; status = kiln_c99_global_object(&globals); if (status != KILN_C99_OK) return status; status = kiln_c99_put_mode_key(host, globals, {symbol}_constant_{name}, NULL, member, 1, depth, thrown); if (status == KILN_C99_THROWN) {unwind} if (status != KILN_C99_OK) return status; }} }}",
                c_string(&format!("{} is not defined", text.to_scalar_string().unwrap_or_default())),
            )
            .unwrap();
        }
        Instruction::StoreGlobalBinding(name) => {
            // Sloppy code settles an unresolvable assignment against the global object, which
            // creates the property. The interpreter does the same thing through its own global
            // object; both engines have to, or a UMD header installs itself under one and throws
            // under the other -- a divergence worse than the shared refusal it replaced.
            let Constant::String(_) = &constants[*name as usize] else {
                unreachable!("verification rejects a non-string binding name before emission")
            };
            writeln!(
                output,
                "    {{ kiln_value member = kiln_val_to_boundary(stack[--sp]); kiln_value globals; int status = kiln_c99_global_object(&globals); if (status != KILN_C99_OK) return status; status = kiln_c99_put_mode_key(host, globals, {symbol}_constant_{name}, NULL, member, 0, depth, thrown); if (status == KILN_C99_THROWN) {unwind} if (status != KILN_C99_OK) return status; }}",
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
            if *compact {
                writeln!(
                    output,
                    "    {{ int status = kiln_c99_tail_define_accessor(stack, &sp, {}, {}, {}, (size_t){index}, {instruction_operation}, (size_t)kiln_c99_diagnostic_active_depth); if (status != KILN_C99_OK) return status; }}",
                    i32::from(*setter),
                    i32::from(*enumerable),
                    diagnostic_source(diagnostic_site),
                )
                .unwrap();
                return;
            }
            let half = if *setter { "setter" } else { "getter" };
            let other = if *setter { "getter" } else { "setter" };
            let prefix = if *setter { "set" } else { "get" };
            let enumerable = i32::from(*enumerable);
            writeln!(
                output,
                "    {{ kiln_value {half} = kiln_val_to_boundary(stack[--sp]); kiln_value key = kiln_val_to_boundary(stack[--sp]); kiln_value {other} = kiln_c99_undefined(); kiln_value name = kiln_c99_property_key(key); int status; if (name.tag != KILN_VALUE_STRING) {{ {direct_refusal} }} status = kiln_c99_set_function_name({half}, key, \"{prefix}\"); if (status != KILN_C99_OK) return status; status = kiln_c99_define_accessor_key(kiln_val_to_boundary(stack[sp - 1]), (const kiln_string *)name.ptr, getter, setter, {enumerable}); if (status != KILN_C99_OK) return status; }}"
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
                "    {{ kiln_value function = kiln_val_to_boundary(stack[--sp]); kiln_value key = kiln_c99_value(KILN_VALUE_STRING, 0, (const char *){symbol}_constant_{name}, {len}); kiln_string private_key = *{symbol}_constant_{name}; private_key.flags |= KILN_STRING_PRIVATE_KEY; if (kiln_c99_set_function_name(function, key, {prefix}) != KILN_C99_OK) {{ {direct_refusal} }} if (kiln_c99_define_private_key(kiln_val_to_boundary(stack[sp - 1]), &private_key, function, {tag}) != KILN_C99_OK) {{ {direct_refusal} }} }}",
                len = text.len()
            )
            .unwrap();
        }
        // A class's methods are not enumerated, which is the whole difference from the
        // `SetProperty` an object literal's methods lower to.
        // The enumerated sibling of `DefineMethod`: an object literal's members are listed,
        // a class's methods are not.
        // The shape fast path in `spread.c` is a pure optimisation of the same operation the
        // `else` branch already implements: `kiln_c99_define_named_fast` only ever reports success
        // after actually storing `value`, and every miss (a dictionary-mode, frozen,
        // non-extensible or accessor-bearing target; a non-string key; a function value that
        // still needs `kiln_c99_set_function_name`; `__proto__`; a shape the transition tree
        // refuses) falls through to the untouched generic sequence. See its definition in
        // spread.c for why the target needs no `kiln_c99_gc_kind_of_ptr` confirmation beyond the
        // cheap one it already does.
        Instruction::DefineNamedProperty { name_function } => {
            // A pair defined here only because it follows a spread names nothing (issue 28).
            let name_function = if *name_function {
                format!(
                    "if (kiln_c99_set_function_name(method, key, NULL) != KILN_C99_OK) {{ {direct_refusal} }} "
                )
            } else {
                String::new()
            };
            writeln!(
                output,
                "    {{ static kiln_c99_site kiln_dnp_site = {{0}}; kiln_val kiln_dnp_value = stack[--sp]; kiln_val kiln_dnp_key = stack[--sp]; int kiln_dnp_status; if (kiln_c99_define_named_fast(stack[sp - 1], kiln_dnp_key, kiln_dnp_value, &kiln_dnp_status, &kiln_dnp_site)) {{ if (kiln_dnp_status != KILN_C99_OK) {{ {direct_refusal} }} }} else {{ kiln_value method = kiln_val_to_boundary(kiln_dnp_value); kiln_value key = kiln_val_to_boundary(kiln_dnp_key); kiln_value name = kiln_c99_property_key(key); if (name.tag != KILN_VALUE_STRING) {{ {direct_refusal} }} {name_function}if (kiln_c99_define_computed(kiln_val_to_boundary(stack[sp - 1]), name, method) != KILN_C99_OK) {{ {direct_refusal} }} }} }}"
            )
            .unwrap();
        }
        Instruction::DefineMethod => {
            if *compact {
                writeln!(
                    output,
                    "    {{ int status = kiln_c99_tail_define_method(stack, &sp, {}, (size_t){index}, {instruction_operation}, (size_t)kiln_c99_diagnostic_active_depth); if (status != KILN_C99_OK) return status; }}",
                    diagnostic_source(diagnostic_site),
                )
                .unwrap();
                return;
            }
            writeln!(
                output,
                "    {{ kiln_value method = kiln_val_to_boundary(stack[--sp]); kiln_value key = kiln_val_to_boundary(stack[--sp]); kiln_value name = kiln_c99_property_key(key); if (name.tag != KILN_VALUE_STRING) {{ {direct_refusal} }} if (kiln_c99_set_function_name(method, key, NULL) != KILN_C99_OK) {{ {direct_refusal} }} if (kiln_c99_define_method_key(kiln_val_to_boundary(stack[sp - 1]), (const kiln_string *)name.ptr, method) != KILN_C99_OK) {{ {direct_refusal} }} }}"
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
            let Constant::String(_) = &constants[*name as usize] else {
                unreachable!("verification rejects a non-string property name before emission")
            };
            writeln!(
                output,
                "    {{ int answer; int status = kiln_c99_delete_or_throw_key(host, kiln_val_to_boundary(stack[sp - 1]), {symbol}_key_{name}, {}, &answer, depth, thrown); if (status == KILN_C99_THROWN) {unwind} if (status != KILN_C99_OK) return status; stack[sp - 1] = answer ? KILN_VAL_TRUE : KILN_VAL_FALSE; }}",
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
            if *compact {
                writeln!(
                    output,
                    "    {{ int status = kiln_c99_tail_apply(host, stack, &sp, depth, thrown, {}, (size_t){index}, {instruction_operation}, (size_t)kiln_c99_diagnostic_active_depth); if (status == KILN_C99_THROWN) {{ {unwind} }} if (status != KILN_C99_OK) return status; }}",
                    diagnostic_source(diagnostic_site),
                )
                .unwrap();
                return;
            }
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
        // The same substitution as `DefineNamedProperty`'s, and for the same reason: a shaped,
        // fresh-into-fresh spread (`{ ...base }`) is a bulk `memcpy` of NaN-boxed slots that
        // `kiln_c99_spread_object` already special-cases (see spread.c), but reaching that branch
        // its own way costs a `kiln_val_to_boundary` per operand -- which for an object resolves
        // the exact kind through a slab-table lookup this case never needed. A miss falls through
        // to the untouched generic call.
        Instruction::SpreadObject => {
            writeln!(
                output,
                "    {{ kiln_val kiln_spread_source = stack[--sp]; int kiln_spread_status; if (kiln_c99_spread_object_fast(stack[sp - 1], kiln_spread_source, &kiln_spread_status)) {{ if (kiln_spread_status != KILN_C99_OK) return kiln_spread_status; }} else {{ kiln_value source = kiln_val_to_boundary(kiln_spread_source); int status = kiln_c99_spread_object(host, kiln_val_to_boundary(stack[sp - 1]), source, depth, thrown); if (status == KILN_C99_THROWN) {unwind} if (status != KILN_C99_OK) return status; }} }}"
            )
            .unwrap();
        }
        // `class D extends C` links the two prototype objects, which is what makes an instance
        // of `D` find a method of `C`.
        Instruction::SetPrototype => {
            writeln!(
                output,
                "    {{ kiln_value prototype = kiln_val_to_boundary(stack[--sp]); int status = kiln_c99_set_prototype_checked(kiln_val_to_boundary(stack[sp - 1]), prototype, thrown); if (status == KILN_C99_THROWN) {unwind} if (status != KILN_C99_OK) return status; }}"
            )
            .unwrap();
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
                "    {{ kiln_value read; int status = kiln_c99_get_with_receiver(host, kiln_val_to_boundary(stack[sp - 1]), kiln_c99_value(KILN_VALUE_STRING, 0, (const char *){symbol}_key_{name}, {}), kiln_val_to_boundary(receiver), &read, depth, thrown); if (status == KILN_C99_THROWN) {unwind} if (status != KILN_C99_OK) return status; stack[sp - 1] = kiln_val_from_boundary(read); }}",
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
        Instruction::SetSuperPropertyStrict { name, strict } => {
            let Constant::String(text) = &constants[*name as usize] else {
                unreachable!("verification rejects a non-string property name before emission")
            };
            let strict_num = if *strict { 1 } else { 0 };
            writeln!(
                output,
                "    {{ kiln_value proto = kiln_val_to_boundary(stack[sp - 2]); kiln_value val = kiln_val_to_boundary(stack[sp - 1]); int status = kiln_c99_set_with_receiver(host, proto, kiln_c99_value(KILN_VALUE_STRING, 0, (const char *){symbol}_key_{name}, {}), val, kiln_val_to_boundary(receiver), {strict_num}, depth, thrown); if (status == KILN_C99_THROWN) {unwind} if (status != KILN_C99_OK) return status; stack[sp - 2] = stack[sp - 1]; sp--; }}",
                text.len()
            )
            .unwrap();
        }
        Instruction::SetSuperElementStrict { strict } => {
            let strict_num = if *strict { 1 } else { 0 };
            writeln!(
                output,
                "    {{ kiln_value key = kiln_val_to_boundary(stack[sp - 3]); kiln_value proto = kiln_val_to_boundary(stack[sp - 2]); kiln_value val = kiln_val_to_boundary(stack[sp - 1]); int status = kiln_c99_set_with_receiver(host, proto, key, val, kiln_val_to_boundary(receiver), {strict_num}, depth, thrown); if (status == KILN_C99_THROWN) {unwind} if (status != KILN_C99_OK) return status; stack[sp - 3] = stack[sp - 1]; sp -= 2; }}"
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
        Instruction::LoadTemplateObject(site) => {
            writeln!(
                output,
                "    if ({symbol}_template_sites[{site}] == 0) {{ int status = {symbol}_materialize_template_site({site}); if (status != KILN_C99_OK) return status; }} stack[sp++] = {symbol}_template_sites[{site}];"
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
                writeln!(output, "    {{ kiln_value host_result; kiln_value host_args[{argc}]; for (size_t i = 0; i < (size_t){argc}; i++) {{ kiln_value arg_val = kiln_val_to_boundary(stack[sp - {argc} + i]); if (kiln_c99_crosses_boundary(arg_val)) arg_val = {}arg_val); host_args[i] = arg_val; }} if (host == NULL || host->call == NULL || host->call(host->context, UINT16_C({}), UINT16_C({argc}), host_args, &host_result) != 0) return KILN_C99_HOST_ERROR; sp -= {argc}; stack[sp++] = kiln_val_from_boundary(host_result); }}", host_argument_inspection(*op), host_op_code(*op)).unwrap()
            } else {
                writeln!(output, "    {{ kiln_value host_result; kiln_value host_args[{argc}]; for (size_t i = 0; i < (size_t){argc}; i++) {{ host_args[i] = kiln_val_to_boundary(stack[sp - {argc} + i]); }} if (host == NULL || host->call == NULL || host->call(host->context, UINT16_C({}), UINT16_C({argc}), host_args, &host_result) != 0) return KILN_C99_HOST_ERROR; sp -= {argc}; stack[sp++] = kiln_val_from_boundary(host_result); }}", host_op_code(*op)).unwrap()
            }
        }
        Instruction::DynamicImport => {
            writeln!(
                output,
                "    {{ kiln_value promise; int status = kiln_graph_dynamic_import(host, {importer}, kiln_val_to_boundary(stack[sp - 1]), kiln_c99_undefined(), &promise, depth, thrown); if (status == KILN_C99_THROWN) {unwind} if (status != KILN_C99_OK) return status; stack[sp - 1] = kiln_val_from_boundary(promise); }}",
                importer = if *compact { "ku_importer".to_owned() } else { c_string(symbol) },
            )
            .unwrap();
        }
        Instruction::DynamicImportWithOptions => {
            writeln!(
                output,
                "    {{ kiln_value promise; int status = kiln_graph_dynamic_import(host, {importer}, kiln_val_to_boundary(stack[sp - 2]), kiln_val_to_boundary(stack[sp - 1]), &promise, depth, thrown); if (status == KILN_C99_THROWN) {unwind} if (status != KILN_C99_OK) return status; sp -= 2; stack[sp++] = kiln_val_from_boundary(promise); }}",
                importer = if *compact { "ku_importer".to_owned() } else { c_string(symbol) },
            )
            .unwrap();
        }
        Instruction::DirectEval(site) => {
            let binding_count = site.bindings.len();
            let array_count = binding_count.max(1);
            output.push_str("    int status = KILN_C99_OK;\n");
            writeln!(output, "    {{ const char *eval_names[{array_count}] = {{").unwrap();
            for binding in &site.bindings {
                writeln!(output, "        {},", c_string(&binding.name)).unwrap();
            }
            if binding_count == 0 {
                output.push_str("        NULL,\n");
            }
            writeln!(
                output,
                "    }}; size_t eval_name_lengths[{array_count}] = {{"
            )
            .unwrap();
            for binding in &site.bindings {
                writeln!(output, "        (size_t){},", binding.name.len()).unwrap();
            }
            if binding_count == 0 {
                output.push_str("        0,\n");
            }
            writeln!(
                output,
                "    }}; kiln_value eval_binding_values[{array_count}] = {{"
            )
            .unwrap();
            for binding in &site.bindings {
                let value = match binding.access {
                    EvalAccess::Cell(slot) => format!("locals[{slot}]"),
                    EvalAccess::Upvalue(index) => format!("upvalues[{index}]"),
                };
                writeln!(output, "        kiln_val_to_boundary({value}),").unwrap();
            }
            if binding_count == 0 {
                output.push_str("        kiln_c99_undefined(),\n");
            }
            output.push_str("    }; kiln_value eval_source = kiln_c99_undefined();\n");

            let fixed_argc = site.argc.map(usize::from);
            match fixed_argc {
                Some(argc) => {
                    if argc > 0 {
                        output.push_str("    eval_source = kiln_val_to_boundary(stack[0]);\n");
                    }
                    if argc > 1 {
                        writeln!(output, "    kiln_value eval_args[{argc}] = {{").unwrap();
                        for index in 0..argc {
                            writeln!(output, "        kiln_val_to_boundary(stack[{index}]),")
                                .unwrap();
                        }
                        output.push_str("    };\n");
                    }
                    writeln!(output, "    size_t eval_argc = (size_t){argc};").unwrap();
                    if argc > 1 {
                        output.push_str("    const kiln_value *eval_call_args = eval_args;\n");
                    } else if argc == 1 {
                        output.push_str("    kiln_value eval_args[1] = { eval_source }; const kiln_value *eval_call_args = eval_args;\n");
                    } else {
                        output.push_str("    const kiln_value *eval_call_args = NULL;\n");
                    }
                }
                None => {
                    output.push_str("    kiln_value eval_spread = kiln_val_to_boundary(stack[0]); kiln_array *eval_array = eval_spread.tag == KILN_VALUE_ARRAY ? kiln_c99_array_of(eval_spread) : NULL; if (eval_array == NULL) return KILN_C99_INVALID_ARGUMENT; eval_source = eval_array->count ? kiln_val_to_boundary(eval_array->elements[0] == KILN_VAL_HOLE ? KILN_VAL_UNDEFINED : eval_array->elements[0]) : kiln_c99_undefined();\n");
                    output.push_str("    size_t eval_argc = eval_array != NULL ? eval_array->count : 0; kiln_value eval_args[eval_argc ? eval_argc : 1]; if (eval_array != NULL) for (size_t eval_i = 0; eval_i < eval_argc; ++eval_i) eval_args[eval_i] = kiln_val_to_boundary(eval_array->elements[eval_i] == KILN_VAL_HOLE ? KILN_VAL_UNDEFINED : eval_array->elements[eval_i]); const kiln_value *eval_call_args = eval_array != NULL ? eval_args : NULL;\n");
                }
            }
            output.push_str("    kiln_value eval_callee = kiln_c99_undefined(); int eval_found = 0; status = kiln_c99_global_binding(host, &eval_callee, \"eval\", 4, &eval_found, depth, thrown); if (status == KILN_C99_THROWN) { ");
            output.push_str(unwind);
            output.push_str(" } if (status != KILN_C99_OK) return status; if (!eval_found) { status = kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_REFERENCE_ERROR, \"eval is not defined\"); if (status == KILN_C99_THROWN) { ");
            output.push_str(unwind);
            output.push_str(" } if (status != KILN_C99_OK) return status; } kiln_value eval_result = kiln_c99_undefined(); if (eval_callee.tag != KILN_VALUE_BUILTIN || eval_callee.bits != KILN_C99_BUILTIN_EVAL) { status = kiln_c99_call(host, &eval_result, eval_callee, kiln_c99_undefined(), eval_call_args, eval_argc, depth, thrown); } else if (eval_source.tag != KILN_VALUE_STRING) { eval_result = eval_source; } else { status = kiln_c99_dynamic_eval_function_expression(host, eval_source, eval_names, eval_name_lengths, eval_binding_values, (size_t)");
            write!(output, "{binding_count}").unwrap();
            write!(output, ", {}, &eval_result, depth, thrown); }} if (status == KILN_C99_THROWN) {{ ", i32::from(site.strict)).unwrap();
            output.push_str(unwind);
            output.push_str(" } if (status != KILN_C99_OK) return status; sp = 0; stack[sp++] = kiln_val_from_boundary(eval_result); }\n");
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
