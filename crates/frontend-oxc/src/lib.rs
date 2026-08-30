//! Oxc adapter for Kiln's owned HIR.
//!
//! This is the only crate allowed to expose Oxc to the compiler build graph. Its public API
//! accepts ordinary strings and returns [`kiln_hir`] values, so an Oxc upgrade cannot leak AST
//! types or arena lifetimes into the query database, interpreter, or backends.

use std::path::Path;

use kiln_hir::{
    AssignmentOperator as HirAssignmentOperator, BinaryOperator as HirBinaryOperator, Diagnostic,
    Export, ExportName, ExportOrigin, Expression as HirExpression,
    ExpressionKind as HirExpressionKind, Function as HirFunction, Import, ImportName, Item,
    ItemKind, ListElement, Literal as HirLiteral, LogicalOperator as HirLogicalOperator, Module,
    ModuleRequestKind, ModuleRequestOccurrence, Parameter as HirParameter, RequestedModule,
    SourceGoal, SourceKind, Span as HirSpan, Statement as HirStatement,
    StatementKind as HirStatementKind, UnaryOperator as HirUnaryOperator,
    VariableDeclarator as HirVariableDeclarator, VariableKind as HirVariableKind,
};
use oxc_allocator::Allocator;
use oxc_ast::ast::{
    Argument, ArrowFunctionBody, ArrowFunctionExpression, AssignmentTarget, BindingPattern, Class,
    ClassElement, Declaration, ExportDefaultDeclarationKind, Expression, FormalParameters,
    Function, MethodDefinitionKind, ObjectExpression, ObjectPropertyKind, PropertyKind, Statement,
    VariableDeclaration, VariableDeclarationKind,
};
use oxc_ast::ast::{
    ArrayExpressionElement, ForStatementInit, ForStatementLeft, SimpleAssignmentTarget,
};
use oxc_parser::Parser;
use oxc_span::{GetSpan, SourceType, Span};
use oxc_syntax::module_record::{
    ExportEntry as OxcExportEntry, ExportExportName, ExportImportName, ExportLocalName,
    ImportImportName,
};
use oxc_syntax::operator::UpdateOperator;
use oxc_syntax::operator::{AssignmentOperator, BinaryOperator, LogicalOperator, UnaryOperator};

/// Parse one JavaScript or TypeScript module and lower its module-level structure into Kiln HIR.
///
/// Parser diagnostics are data, not a transport error: Oxc can recover and produce a useful
/// module after many syntax errors, and the query database must cache that result just like a
/// clean parse. An unknown extension falls back to module-mode JavaScript; CLI source discovery
/// already restricts ordinary project files to known JS/TS extensions.
pub fn parse(path: &str, source: &str) -> Module {
    let source_type = SourceType::from_path(Path::new(path))
        .unwrap_or_else(|_| SourceType::default().with_module(true));
    parse_source(source, source_type)
}

/// Parse with an explicit ECMAScript goal, independent of Oxc's extension convention.
///
/// Test262 uses `.js` for both scripts and modules, so path inference alone is insufficient for
/// conformance work. The language variant (JS/TS/JSX) still comes from the path.
pub fn parse_with_goal(path: &str, source: &str, goal: SourceGoal) -> Module {
    let inferred = SourceType::from_path(Path::new(path)).unwrap_or_default();
    let source_type = match goal {
        SourceGoal::Script => inferred.with_script(true),
        SourceGoal::Module => inferred.with_module(true),
    };
    parse_source(source, source_type)
}

fn parse_source(source: &str, source_type: SourceType) -> Module {
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, source, source_type).parse();

    let mut requested_modules = parsed
        .module_record
        .requested_modules
        .iter()
        .map(|(specifier, occurrences)| RequestedModule {
            specifier: specifier.to_string(),
            occurrences: occurrences
                .iter()
                .map(|occurrence| ModuleRequestOccurrence {
                    statement_span: span(occurrence.statement_span),
                    specifier_span: span(occurrence.span),
                    type_only: occurrence.is_type,
                    kind: if occurrence.is_import {
                        ModuleRequestKind::Import
                    } else {
                        ModuleRequestKind::Export
                    },
                })
                .collect(),
        })
        .collect::<Vec<_>>();
    requested_modules.sort_by_key(|request| {
        request
            .occurrences
            .first()
            .map(|occurrence| occurrence.statement_span)
            .unwrap_or_default()
            .start
    });

    let mut imports = parsed
        .module_record
        .import_entries
        .iter()
        .map(|entry| Import {
            source: entry.module_request.name.to_string(),
            imported: lower_import_name(&entry.import_name),
            local: entry.local_name.name.to_string(),
            type_only: entry.is_type,
            span: span(entry.local_name.span),
            statement_span: span(entry.statement_span),
        })
        .collect::<Vec<_>>();
    imports.sort_by_key(|entry| (entry.statement_span.start, entry.span.start));

    // Use the spec-shaped entries rather than `exported_bindings`: the latter intentionally
    // excludes `default` and erases TypeScript's type-only bit, both of which affect Kiln's
    // runtime interface.
    let mut exports = parsed
        .module_record
        .local_export_entries
        .iter()
        .chain(parsed.module_record.indirect_export_entries.iter())
        .map(lower_export)
        .collect::<Vec<_>>();

    for entry in &parsed.module_record.star_export_entries {
        exports.push(lower_export(entry));
    }
    exports.sort_by_key(|export| export.span.start);

    let diagnostics = parsed
        .diagnostics
        .iter()
        .map(|diagnostic| {
            let diagnostic_span = diagnostic
                .labels
                .iter()
                .find(|label| label.primary())
                .or_else(|| diagnostic.labels.first())
                .map(|label| span(label.span()));
            Diagnostic {
                message: diagnostic.to_string(),
                span: diagnostic_span,
            }
        })
        .collect::<Vec<_>>();
    let mut lowerer = Lowerer;
    let body = parsed
        .program
        .body
        .iter()
        .filter_map(|statement| lowerer.statement(statement))
        .collect();

    Module {
        source_kind: source_kind(source_type),
        source_goal: if source_type.is_module() {
            SourceGoal::Module
        } else {
            SourceGoal::Script
        },
        items: parsed
            .program
            .body
            .iter()
            .map(|statement| Item {
                kind: item_kind(statement),
                span: span(statement.span()),
            })
            .collect(),
        body,
        requested_modules,
        imports,
        exports,
        diagnostics,
        panicked: parsed.panicked,
    }
}

/// A BigInt literal's value as plain decimal digits.
///
/// `0x1fn`, `1_000n` and `10n` all reach the IR as decimal text, so the interpreter and the
/// program-hash never have to re-derive a base. Returns `None` for anything that does not parse,
/// which leaves the refusal in place rather than inventing a number.
fn bigint_digits(raw: &str) -> Option<String> {
    let text = raw.strip_suffix('n').unwrap_or(raw).replace('_', "");
    let (digits, radix) = match text.get(..2).map(str::to_ascii_lowercase).as_deref() {
        Some("0x") => (&text[2..], 16),
        Some("0o") => (&text[2..], 8),
        Some("0b") => (&text[2..], 2),
        _ => (text.as_str(), 10),
    };
    if digits.is_empty() {
        return None;
    }
    Some(num_bigint::BigInt::parse_bytes(digits.as_bytes(), radix)?.to_string())
}

struct Lowerer;

