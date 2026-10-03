//! Keep the release source DCE from deleting what the language can observe.
//!
//! Oxc's dead-code pass (`Compressor::dead_code_elimination`) decides what it may remove, inline or
//! reorder with a bundler's side-effect model, and that model makes assumptions the language does
//! not: "coercion methods are pure" (a `valueOf`, `toString` or `Symbol.toPrimitive` reached by
//! `<`, `==`, `+`, a template literal, a computed key or `new Date(x)` is taken to run no code),
//! comparisons never throw, and a handful of global functions (`decodeURIComponent`, `parseInt`,
//! `String`, `Math.*`, ...) are pure whatever their arguments are. Each of those can call user code
//! or throw, so an expression statement such as `value < 10;` was deleted, the writes its `valueOf`
//! made were then folded away, and `new Date(spy)` was inlined past a read of the variable its
//! `Symbol.toPrimitive` writes (issue 19).
//!
//! Oxc 0.147 has no option for any of this (`TreeShakeOptions` covers property reads, globals and
//! annotations only), so the pass is given an AST it cannot misjudge: before the DCE every
//! expression whose evaluation may run a coercion or a throwing built-in -- and that is not proven
//! to operate on primitives -- is wrapped in a call to [`OBSERVABLE`], an unresolvable global.
//! A call of an unknown global is an effect in every Oxc judgment, so the wrapped expression is
//! neither removed, inlined past other code, nor folded into a decided branch. After the DCE the
//! wrappers are removed again, so lowering never sees them. The wrap is conservative: an operand is
//! "primitive" only when it is syntactically one (a literal, the result of an operator that always
//! yields a primitive) or is a never-reassigned `let`/`const` initialised with one.
//!
//! Two shapes cannot be wrapped as an expression. Oxc removes an unused object literal or class by
//! keeping only the *expressions* of its computed keys and heritage, which drops the
//! `ToPropertyKey` and the constructor check they perform; so an object literal with such a key is
//! wrapped whole, and a class (whose name inference a wrapper would break) gets a marker
//! `static { kiln:observable(); }` block instead, which makes Oxc keep it whole. Both are removed
//! after the DCE like the call wrappers.
//!
//! Not covered, and recorded as such: a read of a `let`/`const`/`class` binding in its temporal
//! dead zone (Oxc ignores TDZ `ReferenceError`s), and a user-modified built-in prototype
//! (`Object.prototype.toString = ...` reached from an object *literal* is kept by the wrap, but a
//! patched global function that Oxc treats as a known pure property read is not).

use std::collections::HashSet;

use oxc_allocator::{Address, Allocator, GetAddress, TakeIn, UnstableAddress};
use oxc_ast::ast::{
    Argument, AssignmentOperator, BinaryOperator, BindingPattern, ChainElement, Class, ClassBody,
    ClassElement, Expression, IdentifierReference, ObjectPropertyKind, Program, PropertyKey,
    Statement, UnaryOperator, VariableDeclaration, VariableDeclarationKind,
};
use oxc_ast::builder::AstBuilder;
use oxc_ast_visit::{Visit, VisitMut, walk, walk_mut};
use oxc_semantic::{Scoping, SemanticBuilder, SymbolId};
use oxc_span::SPAN;

/// The callee of the guard call. Not a valid identifier, so no program can bind or read it; if a
/// wrapper ever survived to lowering it would be a loud `ReferenceError`, never a wrong value.
pub(crate) const OBSERVABLE: &str = "kiln:observable";

/// Remove TypeScript expression wrappers that have no runtime representation before handing the
/// tree to Oxc's JavaScript-only minifier. Oxc's optional-chain fold unwraps these nodes to inspect
/// the inner expression, then attempts to reinterpret the outer node as a member expression; a
/// wrapped chain member consequently panics. Owned HIR already erases these same wrappers, so this
/// normalization changes no runtime expression and does not lower enums or namespaces.
pub(crate) fn erase_expression_types_for_dce<'a>(
    allocator: &'a Allocator,
    program: &mut Program<'a>,
) {
    ExpressionTypeEraser {
        builder: AstBuilder::new(allocator),
    }
    .visit_program(program);
}

