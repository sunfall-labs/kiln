//! A small, deterministic stack interpreter for Kiln's parser-independent IR.
//!
//! The interpreter deliberately knows nothing about the frontend.  It consumes only the owned
//! bytecode in `kiln-ir` and reaches the outside world through the versioned `HostOps` trait.  It
//! is intentionally a scalar runtime for the first execution slice; object allocation,
//! exceptions, and async scheduling belong above this VM.

use std::fmt;

use kiln_host_abi::{HostOps, HostValue};
use kiln_ir::{BinaryOp, Constant, Instruction, ModuleProgram, Program, UnaryOp};

/// A JavaScript scalar value represented by the interpreter.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Undefined,
    Null,
    Boolean(bool),
    Number(f64),
    String(String),
    /// A callable, identified by its index in [`kiln_ir::Program::functions`].
    ///
    /// A bare index is enough only because functions cannot capture anything yet: every
    /// evaluation of the same declaration produces an indistinguishable callable. Closures make
    /// that false, and this becomes a handle to a heap object holding the proto plus its
    /// captured cells.
    Function(u32),
}

impl Value {
    /// The name JavaScript would use for this value's type, for diagnostics only.
    pub fn type_name(&self) -> &'static str {
        match self {
            Self::Undefined => "undefined",
            Self::Null => "null",
            Self::Boolean(_) => "boolean",
            Self::Number(_) => "number",
            Self::String(_) => "string",
            Self::Function(_) => "function",
        }
    }

    pub fn from_number_bits(bits: u64) -> Self {
        Self::Number(f64::from_bits(bits))
    }

    pub fn as_number(&self) -> f64 {
        match self {
            Self::Undefined => f64::NAN,
            Self::Null => 0.0,
            Self::Boolean(value) => f64::from(*value),
            Self::Number(value) => *value,
            Self::String(value) => parse_number(value),
            // `Number(fn)` is NaN in JavaScript: a function has no numeric value, and ToNumber
            // on it goes through ToPrimitive, which cannot produce one.
            Self::Function(_) => f64::NAN,
        }
    }

    pub fn is_truthy(&self) -> bool {
        match self {
            Self::Undefined | Self::Null => false,
            Self::Boolean(value) => *value,
            Self::Number(value) => *value != 0.0 && !value.is_nan(),
            Self::String(value) => !value.is_empty(),
            // Every object is truthy, and a function is an object.
            Self::Function(_) => true,
        }
    }

    pub fn is_nullish(&self) -> bool {
        matches!(self, Self::Undefined | Self::Null)
    }

    pub fn to_display_string(&self) -> String {
        match self {
            Self::Undefined => "undefined".to_owned(),
            Self::Null => "null".to_owned(),
            Self::Boolean(value) => value.to_string(),
            Self::Number(value) => number_string(*value),
            Self::String(value) => value.clone(),
            // Not `Function.prototype.toString`, which returns the source text. Producing that
            // needs the span and the original source, neither of which reaches the VM; naming
            // the function is more useful here than an invented body would be.
            Self::Function(index) => format!("[Function {index}]"),
        }
    }
}

impl From<Constant> for Value {
    fn from(value: Constant) -> Self {
        match value {
            Constant::Undefined => Self::Undefined,
            Constant::Null => Self::Null,
            Constant::Boolean(value) => Self::Boolean(value),
            Constant::Number(bits) => Self::from_number_bits(bits),
            Constant::String(value) => Self::String(value),
        }
    }
}

impl From<Value> for HostValue {
    fn from(value: Value) -> Self {
        match value {
            Value::Undefined => Self::Undefined,
            Value::Null => Self::Null,
            Value::Boolean(value) => Self::Boolean(value),
            Value::Number(value) => Self::Number(value.to_bits()),
            Value::String(value) => Self::String(value),
            // The ABI is scalars only, deliberately: a host operation that received a callable
            // would need the VM to call back into itself, which is a different contract from
            // the one HostOps has. Passing the display form keeps `console.log(fn)` printing
            // something truthful rather than failing.
            Value::Function(index) => Self::String(format!("[Function {index}]")),
        }
    }
}

impl From<HostValue> for Value {
    fn from(value: HostValue) -> Self {
        match value {
            HostValue::Undefined => Self::Undefined,
            HostValue::Null => Self::Null,
            HostValue::Boolean(value) => Self::Boolean(value),
            HostValue::Number(bits) => Self::from_number_bits(bits),
            HostValue::String(value) => Self::String(value),
        }
    }
}