impl Lowerer {
    fn statement(&mut self, statement: &Statement<'_>) -> Option<HirStatement> {
        let statement_span = span(statement.span());
        let kind = match statement {
            Statement::VariableDeclaration(declaration) => self.variable(declaration),
            Statement::ExportDeclaration(declaration) => {
                return self.declaration(&declaration.declaration, statement_span);
            }
            Statement::ExportDefaultDeclaration(declaration) => {
                if let Some(expression) = declaration.declaration.as_expression() {
                    HirStatementKind::ExportDefault(self.expression(expression))
                } else if matches!(
                    declaration.declaration,
                    ExportDefaultDeclarationKind::TSInterfaceDeclaration(_)
                ) {
                    return None;
                } else if let ExportDefaultDeclarationKind::FunctionDeclaration(declaration) =
                    &declaration.declaration
                {
                    // `export default function f() {}` declares `f` and exports it under
                    // `default`; the anonymous form declares the name the module record already
                    // uses for it, which carries characters an identifier cannot.
                    let lowered = self
                        .function(declaration, statement_span)
                        .map(|mut function| {
                            function
                                .name
                                .get_or_insert_with(|| kiln_hir::DEFAULT_EXPORT_LOCAL.to_owned());
                            HirStatementKind::Function(Box::new(function))
                        });
                    self.default_declaration(lowered, statement_span)
                } else if let ExportDefaultDeclarationKind::ClassDeclaration(declaration) =
                    &declaration.declaration
                {
                    let lowered = self.class(declaration, statement_span).map(|mut class| {
                        class
                            .name
                            .get_or_insert_with(|| kiln_hir::DEFAULT_EXPORT_LOCAL.to_owned());
                        HirStatementKind::Class(Box::new(class))
                    });
                    self.default_declaration(lowered, statement_span)
                } else {
                    self.unsupported_statement("default function or class export", statement_span)
                }
            }
            Statement::ExpressionStatement(expression) => {
                HirStatementKind::Expression(self.expression(&expression.expression))
            }
            Statement::BlockStatement(block) => HirStatementKind::Block(
                block
                    .body
                    .iter()
                    .filter_map(|statement| self.statement(statement))
                    .collect(),
            ),
            Statement::IfStatement(statement) => HirStatementKind::If {
                test: self.expression(&statement.test),
                consequent: Box::new(self.statement_or_unsupported(&statement.consequent)),
                alternate: statement
                    .alternate
                    .as_ref()
                    .map(|alternate| Box::new(self.statement_or_unsupported(alternate))),
            },
            Statement::WhileStatement(statement) => HirStatementKind::While {
                test: self.expression(&statement.test),
                body: Box::new(self.statement_or_unsupported(&statement.body)),
            },
            Statement::LabeledStatement(statement) => HirStatementKind::Labelled {
                label: statement.label.name.to_string(),
                body: Box::new(self.statement_or_unsupported(&statement.body)),
            },
            Statement::EmptyStatement(_) => HirStatementKind::Empty,

            // Module linkage and erased TypeScript declarations have no local runtime action.
            Statement::ImportDeclaration(_)
            | Statement::ExportAllDeclaration(_)
            | Statement::ExportNamedDeclaration(_)
            | Statement::ExportFromDeclaration(_)
            | Statement::TSTypeAliasDeclaration(_)
            | Statement::TSInterfaceDeclaration(_)
            | Statement::TSGlobalDeclaration(_)
            | Statement::TSNamespaceExportDeclaration(_) => return None,

            Statement::FunctionDeclaration(declaration) => {
                self.function_declaration(declaration, statement_span)
            }
            Statement::ClassDeclaration(declaration) => {
                match self.class(declaration, statement_span) {
                    Some(class) => HirStatementKind::Class(Box::new(class)),
                    None => HirStatementKind::Unsupported {
                        feature: "class declaration".to_string(),
                    },
                }
            }
            Statement::DoWhileStatement(statement) => HirStatementKind::DoWhile {
                body: Box::new(self.statement_or_unsupported(&statement.body)),
                test: self.expression(&statement.test),
            },
            Statement::ForStatement(statement) => HirStatementKind::For {
                init: statement
                    .init
                    .as_ref()
                    .and_then(|init| self.for_init(init, statement_span))
                    .map(Box::new),
                test: statement.test.as_ref().map(|test| self.expression(test)),
                update: statement
                    .update
                    .as_ref()
                    .map(|update| self.expression(update)),
                body: Box::new(self.statement_or_unsupported(&statement.body)),
            },
            Statement::ForOfStatement(statement) => {
                let ForStatementLeft::VariableDeclaration(declaration) = &statement.left else {
                    return Some(HirStatement {
                        kind: self.unsupported_statement(
                            "for-of over an existing binding",
                            statement_span,
                        ),
                        span: statement_span,
                    });
                };
                let Some(declarator) = declaration.declarations.first() else {
                    return Some(HirStatement {
                        kind: self.unsupported_statement("for-of binding", statement_span),
                        span: statement_span,
                    });
                };
                let pattern = self.pattern(&declarator.id, statement_span);
                HirStatementKind::ForOf {
                    kind: match declaration.kind {
                        VariableDeclarationKind::Var => HirVariableKind::Var,
                        VariableDeclarationKind::Const => HirVariableKind::Const,
                        _ => HirVariableKind::Let,
                    },
                    pattern,
                    iterable: self.expression(&statement.right),
                    body: Box::new(self.statement_or_unsupported(&statement.body)),
                    awaiting: statement.r#await,
                }
            }
            Statement::ForInStatement(statement) => {
                let (kind, name) = match self.for_binding(&statement.left) {
                    Some(binding) => binding,
                    None => {
                        return Some(HirStatement {
                            kind: self.unsupported_statement("for-in binding", statement_span),
                            span: statement_span,
                        });
                    }
                };
                HirStatementKind::ForIn {
                    kind,
                    name,
                    object: self.expression(&statement.right),
                    body: Box::new(self.statement_or_unsupported(&statement.body)),
                }
            }
            Statement::BreakStatement(statement) => HirStatementKind::Break {
                label: statement.label.as_ref().map(|label| label.name.to_string()),
            },
            Statement::ContinueStatement(statement) => HirStatementKind::Continue {
                label: statement.label.as_ref().map(|label| label.name.to_string()),
            },
            Statement::ReturnStatement(statement) => HirStatementKind::Return(
                statement
                    .argument
                    .as_ref()
                    .map(|value| self.expression(value)),
            ),
            Statement::SwitchStatement(statement) => HirStatementKind::Switch {
                discriminant: self.expression(&statement.discriminant),
                cases: statement
                    .cases
                    .iter()
                    .map(|case| kiln_hir::SwitchCase {
                        test: case.test.as_ref().map(|test| self.expression(test)),
                        body: case
                            .consequent
                            .iter()
                            .filter_map(|statement| self.statement(statement))
                            .collect(),
                    })
                    .collect(),
            },
            Statement::ThrowStatement(statement) => {
                HirStatementKind::Throw(self.expression(&statement.argument))
            }
            Statement::TryStatement(statement) => {
                let handler = match &statement.handler {
                    Some(clause) => {
                        let binding = match &clause.param {
                            None => None,
                            Some(param) => match &param.pattern {
                                BindingPattern::BindingIdentifier(identifier) => {
                                    Some(identifier.name.to_string())
                                }
                                _ => {
                                    return Some(HirStatement {
                                        kind: self.unsupported_statement(
                                            "destructuring catch binding",
                                            statement_span,
                                        ),
                                        span: statement_span,
                                    });
                                }
                            },
                        };
                        Some(kiln_hir::CatchClause {
                            binding,
                            body: clause
                                .body
                                .body
                                .iter()
                                .filter_map(|statement| self.statement(statement))
                                .collect(),
                        })
                    }
                    None => None,
                };
                HirStatementKind::Try {
                    block: statement
                        .block
                        .body
                        .iter()
                        .filter_map(|statement| self.statement(statement))
                        .collect(),
                    handler,
                    finalizer: statement
                        .finalizer
                        .as_ref()
                        .map(|block| {
                            block
                                .body
                                .iter()
                                .filter_map(|statement| self.statement(statement))
                                .collect()
                        })
                        .unwrap_or_default(),
                }
            }
            _ => self.unsupported_statement("statement", statement_span),
        };
        Some(HirStatement {
            kind,
            span: statement_span,
        })
    }

    fn declaration(
        &mut self,
        declaration: &Declaration<'_>,
        statement_span: HirSpan,
    ) -> Option<HirStatement> {
        let kind = match declaration {
            Declaration::VariableDeclaration(declaration) => self.variable(declaration),
            Declaration::TSTypeAliasDeclaration(_)
            | Declaration::TSInterfaceDeclaration(_)
            | Declaration::TSGlobalDeclaration(_) => return None,
            // `export function f() {}` is a declaration that also names an export. The export
            // side is read from the module record separately, so lowering only has to produce
            // the declaration -- the same one the unexported form produces.
            Declaration::FunctionDeclaration(declaration) => {
                self.function_declaration(declaration, statement_span)
            }
            Declaration::ClassDeclaration(declaration) => {
                match self.class(declaration, statement_span) {
                    Some(class) => HirStatementKind::Class(Box::new(class)),
                    None => HirStatementKind::Unsupported {
                        feature: "class declaration".to_string(),
                    },
                }
            }
            _ => self.unsupported_statement("TypeScript runtime declaration", statement_span),
        };
        Some(HirStatement {
            kind,
            span: statement_span,
        })
    }

    fn variable(&mut self, declaration: &VariableDeclaration<'_>) -> HirStatementKind {
        let kind = match declaration.kind {
            VariableDeclarationKind::Var => HirVariableKind::Var,
            VariableDeclarationKind::Let => HirVariableKind::Let,
            VariableDeclarationKind::Const => HirVariableKind::Const,
            VariableDeclarationKind::Using | VariableDeclarationKind::AwaitUsing => {
                return self.unsupported_statement("using declaration", span(declaration.span));
            }
        };
        let declarations = declaration
            .declarations
            .iter()
            .map(|declarator| {
                let declarator_span = span(declarator.span);
                let pattern = self.pattern(&declarator.id, declarator_span);
                let mut initializer = declarator
                    .init
                    .as_ref()
                    .map(|expression| self.expression(expression));
                // `const f = () => {}` gives the function the name `f`. The specification calls
                // this named evaluation; the practical effect is that a bundle full of arrows
                // assigned to variables produces a readable stack trace instead of a column of
                // `<anonymous>`.
                if let (kiln_hir::Pattern::Identifier(name), Some(value)) =
                    (&pattern, &mut initializer)
                {
                    name_anonymous_function(value, name);
                }
                HirVariableDeclarator {
                    pattern,
                    initializer,
                    span: declarator_span,
                }
            })
            .collect();
        HirStatementKind::Variable { kind, declarations }
    }

    fn statement_or_unsupported(&mut self, statement: &Statement<'_>) -> HirStatement {
        self.statement(statement).unwrap_or_else(|| HirStatement {
            kind: self.unsupported_statement(
                "erased declaration in executable position",
                span(statement.span()),
            ),
            span: span(statement.span()),
        })
    }

