//! Oxc adapter for Kiln's owned HIR.
//!
//! This is the only crate allowed to expose Oxc to the compiler build graph. Its public API
//! accepts ordinary strings and returns [`kiln_hir`] values, so an Oxc upgrade cannot leak AST
//! types or arena lifetimes into the query database, interpreter, or backends.

use std::path::Path;

use kiln_hir::{
    AssignmentOperator as HirAssignmentOperator, BinaryOperator as HirBinaryOperator, Diagnostic,
    Export, Expression as HirExpression, ExpressionKind as HirExpressionKind, Item, ItemKind,
    Literal as HirLiteral, LogicalOperator as HirLogicalOperator, Module, RequestedModule,
    SourceGoal, SourceKind, Span as HirSpan, Statement as HirStatement,
    StatementKind as HirStatementKind, UnaryOperator as HirUnaryOperator,
    VariableDeclarator as HirVariableDeclarator, VariableKind as HirVariableKind,
};
use oxc_allocator::Allocator;
use oxc_ast::ast::{
    AssignmentTarget, BindingPattern, Declaration, ExportDefaultDeclarationKind, Expression,
    Statement, VariableDeclaration, VariableDeclarationKind,
};
use oxc_parser::Parser;
use oxc_span::{GetSpan, SourceType, Span};
use oxc_syntax::module_record::ExportExportName;
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
                .map(|occurrence| span(occurrence.statement_span))
                .collect(),
        })
        .collect::<Vec<_>>();
    requested_modules.sort_by_key(|request| {
        request
            .occurrences
            .first()
            .copied()
            .unwrap_or_default()
            .start
    });

    // Use the spec-shaped entries rather than `exported_bindings`: the latter intentionally
    // excludes `default` and erases TypeScript's type-only bit, both of which affect Kiln's
    // runtime interface.
    let mut exports = parsed
        .module_record
        .local_export_entries
        .iter()
        .chain(parsed.module_record.indirect_export_entries.iter())
        .map(|entry| Export {
            name: match &entry.export_name {
                ExportExportName::Name(name) => Some(name.name.to_string()),
                ExportExportName::Default(_) => Some("default".to_string()),
                ExportExportName::Null => None,
            },
            source: entry
                .module_request
                .as_ref()
                .map(|request| request.name.to_string()),
            type_only: entry.is_type,
            span: span(entry.span),
        })
        .collect::<Vec<_>>();

    for entry in &parsed.module_record.star_export_entries {
        exports.push(Export {
            name: None,
            source: entry
                .module_request
                .as_ref()
                .map(|request| request.name.to_string()),
            type_only: entry.is_type,
            span: span(entry.span),
        });
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
        exports,
        diagnostics,
        panicked: parsed.panicked,
    }
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
                    HirStatementKind::Expression(self.expression(expression))
                } else if matches!(
                    declaration.declaration,
                    ExportDefaultDeclarationKind::TSInterfaceDeclaration(_)
                ) {
                    return None;
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

            Statement::FunctionDeclaration(_) => {
                self.unsupported_statement("function declaration", statement_span)
            }
            Statement::ClassDeclaration(_) => {
                self.unsupported_statement("class declaration", statement_span)
            }
            Statement::DoWhileStatement(_) => {
                self.unsupported_statement("do-while statement", statement_span)
            }
            Statement::ForStatement(_)
            | Statement::ForInStatement(_)
            | Statement::ForOfStatement(_) => {
                self.unsupported_statement("for statement", statement_span)
            }
            Statement::BreakStatement(_) | Statement::ContinueStatement(_) => {
                self.unsupported_statement("break or continue statement", statement_span)
            }
            Statement::ReturnStatement(_) => {
                self.unsupported_statement("return statement", statement_span)
            }
            Statement::SwitchStatement(_) => {
                self.unsupported_statement("switch statement", statement_span)
            }
            Statement::ThrowStatement(_) => {
                self.unsupported_statement("throw statement", statement_span)
            }
            Statement::TryStatement(_) => {
                self.unsupported_statement("try statement", statement_span)
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
            Declaration::FunctionDeclaration(_) => {
                self.unsupported_statement("exported function declaration", statement_span)
            }
            Declaration::ClassDeclaration(_) => {
                self.unsupported_statement("exported class declaration", statement_span)
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
                let name = match &declarator.id {
                    BindingPattern::BindingIdentifier(identifier) => identifier.name.to_string(),
                    _ => {
                        self.unsupported("destructuring binding", declarator_span);
                        String::new()
                    }
                };
                HirVariableDeclarator {
                    name,
                    initializer: declarator
                        .init
                        .as_ref()
                        .map(|expression| self.expression(expression)),
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
                let AssignmentTarget::AssignmentTargetIdentifier(identifier) = &expression.left
                else {
                    return self
                        .unsupported_expression("non-identifier assignment", expression_span);
                };
                HirExpressionKind::Assignment {
                    operator,
                    target: identifier.name.to_string(),
                    value: Box::new(self.expression(&expression.right)),
                }
            }
            Expression::ConditionalExpression(expression) => HirExpressionKind::Conditional {
                test: Box::new(self.expression(&expression.test)),
                consequent: Box::new(self.expression(&expression.consequent)),
                alternate: Box::new(self.expression(&expression.alternate)),
            },
            Expression::CallExpression(expression) if !expression.optional => {
                let mut arguments = Vec::with_capacity(expression.arguments.len());
                for argument in &expression.arguments {
                    let Some(argument) = argument.as_expression() else {
                        return self
                            .unsupported_expression("spread call argument", expression_span);
                    };
                    arguments.push(self.expression(argument));
                }
                HirExpressionKind::Call {
                    callee: Box::new(self.expression(&expression.callee)),
                    arguments,
                }
            }
            Expression::StaticMemberExpression(member) if !member.optional => {
                HirExpressionKind::StaticMember {
                    object: Box::new(self.expression(&member.object)),
                    property: member.property.name.to_string(),
                }
            }
            Expression::ComputedMemberExpression(_) => {
                return self.unsupported_expression("computed property access", expression_span);
            }
            Expression::PrivateFieldExpression(_) => {
                return self.unsupported_expression("private property access", expression_span);
            }
            Expression::CallExpression(_) | Expression::StaticMemberExpression(_) => {
                return self.unsupported_expression("optional chaining", expression_span);
            }
            _ => return self.unsupported_expression("expression", expression_span),
        };
        HirExpression {
            kind,
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

fn lower_unary_operator(operator: UnaryOperator) -> Option<HirUnaryOperator> {
    match operator {
        UnaryOperator::UnaryPlus => Some(HirUnaryOperator::Plus),
        UnaryOperator::UnaryNegation => Some(HirUnaryOperator::Negate),
        UnaryOperator::LogicalNot => Some(HirUnaryOperator::Not),
        _ => None,
    }
}

fn lower_binary_operator(operator: BinaryOperator) -> Option<HirBinaryOperator> {
    match operator {
        BinaryOperator::Addition => Some(HirBinaryOperator::Add),
        BinaryOperator::Subtraction => Some(HirBinaryOperator::Subtract),
        BinaryOperator::Multiplication => Some(HirBinaryOperator::Multiply),
        BinaryOperator::Division => Some(HirBinaryOperator::Divide),
        BinaryOperator::Remainder => Some(HirBinaryOperator::Remainder),
        BinaryOperator::Exponential => Some(HirBinaryOperator::Exponentiate),
        BinaryOperator::StrictEquality => Some(HirBinaryOperator::StrictEqual),
        BinaryOperator::StrictInequality => Some(HirBinaryOperator::StrictNotEqual),
        BinaryOperator::LessThan => Some(HirBinaryOperator::LessThan),
        BinaryOperator::LessEqualThan => Some(HirBinaryOperator::LessThanOrEqual),
        BinaryOperator::GreaterThan => Some(HirBinaryOperator::GreaterThan),
        BinaryOperator::GreaterEqualThan => Some(HirBinaryOperator::GreaterThanOrEqual),
        _ => None,
    }
}

fn lower_assignment_operator(operator: AssignmentOperator) -> Option<HirAssignmentOperator> {
    match operator {
        AssignmentOperator::Assign => Some(HirAssignmentOperator::Assign),
        AssignmentOperator::Addition => Some(HirAssignmentOperator::Add),
        AssignmentOperator::Subtraction => Some(HirAssignmentOperator::Subtract),
        AssignmentOperator::Multiplication => Some(HirAssignmentOperator::Multiply),
        AssignmentOperator::Division => Some(HirAssignmentOperator::Divide),
        AssignmentOperator::Remainder => Some(HirAssignmentOperator::Remainder),
        AssignmentOperator::Exponential => Some(HirAssignmentOperator::Exponentiate),
        _ => None,
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
        assert!(
            module.exports.iter().any(|export| {
                export.name.is_none() && export.source.as_deref() == Some("./b.js")
            })
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
}
