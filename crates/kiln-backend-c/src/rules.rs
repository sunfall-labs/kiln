//! Table-driven (ISLE-style) instruction fusion rules for `kiln-backend-c`.
//!
//! Rather than hand-rolling individual peephole matchers that test individual
//! patterns with cascading `if let` ladders, rewrites are represented as a unified
//! rule table of prioritized rewrite rules.
//!
//! Each rule defines:
//! 1. An instruction sequence pattern of length `len` (between 2 and 6 instructions).
//! 2. Predicates on operands, constants, and absence of internal jump targets.
//! 3. A resulting `FusedPattern` describing the fused operation and its parameters.

use std::collections::HashSet;

use kiln_ir::{BinaryOp, Constant, Instruction};

/// Relational operator symbol in C99, if the binary operator is a comparison.
pub fn relational_op_symbol(operator: BinaryOp) -> Option<&'static str> {
    match operator {
        BinaryOp::LessThan => Some("<"),
        BinaryOp::LessThanOrEqual => Some("<="),
        BinaryOp::GreaterThan => Some(">"),
        BinaryOp::GreaterThanOrEqual => Some(">="),
        BinaryOp::StrictEqual | BinaryOp::LooseEqual => Some("=="),
        BinaryOp::StrictNotEqual | BinaryOp::LooseNotEqual => Some("!="),
        _ => None,
    }
}

/// Whether a binary operator can be fused into an arithmetic/bitwise operation.
pub fn is_fusible_binary(operator: BinaryOp) -> bool {
    !matches!(operator, BinaryOp::In | BinaryOp::InstanceOf)
        && relational_op_symbol(operator).is_none()
}

/// Checks whether any instruction at an offset strictly between `1` and `len`
/// (exclusive of index 0) is targeted by a jump. If so, fusing this sequence
/// would skip a label that another path might jump into.
#[inline]
pub fn has_internal_jump_targets(index: usize, len: usize, jump_targets: &HashSet<u32>) -> bool {
    (1..len).any(|offset| jump_targets.contains(&((index + offset) as u32)))
}

/// The set of all 28 fused instruction patterns recognized by the emitter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FusedPattern {
    // -------------------------------------------------------------------------
    // Binary rules (length 2)
    // -------------------------------------------------------------------------
    /// `Binary(relational) + JumpIfFalse / JumpIfTruthy`
    CompareJump {
        op: BinaryOp,
        is_jump_if_false: bool,
        target: u32,
    },
    /// `Binary(fusible) + StoreLocal`
    BinaryStore { op: BinaryOp, slot: u32 },

    // -------------------------------------------------------------------------
    // Const rules (lengths 2..=4)
    // -------------------------------------------------------------------------
    /// `Const(Number) + LoadLocal + Binary(relational) + JumpIfFalse / JumpIfTruthy`
    ConstLoadCompareJump {
        bits: u64,
        slot: u32,
        op: BinaryOp,
        is_jump_if_false: bool,
        target: u32,
    },
    /// `Const(Number) + LoadLocal + Binary(fusible) + StoreLocal`
    ConstLoadBinaryStore {
        bits: u64,
        src: u32,
        op: BinaryOp,
        dst: u32,
    },
    /// `Const(Number) + LoadLocal + Binary(fusible)`
    ConstLoadBinary { bits: u64, src: u32, op: BinaryOp },
    /// `Const + StoreLocal`
    ConstStore { constant: u32, slot: u32 },

    // -------------------------------------------------------------------------
    // Call / CallMethod rules (lengths 2..=4)
    // -------------------------------------------------------------------------
    /// `Call/CallMethod + Dup + StoreLocal + Pop`
    CallDupStorePop { dst: u32 },
    /// `Call/CallMethod + StoreLocal`
    CallStore { dst: u32 },
    /// `Call/CallMethod + Pop`
    CallPop,

    // -------------------------------------------------------------------------
    // LoadLocal rules (lengths 2..=6)
    // -------------------------------------------------------------------------
    /// `LoadLocal + LoadLocal + GetComputed + Dup + StoreLocal + Pop`
    LoadLoadGetComputedDupStorePop { src1: u32, src2: u32, dst: u32 },
    /// `LoadLocal + LoadLocal + LoadLocal + SetComputed(Strict) + Pop`
    LoadLoadLoadSetComputedPop {
        obj: u32,
        key: u32,
        val: u32,
        strict: bool,
    },
    /// `LoadLocal + LoadLocal + Const(!BigInt) + SetComputed(Strict) + Pop`
    LoadLoadConstSetComputedPop {
        obj: u32,
        key: u32,
        c_idx: u32,
        strict: bool,
    },
    /// `LoadLocal + GetProperty + Dup + StoreLocal + Pop`
    LoadGetPropertyDupStorePop { src: u32, name: u32, dst: u32 },
    /// `LoadLocal + Const(Number) + Binary(relational) + JumpIfFalse / JumpIfTruthy`
    LoadConstCompareJump {
        slot: u32,
        bits: u64,
        op: BinaryOp,
        is_jump_if_false: bool,
        target: u32,
    },
    /// `LoadLocal + LoadLocal + Binary(relational) + JumpIfFalse / JumpIfTruthy`
    LoadLoadCompareJump {
        slot1: u32,
        slot2: u32,
        op: BinaryOp,
        is_jump_if_false: bool,
        target: u32,
    },
    /// `LoadLocal + Const(Number) + Binary(fusible) + StoreLocal`
    LoadConstBinaryStore {
        src: u32,
        bits: u64,
        op: BinaryOp,
        dst: u32,
    },
    /// `LoadLocal + LoadLocal + Binary(fusible) + StoreLocal`
    LoadLoadBinaryStore {
        src1: u32,
        src2: u32,
        op: BinaryOp,
        dst: u32,
    },
    /// `LoadLocal + LoadLocal + SetProperty(Strict) + Pop`
    LoadLoadSetPropertyPop {
        obj: u32,
        val: u32,
        name: u32,
        strict: bool,
    },
    /// `LoadLocal + Const(!BigInt) + SetProperty(Strict) + Pop`
    LoadConstSetPropertyPop {
        obj: u32,
        c_idx: u32,
        name: u32,
        strict: bool,
    },
    /// `LoadLocal + LoadLocal + GetComputed + StoreLocal`
    LoadLoadGetComputedStore { src1: u32, src2: u32, dst: u32 },
    /// `LoadLocal + LoadLocal + GetComputed + Pop`
    LoadLoadGetComputedPop { src1: u32, src2: u32 },
    /// `LoadLocal + GetProperty + StoreLocal`
    LoadGetPropertyStore { src: u32, name: u32, dst: u32 },
    /// `LoadLocal + GetProperty + Pop`
    LoadGetPropertyPop { src: u32, name: u32 },
    /// `LoadLocal + LoadLocal + GetComputed`
    LoadLoadGetComputed { src1: u32, src2: u32 },
    /// `LoadLocal + Const(Number) + Binary(fusible)`
    LoadConstBinary { src: u32, bits: u64, op: BinaryOp },
    /// `LoadLocal + LoadLocal + Binary(fusible)`
    LoadLoadBinary { src1: u32, src2: u32, op: BinaryOp },
    /// `LoadLocal + GetProperty`
    LoadGetProperty { src: u32, name: u32 },
    /// `LoadLocal + StoreLocal`
    LoadStore { src: u32, dst: u32 },
}

