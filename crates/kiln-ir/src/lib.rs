//! A small, parser-independent stack bytecode for executable Kiln modules.
//!
//! The frontend owns the parser-specific lowering into [`kiln_hir`].  This crate deliberately
//! consumes only that owned HIR and produces plain data, so an interpreter, serializer, or native
//! backend can be added without coupling it to Oxc (or to any other parser).

use std::collections::{HashMap, HashSet};

use kiln_hir::{
    AssignmentOperator, BinaryOperator, Class, ExportName, ExportOrigin, Expression,
    ExpressionKind, Function, ImportName, Literal, LogicalOperator, Module, ObjectKey, Span,
    Statement, StatementKind, UnaryOperator, VariableKind,
};

/// Host operation identifiers are shared with the runtime through the stable ABI crate.
pub use kiln_host_abi::HostOp;

/// Version of the bytecode contract produced by this crate.
pub const BYTECODE_VERSION: u16 = 1;

/// A constant embedded in a [`Program`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Constant {
    Undefined,
    Null,
    Boolean(bool),
    /// IEEE-754 `f64` bits; preserving bits makes program equality total (including NaN/-0).
    Number(u64),
    String(String),
}

/// Unary operations understood by the bytecode runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UnaryOp {
    Plus,
    Negate,
    Not,
    TypeOf,
}

/// Binary operations understood by the bytecode runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BinaryOp {
    In,
    Add,
    Subtract,
    Multiply,
    Divide,
    Remainder,
    Exponentiate,
    StrictEqual,
    StrictNotEqual,
    LessThan,
    LessThanOrEqual,
    GreaterThan,
    GreaterThanOrEqual,
}

/// Stack bytecode instructions.
///
/// Conditional jumps consume their condition.  The compiler uses [`Instruction::Dup`] before a
/// short-circuit jump when the condition must remain as the expression's result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Instruction {
    /// Push a constant-table entry.
    Const(u32),
    LoadLocal(u32),
    StoreLocal(u32),
    Pop,
    Dup,
    /// Duplicate the top two values, preserving their order.
    ///
    /// Needed by compound computed assignment, where the object and key must be read once and
    /// used twice. Doing it with two `Dup`s would reverse them.
    Dup2,
    Unary(UnaryOp),
    Binary(BinaryOp),
    Jump(u32),
    JumpIfFalse(u32),
    JumpIfTruthy(u32),
    JumpIfNotNullish(u32),
    HostCall {
        op: HostOp,
        argc: u16,
    },
    /// Push a callable for `Program::functions[index]`.
    MakeFunction(u32),
    /// Call the value `argc` slots below the top of the stack, consuming it and its arguments
    /// and leaving the result.
    Call {
        argc: u16,
    },
    /// Build an array from `count` values on the stack, in source order.
    NewArray {
        count: u32,
    },
    /// Read `object[key]`. Stack is object, then key.
    GetComputed,
    /// Call `object.name(...)` with the object as receiver. Stack is object, then arguments.
    ///
    /// A method call cannot be a `GetProperty` followed by a `Call`: that discards the receiver,
    /// and a property read must yield the *unbound* function so that `a.f === a.f` holds. Keeping
    /// the receiver on the stack until the call is what lets both be true at once.
    CallMethod {
        name: u32,
        argc: u16,
    },
    /// `object[key](...)`, the computed form of `CallMethod`.
    CallComputed {
        argc: u16,
    },
    /// Write `object[key] = value`. Stack is object, key, value.
    SetComputed,
    /// Push the current `this`.
    LoadThis,
    /// Push a built-in global.
    ///
    /// Built-ins are named by index rather than resolved by string at runtime because the
    /// compiler already knows which one it means: a global lookup that could fail at runtime
    /// would be a worse contract for no benefit while the set is fixed.
    LoadBuiltin(Builtin),
    /// `new callee(...)` with `argc` arguments above the callee on the stack.
    New {
        argc: u16,
    },
    /// Build an object from `count` key/value pairs on the stack, keys first in each pair and
    /// the whole run in source order.
    NewObject {
        count: u32,
    },
    /// Read a property, naming it by constant index. The object is on top of the stack.
    ///
    /// The name is a constant rather than an inline string so the constant table deduplicates
    /// it: `.length` appearing two hundred times costs one entry.
    GetProperty(u32),
    /// Write a property. Stack is object, then value.
    SetProperty(u32),
    /// Put a fresh cell in a local slot, so a nested function can capture the *binding* rather
    /// than a copy of its value.
    ///
    /// Emitted only for locals a nested function actually refers to. Everything else stays an
    /// ordinary slot, so the indirection is paid exactly where the semantics require it.
    NewCell(u32),
    /// Read through the cell in a local slot.
    LoadCell(u32),
    /// Write through the cell in a local slot, so every closure sharing it observes the write.
    StoreCell(u32),
    /// Read the cell captured at this closure's upvalue index.
    LoadUpvalue(u32),
    StoreUpvalue(u32),
    /// Read a module-scope slot from inside a call frame.
    ///
    /// Distinct from `LoadLocal` because a frame's locals are its own: module bindings outlive
    /// every call and are shared by all of them, which is what module scope means in JavaScript
    /// rather than an approximation of it. Keeping the two apart also means a frame can never
    /// reach another frame's locals by accident.
    LoadGlobal(u32),
    StoreGlobal(u32),
    Return,
}

/// A compiled module body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Program {
    pub version: u16,
    pub constants: Vec<Constant>,
    pub code: Vec<Instruction>,
    pub local_count: u32,
    /// Compiled function bodies, referenced by [`Instruction::MakeFunction`].
    ///
    /// Held on the program rather than in the constant table because a constant is a value and
    /// a function is not: it becomes one only when `MakeFunction` runs, and once closures land
    /// the same proto will produce different callables on different evaluations.
    pub functions: Vec<FunctionProto>,
}

/// One compiled function body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionProto {
    /// Diagnostics and stack traces only; calls go by index.
    pub name: Option<String>,
    /// An arrow function: it has no `this` of its own and captures the defining scope's.
    pub arrow: bool,
    pub parameter_count: u32,
    /// Includes the parameters, which occupy the first slots in declaration order.
    pub local_count: u32,
    pub code: Vec<Instruction>,
    /// Where each captured cell comes from, resolved against the frame creating the closure.
    pub upvalues: Vec<Upvalue>,
}

/// A global the compiler resolves by name and the runtime provides.
///
/// This is the native half of design.md §5, which wants built-ins written in JavaScript. Nothing
/// can bootstrap a JS built-in yet -- that needs objects, prototypes and a loader, and two of
/// those have only just arrived -- so the ones the corpus needs first are native. The set is
/// deliberately tiny and each addition should be argued rather than assumed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Builtin {
    Error,
    Symbol,
    Array,
    Object,
    Json,
}

impl Builtin {
    /// The global name that resolves to this built-in.
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "Error" => Some(Self::Error),
            "Symbol" => Some(Self::Symbol),
            "Array" => Some(Self::Array),
            "Object" => Some(Self::Object),
            "JSON" => Some(Self::Json),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Error => "Error",
            Self::Symbol => "Symbol",
            Self::Array => "Array",
            Self::Object => "Object",
            Self::Json => "JSON",
        }
    }
}

/// Where a closure's captured cell is found when the closure is created.
///
/// Resolved against the *creating* frame, not the calling one: a closure captures the bindings
/// that were live where it was written, which is what makes a returned closure keep working
/// after its enclosing call has returned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Upvalue {
    /// A cell in the enclosing frame's local slot.
    ParentLocal(u32),
    /// A cell the enclosing closure had already captured, threaded one level further in.
    ParentUpvalue(u32),
}

/// A runtime import lowered to a local slot. The graph linker initializes the slot from the
/// provider module before evaluation; cyclic graphs are rejected by this first implementation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportBinding {
    pub source: String,
    pub imported: String,
    pub local: String,
    pub local_slot: u32,
}

/// A provider-visible name backed by one module-scope local slot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportBinding {
    pub exported: String,
    pub local_slot: u32,
}

/// Linkage-aware bytecode for one module. [`Program`] stays parser- and graph-independent so the
/// existing single-file interpreter and native backend APIs remain stable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleProgram {
    pub program: Program,
    /// Runtime evaluation dependencies, including side-effect-only imports, in source order.
    pub dependencies: Vec<String>,
    pub imports: Vec<ImportBinding>,
    pub exports: Vec<ExportBinding>,
}

/// A source-located compiler failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompileDiagnostic {
    pub message: String,
    pub span: Option<Span>,
}

/// Compile owned HIR into stack bytecode.
pub fn compile(module: &Module) -> Result<Program, Vec<CompileDiagnostic>> {
    let mut compiler = Compiler::new(module);

    for requested in module.runtime_requested_modules() {
        if let Some(occurrence) = requested.occurrences.first() {
            compiler.error(
                Some(occurrence.specifier_span),
                format!(
                    "module dependency `{}` is not supported by this execution backend",
                    requested.specifier
                ),
            );
        }
    }
    for export in &module.exports {
        if export.type_only {
            continue;
        }
        match &export.origin {
            ExportOrigin::ReExport { source, .. }
            | ExportOrigin::Star { source }
            | ExportOrigin::NamespaceReExport { source } => compiler.error(
                Some(export.span),
                format!("re-export from `{source}` is not supported by this execution backend"),
            ),
            ExportOrigin::Unsupported { description, .. } => compiler.error(
                Some(export.span),
                format!("unsupported export: {description}"),
            ),
            ExportOrigin::Local { .. } | ExportOrigin::DefaultExpression => {}
        }
    }

    compiler.compile_body(module);
    compiler.finish()
}