/// Limits that make execution reproducible and prevent an accidental infinite loop from hanging
/// the daemon.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExecutionLimits {
    pub max_steps: u64,
    /// Frames allowed on the call stack.
    ///
    /// A separate bound from `max_steps` because unbounded recursion exhausts the host stack
    /// long before it exhausts a step budget, and the failure mode is a process abort rather
    /// than a diagnostic. Bounding frames turns it into an error the caller can report.
    pub max_call_depth: usize,
}

impl Default for ExecutionLimits {
    fn default() -> Self {
        Self {
            max_steps: 1_000_000,
            max_call_depth: 512,
        }
    }
}

/// The successful result of one VM invocation.
#[derive(Clone, Debug, PartialEq)]
pub struct ExecutionResult {
    pub value: Value,
    pub steps: u64,
}

/// One value published by a completed module evaluation.
#[derive(Clone, Debug, PartialEq)]
pub struct ModuleExportValue {
    pub name: String,
    pub value: Value,
}

/// A completed module invocation, including the values its local export slots hold.
#[derive(Clone, Debug, PartialEq)]
pub struct ModuleExecutionResult {
    pub execution: ExecutionResult,
    pub exports: Vec<ModuleExportValue>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ExecutionError {
    ImportCountMismatch {
        expected: usize,
        actual: usize,
    },
    StepLimitExceeded {
        steps: u64,
        limit: u64,
    },
    InvalidInstructionPointer {
        instruction: usize,
    },
    ConstantOutOfBounds {
        index: usize,
    },
    LocalOutOfBounds {
        index: usize,
    },
    StackUnderflow {
        instruction: usize,
    },
    InvalidJump {
        instruction: usize,
        target: usize,
    },
    Host {
        instruction: usize,
        message: String,
    },
    /// A `MakeFunction` or `Call` naming a proto the program does not contain.
    UnknownFunction {
        function: u32,
    },
    NotCallable {
        instruction: usize,
        value: &'static str,
    },
    CallDepthExceeded {
        limit: usize,
    },
    GlobalOutOfBounds {
        index: usize,
    },
}

impl fmt::Display for ExecutionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ImportCountMismatch { expected, actual } => write!(
                formatter,
                "module expected {expected} imported values but received {actual}"
            ),
            Self::StepLimitExceeded { steps, limit } => {
                write!(
                    formatter,
                    "execution exceeded {limit} steps after {steps} steps"
                )
            }
            Self::InvalidInstructionPointer { instruction } => {
                write!(formatter, "invalid instruction pointer {instruction}")
            }
            Self::ConstantOutOfBounds { index } => write!(formatter, "constant {index} is missing"),
            Self::LocalOutOfBounds { index } => write!(formatter, "local {index} is missing"),
            Self::StackUnderflow { instruction } => {
                write!(formatter, "stack underflow at instruction {instruction}")
            }
            Self::InvalidJump {
                instruction,
                target,
            } => {
                write!(
                    formatter,
                    "invalid jump from instruction {instruction} to {target}"
                )
            }
            Self::UnknownFunction { function } => {
                write!(formatter, "no compiled function at index {function}")
            }
            Self::NotCallable { instruction, value } => write!(
                formatter,
                "instruction {instruction} tried to call a {value}, which is not a function"
            ),
            Self::CallDepthExceeded { limit } => {
                write!(formatter, "call depth exceeded {limit} frames")
            }
            Self::GlobalOutOfBounds { index } => {
                write!(formatter, "module slot {index} is out of bounds")
            }
            Self::Host {
                instruction,
                message,
            } => {
                write!(
                    formatter,
                    "host operation failed at instruction {instruction}: {message}"
                )
            }
        }
    }
}

impl std::error::Error for ExecutionError {}

/// Execute one IR program against a host implementation.
pub fn execute<H: HostOps + ?Sized>(
    program: &Program,
    host: &mut H,
    limits: ExecutionLimits,
) -> Result<ExecutionResult, ExecutionError> {
    let mut vm = Vm {
        program,
        host,
        stack: Vec::new(),
        locals: vec![Value::Undefined; program.local_count as usize],
        function: None,
        frames: Vec::new(),
        instruction: 0,
        steps: 0,
        limits,
    };
    vm.run()
}