    fn expression(&mut self, expression: &Expression<'_>) -> HirExpression {
        let expression_span = span(expression.span());
        // Oxc's current enum is exhaustive, but retain a defensive fallback for future AST
        // variants rather than silently treating a newly added expression as supported.
        #[allow(unreachable_patterns)]
        let kind = match expression {
            Expression::BooleanLiteral(literal) => {
                HirExpressionKind::Literal(HirLiteral::Boolean(literal.value))
            }
            Expression::NullLiteral(_) => HirExpressionKind::Literal(HirLiteral::Null),
            Expression::NumericLiteral(literal) => {
                HirExpressionKind::Literal(HirLiteral::Number(literal.value.to_bits()))
            }
            Expression::StringLiteral(literal) => {
                HirExpressionKind::Literal(HirLiteral::String(literal.value.to_string()))
            }
            Expression::Identifier(identifier) => {
                HirExpressionKind::Identifier(identifier.name.to_string())
            }
            Expression::ParenthesizedExpression(expression) => {
                return self.expression(&expression.expression);
            }
            Expression::TSAsExpression(expression) => {
                return self.expression(&expression.expression);
            }
            Expression::TSSatisfiesExpression(expression) => {
                return self.expression(&expression.expression);
            }
            Expression::TSTypeAssertion(expression) => {
                return self.expression(&expression.expression);
            }
            Expression::TSNonNullExpression(expression) => {
                return self.expression(&expression.expression);
            }
            // `delete o.x` must not evaluate `o.x`, so it is not a unary operator over a member
            // read even though the syntax says it is.
            Expression::UnaryExpression(expression)
                if matches!(expression.operator, UnaryOperator::Delete) =>
            {
                let Expression::StaticMemberExpression(member) = &expression.argument else {
                    if let Expression::ComputedMemberExpression(member) = &expression.argument {
                        return HirExpression {
                            kind: HirExpressionKind::Delete {
                                object: Box::new(self.expression(&member.object)),
                                property: Box::new(kiln_hir::ObjectKey::Computed(
                                    self.expression(&member.expression),
                                )),
                            },
                            span: expression_span,
                        };
                    }
                    // `delete x` on a plain binding is a no-op that returns false in strict mode
                    // and cannot be spelled at all in a module; refusing beats guessing.
                    return self.unsupported_expression(
                        "delete of something other than a property",
                        expression_span,
                    );
                };
                HirExpressionKind::Delete {
                    object: Box::new(self.expression(&member.object)),
                    property: Box::new(kiln_hir::ObjectKey::Static(
                        member.property.name.to_string(),
                    )),
                }
            }
            Expression::UnaryExpression(expression) => {
                let Some(operator) = lower_unary_operator(expression.operator) else {
                    return self.unsupported_expression(
                        &format!("unary operator `{}`", expression.operator.as_str()),
                        expression_span,
                    );
                };
                HirExpressionKind::Unary {
                    operator,
                    argument: Box::new(self.expression(&expression.argument)),
                }
            }
            Expression::BinaryExpression(expression) => {
                let Some(operator) = lower_binary_operator(expression.operator) else {
                    return self.unsupported_expression(
                        &format!("binary operator `{}`", expression.operator.as_str()),
                        expression_span,
                    );
                };
                HirExpressionKind::Binary {
                    operator,
                    left: Box::new(self.expression(&expression.left)),
                    right: Box::new(self.expression(&expression.right)),
                }
            }
            Expression::LogicalExpression(expression) => HirExpressionKind::Logical {
                operator: match expression.operator {
                    LogicalOperator::And => HirLogicalOperator::And,
                    LogicalOperator::Or => HirLogicalOperator::Or,
                    LogicalOperator::Coalesce => HirLogicalOperator::NullishCoalesce,
                },
                left: Box::new(self.expression(&expression.left)),
                right: Box::new(self.expression(&expression.right)),
            },
            Expression::AssignmentExpression(expression) => {
                let Some(operator) = lower_assignment_operator(expression.operator) else {
                    return self.unsupported_expression(
                        &format!("assignment operator `{}`", expression.operator.as_str()),
                        expression_span,
                    );
                };
                match &expression.left {
                    AssignmentTarget::AssignmentTargetIdentifier(identifier) => {
                        HirExpressionKind::Assignment {
                            operator,
                            target: identifier.name.to_string(),
                            value: Box::new(self.expression(&expression.right)),
                        }
                    }
                    AssignmentTarget::ComputedMemberExpression(member) if !member.optional => {
                        HirExpressionKind::ComputedAssignment {
                            operator,
                            object: Box::new(self.expression(&member.object)),
                            property: Box::new(self.expression(&member.expression)),
                            value: Box::new(self.expression(&expression.right)),
                        }
                    }
                    // `super.x = v` assigns to `this`, not to the parent's prototype: the
                    // prototype is only consulted for a setter, and a write with none creates an
                    // own property on the instance. The one case this gets wrong is a class that
                    // defines `set x` *and* writes `super.x` inside it -- the idiom for avoiding
                    // recursion -- where the specification starts the search above the home
                    // object and this starts at it.
                    AssignmentTarget::StaticMemberExpression(member)
                        if !member.optional && matches!(member.object, Expression::Super(_)) =>
                    {
                        HirExpressionKind::MemberAssignment {
                            operator,
                            object: Box::new(HirExpression {
                                kind: HirExpressionKind::This,
                                span: expression_span,
                            }),
                            property: member.property.name.to_string(),
                            value: Box::new(self.expression(&expression.right)),
                        }
                    }
                    AssignmentTarget::StaticMemberExpression(member) if !member.optional => {
                        HirExpressionKind::MemberAssignment {
                            operator,
                            object: Box::new(self.expression(&member.object)),
                            property: member.property.name.to_string(),
                            value: Box::new(self.expression(&expression.right)),
                        }
                    }
                    AssignmentTarget::PrivateFieldExpression(member) => {
                        HirExpressionKind::PrivateAssignment {
                            operator,
                            object: Box::new(self.expression(&member.object)),
                            name: member.field.name.to_string(),
                            value: Box::new(self.expression(&expression.right)),
                        }
                    }
                    // `[a, b] = pair` and `({ a } = record)`. Only `=` takes a pattern, so a
                    // compound operator here is a different error than an unsupported shape.
                    AssignmentTarget::ArrayAssignmentTarget(_)
                    | AssignmentTarget::ObjectAssignmentTarget(_)
                        if expression.operator.as_str() == "=" =>
                    {
                        match self.assignment_pattern(&expression.left) {
                            Some(pattern) => HirExpressionKind::PatternAssignment {
                                pattern,
                                value: Box::new(self.expression(&expression.right)),
                            },
                            None => {
                                return self.unsupported_expression(
                                    "destructuring assignment target",
                                    expression_span,
                                );
                            }
                        }
                    }
                    _ => {
                        return self
                            .unsupported_expression("non-identifier assignment", expression_span);
                    }
                }
            }
            Expression::ConditionalExpression(expression) => HirExpressionKind::Conditional {
                test: Box::new(self.expression(&expression.test)),
                consequent: Box::new(self.expression(&expression.consequent)),
                alternate: Box::new(self.expression(&expression.alternate)),
            },
            Expression::CallExpression(expression)
                if !expression.optional && matches!(&expression.callee, Expression::Super(_)) =>
            {
                let mut arguments = Vec::with_capacity(expression.arguments.len());
                for argument in &expression.arguments {
                    let Some(argument) = argument.as_expression() else {
                        return self.unsupported_expression("spread argument", expression_span);
                    };
                    arguments.push(self.expression(argument));
                }
                HirExpressionKind::SuperCall { arguments }
            }
            Expression::CallExpression(expression) => HirExpressionKind::Call {
                callee: Box::new(self.expression(&expression.callee)),
                arguments: self.arguments(&expression.arguments),
                optional: expression.optional,
            },
            Expression::StaticMemberExpression(member)
                if !member.optional && matches!(&member.object, Expression::Super(_)) =>
            {
                HirExpressionKind::SuperMember {
                    property: member.property.name.to_string(),
                }
            }
            Expression::StaticMemberExpression(member) => HirExpressionKind::StaticMember {
                object: Box::new(self.expression(&member.object)),
                property: member.property.name.to_string(),
                optional: member.optional,
            },
            Expression::ComputedMemberExpression(member) => HirExpressionKind::ComputedMember {
                object: Box::new(self.expression(&member.object)),
                property: Box::new(self.expression(&member.expression)),
                optional: member.optional,
            },
            Expression::PrivateFieldExpression(member) => HirExpressionKind::PrivateMember {
                object: Box::new(self.expression(&member.object)),
                name: member.field.name.to_string(),
            },
            Expression::CallExpression(_) => {
                return self.unsupported_expression("optional call", expression_span);
            }
            Expression::ObjectExpression(object) => {
                return self.object_literal(object, expression_span);
            }
            Expression::ArrayExpression(expression) => {
                let mut elements = Vec::with_capacity(expression.elements.len());
                for element in &expression.elements {
                    match element {
                        ArrayExpressionElement::SpreadElement(spread) => {
                            elements.push(kiln_hir::ListElement::Spread(
                                self.expression(&spread.argument),
                            ));
                        }
                        ArrayExpressionElement::Elision(_) => {
                            elements.push(kiln_hir::ListElement::Hole);
                        }
                        other => {
                            let Some(value) = other.as_expression() else {
                                return self
                                    .unsupported_expression("array element", expression_span);
                            };
                            elements.push(kiln_hir::ListElement::Item(self.expression(value)));
                        }
                    }
                }
                HirExpressionKind::Array { elements }
            }
            Expression::ArrowFunctionExpression(arrow) => {
                let Some(function) = self.arrow_function(arrow, expression_span) else {
                    return self.unsupported_expression("arrow function", expression_span);
                };
                HirExpressionKind::Function(Box::new(function))
            }
            Expression::FunctionExpression(function) => {
                let Some(function) = self.function(function, expression_span) else {
                    return self.unsupported_expression("function expression", expression_span);
                };
                HirExpressionKind::Function(Box::new(function))
            }
            Expression::ClassExpression(class) => match self.class(class, expression_span) {
                Some(class) => HirExpressionKind::Class(Box::new(class)),
                None => {
                    return self.unsupported_expression("class expression", expression_span);
                }
            },
            Expression::NewExpression(expression) => HirExpressionKind::New {
                callee: Box::new(self.expression(&expression.callee)),
                arguments: self.arguments(&expression.arguments),
            },
            Expression::TemplateLiteral(literal) => HirExpressionKind::Template {
                quasis: literal
                    .quasis
                    .iter()
                    .map(|quasi| {
                        // `cooked` is None only when the raw text has an invalid escape, which
                        // is a syntax error in an untagged template -- so oxc has already
                        // reported it and the empty string keeps lowering going.
                        quasi
                            .value
                            .cooked
                            .as_ref()
                            .map_or_else(String::new, |text| text.to_string())
                    })
                    .collect(),
                expressions: literal
                    .expressions
                    .iter()
                    .map(|expression| self.expression(expression))
                    .collect(),
            },
            // A tag receives the raw strings alongside the cooked ones and decides what to build
            // from them, which is a different node and not a template that happens to be called.
            Expression::TaggedTemplateExpression(tagged) => {
                // A tagged template *is* a call: the tag is evaluated as a callee, with the
                // receiver an `obj.tag` form implies, and the arguments are the template object
                // followed by each substitution. Building it as a `Call` here means none of that
                // has to be reimplemented in lowering.
                let mut arguments = Vec::with_capacity(tagged.quasi.expressions.len() + 1);
                arguments.push(ListElement::Item(HirExpression {
                    kind: HirExpressionKind::TemplateStrings {
                        cooked: tagged
                            .quasi
                            .quasis
                            .iter()
                            .map(|quasi| {
                                quasi
                                    .value
                                    .cooked
                                    .as_ref()
                                    .map(std::string::ToString::to_string)
                            })
                            .collect(),
                        raw: tagged
                            .quasi
                            .quasis
                            .iter()
                            .map(|quasi| quasi.value.raw.to_string())
                            .collect(),
                    },
                    span: expression_span,
                }));
                for substitution in &tagged.quasi.expressions {
                    arguments.push(ListElement::Item(self.expression(substitution)));
                }
                HirExpressionKind::Call {
                    callee: Box::new(self.expression(&tagged.tag)),
                    arguments,
                    optional: false,
                }
            }
            Expression::AwaitExpression(expression) => {
                HirExpressionKind::Await(Box::new(self.expression(&expression.argument)))
            }
            #[expect(unreachable_patterns, reason = "the arm above now covers this")]
            Expression::AwaitExpression(_) => {
                return self.unsupported_expression("await expression", expression_span);
            }
            Expression::YieldExpression(yielded) => HirExpressionKind::Yield {
                delegate: yielded.delegate,
                value: yielded
                    .argument
                    .as_ref()
                    .map(|value| Box::new(self.expression(value))),
            },
            Expression::UpdateExpression(update)
                if !matches!(
                    &update.argument,
                    SimpleAssignmentTarget::AssignmentTargetIdentifier(_)
                ) =>
            {
                let (object, property) = match &update.argument {
                    SimpleAssignmentTarget::StaticMemberExpression(member) => (
                        self.expression(&member.object),
                        kiln_hir::ObjectKey::Static(member.property.name.to_string()),
                    ),
                    SimpleAssignmentTarget::ComputedMemberExpression(member) => (
                        self.expression(&member.object),
                        kiln_hir::ObjectKey::Computed(self.expression(&member.expression)),
                    ),
                    _ => {
                        return self
                            .unsupported_expression("update of that target", expression_span);
                    }
                };
                HirExpressionKind::UpdateMember {
                    increment: matches!(update.operator, UpdateOperator::Increment),
                    prefix: update.prefix,
                    object: Box::new(object),
                    property: Box::new(property),
                }
            }
            Expression::UpdateExpression(update) => {
                let SimpleAssignmentTarget::AssignmentTargetIdentifier(identifier) =
                    &update.argument
                else {
                    unreachable!("the arm above covers every other target");
                };
                HirExpressionKind::Update {
                    increment: matches!(update.operator, UpdateOperator::Increment),
                    prefix: update.prefix,
                    target: identifier.name.to_string(),
                }
            }
            Expression::ThisExpression(_) => HirExpressionKind::This,
            Expression::SequenceExpression(sequence) => HirExpressionKind::Sequence {
                operands: sequence
                    .expressions
                    .iter()
                    .map(|operand| self.expression(operand))
                    .collect(),
            },
            Expression::BigIntLiteral(literal) => {
                // The raw text carries the base prefix and the `n` suffix, and may carry numeric
                // separators. Normalising to plain decimal digits here means nothing downstream
                // has to know how the source spelled it.
                match literal
                    .raw
                    .as_ref()
                    .and_then(|raw| bigint_digits(raw.as_str()))
                {
                    Some(digits) => HirExpressionKind::Literal(HirLiteral::BigInt(digits)),
                    None => return self.unsupported_expression("bigint literal", expression_span),
                }
            }
            Expression::RegExpLiteral(literal) => HirExpressionKind::RegExp {
                // The source text, not a compiled program: `re.source` hands the pattern back to
                // the program, and compiling belongs to whoever owns the engine.
                pattern: literal.regex.pattern.text.to_string(),
                flags: literal.regex.flags.to_string(),
            },
            Expression::Super(_) => {
                return self.unsupported_expression("super expression", expression_span);
            }
            // The chain is what the `?.` jumps leave, so the boundary is marked here rather
            // than inferred later from where the optional links happen to sit.
            Expression::ChainExpression(chain) => {
                let inner = match &chain.expression {
                    oxc_ast::ast::ChainElement::CallExpression(call) => HirExpression {
                        kind: HirExpressionKind::Call {
                            callee: Box::new(self.expression(&call.callee)),
                            arguments: self.arguments(&call.arguments),
                            optional: call.optional,
                        },
                        span: expression_span,
                    },
                    other => match other.as_member_expression() {
                        Some(member) => self.member_expression(member, expression_span),
                        None => self.unsupported_expression("chain element", expression_span),
                    },
                };
                // A chain around something already refused adds nothing, and wrapping it would
                // hide which construct was refused from anything matching on the kind.
                if matches!(inner.kind, HirExpressionKind::Unsupported { .. }) {
                    return inner;
                }
                HirExpressionKind::Chain {
                    expression: Box::new(inner),
                }
            }
            Expression::ImportExpression(_) => {
                return self.unsupported_expression("dynamic import expression", expression_span);
            }
            // `#x in o` is the brand check: it asks whether `o` carries this class's private
            // field, which is exactly what reading one already tests for.
            Expression::PrivateInExpression(expression) => HirExpressionKind::PrivateIn {
                name: expression.left.name.to_string(),
                object: Box::new(self.expression(&expression.right)),
            },
            Expression::PrivateInExpression(_) => {
                return self.unsupported_expression("private-in expression", expression_span);
            }
            Expression::ImportMeta(_) => {
                return self.unsupported_expression("import.meta expression", expression_span);
            }
            Expression::NewTarget(_) => HirExpressionKind::NewTarget,
            Expression::JSXElement(_) | Expression::JSXFragment(_) => {
                return self.unsupported_expression("JSX expression", expression_span);
            }
            Expression::TSInstantiationExpression(_) => {
                return self.unsupported_expression(
                    "TypeScript instantiation expression",
                    expression_span,
                );
            }
            Expression::V8IntrinsicExpression(_) => {
                return self.unsupported_expression("V8 intrinsic expression", expression_span);
            }
            _ => return self.unsupported_expression("expression", expression_span),
        };
        HirExpression {
            kind,
            span: expression_span,
        }
    }

