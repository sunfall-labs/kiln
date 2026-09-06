//! Structured generation of `kiln-ir` bytecode.
//!
//! WHY NOT RANDOM BYTES. `Program` is a `Vec<Instruction>` over a constant table, and almost
//! every random value of that type is malformed in the first way anyone would check: an index
//! past the end of a table, a jump past the end of the code, a `Binary` with an empty stack.
//! Both consumers reject those immediately, so a byte fuzzer measures how fast two validators
//! can say no. What is worth generating is bytecode that is *well-formed*, because that is the
//! only kind either engine will ever be handed in production, and then to perturb it by exactly
//! one field at a time to find the places the two disagree about where well-formedness ends.
//!
//! So this module is a tiny compiler. [`IrProgram`] is a structured, `arbitrary`-derived tree;
//! [`IrProgram::build`] lowers it to bytecode that is well-formed by construction -- every
//! constant index is in the table, every local index is below `local_count`, every jump lands on
//! an instruction boundary, and the operand stack is balanced at every merge point -- and then
//! applies the generated [`Perturbation`]s, each of which changes exactly one field.
//!
//! THREE PROPERTIES THE CONSTRUCTION GUARANTEES, and the reason for each:
//!
//! 1. **No backward jumps, ever, including after perturbation.** Not for tidiness: a loop plus
//!    the operand stack is a doubling machine. `dup; binary(add)` inside one multiplies a string
//!    by two per iteration, and a step limit does not save a process from `2^n` bytes -- it runs
//!    out of memory long before it runs out of steps.
//! 2. **A hard instruction budget** ([`MAX_INSTRUCTIONS`]). Straight-line code can double too;
//!    thirty `dup; add` pairs is a gigabyte. Bounding the emitted code bounds the doublings,
//!    because with no back edge nothing executes more than once.
//! 3. **No recursion.** Function bodies cannot make or call functions, so a call graph is one
//!    level deep and the interpreter's call-depth limit is never the thing that stops a program.
//!
//! Those three are why this target can be run for millions of iterations unattended, which is
//! the whole difference between it and the native differential target.

use arbitrary::Arbitrary;

use crate::{Leafy, Many, Nested};

/// How deep a generated expression may nest. Enforced by [`Nested`] at generation time, so the
/// entropy spent on a subtree is entropy that reaches the bytecode.
type Sub = Nested<Expr, 3>;
use kiln_ir::{BinaryOp, Builtin, Constant, FunctionProto, HostOp, Instruction, Program, UnaryOp};

/// Instructions one body may contain. See property 2 above before raising it.
const MAX_INSTRUCTIONS: usize = 32;
/// Local slots every body declares.
///
/// Split in two halves that never alias, because lowering never lets them: a slot is either an
/// ordinary local, read with `LoadLocal`, or a cell created with `NewCell` and read with
/// `LoadCell`, and the compiler decides which per binding before it emits anything. A generator
/// that mixed the two would spend its whole run on one known finding -- see "A cell read as a
/// local" in `fuzz/README.md` -- instead of looking for the ones nobody has seen.
const LOCAL_COUNT: u32 = 4;
/// Slots reached with `LoadLocal`/`StoreLocal`.
const PLAIN_LOCALS: u32 = 2;
/// The first slot reached with `NewCell`/`LoadCell`/`StoreCell`.
const FIRST_CELL: u32 = PLAIN_LOCALS;
const CELL_SLOTS: u32 = LOCAL_COUNT - PLAIN_LOCALS;

const fn plain_slot(slot: u8) -> u32 {
    slot as u32 % PLAIN_LOCALS
}

const fn cell_slot(slot: u8) -> u32 {
    FIRST_CELL + slot as u32 % CELL_SLOTS
}
/// Function protos a program may contain.
const MAX_FUNCTIONS: usize = 2;

const NUMBERS: [f64; 8] = [
    0.0,
    -0.0,
    1.0,
    -1.0,
    0.5,
    f64::NAN,
    f64::INFINITY,
    9_007_199_254_740_993.0,
];
const STRINGS: [&str; 6] = ["", "x", "0", "p0", "length", "😀"];

#[derive(Debug, Clone, Arbitrary)]
pub enum ConstSpec {
    Undefined,
    Null,
    Bool(bool),
    Number(u8),
    Str(u8),
    BigInt(u8),
}

