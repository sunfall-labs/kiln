//! A small, parser-independent stack bytecode for executable Kiln modules.
//!
//! The frontend owns the parser-specific lowering into [`kiln_hir`].  This crate deliberately
//! consumes only that owned HIR and produces plain data, so an interpreter, serializer, or native
//! backend can be added without coupling it to Oxc (or to any other parser).

use std::collections::HashMap;

use kiln_hir::{
    AssignmentOperator, BinaryOperator, Expression, ExpressionKind, Literal, LogicalOperator,
    Module, Span, Statement, StatementKind, UnaryOperator, VariableKind,
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
    Return,
}

/// A compiled module body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Program {
    pub version: u16,
    pub constants: Vec<Constant>,
    pub code: Vec<Instruction>,
    pub local_count: u32,
}

/// A source-located compiler failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompileDiagnostic {
    pub message: String,
    pub span: Option<Span>,
}

/// Compile owned HIR into stack bytecode.
pub fn compile(module: &Module) -> Result<Program, Vec<CompileDiagnostic>> {
    let mut compiler = Compiler {
        constants: Vec::new(),
        code: Vec::new(),
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
    if !module.requested_modules.is_empty() {
        for requested in &module.requested_modules {
            let span = requested.occurrences.first().copied();
            compiler.error(
                span,
                format!(
                    "module dependency `{}` is not supported by this execution backend",
                    requested.specifier
                ),
            );
        }
    }
    for export in &module.exports {
        if let Some(source) = &export.source {
            compiler.error(
                Some(export.span),
                format!("re-export from `{source}` is not supported by this execution backend"),
            );
        }
    }

    for statement in &module.body {
        compiler.statement(statement);
    }
    compiler.push_constant(Constant::Undefined);
    compiler.code.push(Instruction::Return);

    if compiler.diagnostics.is_empty() {
        Ok(Program {
            version: BYTECODE_VERSION,
            constants: compiler.constants,
            code: compiler.code,
            local_count: compiler.local_count,
        })
    } else {
        Err(compiler.diagnostics)
    }
}

#[derive(Clone, Copy)]
struct Local {
    slot: u32,
    kind: VariableKind,
}

struct Compiler {
    constants: Vec<Constant>,
    code: Vec<Instruction>,
    /// Lexical scopes, module scope first and innermost block last.
    scopes: Vec<HashMap<String, Local>>,
    /// Slots are never recycled: bytecode can jump back into a block in a loop, and a stable
    /// slot keeps that execution independent of compiler scope bookkeeping.
    local_count: u32,
    diagnostics: Vec<CompileDiagnostic>,
}

impl Compiler {
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
        self.scopes[scope_index].insert(name.to_owned(), Local { slot, kind });
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
            ExpressionKind::Identifier(name) => {
                if let Some(local) = self.resolve(name) {
                    self.code.push(Instruction::LoadLocal(local.slot));
                } else if name == "undefined" {
                    self.push_constant(Constant::Undefined);
                } else {
                    self.error(
                        Some(expression.span),
                        format!("unresolved identifier `{name}`"),
                    );
                }
            }
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
        let Some((slot, kind)) = self.resolve(target).map(|local| (local.slot, local.kind)) else {
            self.error(
                Some(span),
                format!("unresolved assignment target `{target}`"),
            );
            self.push_constant(Constant::Undefined);
            return;
        };
        if kind == VariableKind::Const {
            self.error(
                Some(span),
                format!("assignment to const local `{target}` is not supported"),
            );
        }
        if operator == AssignmentOperator::Assign {
            self.expression(value);
        } else {
            self.code.push(Instruction::LoadLocal(slot));
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
        self.code.push(Instruction::StoreLocal(slot));
    }

    fn call(&mut self, callee: &Expression, arguments: &[Expression], span: Span) {
        let is_console_log = self.resolve("console").is_none()
            && matches!(
                &callee.kind,
                ExpressionKind::StaticMember { object, property }
                    if property == "log"
                        && matches!(&object.kind, ExpressionKind::Identifier(name) if name == "console")
            );
        if !is_console_log {
            self.error(Some(span), "only console.log(...) calls are supported");
            return;
        }
        if arguments.len() > u16::MAX as usize {
            self.error(Some(span), "console.log has too many arguments");
            return;
        }
        for argument in arguments {
            self.expression(argument);
        }
        self.code.push(Instruction::HostCall {
            op: HostOp::WriteStdout,
            argc: arguments.len() as u16,
        });
    }

    fn resolve(&self, name: &str) -> Option<Local> {
        self.scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(name).copied())
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
            occurrences: vec![Span::new(0, 9)],
        });
        let diagnostics = compile(&module).expect_err("dependencies are not executable yet");
        assert!(diagnostics[0].message.contains("module dependency"));
        assert_eq!(diagnostics[0].span, Some(Span::new(0, 9)));
    }
}