    /// Lower a function declaration, or refuse it as a whole.
    ///
    /// Refusing the declaration outright rather than lowering a partial function is deliberate:
    /// a generator whose `yield`s were dropped, or an async function whose `await`s were, would
    /// be a function that runs and produces a confidently wrong answer. A missing function is a
    /// diagnostic; a silently altered one is a bug report six months later.
    /// A `export default function`/`class` whose lowering may have been refused.
    fn default_declaration(
        &mut self,
        lowered: Option<HirStatementKind>,
        statement_span: HirSpan,
    ) -> HirStatementKind {
        match lowered {
            Some(kind) => kind,
            None => self.unsupported_statement("default function or class export", statement_span),
        }
    }

    /// An assignment target read as a binding pattern.
    ///
    /// `None` when the target holds something a [`kiln_hir::Pattern`] cannot say -- a member
    /// expression, `[o.a] = xs`, is legal JavaScript and is refused here rather than lowered to
    /// something else. Patterns bind names; teaching them to store through a member would make
    /// them a second assignment language.
    fn assignment_pattern(&mut self, target: &AssignmentTarget<'_>) -> Option<kiln_hir::Pattern> {
        match target {
            AssignmentTarget::AssignmentTargetIdentifier(identifier) => {
                Some(kiln_hir::Pattern::Identifier(identifier.name.to_string()))
            }
            AssignmentTarget::ArrayAssignmentTarget(array) => {
                let mut elements = Vec::with_capacity(array.elements.len());
                for element in &array.elements {
                    elements.push(match element {
                        // A hole assigns nothing, and skips the index all the same.
                        None => None,
                        Some(element) => Some(self.assignment_maybe_default(element)?),
                    });
                }
                let rest = match &array.rest {
                    Some(rest) => Some(Box::new(self.assignment_pattern(&rest.target)?)),
                    None => None,
                };
                Some(kiln_hir::Pattern::Array { elements, rest })
            }
            AssignmentTarget::ObjectAssignmentTarget(object) => {
                let mut properties = Vec::with_capacity(object.properties.len());
                for property in &object.properties {
                    match property {
                        oxc_ast::ast::AssignmentTargetProperty::AssignmentTargetPropertyIdentifier(
                            shorthand,
                        ) => {
                            let name = shorthand.binding.name.to_string();
                            let value = kiln_hir::Pattern::Identifier(name.clone());
                            properties.push(kiln_hir::PatternProperty {
                                key: name,
                                // `({ a = 1 } = o)`: the default belongs to the value, exactly
                                // as it does in a declaration.
                                value: match &shorthand.init {
                                    Some(default) => kiln_hir::Pattern::WithDefault {
                                        inner: Box::new(value),
                                        default: Box::new(self.expression(default)),
                                    },
                                    None => value,
                                },
                            });
                        }
                        oxc_ast::ast::AssignmentTargetProperty::AssignmentTargetPropertyProperty(
                            named,
                        ) => {
                            properties.push(kiln_hir::PatternProperty {
                                key: named.name.static_name()?.to_string(),
                                value: self.assignment_maybe_default(&named.binding)?,
                            });
                        }
                    }
                }
                let rest = match &object.rest {
                    Some(rest) => match &rest.target {
                        AssignmentTarget::AssignmentTargetIdentifier(identifier) => {
                            Some(identifier.name.to_string())
                        }
                        _ => return None,
                    },
                    None => None,
                };
                Some(kiln_hir::Pattern::Object { properties, rest })
            }
            _ => None,
        }
    }