#[derive(Debug, Clone, Copy, Arbitrary)]
pub enum Un {
    Plus,
    Negate,
    Not,
    TypeOf,
    BitwiseNot,
    Void,
}

impl Un {
    const fn op(self) -> UnaryOp {
        match self {
            Self::Plus => UnaryOp::Plus,
            Self::Negate => UnaryOp::Negate,
            Self::Not => UnaryOp::Not,
            Self::TypeOf => UnaryOp::TypeOf,
            Self::BitwiseNot => UnaryOp::BitwiseNot,
            Self::Void => UnaryOp::Void,
        }
    }
}

#[derive(Debug, Clone, Copy, Arbitrary)]
pub enum Bin {
    Add,
    Subtract,
    Multiply,
    Divide,
    Remainder,
    Exponentiate,
    StrictEqual,
    LooseEqual,
    BitwiseAnd,
    ShiftLeft,
    UnsignedShiftRight,
    LessThan,
    GreaterThanOrEqual,
    In,
    InstanceOf,
}

impl Bin {
    const fn op(self) -> BinaryOp {
        match self {
            Self::Add => BinaryOp::Add,
            Self::Subtract => BinaryOp::Subtract,
            Self::Multiply => BinaryOp::Multiply,
            Self::Divide => BinaryOp::Divide,
            Self::Remainder => BinaryOp::Remainder,
            Self::Exponentiate => BinaryOp::Exponentiate,
            Self::StrictEqual => BinaryOp::StrictEqual,
            Self::LooseEqual => BinaryOp::LooseEqual,
            Self::BitwiseAnd => BinaryOp::BitwiseAnd,
            Self::ShiftLeft => BinaryOp::ShiftLeft,
            Self::UnsignedShiftRight => BinaryOp::UnsignedShiftRight,
            Self::LessThan => BinaryOp::LessThan,
            Self::GreaterThanOrEqual => BinaryOp::GreaterThanOrEqual,
            Self::In => BinaryOp::In,
            Self::InstanceOf => BinaryOp::InstanceOf,
        }
    }
}

/// Every expression form pushes exactly one value and pops exactly what it named.
///
/// That invariant is the whole reason the generated code is stack-balanced without a verifier:
/// a statement is an expression plus a `Pop` or a `StoreLocal`, so the stack is empty at every
/// statement boundary, and therefore at every jump target too.
#[derive(Debug, Clone, Arbitrary)]
pub enum Expr {
    Const(ConstSpec),
    Local(u8),
    Cell(u8),
    Unary(Un, Sub),
    Binary(Bin, Sub, Sub),
    Array(Many<Sub, 4>),
    /// A hole, which is not `undefined` -- `in` and `Object.keys` can tell them apart.
    Hole,
    Object(Many<(u8, Sub), 3>),
    GetProperty(Sub, u8),
    GetComputed(Sub, Sub),
    SetProperty(Sub, u8, Sub),
    DeleteProperty(Sub, u8),
    Builtin(u8),
    /// `Program::functions[index % len]`, or a constant if the program declared none.
    MakeFunction(u8),
    Call(u8, Many<Sub, 2>),
    New(u8, Many<Sub, 2>),
    CallMethod(Sub, u8, Many<Sub, 2>),
    EnumerateKeys(Sub),
    IterableToArray(Sub),
    SpreadInto(Sub, Sub),
    This,
    NewTarget,
    Arguments,
}

impl Leafy for Expr {
    /// A leaf that reads state rather than a constant, so a deep tree's fringe still varies.
    fn leaf(u: &mut arbitrary::Unstructured<'_>) -> arbitrary::Result<Self> {
        Ok(match u8::arbitrary(u)? % 3 {
            0 => Self::Local(u8::arbitrary(u)?),
            1 => Self::Builtin(u8::arbitrary(u)?),
            _ => Self::Const(ConstSpec::arbitrary(u)?),
        })
    }
}

#[derive(Debug, Clone, Arbitrary)]
pub enum Stmt {
    Eval(Expr),
    Store(u8, Expr),
    StoreCell(u8, Expr),
    Log(Many<Sub, 3>),
    If(Expr, Many<Stmt, 4>, Many<Stmt, 4>),
    Throw(Expr),
    /// `PushHandler(catch) … PopHandler; Jump(end); catch: Pop; … end:` -- the shape lowering
    /// itself emits for `try`/`catch`.
    Try(Many<Stmt, 3>, Many<Stmt, 3>),
}

