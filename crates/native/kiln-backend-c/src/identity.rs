use super::*;

pub(super) fn hash_program_pair_with_policy(
    program: &Program,
    policy: &EmissionPolicy,
) -> (u64, u64) {
    (
        hash_program_with_policy_and_seed(program, policy, 0xcbf29ce484222325),
        hash_program_with_policy_and_seed(program, policy, 0x84222325cbf29ce4),
    )
}

pub(super) fn hash_program_with_policy_and_seed(
    program: &Program,
    policy: &EmissionPolicy,
    seed: u64,
) -> u64 {
    let mut hash = hash_program_with_seed(program, seed);
    let mut canonical_policy;
    let policy = if policy.speculate == Some(true) {
        canonical_policy = policy.clone();
        canonical_policy.speculate = None;
        &canonical_policy
    } else {
        policy
    };
    if *policy == EmissionPolicy::default() {
        return hash;
    }
    let (entropy_tag, entropy_seed) = match policy.entropy {
        EmissionEntropyPolicy::Deterministic => (0, None),
        EmissionEntropyPolicy::Seeded(seed) => (1, Some(seed)),
        EmissionEntropyPolicy::Host => (2, None),
    };
    for value in [
        0x70,
        u8::from(policy.host_clock),
        entropy_tag,
        u8::from(policy.decoupled_prelude),
        u8::from(policy.compact),
        u8::from(policy.generic_only),
        u8::from(!matches!(policy.speculate, Some(false))),
    ] {
        hash ^= u64::from(value);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    // Mixed in only when it changes the emission, so every existing key is unchanged.
    if policy.generic_only && policy.parallel_in_generic {
        hash ^= 0x7f;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    if policy.no_kiln_extensions {
        hash ^= 0x7e;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    if let Some(bytes) = policy.unit_chunk_bytes {
        hash ^= 0x7d;
        hash = hash.wrapping_mul(0x100000001b3);
        for value in (bytes as u64).to_le_bytes() {
            hash ^= u64::from(value);
            hash = hash.wrapping_mul(0x100000001b3);
        }
    }
    if let Some(seed) = entropy_seed {
        for value in seed.to_le_bytes() {
            hash ^= u64::from(value);
            hash = hash.wrapping_mul(0x100000001b3);
        }
    }
    hash_string_policy(&mut hash, &policy.timezone);
    hash_string_policy(&mut hash, &policy.locale);
    if let Some(slots) = &policy.live_module_slots {
        hash ^= 0x74;
        hash = hash.wrapping_mul(0x100000001b3);
        for slot in slots.iter() {
            for value in slot.to_le_bytes() {
                hash ^= u64::from(value);
                hash = hash.wrapping_mul(0x100000001b3);
            }
        }
    }
    hash
}

pub(super) fn hash_string_policy(hash: &mut u64, policy: &EmissionStringPolicy) {
    let (tag, named) = match policy {
        EmissionStringPolicy::Deterministic => (0_u8, None),
        EmissionStringPolicy::Named(value) => (1, Some(value.as_str())),
        EmissionStringPolicy::Host => (2, None),
    };
    *hash ^= u64::from(tag);
    *hash = hash.wrapping_mul(0x100000001b3);
    if let Some(value) = named {
        for byte in value.as_bytes() {
            *hash ^= u64::from(*byte);
            *hash = hash.wrapping_mul(0x100000001b3);
        }
        *hash ^= 0xff;
        *hash = hash.wrapping_mul(0x100000001b3);
    }
}

#[cfg(test)]
pub(super) fn hash_program(program: &Program) -> u64 {
    hash_program_with_seed(program, 0xcbf29ce484222325)
}

pub(super) fn hash_program_with_seed(program: &Program, seed: u64) -> u64 {
    let mut hash = seed;
    fn byte(hash: &mut u64, value: u8) {
        *hash ^= u64::from(value);
        *hash = hash.wrapping_mul(0x100000001b3);
    }
    fn word(hash: &mut u64, value: u64) {
        for value in value.to_le_bytes() {
            byte(hash, value);
        }
    }
    word(&mut hash, u64::from(C_ABI_VERSION));
    word(&mut hash, u64::from(HOST_ABI_VERSION));
    word(&mut hash, u64::from(program.version));
    word(&mut hash, program.constants.len() as u64);
    for constant in &program.constants {
        match constant {
            Constant::Undefined => byte(&mut hash, 0),
            Constant::Null => byte(&mut hash, 1),
            Constant::Boolean(value) => {
                byte(&mut hash, 2);
                byte(&mut hash, u8::from(*value));
            }
            Constant::Number(bits) => {
                byte(&mut hash, 3);
                word(&mut hash, *bits);
            }
            Constant::String(value) => {
                byte(&mut hash, 4);
                word(&mut hash, value.len() as u64);
                for value in value.units() {
                    word(&mut hash, u64::from(value));
                }
            }
            // Tag 5, appended: a persisted key never renumbers.
            Constant::BigInt(digits) => {
                byte(&mut hash, 5);
                word(&mut hash, digits.len() as u64);
                for digit in digits.bytes() {
                    byte(&mut hash, digit);
                }
            }
            Constant::WellKnownSymbol(symbol) => {
                byte(&mut hash, 6);
                let key = symbol.property_key();
                word(&mut hash, key.len() as u64);
                for digit in key.bytes() {
                    byte(&mut hash, digit);
                }
            }
        }
    }
    word(&mut hash, program.code.len() as u64);
    for instruction in &program.code {
        match instruction {
            // Tags are a persisted cache key: existing ones never change, new instructions
            // take the next free values. Reusing a tag would make two different programs hash
            // alike and serve one's object file for the other.
            Instruction::CallMethod { name, argc } => {
                byte(&mut hash, 31);
                word(&mut hash, u64::from(*name));
                word(&mut hash, u64::from(*argc));
            }
            Instruction::CallComputed { argc } => {
                byte(&mut hash, 32);
                word(&mut hash, u64::from(*argc));
            }
            Instruction::CallMethodPrepared { name, argc } => {
                byte(&mut hash, 199);
                word(&mut hash, u64::from(*name));
                word(&mut hash, u64::from(*argc));
            }
            Instruction::SuperForward => byte(&mut hash, 36),
            Instruction::SetPrototype => byte(&mut hash, 34),
            Instruction::LoadPrototype => byte(&mut hash, 71),
            Instruction::LoadSuperProperty(value) => {
                byte(&mut hash, 72);
                word(&mut hash, u64::from(*value));
            }
            Instruction::LoadSuperElement => byte(&mut hash, 73),
            Instruction::SetSuperPropertyStrict { name, strict } => {
                byte(&mut hash, 80);
                word(&mut hash, u64::from(*name));
                byte(&mut hash, u8::from(*strict));
            }
            Instruction::SetSuperElementStrict { strict } => {
                byte(&mut hash, 81);
                byte(&mut hash, u8::from(*strict));
            }
            Instruction::SuperCall { argc } => {
                byte(&mut hash, 35);
                word(&mut hash, u64::from(*argc));
            }
            Instruction::PushHandler(target) => {
                byte(&mut hash, 38);
                word(&mut hash, u64::from(*target));
            }
            Instruction::PopHandler => byte(&mut hash, 39),
            Instruction::Yield => byte(&mut hash, 42),
            Instruction::YieldStar(target) => {
                byte(&mut hash, 197);
                word(&mut hash, u64::from(*target));
            }
            Instruction::Await => byte(&mut hash, 60),
            Instruction::Throw => byte(&mut hash, 40),
            Instruction::LoadArguments => byte(&mut hash, 37),
            Instruction::RestArguments(count) => {
                byte(&mut hash, 58);
                word(&mut hash, u64::from(*count));
            }
            Instruction::LoadThis => byte(&mut hash, 33),
            Instruction::LoadNewTarget => byte(&mut hash, 59),
            Instruction::Dup2 => byte(&mut hash, 27),
            Instruction::IterableToArray => byte(&mut hash, 44),
            Instruction::SpreadInto => byte(&mut hash, 41),
            Instruction::SpreadObject => byte(&mut hash, 45),
            Instruction::DeleteComputed => byte(&mut hash, 47),
            Instruction::DeleteComputedStrict { strict } => {
                byte(&mut hash, 66);
                byte(&mut hash, u8::from(*strict));
            }
            Instruction::Apply => byte(&mut hash, 48),
            Instruction::SuperApply => byte(&mut hash, 61),
            // 75: the next free tag. Never reuse one -- two programs hashing alike would serve
            // one's object file for the other.
            Instruction::DirectEval(_) => byte(&mut hash, 62),
            Instruction::DynamicImport => byte(&mut hash, 63),
            Instruction::DynamicImportWithOptions => byte(&mut hash, 67),
            Instruction::ToObject => byte(&mut hash, 64),
            Instruction::WithHas(name) => {
                byte(&mut hash, 65);
                word(&mut hash, u64::from(*name));
            }
            Instruction::EnumerateKeys => byte(&mut hash, 50),
            Instruction::PushHole => byte(&mut hash, 55),
            Instruction::ThrowReference(value) => {
                byte(&mut hash, 57);
                word(&mut hash, u64::from(*value));
            }
            Instruction::StoreGlobalBinding(value) => {
                byte(&mut hash, 74);
                word(&mut hash, u64::from(*value));
            }
            Instruction::StoreGlobalBindingStrict(value) => {
                byte(&mut hash, 77);
                word(&mut hash, u64::from(*value));
            }
            Instruction::RequireGlobalBinding(value) => {
                byte(&mut hash, 78);
                word(&mut hash, u64::from(*value));
            }
            Instruction::LoadGlobalBindingOrUndefined(value) => {
                byte(&mut hash, 79);
                word(&mut hash, u64::from(*value));
            }
            Instruction::DeleteGlobalBinding(value) => {
                byte(&mut hash, 75);
                word(&mut hash, u64::from(*value));
            }
            Instruction::DeclareGlobalBinding { name, configurable } => {
                byte(&mut hash, 76);
                word(&mut hash, u64::from(*name));
                byte(&mut hash, u8::from(*configurable));
            }
            Instruction::NewRegExp { pattern, flags } => {
                byte(&mut hash, 56);
                word(&mut hash, u64::from(*pattern));
                word(&mut hash, u64::from(*flags));
            }
            Instruction::GetPrivate(value) => {
                byte(&mut hash, 53);
                word(&mut hash, u64::from(*value));
            }
            Instruction::SetPrivate(value) => {
                byte(&mut hash, 54);
                word(&mut hash, u64::from(*value));
            }
            Instruction::DefineAccessor { setter, enumerable } => {
                byte(&mut hash, 52);
                byte(&mut hash, u8::from(*setter));
                byte(&mut hash, u8::from(*enumerable));
            }
            Instruction::DefineMethod => byte(&mut hash, 43),
            Instruction::DefineNamedProperty { name_function } => {
                byte(&mut hash, 194);
                byte(&mut hash, u8::from(*name_function));
            }
            Instruction::LoadTemplateObject(site) => {
                byte(&mut hash, 195);
                word(&mut hash, u64::from(*site));
            }
            Instruction::DefinePrivate { name, kind } => {
                byte(&mut hash, 70);
                word(&mut hash, u64::from(*name));
                byte(
                    &mut hash,
                    match kind {
                        kiln_ir::PrivateKind::Method => 0,
                        kiln_ir::PrivateKind::Getter => 1,
                        kiln_ir::PrivateKind::Setter => 2,
                    },
                );
            }
            Instruction::MarkUninitialized(value) => {
                byte(&mut hash, 51);
                word(&mut hash, u64::from(*value));
            }
            Instruction::ConstructSpread => byte(&mut hash, 49),
            Instruction::DeleteProperty(value) => {
                byte(&mut hash, 46);
                word(&mut hash, u64::from(*value));
            }
            Instruction::DeletePropertyStrict { name, strict } => {
                byte(&mut hash, 67);
                word(&mut hash, u64::from(*name));
                byte(&mut hash, u8::from(*strict));
            }
            Instruction::NewArray { count } => {
                byte(&mut hash, 28);
                word(&mut hash, u64::from(*count));
            }
            Instruction::GetComputed => byte(&mut hash, 29),
            Instruction::ToPropertyKey => byte(&mut hash, 193),
            Instruction::SetComputed => byte(&mut hash, 30),
            Instruction::SetComputedStrict { strict } => {
                byte(&mut hash, 68);
                byte(&mut hash, u8::from(*strict));
            }
            Instruction::DefineOwnProperty(name) => {
                byte(&mut hash, 194);
                word(&mut hash, u64::from(*name));
            }
            Instruction::DefineOwnComputed => byte(&mut hash, 195),
            Instruction::LoadBuiltin(builtin) => {
                byte(&mut hash, 25);
                word(&mut hash, *builtin as u64);
            }
            Instruction::LoadIntrinsicBuiltin(builtin) => {
                byte(&mut hash, 197);
                word(&mut hash, *builtin as u64);
            }
            Instruction::New { argc } => {
                byte(&mut hash, 26);
                word(&mut hash, u64::from(*argc));
            }
            Instruction::NewObject { count } => {
                byte(&mut hash, 22);
                word(&mut hash, u64::from(*count));
            }
            Instruction::GetProperty(value) => {
                byte(&mut hash, 23);
                word(&mut hash, u64::from(*value));
            }
            Instruction::SetProperty(value) => {
                byte(&mut hash, 24);
                word(&mut hash, u64::from(*value));
            }
            Instruction::SetPropertyStrict { name, strict } => {
                byte(&mut hash, 69);
                word(&mut hash, u64::from(*name));
                byte(&mut hash, u8::from(*strict));
            }
            Instruction::NewCell(value) => {
                byte(&mut hash, 17);
                word(&mut hash, u64::from(*value));
            }
            Instruction::LoadCell(value) => {
                byte(&mut hash, 18);
                word(&mut hash, u64::from(*value));
            }
            Instruction::StoreCell(value) => {
                byte(&mut hash, 19);
                word(&mut hash, u64::from(*value));
            }
            Instruction::LoadUpvalue(value) => {
                byte(&mut hash, 20);
                word(&mut hash, u64::from(*value));
            }
            Instruction::StoreUpvalue(value) => {
                byte(&mut hash, 21);
                word(&mut hash, u64::from(*value));
            }
            Instruction::MarkCellFunctionName(value) => {
                byte(&mut hash, 198);
                word(&mut hash, u64::from(*value));
            }
            Instruction::MakeFunction(value) => {
                byte(&mut hash, 13);
                word(&mut hash, u64::from(*value));
            }
            Instruction::Call { argc } => {
                byte(&mut hash, 14);
                word(&mut hash, u64::from(*argc));
            }
            Instruction::LoadGlobal(value) => {
                byte(&mut hash, 15);
                word(&mut hash, u64::from(*value));
            }
            Instruction::StoreGlobal(value) => {
                byte(&mut hash, 16);
                word(&mut hash, u64::from(*value));
            }
            Instruction::Const(value) => {
                byte(&mut hash, 0);
                word(&mut hash, u64::from(*value));
            }
            Instruction::LoadLocal(value) => {
                byte(&mut hash, 1);
                word(&mut hash, u64::from(*value));
            }
            Instruction::StoreLocal(value) => {
                byte(&mut hash, 2);
                word(&mut hash, u64::from(*value));
            }
            Instruction::Pop => byte(&mut hash, 3),
            Instruction::Dup => byte(&mut hash, 4),
            Instruction::Unary(value) => {
                byte(&mut hash, 5);
                word(&mut hash, u64::from(unary_code(*value)));
            }
            Instruction::Binary(value) => {
                byte(&mut hash, 6);
                word(&mut hash, u64::from(binary_code(*value)));
            }
            Instruction::Jump(value) => {
                byte(&mut hash, 7);
                word(&mut hash, u64::from(*value));
            }
            Instruction::JumpIfFalse(value) => {
                byte(&mut hash, 8);
                word(&mut hash, u64::from(*value));
            }
            Instruction::JumpIfTruthy(value) => {
                byte(&mut hash, 9);
                word(&mut hash, u64::from(*value));
            }
            Instruction::JumpIfNotNullish(value) => {
                byte(&mut hash, 10);
                word(&mut hash, u64::from(*value));
            }
            Instruction::JumpIfReturnSignal(value) => {
                byte(&mut hash, 65);
                word(&mut hash, u64::from(*value));
            }
            Instruction::HostCall { op, argc } => {
                byte(&mut hash, 11);
                word(&mut hash, u64::from(host_op_code(*op)));
                word(&mut hash, u64::from(*argc));
            }
            Instruction::Return => byte(&mut hash, 12),
            Instruction::Nop => byte(&mut hash, 196),
        }
    }
    word(&mut hash, u64::from(program.local_count));
    // Functions are a flat, stable table.  Keep the table marker and count even when it is
    // empty: otherwise an empty table and a stream ending at local_count could be confused by
    // future extensions.  MakeFunction operands are table indices, so hashing the table in
    // order also handles cyclic references without recursive traversal.
    byte(&mut hash, 0x60);
    word(&mut hash, program.functions.len() as u64);
    for function in &program.functions {
        hash_function_proto(&mut hash, function);
    }
    // Appended last, after every byte that was already a persisted cache key. The source is part
    // of the program's identity now that `Function.prototype.toString` answers with it: two units
    // with identical bytecode and different text are not interchangeable, and a cached artifact
    // keyed without this would answer with whichever text was compiled first.
    byte(&mut hash, 0x63);
    word(&mut hash, program.source.len() as u64);
    for value in program.source.bytes() {
        byte(&mut hash, value);
    }
    match &program.source_origin {
        Some(origin) => {
            byte(&mut hash, 0x65);
            word(&mut hash, origin.units.len() as u64);
            for unit in origin.units.units() {
                word(&mut hash, u64::from(unit));
            }
            word(&mut hash, origin.byte_to_unit.len() as u64);
            for offset in origin.byte_to_unit.iter() {
                word(&mut hash, u64::from(*offset));
            }
        }
        None => {}
    }
    if !program.template_sites.is_empty() {
        byte(&mut hash, 0x64);
        word(&mut hash, program.template_sites.len() as u64);
        for site in &program.template_sites {
            word(&mut hash, site.cooked.len() as u64);
            for segment in &site.cooked {
                match segment {
                    Some(text) => {
                        byte(&mut hash, 1);
                        word(&mut hash, text.len() as u64);
                        for unit in text.units() {
                            word(&mut hash, u64::from(unit));
                        }
                    }
                    None => byte(&mut hash, 0),
                }
            }
            word(&mut hash, site.raw.len() as u64);
            for segment in &site.raw {
                word(&mut hash, segment.len() as u64);
                for unit in segment.units() {
                    word(&mut hash, u64::from(unit));
                }
            }
        }
    }
    // Source coordinates affect emitted frame metadata even though they do not alter bytecode.
    // Keep this as a trailing, tagged component so older persisted prefixes remain stable.
    if !program.debug_call_sites.is_empty() {
        byte(&mut hash, 0x66);
        word(&mut hash, program.debug_call_sites.len() as u64);
        for site in &program.debug_call_sites {
            match site.function_span {
                Some(span) => {
                    byte(&mut hash, 1);
                    word(&mut hash, u64::from(span.start));
                    word(&mut hash, u64::from(span.end));
                }
                None => byte(&mut hash, 0),
            }
            word(&mut hash, u64::from(site.instruction));
            word(&mut hash, u64::from(site.line));
            word(&mut hash, u64::from(site.column));
        }
    }
    hash
}

pub(super) fn hash_function_proto(hash: &mut u64, function: &FunctionProto) {
    fn byte(hash: &mut u64, value: u8) {
        *hash ^= u64::from(value);
        *hash = hash.wrapping_mul(0x100000001b3);
    }
    fn word(hash: &mut u64, value: u64) {
        for value in value.to_le_bytes() {
            byte(hash, value);
        }
    }

    byte(hash, 0x61);
    byte(hash, u8::from(function.asynchronous));
    // Identity, not display: a present written name shadows any inferred one
    // (equal programs must hit), while an empty written name is distinct
    // from no written name (different statics emitted). `effective_name_units`
    // filters empties for display and would conflate the last two, serving
    // one's object file for the other.
    match function
        .name
        .as_deref()
        .map(kiln_ir::StringUnits::from)
        .or_else(|| function.inferred_name.clone())
    {
        Some(name) => {
            byte(hash, 1);
            word(hash, name.unit_len() as u64);
            for unit in name.units() {
                word(hash, u64::from(unit));
            }
        }
        None => byte(hash, 0),
    }
    byte(hash, u8::from(function.arrow));
    byte(hash, u8::from(function.generator));
    // Appended after `generator`, which is where a new flag has to go: the bytes before it are a
    // persisted cache key and cannot move.
    byte(hash, u8::from(function.async_generator));
    byte(hash, u8::from(function.is_method));
    word(hash, u64::from(function.parameter_count));
    word(hash, u64::from(function.local_count));

    // Reuse the canonical instruction encoder, including its persisted tag assignments. Seed it
    // with the hash accumulated so far and retain the resulting state directly: reducing the
    // function body to an independently seeded 64-bit digest here would introduce an avoidable
    // collision boundary inside the program identity. The synthetic program's empty function
    // table is also what makes numeric MakeFunction cycles terminate without recursive hashing.
    // Which bytes of the source this body is, appended after the flags that were already
    // persisted so the prefix does not move.
    byte(hash, 0x64);
    word(hash, u64::from(function.span.start));
    word(hash, u64::from(function.span.end));

    byte(hash, 0x62);
    *hash = hash_program_with_seed(
        &Program {
            version: BYTECODE_VERSION,
            source_origin: None,
            constants: Vec::new(),
            code: function.code.clone(),
            local_count: 0,
            functions: Vec::new(),
            template_sites: Vec::new(),
            debug_call_sites: Vec::new(),
            // The body's own bytes are hashed by the caller; this synthetic program exists only
            // to reach the canonical instruction encoder.
            source: Arc::from(""),
        },
        *hash,
    );

    byte(hash, 0x63);
    word(hash, function.upvalues.len() as u64);
    for upvalue in &function.upvalues {
        match upvalue {
            Upvalue::ParentLocal(index) | Upvalue::NamedParentLocal { slot: index, .. } => {
                byte(hash, 0);
                word(hash, u64::from(*index));
            }
            Upvalue::ParentUpvalue(index) | Upvalue::NamedParentUpvalue { index, .. } => {
                byte(hash, 1);
                word(hash, u64::from(*index));
            }
        }
    }
}

pub(super) fn hash_module_pair_with_policy(
    module: &ModuleProgram,
    policy: &EmissionPolicy,
) -> (u64, u64) {
    (
        hash_module_with_policy_and_seed(module, policy, 0xcbf29ce484222325),
        hash_module_with_policy_and_seed(module, policy, 0x84222325cbf29ce4),
    )
}

pub(super) fn hash_module_with_policy_and_seed(
    module: &ModuleProgram,
    policy: &EmissionPolicy,
    seed: u64,
) -> u64 {
    let mut hash = hash_module_with_seed(module, seed);
    if *policy == EmissionPolicy::default() {
        return hash;
    }
    let (entropy_tag, entropy_seed) = match policy.entropy {
        EmissionEntropyPolicy::Deterministic => (0, None),
        EmissionEntropyPolicy::Seeded(seed) => (1, Some(seed)),
        EmissionEntropyPolicy::Host => (2, None),
    };
    for value in [
        0x71,
        u8::from(policy.host_clock),
        entropy_tag,
        u8::from(policy.decoupled_prelude),
        u8::from(policy.compact),
        u8::from(policy.generic_only),
    ] {
        hash ^= u64::from(value);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    // Mixed in only when it changes the emission, so every existing key is unchanged.
    if policy.generic_only && policy.parallel_in_generic {
        hash ^= 0x7f;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    if policy.no_kiln_extensions {
        hash ^= 0x7e;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    if let Some(bytes) = policy.unit_chunk_bytes {
        hash ^= 0x7d;
        hash = hash.wrapping_mul(0x100000001b3);
        for value in (bytes as u64).to_le_bytes() {
            hash ^= u64::from(value);
            hash = hash.wrapping_mul(0x100000001b3);
        }
    }
    if let Some(seed) = entropy_seed {
        for value in seed.to_le_bytes() {
            hash ^= u64::from(value);
            hash = hash.wrapping_mul(0x100000001b3);
        }
    }
    hash_string_policy(&mut hash, &policy.timezone);
    hash_string_policy(&mut hash, &policy.locale);
    if policy.type_profile_record {
        hash ^= 0x72;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    if let Some(prof) = &policy.type_profile {
        hash ^= 0x73;
        hash = hash.wrapping_mul(0x100000001b3);
        if let Ok(json) = prof.to_json() {
            for b in json.as_bytes() {
                hash ^= u64::from(*b);
                hash = hash.wrapping_mul(0x100000001b3);
            }
        }
    }
    if let Some(slots) = &policy.live_module_slots {
        hash ^= 0x74;
        hash = hash.wrapping_mul(0x100000001b3);
        for slot in slots.iter() {
            for value in slot.to_le_bytes() {
                hash ^= u64::from(value);
                hash = hash.wrapping_mul(0x100000001b3);
            }
        }
    }
    hash
}

pub(super) fn hash_logical_path_pair(logical_path: &str) -> (u64, u64) {
    (
        hash_logical_path_with_seed(logical_path, 0xcbf29ce484222325),
        hash_logical_path_with_seed(logical_path, 0x84222325cbf29ce4),
    )
}

pub(super) fn hash_logical_path_with_seed(logical_path: &str, seed: u64) -> u64 {
    let mut hash = seed;
    for byte in b"kiln-logical-module-path\0" {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    for byte in (logical_path.len() as u64).to_le_bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    for byte in logical_path.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

pub(super) fn hash_module_with_seed(module: &ModuleProgram, seed: u64) -> u64 {
    let mut hash = hash_program_with_seed(&module.program, seed);
    fn byte(hash: &mut u64, value: u8) {
        *hash ^= u64::from(value);
        *hash = hash.wrapping_mul(0x100000001b3);
    }
    fn word(hash: &mut u64, value: u64) {
        for value in value.to_le_bytes() {
            byte(hash, value);
        }
    }
    fn text(hash: &mut u64, value: &str) {
        word(hash, value.len() as u64);
        for byte in value.bytes() {
            *hash ^= u64::from(byte);
            *hash = hash.wrapping_mul(0x100000001b3);
        }
    }

    // Tags and lengths make the linkage stream unambiguous even when names happen to share
    // prefixes.  Preserve the source order promised by ModuleProgram for all three lists.
    byte(&mut hash, 0x40);
    word(&mut hash, module.dependencies.len() as u64);
    for dependency in &module.dependencies {
        text(&mut hash, dependency);
    }
    byte(&mut hash, 0x41);
    word(&mut hash, module.imports.len() as u64);
    for binding in &module.imports {
        text(&mut hash, &binding.source);
        text(&mut hash, &binding.imported);
        text(&mut hash, &binding.local);
        word(&mut hash, u64::from(binding.local_slot));
    }
    byte(&mut hash, 0x42);
    word(&mut hash, module.exports.len() as u64);
    for binding in &module.exports {
        text(&mut hash, &binding.exported);
        word(&mut hash, u64::from(binding.local_slot));
    }
    // Appended last, after every byte that was already a persisted cache key -- the same rule
    // the source text follows in `hash_program_with_seed`. Strictness changes the emitted C: a
    // sloppy body's closure carries the bit that gives a bare call the global object as `this`.
    // Two modules with identical bytecode and different strictness are not interchangeable, and
    // a key without this would serve one's object file for the other. The padding matches the
    // emitter's: a table shorter than the function list reads strict.
    byte(&mut hash, 0x43);
    word(&mut hash, module.program.functions.len() as u64);
    for index in 0..module.program.functions.len() {
        byte(
            &mut hash,
            u8::from(
                module
                    .strictness
                    .functions
                    .get(index)
                    .copied()
                    .unwrap_or(true),
            ),
        );
    }
    byte(&mut hash, 0x44);
    word(
        &mut hash,
        u64::from(module.link_facts.module_instantiation_end),
    );
    // These facts select emitted evaluation paths and declaration elision. They are intentionally
    // stored outside Program bytecode, so append them explicitly to the persistent module identity.
    byte(&mut hash, 0x45);
    byte(&mut hash, u8::from(module.link_facts.direct_eval));
    byte(&mut hash, 0x46);
    word(
        &mut hash,
        module.link_facts.removable_declarations.len() as u64,
    );
    for declaration in &module.link_facts.removable_declarations {
        word(&mut hash, declaration.local_slots.len() as u64);
        for slot in &declaration.local_slots {
            word(&mut hash, u64::from(*slot));
        }
        word(&mut hash, u64::from(declaration.pc_start));
        word(&mut hash, u64::from(declaration.pc_end));
    }
    hash
}