/// Compile one module while preserving the scalar linkage needed by a graph linker.
///
/// Named and default value imports are supported. Namespace imports, star/namespace re-exports,
/// indirect re-exports, default expression exports, and cycles remain explicit unsupported forms.
pub fn compile_module(module: &Module) -> Result<ModuleProgram, Vec<CompileDiagnostic>> {
    let mut compiler = Compiler::new(module);
    let mut imports = Vec::new();

    for import in module.imports.iter().filter(|import| !import.type_only) {
        let imported = match &import.imported {
            ImportName::Name(name) => name.clone(),
            ImportName::Default => "default".to_owned(),
            ImportName::Namespace => {
                compiler.error(
                    Some(import.span),
                    format!("namespace import `{}` is not supported yet", import.local),
                );
                continue;
            }
        };
        if let Some(local_slot) = compiler.declare_import(&import.local, import.span) {
            imports.push(ImportBinding {
                source: import.source.clone(),
                imported,
                local: import.local.clone(),
                local_slot,
            });
        }
    }

    compiler.compile_body(module);

    let mut exports = Vec::new();
    for export in module.exports.iter().filter(|export| !export.type_only) {
        let Some(exported) = export.exported.as_ref().map(ExportName::as_str) else {
            if !matches!(export.origin, ExportOrigin::Star { .. }) {
                compiler.error(Some(export.span), "runtime export has no exported name");
            }
            continue;
        };
        match &export.origin {
            ExportOrigin::Local { local } => match compiler.resolve_module_local(local) {
                Some(local_slot) => exports.push(ExportBinding {
                    exported: exported.to_owned(),
                    local_slot,
                }),
                None => compiler.error(
                    Some(export.span),
                    format!("export `{exported}` refers to missing local `{local}`"),
                ),
            },
            ExportOrigin::DefaultExpression => compiler.error(
                Some(export.span),
                "default expression exports are not supported yet",
            ),
            ExportOrigin::ReExport { source, .. } => compiler.error(
                Some(export.span),
                format!("re-export from `{source}` is not supported yet"),
            ),
            ExportOrigin::Star { source } => compiler.error(
                Some(export.span),
                format!("star re-export from `{source}` is not supported yet"),
            ),
            ExportOrigin::NamespaceReExport { source } => compiler.error(
                Some(export.span),
                format!("namespace re-export from `{source}` is not supported yet"),
            ),
            ExportOrigin::Unsupported { description, .. } => compiler.error(
                Some(export.span),
                format!("unsupported export: {description}"),
            ),
        }
    }

    let mut dependencies = Vec::new();
    for request in module.runtime_requested_modules() {
        if !dependencies.contains(&request.specifier) {
            dependencies.push(request.specifier.clone());
        }
    }

    compiler.finish().map(|program| ModuleProgram {
        program,
        dependencies,
        imports,
        exports,
    })
}

#[derive(Clone, Copy)]
struct Local {
    slot: u32,
    kind: VariableKind,
    imported: bool,
    /// The slot holds a cell rather than the value, because a nested function refers to this
    /// name. Reads and writes go through `LoadCell`/`StoreCell` so every closure sharing the
    /// binding sees the same mutations.
    celled: bool,
}

/// The per-function half of [`Compiler`], saved and restored around a nested function body.
///
/// Constants and diagnostics are deliberately not in here: they are program-wide, and splitting
/// them would mean merging them back with index fixups for no benefit.
struct FunctionState {
    code: Vec<Instruction>,
    scopes: Vec<HashMap<String, Local>>,
    local_count: u32,
    upvalues: Vec<Upvalue>,
    /// Names already captured, so a name referenced twice captures one cell rather than two.
    upvalue_names: HashMap<String, u32>,
    /// Names this function's nested functions refer to, so declarations know to box themselves.
    captured: HashSet<String>,
}

/// Where a name lives, which decides how it is read and whether it can be read at all.
enum Resolution {
    /// A binding in the function currently being compiled.
    Local(Local),
    /// A module-scope binding, reached from inside a call frame.
    Global(u32),
    /// A binding in an enclosing *function*. That is a closure capture, and compiling it as
    /// either of the above would read an unrelated slot -- silently, and with a plausible value
    /// in it. Refused until upvalues exist.
    /// A binding in an enclosing function, reached through this closure's captured cells.
    Upvalue(u32),
    Unresolved,
}

struct Compiler {
    constants: Vec<Constant>,
    functions: Vec<FunctionProto>,
    /// Enclosing function states, module body first. Empty while compiling the module body.
    enclosing: Vec<FunctionState>,
    upvalues: Vec<Upvalue>,
    upvalue_names: HashMap<String, u32>,
    captured: HashSet<String>,
    code: Vec<Instruction>,
    /// Lexical scopes of the function being compiled, innermost last. At module level the first
    /// entry is module scope itself.
    scopes: Vec<HashMap<String, Local>>,
    /// Slots are never recycled: bytecode can jump back into a block in a loop, and a stable
    /// slot keeps that execution independent of compiler scope bookkeeping.
    local_count: u32,
    diagnostics: Vec<CompileDiagnostic>,
}

impl Compiler {
    fn new(module: &Module) -> Self {
        let mut compiler = Self {
            constants: Vec::new(),
            code: Vec::new(),
            functions: Vec::new(),
            enclosing: Vec::new(),
            upvalues: Vec::new(),
            upvalue_names: HashMap::new(),
            captured: HashSet::new(),
            scopes: vec![HashMap::new()],
            local_count: 0,
            diagnostics: Vec::new(),
        };
        if module.panicked {
            compiler.error(None, "frontend could not produce an executable module");
        }
        for diagnostic in &module.diagnostics {
            compiler.diagnostics.push(CompileDiagnostic {
                message: diagnostic.message.clone(),
                span: diagnostic.span,
            });
        }
        compiler
    }

    fn compile_body(&mut self, module: &Module) {
        for statement in &module.body {
            self.statement(statement);
        }
        self.push_constant(Constant::Undefined);
        self.code.push(Instruction::Return);
    }

    fn finish(self) -> Result<Program, Vec<CompileDiagnostic>> {
        if self.diagnostics.is_empty() {
            Ok(Program {
                version: BYTECODE_VERSION,
                constants: self.constants,
                code: self.code,
                local_count: self.local_count,
                functions: self.functions,
            })
        } else {
            Err(self.diagnostics)
        }
    }

    fn declare_import(&mut self, name: &str, span: Span) -> Option<u32> {
        if self.scopes[0].contains_key(name) {
            self.error(Some(span), format!("duplicate module binding `{name}`"));
            return None;
        }
        let slot = self.local_count;
        self.local_count += 1;
        self.scopes[0].insert(
            name.to_owned(),
            Local {
                slot,
                kind: VariableKind::Const,
                imported: true,
                celled: false,
            },
        );
        Some(slot)
    }

    fn resolve_module_local(&self, name: &str) -> Option<u32> {
        self.scopes[0].get(name).map(|local| local.slot)
    }

    fn error(&mut self, span: Option<Span>, message: impl Into<String>) {
        self.diagnostics.push(CompileDiagnostic {
            message: message.into(),
            span,
        });
    }

    fn statement(&mut self, statement: &Statement) {
        match &statement.kind {
            StatementKind::Variable { kind, declarations } => {
                for declaration in declarations {
                    self.variable(
                        *kind,
                        declaration.name.as_str(),
                        declaration.initializer.as_ref(),
                        declaration.span,
                    );
                }
            }
            StatementKind::Expression(expression) => {
                self.expression(expression);
                self.code.push(Instruction::Pop);
            }
            StatementKind::Block(statements) => {
                self.scopes.push(HashMap::new());
                for statement in statements {
                    self.statement(statement);
                }
                self.scopes.pop().expect("block scope was just pushed");
            }
            StatementKind::If {
                test,
                consequent,
                alternate,
            } => self.if_statement(test, consequent, alternate.as_deref()),
            StatementKind::While { test, body } => self.while_statement(test, body),
            StatementKind::Empty => {}
            StatementKind::Function(function) => {
                self.function_declaration(function, statement.span)
            }
            StatementKind::Return(value) => self.return_statement(value.as_ref(), statement.span),
            StatementKind::Class(class) => self.class_declaration(class),
            StatementKind::ExportDefault(_) => self.error(
                Some(statement.span),
                "default expression exports are not supported yet",
            ),
            StatementKind::Unsupported { feature } => self.error(
                Some(statement.span),
                format!("unsupported statement: {feature}"),
            ),
        }
    }

    fn variable(
        &mut self,
        kind: VariableKind,
        name: &str,
        initializer: Option<&Expression>,
        span: Span,
    ) {
        if name.is_empty() {
            self.error(Some(span), "destructuring bindings are not supported yet");
            return;
        }
        if kind != VariableKind::Var
            && initializer.is_some_and(|initializer| expression_references(initializer, name))
        {
            self.error(
                Some(span),
                format!("temporal-dead-zone access in initializer for `{name}` is not supported"),
            );
            return;
        }

        let scope_index = if kind == VariableKind::Var {
            0
        } else {
            self.scopes.len() - 1
        };
        if let Some(existing) = self.scopes[scope_index].get(name).copied() {
            if kind == VariableKind::Var && existing.kind == VariableKind::Var {
                if let Some(initializer) = initializer {
                    self.expression(initializer);
                    self.code.push(if existing.celled {
                        Instruction::StoreCell(existing.slot)
                    } else {
                        Instruction::StoreLocal(existing.slot)
                    });
                }
                return;
            }
            self.error(
                Some(span),
                format!("redeclaration of local `{name}` is not supported"),
            );
            return;
        }
        if kind == VariableKind::Const && initializer.is_none() {
            self.error(
                Some(span),
                format!("const local `{name}` requires an initializer"),
            );
        }
        let slot = self.local_count;
        self.local_count += 1;
        // Boxed only when a nested function refers to this name. `self.captured` is empty while
        // compiling the module body, so module bindings -- which functions reach as globals --
        // never pay for a cell.
        let celled = self.captured.contains(name);
        self.scopes[scope_index].insert(
            name.to_owned(),
            Local {
                slot,
                kind,
                imported: false,
                celled,
            },
        );
        if celled {
            self.code.push(Instruction::NewCell(slot));
        }
        if let Some(initializer) = initializer {
            self.expression(initializer);
        } else {
            self.push_constant(Constant::Undefined);
        }
        self.code.push(if celled {
            Instruction::StoreCell(slot)
        } else {
            Instruction::StoreLocal(slot)
        });
    }