impl FusedPattern {
    /// Number of bytecode instructions consumed by this fused pattern.
    #[inline]
    pub fn len(&self) -> usize {
        match self {
            Self::CompareJump { .. }
            | Self::BinaryStore { .. }
            | Self::ConstStore { .. }
            | Self::CallStore { .. }
            | Self::CallPop
            | Self::LoadGetProperty { .. }
            | Self::LoadStore { .. } => 2,

            Self::ConstLoadBinary { .. }
            | Self::LoadGetPropertyStore { .. }
            | Self::LoadGetPropertyPop { .. }
            | Self::LoadLoadGetComputed { .. }
            | Self::LoadConstBinary { .. }
            | Self::LoadLoadBinary { .. } => 3,

            Self::ConstLoadCompareJump { .. }
            | Self::ConstLoadBinaryStore { .. }
            | Self::CallDupStorePop { .. }
            | Self::LoadConstCompareJump { .. }
            | Self::LoadLoadCompareJump { .. }
            | Self::LoadConstBinaryStore { .. }
            | Self::LoadLoadBinaryStore { .. }
            | Self::LoadLoadSetPropertyPop { .. }
            | Self::LoadConstSetPropertyPop { .. }
            | Self::LoadLoadGetComputedStore { .. }
            | Self::LoadLoadGetComputedPop { .. } => 4,

            Self::LoadLoadLoadSetComputedPop { .. }
            | Self::LoadLoadConstSetComputedPop { .. }
            | Self::LoadGetPropertyDupStorePop { .. } => 5,

            Self::LoadLoadGetComputedDupStorePop { .. } => 6,
        }
    }

    /// Whether this pattern is empty (always false since patterns have length >= 2).
    #[inline]
    pub fn is_empty(&self) -> bool {
        false
    }

    /// Whether this fused pattern represents an object property or element access
    /// (which can trigger traps / dynamic checks and therefore can refuse).
    #[inline]
    pub fn is_property_or_element_access(&self) -> bool {
        matches!(
            self,
            Self::LoadGetPropertyStore { .. }
                | Self::LoadGetPropertyDupStorePop { .. }
                | Self::LoadGetPropertyPop { .. }
                | Self::LoadGetProperty { .. }
                | Self::LoadLoadSetPropertyPop { .. }
                | Self::LoadConstSetPropertyPop { .. }
                | Self::LoadLoadGetComputedDupStorePop { .. }
                | Self::LoadLoadGetComputedStore { .. }
                | Self::LoadLoadGetComputedPop { .. }
                | Self::LoadLoadGetComputed { .. }
                | Self::LoadLoadLoadSetComputedPop { .. }
                | Self::LoadLoadConstSetComputedPop { .. }
        )
    }
}

/// Matcher function signature for a fusion rule.
pub type RuleMatcher =
    fn(&[Instruction], &[Constant], usize, &HashSet<u32>) -> Option<FusedPattern>;

/// A pattern matching rule in the rule table.
pub struct Rule {
    pub name: &'static str,
    pub match_fn: RuleMatcher,
}

// =============================================================================
// Individual Rule Matchers
// =============================================================================