    /// One element of a destructuring assignment, which may carry a default.
    fn assignment_maybe_default(
        &mut self,
        element: &oxc_ast::ast::AssignmentTargetMaybeDefault<'_>,
    ) -> Option<kiln_hir::Pattern> {
        match element {
            oxc_ast::ast::AssignmentTargetMaybeDefault::AssignmentTargetWithDefault(with) => {
                Some(kiln_hir::Pattern::WithDefault {
                    inner: Box::new(self.assignment_pattern(&with.binding)?),
                    default: Box::new(self.expression(&with.init)),
                })
            }
            other => self.assignment_pattern(other.as_assignment_target()?),
        }
    }

    fn function_declaration(
        &mut self,
        declaration: &Function<'_>,
        statement_span: HirSpan,
    ) -> HirStatementKind {
        let Some(function) = self.function(declaration, statement_span) else {
            return HirStatementKind::Unsupported {
                feature: "function declaration".to_string(),
            };
        };
        HirStatementKind::Function(Box::new(function))
    }

    /// A `for` initialiser is either a declaration or an expression.
    fn for_init(
        &mut self,
        init: &ForStatementInit<'_>,
        statement_span: HirSpan,
    ) -> Option<HirStatement> {
        match init {
            ForStatementInit::VariableDeclaration(declaration) => Some(HirStatement {
                kind: self.variable(declaration),
                span: span(declaration.span),
            }),
            other => {
                let expression = other.as_expression()?;
                Some(HirStatement {
                    kind: HirStatementKind::Expression(self.expression(expression)),
                    span: statement_span,
                })
            }
        }
    }

    /// Lower a binding pattern.
    ///
    /// An unsupported shape becomes an identifier binding a name no source can produce, so the
    /// diagnostic already recorded is the failure the user sees rather than a second, confusing
    /// one about an unresolved name.
    fn pattern(&mut self, pattern: &BindingPattern<'_>, span: HirSpan) -> kiln_hir::Pattern {
        match pattern {
            BindingPattern::BindingIdentifier(identifier) => {
                kiln_hir::Pattern::Identifier(identifier.name.to_string())
            }
            BindingPattern::AssignmentPattern(assignment) => kiln_hir::Pattern::WithDefault {
                inner: Box::new(self.pattern(&assignment.left, span)),
                default: Box::new(self.expression(&assignment.right)),
            },
            BindingPattern::ObjectPattern(object) => {
                let mut properties = Vec::with_capacity(object.properties.len());
                for property in &object.properties {
                    if property.computed {
                        self.unsupported("computed key in a destructuring pattern", span);
                        return kiln_hir::Pattern::Identifier(String::new());
                    }
                    let Some(key) = property.key.static_name() else {
                        self.unsupported("destructuring key", span);
                        return kiln_hir::Pattern::Identifier(String::new());
                    };
                    properties.push(kiln_hir::PatternProperty {
                        key: key.to_string(),
                        value: self.pattern(&property.value, span),
                    });
                }
                let rest = match &object.rest {
                    None => None,
                    Some(rest) => match &rest.argument {
                        BindingPattern::BindingIdentifier(identifier) => {
                            Some(identifier.name.to_string())
                        }
                        // `{ ...{ a } }` is not valid as a binding rest, and anything else here
                        // would need a nested pattern the rest object does not have.
                        _ => {
                            self.unsupported("destructuring rest pattern", span);
                            return kiln_hir::Pattern::Identifier(String::new());
                        }
                    },
                };
                kiln_hir::Pattern::Object { properties, rest }
            }
            BindingPattern::ArrayPattern(array) => kiln_hir::Pattern::Array {
                elements: array
                    .elements
                    .iter()
                    .map(|element| element.as_ref().map(|element| self.pattern(element, span)))
                    .collect(),
                rest: array
                    .rest
                    .as_ref()
                    .map(|rest| Box::new(self.pattern(&rest.argument, span))),
            },
        }
    }

    /// Lower a class, or refuse it whole.
    ///
    /// Refusing wholesale for the same reason a partial function is refused: a class missing a
    /// method it declared would construct successfully and fail somewhere else entirely.
    /// The key a class member declares, including the private form.
    ///
    /// A private name is not a string key: `#x` and `"#x"` name different properties, and only
    /// the first is hidden from everything that enumerates.
    fn member_key(
        &mut self,
        key: &oxc_ast::ast::PropertyKey<'_>,
        computed: bool,
    ) -> Option<kiln_hir::ObjectKey> {
        if let oxc_ast::ast::PropertyKey::PrivateIdentifier(private) = key {
            return Some(kiln_hir::ObjectKey::Private(private.name.to_string()));
        }
        if computed {
            return Some(kiln_hir::ObjectKey::Computed(
                self.expression(key.as_expression()?),
            ));
        }
        Some(kiln_hir::ObjectKey::Static(key.static_name()?.to_string()))
    }

    fn class(&mut self, class: &Class<'_>, statement_span: HirSpan) -> Option<kiln_hir::Class> {
        let mut constructor = None;
        let mut methods = Vec::new();
        let mut fields = Vec::new();

        for element in &class.body.body {
            if let ClassElement::PropertyDefinition(property) = element {
                let key = self.member_key(&property.key, property.computed)?;
                fields.push(kiln_hir::ClassField {
                    key,
                    value: property.value.as_ref().map(|value| self.expression(value)),
                    is_static: property.r#static,
                });
                continue;
            }
            let ClassElement::MethodDefinition(method) = element else {
                self.unsupported("class static block", statement_span);
                return None;
            };
            if method.value.r#async {
                self.unsupported("async method", statement_span);
                return None;
            }
            let lowered = self.function(&method.value, span(method.span))?;
            match method.kind {
                MethodDefinitionKind::Constructor => constructor = Some(lowered),
                kind @ (MethodDefinitionKind::Method
                | MethodDefinitionKind::Get
                | MethodDefinitionKind::Set) => {
                    let key = self.member_key(&method.key, method.computed)?;
                    methods.push(kiln_hir::ClassMethod {
                        key,
                        function: lowered,
                        is_static: method.r#static,
                        kind: match kind {
                            MethodDefinitionKind::Get => kiln_hir::MethodKind::Get,
                            MethodDefinitionKind::Set => kiln_hir::MethodKind::Set,
                            _ => kiln_hir::MethodKind::Normal,
                        },
                    });
                }
            }
        }

