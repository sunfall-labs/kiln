//! A small, deterministic stack interpreter for Kiln's parser-independent IR.
//!
//! The interpreter deliberately knows nothing about the frontend.  It consumes only the owned
//! bytecode in `kiln-ir` and reaches the outside world through the versioned `HostOps` trait.  It
//! is intentionally a scalar runtime for the first execution slice; object allocation,
//! exceptions, and async scheduling belong above this VM.

use std::fmt;

use kiln_host_abi::{HostOps, HostValue};
use kiln_ir::{BinaryOp, Constant, Instruction, Program, UnaryOp};

/// A JavaScript scalar value represented by the interpreter.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Undefined,
    Null,
    Boolean(bool),
    Number(f64),
    String(String),
}

impl Value {
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
        }
    }

    pub fn is_truthy(&self) -> bool {
        match self {
            Self::Undefined | Self::Null => false,
            Self::Boolean(value) => *value,
            Self::Number(value) => *value != 0.0 && !value.is_nan(),
            Self::String(value) => !value.is_empty(),
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
}

impl Default for ExecutionLimits {
    fn default() -> Self {
        Self {
            max_steps: 1_000_000,
        }
    }
}

/// The successful result of one VM invocation.
#[derive(Clone, Debug, PartialEq)]
pub struct ExecutionResult {
    pub value: Value,
    pub steps: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ExecutionError {
    StepLimitExceeded { steps: u64, limit: u64 },
    InvalidInstructionPointer { instruction: usize },
    ConstantOutOfBounds { index: usize },
    LocalOutOfBounds { index: usize },
    StackUnderflow { instruction: usize },
    InvalidJump { instruction: usize, target: usize },
    Host { instruction: usize, message: String },
}

impl fmt::Display for ExecutionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
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
        instruction: 0,
        steps: 0,
        limits,
    };
    vm.run()
}

struct Vm<'program, 'host, H: HostOps + ?Sized> {
    program: &'program Program,
    host: &'host mut H,
    stack: Vec<Value>,
    locals: Vec<Value>,
    instruction: usize,
    steps: u64,
    limits: ExecutionLimits,
}

impl<H: HostOps + ?Sized> Vm<'_, '_, H> {
    fn run(&mut self) -> Result<ExecutionResult, ExecutionError> {
        loop {
            if self.steps >= self.limits.max_steps {
                return Err(ExecutionError::StepLimitExceeded {
                    steps: self.steps,
                    limit: self.limits.max_steps,
                });
            }
            let Some(opcode) = self.program.code.get(self.instruction) else {
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
                Instruction::Return => {
                    return Ok(ExecutionResult {
                        value: self.stack.pop().unwrap_or(Value::Undefined),
                        steps: self.steps,
                    });
                }
            }
        }
    }

    fn pop(&mut self, instruction: usize) -> Result<Value, ExecutionError> {
        self.stack
            .pop()
            .ok_or(ExecutionError::StackUnderflow { instruction })
    }

    fn jump(&mut self, instruction: usize, target: u32) -> Result<(), ExecutionError> {
        let target = target as usize;
        if target >= self.program.code.len() {
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

    fn number(value: f64) -> Constant {
        Constant::Number(value.to_bits())
    }

    fn program(constants: Vec<Constant>, code: Vec<Instruction>) -> Program {
        Program {
            version: kiln_ir::BYTECODE_VERSION,
            constants,
            code,
            local_count: 0,
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
        let error = execute(&program, &mut host, ExecutionLimits { max_steps: 3 }).unwrap_err();
        assert_eq!(
            error,
            ExecutionError::StepLimitExceeded { steps: 3, limit: 3 }
        );
    }
}