fn rule_compare_jump(
    code: &[Instruction],
    _constants: &[Constant],
    index: usize,
    jump_targets: &HashSet<u32>,
) -> Option<FusedPattern> {
    if has_internal_jump_targets(index, 2, jump_targets) {
        return None;
    }
    let Instruction::Binary(op) = code.get(index)? else {
        return None;
    };
    relational_op_symbol(*op)?;
    match code.get(index + 1)? {
        Instruction::JumpIfFalse(target) => Some(FusedPattern::CompareJump {
            op: *op,
            is_jump_if_false: true,
            target: *target,
        }),
        Instruction::JumpIfTruthy(target) => Some(FusedPattern::CompareJump {
            op: *op,
            is_jump_if_false: false,
            target: *target,
        }),
        _ => None,
    }
}

fn rule_binary_store(
    code: &[Instruction],
    _constants: &[Constant],
    index: usize,
    jump_targets: &HashSet<u32>,
) -> Option<FusedPattern> {
    if has_internal_jump_targets(index, 2, jump_targets) {
        return None;
    }
    let Instruction::Binary(op) = code.get(index)? else {
        return None;
    };
    if !is_fusible_binary(*op) {
        return None;
    }
    match code.get(index + 1)? {
        Instruction::StoreLocal(slot) => Some(FusedPattern::BinaryStore {
            op: *op,
            slot: *slot,
        }),
        _ => None,
    }
}

fn rule_const_load_compare_jump(
    code: &[Instruction],
    constants: &[Constant],
    index: usize,
    jump_targets: &HashSet<u32>,
) -> Option<FusedPattern> {
    if has_internal_jump_targets(index, 4, jump_targets) {
        return None;
    }
    let Instruction::Const(c) = code.get(index)? else {
        return None;
    };
    let Constant::Number(bits) = constants.get(*c as usize)? else {
        return None;
    };
    let Instruction::LoadLocal(slot) = code.get(index + 1)? else {
        return None;
    };
    let Instruction::Binary(op) = code.get(index + 2)? else {
        return None;
    };
    relational_op_symbol(*op)?;
    match code.get(index + 3)? {
        Instruction::JumpIfFalse(target) => Some(FusedPattern::ConstLoadCompareJump {
            bits: *bits,
            slot: *slot,
            op: *op,
            is_jump_if_false: true,
            target: *target,
        }),
        Instruction::JumpIfTruthy(target) => Some(FusedPattern::ConstLoadCompareJump {
            bits: *bits,
            slot: *slot,
            op: *op,
            is_jump_if_false: false,
            target: *target,
        }),
        _ => None,
    }
}

fn rule_const_load_binary_store(
    code: &[Instruction],
    constants: &[Constant],
    index: usize,
    jump_targets: &HashSet<u32>,
) -> Option<FusedPattern> {
    if has_internal_jump_targets(index, 4, jump_targets) {
        return None;
    }
    let Instruction::Const(c) = code.get(index)? else {
        return None;
    };
    let Constant::Number(bits) = constants.get(*c as usize)? else {
        return None;
    };
    let Instruction::LoadLocal(src) = code.get(index + 1)? else {
        return None;
    };
    let Instruction::Binary(op) = code.get(index + 2)? else {
        return None;
    };
    if !is_fusible_binary(*op) {
        return None;
    }
    let Instruction::StoreLocal(dst) = code.get(index + 3)? else {
        return None;
    };
    Some(FusedPattern::ConstLoadBinaryStore {
        bits: *bits,
        src: *src,
        op: *op,
        dst: *dst,
    })
}

fn rule_const_load_binary(
    code: &[Instruction],
    constants: &[Constant],
    index: usize,
    jump_targets: &HashSet<u32>,
) -> Option<FusedPattern> {
    if has_internal_jump_targets(index, 3, jump_targets) {
        return None;
    }
    let Instruction::Const(c) = code.get(index)? else {
        return None;
    };
    let Constant::Number(bits) = constants.get(*c as usize)? else {
        return None;
    };
    let Instruction::LoadLocal(src) = code.get(index + 1)? else {
        return None;
    };
    let Instruction::Binary(op) = code.get(index + 2)? else {
        return None;
    };
    if !is_fusible_binary(*op) {
        return None;
    }
    Some(FusedPattern::ConstLoadBinary {
        bits: *bits,
        src: *src,
        op: *op,
    })
}

fn rule_const_store(
    code: &[Instruction],
    _constants: &[Constant],
    index: usize,
    jump_targets: &HashSet<u32>,
) -> Option<FusedPattern> {
    if has_internal_jump_targets(index, 2, jump_targets) {
        return None;
    }
    let Instruction::Const(c) = code.get(index)? else {
        return None;
    };
    match code.get(index + 1)? {
        Instruction::StoreLocal(slot) => Some(FusedPattern::ConstStore {
            constant: *c,
            slot: *slot,
        }),
        _ => None,
    }
}

fn rule_call_dup_store_pop(
    code: &[Instruction],
    _constants: &[Constant],
    index: usize,
    jump_targets: &HashSet<u32>,
) -> Option<FusedPattern> {
    if has_internal_jump_targets(index, 4, jump_targets) {
        return None;
    }
    if !matches!(
        code.get(index)?,
        Instruction::Call { .. } | Instruction::CallMethod { .. }
    ) {
        return None;
    }
    if !matches!(code.get(index + 1)?, Instruction::Dup) {
        return None;
    }
    let Instruction::StoreLocal(dst) = code.get(index + 2)? else {
        return None;
    };
    if !matches!(code.get(index + 3)?, Instruction::Pop) {
        return None;
    }
    Some(FusedPattern::CallDupStorePop { dst: *dst })
}