struct ExpressionTypeEraser<'a> {
    builder: AstBuilder<'a>,
}

impl<'a> VisitMut<'a> for ExpressionTypeEraser<'a> {
    fn visit_expression(&mut self, expression: &mut Expression<'a>) {
        walk_mut::walk_expression(self, expression);
        let inner = match expression {
            Expression::TSAsExpression(wrapper) => Some(wrapper.expression.take_in(&self.builder)),
            Expression::TSSatisfiesExpression(wrapper) => {
                Some(wrapper.expression.take_in(&self.builder))
            }
            Expression::TSTypeAssertion(wrapper) => Some(wrapper.expression.take_in(&self.builder)),
            Expression::TSNonNullExpression(wrapper) => {
                Some(wrapper.expression.take_in(&self.builder))
            }
            Expression::TSInstantiationExpression(wrapper) => {
                Some(wrapper.expression.take_in(&self.builder))
            }
            _ => None,
        };
        if let Some(inner) = inner {
            *expression = inner;
        }
    }
}

/// Wrap every expression in `program` that Oxc could wrongly judge free of effects.
pub(crate) fn guard_observable_effects<'a>(allocator: &'a Allocator, program: &mut Program<'a>) {
    FunctionSemanticsGuard {
        builder: AstBuilder::new(allocator),
    }
    .visit_program(program);
    let scoping = SemanticBuilder::new()
        .build(program)
        .semantic
        .into_scoping();
    let mut primitives = PrimitiveBindings {
        scoping: &scoping,
        symbols: HashSet::new(),
        changed: false,
    };
    // A binding initialised from another primitive binding is primitive too; a few rounds reach
    // every chain that occurs in practice, and stopping early only makes the wrap wider.
    for _ in 0..4 {
        primitives.changed = false;
        primitives.visit_program(program);
        if !primitives.changed {
            break;
        }
    }
    let mut collect = Collect {
        facts: Facts {
            scoping: &scoping,
            primitive_symbols: &primitives.symbols,
        },
        targets: HashSet::new(),
        classes: HashSet::new(),
    };
    collect.visit_program(program);
    if collect.targets.is_empty() && collect.classes.is_empty() {
        return;
    }
    Wrap {
        builder: AstBuilder::new(allocator),
        targets: collect.targets,
        classes: collect.classes,
    }
    .visit_program(program);
}

/// Remove every wrapper [`guard_observable_effects`] added (and the DCE kept).
pub(crate) fn release_observable_guards<'a>(allocator: &'a Allocator, program: &mut Program<'a>) {
    Unwrap { allocator }.visit_program(program);
}

// A sequence expression is not a named-evaluation position. DCE may remove its leading
// pure operands and expose an anonymous function to object/binding name inference. Keep an
// opaque marker around that function, then restore a harmless sequence after compression.
const ANONYMOUS_SEQUENCE: &str = "kiln:anonymous-sequence";

struct FunctionSemanticsGuard<'a> {
    builder: AstBuilder<'a>,
}
impl<'a> VisitMut<'a> for FunctionSemanticsGuard<'a> {
    fn visit_expression(&mut self, it: &mut Expression<'a>) {
        walk_mut::walk_expression(self, it);
        if let Expression::BinaryExpression(binary) = it
            && binary.operator == BinaryOperator::Instanceof
        {
            let inner = binary.right.take_in(&self.builder);
            binary.right = Expression::new_call_expression(
                SPAN,
                Expression::new_identifier(SPAN, OBSERVABLE, &self.builder),
                None::<oxc_allocator::Box<'a, _>>,
                oxc_allocator::Vec::from_array_in([Argument::from(inner)], &self.builder),
                false,
                &self.builder,
            );
        }
        let Expression::SequenceExpression(sequence) = it else {
            return;
        };
        let Some(last) = sequence.expressions.last_mut() else {
            return;
        };
        let anonymous = match last.without_parentheses() {
            Expression::FunctionExpression(function) => function.id.is_none(),
            Expression::ArrowFunctionExpression(_) => true,
            Expression::ClassExpression(class) => class.id.is_none(),
            _ => false,
        };
        if anonymous {
            let inner = last.take_in(&self.builder);
            *last = Expression::new_call_expression(
                SPAN,
                Expression::new_identifier(SPAN, ANONYMOUS_SEQUENCE, &self.builder),
                None::<oxc_allocator::Box<'a, _>>,
                oxc_allocator::Vec::from_array_in([Argument::from(inner)], &self.builder),
                false,
                &self.builder,
            );
        }
    }
}

