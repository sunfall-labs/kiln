//! A small, deterministic stack interpreter for Kiln's parser-independent IR.
//!
//! The interpreter deliberately knows nothing about the frontend.  It consumes only the owned
//! bytecode in `kiln-ir` and reaches the outside world through the versioned `HostOps` trait.  It
//! is intentionally a scalar runtime for the first execution slice; object allocation,
//! exceptions, and async scheduling belong above this VM.

use std::fmt;

use std::cell::RefCell;
use std::rc::Rc;

use kiln_host_abi::{HostOps, HostValue};
use kiln_ir::{BinaryOp, Constant, Instruction, ModuleProgram, Program, UnaryOp, Upvalue};

/// An ordinary object: string-keyed properties in insertion order.
///
/// A `Vec` rather than a map because objects here are small and property order is observable --
/// JavaScript enumerates string keys in insertion order, so the structure that preserves it for
/// free is the right one until a profile says otherwise. Lookup is linear, which is fine at this
/// size and is the kind of decision the timing harness exists to revisit.
/// Object identity is reference identity, which is what `===` asks of two objects. Structural
/// equality would call two distinct objects with equal contents the same object, and they are
/// not: assigning through one would then appear to change the other.
impl PartialEq for ObjectData {
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self, other)
    }
}

#[derive(Debug, Default)]
pub struct ObjectData {
    properties: Vec<(String, Value)>,
}

impl ObjectData {
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.properties
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value)
    }

    /// Insert or overwrite, preserving the position of an existing key.
    ///
    /// Re-assigning a property does not move it to the end in JavaScript, so it must not here.
    pub fn set(&mut self, key: &str, value: Value) {
        match self.properties.iter_mut().find(|(name, _)| name == key) {
            Some((_, existing)) => *existing = value,
            None => self.properties.push((key.to_owned(), value)),
        }
    }

    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.properties.iter().map(|(name, _)| name.as_str())
    }
}

/// A callable value: which function body to run, and the bindings it closed over.
#[derive(Debug)]
pub struct Closure {
    pub function: u32,
    /// One cell per [`kiln_ir::FunctionProto::upvalues`] entry, in the same order.
    pub upvalues: Vec<Cell>,
}

/// A shared, mutable binding.
///
/// Captured variables are shared rather than copied because JavaScript closes over the binding,
/// not the value: two closures over the same `let` must observe each other's writes, and so must
/// the enclosing function after they run.
pub type Cell = Rc<RefCell<Value>>;

/// Two closures are equal when they are the same object, which is what `===` asks.
///
/// Structural equality would be wrong: two closures over different cells can have identical
/// protos and identical current values without being the same function.
impl PartialEq for Closure {
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self, other)
    }
}