    fn if_statement(
        &mut self,
        test: &Expression,
        consequent: &Statement,
        alternate: Option<&Statement>,
    ) {
        self.expression(test);
        let false_jump = self.emit_jump(Instruction::JumpIfFalse(0));
        self.statement(consequent);
        if let Some(alternate) = alternate {
            let end_jump = self.emit_jump(Instruction::Jump(0));
            self.patch_jump(false_jump);
            self.statement(alternate);
            self.patch_jump(end_jump);
        } else {
            self.patch_jump(false_jump);
        }
    }

    fn while_statement(&mut self, test: &Expression, body: &Statement) {
        let loop_start = self.code.len();
        self.expression(test);
        let exit_jump = self.emit_jump(Instruction::JumpIfFalse(0));
        self.statement(body);
        self.code.push(Instruction::Jump(loop_start as u32));
        self.patch_jump(exit_jump);
    }

    fn expression(&mut self, expression: &Expression) {
        match &expression.kind {
            ExpressionKind::Literal(literal) => self.push_literal(literal),
            ExpressionKind::Identifier(name) => match self.resolve_or_capture(name) {
                Resolution::Local(local) if local.celled => {
                    self.code.push(Instruction::LoadCell(local.slot))
                }
                Resolution::Local(local) => self.code.push(Instruction::LoadLocal(local.slot)),
                Resolution::Global(slot) => self.code.push(Instruction::LoadGlobal(slot)),
                Resolution::Upvalue(index) => self.code.push(Instruction::LoadUpvalue(index)),
                Resolution::Unresolved if name == "this" => self.code.push(Instruction::LoadThis),
                Resolution::Unresolved if name == "undefined" => {
                    self.push_constant(Constant::Undefined)
                }
                // A built-in is only reachable when nothing else bound the name, so a program
                // that declares its own `Error` shadows it exactly as it would any other global.
                Resolution::Unresolved => match Builtin::from_name(name) {
                    Some(builtin) => self.code.push(Instruction::LoadBuiltin(builtin)),
                    None => self.error(
                        Some(expression.span),
                        format!("unresolved identifier `{name}`"),
                    ),
                },
            },
            ExpressionKind::Unary { operator, argument } => {
                self.expression(argument);
                self.code.push(Instruction::Unary(match operator {
                    UnaryOperator::Plus => UnaryOp::Plus,
                    UnaryOperator::Negate => UnaryOp::Negate,
                    UnaryOperator::Not => UnaryOp::Not,
                    UnaryOperator::TypeOf => UnaryOp::TypeOf,
                }));
            }
            ExpressionKind::Binary {
                operator,
                left,
                right,
            } => {
                self.expression(left);
                self.expression(right);
                self.code.push(Instruction::Binary(match operator {
                    BinaryOperator::Add => BinaryOp::Add,
                    BinaryOperator::Subtract => BinaryOp::Subtract,
                    BinaryOperator::Multiply => BinaryOp::Multiply,
                    BinaryOperator::Divide => BinaryOp::Divide,
                    BinaryOperator::Remainder => BinaryOp::Remainder,
                    BinaryOperator::Exponentiate => BinaryOp::Exponentiate,
                    BinaryOperator::StrictEqual => BinaryOp::StrictEqual,
                    BinaryOperator::In => BinaryOp::In,
                    BinaryOperator::StrictNotEqual => BinaryOp::StrictNotEqual,
                    BinaryOperator::LessThan => BinaryOp::LessThan,
                    BinaryOperator::LessThanOrEqual => BinaryOp::LessThanOrEqual,
                    BinaryOperator::GreaterThan => BinaryOp::GreaterThan,
                    BinaryOperator::GreaterThanOrEqual => BinaryOp::GreaterThanOrEqual,
                }));
            }
            ExpressionKind::Logical {
                operator,
                left,
                right,
            } => self.logical(*operator, left, right),
            ExpressionKind::Assignment {
                operator,
                target,
                value,
            } => self.assignment(*operator, target, value, expression.span),
            ExpressionKind::Conditional {
                test,
                consequent,
                alternate,
            } => {
                self.expression(test);
                let false_jump = self.emit_jump(Instruction::JumpIfFalse(0));
                self.expression(consequent);
                let end_jump = self.emit_jump(Instruction::Jump(0));
                self.patch_jump(false_jump);
                self.expression(alternate);
                self.patch_jump(end_jump);
            }
            ExpressionKind::Call { callee, arguments } => {
                self.call(callee, arguments, expression.span)
            }
            ExpressionKind::This => self.code.push(Instruction::LoadThis),
            ExpressionKind::Function(function) => {
                if let Some(index) = self.compile_function(function, expression.span) {
                    self.code.push(Instruction::MakeFunction(index));
                }
            }
            ExpressionKind::Array { elements } => {
                if elements.len() > u32::MAX as usize {
                    self.error(Some(expression.span), "array literal has too many elements");
                    return;
                }
                for element in elements {
                    self.expression(element);
                }
                self.code.push(Instruction::NewArray {
                    count: elements.len() as u32,
                });
            }
            ExpressionKind::ComputedMember { object, property } => {
                self.expression(object);
                self.expression(property);
                self.code.push(Instruction::GetComputed);
            }
            ExpressionKind::ComputedAssignment {
                operator,
                object,
                property,
                value,
            } => self.computed_assignment(*operator, object, property, value),
            ExpressionKind::New { callee, arguments } => {
                if arguments.len() > u16::MAX as usize {
                    self.error(Some(expression.span), "too many constructor arguments");
                    return;
                }
                self.expression(callee);
                for argument in arguments {
                    self.expression(argument);
                }
                self.code.push(Instruction::New {
                    argc: arguments.len() as u16,
                });
            }
            ExpressionKind::MemberAssignment {
                operator,
                object,
                property,
                value,
            } => self.member_assignment(*operator, object, property, value),
            ExpressionKind::Object { properties } => {
                if properties.len() > u32::MAX as usize {
                    self.error(
                        Some(expression.span),
                        "object literal has too many properties",
                    );
                    return;
                }
                for property in properties {
                    match &property.key {
                        ObjectKey::Static(key) => {
                            self.push_constant(Constant::String(key.clone()));
                        }
                        // Evaluated in source order, between the properties around it, because
                        // a computed key is an arbitrary expression and may have side effects.
                        ObjectKey::Computed(key) => self.expression(key),
                    }
                    self.expression(&property.value);
                }
                self.code.push(Instruction::NewObject {
                    count: properties.len() as u32,
                });
            }
            ExpressionKind::StaticMember { object, property } => {
                self.expression(object);
                let name = self.constant_index(Constant::String(property.clone()));
                self.code.push(Instruction::GetProperty(name));
            }
            ExpressionKind::Unsupported { feature } => {
                self.error(
                    Some(expression.span),
                    format!("unsupported expression: {feature}"),
                );
                self.push_constant(Constant::Undefined);
            }
        }
    }

    fn logical(&mut self, operator: LogicalOperator, left: &Expression, right: &Expression) {
        self.expression(left);
        self.code.push(Instruction::Dup);
        let jump = self.emit_jump(match operator {
            LogicalOperator::And => Instruction::JumpIfFalse(0),
            LogicalOperator::Or => Instruction::JumpIfTruthy(0),
            LogicalOperator::NullishCoalesce => Instruction::JumpIfNotNullish(0),
        });
        self.code.push(Instruction::Pop);
        self.expression(right);
        self.patch_jump(jump);
    }

    fn assignment(
        &mut self,
        operator: AssignmentOperator,
        target: &str,
        value: &Expression,
        span: Span,
    ) {
        // Whether the target is a frame local or a module slot decides the store instruction,
        // so it has to be carried alongside the slot rather than assumed.
        enum Place {
            Local(u32),
            Cell(u32),
            Upvalue(u32),
            Global(u32),
        }
        let (place, kind, imported) = match self.resolve_or_capture(target) {
            Resolution::Local(local) if local.celled => {
                (Place::Cell(local.slot), local.kind, local.imported)
            }
            Resolution::Local(local) => (Place::Local(local.slot), local.kind, local.imported),
            Resolution::Upvalue(index) => {
                // The owning declaration decides const-ness, and it was checked where the name
                // was captured; an upvalue carries no separate mutability of its own.
                (Place::Upvalue(index), VariableKind::Let, false)
            }
            Resolution::Global(slot) => {
                let module = &self.enclosing[0].scopes[0];
                let local = module
                    .get(target)
                    .copied()
                    .expect("Global resolution just found this name in module scope");
                (Place::Global(slot), local.kind, local.imported)
            }
            Resolution::Unresolved => {
                self.error(
                    Some(span),
                    format!("unresolved assignment target `{target}`"),
                );
                self.push_constant(Constant::Undefined);
                return;
            }
        };
        let (load, store) = match place {
            Place::Local(slot) => (Instruction::LoadLocal(slot), Instruction::StoreLocal(slot)),
            Place::Cell(slot) => (Instruction::LoadCell(slot), Instruction::StoreCell(slot)),
            Place::Upvalue(index) => (
                Instruction::LoadUpvalue(index),
                Instruction::StoreUpvalue(index),
            ),
            Place::Global(slot) => (
                Instruction::LoadGlobal(slot),
                Instruction::StoreGlobal(slot),
            ),
        };
        if imported {
            self.error(
                Some(span),
                format!("assignment to imported binding `{target}` is not allowed"),
            );
        } else if kind == VariableKind::Const {
            self.error(
                Some(span),
                format!("assignment to const local `{target}` is not supported"),
            );
        }
        if operator == AssignmentOperator::Assign {
            self.expression(value);
        } else {
            self.code.push(load);
            self.expression(value);
            let binary = match operator {
                AssignmentOperator::Add => BinaryOp::Add,
                AssignmentOperator::Subtract => BinaryOp::Subtract,
                AssignmentOperator::Multiply => BinaryOp::Multiply,
                AssignmentOperator::Divide => BinaryOp::Divide,
                AssignmentOperator::Remainder => BinaryOp::Remainder,
                AssignmentOperator::Exponentiate => BinaryOp::Exponentiate,
                AssignmentOperator::Assign => unreachable!(),
            };
            self.code.push(Instruction::Binary(binary));
        }
        self.code.push(Instruction::Dup);
        self.code.push(store);
    }