struct Facts<'s> {
    scoping: &'s Scoping,
    primitive_symbols: &'s HashSet<SymbolId>,
}

impl Facts<'_> {
    fn resolve(&self, identifier: &IdentifierReference<'_>) -> Option<SymbolId> {
        let reference = identifier.reference_id.get()?;
        self.scoping.get_reference(reference).symbol_id()
    }

    fn is_unresolved(&self, identifier: &IdentifierReference<'_>) -> bool {
        self.resolve(identifier).is_none()
    }

    /// Whether evaluating `expression` certainly yields a primitive, so that a coercion of it runs
    /// no user code. Says nothing about the effects of evaluating it.
    fn is_primitive(&self, expression: &Expression<'_>) -> bool {
        match expression {
            Expression::NumericLiteral(_)
            | Expression::StringLiteral(_)
            | Expression::BooleanLiteral(_)
            | Expression::NullLiteral(_)
            | Expression::BigIntLiteral(_)
            | Expression::TemplateLiteral(_)
            | Expression::UnaryExpression(_)
            | Expression::BinaryExpression(_)
            | Expression::UpdateExpression(_)
            | Expression::PrivateInExpression(_) => true,
            Expression::LogicalExpression(logical) => {
                self.is_primitive(&logical.left) && self.is_primitive(&logical.right)
            }
            Expression::ConditionalExpression(conditional) => {
                self.is_primitive(&conditional.consequent)
                    && self.is_primitive(&conditional.alternate)
            }
            Expression::SequenceExpression(sequence) => sequence
                .expressions
                .last()
                .is_some_and(|last| self.is_primitive(last)),
            Expression::ParenthesizedExpression(inner) => self.is_primitive(&inner.expression),
            Expression::TSAsExpression(inner) => self.is_primitive(&inner.expression),
            Expression::TSSatisfiesExpression(inner) => self.is_primitive(&inner.expression),
            Expression::TSTypeAssertion(inner) => self.is_primitive(&inner.expression),
            Expression::TSNonNullExpression(inner) => self.is_primitive(&inner.expression),
            Expression::AssignmentExpression(assignment) => match assignment.operator {
                AssignmentOperator::Assign => self.is_primitive(&assignment.right),
                AssignmentOperator::LogicalAnd
                | AssignmentOperator::LogicalOr
                | AssignmentOperator::LogicalNullish => false,
                _ => true,
            },
            Expression::Identifier(identifier) => match self.resolve(identifier) {
                None => matches!(identifier.name.as_str(), "undefined" | "NaN" | "Infinity"),
                Some(symbol) => self.primitive_symbols.contains(&symbol),
            },
            _ => false,
        }
    }

    fn is_nullish_literal(&self, expression: &Expression<'_>) -> bool {
        match expression.without_parentheses() {
            Expression::NullLiteral(_) => true,
            Expression::Identifier(identifier) => {
                identifier.name == "undefined" && self.is_unresolved(identifier)
            }
            Expression::UnaryExpression(unary) => {
                unary.operator == UnaryOperator::Void && self.is_primitive(&unary.argument)
            }
            _ => false,
        }
    }

    /// `Symbol.iterator` and its siblings: a symbol, so `ToPropertyKey` runs no code.
    fn is_well_known_symbol(&self, expression: &Expression<'_>) -> bool {
        let Expression::StaticMemberExpression(member) = expression.without_parentheses() else {
            return false;
        };
        matches!(&member.object, Expression::Identifier(object)
            if object.name == "Symbol" && self.is_unresolved(object))
    }

    /// A callee Oxc may recognise as a pure global: a global identifier, or a property of one.
    fn callee_may_be_known_global(&self, callee: &Expression<'_>) -> bool {
        match callee.without_parentheses() {
            Expression::Identifier(identifier) => self.is_unresolved(identifier),
            Expression::StaticMemberExpression(member) => {
                matches!(&member.object, Expression::Identifier(object) if self.is_unresolved(object))
            }
            Expression::ComputedMemberExpression(member) => {
                matches!(&member.object, Expression::Identifier(object) if self.is_unresolved(object))
            }
            _ => false,
        }
    }

    /// Construction and calls of globals that run no user code and cannot throw with exactly
    /// these arguments. Kept small: these are the allocations bundles declare at module scope
    /// and never use, which is what the source DCE exists to remove.
    fn is_proven_pure_global_call(
        &self,
        callee: &Expression<'_>,
        arguments: &[Argument<'_>],
        construct: bool,
    ) -> bool {
        let string_literal_or_nothing = || match arguments {
            [] => true,
            [Argument::StringLiteral(_)] => true,
            _ => false,
        };
        match callee.without_parentheses() {
            Expression::Identifier(identifier) if self.is_unresolved(identifier) => {
                let name = identifier.name.as_str();
                if construct {
                    arguments.is_empty()
                        && matches!(
                            name,
                            "Map" | "Set" | "WeakMap" | "WeakSet" | "Object" | "Array" | "Error"
                        )
                } else {
                    name == "Symbol" && string_literal_or_nothing()
                }
            }
            Expression::StaticMemberExpression(member) if !construct => {
                let Expression::Identifier(object) = &member.object else {
                    return false;
                };
                if !self.is_unresolved(object) {
                    return false;
                }
                match (object.name.as_str(), member.property.name.as_str()) {
                    ("Symbol", "for") => {
                        matches!(arguments, [Argument::StringLiteral(_)])
                    }
                    ("Object", "create") => matches!(arguments, [Argument::NullLiteral(_)]),
                    _ => false,
                }
            }
            _ => false,
        }
    }

    /// A class heritage that is certainly a constructor, so evaluating the class cannot throw.
    fn is_safe_heritage(&self, expression: &Expression<'_>) -> bool {
        match expression.without_parentheses() {
            Expression::NullLiteral(_) => true,
            Expression::Identifier(identifier) => match self.resolve(identifier) {
                Some(symbol) => {
                    self.scoping.symbol_flags(symbol).is_class()
                        && !self.scoping.symbol_is_mutated(symbol)
                }
                None => matches!(
                    identifier.name.as_str(),
                    "Object"
                        | "Array"
                        | "Map"
                        | "Set"
                        | "WeakMap"
                        | "WeakSet"
                        | "Promise"
                        | "Error"
                        | "TypeError"
                        | "RangeError"
                        | "SyntaxError"
                        | "ReferenceError"
                ),
            },
            // Anything else Oxc already treats as an effect (a read, a call) or as a function
            // expression, which is a constructor unless it is an arrow -- and Oxc keeps those.
            _ => true,
        }
    }

    /// Whether Oxc might judge `expression` itself free of effects when it is not.
    fn needs_guard(&self, expression: &Expression<'_>) -> bool {
        match expression {
            Expression::BinaryExpression(binary) => {
                let coerces_both =
                    || !self.is_primitive(&binary.left) || !self.is_primitive(&binary.right);
                match binary.operator {
                    BinaryOperator::StrictEquality
                    | BinaryOperator::StrictInequality
                    | BinaryOperator::In
                    | BinaryOperator::Instanceof => false,
                    // `x == null` compares without converting either side.
                    BinaryOperator::Equality | BinaryOperator::Inequality => {
                        !self.is_nullish_literal(&binary.left)
                            && !self.is_nullish_literal(&binary.right)
                            && coerces_both()
                    }
                    _ => coerces_both(),
                }
            }
            Expression::UnaryExpression(unary) => {
                matches!(
                    unary.operator,
                    UnaryOperator::UnaryPlus
                        | UnaryOperator::UnaryNegation
                        | UnaryOperator::BitwiseNot
                ) && !self.is_primitive(&unary.argument)
            }
            Expression::TemplateLiteral(template) => template
                .expressions
                .iter()
                .any(|expression| !self.is_primitive(expression)),
            Expression::CallExpression(call) => {
                !call.pure
                    && self.callee_may_be_known_global(&call.callee)
                    && !self.is_proven_pure_global_call(&call.callee, &call.arguments, false)
            }
            Expression::NewExpression(new) => {
                !new.pure
                    && self.callee_may_be_known_global(&new.callee)
                    && !self.is_proven_pure_global_call(&new.callee, &new.arguments, true)
            }
            // An unused object literal is reduced to its computed keys' *expressions*, which
            // drops the `ToPropertyKey` each one performs; guard the literal as a whole.
            Expression::ObjectExpression(object) => object.properties.iter().any(|property| {
                matches!(property, ObjectPropertyKind::ObjectProperty(property)
                    if property.computed && self.key_needs_guard(&property.key))
            }),
            // `decodeURIComponent?.("%")`: Oxc reads an optional call of a global the same way.
            Expression::ChainExpression(chain) => match &chain.expression {
                ChainElement::CallExpression(call) => {
                    !call.pure && self.callee_may_be_known_global(&call.callee)
                }
                _ => false,
            },
            _ => false,
        }
    }

    /// A computed key whose `ToPropertyKey` may call user code.
    fn key_needs_guard(&self, key: &PropertyKey<'_>) -> bool {
        key.as_expression().is_some_and(|expression| {
            !self.is_primitive(expression) && !self.is_well_known_symbol(expression)
        })
    }

    /// A class whose evaluation may run user code or throw where Oxc cannot see it: a heritage
    /// that may not be a constructor, or a computed key that may convert an object. Oxc removes
    /// an unused class by keeping only the *expressions* of its heritage and keys, which drops
    /// the constructor check and each `ToPropertyKey`, so such a class must stay whole.
    fn class_needs_guard(&self, class: &Class<'_>) -> bool {
        class
            .heritage_expression()
            .is_some_and(|heritage| !self.is_safe_heritage(heritage))
            || class.body.body.iter().any(|element| match element {
                ClassElement::MethodDefinition(method) => {
                    method.computed && self.key_needs_guard(&method.key)
                }
                ClassElement::PropertyDefinition(property) => {
                    property.computed && self.key_needs_guard(&property.key)
                }
                ClassElement::AccessorProperty(accessor) => {
                    accessor.computed && self.key_needs_guard(&accessor.key)
                }
                _ => false,
            })
    }
}