/// Execute one linkage-aware module after its acyclic dependencies have completed.
///
/// `import_values` is ordered exactly like [`ModuleProgram::imports`]. Each value is installed in
/// the compiler-assigned local slot before the first bytecode instruction executes.
pub fn execute_module<H: HostOps + ?Sized>(
    module: &ModuleProgram,
    import_values: &[Value],
    host: &mut H,
    limits: ExecutionLimits,
) -> Result<ModuleExecutionResult, ExecutionError> {
    if import_values.len() != module.imports.len() {
        return Err(ExecutionError::ImportCountMismatch {
            expected: module.imports.len(),
            actual: import_values.len(),
        });
    }

    let mut locals = vec![Value::Undefined; module.program.local_count as usize];
    for (binding, value) in module.imports.iter().zip(import_values) {
        let Some(local) = locals.get_mut(binding.local_slot as usize) else {
            return Err(ExecutionError::LocalOutOfBounds {
                index: binding.local_slot as usize,
            });
        };
        *local = value.clone();
    }

    let mut vm = Vm {
        program: &module.program,
        host,
        stack: Vec::new(),
        locals,
        function: None,
        frames: Vec::new(),
        instruction: 0,
        steps: 0,
        limits,
    };
    let execution = vm.run()?;
    let mut exports = Vec::with_capacity(module.exports.len());
    for binding in &module.exports {
        let Some(value) = vm.locals.get(binding.local_slot as usize) else {
            return Err(ExecutionError::LocalOutOfBounds {
                index: binding.local_slot as usize,
            });
        };
        exports.push(ModuleExportValue {
            name: binding.exported.clone(),
            value: value.clone(),
        });
    }
    Ok(ModuleExecutionResult { execution, exports })
}

/// A suspended caller, restored when the callee returns.
///
/// The module body is not represented here: it is the bottom of the call stack rather than a
/// frame in it, which is why an empty `frames` means `Return` ends execution.
struct Frame {
    /// Bytecode of the function that was running, or `None` for the module body.
    function: Option<u32>,
    locals: Vec<Value>,
    /// Where to resume in the caller.
    instruction: usize,
    /// Stack height when the call began, so a callee that leaves debris cannot corrupt the
    /// caller's operand stack.
    stack_base: usize,
}

struct Vm<'program, 'host, H: HostOps + ?Sized> {
    program: &'program Program,
    host: &'host mut H,
    stack: Vec<Value>,
    locals: Vec<Value>,
    /// Bytecode currently executing: `None` is the module body, `Some(i)` is `functions[i]`.
    function: Option<u32>,
    frames: Vec<Frame>,
    instruction: usize,
    steps: u64,
    limits: ExecutionLimits,
}