/// One field of one instruction, changed after the code is otherwise well-formed.
///
/// This is where the target earns its keep. Both consumers carry their own idea of what a valid
/// program is -- `kiln_backend_c::validate` statically, the interpreter dynamically -- and two
/// separately written validators for the same contract is exactly the shape of thing that
/// disagrees. A perturbation moves a program to the boundary; the test asserts they agree about
/// which side of it the program landed on.
#[derive(Debug, Clone, Copy, Arbitrary)]
pub enum Perturbation {
    ConstIndex(u16, u16),
    LocalIndex(u16, u16),
    JumpTarget(u16, u16),
    FunctionIndex(u16, u16),
    Argc(u16, u16),
    /// Deleting one instruction is the cheapest way to unbalance a stack that was balanced.
    Delete(u16),
}

#[derive(Debug, Clone, Arbitrary)]
pub struct IrProgram {
    functions: Many<(u8, Many<Stmt, 4>, Expr), 2>,
    body: Many<Stmt, 8>,
    perturbations: Many<Perturbation, 3>,
}

/// Emits one body, refusing to exceed the instruction budget.
struct Builder {
    code: Vec<Instruction>,
    constants: Vec<Constant>,
    function_count: u32,
    /// True while emitting a function body: no closures and no calls, so no recursion.
    nested: bool,
}

impl Builder {
    fn budget_left(&self) -> bool {
        self.code.len() < MAX_INSTRUCTIONS
    }

    fn push(&mut self, instruction: Instruction) {
        self.code.push(instruction);
    }

    fn constant(&mut self, constant: Constant) -> u32 {
        if let Some(index) = self.constants.iter().position(|held| *held == constant) {
            return u32::try_from(index).unwrap_or(0);
        }
        self.constants.push(constant);
        u32::try_from(self.constants.len() - 1).unwrap_or(0)
    }

    fn string_constant(&mut self, index: u8) -> u32 {
        let text = STRINGS[index as usize % STRINGS.len()].to_owned();
        self.constant(Constant::String(text))
    }