/// `let`/`const` bindings that are never reassigned and are initialised with a primitive.
struct PrimitiveBindings<'s> {
    scoping: &'s Scoping,
    symbols: HashSet<SymbolId>,
    changed: bool,
}

impl<'a> Visit<'a> for PrimitiveBindings<'_> {
    fn visit_variable_declaration(&mut self, it: &VariableDeclaration<'a>) {
        if matches!(
            it.kind,
            VariableDeclarationKind::Const | VariableDeclarationKind::Let
        ) {
            for declarator in &it.declarations {
                if let BindingPattern::BindingIdentifier(binding) = &declarator.id
                    && let Some(symbol) = binding.symbol_id.get()
                    && let Some(init) = &declarator.init
                    && !self.symbols.contains(&symbol)
                    && !self.scoping.symbol_is_mutated(symbol)
                {
                    let facts = Facts {
                        scoping: self.scoping,
                        primitive_symbols: &self.symbols,
                    };
                    if facts.is_primitive(init) {
                        self.symbols.insert(symbol);
                        self.changed = true;
                    }
                }
            }
        }
        walk::walk_variable_declaration(self, it);
    }
}

struct Collect<'s> {
    facts: Facts<'s>,
    targets: HashSet<Address>,
    classes: HashSet<Address>,
}