fn rule_call_store(
    code: &[Instruction],
    _constants: &[Constant],
    index: usize,
    jump_targets: &HashSet<u32>,
) -> Option<FusedPattern> {
    if has_internal_jump_targets(index, 2, jump_targets) {
        return None;
    }
    if !matches!(
        code.get(index)?,
        Instruction::Call { .. } | Instruction::CallMethod { .. }
    ) {
        return None;
    }
    match code.get(index + 1)? {
        Instruction::StoreLocal(dst) => Some(FusedPattern::CallStore { dst: *dst }),
        _ => None,
    }
}

fn rule_call_pop(
    code: &[Instruction],
    _constants: &[Constant],
    index: usize,
    jump_targets: &HashSet<u32>,
) -> Option<FusedPattern> {
    if has_internal_jump_targets(index, 2, jump_targets) {
        return None;
    }
    if !matches!(
        code.get(index)?,
        Instruction::Call { .. } | Instruction::CallMethod { .. }
    ) {
        return None;
    }
    match code.get(index + 1)? {
        Instruction::Pop => Some(FusedPattern::CallPop),
        _ => None,
    }
}

fn rule_load_load_get_computed_dup_store_pop(
    code: &[Instruction],
    _constants: &[Constant],
    index: usize,
    jump_targets: &HashSet<u32>,
) -> Option<FusedPattern> {
    if has_internal_jump_targets(index, 6, jump_targets) {
        return None;
    }
    let Instruction::LoadLocal(src1) = code.get(index)? else {
        return None;
    };
    let Instruction::LoadLocal(src2) = code.get(index + 1)? else {
        return None;
    };
    if !matches!(code.get(index + 2)?, Instruction::GetComputed) {
        return None;
    }
    if !matches!(code.get(index + 3)?, Instruction::Dup) {
        return None;
    }
    let Instruction::StoreLocal(dst) = code.get(index + 4)? else {
        return None;
    };
    if !matches!(code.get(index + 5)?, Instruction::Pop) {
        return None;
    }
    Some(FusedPattern::LoadLoadGetComputedDupStorePop {
        src1: *src1,
        src2: *src2,
        dst: *dst,
    })
}

fn rule_load_load_load_set_computed_pop(
    code: &[Instruction],
    _constants: &[Constant],
    index: usize,
    jump_targets: &HashSet<u32>,
) -> Option<FusedPattern> {
    if has_internal_jump_targets(index, 5, jump_targets) {
        return None;
    }
    let Instruction::LoadLocal(obj) = code.get(index)? else {
        return None;
    };
    let Instruction::LoadLocal(key) = code.get(index + 1)? else {
        return None;
    };
    let Instruction::LoadLocal(val) = code.get(index + 2)? else {
        return None;
    };
    let strict = match code.get(index + 3)? {
        Instruction::SetComputedStrict { strict } => *strict,
        Instruction::SetComputed => false,
        _ => return None,
    };
    if !matches!(code.get(index + 4)?, Instruction::Pop) {
        return None;
    }
    Some(FusedPattern::LoadLoadLoadSetComputedPop {
        obj: *obj,
        key: *key,
        val: *val,
        strict,
    })
}

fn rule_load_load_const_set_computed_pop(
    code: &[Instruction],
    constants: &[Constant],
    index: usize,
    jump_targets: &HashSet<u32>,
) -> Option<FusedPattern> {
    if has_internal_jump_targets(index, 5, jump_targets) {
        return None;
    }
    let Instruction::LoadLocal(obj) = code.get(index)? else {
        return None;
    };
    let Instruction::LoadLocal(key) = code.get(index + 1)? else {
        return None;
    };
    let Instruction::Const(c_idx) = code.get(index + 2)? else {
        return None;
    };
    if matches!(constants.get(*c_idx as usize), Some(Constant::BigInt(_))) {
        return None;
    }
    let strict = match code.get(index + 3)? {
        Instruction::SetComputedStrict { strict } => *strict,
        Instruction::SetComputed => false,
        _ => return None,
    };
    if !matches!(code.get(index + 4)?, Instruction::Pop) {
        return None;
    }
    Some(FusedPattern::LoadLoadConstSetComputedPop {
        obj: *obj,
        key: *key,
        c_idx: *c_idx,
        strict,
    })
}

fn rule_load_get_property_dup_store_pop(
    code: &[Instruction],
    _constants: &[Constant],
    index: usize,
    jump_targets: &HashSet<u32>,
) -> Option<FusedPattern> {
    if has_internal_jump_targets(index, 5, jump_targets) {
        return None;
    }
    let Instruction::LoadLocal(src) = code.get(index)? else {
        return None;
    };
    let Instruction::GetProperty(name) = code.get(index + 1)? else {
        return None;
    };
    if !matches!(code.get(index + 2)?, Instruction::Dup) {
        return None;
    }
    let Instruction::StoreLocal(dst) = code.get(index + 3)? else {
        return None;
    };
    if !matches!(code.get(index + 4)?, Instruction::Pop) {
        return None;
    }
    Some(FusedPattern::LoadGetPropertyDupStorePop {
        src: *src,
        name: *name,
        dst: *dst,
    })
}