    /// Compile a class into a constructor and a prototype.
    ///
    /// The desugaring is not a shortcut -- a class really is a constructor function whose
    /// `prototype` holds the instance methods -- so it reuses the machinery `new` already needs
    /// rather than introducing a parallel object model for classes.
    fn class_declaration(&mut self, class: &Class) {
        let Some(name) = class.name.clone() else {
            self.error(Some(class.span), "a class declaration requires a name");
            return;
        };
        if class.superclass.is_some() {
            // A derived class needs `super`, a prototype chain between the two prototypes, and a
            // constructor that cannot touch `this` before calling `super()`. Half of that would
            // produce instances that look right and inherit nothing.
            self.error(
                Some(class.span),
                "class inheritance (`extends`) is not supported yet",
            );
            return;
        }

        // Bound before the body is compiled, so a method can refer to the class by name.
        self.variable(VariableKind::Let, &name, None, class.span);

        let constructor = match &class.constructor {
            Some(constructor) => constructor.clone(),
            // A class with no constructor gets an empty one. Synthesizing it here keeps `new`
            // from needing a special case for classes that declared none.
            None => Function {
                name: Some(name.clone()),
                arrow: false,
                parameters: Vec::new(),
                body: Vec::new(),
                span: class.span,
            },
        };
        let Some(index) = self.compile_function(&constructor, class.span) else {
            return;
        };

        let Resolution::Local(local) = self.resolve(&name) else {
            self.error(Some(class.span), format!("could not bind class `{name}`"));
            return;
        };
        self.code.push(Instruction::MakeFunction(index));
        self.code.push(if local.celled {
            Instruction::StoreCell(local.slot)
        } else {
            Instruction::StoreLocal(local.slot)
        });

        let prototype = self.constant_index(Constant::String("prototype".to_owned()));
        for method in &class.methods {
            let Some(body) = self.compile_function(&method.function, class.span) else {
                return;
            };

            // Static methods hang off the constructor; instance methods off its prototype.
            self.load_local_or_cell(local);
            if !method.is_static {
                self.code.push(Instruction::GetProperty(prototype));
            }

            match &method.key {
                ObjectKey::Static(key) => {
                    let name = self.constant_index(Constant::String(key.clone()));
                    self.code.push(Instruction::MakeFunction(body));
                    self.code.push(Instruction::SetProperty(name));
                }
                ObjectKey::Computed(key) => {
                    self.expression(key);
                    self.code.push(Instruction::MakeFunction(body));
                    self.code.push(Instruction::SetComputed);
                }
            }
            // Each assignment leaves its value; nothing consumes it.
            self.code.push(Instruction::Pop);
        }
    }

    fn load_local_or_cell(&mut self, local: Local) {
        self.code.push(if local.celled {
            Instruction::LoadCell(local.slot)
        } else {
            Instruction::LoadLocal(local.slot)
        });
    }

    /// Compile a function body into its own proto and bind the name to a callable.
    ///
    /// Declarations are not hoisted yet: the binding exists from this point on, so calling a
    /// function textually above its declaration is an unresolved identifier rather than a call
    /// to `undefined`. That is a smaller lie than hoisting half-way would be, and it is a
    /// diagnostic rather than a wrong answer.
    fn function_declaration(&mut self, function: &Function, span: Span) {
        let Some(name) = function.name.clone() else {
            self.error(Some(span), "a function declaration requires a name");
            return;
        };

        // The binding exists before the body is compiled, which is what makes a function able
        // to call itself: the body resolves its own name through the enclosing scope, and the
        // slot it reads is filled by the `MakeFunction` below before any call can reach it.
        self.variable(VariableKind::Let, &name, None, span);

        let Some(index) = self.compile_function(function, span) else {
            return;
        };

        match self.resolve(&name) {
            Resolution::Local(local) => {
                self.code.push(Instruction::MakeFunction(index));
                self.code.push(Instruction::StoreLocal(local.slot));
            }
            // `declare_variable` just inserted this into the current function's innermost scope,
            // so any other resolution means the scope bookkeeping is inconsistent.
            _ => self.error(Some(span), format!("could not bind function `{name}`")),
        }
    }

    /// Compile one function body, returning its index in [`Program::functions`].
    ///
    /// The body is compiled against a fresh state: its own code buffer, its own slot numbering
    /// starting at the parameters, and its own scope stack. Module scope stays reachable through
    /// `enclosing[0]`; anything between is a capture and is refused by `resolve`.
    fn compile_function(&mut self, function: &Function, span: Span) -> Option<u32> {
        if function.parameters.len() > u32::MAX as usize {
            self.error(Some(span), "too many parameters");
            return None;
        }
        if self.functions.len() >= u32::MAX as usize {
            self.error(Some(span), "too many functions in one module");
            return None;
        }

        // Which of this function's own bindings its nested functions refer to. Computed before
        // the body is compiled because a declaration has to know, at the point it allocates its
        // slot, whether that slot holds a value or a cell.
        let captured = names_captured_by_nested_functions(&function.body);

        let mut parameters = HashMap::with_capacity(function.parameters.len());
        let mut celled_parameters = Vec::new();
        for (slot, parameter) in function.parameters.iter().enumerate() {
            if parameters.contains_key(&parameter.name) {
                self.error(
                    Some(parameter.span),
                    format!("duplicate parameter `{}`", parameter.name),
                );
                return None;
            }
            let slot = slot as u32;
            let celled = captured.contains(&parameter.name);
            if celled {
                celled_parameters.push(slot);
            }
            parameters.insert(
                parameter.name.clone(),
                Local {
                    slot,
                    kind: VariableKind::Let,
                    imported: false,
                    celled,
                },
            );
        }

        let parameter_count = function.parameters.len() as u32;
        let outer = FunctionState {
            code: std::mem::take(&mut self.code),
            scopes: std::mem::replace(&mut self.scopes, vec![parameters]),
            local_count: std::mem::replace(&mut self.local_count, parameter_count),
            upvalues: std::mem::take(&mut self.upvalues),
            upvalue_names: std::mem::take(&mut self.upvalue_names),
            captured: std::mem::replace(&mut self.captured, captured),
        };
        self.enclosing.push(outer);

        // A captured parameter arrives as a plain value in its slot, so the cell has to be
        // created and seeded before the body can read it.
        for slot in celled_parameters {
            self.code.push(Instruction::LoadLocal(slot));
            self.code.push(Instruction::NewCell(slot));
            self.code.push(Instruction::StoreCell(slot));
        }

        for statement in &function.body {
            self.statement(statement);
        }
        // Falling off the end of a function returns undefined. Emitted unconditionally rather
        // than only when the body cannot fall through: proving that needs flow analysis, and an
        // unreachable epilogue costs two instructions.
        self.push_constant(Constant::Undefined);
        self.code.push(Instruction::Return);

        let restored = self
            .enclosing
            .pop()
            .expect("this function pushed its own enclosing state");
        let proto = FunctionProto {
            name: function.name.clone(),
            arrow: function.arrow,
            parameter_count,
            local_count: std::mem::replace(&mut self.local_count, restored.local_count),
            code: std::mem::replace(&mut self.code, restored.code),
            upvalues: std::mem::replace(&mut self.upvalues, restored.upvalues),
        };
        self.scopes = restored.scopes;
        self.upvalue_names = restored.upvalue_names;
        self.captured = restored.captured;

        let index = self.functions.len() as u32;
        self.functions.push(proto);
        Some(index)
    }

    fn return_statement(&mut self, value: Option<&Expression>, span: Span) {
        if !self.compiling_function() {
            self.error(Some(span), "`return` outside of a function");
            return;
        }
        match value {
            Some(value) => self.expression(value),
            None => self.push_constant(Constant::Undefined),
        }
        self.code.push(Instruction::Return);
    }