impl<'a> Visit<'a> for Collect<'_> {
    fn visit_expression(&mut self, it: &Expression<'a>) {
        if self.facts.needs_guard(it) {
            self.targets.insert(it.address());
        }
        walk::walk_expression(self, it);
    }

    fn visit_class(&mut self, it: &Class<'a>) {
        if self.facts.class_needs_guard(it) {
            self.classes.insert(it.unstable_address());
        }
        walk::walk_class(self, it);
    }
}

struct Wrap<'a> {
    builder: AstBuilder<'a>,
    targets: HashSet<Address>,
    classes: HashSet<Address>,
}

impl<'a> Wrap<'a> {
    fn guard_call(
        &self,
        span: oxc_span::Span,
        arguments: oxc_allocator::Vec<'a, Argument<'a>>,
    ) -> Expression<'a> {
        let callee = Expression::new_identifier(SPAN, OBSERVABLE, &self.builder);
        Expression::new_call_expression(
            span,
            callee,
            None::<oxc_allocator::Box<'a, _>>,
            arguments,
            false,
            &self.builder,
        )
    }
}

impl<'a> VisitMut<'a> for Wrap<'a> {
    fn visit_class(&mut self, it: &mut Class<'a>) {
        let guarded = self.classes.contains(&it.unstable_address());
        walk_mut::walk_class(self, it);
        if guarded {
            // `static { kiln:observable(); }`: Oxc keeps a class with a non-empty static block
            // whole, and the block is removed again after the DCE.
            let call = self.guard_call(SPAN, oxc_allocator::Vec::new_in(&self.builder));
            let statement = Statement::new_expression_statement(SPAN, call, &self.builder);
            let body = oxc_allocator::Vec::from_array_in([statement], &self.builder);
            it.body
                .body
                .push(ClassElement::new_static_block(SPAN, body, &self.builder));
        }
    }

    fn visit_expression(&mut self, it: &mut Expression<'a>) {
        let guarded = self.targets.contains(&it.address());
        walk_mut::walk_expression(self, it);
        if guarded {
            let span = oxc_span::GetSpan::span(it);
            let inner = it.take_in(&self.builder);
            let arguments =
                oxc_allocator::Vec::from_array_in([Argument::from(inner)], &self.builder);
            *it = self.guard_call(span, arguments);
        }
    }
}