impl<'program, H: HostOps + ?Sized> Vm<'program, '_, H> {
    fn run(&mut self) -> Result<ExecutionResult, ExecutionError> {
        loop {
            if self.steps >= self.limits.max_steps {
                return Err(ExecutionError::StepLimitExceeded {
                    steps: self.steps,
                    limit: self.limits.max_steps,
                });
            }
            let Some(opcode) = self.code().get(self.instruction) else {
                return Err(ExecutionError::InvalidInstructionPointer {
                    instruction: self.instruction,
                });
            };
            let current = self.instruction;
            self.steps += 1;
            self.instruction += 1;

            match opcode {
                Instruction::Const(index) => {
                    let Some(value) = self.program.constants.get(*index as usize) else {
                        return Err(ExecutionError::ConstantOutOfBounds {
                            index: *index as usize,
                        });
                    };
                    self.stack.push(value.clone().into());
                }
                Instruction::LoadLocal(index) => {
                    let Some(value) = self.locals.get(*index as usize) else {
                        return Err(ExecutionError::LocalOutOfBounds {
                            index: *index as usize,
                        });
                    };
                    self.stack.push(value.clone());
                }
                Instruction::StoreLocal(index) => {
                    let value = self.pop(current)?;
                    let Some(local) = self.locals.get_mut(*index as usize) else {
                        return Err(ExecutionError::LocalOutOfBounds {
                            index: *index as usize,
                        });
                    };
                    *local = value;
                }
                Instruction::Pop => {
                    self.pop(current)?;
                }
                Instruction::Dup => {
                    let value =
                        self.stack
                            .last()
                            .cloned()
                            .ok_or(ExecutionError::StackUnderflow {
                                instruction: current,
                            })?;
                    self.stack.push(value);
                }
                Instruction::Unary(operator) => {
                    let value = self.pop(current)?;
                    self.stack.push(unary(*operator, value));
                }
                Instruction::Binary(operator) => {
                    let right = self.pop(current)?;
                    let left = self.pop(current)?;
                    self.stack.push(binary(*operator, left, right));
                }
                Instruction::JumpIfFalse(target) => {
                    if !self.pop(current)?.is_truthy() {
                        self.jump(current, *target)?;
                    }
                }
                Instruction::JumpIfTruthy(target) => {
                    if self.pop(current)?.is_truthy() {
                        self.jump(current, *target)?;
                    }
                }
                Instruction::JumpIfNotNullish(target) => {
                    if !self.pop(current)?.is_nullish() {
                        self.jump(current, *target)?;
                    }
                }
                Instruction::Jump(target) => self.jump(current, *target)?,
                Instruction::HostCall { op, argc } => {
                    let argc = usize::from(*argc);
                    if self.stack.len() < argc {
                        return Err(ExecutionError::StackUnderflow {
                            instruction: current,
                        });
                    }
                    let args_start = self.stack.len() - argc;
                    let args = self
                        .stack
                        .drain(args_start..)
                        .map(HostValue::from)
                        .collect::<Vec<_>>();
                    let result =
                        self.host
                            .call(*op, &args)
                            .map_err(|error| ExecutionError::Host {
                                instruction: current,
                                message: error.to_string(),
                            })?;
                    self.stack.push(result.into());
                }
                Instruction::MakeFunction(index) => {
                    let index = *index;
                    if index as usize >= self.program.functions.len() {
                        return Err(ExecutionError::UnknownFunction { function: index });
                    }
                    self.stack.push(Value::Function(index));
                }
                Instruction::Call { argc } => {
                    let argc = *argc;
                    let depth = argc as usize;
                    let callee = self
                        .stack
                        .len()
                        .checked_sub(depth + 1)
                        .and_then(|index| self.stack.get(index))
                        .ok_or(ExecutionError::StackUnderflow {
                            instruction: current,
                        })?;
                    let Value::Function(function) = callee else {
                        return Err(ExecutionError::NotCallable {
                            instruction: current,
                            value: callee.type_name(),
                        });
                    };
                    let function = *function;
                    self.call(function, argc, current)?;
                }
                Instruction::LoadGlobal(slot) => {
                    let value = self.global(*slot as usize)?;
                    self.stack.push(value);
                }
                Instruction::StoreGlobal(slot) => {
                    let value = self.pop(current)?;
                    self.set_global(*slot as usize, value)?;
                }
                Instruction::Return => {
                    let value = self.stack.pop().unwrap_or(Value::Undefined);
                    let Some(frame) = self.frames.pop() else {
                        return Ok(ExecutionResult {
                            value,
                            steps: self.steps,
                        });
                    };
                    // Discard anything the callee left behind. A well-formed program leaves
                    // nothing, but truncating means a malformed one cannot corrupt its caller.
                    self.stack.truncate(frame.stack_base);
                    self.locals = frame.locals;
                    self.function = frame.function;
                    self.instruction = frame.instruction;
                    self.stack.push(value);
                }
            }
        }
    }

    /// Read a module-scope slot from wherever the module body's locals currently live.
    ///
    /// At module level that is `self.locals`; inside any call it is the bottom frame's, because
    /// the module body is the bottom of the call stack. Going through the frame rather than
    /// copying module locals into each callee is what makes a module binding shared: an
    /// assignment from inside a function has to be visible to the next call that reads it.
    fn global(&self, slot: usize) -> Result<Value, ExecutionError> {
        self.module_locals()
            .get(slot)
            .cloned()
            .ok_or(ExecutionError::GlobalOutOfBounds { index: slot })
    }

    fn set_global(&mut self, slot: usize, value: Value) -> Result<(), ExecutionError> {
        let locals = match self.frames.first_mut() {
            Some(module) => &mut module.locals,
            None => &mut self.locals,
        };
        *locals
            .get_mut(slot)
            .ok_or(ExecutionError::GlobalOutOfBounds { index: slot })? = value;
        Ok(())
    }

    fn module_locals(&self) -> &[Value] {
        match self.frames.first() {
            Some(module) => &module.locals,
            None => &self.locals,
        }
    }

    /// Bytecode of the function currently executing.
    fn code(&self) -> &'program [Instruction] {
        match self.function {
            None => &self.program.code,
            // Validated when the frame was pushed; a call never installs an out-of-range index.
            Some(index) => &self.program.functions[index as usize].code,
        }
    }

    /// Enter `function` with `argc` arguments sitting on top of the stack.
    ///
    /// Missing arguments become `undefined` and extra ones are discarded, which is what calling
    /// a JavaScript function with the wrong arity does. Neither is an error, so neither is
    /// reported as one.
    fn call(&mut self, function: u32, argc: u16, instruction: usize) -> Result<(), ExecutionError> {
        let Some(proto) = self.program.functions.get(function as usize) else {
            return Err(ExecutionError::UnknownFunction { function });
        };
        if self.frames.len() >= self.limits.max_call_depth {
            return Err(ExecutionError::CallDepthExceeded {
                limit: self.limits.max_call_depth,
            });
        }

        let argc = argc as usize;
        if self.stack.len() < argc + 1 {
            return Err(ExecutionError::StackUnderflow { instruction });
        }
        let mut arguments = self.stack.split_off(self.stack.len() - argc);
        // The callee sits directly below its arguments; removing it here is what makes the
        // stack look, to the caller, as though the call left only its result.
        self.stack.pop();

        let mut locals = vec![Value::Undefined; proto.local_count as usize];
        arguments.truncate(proto.parameter_count as usize);
        for (slot, argument) in arguments.into_iter().enumerate() {
            locals[slot] = argument;
        }

        self.frames.push(Frame {
            function: self.function,
            locals: std::mem::replace(&mut self.locals, locals),
            instruction: self.instruction,
            stack_base: self.stack.len(),
        });
        self.function = Some(function);
        self.instruction = 0;
        Ok(())
    }

    fn pop(&mut self, instruction: usize) -> Result<Value, ExecutionError> {
        self.stack
            .pop()
            .ok_or(ExecutionError::StackUnderflow { instruction })
    }

    fn jump(&mut self, instruction: usize, target: u32) -> Result<(), ExecutionError> {
        let target = target as usize;
        if target >= self.code().len() {
            return Err(ExecutionError::InvalidJump {
                instruction,
                target,
            });
        }
        self.instruction = target;
        Ok(())
    }
}