    /// `object.property = value`, and the compound forms.
    ///
    /// The object expression is evaluated once. `o().x += 1` must not call `o` twice, so the
    /// reference is duplicated on the stack rather than the expression being compiled twice --
    /// which would be the easy mistake and would double any side effect in the receiver.
    /// `object[key] = value`, and the compound forms.
    ///
    /// Both the object and the key are evaluated once, in that order, which is the order the
    /// language specifies and the one a reader assumes. A compound form has to re-read through
    /// the same pair rather than re-evaluating either, so both are duplicated on the stack.
    fn computed_assignment(
        &mut self,
        operator: AssignmentOperator,
        object: &Expression,
        property: &Expression,
        value: &Expression,
    ) {
        self.expression(object);
        self.expression(property);

        if operator != AssignmentOperator::Assign {
            self.code.push(Instruction::Dup2);
            self.code.push(Instruction::GetComputed);
            self.expression(value);
            self.code.push(Instruction::Binary(match operator {
                AssignmentOperator::Add => BinaryOp::Add,
                AssignmentOperator::Subtract => BinaryOp::Subtract,
                AssignmentOperator::Multiply => BinaryOp::Multiply,
                AssignmentOperator::Divide => BinaryOp::Divide,
                AssignmentOperator::Remainder => BinaryOp::Remainder,
                AssignmentOperator::Exponentiate => BinaryOp::Exponentiate,
                AssignmentOperator::Assign => unreachable!("handled above"),
            }));
        } else {
            self.expression(value);
        }

        self.code.push(Instruction::SetComputed);
    }

    fn member_assignment(
        &mut self,
        operator: AssignmentOperator,
        object: &Expression,
        property: &str,
        value: &Expression,
    ) {
        let name = self.constant_index(Constant::String(property.to_owned()));
        self.expression(object);

        if operator != AssignmentOperator::Assign {
            // Read the current value from the same object reference the write will use.
            self.code.push(Instruction::Dup);
            self.code.push(Instruction::GetProperty(name));
            self.expression(value);
            self.code.push(Instruction::Binary(match operator {
                AssignmentOperator::Add => BinaryOp::Add,
                AssignmentOperator::Subtract => BinaryOp::Subtract,
                AssignmentOperator::Multiply => BinaryOp::Multiply,
                AssignmentOperator::Divide => BinaryOp::Divide,
                AssignmentOperator::Remainder => BinaryOp::Remainder,
                AssignmentOperator::Exponentiate => BinaryOp::Exponentiate,
                AssignmentOperator::Assign => unreachable!("handled above"),
            }));
        } else {
            self.expression(value);
        }

        // `SetProperty` leaves the assigned value, which is what an assignment expression
        // evaluates to.
        self.code.push(Instruction::SetProperty(name));
    }

    fn call(&mut self, callee: &Expression, arguments: &[Expression], span: Span) {
        let is_console_log = matches!(self.resolve("console"), Resolution::Unresolved)
            && matches!(
                &callee.kind,
                ExpressionKind::StaticMember { object, property }
                    if property == "log"
                        && matches!(&object.kind, ExpressionKind::Identifier(name) if name == "console")
            );
        if arguments.len() > u16::MAX as usize {
            self.error(Some(span), "too many call arguments");
            return;
        }
        let argc = arguments.len() as u16;

        if is_console_log {
            // Still a compile-time shape rather than a value. `console` becomes an ordinary
            // object once there are objects to hold it; until then this is the only way to
            // reach stdout, and turning it into a real call would mean inventing a callable
            // with no object model underneath it.
            for argument in arguments {
                self.expression(argument);
            }
            self.code.push(Instruction::HostCall {
                op: HostOp::WriteStdout,
                argc,
            });
            return;
        }

        // A method call keeps its receiver rather than reading the function out first.
        match &callee.kind {
            ExpressionKind::StaticMember { object, property } => {
                let name = self.constant_index(Constant::String(property.clone()));
                self.expression(object);
                for argument in arguments {
                    self.expression(argument);
                }
                self.code.push(Instruction::CallMethod { name, argc });
                return;
            }
            ExpressionKind::ComputedMember { object, property } => {
                self.expression(object);
                self.expression(property);
                for argument in arguments {
                    self.expression(argument);
                }
                self.code.push(Instruction::CallComputed { argc });
                return;
            }
            _ => {}
        }

        // Everything else is a real call. The callee is evaluated first so it sits below its
        // arguments, which is both the evaluation order the language requires and the layout
        // the VM needs to find it at a fixed depth.
        self.expression(callee);
        for argument in arguments {
            self.expression(argument);
        }
        self.code.push(Instruction::Call { argc });
    }

    /// Names visible to the function being compiled, and how each is reached.
    fn resolve(&self, name: &str) -> Resolution {
        if let Some(local) = self.scopes.iter().rev().find_map(|scope| scope.get(name)) {
            return Resolution::Local(*local);
        }
        let Some(module) = self.enclosing.first() else {
            // Compiling the module body: its scopes were searched above, so there is nowhere
            // else a name could be.
            return Resolution::Unresolved;
        };
        if let Some(local) = module.scopes[0].get(name) {
            return Resolution::Global(local.slot);
        }
        Resolution::Unresolved
    }

    /// Resolve `name` and, if it lives in an enclosing function, capture it.
    ///
    /// Split from [`Compiler::resolve`] because capturing mutates the upvalue lists of every
    /// function between the binding and here, and a read-only lookup is wanted in places that
    /// must not have that side effect -- the `console` check in [`Compiler::call`] above all,
    /// which asks whether a name exists and would otherwise capture one by asking.
    fn resolve_or_capture(&mut self, name: &str) -> Resolution {
        match self.resolve(name) {
            Resolution::Unresolved => {}
            resolved => return resolved,
        }
        match self.capture(name) {
            Some(index) => Resolution::Upvalue(index),
            None => Resolution::Unresolved,
        }
    }

    /// Thread a binding from the function that owns it down to the one being compiled.
    ///
    /// Every function in between gains an upvalue too, because a closure can only reach cells
    /// its immediately enclosing frame holds -- a two-level capture is two hops, not one long
    /// one. Levels are indices into `enclosing` followed by the current function.
    fn capture(&mut self, name: &str) -> Option<u32> {
        // Level 0 is the module body, whose bindings are globals rather than captures.
        let owner = (1..self.enclosing.len()).rev().find(|level| {
            self.enclosing[*level]
                .scopes
                .iter()
                .any(|scope| scope.contains_key(name))
        })?;

        let local = self.enclosing[owner]
            .scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(name))
            .copied()
            .expect("the owning level was just located by this name");
        if !local.celled {
            // The capture analysis said no nested function referred to this name, yet here we
            // are. Compiling it anyway would read an uninitialised slot as though it were a
            // cell, so refuse rather than emit that.
            return None;
        }

        let mut source = Upvalue::ParentLocal(local.slot);
        for level in (owner + 1)..=self.enclosing.len() {
            let index = self.add_upvalue(level, name, source);
            source = Upvalue::ParentUpvalue(index);
        }
        // The last index added belongs to the function being compiled.
        self.upvalue_names.get(name).copied()
    }

    /// Add one upvalue to the function at `level`, reusing an existing capture of the same name.
    fn add_upvalue(&mut self, level: usize, name: &str, source: Upvalue) -> u32 {
        let (upvalues, names) = if level == self.enclosing.len() {
            (&mut self.upvalues, &mut self.upvalue_names)
        } else {
            let state = &mut self.enclosing[level];
            (&mut state.upvalues, &mut state.upvalue_names)
        };
        if let Some(existing) = names.get(name) {
            return *existing;
        }
        let index = upvalues.len() as u32;
        upvalues.push(source);
        names.insert(name.to_owned(), index);
        index
    }

    fn compiling_function(&self) -> bool {
        !self.enclosing.is_empty()
    }

    fn push_literal(&mut self, literal: &Literal) {
        self.push_constant(match literal {
            Literal::Undefined => Constant::Undefined,
            Literal::Null => Constant::Null,
            Literal::Boolean(value) => Constant::Boolean(*value),
            Literal::Number(value) => Constant::Number(*value),
            Literal::String(value) => Constant::String(value.clone()),
        });
    }

    fn push_constant(&mut self, constant: Constant) {
        let index = self.constant_index(constant);
        self.code.push(Instruction::Const(index));
    }

    /// Intern a constant, returning its table index.
    ///
    /// Property names repeat heavily -- every `.length` in a program is the same string -- and
    /// an instruction that names one by index rather than carrying it inline is what lets the
    /// table hold one copy.
    fn constant_index(&mut self, constant: Constant) -> u32 {
        if let Some(existing) = self
            .constants
            .iter()
            .position(|candidate| *candidate == constant)
        {
            return existing as u32;
        }
        let index = self.constants.len() as u32;
        self.constants.push(constant);
        index
    }

    fn emit_jump(&mut self, instruction: Instruction) -> usize {
        let index = self.code.len();
        self.code.push(instruction);
        index
    }

    fn patch_jump(&mut self, index: usize) {
        let target = self.code.len() as u32;
        match &mut self.code[index] {
            Instruction::Jump(offset)
            | Instruction::JumpIfFalse(offset)
            | Instruction::JumpIfTruthy(offset)
            | Instruction::JumpIfNotNullish(offset) => *offset = target,
            _ => unreachable!("patch_jump called for non-jump instruction"),
        }
    }
}

/// Names that nested functions inside `body` refer to.
///
/// Deliberately an over-approximation: it collects every identifier appearing anywhere inside a
/// nested function, without modelling that function's own shadowing. A local that is boxed but
/// never actually captured still behaves correctly -- one pointer chase, no semantic difference
/// -- whereas a captured local that was *missed* would be read out of a dead frame. The analysis
/// is allowed to be wrong only in the direction that costs performance.
fn names_captured_by_nested_functions(body: &[Statement]) -> HashSet<String> {
    let mut captured = HashSet::new();
    for statement in body {
        collect_nested_function_names(statement, &mut captured, false);
    }
    captured
}