struct Unwrap<'a> {
    allocator: &'a Allocator,
}

fn is_guard(expression: &Expression<'_>) -> bool {
    matches!(expression, Expression::CallExpression(call)
        if call.arguments.len() == 1
            && call.arguments[0].as_expression().is_some()
            && matches!(&call.callee, Expression::Identifier(callee) if callee.name == OBSERVABLE))
}

fn is_class_guard(element: &ClassElement<'_>) -> bool {
    let ClassElement::StaticBlock(block) = element else {
        return false;
    };
    matches!(block.body.as_slice(), [Statement::ExpressionStatement(statement)]
        if matches!(&statement.expression, Expression::CallExpression(call)
            if call.arguments.is_empty()
                && matches!(&call.callee, Expression::Identifier(callee) if callee.name == OBSERVABLE)))
}

impl<'a> VisitMut<'a> for Unwrap<'a> {
    fn visit_class_body(&mut self, it: &mut ClassBody<'a>) {
        it.body.retain(|element| !is_class_guard(element));
        walk_mut::walk_class_body(self, it);
    }

    fn visit_expression(&mut self, it: &mut Expression<'a>) {
        while is_guard(it) {
            let Expression::CallExpression(call) = it else {
                unreachable!()
            };
            let inner = call.arguments[0]
                .as_expression_mut()
                .expect("a guard wraps an expression")
                .take_in(&self.allocator);
            *it = inner;
        }
        if matches!(it, Expression::CallExpression(call)
            if call.arguments.len() == 1 && matches!(&call.callee, Expression::Identifier(callee) if callee.name == ANONYMOUS_SEQUENCE))
        {
            let Expression::CallExpression(call) = it else {
                unreachable!()
            };
            let inner = call.arguments[0]
                .as_expression_mut()
                .expect("anonymous marker argument")
                .take_in(&self.allocator);
            let zero = Expression::new_numeric_literal(
                SPAN,
                0.0,
                None,
                oxc_syntax::number::NumberBase::Decimal,
                &AstBuilder::new(self.allocator),
            );
            *it = Expression::new_sequence_expression(
                SPAN,
                oxc_allocator::Vec::from_array_in([zero, inner], &self.allocator),
                &AstBuilder::new(self.allocator),
            );
        }
        walk_mut::walk_expression(self, it);
    }
}