fn rule_load_const_compare_jump(
    code: &[Instruction],
    constants: &[Constant],
    index: usize,
    jump_targets: &HashSet<u32>,
) -> Option<FusedPattern> {
    if has_internal_jump_targets(index, 4, jump_targets) {
        return None;
    }
    let Instruction::LoadLocal(slot) = code.get(index)? else {
        return None;
    };
    let Instruction::Const(c) = code.get(index + 1)? else {
        return None;
    };
    let Constant::Number(bits) = constants.get(*c as usize)? else {
        return None;
    };
    let Instruction::Binary(op) = code.get(index + 2)? else {
        return None;
    };
    relational_op_symbol(*op)?;
    match code.get(index + 3)? {
        Instruction::JumpIfFalse(target) => Some(FusedPattern::LoadConstCompareJump {
            slot: *slot,
            bits: *bits,
            op: *op,
            is_jump_if_false: true,
            target: *target,
        }),
        Instruction::JumpIfTruthy(target) => Some(FusedPattern::LoadConstCompareJump {
            slot: *slot,
            bits: *bits,
            op: *op,
            is_jump_if_false: false,
            target: *target,
        }),
        _ => None,
    }
}

fn rule_load_load_compare_jump(
    code: &[Instruction],
    _constants: &[Constant],
    index: usize,
    jump_targets: &HashSet<u32>,
) -> Option<FusedPattern> {
    if has_internal_jump_targets(index, 4, jump_targets) {
        return None;
    }
    let Instruction::LoadLocal(slot1) = code.get(index)? else {
        return None;
    };
    let Instruction::LoadLocal(slot2) = code.get(index + 1)? else {
        return None;
    };
    let Instruction::Binary(op) = code.get(index + 2)? else {
        return None;
    };
    relational_op_symbol(*op)?;
    match code.get(index + 3)? {
        Instruction::JumpIfFalse(target) => Some(FusedPattern::LoadLoadCompareJump {
            slot1: *slot1,
            slot2: *slot2,
            op: *op,
            is_jump_if_false: true,
            target: *target,
        }),
        Instruction::JumpIfTruthy(target) => Some(FusedPattern::LoadLoadCompareJump {
            slot1: *slot1,
            slot2: *slot2,
            op: *op,
            is_jump_if_false: false,
            target: *target,
        }),
        _ => None,
    }
}

fn rule_load_const_binary_store(
    code: &[Instruction],
    constants: &[Constant],
    index: usize,
    jump_targets: &HashSet<u32>,
) -> Option<FusedPattern> {
    if has_internal_jump_targets(index, 4, jump_targets) {
        return None;
    }
    let Instruction::LoadLocal(src) = code.get(index)? else {
        return None;
    };
    let Instruction::Const(c) = code.get(index + 1)? else {
        return None;
    };
    let Constant::Number(bits) = constants.get(*c as usize)? else {
        return None;
    };
    let Instruction::Binary(op) = code.get(index + 2)? else {
        return None;
    };
    if !is_fusible_binary(*op) {
        return None;
    }
    let Instruction::StoreLocal(dst) = code.get(index + 3)? else {
        return None;
    };
    Some(FusedPattern::LoadConstBinaryStore {
        src: *src,
        bits: *bits,
        op: *op,
        dst: *dst,
    })
}

fn rule_load_load_binary_store(
    code: &[Instruction],
    _constants: &[Constant],
    index: usize,
    jump_targets: &HashSet<u32>,
) -> Option<FusedPattern> {
    if has_internal_jump_targets(index, 4, jump_targets) {
        return None;
    }
    let Instruction::LoadLocal(src1) = code.get(index)? else {
        return None;
    };
    let Instruction::LoadLocal(src2) = code.get(index + 1)? else {
        return None;
    };
    let Instruction::Binary(op) = code.get(index + 2)? else {
        return None;
    };
    if !is_fusible_binary(*op) {
        return None;
    }
    let Instruction::StoreLocal(dst) = code.get(index + 3)? else {
        return None;
    };
    Some(FusedPattern::LoadLoadBinaryStore {
        src1: *src1,
        src2: *src2,
        op: *op,
        dst: *dst,
    })
}

fn rule_load_load_set_property_pop(
    code: &[Instruction],
    _constants: &[Constant],
    index: usize,
    jump_targets: &HashSet<u32>,
) -> Option<FusedPattern> {
    if has_internal_jump_targets(index, 4, jump_targets) {
        return None;
    }
    let Instruction::LoadLocal(obj) = code.get(index)? else {
        return None;
    };
    let Instruction::LoadLocal(val) = code.get(index + 1)? else {
        return None;
    };
    let (name, strict) = match code.get(index + 2)? {
        Instruction::SetPropertyStrict { name, strict } => (*name, *strict),
        Instruction::SetProperty(name) => (*name, false),
        _ => return None,
    };
    if !matches!(code.get(index + 3)?, Instruction::Pop) {
        return None;
    }
    Some(FusedPattern::LoadLoadSetPropertyPop {
        obj: *obj,
        val: *val,
        name,
        strict,
    })
}

fn rule_load_const_set_property_pop(
    code: &[Instruction],
    constants: &[Constant],
    index: usize,
    jump_targets: &HashSet<u32>,
) -> Option<FusedPattern> {
    if has_internal_jump_targets(index, 4, jump_targets) {
        return None;
    }
    let Instruction::LoadLocal(obj) = code.get(index)? else {
        return None;
    };
    let Instruction::Const(c_idx) = code.get(index + 1)? else {
        return None;
    };
    if matches!(constants.get(*c_idx as usize), Some(Constant::BigInt(_))) {
        return None;
    }
    let (name, strict) = match code.get(index + 2)? {
        Instruction::SetPropertyStrict { name, strict } => (*name, *strict),
        Instruction::SetProperty(name) => (*name, false),
        _ => return None,
    };
    if !matches!(code.get(index + 3)?, Instruction::Pop) {
        return None;
    }
    Some(FusedPattern::LoadConstSetPropertyPop {
        obj: *obj,
        c_idx: *c_idx,
        name,
        strict,
    })
}