fn collect_nested_function_names(
    statement: &Statement,
    names: &mut HashSet<String>,
    inside_function: bool,
) {
    match &statement.kind {
        StatementKind::Function(function) => {
            for statement in &function.body {
                collect_nested_function_names(statement, names, true);
            }
        }
        // Every method body is a nested function, so its free names are capture candidates.
        StatementKind::Class(class) => {
            if let Some(superclass) = &class.superclass {
                collect_identifiers(superclass, names, inside_function);
            }
            let bodies = class
                .constructor
                .iter()
                .chain(class.methods.iter().map(|method| &method.function));
            for function in bodies {
                for statement in &function.body {
                    collect_nested_function_names(statement, names, true);
                }
            }
        }
        StatementKind::Variable { declarations, .. } => {
            for declaration in declarations {
                if inside_function {
                    names.insert(declaration.name.clone());
                }
                if let Some(initializer) = &declaration.initializer {
                    collect_identifiers(initializer, names, inside_function);
                }
            }
        }
        StatementKind::Expression(expression) | StatementKind::ExportDefault(expression) => {
            collect_identifiers(expression, names, inside_function);
        }
        StatementKind::Return(value) => {
            if let Some(value) = value {
                collect_identifiers(value, names, inside_function);
            }
        }
        StatementKind::Block(body) => {
            for statement in body {
                collect_nested_function_names(statement, names, inside_function);
            }
        }
        StatementKind::If {
            test,
            consequent,
            alternate,
        } => {
            collect_identifiers(test, names, inside_function);
            collect_nested_function_names(consequent, names, inside_function);
            if let Some(alternate) = alternate {
                collect_nested_function_names(alternate, names, inside_function);
            }
        }
        StatementKind::While { test, body } => {
            collect_identifiers(test, names, inside_function);
            collect_nested_function_names(body, names, inside_function);
        }
        StatementKind::Empty | StatementKind::Unsupported { .. } => {}
    }
}

/// Walk an expression, recording identifiers when `collecting`.
///
/// It descends unconditionally and gates only the recording. That distinction matters: a function
/// *expression* nested in a `return` is a nested function whose free names are captures, and an
/// early return on `!collecting` would never reach it. That was a real bug -- `function f(n) {
/// return (m) => n + m; }` compiled with `n` unboxed, so the arrow could not resolve it.
fn collect_identifiers(expression: &Expression, names: &mut HashSet<String>, collecting: bool) {
    match &expression.kind {
        // A nested function's body is always collected, whatever the enclosing flag says.
        ExpressionKind::Function(function) => {
            for statement in &function.body {
                collect_nested_function_names(statement, names, true);
            }
        }
        ExpressionKind::Identifier(name) => {
            if collecting {
                names.insert(name.clone());
            }
        }
        ExpressionKind::Literal(_) | ExpressionKind::This | ExpressionKind::Unsupported { .. } => {}
        ExpressionKind::Unary { argument, .. } => collect_identifiers(argument, names, collecting),
        ExpressionKind::Binary { left, right, .. }
        | ExpressionKind::Logical { left, right, .. } => {
            collect_identifiers(left, names, collecting);
            collect_identifiers(right, names, collecting);
        }
        ExpressionKind::Assignment { target, value, .. } => {
            if collecting {
                names.insert(target.clone());
            }
            collect_identifiers(value, names, collecting);
        }
        ExpressionKind::Conditional {
            test,
            consequent,
            alternate,
        } => {
            collect_identifiers(test, names, collecting);
            collect_identifiers(consequent, names, collecting);
            collect_identifiers(alternate, names, collecting);
        }
        ExpressionKind::Call { callee, arguments } => {
            collect_identifiers(callee, names, collecting);
            for argument in arguments {
                collect_identifiers(argument, names, collecting);
            }
        }
        ExpressionKind::New { callee, arguments } => {
            collect_identifiers(callee, names, collecting);
            for argument in arguments {
                collect_identifiers(argument, names, collecting);
            }
        }
        ExpressionKind::Array { elements } => {
            for element in elements {
                collect_identifiers(element, names, collecting);
            }
        }
        ExpressionKind::ComputedMember { object, property } => {
            collect_identifiers(object, names, collecting);
            collect_identifiers(property, names, collecting);
        }
        ExpressionKind::ComputedAssignment {
            object,
            property,
            value,
            ..
        } => {
            collect_identifiers(object, names, collecting);
            collect_identifiers(property, names, collecting);
            collect_identifiers(value, names, collecting);
        }
        ExpressionKind::MemberAssignment { object, value, .. } => {
            collect_identifiers(object, names, collecting);
            collect_identifiers(value, names, collecting);
        }
        ExpressionKind::Object { properties } => {
            for property in properties {
                if let ObjectKey::Computed(key) = &property.key {
                    collect_identifiers(key, names, collecting);
                }
                collect_identifiers(&property.value, names, collecting);
            }
        }
        ExpressionKind::StaticMember { object, .. } => {
            collect_identifiers(object, names, collecting);
        }
    }
}