fn unary(operator: UnaryOp, value: Value) -> Value {
    match operator {
        UnaryOp::Plus => Value::Number(value.as_number()),
        UnaryOp::Negate => Value::Number(-value.as_number()),
        UnaryOp::Not => Value::Boolean(!value.is_truthy()),
    }
}

fn binary(operator: BinaryOp, left: Value, right: Value) -> Value {
    match operator {
        BinaryOp::Add => {
            if matches!(left, Value::String(_)) || matches!(right, Value::String(_)) {
                Value::String(format!(
                    "{}{}",
                    left.to_display_string(),
                    right.to_display_string()
                ))
            } else {
                Value::Number(left.as_number() + right.as_number())
            }
        }
        BinaryOp::Subtract => Value::Number(left.as_number() - right.as_number()),
        BinaryOp::Multiply => Value::Number(left.as_number() * right.as_number()),
        BinaryOp::Divide => Value::Number(left.as_number() / right.as_number()),
        BinaryOp::Remainder => Value::Number(left.as_number() % right.as_number()),
        BinaryOp::Exponentiate => Value::Number(left.as_number().powf(right.as_number())),
        BinaryOp::StrictEqual => Value::Boolean(strict_equal(&left, &right)),
        BinaryOp::StrictNotEqual => Value::Boolean(!strict_equal(&left, &right)),
        BinaryOp::LessThan => Value::Boolean(compare(&left, &right, OrderingPredicate::Less)),
        BinaryOp::LessThanOrEqual => {
            Value::Boolean(compare(&left, &right, OrderingPredicate::LessEqual))
        }
        BinaryOp::GreaterThan => Value::Boolean(compare(&left, &right, OrderingPredicate::Greater)),
        BinaryOp::GreaterThanOrEqual => {
            Value::Boolean(compare(&left, &right, OrderingPredicate::GreaterEqual))
        }
    }
}

fn strict_equal(left: &Value, right: &Value) -> bool {
    match (left, right) {
        (Value::Undefined, Value::Undefined) | (Value::Null, Value::Null) => true,
        (Value::Boolean(a), Value::Boolean(b)) => a == b,
        (Value::Number(a), Value::Number(b)) => a == b,
        (Value::String(a), Value::String(b)) => a == b,
        _ => false,
    }
}