        Some(kiln_hir::Class {
            name: class.id.as_ref().map(|id| id.name.to_string()),
            superclass: class
                .heritage
                .as_ref()
                .map(|heritage| self.expression(&heritage.expression)),
            constructor,
            methods,
            fields,
            span: statement_span,
        })
    }

    /// Lower a parameter list, including defaults and a rest parameter.
    ///
    /// oxc keeps a parameter's default on the `FormalParameter` rather than inside its binding
    /// pattern, so reading only `item.pattern` sees a plain identifier for `f(x = 1)` and drops
    /// the default without a word. That is exactly the silent-alteration this frontend refuses
    /// everywhere else, and it is why the default is folded in here rather than in `pattern`.
    fn parameters(
        &mut self,
        parameters: &FormalParameters<'_>,
    ) -> (Vec<HirParameter>, Option<kiln_hir::Pattern>) {
        let lowered = parameters
            .items
            .iter()
            .map(|item| {
                let item_span = span(item.span);
                let pattern = self.pattern(&item.pattern, item_span);
                HirParameter {
                    pattern: match &item.initializer {
                        Some(default) => kiln_hir::Pattern::WithDefault {
                            inner: Box::new(pattern),
                            default: Box::new(self.expression(default)),
                        },
                        None => pattern,
                    },
                    span: item_span,
                }
            })
            .collect();
        let rest = parameters
            .rest
            .as_ref()
            .map(|rest| self.pattern(&rest.rest.argument, span(rest.span)));
        (lowered, rest)
    }

    /// Lower an arrow function.
    ///
    /// An expression body becomes a single `return`, which is what it means. The arrow flag is
    /// carried even though neither `this` nor `arguments` is implemented: the difference is real,
    /// and a later step that has to reconstruct it from the shape of the body would get it wrong.
    fn arrow_function(
        &mut self,
        arrow: &ArrowFunctionExpression<'_>,
        expression_span: HirSpan,
    ) -> Option<HirFunction> {
        let (parameters, rest) = self.parameters(&arrow.params);

        let body = match &arrow.body {
            ArrowFunctionBody::FunctionBody(block) => block
                .statements
                .iter()
                .filter_map(|statement| self.statement(statement))
                .collect(),
            // A concise body (`x => x + 1`) is an expression whose value is returned, not an
            // expression statement whose value is discarded.
            concise => {
                let Some(expression) = concise.as_expression() else {
                    self.unsupported("arrow function body", expression_span);
                    return None;
                };
                vec![HirStatement {
                    kind: HirStatementKind::Return(Some(self.expression(expression))),
                    span: expression_span,
                }]
            }
        };

        Some(HirFunction {
            name: None,
            // An arrow can never be given a name, so it never binds one.
            binds_own_name: false,
            arrow: true,
            generator: false,
            asynchronous: arrow.r#async,
            parameters,
            rest,
            body,
            span: expression_span,
        })
    }

    /// The shared part of every function form. `None` means a diagnostic was already recorded.
    fn function(
        &mut self,
        declaration: &Function<'_>,
        statement_span: HirSpan,
    ) -> Option<HirFunction> {
        let (parameters, rest) = self.parameters(&declaration.params);

        let body = declaration.body.as_ref()?;
        let statements = body
            .statements
            .iter()
            .filter_map(|statement| self.statement(statement))
            .collect();

        Some(HirFunction {
            name: declaration.id.as_ref().map(|id| id.name.to_string()),
            binds_own_name: declaration.id.is_some(),
            arrow: false,
            generator: declaration.generator,
            asynchronous: declaration.r#async,
            parameters,
            rest,
            body: statements,
            span: statement_span,
        })
    }

    /// The `kind` and single bound name of a `for-in` or `for-of` head, when it is a simple
    /// declaration of one identifier. `None` for every other shape, each of which needs its own
    /// lowering and gets its own diagnostic at the call site.
    fn for_binding(&mut self, left: &ForStatementLeft<'_>) -> Option<(HirVariableKind, String)> {
        let ForStatementLeft::VariableDeclaration(declaration) = left else {
            return None;
        };
        let declarator = declaration.declarations.first()?;
        let BindingPattern::BindingIdentifier(identifier) = &declarator.id else {
            return None;
        };
        let kind = match declaration.kind {
            VariableDeclarationKind::Var => HirVariableKind::Var,
            VariableDeclarationKind::Const => HirVariableKind::Const,
            _ => HirVariableKind::Let,
        };
        Some((kind, identifier.name.to_string()))
    }

    /// Lower a member access, optional or not. Shared with the chain path, where the same three
    /// shapes appear behind a `?.`.
    fn member_expression(
        &mut self,
        member: &oxc_ast::ast::MemberExpression<'_>,
        expression_span: HirSpan,
    ) -> HirExpression {
        let kind = match member {
            oxc_ast::ast::MemberExpression::StaticMemberExpression(member) => {
                HirExpressionKind::StaticMember {
                    object: Box::new(self.expression(&member.object)),
                    property: member.property.name.to_string(),
                    optional: member.optional,
                }
            }
            oxc_ast::ast::MemberExpression::ComputedMemberExpression(member) => {
                HirExpressionKind::ComputedMember {
                    object: Box::new(self.expression(&member.object)),
                    property: Box::new(self.expression(&member.expression)),
                    optional: member.optional,
                }
            }
            oxc_ast::ast::MemberExpression::PrivateFieldExpression(member) => {
                HirExpressionKind::PrivateMember {
                    object: Box::new(self.expression(&member.object)),
                    name: member.field.name.to_string(),
                }
            }
        };
        HirExpression {
            kind,
            span: expression_span,
        }
    }

    /// Lower an argument list, where a spread contributes however many an iterable holds.
    fn arguments(&mut self, arguments: &oxc_allocator::Vec<'_, Argument<'_>>) -> Vec<ListElement> {
        let mut lowered = Vec::with_capacity(arguments.len());
        for argument in arguments {
            lowered.push(match argument {
                Argument::SpreadElement(spread) => {
                    ListElement::Spread(self.expression(&spread.argument))
                }
                other => match other.as_expression() {
                    Some(expression) => ListElement::Item(self.expression(expression)),
                    // Every non-spread argument is an expression; this arm exists because the
                    // oxc enum cannot say so in its type.
                    None => ListElement::Item(
                        self.unsupported_expression("argument", HirSpan::default()),
                    ),
                },
            });
        }
        lowered
    }

    /// Lower an object literal, or refuse the whole literal.
    ///
    /// Refusing wholesale rather than dropping the members it cannot lower: an object missing
    /// some of its properties looks like a valid object and would be wrong everywhere it was
    /// used, whereas a missing object is a diagnostic at the point of the mistake.
    fn object_literal(
        &mut self,
        expression: &ObjectExpression<'_>,
        expression_span: HirSpan,
    ) -> HirExpression {
        let mut members = Vec::with_capacity(expression.properties.len());
        for property in &expression.properties {
            let property = match property {
                ObjectPropertyKind::ObjectProperty(property) => property,
                ObjectPropertyKind::SpreadProperty(spread) => {
                    members.push(kiln_hir::ObjectMember::Spread(
                        self.expression(&spread.argument),
                    ));
                    continue;
                }
            };
            // A shorthand method is a property whose value is a function expression. It differs
            // from one written longhand only in having no `prototype` and not being constructable,
            // neither of which is observable yet. A getter or setter is not: reading it is a call.
            if !matches!(property.kind, PropertyKind::Init) {
                let key = if property.computed {
                    match property.key.as_expression() {
                        Some(key) => kiln_hir::ObjectKey::Computed(self.expression(key)),
                        None => {
                            return self
                                .unsupported_expression("computed object key", expression_span);
                        }
                    }
                } else {
                    match property.key.static_name() {
                        Some(key) => kiln_hir::ObjectKey::Static(key.to_string()),
                        None => return self.unsupported_expression("object key", expression_span),
                    }
                };
                let Expression::FunctionExpression(function) = &property.value else {
                    return self.unsupported_expression("accessor body", expression_span);
                };
                let Some(function) = self.function(function, expression_span) else {
                    return self.unsupported_expression("accessor body", expression_span);
                };
                members.push(kiln_hir::ObjectMember::Accessor {
                    key,
                    kind: if matches!(property.kind, PropertyKind::Get) {
                        kiln_hir::MethodKind::Get
                    } else {
                        kiln_hir::MethodKind::Set
                    },
                    function,
                });
                continue;
            }
            let key = if property.computed {
                let Some(key) = property.key.as_expression() else {
                    return self.unsupported_expression("computed object key", expression_span);
                };
                kiln_hir::ObjectKey::Computed(self.expression(key))
            } else {
                let Some(key) = property.key.static_name() else {
                    return self.unsupported_expression("object key", expression_span);
                };
                kiln_hir::ObjectKey::Static(key.to_string())
            };
            let mut value = self.expression(&property.value);
            // `{ handle: () => {} }` names the function `handle`, for the same reason a
            // variable initialiser does.
            if let kiln_hir::ObjectKey::Static(name) = &key {
                name_anonymous_function(&mut value, name);
            }
            members.push(kiln_hir::ObjectMember::Property(kiln_hir::ObjectProperty {
                key,
                value,
                span: span(property.span),
            }));
        }
        HirExpression {
            kind: HirExpressionKind::Object { members },
            span: expression_span,
        }
    }

    fn unsupported_statement(&mut self, feature: &str, feature_span: HirSpan) -> HirStatementKind {
        self.unsupported(feature, feature_span);
        HirStatementKind::Unsupported {
            feature: feature.to_string(),
        }
    }

    fn unsupported_expression(&mut self, feature: &str, feature_span: HirSpan) -> HirExpression {
        self.unsupported(feature, feature_span);
        HirExpression {
            kind: HirExpressionKind::Unsupported {
                feature: feature.to_string(),
            },
            span: feature_span,
        }
    }

    fn unsupported(&mut self, feature: &str, feature_span: HirSpan) {
        // Unsupported-but-valid syntax is represented in executable HIR and diagnosed by the
        // IR compiler. Keeping it out of parser diagnostics lets `kiln check` remain a syntax
        // check even while the execution surface grows.
        let _ = (feature, feature_span);
    }
}

fn lower_import_name(name: &ImportImportName<'_>) -> ImportName {
    match name {
        ImportImportName::Name(name) => ImportName::Name(name.name.to_string()),
        ImportImportName::NamespaceObject => ImportName::Namespace,
        ImportImportName::Default(_) => ImportName::Default,
    }
}