/// A JavaScript scalar value represented by the interpreter.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Undefined,
    Null,
    Boolean(bool),
    Number(f64),
    String(String),
    /// A shared binding living in a local slot, put there by `NewCell`.
    ///
    /// Never observable from JavaScript: the compiler only emits cell instructions for slots it
    /// boxed, so a cell is always read through `LoadCell`/`LoadUpvalue`, never pushed as a value.
    /// It is a variant of `Value` rather than a separate slot type so that a frame's locals stay
    /// one homogeneous vector.
    Cell(Cell),
    /// An object. Shared by `Rc` because objects have identity: `a === b` asks whether two
    /// references name the same object, and a mutation through one must be visible through the
    /// other.
    Object(Rc<RefCell<ObjectData>>),
    /// A callable: a proto plus the cells it captured when it was created.
    ///
    /// Shared by `Rc` because two references to the same closure must be the same object --
    /// `f === f` -- and because a closure outlives the frame that produced it. Reference cycles
    /// through captured cells are not collected; that is a real limitation and the right fix is
    /// a collector, not a weaker representation here.
    Function(Rc<Closure>),
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
            Self::Object(_) => "object",
            Self::Cell(_) => "cell",
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
            // `Number({})` is NaN: ToPrimitive on a plain object yields "[object Object]",
            // which does not parse as a number.
            Self::Object(_) => f64::NAN,
            Self::Cell(_) => unreachable!("a cell is never used as a value"),
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
            // Every object is truthy, including an empty one.
            Self::Object(_) => true,
            Self::Cell(_) => unreachable!("a cell is never used as a value"),
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
            // Not `Function.prototype.toString`, which returns source text the VM never sees.
            // The proto index is stable and useful when reading a trace; a made-up body is not.
            Self::Function(closure) => format!("[Function {}]", closure.function),
            // What `String({})` produces. `console.log` in a real console prints the contents
            // instead, which needs a formatter the host side does not have yet.
            Self::Object(_) => "[object Object]".to_owned(),
            Self::Cell(_) => unreachable!("a cell is never used as a value"),
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
            Value::Function(closure) => Self::String(format!("[Function {}]", closure.function)),
            Value::Object(_) => Self::String("[object Object]".to_owned()),
            Value::Cell(_) => unreachable!("a cell is never passed to a host operation"),
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
    /// A property name that is not a string constant, which the compiler never emits.
    InvalidPropertyKey {
        instruction: usize,
        value: &'static str,
    },
    /// A property assignment on something that is not an object.
    NotAnObject {
        instruction: usize,
        value: &'static str,
    },
    /// A capture descriptor pointing past the creating closure's upvalues.
    MissingUpvalue {
        index: u32,
    },
    /// A capture or cell access on a slot that was never boxed.
    NotACell {
        slot: usize,
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
            Self::InvalidPropertyKey { instruction, value } => write!(
                formatter,
                "instruction {instruction} used a {value} as a property name"
            ),
            Self::NotAnObject { instruction, value } => write!(
                formatter,
                "instruction {instruction} set a property on a {value}, which is not an object"
            ),
            Self::MissingUpvalue { index } => {
                write!(formatter, "no captured cell at upvalue index {index}")
            }
            Self::NotACell { slot } => {
                write!(formatter, "local slot {slot} does not hold a captured cell")
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
    /// The closure that was running, or `None` for the module body.
    function: Option<Rc<Closure>>,
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
    /// The closure currently executing: `None` is the module body.
    function: Option<Rc<Closure>>,
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
                    let closure = self.make_closure(*index)?;
                    self.stack.push(Value::Function(closure));
                }
                Instruction::NewObject { count } => {
                    let count = *count as usize;
                    if self.stack.len() < count * 2 {
                        return Err(ExecutionError::StackUnderflow {
                            instruction: current,
                        });
                    }
                    let flat = self.stack.split_off(self.stack.len() - count * 2);
                    let mut object = ObjectData::default();
                    for pair in flat.chunks_exact(2) {
                        // A duplicate key in a literal keeps the last value and the first
                        // position, which is what `set` already does.
                        let Value::String(key) = &pair[0] else {
                            return Err(ExecutionError::InvalidPropertyKey {
                                instruction: current,
                                value: pair[0].type_name(),
                            });
                        };
                        object.set(key, pair[1].clone());
                    }
                    self.stack
                        .push(Value::Object(Rc::new(RefCell::new(object))));
                }
                Instruction::GetProperty(name) => {
                    let name = self.string_constant(*name, current)?;
                    let target = self.pop(current)?;
                    self.stack.push(self.property(&target, &name));
                }
                Instruction::SetProperty(name) => {
                    let name = self.string_constant(*name, current)?;
                    let value = self.pop(current)?;
                    let target = self.pop(current)?;
                    let Value::Object(object) = &target else {
                        // Assigning a property of a primitive is silently discarded in sloppy
                        // mode and a TypeError in strict mode. Modules are always strict, and
                        // there is nowhere to throw yet, so it is an execution error.
                        return Err(ExecutionError::NotAnObject {
                            instruction: current,
                            value: target.type_name(),
                        });
                    };
                    object.borrow_mut().set(&name, value.clone());
                    self.stack.push(value);
                }
                Instruction::NewCell(slot) => {
                    let slot = *slot as usize;
                    let cell = Rc::new(RefCell::new(Value::Undefined));
                    *self
                        .locals
                        .get_mut(slot)
                        .ok_or(ExecutionError::LocalOutOfBounds { index: slot })? =
                        Value::Cell(cell);
                }
                Instruction::LoadCell(slot) => {
                    let cell = self.cell(*slot as usize)?;
                    let value = cell.borrow().clone();
                    self.stack.push(value);
                }
                Instruction::StoreCell(slot) => {
                    let value = self.pop(current)?;
                    let cell = self.cell(*slot as usize)?;
                    *cell.borrow_mut() = value;
                }
                Instruction::LoadUpvalue(index) => {
                    let value = self.upvalue(*index)?.borrow().clone();
                    self.stack.push(value);
                }
                Instruction::StoreUpvalue(index) => {
                    let value = self.pop(current)?;
                    *self.upvalue(*index)?.borrow_mut() = value;
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
                    let Value::Function(closure) = callee else {
                        return Err(ExecutionError::NotCallable {
                            instruction: current,
                            value: callee.type_name(),
                        });
                    };
                    let closure = Rc::clone(closure);
                    self.call(closure, argc, current)?;
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
        match &self.function {
            None => &self.program.code,
            // Validated when the closure was created; a call never installs a bad index.
            Some(closure) => &self.program.functions[closure.function as usize].code,
        }
    }

    /// Build a closure, resolving each upvalue descriptor against the frame creating it.
    fn make_closure(&mut self, function: u32) -> Result<Rc<Closure>, ExecutionError> {
        let Some(proto) = self.program.functions.get(function as usize) else {
            return Err(ExecutionError::UnknownFunction { function });
        };
        let mut upvalues = Vec::with_capacity(proto.upvalues.len());
        for descriptor in &proto.upvalues {
            let cell = match descriptor {
                Upvalue::ParentLocal(slot) => self.cell(*slot as usize)?,
                Upvalue::ParentUpvalue(index) => {
                    let Some(current) = &self.function else {
                        // Only a nested function has a parent closure to capture from, and the
                        // compiler never emits ParentUpvalue at module level.
                        return Err(ExecutionError::MissingUpvalue { index: *index });
                    };
                    current
                        .upvalues
                        .get(*index as usize)
                        .cloned()
                        .ok_or(ExecutionError::MissingUpvalue { index: *index })?
                }
            };
            upvalues.push(cell);
        }
        Ok(Rc::new(Closure { function, upvalues }))
    }

    /// A property read. Missing properties are `undefined`, which is not an error in JavaScript.
    ///
    /// Reads off a primitive currently yield `undefined` rather than consulting a prototype:
    /// `"abc".length` needs `String.prototype`, which does not exist yet. Returning undefined is
    /// wrong for those, and it is wrong in the direction that shows up as a visibly missing
    /// value rather than a plausible one.
    fn property(&self, target: &Value, name: &str) -> Value {
        match target {
            Value::Object(object) => object
                .borrow()
                .get(name)
                .cloned()
                .unwrap_or(Value::Undefined),
            _ => Value::Undefined,
        }
    }

    fn string_constant(&self, index: u32, instruction: usize) -> Result<String, ExecutionError> {
        match self.program.constants.get(index as usize) {
            Some(Constant::String(name)) => Ok(name.clone()),
            Some(_) => Err(ExecutionError::InvalidPropertyKey {
                instruction,
                value: "non-string constant",
            }),
            None => Err(ExecutionError::ConstantOutOfBounds {
                index: index as usize,
            }),
        }
    }

    /// The cell at this closure's upvalue index.
    fn upvalue(&self, index: u32) -> Result<Cell, ExecutionError> {
        self.function
            .as_ref()
            .and_then(|closure| closure.upvalues.get(index as usize))
            .cloned()
            .ok_or(ExecutionError::MissingUpvalue { index })
    }

    /// The cell held in a local slot, for capture.
    fn cell(&self, slot: usize) -> Result<Cell, ExecutionError> {
        match self.locals.get(slot) {
            Some(Value::Cell(cell)) => Ok(Rc::clone(cell)),
            // `NewCell` runs before anything can capture the slot, so a non-cell here means the
            // compiler emitted a capture of a binding it never boxed.
            Some(_) | None => Err(ExecutionError::NotACell { slot }),
        }
    }

    /// Enter `function` with `argc` arguments sitting on top of the stack.
    ///
    /// Missing arguments become `undefined` and extra ones are discarded, which is what calling
    /// a JavaScript function with the wrong arity does. Neither is an error, so neither is
    /// reported as one.
    fn call(
        &mut self,
        closure: Rc<Closure>,
        argc: u16,
        instruction: usize,
    ) -> Result<(), ExecutionError> {
        let function = closure.function;
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
            function: self.function.take(),
            locals: std::mem::replace(&mut self.locals, locals),
            instruction: self.instruction,
            stack_base: self.stack.len(),
        });
        self.function = Some(closure);
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
        // Objects and functions compare by identity: `===` asks whether two references name the
        // same object, not whether their contents match. `Rc::ptr_eq` is that question exactly,
        // and it also avoids borrowing a `RefCell` that a caller may already hold mutably.
        (Value::Object(a), Value::Object(b)) => Rc::ptr_eq(a, b),
        (Value::Function(a), Value::Function(b)) => Rc::ptr_eq(a, b),
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
                upvalues: Vec::new(),
            }],
        }
    }

    fn run(program: &Program) -> Result<Value, ExecutionError> {
        let mut host = TestHost { calls: Vec::new() };
        execute(program, &mut host, ExecutionLimits::default()).map(|result| result.value)
    }

    /// Two closures made by separate calls must not share a cell, and two made by one call must.
    /// This is the property that separates capturing a binding from capturing a frame.
    fn object(pairs: &[(&str, Value)]) -> Value {
        let mut data = ObjectData::default();
        for (key, value) in pairs {
            data.set(key, value.clone());
        }
        Value::Object(Rc::new(RefCell::new(data)))
    }

    /// Re-assigning a key keeps its position: JavaScript enumerates string keys in insertion
    /// order, so a `set` that moved the key would change an observable property of the object.
    #[test]
    fn reassigning_a_property_keeps_its_position() {
        let mut data = ObjectData::default();
        data.set("first", Value::Number(1.0));
        data.set("second", Value::Number(2.0));
        data.set("first", Value::Number(99.0));
        assert_eq!(data.keys().collect::<Vec<_>>(), ["first", "second"]);
        assert_eq!(data.get("first"), Some(&Value::Number(99.0)));
    }

    /// `===` on objects asks whether two references name the same object. Structural equality
    /// would say two distinct objects with equal contents are the same, and then a write through
    /// one would appear to change the other.
    #[test]
    fn object_equality_is_identity_not_structure() {
        let left = object(&[("a", Value::Number(1.0))]);
        let right = object(&[("a", Value::Number(1.0))]);
        assert!(strict_equal(&left, &left), "an object equals itself");
        assert!(
            !strict_equal(&left, &right),
            "equal contents are not the same object"
        );
        assert!(strict_equal(&left, &left.clone()), "a clone shares the Rc");
    }

    /// A missing property is `undefined`, not an error: reading one is legal JavaScript.
    #[test]
    fn a_missing_property_reads_as_undefined() {
        let data = ObjectData::default();
        assert_eq!(data.get("absent"), None);
    }

    /// Assigning a property of a primitive is a TypeError in strict mode, and modules are always
    /// strict. Silently discarding it would lose a write the program believes it made.
    #[test]
    fn setting_a_property_on_a_primitive_is_rejected() {
        let program = Program {
            version: kiln_ir::BYTECODE_VERSION,
            constants: vec![Constant::String("x".to_owned()), number(1.0)],
            code: vec![
                Instruction::Const(1),
                Instruction::Const(1),
                Instruction::SetProperty(0),
                Instruction::Return,
            ],
            local_count: 0,
            functions: Vec::new(),
        };
        assert!(matches!(
            run(&program),
            Err(ExecutionError::NotAnObject {
                value: "number",
                ..
            })
        ));
    }

    #[test]
    fn closures_from_separate_calls_capture_separate_cells() {
        // outer(): let c = <cell>; return closure(inner) -- inner increments and returns c.
        let program = Program {
            version: kiln_ir::BYTECODE_VERSION,
            constants: vec![number(1.0)],
            code: vec![
                Instruction::MakeFunction(0),
                Instruction::Call { argc: 0 },
                Instruction::Call { argc: 0 },
                Instruction::MakeFunction(0),
                Instruction::Call { argc: 0 },
                Instruction::Call { argc: 0 },
                Instruction::Return,
            ],
            local_count: 0,
            functions: vec![
                kiln_ir::FunctionProto {
                    name: Some("outer".to_owned()),
                    parameter_count: 0,
                    local_count: 1,
                    code: vec![
                        Instruction::NewCell(0),
                        Instruction::Const(0),
                        Instruction::StoreCell(0),
                        Instruction::MakeFunction(1),
                        Instruction::Return,
                    ],
                    upvalues: Vec::new(),
                },
                kiln_ir::FunctionProto {
                    name: Some("inner".to_owned()),
                    parameter_count: 0,
                    local_count: 0,
                    code: vec![
                        Instruction::LoadUpvalue(0),
                        Instruction::Const(0),
                        Instruction::Binary(BinaryOp::Add),
                        Instruction::Dup,
                        Instruction::StoreUpvalue(0),
                        Instruction::Return,
                    ],
                    upvalues: vec![Upvalue::ParentLocal(0)],
                },
            ],
        };
        // Each outer() call makes a fresh cell seeded to 1, so each inner() returns 2.
        assert_eq!(run(&program), Ok(Value::Number(2.0)));
    }

    /// A closure keeps working after the call that created it has returned, which is the whole
    /// reason captures are cells rather than pointers into a frame.
    #[test]
    fn a_captured_cell_outlives_the_frame_that_created_it() {
        let program = Program {
            version: kiln_ir::BYTECODE_VERSION,
            constants: vec![number(5.0)],
            code: vec![
                Instruction::MakeFunction(0),
                Instruction::Call { argc: 0 },
                Instruction::Call { argc: 0 },
                Instruction::Return,
            ],
            local_count: 0,
            functions: vec![
                kiln_ir::FunctionProto {
                    name: Some("outer".to_owned()),
                    parameter_count: 0,
                    local_count: 1,
                    code: vec![
                        Instruction::NewCell(0),
                        Instruction::Const(0),
                        Instruction::StoreCell(0),
                        Instruction::MakeFunction(1),
                        Instruction::Return,
                    ],
                    upvalues: Vec::new(),
                },
                kiln_ir::FunctionProto {
                    name: Some("read".to_owned()),
                    parameter_count: 0,
                    local_count: 0,
                    code: vec![Instruction::LoadUpvalue(0), Instruction::Return],
                    upvalues: vec![Upvalue::ParentLocal(0)],
                },
            ],
        };
        assert_eq!(run(&program), Ok(Value::Number(5.0)));
    }

    #[test]
    fn capturing_a_slot_that_was_never_boxed_is_rejected() {
        let program = Program {
            version: kiln_ir::BYTECODE_VERSION,
            constants: Vec::new(),
            code: vec![Instruction::MakeFunction(0), Instruction::Return],
            local_count: 1,
            functions: vec![kiln_ir::FunctionProto {
                name: None,
                parameter_count: 0,
                local_count: 0,
                code: vec![Instruction::Return],
                upvalues: vec![Upvalue::ParentLocal(0)],
            }],
        };
        assert!(matches!(
            run(&program),
            Err(ExecutionError::NotACell { slot: 0 })
        ));
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
                upvalues: Vec::new(),
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
                    upvalues: Vec::new(),
                },
                kiln_ir::FunctionProto {
                    name: Some("read".to_owned()),
                    parameter_count: 0,
                    local_count: 0,
                    code: vec![Instruction::LoadGlobal(0), Instruction::Return],
                    upvalues: Vec::new(),
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