    /// Emit an expression. Always leaves exactly one value on the stack, budget or no budget.
    fn expression(&mut self, expression: &Expr, depth: u32) {
        if depth >= 4 || !self.budget_left() {
            let index = self.constant(Constant::Number(1.0_f64.to_bits()));
            self.push(Instruction::Const(index));
            return;
        }
        match expression {
            Expr::Const(spec) => {
                let constant = match spec {
                    ConstSpec::Undefined => Constant::Undefined,
                    ConstSpec::Null => Constant::Null,
                    ConstSpec::Bool(value) => Constant::Boolean(*value),
                    ConstSpec::Number(index) => {
                        Constant::Number(NUMBERS[*index as usize % NUMBERS.len()].to_bits())
                    }
                    ConstSpec::Str(index) => {
                        Constant::String(STRINGS[*index as usize % STRINGS.len()].to_owned())
                    }
                    // Digits only, no sign: the IR spells `-1n` as a negation of `1n`.
                    ConstSpec::BigInt(index) => Constant::BigInt(format!("{}", index % 100)),
                };
                let index = self.constant(constant);
                self.push(Instruction::Const(index));
            }
            Expr::Local(slot) => self.push(Instruction::LoadLocal(plain_slot(*slot))),
            Expr::Cell(slot) => {
                // A cell has to exist before it is read. Creating it here rather than trusting
                // the generator to order the two keeps every generated program well-formed.
                let slot = cell_slot(*slot);
                self.push(Instruction::NewCell(slot));
                self.push(Instruction::LoadCell(slot));
            }
            Expr::Unary(op, operand) => {
                self.expression(operand, depth + 1);
                self.push(Instruction::Unary(op.op()));
            }
            Expr::Binary(op, left, right) => {
                self.expression(left, depth + 1);
                self.expression(right, depth + 1);
                self.push(Instruction::Binary(op.op()));
            }
            Expr::Array(elements) => {
                let elements: Vec<&Expr> =
                    elements.iter().map(|element| &**element).take(4).collect();
                for element in &elements {
                    // A hole is legal here and nowhere else, which is also the only place
                    // lowering ever emits one: `PushHole` describes a *missing* element of an
                    // array literal, and the marker is peeled off again by every read. Letting
                    // it be a general expression reproduces a known interpreter panic (README,
                    // "An array hole used as a value") rather than looking for a new one.
                    if matches!(element, Expr::Hole) {
                        self.push(Instruction::PushHole);
                    } else {
                        self.expression(element, depth + 1);
                    }
                }
                self.push(Instruction::NewArray {
                    count: u32::try_from(elements.len()).unwrap_or(0),
                });
            }
            // Outside an array literal a hole is not a value. See `Expr::Array`.
            Expr::Hole => {
                let index = self.constant(Constant::Undefined);
                self.push(Instruction::Const(index));
            }
            Expr::Object(entries) => {
                let entries: Vec<(&u8, &Expr)> = entries
                    .iter()
                    .map(|(key, value)| (key, &**value))
                    .take(3)
                    .collect();
                for (key, value) in &entries {
                    let name = self.string_constant(**key);
                    self.push(Instruction::Const(name));
                    self.expression(value, depth + 1);
                }
                self.push(Instruction::NewObject {
                    count: u32::try_from(entries.len()).unwrap_or(0),
                });
            }
            Expr::GetProperty(object, key) => {
                self.expression(object, depth + 1);
                let name = self.string_constant(*key);
                self.push(Instruction::GetProperty(name));
            }
            Expr::GetComputed(object, key) => {
                self.expression(object, depth + 1);
                self.expression(key, depth + 1);
                self.push(Instruction::GetComputed);
            }
            Expr::SetProperty(object, key, value) => {
                self.expression(object, depth + 1);
                self.expression(value, depth + 1);
                let name = self.string_constant(*key);
                self.push(Instruction::SetProperty(name));
            }
            Expr::DeleteProperty(object, key) => {
                self.expression(object, depth + 1);
                let name = self.string_constant(*key);
                self.push(Instruction::DeleteProperty(name));
            }
            Expr::Builtin(index) => self.push(Instruction::LoadBuiltin(builtin(*index))),
            Expr::MakeFunction(index) | Expr::New(index, _) | Expr::Call(index, _)
                if self.nested || self.function_count == 0 =>
            {
                // No closures and no calls inside a function body: see property 3.
                let _ = index;
                let constant = self.constant(Constant::Undefined);
                self.push(Instruction::Const(constant));
            }
            Expr::MakeFunction(index) => {
                self.push(Instruction::MakeFunction(
                    u32::from(*index) % self.function_count,
                ));
            }
            Expr::Call(index, arguments) => {
                self.push(Instruction::MakeFunction(
                    u32::from(*index) % self.function_count,
                ));
                let argc = self.arguments(arguments, depth);
                self.push(Instruction::Call { argc });
            }
            Expr::New(index, arguments) => {
                self.push(Instruction::MakeFunction(
                    u32::from(*index) % self.function_count,
                ));
                let argc = self.arguments(arguments, depth);
                self.push(Instruction::New { argc });
            }
            Expr::CallMethod(receiver, name, arguments) => {
                self.expression(receiver, depth + 1);
                let name = self.string_constant(*name);
                let argc = self.arguments(arguments, depth);
                self.push(Instruction::CallMethod { name, argc });
            }
            Expr::EnumerateKeys(operand) => {
                self.expression(operand, depth + 1);
                self.push(Instruction::EnumerateKeys);
            }
            Expr::IterableToArray(operand) => {
                self.expression(operand, depth + 1);
                self.push(Instruction::IterableToArray);
            }
            Expr::SpreadInto(target, source) => {
                self.expression(target, depth + 1);
                self.push(Instruction::IterableToArray);
                self.expression(source, depth + 1);
                self.push(Instruction::SpreadInto);
            }
            Expr::This => self.push(Instruction::LoadThis),
            Expr::NewTarget => self.push(Instruction::LoadNewTarget),
            Expr::Arguments => self.push(Instruction::LoadArguments),
        }
    }

    fn arguments(&mut self, arguments: &[Sub], depth: u32) -> u16 {
        let arguments: Vec<&Expr> = arguments
            .iter()
            .map(|argument| &**argument)
            .take(2)
            .collect();
        for argument in &arguments {
            self.expression(argument, depth + 1);
        }
        u16::try_from(arguments.len()).unwrap_or(0)
    }

    fn statements(&mut self, statements: &[Stmt]) {
        for statement in statements.iter().take(6) {
            if !self.budget_left() {
                return;
            }
            self.statement(statement);
        }
    }