fn rule_load_load_get_computed_store(
    code: &[Instruction],
    _constants: &[Constant],
    index: usize,
    jump_targets: &HashSet<u32>,
) -> Option<FusedPattern> {
    if has_internal_jump_targets(index, 4, jump_targets) {
        return None;
    }
    let Instruction::LoadLocal(src1) = code.get(index)? else {
        return None;
    };
    let Instruction::LoadLocal(src2) = code.get(index + 1)? else {
        return None;
    };
    if !matches!(code.get(index + 2)?, Instruction::GetComputed) {
        return None;
    }
    let Instruction::StoreLocal(dst) = code.get(index + 3)? else {
        return None;
    };
    Some(FusedPattern::LoadLoadGetComputedStore {
        src1: *src1,
        src2: *src2,
        dst: *dst,
    })
}

fn rule_load_load_get_computed_pop(
    code: &[Instruction],
    _constants: &[Constant],
    index: usize,
    jump_targets: &HashSet<u32>,
) -> Option<FusedPattern> {
    if has_internal_jump_targets(index, 4, jump_targets) {
        return None;
    }
    let Instruction::LoadLocal(src1) = code.get(index)? else {
        return None;
    };
    let Instruction::LoadLocal(src2) = code.get(index + 1)? else {
        return None;
    };
    if !matches!(code.get(index + 2)?, Instruction::GetComputed) {
        return None;
    }
    if !matches!(code.get(index + 3)?, Instruction::Pop) {
        return None;
    }
    Some(FusedPattern::LoadLoadGetComputedPop {
        src1: *src1,
        src2: *src2,
    })
}

fn rule_load_get_property_store(
    code: &[Instruction],
    _constants: &[Constant],
    index: usize,
    jump_targets: &HashSet<u32>,
) -> Option<FusedPattern> {
    if has_internal_jump_targets(index, 3, jump_targets) {
        return None;
    }
    let Instruction::LoadLocal(src) = code.get(index)? else {
        return None;
    };
    let Instruction::GetProperty(name) = code.get(index + 1)? else {
        return None;
    };
    let Instruction::StoreLocal(dst) = code.get(index + 2)? else {
        return None;
    };
    Some(FusedPattern::LoadGetPropertyStore {
        src: *src,
        name: *name,
        dst: *dst,
    })
}

fn rule_load_get_property_pop(
    code: &[Instruction],
    _constants: &[Constant],
    index: usize,
    jump_targets: &HashSet<u32>,
) -> Option<FusedPattern> {
    if has_internal_jump_targets(index, 3, jump_targets) {
        return None;
    }
    let Instruction::LoadLocal(src) = code.get(index)? else {
        return None;
    };
    let Instruction::GetProperty(name) = code.get(index + 1)? else {
        return None;
    };
    if !matches!(code.get(index + 2)?, Instruction::Pop) {
        return None;
    }
    Some(FusedPattern::LoadGetPropertyPop {
        src: *src,
        name: *name,
    })
}

fn rule_load_load_get_computed(
    code: &[Instruction],
    _constants: &[Constant],
    index: usize,
    jump_targets: &HashSet<u32>,
) -> Option<FusedPattern> {
    if has_internal_jump_targets(index, 3, jump_targets) {
        return None;
    }
    let Instruction::LoadLocal(src1) = code.get(index)? else {
        return None;
    };
    let Instruction::LoadLocal(src2) = code.get(index + 1)? else {
        return None;
    };
    if !matches!(code.get(index + 2)?, Instruction::GetComputed) {
        return None;
    }
    Some(FusedPattern::LoadLoadGetComputed {
        src1: *src1,
        src2: *src2,
    })
}

fn rule_load_const_binary(
    code: &[Instruction],
    constants: &[Constant],
    index: usize,
    jump_targets: &HashSet<u32>,
) -> Option<FusedPattern> {
    if has_internal_jump_targets(index, 3, jump_targets) {
        return None;
    }
    let Instruction::LoadLocal(src) = code.get(index)? else {
        return None;
    };
    let Instruction::Const(c) = code.get(index + 1)? else {
        return None;
    };
    let Constant::Number(bits) = constants.get(*c as usize)? else {
        return None;
    };
    let Instruction::Binary(op) = code.get(index + 2)? else {
        return None;
    };
    if !is_fusible_binary(*op) {
        return None;
    }
    Some(FusedPattern::LoadConstBinary {
        src: *src,
        bits: *bits,
        op: *op,
    })
}

fn rule_load_load_binary(
    code: &[Instruction],
    _constants: &[Constant],
    index: usize,
    jump_targets: &HashSet<u32>,
) -> Option<FusedPattern> {
    if has_internal_jump_targets(index, 3, jump_targets) {
        return None;
    }
    let Instruction::LoadLocal(src1) = code.get(index)? else {
        return None;
    };
    let Instruction::LoadLocal(src2) = code.get(index + 1)? else {
        return None;
    };
    let Instruction::Binary(op) = code.get(index + 2)? else {
        return None;
    };
    if !is_fusible_binary(*op) {
        return None;
    }
    Some(FusedPattern::LoadLoadBinary {
        src1: *src1,
        src2: *src2,
        op: *op,
    })
}