fn lower_export(entry: &OxcExportEntry<'_>) -> Export {
    let exported = match &entry.export_name {
        ExportExportName::Name(name) => Some(ExportName::Name(name.name.to_string())),
        ExportExportName::Default(_) => Some(ExportName::Default),
        ExportExportName::Null => None,
    };
    let source = entry
        .module_request
        .as_ref()
        .map(|request| request.name.to_string());
    let origin = match (source, &entry.import_name, &entry.local_name) {
        (None, ExportImportName::Null, ExportLocalName::Name(local))
        | (None, ExportImportName::Null, ExportLocalName::Default(local)) => ExportOrigin::Local {
            local: local.name.to_string(),
        },
        (None, ExportImportName::Null, ExportLocalName::Null) => ExportOrigin::DefaultExpression,
        (Some(source), ExportImportName::Name(imported), _) => ExportOrigin::ReExport {
            source,
            imported: ImportName::Name(imported.name.to_string()),
        },
        (Some(source), ExportImportName::All, _) => ExportOrigin::NamespaceReExport { source },
        (Some(source), ExportImportName::AllButDefault, _) => ExportOrigin::Star { source },
        (source, import_name, _) => ExportOrigin::Unsupported {
            source,
            description: format!("unexpected export import name {import_name:?}"),
        },
    };
    Export {
        exported,
        origin,
        type_only: entry.is_type,
        span: span(entry.span),
        statement_span: span(entry.statement_span),
    }
}

fn lower_unary_operator(operator: UnaryOperator) -> Option<HirUnaryOperator> {
    match operator {
        UnaryOperator::UnaryPlus => Some(HirUnaryOperator::Plus),
        UnaryOperator::UnaryNegation => Some(HirUnaryOperator::Negate),
        UnaryOperator::LogicalNot => Some(HirUnaryOperator::Not),
        UnaryOperator::Typeof => Some(HirUnaryOperator::TypeOf),
        UnaryOperator::BitwiseNot => Some(HirUnaryOperator::BitwiseNot),
        UnaryOperator::Void => Some(HirUnaryOperator::Void),
        _ => None,
    }
}

/// Every binary operator oxc can produce now has an HIR counterpart, so there is no catch-all
/// here. It keeps returning `Option` to match its unary sibling -- and losing the catch-all means
/// the compiler, rather than a user, reports the day oxc grows another.
fn lower_binary_operator(operator: BinaryOperator) -> Option<HirBinaryOperator> {
    match operator {
        BinaryOperator::Addition => Some(HirBinaryOperator::Add),
        BinaryOperator::Subtraction => Some(HirBinaryOperator::Subtract),
        BinaryOperator::Multiplication => Some(HirBinaryOperator::Multiply),
        BinaryOperator::Division => Some(HirBinaryOperator::Divide),
        BinaryOperator::Remainder => Some(HirBinaryOperator::Remainder),
        BinaryOperator::Exponential => Some(HirBinaryOperator::Exponentiate),
        BinaryOperator::StrictEquality => Some(HirBinaryOperator::StrictEqual),
        BinaryOperator::In => Some(HirBinaryOperator::In),
        BinaryOperator::Instanceof => Some(HirBinaryOperator::InstanceOf),
        BinaryOperator::StrictInequality => Some(HirBinaryOperator::StrictNotEqual),
        BinaryOperator::Equality => Some(HirBinaryOperator::LooseEqual),
        BinaryOperator::Inequality => Some(HirBinaryOperator::LooseNotEqual),
        BinaryOperator::BitwiseAnd => Some(HirBinaryOperator::BitwiseAnd),
        BinaryOperator::BitwiseOR => Some(HirBinaryOperator::BitwiseOr),
        BinaryOperator::BitwiseXOR => Some(HirBinaryOperator::BitwiseXor),
        BinaryOperator::ShiftLeft => Some(HirBinaryOperator::ShiftLeft),
        BinaryOperator::ShiftRight => Some(HirBinaryOperator::ShiftRight),
        BinaryOperator::ShiftRightZeroFill => Some(HirBinaryOperator::UnsignedShiftRight),
        BinaryOperator::LessThan => Some(HirBinaryOperator::LessThan),
        BinaryOperator::LessEqualThan => Some(HirBinaryOperator::LessThanOrEqual),
        BinaryOperator::GreaterThan => Some(HirBinaryOperator::GreaterThan),
        BinaryOperator::GreaterEqualThan => Some(HirBinaryOperator::GreaterThanOrEqual),
    }
}

/// Every assignment operator oxc can produce now has an HIR counterpart, so there is no
/// catch-all -- and losing it means the compiler reports the day oxc grows another.
fn lower_assignment_operator(operator: AssignmentOperator) -> Option<HirAssignmentOperator> {
    match operator {
        AssignmentOperator::Assign => Some(HirAssignmentOperator::Assign),
        AssignmentOperator::Addition => Some(HirAssignmentOperator::Add),
        AssignmentOperator::Subtraction => Some(HirAssignmentOperator::Subtract),
        AssignmentOperator::Multiplication => Some(HirAssignmentOperator::Multiply),
        AssignmentOperator::Division => Some(HirAssignmentOperator::Divide),
        AssignmentOperator::Remainder => Some(HirAssignmentOperator::Remainder),
        AssignmentOperator::Exponential => Some(HirAssignmentOperator::Exponentiate),
        AssignmentOperator::BitwiseAnd => Some(HirAssignmentOperator::BitwiseAnd),
        AssignmentOperator::BitwiseOR => Some(HirAssignmentOperator::BitwiseOr),
        AssignmentOperator::BitwiseXOR => Some(HirAssignmentOperator::BitwiseXor),
        AssignmentOperator::ShiftLeft => Some(HirAssignmentOperator::ShiftLeft),
        AssignmentOperator::ShiftRight => Some(HirAssignmentOperator::ShiftRight),
        AssignmentOperator::ShiftRightZeroFill => Some(HirAssignmentOperator::UnsignedShiftRight),
        AssignmentOperator::LogicalAnd => Some(HirAssignmentOperator::LogicalAnd),
        AssignmentOperator::LogicalOr => Some(HirAssignmentOperator::LogicalOr),
        AssignmentOperator::LogicalNullish => Some(HirAssignmentOperator::Nullish),
    }
}

fn source_kind(source_type: SourceType) -> SourceKind {
    match (source_type.is_typescript(), source_type.is_jsx()) {
        (false, false) => SourceKind::JavaScript,
        (false, true) => SourceKind::Jsx,
        (true, false) => SourceKind::TypeScript,
        (true, true) => SourceKind::Tsx,
    }
}

fn item_kind(statement: &Statement<'_>) -> ItemKind {
    match statement {
        Statement::ImportDeclaration(_) | Statement::TSImportEqualsDeclaration(_) => {
            ItemKind::Import
        }
        Statement::ExportAllDeclaration(_)
        | Statement::ExportDefaultDeclaration(_)
        | Statement::ExportDeclaration(_)
        | Statement::ExportFromDeclaration(_)
        | Statement::ExportNamedDeclaration(_)
        | Statement::TSExportAssignment(_)
        | Statement::TSNamespaceExportDeclaration(_) => ItemKind::Export,
        Statement::VariableDeclaration(_) => ItemKind::Variable,
        Statement::FunctionDeclaration(_) => ItemKind::Function,
        Statement::ClassDeclaration(_) => ItemKind::Class,
        Statement::BlockStatement(_) => ItemKind::Block,
        Statement::ExpressionStatement(_) => ItemKind::Expression,
        Statement::EmptyStatement(_) => ItemKind::Empty,
        Statement::TSTypeAliasDeclaration(_)
        | Statement::TSInterfaceDeclaration(_)
        | Statement::TSEnumDeclaration(_)
        | Statement::TSExternalModuleDeclaration(_)
        | Statement::TSNamespaceDeclaration(_)
        | Statement::TSGlobalDeclaration(_) => ItemKind::TypeScript,
        Statement::BreakStatement(_)
        | Statement::ContinueStatement(_)
        | Statement::DoWhileStatement(_)
        | Statement::ForInStatement(_)
        | Statement::ForOfStatement(_)
        | Statement::ForStatement(_)
        | Statement::IfStatement(_)
        | Statement::ReturnStatement(_)
        | Statement::SwitchStatement(_)
        | Statement::ThrowStatement(_)
        | Statement::TryStatement(_)
        | Statement::WhileStatement(_) => ItemKind::ControlFlow,
        _ => ItemKind::Other,
    }
}

fn span(value: Span) -> HirSpan {
    HirSpan::new(value.start, value.end)
}