    fn statement(&mut self, statement: &Stmt) {
        match statement {
            Stmt::Eval(expression) => {
                self.expression(expression, 0);
                self.push(Instruction::Pop);
            }
            Stmt::Store(slot, expression) => {
                self.expression(expression, 0);
                self.push(Instruction::StoreLocal(plain_slot(*slot)));
            }
            Stmt::StoreCell(slot, expression) => {
                let slot = cell_slot(*slot);
                self.push(Instruction::NewCell(slot));
                self.expression(expression, 0);
                // `StoreCell` consumes the value and leaves nothing, unlike `SetProperty`
                // which leaves it -- an assignment expression has a value, a cell write is a
                // statement. Getting this wrong is a stack underflow, which is why the test
                // checks the unperturbed program before it checks anything about kiln.
                self.push(Instruction::StoreCell(slot));
            }
            Stmt::Log(arguments) => {
                let arguments: Vec<&Expr> = arguments
                    .iter()
                    .map(|argument| &**argument)
                    .take(3)
                    .collect();
                for argument in &arguments {
                    self.expression(argument, 0);
                }
                self.push(Instruction::HostCall {
                    op: HostOp::WriteStdout,
                    argc: u16::try_from(arguments.len()).unwrap_or(0),
                });
                self.push(Instruction::Pop);
            }
            Stmt::Throw(expression) => {
                self.expression(expression, 0);
                self.push(Instruction::Throw);
            }
            Stmt::If(test, consequent, alternate) => {
                self.expression(test, 0);
                let to_else = self.code.len();
                self.push(Instruction::JumpIfFalse(0));
                self.statements(consequent);
                let to_end = self.code.len();
                self.push(Instruction::Jump(0));
                self.patch(to_else);
                self.statements(alternate);
                self.patch(to_end);
            }
            Stmt::Try(block, handler) => {
                let to_handler = self.code.len();
                self.push(Instruction::PushHandler(0));
                self.statements(block);
                self.push(Instruction::PopHandler);
                let to_end = self.code.len();
                self.push(Instruction::Jump(0));
                self.patch(to_handler);
                // The handler is entered with the thrown value on the stack, and lowering pops
                // it when the clause binds nothing. Mirrored exactly, because a handler that
                // leaves it there is a program the real compiler never emits.
                self.push(Instruction::Pop);
                self.statements(handler);
                self.patch(to_end);
            }
        }
    }

    /// Point a previously emitted jump at the current end of the code.
    fn patch(&mut self, at: usize) {
        let target = u32::try_from(self.code.len()).unwrap_or(0);
        match &mut self.code[at] {
            Instruction::Jump(slot)
            | Instruction::JumpIfFalse(slot)
            | Instruction::JumpIfTruthy(slot)
            | Instruction::JumpIfNotNullish(slot)
            | Instruction::PushHandler(slot) => *slot = target,
            _ => {}
        }
    }
}

const fn builtin(index: u8) -> Builtin {
    match index % 9 {
        0 => Builtin::Error,
        1 => Builtin::Symbol,
        2 => Builtin::String,
        3 => Builtin::GlobalThis,
        4 => Builtin::Set,
        5 => Builtin::Map,
        6 => Builtin::Array,
        7 => Builtin::Object,
        _ => Builtin::Json,
    }
}

impl IrProgram {
    /// Lower to bytecode, then apply the perturbations.
    ///
    /// Two programs come back: the well-formed one and the perturbed one. Callers that want to
    /// know whether a disagreement is *caused* by a perturbation need both.
    pub fn build(&self) -> (Program, Program) {
        let functions: Vec<&(u8, Many<Stmt, 4>, Expr)> =
            self.functions.iter().take(MAX_FUNCTIONS).collect();
        let mut protos = Vec::new();
        let mut builder = Builder {
            code: Vec::new(),
            constants: Vec::new(),
            function_count: u32::try_from(functions.len()).unwrap_or(0),
            nested: true,
        };

        for (parameters, statements, result) in &functions {
            builder.code = Vec::new();
            builder.statements(statements);
            builder.expression(result, 1);
            builder.push(Instruction::Return);
            let parameter_count = u32::from(*parameters) % LOCAL_COUNT;
            protos.push(FunctionProto {
                asynchronous: false,
                name: None,
                arrow: false,
                generator: false,
                async_generator: false,
                parameter_count,
                declared_length: parameter_count,
                local_count: LOCAL_COUNT,
                // Generated functions have no parameter-binding prologue; arguments already
                // occupy their declared slots when the fuzz harness calls them.
                prologue_end: 0,
                mapped_arguments: false,
                span: kiln_ir::Span::new(0, 0),
                code: std::mem::take(&mut builder.code),
                upvalues: Vec::new(),
            });
        }

        builder.nested = false;
        builder.code = Vec::new();
        builder.statements(&self.body);
        let undefined = builder.constant(Constant::Undefined);
        builder.push(Instruction::Const(undefined));
        builder.push(Instruction::Return);

        let well_formed = Program {
            source: std::sync::Arc::from(""),
            template_sites: Vec::new(),
            version: kiln_ir::BYTECODE_VERSION,
            constants: builder.constants,
            code: builder.code,
            local_count: LOCAL_COUNT,
            functions: protos,
        };
        let mut perturbed = well_formed.clone();
        for perturbation in self.perturbations.iter().take(3) {
            apply(&mut perturbed, *perturbation);
        }
        (well_formed, perturbed)
    }
}