fn compare(left: &Value, right: &Value, predicate: OrderingPredicate) -> bool {
    if let (Value::String(left), Value::String(right)) = (left, right) {
        // ECMAScript compares strings by UTF-16 code units, rather than Unicode scalar values.
        let ordering = left.encode_utf16().cmp(right.encode_utf16());
        return match predicate {
            OrderingPredicate::Less => ordering.is_lt(),
            OrderingPredicate::LessEqual => ordering.is_le(),
            OrderingPredicate::Greater => ordering.is_gt(),
            OrderingPredicate::GreaterEqual => ordering.is_ge(),
        };
    }
    match predicate {
        OrderingPredicate::Less => left.as_number() < right.as_number(),
        OrderingPredicate::LessEqual => left.as_number() <= right.as_number(),
        OrderingPredicate::Greater => left.as_number() > right.as_number(),
        OrderingPredicate::GreaterEqual => left.as_number() >= right.as_number(),
    }
}

#[derive(Clone, Copy)]
enum OrderingPredicate {
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
}

fn parse_number(value: &str) -> f64 {
    let value = value.trim();
    if value.is_empty() {
        return 0.0;
    }
    value.parse::<f64>().unwrap_or(f64::NAN)
}

fn number_string(value: f64) -> String {
    if value.is_nan() {
        "NaN".to_owned()
    } else if value == f64::INFINITY {
        "Infinity".to_owned()
    } else if value == f64::NEG_INFINITY {
        "-Infinity".to_owned()
    } else if value == 0.0 {
        "0".to_owned()
    } else {
        value.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kiln_host_abi::{HostError, HostOp};

    struct TestHost {
        calls: Vec<(HostOp, Vec<HostValue>)>,
    }

    impl HostOps for TestHost {
        fn call(
            &mut self,
            operation: HostOp,
            arguments: &[HostValue],
        ) -> Result<HostValue, HostError> {
            self.calls.push((operation, arguments.to_vec()));
            Ok(HostValue::Undefined)
        }
    }

    /// `f(a, b)` compiled by hand: proto 0 returns its second parameter.
    fn calling_program(argc: u16, arguments: Vec<Instruction>) -> Program {
        let mut code = vec![Instruction::MakeFunction(0)];
        code.extend(arguments);
        code.push(Instruction::Call { argc });
        code.push(Instruction::Return);
        Program {
            version: kiln_ir::BYTECODE_VERSION,
            constants: vec![number(1.0), number(2.0), number(3.0)],
            code,
            local_count: 0,
            functions: vec![kiln_ir::FunctionProto {
                name: Some("second".to_owned()),
                parameter_count: 2,
                local_count: 2,
                code: vec![Instruction::LoadLocal(1), Instruction::Return],
            }],
        }
    }

    fn run(program: &Program) -> Result<Value, ExecutionError> {
        let mut host = TestHost { calls: Vec::new() };
        execute(program, &mut host, ExecutionLimits::default()).map(|result| result.value)
    }

    #[test]
    fn a_call_binds_arguments_to_parameters_in_order() {
        let program = calling_program(2, vec![Instruction::Const(0), Instruction::Const(1)]);
        assert_eq!(run(&program), Ok(Value::Number(2.0)));
    }

    /// Wrong arity is not an error in JavaScript, so it is not one here either.
    #[test]
    fn missing_arguments_are_undefined_and_extra_ones_are_discarded() {
        let short = calling_program(1, vec![Instruction::Const(0)]);
        assert_eq!(
            run(&short),
            Ok(Value::Undefined),
            "second parameter unbound"
        );

        let long = calling_program(
            3,
            vec![
                Instruction::Const(0),
                Instruction::Const(1),
                Instruction::Const(2),
            ],
        );
        assert_eq!(run(&long), Ok(Value::Number(2.0)), "third argument dropped");
    }

    /// The caller's operand stack must survive the call untouched.
    #[test]
    fn a_call_leaves_only_its_result_on_the_caller_stack() {
        let mut program = calling_program(2, vec![Instruction::Const(0), Instruction::Const(1)]);
        // Push a sentinel first; it must still be underneath the result afterwards.
        program.code.insert(0, Instruction::Const(2));
        program.code.pop();
        program.code.push(Instruction::Pop);
        program.code.push(Instruction::Return);
        assert_eq!(run(&program), Ok(Value::Number(3.0)), "sentinel survived");
    }

    #[test]
    fn calling_a_non_function_is_an_error_rather_than_a_coercion() {
        let program = Program {
            version: kiln_ir::BYTECODE_VERSION,
            constants: vec![number(1.0)],
            code: vec![
                Instruction::Const(0),
                Instruction::Call { argc: 0 },
                Instruction::Return,
            ],
            local_count: 0,
            functions: Vec::new(),
        };
        assert!(matches!(
            run(&program),
            Err(ExecutionError::NotCallable {
                value: "number",
                ..
            })
        ));
    }

    /// Unbounded recursion has to be reported, not abort the process by exhausting the host
    /// stack. The step limit alone does not do it: frames are consumed far faster than steps.
    #[test]
    fn runaway_recursion_is_bounded_by_call_depth() {
        let program = Program {
            version: kiln_ir::BYTECODE_VERSION,
            constants: Vec::new(),
            code: vec![
                Instruction::MakeFunction(0),
                Instruction::Call { argc: 0 },
                Instruction::Return,
            ],
            local_count: 0,
            functions: vec![kiln_ir::FunctionProto {
                name: Some("forever".to_owned()),
                parameter_count: 0,
                local_count: 0,
                code: vec![
                    Instruction::MakeFunction(0),
                    Instruction::Call { argc: 0 },
                    Instruction::Return,
                ],
            }],
        };
        let mut host = TestHost { calls: Vec::new() };
        let limits = ExecutionLimits {
            max_call_depth: 8,
            ..ExecutionLimits::default()
        };
        assert!(matches!(
            execute(&program, &mut host, limits),
            Err(ExecutionError::CallDepthExceeded { limit: 8 })
        ));
    }

    /// A module slot written inside a frame must be visible to the next frame that reads it:
    /// that is the difference between module scope and a per-frame copy.
    #[test]
    fn module_slots_are_shared_across_frames() {
        let program = Program {
            version: kiln_ir::BYTECODE_VERSION,
            constants: vec![number(7.0)],
            code: vec![
                Instruction::MakeFunction(0),
                Instruction::Call { argc: 0 },
                Instruction::Pop,
                Instruction::MakeFunction(1),
                Instruction::Call { argc: 0 },
                Instruction::Return,
            ],
            local_count: 1,
            functions: vec![
                kiln_ir::FunctionProto {
                    name: Some("write".to_owned()),
                    parameter_count: 0,
                    local_count: 0,
                    code: vec![
                        Instruction::Const(0),
                        Instruction::StoreGlobal(0),
                        Instruction::Const(0),
                        Instruction::Return,
                    ],
                },
                kiln_ir::FunctionProto {
                    name: Some("read".to_owned()),
                    parameter_count: 0,
                    local_count: 0,
                    code: vec![Instruction::LoadGlobal(0), Instruction::Return],
                },
            ],
        };
        assert_eq!(run(&program), Ok(Value::Number(7.0)));
    }

    #[test]
    fn an_unknown_function_index_is_rejected_rather_than_indexed() {
        let program = Program {
            version: kiln_ir::BYTECODE_VERSION,
            constants: Vec::new(),
            code: vec![Instruction::MakeFunction(3), Instruction::Return],
            local_count: 0,
            functions: Vec::new(),
        };
        assert!(matches!(
            run(&program),
            Err(ExecutionError::UnknownFunction { function: 3 })
        ));
    }

    fn number(value: f64) -> Constant {
        Constant::Number(value.to_bits())
    }

    fn program(constants: Vec<Constant>, code: Vec<Instruction>) -> Program {
        Program {
            version: kiln_ir::BYTECODE_VERSION,
            constants,
            code,
            local_count: 0,
            functions: Vec::new(),
        }
    }

    #[test]
    fn scalar_coercions_match_the_execution_slice() {
        assert!(!Value::Number(0.0).is_truthy());
        assert!(!Value::Number(f64::NAN).is_truthy());
        assert_eq!(Value::String("  ".into()).as_number(), 0.0);
        assert!(Value::String("x".into()).as_number().is_nan());
        assert_eq!(Value::Number(-0.0).to_display_string(), "0");
    }

    #[test]
    fn arithmetic_and_concat_are_scalar_operations() {
        assert_eq!(
            binary(BinaryOp::Add, Value::Number(2.0), Value::Number(3.0)),
            Value::Number(5.0)
        );
        assert_eq!(
            binary(BinaryOp::Add, Value::String("x".into()), Value::Number(3.0)),
            Value::String("x3".into())
        );
        assert_eq!(
            binary(
                BinaryOp::Exponentiate,
                Value::Number(2.0),
                Value::Number(3.0)
            ),
            Value::Number(8.0)
        );
        assert_eq!(
            binary(
                BinaryOp::StrictEqual,
                Value::Number(f64::NAN),
                Value::Number(f64::NAN)
            ),
            Value::Boolean(false)
        );
    }

    #[test]
    fn executes_arithmetic_and_reports_instruction_count() {
        let program = program(
            vec![number(2.0), number(3.0)],
            vec![
                Instruction::Const(0),
                Instruction::Const(1),
                Instruction::Binary(BinaryOp::Add),
                Instruction::Return,
            ],
        );
        let mut host = TestHost { calls: Vec::new() };
        let result = execute(&program, &mut host, ExecutionLimits::default()).unwrap();
        assert_eq!(result.value, Value::Number(5.0));
        assert_eq!(result.steps, 4);
    }

    #[test]
    fn conditional_jumps_consume_conditions() {
        let program = program(
            vec![Constant::Boolean(false), number(1.0), number(2.0)],
            vec![
                Instruction::Const(0),
                Instruction::JumpIfFalse(4),
                Instruction::Const(1),
                Instruction::Jump(5),
                Instruction::Const(2),
                Instruction::Return,
            ],
        );
        let mut host = TestHost { calls: Vec::new() };
        let result = execute(&program, &mut host, ExecutionLimits::default()).unwrap();
        assert_eq!(result.value, Value::Number(2.0));
    }

    #[test]
    fn nullish_jump_keeps_non_nullish_left_operand() {
        // This is the shape emitted for `left ?? right`: duplicate the left value, consume the
        // duplicate for the branch, and leave the selected operand on the stack.
        let program = program(
            vec![
                Constant::String("left".into()),
                Constant::String("right".into()),
            ],
            vec![
                Instruction::Const(0),
                Instruction::Dup,
                Instruction::JumpIfNotNullish(6),
                Instruction::Pop,
                Instruction::Const(1),
                Instruction::Jump(6),
                Instruction::Return,
            ],
        );
        let mut host = TestHost { calls: Vec::new() };
        let result = execute(&program, &mut host, ExecutionLimits::default()).unwrap();
        assert_eq!(result.value, Value::String("left".into()));
    }

    #[test]
    fn host_calls_preserve_argument_order_and_round_trip_values() {
        let program = program(
            vec![Constant::String("first".into()), number(2.0)],
            vec![
                Instruction::Const(0),
                Instruction::Const(1),
                Instruction::HostCall {
                    op: HostOp::WriteStdout,
                    argc: 2,
                },
                Instruction::Return,
            ],
        );
        let mut host = TestHost { calls: Vec::new() };
        execute(&program, &mut host, ExecutionLimits::default()).unwrap();
        assert_eq!(host.calls.len(), 1);
        assert_eq!(host.calls[0].0, HostOp::WriteStdout);
        assert_eq!(host.calls[0].1[0], HostValue::String("first".into()));
        assert_eq!(host.calls[0].1[1], HostValue::number(2.0));
    }

    #[test]
    fn step_limit_stops_an_infinite_program() {
        let program = program(Vec::new(), vec![Instruction::Jump(0)]);
        let mut host = TestHost { calls: Vec::new() };
        let error = execute(
            &program,
            &mut host,
            ExecutionLimits {
                max_steps: 3,
                ..ExecutionLimits::default()
            },
        )
        .unwrap_err();
        assert_eq!(
            error,
            ExecutionError::StepLimitExceeded { steps: 3, limit: 3 }
        );
    }

    #[test]
    fn module_execution_initializes_import_slots_and_publishes_exports() {
        let module = ModuleProgram {
            program: Program {
                version: kiln_ir::BYTECODE_VERSION,
                constants: vec![number(2.0)],
                code: vec![
                    Instruction::LoadLocal(0),
                    Instruction::Const(0),
                    Instruction::Binary(BinaryOp::Multiply),
                    Instruction::StoreLocal(1),
                    Instruction::Const(0),
                    Instruction::Return,
                ],
                local_count: 2,
                functions: Vec::new(),
            },
            dependencies: vec!["./dep.js".into()],
            imports: vec![kiln_ir::ImportBinding {
                source: "./dep.js".into(),
                imported: "value".into(),
                local: "input".into(),
                local_slot: 0,
            }],
            exports: vec![kiln_ir::ExportBinding {
                exported: "answer".into(),
                local_slot: 1,
            }],
        };
        let mut host = TestHost { calls: Vec::new() };

        let result = execute_module(
            &module,
            &[Value::Number(21.0)],
            &mut host,
            ExecutionLimits::default(),
        )
        .expect("module executes");

        assert_eq!(result.exports[0].name, "answer");
        assert_eq!(result.exports[0].value, Value::Number(42.0));
    }
}
