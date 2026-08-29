//! A small, parser-independent stack bytecode for executable Kiln modules.
//!
//! The frontend owns the parser-specific lowering into [`kiln_hir`].  This crate deliberately
//! consumes only that owned HIR and produces plain data, so an interpreter, serializer, or native
//! backend can be added without coupling it to Oxc (or to any other parser).

use std::collections::HashMap;

use kiln_hir::{
    AssignmentOperator, BinaryOperator, ExportName, ExportOrigin, Expression, ExpressionKind,
    Function, ImportName, Literal, LogicalOperator, Module, Span, Statement, StatementKind,
    UnaryOperator, VariableKind,
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
}

/// Binary operations understood by the bytecode runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BinaryOp {
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
    pub parameter_count: u32,
    /// Includes the parameters, which occupy the first slots in declaration order.
    pub local_count: u32,
    pub code: Vec<Instruction>,
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
}

/// The per-function half of [`Compiler`], saved and restored around a nested function body.
///
/// Constants and diagnostics are deliberately not in here: they are program-wide, and splitting
/// them would mean merging them back with index fixups for no benefit.
struct FunctionState {
    code: Vec<Instruction>,
    scopes: Vec<HashMap<String, Local>>,
    local_count: u32,
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
    Captured,
    Unresolved,
}

struct Compiler {
    constants: Vec<Constant>,
    functions: Vec<FunctionProto>,
    /// Enclosing function states, module body first. Empty while compiling the module body.
    enclosing: Vec<FunctionState>,
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
                    self.code.push(Instruction::StoreLocal(existing.slot));
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
        self.scopes[scope_index].insert(
            name.to_owned(),
            Local {
                slot,
                kind,
                imported: false,
            },
        );
        if let Some(initializer) = initializer {
            self.expression(initializer);
        } else {
            self.push_constant(Constant::Undefined);
        }
        self.code.push(Instruction::StoreLocal(slot));
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
            ExpressionKind::Identifier(name) => match self.resolve(name) {
                Resolution::Local(local) => self.code.push(Instruction::LoadLocal(local.slot)),
                Resolution::Global(slot) => self.code.push(Instruction::LoadGlobal(slot)),
                Resolution::Captured => self.error(
                    Some(expression.span),
                    format!("`{name}` is captured from an enclosing function, which is not supported yet"),
                ),
                Resolution::Unresolved if name == "undefined" => {
                    self.push_constant(Constant::Undefined)
                }
                Resolution::Unresolved => self.error(
                    Some(expression.span),
                    format!("unresolved identifier `{name}`"),
                ),
            },
            ExpressionKind::Unary { operator, argument } => {
                self.expression(argument);
                self.code.push(Instruction::Unary(match operator {
                    UnaryOperator::Plus => UnaryOp::Plus,
                    UnaryOperator::Negate => UnaryOp::Negate,
                    UnaryOperator::Not => UnaryOp::Not,
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
            ExpressionKind::StaticMember { object, property } => {
                self.error(
                    Some(expression.span),
                    format!("unsupported member access `.{property}`"),
                );
                self.expression(object);
                self.code.push(Instruction::Pop);
                self.push_constant(Constant::Undefined);
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
        let (slot, global, kind, imported) = match self.resolve(target) {
            Resolution::Local(local) => (local.slot, false, local.kind, local.imported),
            Resolution::Global(slot) => {
                let module = &self.enclosing[0].scopes[0];
                let local = module
                    .get(target)
                    .copied()
                    .expect("Global resolution just found this name in module scope");
                (slot, true, local.kind, local.imported)
            }
            Resolution::Captured => {
                self.error(
                    Some(span),
                    format!(
                        "`{target}` is captured from an enclosing function, which is not supported yet"
                    ),
                );
                self.push_constant(Constant::Undefined);
                return;
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
        let load = if global {
            Instruction::LoadGlobal(slot)
        } else {
            Instruction::LoadLocal(slot)
        };
        let store = if global {
            Instruction::StoreGlobal(slot)
        } else {
            Instruction::StoreLocal(slot)
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

        let mut parameters = HashMap::with_capacity(function.parameters.len());
        for (slot, parameter) in function.parameters.iter().enumerate() {
            if parameters.contains_key(&parameter.name) {
                self.error(
                    Some(parameter.span),
                    format!("duplicate parameter `{}`", parameter.name),
                );
                return None;
            }
            parameters.insert(
                parameter.name.clone(),
                Local {
                    slot: slot as u32,
                    kind: VariableKind::Let,
                    imported: false,
                },
            );
        }

        let parameter_count = function.parameters.len() as u32;
        let outer = FunctionState {
            code: std::mem::take(&mut self.code),
            scopes: std::mem::replace(&mut self.scopes, vec![parameters]),
            local_count: std::mem::replace(&mut self.local_count, parameter_count),
        };
        self.enclosing.push(outer);

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
            parameter_count,
            local_count: std::mem::replace(&mut self.local_count, restored.local_count),
            code: std::mem::replace(&mut self.code, restored.code),
        };
        self.scopes = restored.scopes;

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
        // Anything visible in a function between the module and here is a capture. Reported as
        // such rather than as "unresolved", because the name is in scope in the source and
        // saying otherwise would send the reader looking for a typo.
        if self
            .enclosing
            .iter()
            .skip(1)
            .any(|state| state.scopes.iter().any(|scope| scope.contains_key(name)))
        {
            return Resolution::Captured;
        }
        Resolution::Unresolved
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
        let index = self.constants.len();
        self.constants.push(constant);
        self.code.push(Instruction::Const(index as u32));
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

    /// The whole point of refusing captures rather than compiling them: `LoadLocal` on an
    /// enclosing function's slot would read whatever the *callee's* frame happens to hold
    /// there, which is a plausible-looking wrong answer instead of a diagnostic.
    #[test]
    fn capturing_an_enclosing_functions_local_is_refused() {
        let messages = errors(vec![function(
            "outer",
            &["x"],
            vec![function(
                "inner",
                &[],
                vec![statement(StatementKind::Return(Some(identifier("x"))))],
            )],
        )]);
        assert!(
            messages
                .iter()
                .any(|message| message.contains("captured from an enclosing function")),
            "got: {messages:?}"
        );
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