fn rule_load_get_property(
    code: &[Instruction],
    _constants: &[Constant],
    index: usize,
    jump_targets: &HashSet<u32>,
) -> Option<FusedPattern> {
    if has_internal_jump_targets(index, 2, jump_targets) {
        return None;
    }
    let Instruction::LoadLocal(src) = code.get(index)? else {
        return None;
    };
    let Instruction::GetProperty(name) = code.get(index + 1)? else {
        return None;
    };
    Some(FusedPattern::LoadGetProperty {
        src: *src,
        name: *name,
    })
}

fn rule_load_store(
    code: &[Instruction],
    _constants: &[Constant],
    index: usize,
    jump_targets: &HashSet<u32>,
) -> Option<FusedPattern> {
    if has_internal_jump_targets(index, 2, jump_targets) {
        return None;
    }
    let Instruction::LoadLocal(src) = code.get(index)? else {
        return None;
    };
    match code.get(index + 1)? {
        Instruction::StoreLocal(dst) => Some(FusedPattern::LoadStore {
            src: *src,
            dst: *dst,
        }),
        _ => None,
    }
}

// =============================================================================
// Rule Tables (ISLE-style prioritized tables)
// =============================================================================

/// Rules matching sequences beginning with `Instruction::Binary`.
pub const BINARY_RULES: &[Rule] = &[
    Rule {
        name: "compare_jump",
        match_fn: rule_compare_jump,
    },
    Rule {
        name: "binary_store",
        match_fn: rule_binary_store,
    },
];

/// Rules matching sequences beginning with `Instruction::Const`.
/// Prioritized from longest pattern to shortest.
pub const CONST_RULES: &[Rule] = &[
    Rule {
        name: "const_load_compare_jump",
        match_fn: rule_const_load_compare_jump,
    },
    Rule {
        name: "const_load_binary_store",
        match_fn: rule_const_load_binary_store,
    },
    Rule {
        name: "const_load_binary",
        match_fn: rule_const_load_binary,
    },
    Rule {
        name: "const_store",
        match_fn: rule_const_store,
    },
];

/// Rules matching sequences beginning with `Instruction::Call` or `Instruction::CallMethod`.
/// Prioritized from longest pattern to shortest.
pub const CALL_RULES: &[Rule] = &[
    Rule {
        name: "call_dup_store_pop",
        match_fn: rule_call_dup_store_pop,
    },
    Rule {
        name: "call_store",
        match_fn: rule_call_store,
    },
    Rule {
        name: "call_pop",
        match_fn: rule_call_pop,
    },
];

/// Rules matching sequences beginning with `Instruction::LoadLocal`.
/// Prioritized from longest pattern to shortest.
pub const LOAD_LOCAL_RULES: &[Rule] = &[
    // Length 6
    Rule {
        name: "load_load_get_computed_dup_store_pop",
        match_fn: rule_load_load_get_computed_dup_store_pop,
    },
    // Length 5
    Rule {
        name: "load_load_load_set_computed_pop",
        match_fn: rule_load_load_load_set_computed_pop,
    },
    Rule {
        name: "load_load_const_set_computed_pop",
        match_fn: rule_load_load_const_set_computed_pop,
    },
    Rule {
        name: "load_get_property_dup_store_pop",
        match_fn: rule_load_get_property_dup_store_pop,
    },
    // Length 4
    Rule {
        name: "load_const_compare_jump",
        match_fn: rule_load_const_compare_jump,
    },
    Rule {
        name: "load_load_compare_jump",
        match_fn: rule_load_load_compare_jump,
    },
    Rule {
        name: "load_const_binary_store",
        match_fn: rule_load_const_binary_store,
    },
    Rule {
        name: "load_load_binary_store",
        match_fn: rule_load_load_binary_store,
    },
    Rule {
        name: "load_load_set_property_pop",
        match_fn: rule_load_load_set_property_pop,
    },
    Rule {
        name: "load_const_set_property_pop",
        match_fn: rule_load_const_set_property_pop,
    },
    Rule {
        name: "load_load_get_computed_store",
        match_fn: rule_load_load_get_computed_store,
    },
    Rule {
        name: "load_load_get_computed_pop",
        match_fn: rule_load_load_get_computed_pop,
    },
    // Length 3
    Rule {
        name: "load_get_property_store",
        match_fn: rule_load_get_property_store,
    },
    Rule {
        name: "load_get_property_pop",
        match_fn: rule_load_get_property_pop,
    },
    Rule {
        name: "load_load_get_computed",
        match_fn: rule_load_load_get_computed,
    },
    Rule {
        name: "load_const_binary",
        match_fn: rule_load_const_binary,
    },
    Rule {
        name: "load_load_binary",
        match_fn: rule_load_load_binary,
    },
    // Length 2
    Rule {
        name: "load_get_property",
        match_fn: rule_load_get_property,
    },
    Rule {
        name: "load_store",
        match_fn: rule_load_store,
    },
];