/// Give an anonymous function expression the name of the binding it is being assigned to.
///
/// Only functions: a class expression's name becomes a binding inside its own body, so inventing
/// one would invent a scope the program did not write. A function expression's name is metadata
/// -- `f.name` and a stack frame -- which is exactly what is missing without this.
fn name_anonymous_function(value: &mut kiln_hir::Expression, name: &str) {
    if let kiln_hir::ExpressionKind::Function(function) = &mut value.kind
        && function.name.is_none()
    {
        function.name = Some(name.to_owned());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_real_javascript_export_forms_without_scanning_comments_or_strings() {
        let module = parse(
            "module.js",
            r#"
                const local = 1;
                const text = "export const fake = 1";
                // export const commented = 2;
                export { local as answer };
                export default function () {}
            "#,
        );

        assert!(!module.panicked);
        assert!(module.diagnostics.is_empty(), "{:?}", module.diagnostics);
        assert_eq!(module.runtime_export_names(), ["answer", "default"]);
    }

    #[test]
    fn parses_typescript_and_excludes_type_only_exports_from_runtime_interface() {
        let module = parse(
            "module.ts",
            r#"
                export interface Shape { width: number }
                export type Id = string;
                export const value: Id = "ok";
            "#,
        );

        assert_eq!(module.source_kind, SourceKind::TypeScript);
        assert!(module.diagnostics.is_empty(), "{:?}", module.diagnostics);
        assert_eq!(module.runtime_export_names(), ["value"]);
    }

    #[test]
    fn records_dependencies_and_star_reexports() {
        let module = parse(
            "module.mjs",
            "import { a } from './a.js';\nexport * from './b.js';\n",
        );

        assert_eq!(
            module
                .requested_modules
                .iter()
                .map(|request| request.specifier.as_str())
                .collect::<Vec<_>>(),
            ["./a.js", "./b.js"]
        );
        assert!(module.exports.iter().any(|export| {
            export.exported.is_none()
                && matches!(
                    &export.origin,
                    ExportOrigin::Star { source } if source == "./b.js"
                )
        }));
        assert_eq!(module.imports.len(), 1);
        assert_eq!(module.imports[0].source, "./a.js");
        assert_eq!(module.imports[0].local, "a");
        assert_eq!(module.imports[0].imported, ImportName::Name("a".into()));
        assert_eq!(
            module.requested_modules[0].occurrences[0].kind,
            ModuleRequestKind::Import
        );
        assert_eq!(
            module.requested_modules[1].occurrences[0].kind,
            ModuleRequestKind::Export
        );
    }

    #[test]
    fn preserves_aliases_defaults_namespaces_and_type_only_bindings() {
        let module = parse(
            "module.ts",
            r#"
                import primary, { value as local, type Shape } from "./dep.ts";
                import * as namespace from "./other.ts";
                const result = local;
                export { result as answer };
                export { value as forwarded } from "./dep.ts";
            "#,
        );

        assert!(module.diagnostics.is_empty(), "{:?}", module.diagnostics);
        assert_eq!(module.imports.len(), 4);
        assert!(module.imports.iter().any(|entry| {
            entry.local == "primary" && entry.imported == ImportName::Default && !entry.type_only
        }));
        assert!(module.imports.iter().any(|entry| {
            entry.local == "local"
                && entry.imported == ImportName::Name("value".into())
                && !entry.type_only
        }));
        assert!(module.imports.iter().any(|entry| {
            entry.local == "Shape"
                && entry.imported == ImportName::Name("Shape".into())
                && entry.type_only
        }));
        assert!(module.imports.iter().any(|entry| {
            entry.local == "namespace" && entry.imported == ImportName::Namespace
        }));
        assert!(module.exports.iter().any(|entry| {
            entry.exported == Some(ExportName::Name("answer".into()))
                && entry.origin
                    == ExportOrigin::Local {
                        local: "result".into(),
                    }
        }));
        assert!(module.exports.iter().any(|entry| {
            entry.exported == Some(ExportName::Name("forwarded".into()))
                && entry.origin
                    == ExportOrigin::ReExport {
                        source: "./dep.ts".into(),
                        imported: ImportName::Name("value".into()),
                    }
        }));
    }

    #[test]
    fn default_expression_is_not_lowered_as_a_discarded_expression() {
        let module = parse("module.js", "export default 40 + 2;");

        assert!(matches!(
            module.body.as_slice(),
            [HirStatement {
                kind: HirStatementKind::ExportDefault(_),
                ..
            }]
        ));
        assert!(matches!(
            module.exports[0].origin,
            ExportOrigin::DefaultExpression
        ));
    }

    #[test]
    fn unsupported_expression_diagnostics_name_the_oxc_variant() {
        // This list shrinks as constructs get lowered -- object and `new` expressions were both
        // here and both left. That churn is the point rather than a nuisance: what the test
        // protects is that whatever *remains* unsupported names the construct, instead of
        // falling into a catch-all reporting the bare word "expression". A third of the guard
        // corpus was once hidden behind exactly that, which is why it is worth a test at all.
        // If a case here starts failing, the construct was implemented: delete the line.
        let cases = [
            ("const value = import.meta;", "import.meta expression"),
            (
                "const value = import(\"./x.js\");",
                "dynamic import expression",
            ),
        ];
        for (source, expected) in cases {
            let module = parse("module.js", source);
            let HirStatementKind::Variable { declarations, .. } = &module.body[0].kind else {
                panic!("expected variable declaration for {source:?}");
            };
            let initializer = declarations[0]
                .initializer
                .as_ref()
                .expect("fixture has an initializer");
            assert!(matches!(
                &initializer.kind,
                HirExpressionKind::Unsupported { feature } if feature == expected
            ));
        }
    }

    #[test]
    fn runtime_requests_exclude_inline_type_only_bindings_but_keep_side_effects() {
        let module = parse(
            "module.ts",
            r#"
                import { type Shape } from "./shape.ts";
                export { type Id } from "./id.ts";
                import "./side-effect.ts";
            "#,
        );

        assert!(module.diagnostics.is_empty(), "{:?}", module.diagnostics);
        assert_eq!(
            module
                .runtime_requested_modules()
                .into_iter()
                .map(|request| request.specifier.as_str())
                .collect::<Vec<_>>(),
            ["./side-effect.ts"]
        );
    }

    #[test]
    fn syntax_errors_are_owned_diagnostics_instead_of_panics() {
        let module = parse("broken.ts", "export const value: = ;");

        assert!(module.panicked || !module.diagnostics.is_empty());
        assert!(
            module.panicked
                || module
                    .diagnostics
                    .iter()
                    .any(|diagnostic| diagnostic.span.is_some()),
            "recoverable diagnostics should retain a useful source range: {:?}",
            module.diagnostics
        );
    }

    #[test]
    fn recognises_tsx_from_the_path() {
        let module = parse("component.tsx", "export const view = <div />;");
        assert_eq!(module.source_kind, SourceKind::Tsx);
        assert!(module.diagnostics.is_empty(), "{:?}", module.diagnostics);
    }

    #[test]
    fn explicit_goal_overrides_the_javascript_extension() {
        let script = parse_with_goal("case.js", "var await = 1;", SourceGoal::Script);
        let module = parse_with_goal("case.js", "var await = 1;", SourceGoal::Module);

        assert_eq!(script.source_goal, SourceGoal::Script);
        assert!(script.diagnostics.is_empty(), "{:?}", script.diagnostics);
        assert_eq!(module.source_goal, SourceGoal::Module);
        assert!(
            !module.diagnostics.is_empty(),
            "module goal must reject an await binding"
        );
    }

    #[test]
    fn lowers_the_interpreter_language_without_retaining_oxc_values() {
        let module = parse(
            "main.ts",
            r#"
                let total: number = 1;
                while (total < 4) total += 1;
                if (total === 4) console.log("total", total);
            "#,
        );

        assert!(module.diagnostics.is_empty(), "{:?}", module.diagnostics);
        assert_eq!(module.body.len(), 3);
        assert!(matches!(
            module.body[0].kind,
            HirStatementKind::Variable { .. }
        ));
        assert!(matches!(
            module.body[1].kind,
            HirStatementKind::While { .. }
        ));
        assert!(matches!(module.body[2].kind, HirStatementKind::If { .. }));
    }

    /// The parameters of the single function a module declares.
    fn parameters_of(source: &str) -> (Vec<kiln_hir::Parameter>, Option<kiln_hir::Pattern>) {
        let module = parse("parameters.ts", source);
        assert!(module.diagnostics.is_empty(), "{:?}", module.diagnostics);
        let Some(kiln_hir::StatementKind::Function(function)) =
            module.body.first().map(|statement| &statement.kind)
        else {
            panic!("expected one function declaration");
        };
        (function.parameters.clone(), function.rest.clone())
    }

    #[test]
    fn a_parameter_default_survives_lowering() {
        // oxc keeps the default on the `FormalParameter`, not inside the binding pattern, so a
        // lowering that reads only the pattern sees a bare identifier and drops the default in
        // silence -- which is how `function a(x = 5) { return x }` came to return undefined.
        let (parameters, _) = parameters_of("function a(x = 5) { return x; }");
        let kiln_hir::Pattern::WithDefault { inner, default } = &parameters[0].pattern else {
            panic!("expected a default, got {:?}", parameters[0].pattern);
        };
        assert_eq!(**inner, kiln_hir::Pattern::Identifier("x".to_owned()));
        assert!(matches!(
            default.kind,
            kiln_hir::ExpressionKind::Literal(kiln_hir::Literal::Number(_))
        ));
    }

    #[test]
    fn a_parameter_binds_whatever_pattern_was_written() {
        let (parameters, _) = parameters_of("function a(plain, { key }, [item]) {}");
        assert!(matches!(
            parameters[0].pattern,
            kiln_hir::Pattern::Identifier(_)
        ));
        assert!(matches!(
            parameters[1].pattern,
            kiln_hir::Pattern::Object { .. }
        ));
        assert!(matches!(
            parameters[2].pattern,
            kiln_hir::Pattern::Array { .. }
        ));
    }

    #[test]
    fn a_rest_parameter_is_separate_from_the_positional_ones() {
        // It takes no positional slot and its arity is the caller's, so it cannot be one of them.
        let (parameters, rest) = parameters_of("function a(first, ...others) {}");
        assert_eq!(parameters.len(), 1);
        assert_eq!(
            rest,
            Some(kiln_hir::Pattern::Identifier("others".to_owned()))
        );
        let (_, none) = parameters_of("function a(first) {}");
        assert_eq!(none, None);
    }
}