fn expression_references(expression: &Expression, name: &str) -> bool {
    match &expression.kind {
        ExpressionKind::Identifier(identifier) => identifier == name,
        ExpressionKind::Unary { argument, .. } => expression_references(argument, name),
        ExpressionKind::Binary { left, right, .. }
        | ExpressionKind::Logical { left, right, .. } => {
            expression_references(left, name) || expression_references(right, name)
        }
        ExpressionKind::Assignment { target, value, .. } => {
            target == name || expression_references(value, name)
        }
        ExpressionKind::Conditional {
            test,
            consequent,
            alternate,
        } => {
            expression_references(test, name)
                || expression_references(consequent, name)
                || expression_references(alternate, name)
        }
        ExpressionKind::Call { callee, arguments } => {
            expression_references(callee, name)
                || arguments
                    .iter()
                    .any(|argument| expression_references(argument, name))
        }
        ExpressionKind::Array { elements } => elements
            .iter()
            .any(|element| expression_references(element, name)),
        ExpressionKind::ComputedMember { object, property } => {
            expression_references(object, name) || expression_references(property, name)
        }
        ExpressionKind::ComputedAssignment {
            object,
            property,
            value,
            ..
        } => {
            expression_references(object, name)
                || expression_references(property, name)
                || expression_references(value, name)
        }
        ExpressionKind::New { callee, arguments } => {
            expression_references(callee, name)
                || arguments
                    .iter()
                    .any(|argument| expression_references(argument, name))
        }
        ExpressionKind::MemberAssignment { object, value, .. } => {
            expression_references(object, name) || expression_references(value, name)
        }
        ExpressionKind::This | ExpressionKind::Function(_) => false,
        ExpressionKind::Object { properties } => properties.iter().any(|property| {
            expression_references(&property.value, name)
                || matches!(&property.key, ObjectKey::Computed(key) if expression_references(key, name))
        }),
        ExpressionKind::StaticMember { object, .. } => expression_references(object, name),
        ExpressionKind::Literal(_) | ExpressionKind::Unsupported { .. } => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn module(body: Vec<Statement>) -> Module {
        Module {
            source_kind: kiln_hir::SourceKind::JavaScript,
            source_goal: kiln_hir::SourceGoal::Module,
            items: Vec::new(),
            body,
            requested_modules: Vec::new(),
            imports: Vec::new(),
            exports: Vec::new(),
            diagnostics: Vec::new(),
            panicked: false,
        }
    }

    fn number(value: f64) -> Expression {
        Expression {
            kind: ExpressionKind::Literal(Literal::Number(value.to_bits())),
            span: Span::default(),
        }
    }

    fn identifier(name: &str) -> Expression {
        Expression {
            kind: ExpressionKind::Identifier(name.into()),
            span: Span::default(),
        }
    }

    fn statement(kind: StatementKind) -> Statement {
        Statement {
            kind,
            span: Span::default(),
        }
    }

    fn function(name: &str, parameters: &[&str], body: Vec<Statement>) -> Statement {
        statement(StatementKind::Function(Box::new(kiln_hir::Function {
            name: Some(name.to_owned()),
            arrow: false,
            parameters: parameters
                .iter()
                .map(|name| kiln_hir::Parameter {
                    name: (*name).to_owned(),
                    span: Span::default(),
                })
                .collect(),
            body,
            span: Span::default(),
        })))
    }

    fn errors(body: Vec<Statement>) -> Vec<String> {
        compile(&module(body))
            .expect_err("expected the module to be rejected")
            .into_iter()
            .map(|diagnostic| diagnostic.message)
            .collect()
    }

    /// A capture must box the *binding*: both closures over one variable have to see the same
    /// cell, or the second one reads a stale copy.
    /// A compound member assignment must evaluate its receiver once. Compiling the object
    /// expression twice would be the easy mistake and would double any side effect in it.
    fn arrow(parameters: &[&str], body: Vec<Statement>) -> Expression {
        Expression {
            kind: ExpressionKind::Function(Box::new(kiln_hir::Function {
                name: None,
                arrow: true,
                parameters: parameters
                    .iter()
                    .map(|name| kiln_hir::Parameter {
                        name: (*name).to_owned(),
                        span: Span::default(),
                    })
                    .collect(),
                body,
                span: Span::default(),
            })),
            span: Span::default(),
        }
    }

    /// The capture analysis has to descend into a function *expression* nested in a `return`.
    /// It did not at first -- it stopped as soon as it was not already collecting -- so
    /// `function f(n) { return (m) => n + m; }` compiled with `n` unboxed and the arrow could
    /// not resolve it. This is that program.
    #[test]
    fn an_arrow_returned_from_a_function_captures_its_parameter() {
        let program = compile(&module(vec![function(
            "make_adder",
            &["n"],
            vec![statement(StatementKind::Return(Some(arrow(
                &["m"],
                vec![statement(StatementKind::Return(Some(Expression {
                    kind: ExpressionKind::Binary {
                        operator: kiln_hir::BinaryOperator::Add,
                        left: Box::new(identifier("n")),
                        right: Box::new(identifier("m")),
                    },
                    span: Span::default(),
                })))],
            ))))],
        )]))
        .expect("compiles");

        let outer = program
            .functions
            .iter()
            .find(|proto| proto.name.as_deref() == Some("make_adder"))
            .expect("outer proto");
        assert!(
            outer
                .code
                .iter()
                .any(|i| matches!(i, Instruction::NewCell(_))),
            "the captured parameter must be boxed"
        );
        let inner = program
            .functions
            .iter()
            .find(|proto| proto.name.is_none())
            .expect("the arrow has no name");
        assert!(inner.code.contains(&Instruction::LoadUpvalue(0)));
    }

    /// A computed key is an arbitrary expression evaluated in source order, so it must be
    /// compiled rather than folded into a constant.
    #[test]
    fn a_computed_object_key_is_evaluated_not_interned() {
        let program = compile(&module(vec![statement(StatementKind::Expression(
            Expression {
                kind: ExpressionKind::Object {
                    properties: vec![
                        kiln_hir::ObjectProperty {
                            key: kiln_hir::ObjectKey::Static("fixed".into()),
                            value: number(1.0),
                            span: Span::default(),
                        },
                        kiln_hir::ObjectProperty {
                            key: kiln_hir::ObjectKey::Computed(Expression {
                                kind: ExpressionKind::Binary {
                                    operator: kiln_hir::BinaryOperator::Add,
                                    left: Box::new(number(1.0)),
                                    right: Box::new(number(2.0)),
                                },
                                span: Span::default(),
                            }),
                            value: number(3.0),
                            span: Span::default(),
                        },
                    ],
                },
                span: Span::default(),
            },
        ))]))
        .expect("compiles");
        assert!(
            program
                .code
                .iter()
                .any(|i| matches!(i, Instruction::Binary(_))),
            "the computed key is evaluated, not folded to a constant"
        );
    }

    #[test]
    fn a_compound_member_assignment_evaluates_its_receiver_once() {
        let program = compile(&module(vec![
            statement(StatementKind::Variable {
                kind: VariableKind::Let,
                declarations: vec![kiln_hir::VariableDeclarator {
                    name: "o".into(),
                    initializer: Some(Expression {
                        kind: ExpressionKind::Object {
                            properties: Vec::new(),
                        },
                        span: Span::default(),
                    }),
                    span: Span::default(),
                }],
            }),
            statement(StatementKind::Expression(Expression {
                kind: ExpressionKind::MemberAssignment {
                    operator: AssignmentOperator::Add,
                    object: Box::new(identifier("o")),
                    property: "count".into(),
                    value: Box::new(number(1.0)),
                },
                span: Span::default(),
            })),
        ]))
        .expect("compiles");

        let loads = program
            .code
            .iter()
            .filter(|i| matches!(i, Instruction::LoadLocal(_)))
            .count();
        assert_eq!(loads, 1, "the receiver is loaded once and duplicated");
        assert!(program.code.contains(&Instruction::Dup));
    }

    /// Property names repeat across a program, so they are interned rather than carried inline.
    #[test]
    fn repeated_property_names_share_one_constant() {
        let read = |name: &str| {
            statement(StatementKind::Expression(Expression {
                kind: ExpressionKind::StaticMember {
                    object: Box::new(identifier("o")),
                    property: name.into(),
                },
                span: Span::default(),
            }))
        };
        let program = compile(&module(vec![
            statement(StatementKind::Variable {
                kind: VariableKind::Let,
                declarations: vec![kiln_hir::VariableDeclarator {
                    name: "o".into(),
                    initializer: Some(Expression {
                        kind: ExpressionKind::Object {
                            properties: Vec::new(),
                        },
                        span: Span::default(),
                    }),
                    span: Span::default(),
                }],
            }),
            read("length"),
            read("length"),
            read("length"),
        ]))
        .expect("compiles");

        let names = program
            .constants
            .iter()
            .filter(|constant| **constant == Constant::String("length".to_owned()))
            .count();
        assert_eq!(names, 1, "three reads of .length share one constant");
    }

    #[test]
    fn a_captured_local_is_boxed_and_the_capture_points_at_its_slot() {
        let program = compile(&module(vec![function(
            "outer",
            &[],
            vec![
                statement(StatementKind::Variable {
                    kind: VariableKind::Let,
                    declarations: vec![kiln_hir::VariableDeclarator {
                        name: "captured".into(),
                        initializer: Some(number(0.0)),
                        span: Span::default(),
                    }],
                }),
                function(
                    "inner",
                    &[],
                    vec![statement(StatementKind::Return(Some(identifier(
                        "captured",
                    ))))],
                ),
            ],
        )]))
        .expect("compiles");

        let outer = program
            .functions
            .iter()
            .find(|proto| proto.name.as_deref() == Some("outer"))
            .expect("outer proto");
        assert!(
            outer
                .code
                .iter()
                .any(|i| matches!(i, Instruction::NewCell(_))),
            "the captured local must be boxed"
        );

        let inner = program
            .functions
            .iter()
            .find(|proto| proto.name.as_deref() == Some("inner"))
            .expect("inner proto");
        assert_eq!(inner.upvalues.len(), 1);
        assert!(matches!(inner.upvalues[0], Upvalue::ParentLocal(_)));
        assert!(inner.code.contains(&Instruction::LoadUpvalue(0)));
    }

    /// A local nothing captures must stay an ordinary slot: the indirection is paid where the
    /// semantics need it and nowhere else.
    #[test]
    fn an_uncaptured_local_is_not_boxed() {
        let program = compile(&module(vec![function(
            "plain",
            &[],
            vec![statement(StatementKind::Variable {
                kind: VariableKind::Let,
                declarations: vec![kiln_hir::VariableDeclarator {
                    name: "local".into(),
                    initializer: Some(number(1.0)),
                    span: Span::default(),
                }],
            })],
        )]))
        .expect("compiles");
        assert!(
            !program.functions[0]
                .code
                .iter()
                .any(|i| matches!(i, Instruction::NewCell(_))),
            "nothing captures this local, so it must not be boxed"
        );
    }

    /// Two levels of nesting is two hops: the middle function captures from its parent, and the
    /// innermost captures from the middle. A single long hop would reach a frame that is gone.
    #[test]
    fn a_two_level_capture_threads_through_the_function_between() {
        let program = compile(&module(vec![function(
            "outer",
            &["x"],
            vec![function(
                "middle",
                &[],
                vec![function(
                    "inner",
                    &[],
                    vec![statement(StatementKind::Return(Some(identifier("x"))))],
                )],
            )],
        )]))
        .expect("compiles");

        let middle = program
            .functions
            .iter()
            .find(|proto| proto.name.as_deref() == Some("middle"))
            .expect("middle proto");
        let inner = program
            .functions
            .iter()
            .find(|proto| proto.name.as_deref() == Some("inner"))
            .expect("inner proto");

        assert!(
            matches!(middle.upvalues.as_slice(), [Upvalue::ParentLocal(_)]),
            "middle captures outer's boxed parameter directly"
        );
        assert!(
            matches!(inner.upvalues.as_slice(), [Upvalue::ParentUpvalue(0)]),
            "inner captures what middle had already captured"
        );
    }

    /// Writing through a capture has to be a write to the shared cell, not to a frame slot.
    #[test]
    fn assigning_to_a_captured_name_writes_through_the_cell() {
        let program = compile(&module(vec![function(
            "outer",
            &["seed"],
            vec![function(
                "bump",
                &[],
                vec![statement(StatementKind::Expression(Expression {
                    kind: ExpressionKind::Assignment {
                        operator: AssignmentOperator::Assign,
                        target: "seed".into(),
                        value: Box::new(number(1.0)),
                    },
                    span: Span::default(),
                }))],
            )],
        )]))
        .expect("compiles");
        let bump = program
            .functions
            .iter()
            .find(|proto| proto.name.as_deref() == Some("bump"))
            .expect("bump proto");
        assert!(bump.code.contains(&Instruction::StoreUpvalue(0)));
    }

    #[test]
    fn a_function_body_compiles_into_its_own_proto() {
        let program = compile(&module(vec![function(
            "second",
            &["a", "b"],
            vec![statement(StatementKind::Return(Some(identifier("b"))))],
        )]))
        .expect("compiles");

        assert_eq!(program.functions.len(), 1);
        let proto = &program.functions[0];
        assert_eq!(proto.parameter_count, 2);
        assert_eq!(proto.local_count, 2, "parameters occupy the first slots");
        assert_eq!(proto.code.first(), Some(&Instruction::LoadLocal(1)));
        // Every body ends in a Return, so a caller can never fall off the end of one.
        assert_eq!(proto.code.last(), Some(&Instruction::Return));
    }

    /// The binding has to exist before the body is compiled, or a function cannot call itself.
    #[test]
    fn a_function_can_refer_to_itself() {
        let program = compile(&module(vec![function(
            "loop_forever",
            &[],
            vec![statement(StatementKind::Return(Some(Expression {
                kind: ExpressionKind::Call {
                    callee: Box::new(identifier("loop_forever")),
                    arguments: Vec::new(),
                },
                span: Span::default(),
            })))],
        )]))
        .expect("compiles");
        assert!(
            program.functions[0]
                .code
                .contains(&Instruction::Call { argc: 0 })
        );
    }

    /// Module bindings are read through a distinct instruction, because a frame's locals are
    /// its own and a module slot is shared by every frame.
    #[test]
    fn a_function_reads_module_scope_through_a_global_slot() {
        let program = compile(&module(vec![
            statement(StatementKind::Variable {
                kind: VariableKind::Let,
                declarations: vec![kiln_hir::VariableDeclarator {
                    name: "shared".into(),
                    initializer: Some(number(1.0)),
                    span: Span::default(),
                }],
            }),
            function(
                "read",
                &[],
                vec![statement(StatementKind::Return(Some(identifier("shared"))))],
            ),
        ]))
        .expect("compiles");
        assert!(
            program.functions[0]
                .code
                .iter()
                .any(|instruction| matches!(instruction, Instruction::LoadGlobal(_))),
            "module reads must not compile to LoadLocal, which would read the frame's own slot"
        );
    }

    /// Step 1a refused captures outright; 1b compiles them. What must remain true is that a
    /// capture never becomes a `LoadLocal` on the enclosing frame's slot -- that would read
    /// whatever the callee's own frame happened to hold there, which is a plausible wrong
    /// answer rather than a diagnostic.
    #[test]
    fn a_capture_never_compiles_to_a_frame_local_read() {
        let program = compile(&module(vec![function(
            "outer",
            &["x"],
            vec![function(
                "inner",
                &[],
                vec![statement(StatementKind::Return(Some(identifier("x"))))],
            )],
        )]))
        .expect("compiles");
        let inner = program
            .functions
            .iter()
            .find(|proto| proto.name.as_deref() == Some("inner"))
            .expect("inner proto");
        assert!(
            !inner
                .code
                .iter()
                .any(|i| matches!(i, Instruction::LoadLocal(_))),
            "inner has no locals of its own; reading x must go through an upvalue"
        );
        assert!(inner.code.contains(&Instruction::LoadUpvalue(0)));
    }

    #[test]
    fn return_outside_a_function_is_refused() {
        let messages = errors(vec![statement(StatementKind::Return(None))]);
        assert!(
            messages.iter().any(|m| m.contains("outside of a function")),
            "got: {messages:?}"
        );
    }

    #[test]
    fn duplicate_parameters_are_refused() {
        let messages = errors(vec![function("dup", &["a", "a"], Vec::new())]);
        assert!(
            messages.iter().any(|m| m.contains("duplicate parameter")),
            "got: {messages:?}"
        );
    }

    /// A bare `return;` and falling off the end both produce undefined, and both must leave a
    /// value on the stack: a caller unconditionally pops one.
    #[test]
    fn every_exit_from_a_function_yields_a_value() {
        let program = compile(&module(vec![function(
            "bare",
            &[],
            vec![statement(StatementKind::Return(None))],
        )]))
        .expect("compiles");
        let code = &program.functions[0].code;
        let returns = code
            .iter()
            .filter(|instruction| **instruction == Instruction::Return)
            .count();
        assert_eq!(returns, 2, "explicit return plus the fall-through epilogue");
        assert!(matches!(code.first(), Some(Instruction::Const(_))));
    }

    #[test]
    fn compiles_variables_and_arithmetic() {
        let body = vec![
            Statement {
                kind: StatementKind::Variable {
                    kind: VariableKind::Let,
                    declarations: vec![kiln_hir::VariableDeclarator {
                        name: "answer".into(),
                        initializer: Some(Expression {
                            kind: ExpressionKind::Binary {
                                operator: BinaryOperator::Add,
                                left: Box::new(number(40.0)),
                                right: Box::new(number(2.0)),
                            },
                            span: Span::default(),
                        }),
                        span: Span::default(),
                    }],
                },
                span: Span::default(),
            },
            Statement {
                kind: StatementKind::Expression(Expression {
                    kind: ExpressionKind::Assignment {
                        operator: AssignmentOperator::Add,
                        target: "answer".into(),
                        value: Box::new(number(1.0)),
                    },
                    span: Span::default(),
                }),
                span: Span::default(),
            },
        ];
        let program = compile(&module(body)).expect("valid HIR compiles");
        assert_eq!(program.version, BYTECODE_VERSION);
        assert_eq!(program.local_count, 1);
        assert!(
            program
                .code
                .iter()
                .any(|instruction| matches!(instruction, Instruction::Binary(BinaryOp::Add)))
        );
        assert!(
            program
                .code
                .iter()
                .any(|instruction| matches!(instruction, Instruction::StoreLocal(0)))
        );
    }

    #[test]
    fn emits_short_circuit_jumps_and_host_call() {
        let call = Expression {
            kind: ExpressionKind::Call {
                callee: Box::new(Expression {
                    kind: ExpressionKind::StaticMember {
                        object: Box::new(identifier("console")),
                        property: "log".into(),
                    },
                    span: Span::default(),
                }),
                arguments: vec![number(1.0)],
            },
            span: Span::default(),
        };
        let body = vec![Statement {
            kind: StatementKind::Expression(Expression {
                kind: ExpressionKind::Logical {
                    operator: LogicalOperator::And,
                    left: Box::new(Expression {
                        kind: ExpressionKind::Literal(Literal::Boolean(true)),
                        span: Span::default(),
                    }),
                    right: Box::new(call),
                },
                span: Span::default(),
            }),
            span: Span::default(),
        }];
        let program = compile(&module(body)).expect("valid HIR compiles");
        assert!(
            program
                .code
                .iter()
                .any(|instruction| matches!(instruction, Instruction::JumpIfFalse(_)))
        );
        assert!(program.code.iter().any(|instruction| matches!(
            instruction,
            Instruction::HostCall {
                op: HostOp::WriteStdout,
                argc: 1
            }
        )));
    }

    #[test]
    fn rejects_unresolved_and_redeclared_locals() {
        let body = vec![
            Statement {
                kind: StatementKind::Expression(identifier("missing")),
                span: Span::new(2, 9),
            },
            Statement {
                kind: StatementKind::Variable {
                    kind: VariableKind::Let,
                    declarations: vec![kiln_hir::VariableDeclarator {
                        name: "x".into(),
                        initializer: None,
                        span: Span::new(10, 11),
                    }],
                },
                span: Span::default(),
            },
            Statement {
                kind: StatementKind::Variable {
                    kind: VariableKind::Var,
                    declarations: vec![kiln_hir::VariableDeclarator {
                        name: "x".into(),
                        initializer: None,
                        span: Span::new(12, 13),
                    }],
                },
                span: Span::default(),
            },
        ];
        let diagnostics = compile(&module(body)).expect_err("invalid HIR is rejected");
        assert!(
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("unresolved identifier"))
        );
        assert!(
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("redeclaration"))
        );
    }

    #[test]
    fn rejects_module_dependencies() {
        let mut module = module(Vec::new());
        module.requested_modules.push(kiln_hir::RequestedModule {
            specifier: "./dep.js".into(),
            occurrences: vec![kiln_hir::ModuleRequestOccurrence {
                statement_span: Span::new(0, 9),
                specifier_span: Span::new(0, 9),
                type_only: false,
                kind: kiln_hir::ModuleRequestKind::Import,
            }],
        });
        let diagnostics = compile(&module).expect_err("dependencies are not executable yet");
        assert!(diagnostics[0].message.contains("module dependency"));
        assert_eq!(diagnostics[0].span, Some(Span::new(0, 9)));
    }

    #[test]
    fn linkage_compilation_maps_named_imports_and_local_exports_to_slots() {
        let mut module = module(vec![Statement {
            kind: StatementKind::Variable {
                kind: VariableKind::Const,
                declarations: vec![kiln_hir::VariableDeclarator {
                    name: "answer".into(),
                    initializer: Some(Expression {
                        kind: ExpressionKind::Binary {
                            operator: BinaryOperator::Multiply,
                            left: Box::new(identifier("input")),
                            right: Box::new(number(2.0)),
                        },
                        span: Span::default(),
                    }),
                    span: Span::default(),
                }],
            },
            span: Span::default(),
        }]);
        module.requested_modules.push(kiln_hir::RequestedModule {
            specifier: "./dep.js".into(),
            occurrences: vec![kiln_hir::ModuleRequestOccurrence {
                statement_span: Span::new(0, 40),
                specifier_span: Span::new(23, 33),
                type_only: false,
                kind: kiln_hir::ModuleRequestKind::Import,
            }],
        });
        module.imports.push(kiln_hir::Import {
            source: "./dep.js".into(),
            imported: ImportName::Name("value".into()),
            local: "input".into(),
            type_only: false,
            span: Span::new(9, 14),
            statement_span: Span::new(0, 40),
        });
        module.exports.push(kiln_hir::Export {
            exported: Some(ExportName::Name("answer".into())),
            origin: ExportOrigin::Local {
                local: "answer".into(),
            },
            type_only: false,
            span: Span::default(),
            statement_span: Span::default(),
        });

        let linked = compile_module(&module).expect("named scalar linkage compiles");
        assert_eq!(linked.dependencies, ["./dep.js"]);
        assert_eq!(linked.imports[0].imported, "value");
        assert_eq!(linked.imports[0].local_slot, 0);
        assert_eq!(linked.exports[0].exported, "answer");
        assert_eq!(linked.exports[0].local_slot, 1);
        assert!(linked.program.code.contains(&Instruction::LoadLocal(0)));
    }

    #[test]
    fn linkage_compilation_rejects_assignment_to_import() {
        let mut module = module(vec![Statement {
            kind: StatementKind::Expression(Expression {
                kind: ExpressionKind::Assignment {
                    operator: AssignmentOperator::Assign,
                    target: "input".into(),
                    value: Box::new(number(1.0)),
                },
                span: Span::default(),
            }),
            span: Span::default(),
        }]);
        module.imports.push(kiln_hir::Import {
            source: "./dep.js".into(),
            imported: ImportName::Name("value".into()),
            local: "input".into(),
            type_only: false,
            span: Span::default(),
            statement_span: Span::default(),
        });

        let diagnostics = compile_module(&module).expect_err("imports are immutable bindings");
        assert!(
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("imported binding"))
        );
    }
}