/// The complete unified rule table containing all 28 fusion rules.
pub const ALL_RULES: &[Rule] = &[
    // Binary rules (2)
    Rule {
        name: "compare_jump",
        match_fn: rule_compare_jump,
    },
    Rule {
        name: "binary_store",
        match_fn: rule_binary_store,
    },
    // Const rules (4)
    Rule {
        name: "const_load_compare_jump",
        match_fn: rule_const_load_compare_jump,
    },
    Rule {
        name: "const_load_binary_store",
        match_fn: rule_const_load_binary_store,
    },
    Rule {
        name: "const_load_binary",
        match_fn: rule_const_load_binary,
    },
    Rule {
        name: "const_store",
        match_fn: rule_const_store,
    },
    // Call rules (3)
    Rule {
        name: "call_dup_store_pop",
        match_fn: rule_call_dup_store_pop,
    },
    Rule {
        name: "call_store",
        match_fn: rule_call_store,
    },
    Rule {
        name: "call_pop",
        match_fn: rule_call_pop,
    },
    // LoadLocal rules (19)
    Rule {
        name: "load_load_get_computed_dup_store_pop",
        match_fn: rule_load_load_get_computed_dup_store_pop,
    },
    Rule {
        name: "load_load_load_set_computed_pop",
        match_fn: rule_load_load_load_set_computed_pop,
    },
    Rule {
        name: "load_load_const_set_computed_pop",
        match_fn: rule_load_load_const_set_computed_pop,
    },
    Rule {
        name: "load_get_property_dup_store_pop",
        match_fn: rule_load_get_property_dup_store_pop,
    },
    Rule {
        name: "load_const_compare_jump",
        match_fn: rule_load_const_compare_jump,
    },
    Rule {
        name: "load_load_compare_jump",
        match_fn: rule_load_load_compare_jump,
    },
    Rule {
        name: "load_const_binary_store",
        match_fn: rule_load_const_binary_store,
    },
    Rule {
        name: "load_load_binary_store",
        match_fn: rule_load_load_binary_store,
    },
    Rule {
        name: "load_load_set_property_pop",
        match_fn: rule_load_load_set_property_pop,
    },
    Rule {
        name: "load_const_set_property_pop",
        match_fn: rule_load_const_set_property_pop,
    },
    Rule {
        name: "load_load_get_computed_store",
        match_fn: rule_load_load_get_computed_store,
    },
    Rule {
        name: "load_load_get_computed_pop",
        match_fn: rule_load_load_get_computed_pop,
    },
    Rule {
        name: "load_get_property_store",
        match_fn: rule_load_get_property_store,
    },
    Rule {
        name: "load_get_property_pop",
        match_fn: rule_load_get_property_pop,
    },
    Rule {
        name: "load_load_get_computed",
        match_fn: rule_load_load_get_computed,
    },
    Rule {
        name: "load_const_binary",
        match_fn: rule_load_const_binary,
    },
    Rule {
        name: "load_load_binary",
        match_fn: rule_load_load_binary,
    },
    Rule {
        name: "load_get_property",
        match_fn: rule_load_get_property,
    },
    Rule {
        name: "load_store",
        match_fn: rule_load_store,
    },
];

/// Matches the instruction sequence starting at `index` against the rule table.
/// Returns the first matching `FusedPattern` according to rule priority.
pub fn match_rule(
    code: &[Instruction],
    constants: &[Constant],
    index: usize,
    jump_targets: &HashSet<u32>,
) -> Option<FusedPattern> {
    let rules: &[Rule] = match code.get(index)? {
        Instruction::LoadLocal(_) => LOAD_LOCAL_RULES,
        Instruction::Const(_) => CONST_RULES,
        Instruction::Call { .. } | Instruction::CallMethod { .. } => CALL_RULES,
        Instruction::Binary(_) => BINARY_RULES,
        _ => return None,
    };
    for rule in rules {
        if let Some(pattern) = (rule.match_fn)(code, constants, index, jump_targets) {
            return Some(pattern);
        }
    }
    None
}

/// Checks whether the instruction at `index` was already consumed by a fused pattern
/// starting at an earlier index (`index - 1` through `index - 5`).
pub fn is_fused_with_previous(
    code: &[Instruction],
    constants: &[Constant],
    index: usize,
    jump_targets: &HashSet<u32>,
) -> bool {
    if index == 0 {
        return false;
    }
    for offset in 1..=5 {
        if index >= offset {
            let start = index - offset;
            if match_rule(code, constants, start, jump_targets)
                .is_some_and(|pattern| pattern.len() > offset)
            {
                return true;
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rule_table_covers_all_28_patterns() {
        assert_eq!(ALL_RULES.len(), 28);
        assert_eq!(
            BINARY_RULES.len() + CONST_RULES.len() + CALL_RULES.len() + LOAD_LOCAL_RULES.len(),
            28
        );
    }

    #[test]
    fn jump_target_prevents_fusion() {
        let code = vec![
            Instruction::LoadLocal(0),
            Instruction::GetProperty(0),
            Instruction::StoreLocal(1),
        ];
        let constants = vec![];
        let mut jump_targets = HashSet::new();
        // Without jump targets, matches LoadGetPropertyStore
        assert_eq!(
            match_rule(&code, &constants, 0, &jump_targets),
            Some(FusedPattern::LoadGetPropertyStore {
                src: 0,
                name: 0,
                dst: 1
            })
        );

        // With jump target at 1, cannot fuse
        jump_targets.insert(1);
        assert_eq!(match_rule(&code, &constants, 0, &jump_targets), None);
    }
}