/// Change exactly one field, in place, without ever creating a back edge.
///
/// The forward-only clamp on jump targets is not a nicety: a perturbed back edge is an
/// unbounded loop, and an unbounded loop over an operand stack is an allocator that never
/// returns. See property 1 in the module documentation.
fn apply(program: &mut Program, perturbation: Perturbation) {
    let length = program.code.len();
    if length == 0 {
        return;
    }
    let at = |index: u16| usize::from(index) % length;
    match perturbation {
        Perturbation::ConstIndex(index, value) => {
            let value = u32::from(value);
            if let Instruction::Const(slot)
            | Instruction::GetProperty(slot)
            | Instruction::SetProperty(slot)
            | Instruction::DeleteProperty(slot)
            | Instruction::GetPrivate(slot)
            | Instruction::SetPrivate(slot)
            | Instruction::ThrowReference(slot) = &mut program.code[at(index)]
            {
                *slot = value;
            }
        }
        // Each half of the local space is perturbed within its own half plus the two slots just
        // past the end of the frame, which is the boundary both consumers must have an opinion
        // about. Moving a `LoadLocal` onto a cell slot is deliberately not generated: it
        // reproduces a known interpreter panic (README, "A cell read as a local") that would
        // otherwise be the only thing this target ever reported.
        Perturbation::LocalIndex(index, value) => {
            let here = at(index);
            match &mut program.code[here] {
                Instruction::LoadLocal(slot) | Instruction::StoreLocal(slot) => {
                    *slot = match u32::from(value) % (PLAIN_LOCALS + 2) {
                        in_range if in_range < PLAIN_LOCALS => in_range,
                        past_the_end => LOCAL_COUNT + past_the_end - PLAIN_LOCALS,
                    };
                }
                Instruction::NewCell(slot)
                | Instruction::LoadCell(slot)
                | Instruction::StoreCell(slot)
                | Instruction::MarkUninitialized(slot) => {
                    *slot = match u32::from(value) % (CELL_SLOTS + 2) {
                        in_range if in_range < CELL_SLOTS => FIRST_CELL + in_range,
                        past_the_end => LOCAL_COUNT + past_the_end - CELL_SLOTS,
                    };
                }
                _ => {}
            }
        }
        Perturbation::JumpTarget(index, value) => {
            let here = at(index);
            // Clamped forward, and allowed to land one past the end -- which is the boundary
            // case both consumers have to have an opinion about.
            let forward = u32::try_from(here).unwrap_or(0)
                + 1
                + u32::from(value) % u32::try_from(length + 2).unwrap_or(1);
            if let Instruction::Jump(slot)
            | Instruction::JumpIfFalse(slot)
            | Instruction::JumpIfTruthy(slot)
            | Instruction::JumpIfNotNullish(slot)
            | Instruction::PushHandler(slot) = &mut program.code[here]
            {
                *slot = forward;
            }
        }
        Perturbation::FunctionIndex(index, value) => {
            let value = u32::from(value);
            if let Instruction::MakeFunction(slot) = &mut program.code[at(index)] {
                *slot = value;
            }
        }
        Perturbation::Argc(index, value) => {
            if let Instruction::Call { argc }
            | Instruction::New { argc }
            | Instruction::CallMethod { argc, .. }
            | Instruction::CallComputed { argc }
            | Instruction::SuperCall { argc }
            | Instruction::HostCall { argc, .. } = &mut program.code[at(index)]
            {
                *argc = value;
            }
        }
        Perturbation::Delete(index) => {
            program.code.remove(at(index));
        }
    }
}
