use super::*;
pub(super) fn resource_declarations(list: &[Statement]) -> Vec<bool> {
    let mut resources = Vec::new();
    for statement in list {
        if let StatementKind::Variable { kind, declarations } = &statement.kind
            && let Some(awaiting) = kind.disposal()
        {
            resources.extend(std::iter::repeat_n(awaiting, declarations.len()));
        }
    }
    resources
}

pub(super) fn removable_class_structure(class: &Class) -> bool {
    class
        .methods
        .iter()
        .all(|method| matches!(method.key, ObjectKey::Static(_) | ObjectKey::Private(_)))
        && class
            .initialisers
            .iter()
            .all(|initialiser| match initialiser {
                ClassInitialiser::Field(field) => {
                    !field.is_static
                        && matches!(field.key, ObjectKey::Static(_) | ObjectKey::Private(_))
                }
                ClassInitialiser::StaticBlock(_) => false,
            })
}

pub(super) fn removable_class_evaluation(class: &Class) -> bool {
    // Evaluating a superclass reads a binding and validates its current value. Even a name
    // declared as a class elsewhere may be in its TDZ or reassigned before this statement.
    // Keep all heritage evaluation until we can prove initialization and value stability.
    class.superclass.is_none() && removable_class_structure(class)
}

pub(super) fn is_removable_expression(expr: &Expression) -> bool {
    match &expr.kind {
        ExpressionKind::Literal(_) => true,
        ExpressionKind::Function(_) => true,
        ExpressionKind::Class(class) => removable_class_evaluation(class),
        ExpressionKind::Unary { operator, argument } => {
            matches!(
                operator,
                UnaryOperator::Plus
                    | UnaryOperator::Negate
                    | UnaryOperator::Not
                    | UnaryOperator::Void
            ) && is_removable_expression(argument)
        }
        ExpressionKind::Array { elements } => elements.iter().all(|elem| match elem {
            ListElement::Item(e) => is_removable_expression(e),
            ListElement::Hole => true,
            ListElement::Spread(_) => false,
        }),
        ExpressionKind::Object { members } => members.iter().all(|member| match member {
            ObjectMember::Property(prop) => match &prop.key {
                ObjectKey::Static(s) => s != "__proto__" && is_removable_expression(&prop.value),
                _ => false,
            },
            _ => false,
        }),
        _ => false,
    }
}

pub(super) fn removable_declaration_names(statement: &Statement) -> Option<Vec<String>> {
    match &statement.kind {
        StatementKind::Class(class) if removable_class_evaluation(class) => {
            class.name.clone().map(|name| vec![name])
        }
        StatementKind::Variable { kind, declarations }
            if kind.disposal().is_none() && !declarations.is_empty() =>
        {
            let mut names = Vec::with_capacity(declarations.len());
            for declaration in declarations {
                let name = declaration.pattern.as_identifier()?.to_owned();
                let removable = match &declaration.initializer {
                    None => true,
                    Some(expr) => is_removable_expression(expr),
                };
                if !removable {
                    return None;
                }
                names.push(name);
            }
            Some(names)
        }
        _ => None,
    }
}

pub fn mutable_module_bindings(body: &[Statement]) -> HashSet<String> {
    let mut names = HashSet::new();
    for statement in body {
        if let StatementKind::Variable { kind, declarations } = &statement.kind
            && *kind != VariableKind::Const
        {
            for declaration in declarations {
                collect_pattern_names(&declaration.pattern, &mut names);
            }
        }
        collect_assigned_names(statement, &mut names);
    }
    names
}

/// Identify exported and top-level module bindings that are never mutated.
pub fn immutable_module_bindings(module: &Module) -> Vec<String> {
    let mutable = mutable_module_bindings(&module.body);
    let mut names = HashSet::new();
    for statement in &module.body {
        match &statement.kind {
            StatementKind::Variable { declarations, .. } => {
                for declaration in declarations {
                    collect_pattern_names(&declaration.pattern, &mut names);
                }
            }
            StatementKind::Function(function) => {
                if let Some(name) = &function.name {
                    names.insert(name.clone());
                }
            }
            StatementKind::Class(class) => {
                if let Some(name) = &class.name {
                    names.insert(name.clone());
                }
            }
            _ => {}
        }
    }
    for import in &module.imports {
        names.insert(import.local.clone());
    }
    for export in &module.exports {
        if let ExportOrigin::Local { local } = &export.origin {
            names.insert(local.clone());
        }
    }
    let mut immutable: Vec<String> = names
        .into_iter()
        .filter(|name| !mutable.contains(name))
        .collect();
    immutable.sort();
    immutable
}

/// Every name assigned or updated anywhere in `statement`, however deeply nested.
///
/// Deliberately an over-approximation, like the capture analysis: a name boxed that never changes
/// costs one pointer chase, where a name *missed* would read as a stale value through an import.
pub(super) fn collect_assigned_names(statement: &Statement, names: &mut HashSet<String>) {
    let mut expressions: Vec<&Expression> = Vec::new();
    let mut statements: Vec<&Statement> = vec![statement];
    while let Some(statement) = statements.pop() {
        match &statement.kind {
            StatementKind::Variable { declarations, .. } => {
                for declaration in declarations {
                    if let Some(initializer) = &declaration.initializer {
                        expressions.push(initializer);
                    }
                }
            }
            StatementKind::Function(function) => statements.extend(function.body.iter()),
            StatementKind::Class(class) => {
                for method in &class.methods {
                    statements.extend(method.function.body.iter());
                }
                if let Some(constructor) = &class.constructor {
                    statements.extend(constructor.body.iter());
                }
            }
            StatementKind::Expression(expression)
            | StatementKind::Throw(expression)
            | StatementKind::ExportDefault(expression) => expressions.push(expression),
            StatementKind::Return(value) => expressions.extend(value.iter()),
            StatementKind::Block(body) => statements.extend(body.iter()),
            StatementKind::If {
                test,
                consequent,
                alternate,
            } => {
                expressions.push(test);
                statements.push(consequent);
                statements.extend(alternate.iter().map(std::convert::AsRef::as_ref));
            }
            StatementKind::While { test, body } | StatementKind::DoWhile { test, body } => {
                expressions.push(test);
                statements.push(body);
            }
            StatementKind::For {
                init,
                test,
                update,
                body,
            } => {
                statements.extend(init.iter().map(std::convert::AsRef::as_ref));
                expressions.extend(test.iter());
                expressions.extend(update.iter());
                statements.push(body);
            }
            StatementKind::ForOf { iterable, body, .. } => {
                expressions.push(iterable);
                statements.push(body);
            }
            StatementKind::ForIn { object, body, .. } => {
                expressions.push(object);
                statements.push(body);
            }
            StatementKind::Labelled { body, .. } => statements.push(body),
            StatementKind::With { object, body } => {
                expressions.push(object);
                statements.push(body);
            }
            StatementKind::Switch {
                discriminant,
                cases,
            } => {
                expressions.push(discriminant);
                for case in cases {
                    expressions.extend(case.test.iter());
                    statements.extend(case.body.iter());
                }
            }
            StatementKind::Try {
                block,
                handler,
                finalizer,
            } => {
                statements.extend(block.iter());
                if let Some(handler) = handler {
                    statements.extend(handler.body.iter());
                }
                statements.extend(finalizer.iter());
            }
            StatementKind::Break { .. }
            | StatementKind::Continue { .. }
            | StatementKind::Empty
            | StatementKind::Unsupported { .. } => {}
        }
    }
    for expression in expressions {
        collect_identifiers(expression, names, true);
    }
}

/// Names that nested functions inside `body` refer to.
///
/// Deliberately an over-approximation: it collects every identifier appearing anywhere inside a
/// nested function, without modelling that function's own shadowing. A local that is boxed but
/// never actually captured still behaves correctly -- one pointer chase, no semantic difference
/// -- whereas a captured local that was *missed* would be read out of a dead frame. The analysis
/// is allowed to be wrong only in the direction that costs performance.
pub(super) fn names_captured_by_nested_functions(body: &[Statement]) -> HashSet<String> {
    let mut captured = HashSet::new();
    for statement in body {
        collect_nested_function_names(statement, &mut captured, false);
    }
    captured
}

/// As [`names_captured_by_nested_functions`], but for a whole function rather than a body.
///
/// A parameter default is compiled in the function's own scope and may contain a nested function
/// of its own -- `function f(a, cb = () => a)` captures `a`, and nothing in the body says so. The
/// body-only scan leaves `a` unboxed, and the arrow then cannot reach it at all.
pub(super) fn names_captured_by_this_function(function: &Function) -> HashSet<String> {
    let mut captured = names_captured_by_nested_functions(&function.body);
    for parameter in &function.parameters {
        // A parameter default runs in this function's own scope. Ordinary identifiers in it may
        // therefore refer outward and must be captured by this function itself, not only when a
        // second nested function appears inside the pattern. `path-to-regexp` relies on exactly
        // this shape: `function match(path, { delimiter = DEFAULT_DELIMITER } = {})`.
        collect_pattern_identifiers(&parameter.pattern, &mut captured);
    }
    if let Some(rest) = &function.rest {
        collect_pattern_identifiers(rest, &mut captured);
    }
    captured
}

/// Whether `body` contains a syntactically direct `eval(...)` call.
///
/// This deliberately looks through nested functions. An eval in a nested function can still name
/// a binding owned by any enclosing function, so every enclosing level has to box its bindings as
/// well. Shadowing is left to normal resolution when the call itself is compiled; treating a
/// shadowed `eval` as direct here merely boxes a few extra locals and cannot change their values.
pub fn body_contains_direct_eval(body: &[Statement]) -> bool {
    body.iter().any(statement_contains_direct_eval)
}

pub fn function_contains_direct_eval(function: &Function) -> bool {
    function
        .parameters
        .iter()
        .any(|parameter| pattern_contains_direct_eval(&parameter.pattern))
        || function
            .rest
            .as_ref()
            .is_some_and(pattern_contains_direct_eval)
        || body_contains_direct_eval(&function.body)
}

pub fn module_contains_direct_eval(module: &Module) -> bool {
    body_contains_direct_eval(&module.body)
}

pub(super) fn statement_contains_direct_eval(statement: &Statement) -> bool {
    match &statement.kind {
        StatementKind::Variable { declarations, .. } => declarations.iter().any(|declaration| {
            pattern_contains_direct_eval(&declaration.pattern)
                || declaration
                    .initializer
                    .as_ref()
                    .is_some_and(expression_contains_direct_eval)
        }),
        StatementKind::Function(function) => function_contains_direct_eval(function),
        StatementKind::Class(class) => class_contains_direct_eval(class),
        StatementKind::Expression(expression)
        | StatementKind::Throw(expression)
        | StatementKind::ExportDefault(expression) => expression_contains_direct_eval(expression),
        StatementKind::Return(value) => value.as_ref().is_some_and(expression_contains_direct_eval),
        StatementKind::Block(body) => body_contains_direct_eval(body),
        StatementKind::If {
            test,
            consequent,
            alternate,
        } => {
            expression_contains_direct_eval(test)
                || statement_contains_direct_eval(consequent)
                || alternate
                    .as_deref()
                    .is_some_and(statement_contains_direct_eval)
        }
        StatementKind::While { test, body } | StatementKind::DoWhile { test, body } => {
            expression_contains_direct_eval(test) || statement_contains_direct_eval(body)
        }
        StatementKind::For {
            init,
            test,
            update,
            body,
        } => {
            init.as_deref().is_some_and(statement_contains_direct_eval)
                || test.as_ref().is_some_and(expression_contains_direct_eval)
                || update.as_ref().is_some_and(expression_contains_direct_eval)
                || statement_contains_direct_eval(body)
        }
        StatementKind::ForOf { iterable, body, .. } => {
            expression_contains_direct_eval(iterable) || statement_contains_direct_eval(body)
        }
        StatementKind::ForIn { object, body, .. } => {
            expression_contains_direct_eval(object) || statement_contains_direct_eval(body)
        }
        StatementKind::Labelled { body, .. } => statement_contains_direct_eval(body),
        StatementKind::With { object, body } => {
            expression_contains_direct_eval(object) || statement_contains_direct_eval(body)
        }
        StatementKind::Switch {
            discriminant,
            cases,
        } => {
            expression_contains_direct_eval(discriminant)
                || cases.iter().any(|case| {
                    case.test
                        .as_ref()
                        .is_some_and(expression_contains_direct_eval)
                        || body_contains_direct_eval(&case.body)
                })
        }
        StatementKind::Try {
            block,
            handler,
            finalizer,
        } => {
            body_contains_direct_eval(block)
                || handler
                    .as_ref()
                    .is_some_and(|handler| body_contains_direct_eval(&handler.body))
                || body_contains_direct_eval(finalizer)
        }
        StatementKind::Break { .. }
        | StatementKind::Continue { .. }
        | StatementKind::Empty
        | StatementKind::Unsupported { .. } => false,
    }
}

pub(super) fn class_contains_direct_eval(class: &Class) -> bool {
    class
        .superclass
        .as_ref()
        .is_some_and(expression_contains_direct_eval)
        || class
            .constructor
            .as_ref()
            .is_some_and(function_contains_direct_eval)
        || class
            .methods
            .iter()
            .any(|method| function_contains_direct_eval(&method.function))
        || class.initialisers.iter().any(|initialiser| match initialiser {
            ClassInitialiser::Field(field) => {
                matches!(&field.key, ObjectKey::Computed(key) if expression_contains_direct_eval(key))
                    || field
                        .value
                        .as_ref()
                        .is_some_and(expression_contains_direct_eval)
            }
            // A static block is a body like any other, and a direct `eval` inside it sees the
            // same scope the block does.
            ClassInitialiser::StaticBlock(body) => function_contains_direct_eval(body),
        })
}

pub(super) fn pattern_contains_direct_eval(pattern: &Pattern) -> bool {
    match pattern {
        Pattern::Identifier(_) => false,
        Pattern::Member { object, .. } | Pattern::PrivateMember { object, .. } => {
            expression_contains_direct_eval(object)
        }
        Pattern::Computed { object, property } => {
            expression_contains_direct_eval(object) || expression_contains_direct_eval(property)
        }
        Pattern::WithDefault { inner, default } => {
            pattern_contains_direct_eval(inner) || expression_contains_direct_eval(default)
        }
        Pattern::Object { properties, rest } => {
            properties.iter().any(|property| {
                matches!(&property.key, PatternKey::Computed(key) if expression_contains_direct_eval(key))
                    || pattern_contains_direct_eval(&property.value)
            }) || rest
                .as_deref()
                .is_some_and(pattern_contains_direct_eval)
        }
        Pattern::Array { elements, rest } => {
            elements
                .iter()
                .flatten()
                .any(pattern_contains_direct_eval)
                || rest
                    .as_deref()
                    .is_some_and(pattern_contains_direct_eval)
        }
    }
}

pub(super) fn expression_contains_direct_eval(expression: &Expression) -> bool {
    match &expression.kind {
        ExpressionKind::Call {
            callee,
            arguments,
            optional,
        } => {
            (!optional
                && matches!(&callee.kind, ExpressionKind::Identifier(name) if name == "eval"))
                || expression_contains_direct_eval(callee)
                || arguments.iter().any(list_element_contains_direct_eval)
        }
        ExpressionKind::InlineLiteralEval { .. } => true,
        ExpressionKind::Function(function) => function_contains_direct_eval(function),
        ExpressionKind::Class(class) => class_contains_direct_eval(class),
        ExpressionKind::ImportCall { specifier, options } => {
            expression_contains_direct_eval(specifier)
                || options
                    .as_deref()
                    .is_some_and(expression_contains_direct_eval)
        }
        ExpressionKind::Unary { argument, .. }
        | ExpressionKind::Chain {
            expression: argument,
        }
        | ExpressionKind::Await(argument)
        | ExpressionKind::PrivateIn {
            object: argument, ..
        }
        | ExpressionKind::PrivateMember {
            object: argument, ..
        }
        | ExpressionKind::StaticMember {
            object: argument, ..
        } => expression_contains_direct_eval(argument),
        ExpressionKind::Binary { left, right, .. }
        | ExpressionKind::Logical { left, right, .. } => {
            expression_contains_direct_eval(left) || expression_contains_direct_eval(right)
        }
        ExpressionKind::Assignment { value, .. } => expression_contains_direct_eval(value),
        ExpressionKind::PatternAssignment { pattern, value } => {
            pattern_contains_direct_eval(pattern) || expression_contains_direct_eval(value)
        }
        ExpressionKind::Conditional {
            test,
            consequent,
            alternate,
        } => {
            expression_contains_direct_eval(test)
                || expression_contains_direct_eval(consequent)
                || expression_contains_direct_eval(alternate)
        }
        ExpressionKind::New { callee, arguments } => {
            expression_contains_direct_eval(callee)
                || arguments.iter().any(list_element_contains_direct_eval)
        }
        ExpressionKind::SuperCall { arguments } => {
            arguments.iter().any(list_element_contains_direct_eval)
        }
        ExpressionKind::Array { elements } => {
            elements.iter().any(list_element_contains_direct_eval)
        }
        ExpressionKind::Sequence { operands } => {
            operands.iter().any(expression_contains_direct_eval)
        }
        ExpressionKind::Yield { value, .. } => value
            .as_ref()
            .is_some_and(|value| expression_contains_direct_eval(value)),
        ExpressionKind::ComputedMember {
            object, property, ..
        } => {
            expression_contains_direct_eval(object) || expression_contains_direct_eval(property)
        }
        ExpressionKind::ComputedAssignment {
            object,
            property,
            value,
            ..
        } => {
            expression_contains_direct_eval(object)
                || expression_contains_direct_eval(property)
                || expression_contains_direct_eval(value)
        }
        ExpressionKind::MemberAssignment { object, value, .. }
        | ExpressionKind::PrivateAssignment { object, value, .. } => {
            expression_contains_direct_eval(object) || expression_contains_direct_eval(value)
        }
        ExpressionKind::SuperMemberAssignment { value, .. } => {
            expression_contains_direct_eval(value)
        }
        ExpressionKind::SuperComputedAssignment { property, value, .. } => {
            expression_contains_direct_eval(property) || expression_contains_direct_eval(value)
        }
        ExpressionKind::UpdateMember {
            object, property, ..
        }
        | ExpressionKind::Delete { object, property } => {
            expression_contains_direct_eval(object)
                || matches!(property.as_ref(), ObjectKey::Computed(key) if expression_contains_direct_eval(key))
        }
        ExpressionKind::DeleteValue { argument } => expression_contains_direct_eval(argument),
        ExpressionKind::DeleteBinding { .. } => false,
        ExpressionKind::Template { expressions, .. } => {
            expressions.iter().any(expression_contains_direct_eval)
        }
        ExpressionKind::Object { members } => members.iter().any(|member| match member {
            ObjectMember::Property(property) => {
                matches!(&property.key, ObjectKey::Computed(key) if expression_contains_direct_eval(key))
                    || expression_contains_direct_eval(&property.value)
            }
            ObjectMember::Spread(source) => expression_contains_direct_eval(source),
            ObjectMember::Accessor { key, function, .. } => {
                matches!(key, ObjectKey::Computed(key) if expression_contains_direct_eval(key))
                    || function_contains_direct_eval(function)
            }
        }),
        ExpressionKind::SuperComputedMember { property } => {
            expression_contains_direct_eval(property)
        }
        ExpressionKind::Identifier(_)
        | ExpressionKind::Update { .. }
        | ExpressionKind::Literal(_)
        | ExpressionKind::This
        | ExpressionKind::GlobalObject
        | ExpressionKind::NewTarget
        | ExpressionKind::SuperMember { .. }
        | ExpressionKind::RegExp { .. }
        | ExpressionKind::TemplateStrings { .. }
        | ExpressionKind::Unsupported { .. } => false,
    }
}

pub(super) fn list_element_contains_direct_eval(element: &ListElement) -> bool {
    match element {
        ListElement::Item(value) | ListElement::Spread(value) => {
            expression_contains_direct_eval(value)
        }
        ListElement::Hole => false,
    }
}

/// Whether `body` contains `new Function(...)` or dynamic `Function(...)`.
pub fn body_contains_new_function(body: &[Statement]) -> bool {
    body.iter().any(statement_contains_new_function)
}

pub fn function_contains_new_function(function: &Function) -> bool {
    function
        .parameters
        .iter()
        .any(|parameter| pattern_contains_new_function(&parameter.pattern))
        || function
            .rest
            .as_ref()
            .is_some_and(pattern_contains_new_function)
        || body_contains_new_function(&function.body)
}

pub fn module_contains_new_function(module: &Module) -> bool {
    body_contains_new_function(&module.body)
}

pub(super) fn statement_contains_new_function(statement: &Statement) -> bool {
    match &statement.kind {
        StatementKind::Variable { declarations, .. } => declarations.iter().any(|declaration| {
            pattern_contains_new_function(&declaration.pattern)
                || declaration
                    .initializer
                    .as_ref()
                    .is_some_and(expression_contains_new_function)
        }),
        StatementKind::Function(function) => function_contains_new_function(function),
        StatementKind::Class(class) => class_contains_new_function(class),
        StatementKind::Expression(expression)
        | StatementKind::Throw(expression)
        | StatementKind::ExportDefault(expression) => expression_contains_new_function(expression),
        StatementKind::Return(value) => {
            value.as_ref().is_some_and(expression_contains_new_function)
        }
        StatementKind::Block(body) => body_contains_new_function(body),
        StatementKind::If {
            test,
            consequent,
            alternate,
        } => {
            expression_contains_new_function(test)
                || statement_contains_new_function(consequent)
                || alternate
                    .as_deref()
                    .is_some_and(statement_contains_new_function)
        }
        StatementKind::While { test, body } | StatementKind::DoWhile { test, body } => {
            expression_contains_new_function(test) || statement_contains_new_function(body)
        }
        StatementKind::For {
            init,
            test,
            update,
            body,
        } => {
            init.as_deref().is_some_and(statement_contains_new_function)
                || test.as_ref().is_some_and(expression_contains_new_function)
                || update
                    .as_ref()
                    .is_some_and(expression_contains_new_function)
                || statement_contains_new_function(body)
        }
        StatementKind::ForOf { iterable, body, .. } => {
            expression_contains_new_function(iterable) || statement_contains_new_function(body)
        }
        StatementKind::ForIn { object, body, .. } => {
            expression_contains_new_function(object) || statement_contains_new_function(body)
        }
        StatementKind::Labelled { body, .. } => statement_contains_new_function(body),
        StatementKind::With { object, body } => {
            expression_contains_new_function(object) || statement_contains_new_function(body)
        }
        StatementKind::Switch {
            discriminant,
            cases,
        } => {
            expression_contains_new_function(discriminant)
                || cases.iter().any(|case| {
                    case.test
                        .as_ref()
                        .is_some_and(expression_contains_new_function)
                        || body_contains_new_function(&case.body)
                })
        }
        StatementKind::Try {
            block,
            handler,
            finalizer,
        } => {
            body_contains_new_function(block)
                || handler
                    .as_ref()
                    .is_some_and(|handler| body_contains_new_function(&handler.body))
                || body_contains_new_function(finalizer)
        }
        StatementKind::Break { .. }
        | StatementKind::Continue { .. }
        | StatementKind::Empty
        | StatementKind::Unsupported { .. } => false,
    }
}

pub(super) fn class_contains_new_function(class: &Class) -> bool {
    class
        .superclass
        .as_ref()
        .is_some_and(expression_contains_new_function)
        || class
            .constructor
            .as_ref()
            .is_some_and(function_contains_new_function)
        || class
            .methods
            .iter()
            .any(|method| function_contains_new_function(&method.function))
        || class.initialisers.iter().any(|initialiser| match initialiser {
            ClassInitialiser::Field(field) => {
                matches!(&field.key, ObjectKey::Computed(key) if expression_contains_new_function(key))
                    || field
                        .value
                        .as_ref()
                        .is_some_and(expression_contains_new_function)
            }
            ClassInitialiser::StaticBlock(body) => function_contains_new_function(body),
        })
}

pub(super) fn pattern_contains_new_function(pattern: &Pattern) -> bool {
    match pattern {
        Pattern::Identifier(_) => false,
        Pattern::Member { object, .. } | Pattern::PrivateMember { object, .. } => {
            expression_contains_new_function(object)
        }
        Pattern::Computed { object, property } => {
            expression_contains_new_function(object) || expression_contains_new_function(property)
        }
        Pattern::WithDefault { inner, default } => {
            pattern_contains_new_function(inner) || expression_contains_new_function(default)
        }
        Pattern::Object { properties, rest } => {
            properties.iter().any(|property| {
                matches!(&property.key, PatternKey::Computed(key) if expression_contains_new_function(key))
                    || pattern_contains_new_function(&property.value)
            }) || rest
                .as_deref()
                .is_some_and(pattern_contains_new_function)
        }
        Pattern::Array { elements, rest } => {
            elements
                .iter()
                .flatten()
                .any(pattern_contains_new_function)
                || rest
                    .as_deref()
                    .is_some_and(pattern_contains_new_function)
        }
    }
}

pub(super) fn expression_contains_new_function(expression: &Expression) -> bool {
    match &expression.kind {
        ExpressionKind::Call {
            callee,
            arguments,
            ..
        } => {
            matches!(&callee.kind, ExpressionKind::Identifier(name) if name == "Function")
                || expression_contains_new_function(callee)
                || arguments.iter().any(list_element_contains_new_function)
        }
        ExpressionKind::InlineLiteralEval { callee, argument, expression } => {
            expression_contains_new_function(callee) || expression_contains_new_function(argument) || expression_contains_new_function(expression)
        }
        ExpressionKind::New { callee, arguments } => {
            matches!(&callee.kind, ExpressionKind::Identifier(name) if name == "Function")
                || expression_contains_new_function(callee)
                || arguments.iter().any(list_element_contains_new_function)
        }
        ExpressionKind::Function(function) => function_contains_new_function(function),
        ExpressionKind::Class(class) => class_contains_new_function(class),
        ExpressionKind::ImportCall { specifier, options } => {
            expression_contains_new_function(specifier)
                || options
                    .as_deref()
                    .is_some_and(expression_contains_new_function)
        }
        ExpressionKind::Unary { argument, .. }
        | ExpressionKind::Chain {
            expression: argument,
        }
        | ExpressionKind::Await(argument)
        | ExpressionKind::PrivateIn {
            object: argument, ..
        }
        | ExpressionKind::PrivateMember {
            object: argument, ..
        }
        | ExpressionKind::StaticMember {
            object: argument, ..
        } => expression_contains_new_function(argument),
        ExpressionKind::Binary { left, right, .. }
        | ExpressionKind::Logical { left, right, .. } => {
            expression_contains_new_function(left) || expression_contains_new_function(right)
        }
        ExpressionKind::Assignment { value, .. } => expression_contains_new_function(value),
        ExpressionKind::PatternAssignment { pattern, value } => {
            pattern_contains_new_function(pattern) || expression_contains_new_function(value)
        }
        ExpressionKind::Conditional {
            test,
            consequent,
            alternate,
        } => {
            expression_contains_new_function(test)
                || expression_contains_new_function(consequent)
                || expression_contains_new_function(alternate)
        }
        ExpressionKind::SuperCall { arguments } => {
            arguments.iter().any(list_element_contains_new_function)
        }
        ExpressionKind::Array { elements } => {
            elements.iter().any(list_element_contains_new_function)
        }
        ExpressionKind::Sequence { operands } => {
            operands.iter().any(expression_contains_new_function)
        }
        ExpressionKind::Yield { value, .. } => value
            .as_ref()
            .is_some_and(|value| expression_contains_new_function(value)),
        ExpressionKind::ComputedMember {
            object, property, ..
        } => {
            expression_contains_new_function(object) || expression_contains_new_function(property)
        }
        ExpressionKind::ComputedAssignment {
            object,
            property,
            value,
            ..
        } => {
            expression_contains_new_function(object)
                || expression_contains_new_function(property)
                || expression_contains_new_function(value)
        }
        ExpressionKind::MemberAssignment { object, value, .. }
        | ExpressionKind::PrivateAssignment { object, value, .. } => {
            expression_contains_new_function(object) || expression_contains_new_function(value)
        }
        ExpressionKind::SuperMemberAssignment { value, .. } => {
            expression_contains_new_function(value)
        }
        ExpressionKind::SuperComputedAssignment { property, value, .. } => {
            expression_contains_new_function(property) || expression_contains_new_function(value)
        }
        ExpressionKind::UpdateMember {
            object, property, ..
        }
        | ExpressionKind::Delete { object, property } => {
            expression_contains_new_function(object)
                || matches!(property.as_ref(), ObjectKey::Computed(key) if expression_contains_new_function(key))
        }
        ExpressionKind::DeleteValue { argument } => expression_contains_new_function(argument),
        ExpressionKind::DeleteBinding { .. } => false,
        ExpressionKind::Template { expressions, .. } => {
            expressions.iter().any(expression_contains_new_function)
        }
        ExpressionKind::Object { members } => members.iter().any(|member| match member {
            ObjectMember::Property(property) => {
                matches!(&property.key, ObjectKey::Computed(key) if expression_contains_new_function(key))
                    || expression_contains_new_function(&property.value)
            }
            ObjectMember::Spread(source) => expression_contains_new_function(source),
            ObjectMember::Accessor { key, function, .. } => {
                matches!(key, ObjectKey::Computed(key) if expression_contains_new_function(key))
                    || function_contains_new_function(function)
            }
        }),
        ExpressionKind::SuperComputedMember { property } => {
            expression_contains_new_function(property)
        }
        ExpressionKind::Identifier(_)
        | ExpressionKind::Update { .. }
        | ExpressionKind::Literal(_)
        | ExpressionKind::This
        | ExpressionKind::GlobalObject
        | ExpressionKind::NewTarget
        | ExpressionKind::SuperMember { .. }
        | ExpressionKind::RegExp { .. }
        | ExpressionKind::TemplateStrings { .. }
        | ExpressionKind::Unsupported { .. } => false,
    }
}

pub(super) fn list_element_contains_new_function(element: &ListElement) -> bool {
    match element {
        ListElement::Item(value) | ListElement::Spread(value) => {
            expression_contains_new_function(value)
        }
        ListElement::Hole => false,
    }
}

/// Collect all dynamic imports from `body`.
pub fn collect_dynamic_imports(body: &[Statement]) -> Vec<DynamicImportTarget> {
    let mut imports = Vec::new();
    for statement in body {
        collect_dynamic_imports_from_statement(statement, &mut imports);
    }
    imports
}

pub(super) fn collect_dynamic_imports_from_statement(
    statement: &Statement,
    imports: &mut Vec<DynamicImportTarget>,
) {
    match &statement.kind {
        StatementKind::Variable { declarations, .. } => {
            for declaration in declarations {
                collect_dynamic_imports_from_pattern(&declaration.pattern, imports);
                if let Some(init) = &declaration.initializer {
                    collect_dynamic_imports_from_expression(init, imports);
                }
            }
        }
        StatementKind::Function(function) => {
            collect_dynamic_imports_from_function(function, imports)
        }
        StatementKind::Class(class) => collect_dynamic_imports_from_class(class, imports),
        StatementKind::Expression(expression)
        | StatementKind::Throw(expression)
        | StatementKind::ExportDefault(expression) => {
            collect_dynamic_imports_from_expression(expression, imports);
        }
        StatementKind::Return(value) => {
            if let Some(v) = value {
                collect_dynamic_imports_from_expression(v, imports);
            }
        }
        StatementKind::Block(body) => {
            for s in body {
                collect_dynamic_imports_from_statement(s, imports);
            }
        }
        StatementKind::If {
            test,
            consequent,
            alternate,
        } => {
            collect_dynamic_imports_from_expression(test, imports);
            collect_dynamic_imports_from_statement(consequent, imports);
            if let Some(alt) = alternate {
                collect_dynamic_imports_from_statement(alt, imports);
            }
        }
        StatementKind::While { test, body } | StatementKind::DoWhile { test, body } => {
            collect_dynamic_imports_from_expression(test, imports);
            collect_dynamic_imports_from_statement(body, imports);
        }
        StatementKind::For {
            init,
            test,
            update,
            body,
        } => {
            if let Some(i) = init {
                collect_dynamic_imports_from_statement(i, imports);
            }
            if let Some(t) = test {
                collect_dynamic_imports_from_expression(t, imports);
            }
            if let Some(u) = update {
                collect_dynamic_imports_from_expression(u, imports);
            }
            collect_dynamic_imports_from_statement(body, imports);
        }
        StatementKind::ForOf { iterable, body, .. } => {
            collect_dynamic_imports_from_expression(iterable, imports);
            collect_dynamic_imports_from_statement(body, imports);
        }
        StatementKind::ForIn { object, body, .. } => {
            collect_dynamic_imports_from_expression(object, imports);
            collect_dynamic_imports_from_statement(body, imports);
        }
        StatementKind::Labelled { body, .. } => {
            collect_dynamic_imports_from_statement(body, imports)
        }
        StatementKind::With { object, body } => {
            collect_dynamic_imports_from_expression(object, imports);
            collect_dynamic_imports_from_statement(body, imports);
        }
        StatementKind::Switch {
            discriminant,
            cases,
        } => {
            collect_dynamic_imports_from_expression(discriminant, imports);
            for case in cases {
                if let Some(test) = &case.test {
                    collect_dynamic_imports_from_expression(test, imports);
                }
                for s in &case.body {
                    collect_dynamic_imports_from_statement(s, imports);
                }
            }
        }
        StatementKind::Try {
            block,
            handler,
            finalizer,
        } => {
            for s in block {
                collect_dynamic_imports_from_statement(s, imports);
            }
            if let Some(h) = handler {
                for s in &h.body {
                    collect_dynamic_imports_from_statement(s, imports);
                }
            }
            for s in finalizer {
                collect_dynamic_imports_from_statement(s, imports);
            }
        }
        StatementKind::Break { .. }
        | StatementKind::Continue { .. }
        | StatementKind::Empty
        | StatementKind::Unsupported { .. } => {}
    }
}

pub(super) fn collect_dynamic_imports_from_function(
    function: &Function,
    imports: &mut Vec<DynamicImportTarget>,
) {
    for param in &function.parameters {
        collect_dynamic_imports_from_pattern(&param.pattern, imports);
    }
    if let Some(rest) = &function.rest {
        collect_dynamic_imports_from_pattern(rest, imports);
    }
    for s in &function.body {
        collect_dynamic_imports_from_statement(s, imports);
    }
}

pub(super) fn collect_dynamic_imports_from_class(
    class: &Class,
    imports: &mut Vec<DynamicImportTarget>,
) {
    if let Some(superclass) = &class.superclass {
        collect_dynamic_imports_from_expression(superclass, imports);
    }
    if let Some(ctor) = &class.constructor {
        collect_dynamic_imports_from_function(ctor, imports);
    }
    for method in &class.methods {
        collect_dynamic_imports_from_function(&method.function, imports);
    }
    for initialiser in &class.initialisers {
        match initialiser {
            ClassInitialiser::Field(field) => {
                if let ObjectKey::Computed(key) = &field.key {
                    collect_dynamic_imports_from_expression(key, imports);
                }
                if let Some(value) = &field.value {
                    collect_dynamic_imports_from_expression(value, imports);
                }
            }
            ClassInitialiser::StaticBlock(body) => {
                collect_dynamic_imports_from_function(body, imports);
            }
        }
    }
}

pub(super) fn collect_dynamic_imports_from_pattern(
    pattern: &Pattern,
    imports: &mut Vec<DynamicImportTarget>,
) {
    match pattern {
        Pattern::Identifier(_) => {}
        Pattern::Member { object, .. } | Pattern::PrivateMember { object, .. } => {
            collect_dynamic_imports_from_expression(object, imports);
        }
        Pattern::Computed { object, property } => {
            collect_dynamic_imports_from_expression(object, imports);
            collect_dynamic_imports_from_expression(property, imports);
        }
        Pattern::WithDefault { inner, default } => {
            collect_dynamic_imports_from_pattern(inner, imports);
            collect_dynamic_imports_from_expression(default, imports);
        }
        Pattern::Object { properties, rest } => {
            for property in properties {
                if let PatternKey::Computed(key) = &property.key {
                    collect_dynamic_imports_from_expression(key, imports);
                }
                collect_dynamic_imports_from_pattern(&property.value, imports);
            }
            if let Some(r) = rest {
                collect_dynamic_imports_from_pattern(r, imports);
            }
        }
        Pattern::Array { elements, rest } => {
            for el in elements.iter().flatten() {
                collect_dynamic_imports_from_pattern(el, imports);
            }
            if let Some(r) = rest {
                collect_dynamic_imports_from_pattern(r, imports);
            }
        }
    }
}

pub(super) fn collect_dynamic_imports_from_expression(
    expression: &Expression,
    imports: &mut Vec<DynamicImportTarget>,
) {
    match &expression.kind {
        ExpressionKind::ImportCall { specifier, options } => {
            let target = match &specifier.kind {
                ExpressionKind::Literal(Literal::String(s)) => {
                    DynamicImportTarget::Static(s.to_lossy_string())
                }
                ExpressionKind::Template {
                    quasis,
                    expressions,
                } if expressions.is_empty() && quasis.len() == 1 => {
                    DynamicImportTarget::Static(quasis[0].to_lossy_string())
                }
                _ => DynamicImportTarget::Dynamic,
            };
            imports.push(target);
            collect_dynamic_imports_from_expression(specifier, imports);
            if let Some(opts) = options {
                collect_dynamic_imports_from_expression(opts, imports);
            }
        }
        ExpressionKind::Call {
            callee, arguments, ..
        } => {
            collect_dynamic_imports_from_expression(callee, imports);
            for arg in arguments {
                collect_dynamic_imports_from_list_element(arg, imports);
            }
        }
        ExpressionKind::InlineLiteralEval {
            callee,
            argument,
            expression,
        } => {
            collect_dynamic_imports_from_expression(callee, imports);
            collect_dynamic_imports_from_expression(argument, imports);
            collect_dynamic_imports_from_expression(expression, imports);
        }
        ExpressionKind::New { callee, arguments } => {
            collect_dynamic_imports_from_expression(callee, imports);
            for arg in arguments {
                collect_dynamic_imports_from_list_element(arg, imports);
            }
        }
        ExpressionKind::Function(function) => {
            collect_dynamic_imports_from_function(function, imports)
        }
        ExpressionKind::Class(class) => collect_dynamic_imports_from_class(class, imports),
        ExpressionKind::Unary { argument, .. }
        | ExpressionKind::Chain {
            expression: argument,
        }
        | ExpressionKind::Await(argument)
        | ExpressionKind::PrivateIn {
            object: argument, ..
        }
        | ExpressionKind::PrivateMember {
            object: argument, ..
        }
        | ExpressionKind::StaticMember {
            object: argument, ..
        } => {
            collect_dynamic_imports_from_expression(argument, imports);
        }
        ExpressionKind::Binary { left, right, .. }
        | ExpressionKind::Logical { left, right, .. } => {
            collect_dynamic_imports_from_expression(left, imports);
            collect_dynamic_imports_from_expression(right, imports);
        }
        ExpressionKind::Assignment { value, .. } => {
            collect_dynamic_imports_from_expression(value, imports);
        }
        ExpressionKind::PatternAssignment { pattern, value } => {
            collect_dynamic_imports_from_pattern(pattern, imports);
            collect_dynamic_imports_from_expression(value, imports);
        }
        ExpressionKind::Conditional {
            test,
            consequent,
            alternate,
        } => {
            collect_dynamic_imports_from_expression(test, imports);
            collect_dynamic_imports_from_expression(consequent, imports);
            collect_dynamic_imports_from_expression(alternate, imports);
        }
        ExpressionKind::SuperCall { arguments } => {
            for arg in arguments {
                collect_dynamic_imports_from_list_element(arg, imports);
            }
        }
        ExpressionKind::Array { elements } => {
            for el in elements {
                collect_dynamic_imports_from_list_element(el, imports);
            }
        }
        ExpressionKind::Sequence { operands } => {
            for op in operands {
                collect_dynamic_imports_from_expression(op, imports);
            }
        }
        ExpressionKind::Yield { value, .. } => {
            if let Some(v) = value {
                collect_dynamic_imports_from_expression(v, imports);
            }
        }
        ExpressionKind::ComputedMember {
            object, property, ..
        } => {
            collect_dynamic_imports_from_expression(object, imports);
            collect_dynamic_imports_from_expression(property, imports);
        }
        ExpressionKind::ComputedAssignment {
            object,
            property,
            value,
            ..
        } => {
            collect_dynamic_imports_from_expression(object, imports);
            collect_dynamic_imports_from_expression(property, imports);
            collect_dynamic_imports_from_expression(value, imports);
        }
        ExpressionKind::MemberAssignment { object, value, .. }
        | ExpressionKind::PrivateAssignment { object, value, .. } => {
            collect_dynamic_imports_from_expression(object, imports);
            collect_dynamic_imports_from_expression(value, imports);
        }
        ExpressionKind::SuperMemberAssignment { value, .. } => {
            collect_dynamic_imports_from_expression(value, imports);
        }
        ExpressionKind::SuperComputedAssignment {
            property, value, ..
        } => {
            collect_dynamic_imports_from_expression(property, imports);
            collect_dynamic_imports_from_expression(value, imports);
        }
        ExpressionKind::UpdateMember {
            object, property, ..
        }
        | ExpressionKind::Delete { object, property } => {
            collect_dynamic_imports_from_expression(object, imports);
            if let ObjectKey::Computed(key) = property.as_ref() {
                collect_dynamic_imports_from_expression(key, imports);
            }
        }
        ExpressionKind::DeleteValue { argument } => {
            collect_dynamic_imports_from_expression(argument, imports);
        }
        ExpressionKind::DeleteBinding { .. } => {}
        ExpressionKind::Template { expressions, .. } => {
            for expr in expressions {
                collect_dynamic_imports_from_expression(expr, imports);
            }
        }
        ExpressionKind::Object { members } => {
            for member in members {
                match member {
                    ObjectMember::Property(prop) => {
                        if let ObjectKey::Computed(key) = &prop.key {
                            collect_dynamic_imports_from_expression(key, imports);
                        }
                        collect_dynamic_imports_from_expression(&prop.value, imports);
                    }
                    ObjectMember::Spread(source) => {
                        collect_dynamic_imports_from_expression(source, imports);
                    }
                    ObjectMember::Accessor { key, function, .. } => {
                        if let ObjectKey::Computed(key) = key {
                            collect_dynamic_imports_from_expression(key, imports);
                        }
                        collect_dynamic_imports_from_function(function, imports);
                    }
                }
            }
        }
        ExpressionKind::SuperComputedMember { property } => {
            collect_dynamic_imports_from_expression(property, imports);
        }
        ExpressionKind::Identifier(_)
        | ExpressionKind::Update { .. }
        | ExpressionKind::Literal(_)
        | ExpressionKind::This
        | ExpressionKind::GlobalObject
        | ExpressionKind::NewTarget
        | ExpressionKind::SuperMember { .. }
        | ExpressionKind::RegExp { .. }
        | ExpressionKind::TemplateStrings { .. }
        | ExpressionKind::Unsupported { .. } => {}
    }
}

pub(super) fn collect_dynamic_imports_from_list_element(
    element: &ListElement,
    imports: &mut Vec<DynamicImportTarget>,
) {
    match element {
        ListElement::Item(value) | ListElement::Spread(value) => {
            collect_dynamic_imports_from_expression(value, imports);
        }
        ListElement::Hole => {}
    }
}

/// Names that nested functions inside a pattern's expressions refer to.
///
/// A pattern reads names as well as binding them: a default, a computed key, and the object a
/// reference target stores through are all ordinary expressions, and a closure in any of them
/// decides whether the name it reaches has to be boxed.
pub(super) fn collect_pattern_captures(pattern: &Pattern, names: &mut HashSet<String>) {
    match pattern {
        Pattern::Identifier(_) => {}
        Pattern::Member { object, .. } | Pattern::PrivateMember { object, .. } => {
            collect_identifiers(object, names, false);
        }
        Pattern::Computed { object, property } => {
            collect_identifiers(object, names, false);
            collect_identifiers(property, names, false);
        }
        Pattern::WithDefault { inner, default } => {
            collect_identifiers(default, names, false);
            collect_pattern_captures(inner, names);
        }
        Pattern::Object { properties, rest } => {
            for property in properties {
                if let PatternKey::Computed(key) = &property.key {
                    collect_identifiers(key, names, false);
                }
                collect_pattern_captures(&property.value, names);
            }
            if let Some(rest) = rest {
                collect_pattern_captures(rest, names);
            }
        }
        Pattern::Array { elements, rest } => {
            for element in elements.iter().flatten() {
                collect_pattern_captures(element, names);
            }
            if let Some(rest) = rest {
                collect_pattern_captures(rest, names);
            }
        }
    }
}

/// Every identifier evaluated while a pattern is initialised.
///
/// Unlike [`collect_pattern_captures`], this starts in collecting mode: the pattern itself is
/// already inside a nested function relative to its owner. A function parameter default such as
/// `function f(x = f.value) {}` therefore captures the outer `f` binding even though the
/// function body never mentions it.
pub(super) fn collect_pattern_identifiers(pattern: &Pattern, names: &mut HashSet<String>) {
    match pattern {
        Pattern::Identifier(_) => {}
        Pattern::Member { object, .. } | Pattern::PrivateMember { object, .. } => {
            collect_identifiers(object, names, true);
        }
        Pattern::Computed { object, property } => {
            collect_identifiers(object, names, true);
            collect_identifiers(property, names, true);
        }
        Pattern::WithDefault { inner, default } => {
            collect_identifiers(default, names, true);
            collect_pattern_identifiers(inner, names);
        }
        Pattern::Object { properties, rest } => {
            for property in properties {
                if let PatternKey::Computed(key) = &property.key {
                    collect_identifiers(key, names, true);
                }
                // `{ delimiter = DEFAULT_DELIMITER }` stores the default on the property value
                // pattern, so recurse into that pattern's expressions as well as its computed
                // key. Merely walking the bound name misses the initializer entirely.
                collect_pattern_identifiers(&property.value, names);
            }
            if let Some(rest) = rest {
                collect_pattern_identifiers(rest, names);
            }
        }
        Pattern::Array { elements, rest } => {
            for element in elements.iter().flatten() {
                collect_pattern_identifiers(element, names);
            }
            if let Some(rest) = rest {
                collect_pattern_identifiers(rest, names);
            }
        }
    }
}

/// Whether a target stores through a property reference rather than binding a name.
pub(super) fn stores_through_reference(pattern: &Pattern) -> bool {
    match pattern {
        Pattern::Member { .. } | Pattern::Computed { .. } | Pattern::PrivateMember { .. } => true,
        Pattern::WithDefault { inner, .. } => stores_through_reference(inner),
        _ => false,
    }
}

pub(super) fn collect_nested_function_names(
    statement: &Statement,
    names: &mut HashSet<String>,
    inside_function: bool,
) {
    match &statement.kind {
        StatementKind::Function(function) => {
            for parameter in &function.parameters {
                collect_pattern_identifiers(&parameter.pattern, names);
            }
            if let Some(rest) = &function.rest {
                collect_pattern_identifiers(rest, names);
            }
            for statement in &function.body {
                collect_nested_function_names(statement, names, true);
            }
        }
        // Every method body is a nested function, so its free names are capture candidates.
        StatementKind::Class(class) => {
            if let Some(superclass) = &class.superclass {
                collect_identifiers(superclass, names, inside_function);
            }
            collect_class_names(class, names);
        }
        StatementKind::Variable { declarations, .. } => {
            for declaration in declarations {
                if let Some(initializer) = &declaration.initializer {
                    collect_identifiers(initializer, names, inside_function);
                }
                // The *pattern* reads names too, and only the initializer was ever walked. A
                // destructuring default and a computed key are both ordinary expressions
                // evaluated in this scope -- `const { a = n } = {}` inside a nested function
                // reads `n` from the enclosing one -- so a binding named only there was left
                // unboxed and the nested function could not resolve it at all.
                //
                // `collect_pattern_identifiers` records what a pattern *reads*, never what it
                // binds, which is why walking it here cannot box the declaration's own names.
                if inside_function {
                    collect_pattern_identifiers(&declaration.pattern, names);
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
        StatementKind::Labelled { body, .. } => {
            collect_nested_function_names(body, names, inside_function);
        }
        StatementKind::With { object, body } => {
            collect_identifiers(object, names, inside_function);
            collect_nested_function_names(body, names, inside_function);
        }
        StatementKind::ForIn { object, body, .. } => {
            collect_identifiers(object, names, inside_function);
            collect_nested_function_names(body, names, inside_function);
        }
        StatementKind::DoWhile { body, test } => {
            collect_nested_function_names(body, names, inside_function);
            collect_identifiers(test, names, inside_function);
        }
        StatementKind::Switch {
            discriminant,
            cases,
        } => {
            collect_identifiers(discriminant, names, inside_function);
            for case in cases {
                if let Some(test) = &case.test {
                    collect_identifiers(test, names, inside_function);
                }
                for statement in &case.body {
                    collect_nested_function_names(statement, names, inside_function);
                }
            }
        }
        StatementKind::For {
            init,
            test,
            update,
            body,
        } => {
            if let Some(init) = init {
                collect_nested_function_names(init, names, inside_function);
            }
            if let Some(test) = test {
                collect_identifiers(test, names, inside_function);
            }
            if let Some(update) = update {
                collect_identifiers(update, names, inside_function);
            }
            collect_nested_function_names(body, names, inside_function);
        }
        StatementKind::ForOf {
            pattern,
            iterable,
            body,
            ..
        } => {
            collect_identifiers(iterable, names, inside_function);
            // The loop's own pattern, for the reason a declaration's is walked above:
            // `for (const { a = n } of xs)` reads `n` where the loop is written.
            if inside_function {
                collect_pattern_identifiers(pattern, names);
            }
            collect_nested_function_names(body, names, inside_function);
        }
        StatementKind::Throw(value) => collect_identifiers(value, names, inside_function),
        StatementKind::Try {
            block,
            handler,
            finalizer,
        } => {
            for statement in block.iter().chain(finalizer) {
                collect_nested_function_names(statement, names, inside_function);
            }
            if let Some(handler) = handler {
                for statement in &handler.body {
                    collect_nested_function_names(statement, names, inside_function);
                }
            }
        }
        StatementKind::Break { .. }
        | StatementKind::Continue { .. }
        | StatementKind::Empty
        | StatementKind::Unsupported { .. } => {}
    }
}

/// Walk an expression, recording identifiers when `collecting`.
///
/// It descends unconditionally and gates only the recording. That distinction matters: a function
/// *expression* nested in a `return` is a nested function whose free names are captures, and an
/// early return on `!collecting` would never reach it. That was a real bug -- `function f(n) {
/// return (m) => n + m; }` compiled with `n` unboxed, so the arrow could not resolve it.
/// Names a class's methods and field initialisers refer to.
///
/// Field initialisers count: they run inside the constructor, so a name one of them reads is
/// captured by the constructor exactly as a method body's would be.
pub(super) fn collect_class_names(class: &Class, names: &mut HashSet<String>) {
    let bodies = class
        .constructor
        .iter()
        .chain(class.methods.iter().map(|method| &method.function))
        // A static block closes over the class's surroundings exactly as a method body does.
        .chain(
            class
                .initialisers
                .iter()
                .filter_map(ClassInitialiser::as_static_block),
        );
    for function in bodies {
        for parameter in &function.parameters {
            collect_pattern_identifiers(&parameter.pattern, names);
        }
        if let Some(rest) = &function.rest {
            collect_pattern_identifiers(rest, names);
        }
        for statement in &function.body {
            collect_nested_function_names(statement, names, true);
        }
    }
    for field in class
        .initialisers
        .iter()
        .filter_map(ClassInitialiser::as_field)
    {
        if let ObjectKey::Computed(key) = &field.key {
            collect_identifiers(key, names, true);
        }
        if let Some(value) = &field.value {
            collect_identifiers(value, names, true);
        }
    }
}

/// Every name a pattern binds.
/// The binary operation a compound assignment performs before storing.
pub(super) fn compound_operator(operator: AssignmentOperator) -> BinaryOp {
    match operator {
        AssignmentOperator::Add => BinaryOp::Add,
        AssignmentOperator::Subtract => BinaryOp::Subtract,
        AssignmentOperator::Multiply => BinaryOp::Multiply,
        AssignmentOperator::Divide => BinaryOp::Divide,
        AssignmentOperator::Remainder => BinaryOp::Remainder,
        AssignmentOperator::Exponentiate => BinaryOp::Exponentiate,
        AssignmentOperator::BitwiseAnd => BinaryOp::BitwiseAnd,
        AssignmentOperator::BitwiseOr => BinaryOp::BitwiseOr,
        AssignmentOperator::BitwiseXor => BinaryOp::BitwiseXor,
        AssignmentOperator::ShiftLeft => BinaryOp::ShiftLeft,
        AssignmentOperator::ShiftRight => BinaryOp::ShiftRight,
        AssignmentOperator::UnsignedShiftRight => BinaryOp::UnsignedShiftRight,
        AssignmentOperator::Assign => unreachable!("a plain assignment performs no operation"),
        AssignmentOperator::LogicalAnd
        | AssignmentOperator::LogicalOr
        | AssignmentOperator::Nullish => {
            unreachable!("a short-circuiting assignment computes nothing")
        }
    }
}

/// The jump that skips the store for a short-circuiting assignment.
///
/// `a ||= b` assigns only when `a` is falsy, so the jump taken *past* the store is the one taken
/// when `a` is truthy -- which is why each operator maps to the opposite test.
pub(super) fn short_circuit_jump(operator: AssignmentOperator) -> Instruction {
    match operator {
        AssignmentOperator::LogicalAnd => Instruction::JumpIfFalse(0),
        AssignmentOperator::LogicalOr => Instruction::JumpIfTruthy(0),
        AssignmentOperator::Nullish => Instruction::JumpIfNotNullish(0),
        _ => unreachable!("only a short-circuiting assignment skips its store"),
    }
}

/// The kind an installed private element carries, from the syntax that declared it.
pub(super) fn private_kind(kind: kiln_hir::MethodKind) -> PrivateKind {
    match kind {
        kiln_hir::MethodKind::Normal => PrivateKind::Method,
        kiln_hir::MethodKind::Get => PrivateKind::Getter,
        kiln_hir::MethodKind::Set => PrivateKind::Setter,
    }
}

/// The reserved property name a private member is stored under.
///
/// Reserved rather than ordinary, so nothing that enumerates own string properties can see it and
/// no computed access can spell it -- `obj["#x"]` names a different property, as it should.
pub(super) fn private_key(module_id: &str, scope: u32, name: &str) -> String {
    // The module identity is hex-encoded so it cannot introduce a separator into the reserved
    // key grammar. A class ordinal is only unique within one Compiler; graph modules compile
    // independently, so the stable module identity is part of the property key as well.
    if module_id.is_empty() {
        // Hand-built HIR fixtures have no source graph identity; retain their compact local
        // spelling while parsed/linked modules always supply one.
        return format!("@@private:{scope}:{name}");
    }
    let mut identity = String::with_capacity(module_id.len() * 2 + 8);
    for byte in module_id.bytes() {
        use std::fmt::Write as _;
        write!(identity, "{byte:02x}").expect("writing to String cannot fail");
    }
    format!("@@private:{identity}-{scope}:{name}")
}

/// The names a pattern binds, in source order.
///
/// Ordered, unlike [`collect_pattern_names`], because these decide slot numbering and the
/// bytecode is a cache key: the same source has to produce the same instructions, and a hash set
/// does not promise that.
pub(super) fn pattern_names_in_order(pattern: &Pattern, names: &mut Vec<String>) {
    match pattern {
        Pattern::Identifier(name) => names.push(name.clone()),
        // A reference target binds nothing: `[o.p] = xs` stores into an object that already
        // exists, and `o` was declared somewhere else.
        Pattern::Member { .. } | Pattern::Computed { .. } | Pattern::PrivateMember { .. } => {}
        Pattern::WithDefault { inner, .. } => pattern_names_in_order(inner, names),
        Pattern::Object { properties, rest } => {
            for property in properties {
                pattern_names_in_order(&property.value, names);
            }
            if let Some(rest) = rest {
                pattern_names_in_order(rest, names);
            }
        }
        Pattern::Array { elements, rest } => {
            for element in elements.iter().flatten() {
                pattern_names_in_order(element, names);
            }
            if let Some(rest) = rest {
                pattern_names_in_order(rest, names);
            }
        }
    }
}

pub(super) fn collect_pattern_names(pattern: &Pattern, names: &mut HashSet<String>) {
    match pattern {
        Pattern::Identifier(name) => {
            names.insert(name.clone());
        }
        Pattern::Member { .. } | Pattern::Computed { .. } | Pattern::PrivateMember { .. } => {}
        Pattern::WithDefault { inner, .. } => collect_pattern_names(inner, names),
        Pattern::Object { properties, rest } => {
            for property in properties {
                collect_pattern_names(&property.value, names);
            }
            if let Some(rest) = rest {
                collect_pattern_names(rest, names);
            }
        }
        Pattern::Array { elements, rest } => {
            for element in elements.iter().flatten() {
                collect_pattern_names(element, names);
            }
            if let Some(rest) = rest {
                collect_pattern_names(rest, names);
            }
        }
    }
}

pub(super) fn collect_identifiers(
    expression: &Expression,
    names: &mut HashSet<String>,
    collecting: bool,
) {
    match &expression.kind {
        // A nested function's body is always collected, whatever the enclosing flag says.
        ExpressionKind::Function(function) => {
            for parameter in &function.parameters {
                collect_pattern_identifiers(&parameter.pattern, names);
            }
            if let Some(rest) = &function.rest {
                collect_pattern_identifiers(rest, names);
            }
            for statement in &function.body {
                // A function expression always establishes a nested function relative to the
                // expression that contains it. This must not inherit `collecting`: at module
                // level that flag is false by design, yet
                // `Layer.prototype.match = function match () { return MODULE_CONST }` still
                // closes over the module. Inheriting false emitted a `LoadGlobal` that later
                // indexed whichever CommonJS module called the exported method.
                collect_nested_function_names(statement, names, true);
            }
        }
        // A class expression's methods and field initialisers are nested functions too, and a
        // field initialiser closing over the enclosing scope is exactly what g03 exercises.
        ExpressionKind::Class(class) => {
            collect_class_names(class, names);
        }
        ExpressionKind::Identifier(name) => {
            if collecting {
                names.insert(name.clone());
            }
        }
        ExpressionKind::Update { target, .. } => {
            if collecting {
                names.insert(target.clone());
            }
        }
        ExpressionKind::PrivateIn { object, .. } => {
            collect_identifiers(object, names, collecting);
        }
        ExpressionKind::SuperComputedMember { property } => {
            collect_identifiers(property, names, collecting);
        }
        ExpressionKind::Literal(_)
        | ExpressionKind::This
        | ExpressionKind::GlobalObject
        | ExpressionKind::NewTarget
        | ExpressionKind::SuperMember { .. }
        | ExpressionKind::Unsupported { .. } => {}
        ExpressionKind::SuperCall { arguments } => {
            for argument in arguments {
                let (ListElement::Item(value) | ListElement::Spread(value)) = argument else {
                    continue;
                };
                collect_identifiers(value, names, collecting);
            }
        }
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
        ExpressionKind::ImportCall { specifier, options } => {
            collect_identifiers(specifier, names, collecting);
            if let Some(options) = options {
                collect_identifiers(options, names, collecting);
            }
        }
        // Every name the pattern assigns is written by this function, so each is a capture
        // candidate for the same reason a plain assignment target is.
        ExpressionKind::PatternAssignment { pattern, value } => {
            if collecting {
                collect_pattern_names(pattern, names);
            }
            collect_pattern_captures(pattern, names);
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
        ExpressionKind::Call {
            callee, arguments, ..
        }
        | ExpressionKind::New { callee, arguments } => {
            collect_identifiers(callee, names, collecting);
            for argument in arguments {
                let (ListElement::Item(value) | ListElement::Spread(value)) = argument else {
                    continue;
                };
                collect_identifiers(value, names, collecting);
            }
        }
        ExpressionKind::InlineLiteralEval {
            callee,
            argument,
            expression,
        } => {
            collect_identifiers(callee, names, collecting);
            collect_identifiers(argument, names, collecting);
            collect_identifiers(expression, names, collecting);
        }
        ExpressionKind::Array { elements } => {
            for element in elements {
                let (ListElement::Item(value) | ListElement::Spread(value)) = element else {
                    continue;
                };
                collect_identifiers(value, names, collecting);
            }
        }
        ExpressionKind::Sequence { operands } => {
            for operand in operands {
                collect_identifiers(operand, names, collecting);
            }
        }
        ExpressionKind::Yield { value, .. } => {
            if let Some(value) = value {
                collect_identifiers(value, names, collecting);
            }
        }
        ExpressionKind::ComputedMember {
            object, property, ..
        } => {
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
            // The value is walked like any other expression: a function expression in it
            // establishes its own nesting (the `Function` arm above collects its body whatever
            // this flag says, which is what keeps `Layer.prototype.match = function match (...)
            // {}` closing over its module), and a plain value does not. Walking the value as if
            // it were nested boxed every local a member write read -- `m.h = m.f` made `m` a
            // cell -- and a cell is a heap read at every use and an escape to the scalar
            // analysis.
            collect_identifiers(value, names, collecting);
        }
        ExpressionKind::SuperMemberAssignment { value, .. } => {
            collect_identifiers(value, names, collecting);
        }
        ExpressionKind::SuperComputedAssignment {
            property, value, ..
        } => {
            collect_identifiers(property, names, collecting);
            collect_identifiers(value, names, collecting);
        }
        ExpressionKind::Chain { expression } | ExpressionKind::Await(expression) => {
            collect_identifiers(expression, names, collecting);
        }
        ExpressionKind::RegExp { .. } => {}
        ExpressionKind::PrivateMember { object, .. } => {
            collect_identifiers(object, names, collecting);
        }
        ExpressionKind::PrivateAssignment { object, value, .. } => {
            collect_identifiers(object, names, collecting);
            collect_identifiers(value, names, collecting);
        }
        ExpressionKind::UpdateMember {
            object, property, ..
        } => {
            collect_identifiers(object, names, collecting);
            if let ObjectKey::Computed(key) = property.as_ref() {
                collect_identifiers(key, names, collecting);
            }
        }
        // A template object is built from constants; it names nothing.
        ExpressionKind::TemplateStrings { .. } => {}
        ExpressionKind::Template { expressions, .. } => {
            for expression in expressions {
                collect_identifiers(expression, names, collecting);
            }
        }
        ExpressionKind::Delete { object, property } => {
            collect_identifiers(object, names, collecting);
            if let ObjectKey::Computed(key) = property.as_ref() {
                collect_identifiers(key, names, collecting);
            }
        }
        ExpressionKind::DeleteValue { argument } => {
            collect_identifiers(argument, names, collecting);
        }
        // The name is a reference like any other: an eval that writes `delete x` needs the outer
        // `x` bound, or it would ask the global object about a binding a caller's scope holds.
        ExpressionKind::DeleteBinding { name } => {
            if collecting {
                names.insert(name.clone());
            }
        }
        ExpressionKind::Object { members } => {
            for member in members {
                match member {
                    ObjectMember::Property(property) => {
                        if let ObjectKey::Computed(key) = &property.key {
                            collect_identifiers(key, names, collecting);
                        }
                        // Property values are expressions in this lexical region. For capture
                        // analysis the important case is a function value: its body is nested
                        // relative to the current owner even when the surrounding expression is
                        // itself already inside another function.
                        collect_identifiers(&property.value, names, collecting);
                    }
                    ObjectMember::Spread(source) => {
                        collect_identifiers(source, names, collecting);
                    }
                    ObjectMember::Accessor { key, function, .. } => {
                        if let ObjectKey::Computed(key) = key {
                            collect_identifiers(key, names, collecting);
                        }
                        // The body is a nested function, so every name in it is a candidate.
                        for statement in &function.body {
                            collect_nested_function_names(statement, names, true);
                        }
                    }
                }
            }
        }
        ExpressionKind::StaticMember { object, .. } => {
            collect_identifiers(object, names, collecting);
        }
    }
}
/// The block-level function declarations Annex B gives a var-scoped binding, in source order.
///
/// `nested` says whether this list is one the specification calls a `Block`, `CaseClause` or
/// `DefaultClause` rather than the top level of the variable environment: a declaration written
/// at the top level is already var-scoped by the main grammar and is not Annex B's business.
///
/// `blocked` is every name a lexical declaration between the variable environment and here has
/// taken, because replacing the function declaration with a `var` of that name would be an early
/// error and Annex B declines exactly when it would be.
pub(super) fn collect_annexb_function_names(
    body: &[Statement],
    nested: bool,
    blocked: &mut Vec<String>,
    out: &mut Vec<(String, Span)>,
) {
    let depth = blocked.len();
    for statement in body {
        collect_lexical_names(statement, blocked);
    }
    for statement in body {
        if nested
            && let StatementKind::Function(function) = &statement.kind
            && let Some(name) = &function.name
            // Only a plain function declaration. A generator or an async function inside a block
            // is block-scoped and nothing more; Annex B never mentions either.
            && !function.generator
            && !function.asynchronous
            && !blocked.contains(name)
        {
            out.push((name.clone(), function.span));
        }
        collect_annexb_in_statement(statement, blocked, out);
    }
    blocked.truncate(depth);
}

/// Walk one statement for the nested lists Annex B looks inside, without leaving this variable
/// environment: a nested function owns its own, and so does a class body.
pub(super) fn collect_annexb_in_statement(
    statement: &Statement,
    blocked: &mut Vec<String>,
    out: &mut Vec<(String, Span)>,
) {
    match &statement.kind {
        StatementKind::Block(body) => collect_annexb_function_names(body, true, blocked, out),
        StatementKind::If {
            consequent,
            alternate,
            ..
        } => {
            collect_annexb_branch(consequent, blocked, out);
            if let Some(alternate) = alternate {
                collect_annexb_branch(alternate, blocked, out);
            }
        }
        StatementKind::While { body, .. } | StatementKind::DoWhile { body, .. } => {
            collect_annexb_in_statement(body, blocked, out);
        }
        // A label does not open a scope, so a declaration under one is at whatever level the
        // label is: `l: function f() {}` at the top of a body is var-scoped already.
        StatementKind::Labelled { body, .. } => collect_annexb_in_statement(body, blocked, out),
        StatementKind::With { body, .. } => collect_annexb_in_statement(body, blocked, out),
        StatementKind::For { init, body, .. } => {
            let depth = blocked.len();
            if let Some(init) = init {
                collect_lexical_names(init, blocked);
            }
            collect_annexb_in_statement(body, blocked, out);
            blocked.truncate(depth);
        }
        StatementKind::ForIn {
            kind, name, body, ..
        } => {
            let depth = blocked.len();
            if *kind != VariableKind::Var {
                blocked.push(name.clone());
            }
            collect_annexb_in_statement(body, blocked, out);
            blocked.truncate(depth);
        }
        StatementKind::ForOf {
            kind,
            pattern,
            body,
            ..
        } => {
            let depth = blocked.len();
            if *kind != VariableKind::Var {
                pattern_names_in_order(pattern, blocked);
            }
            collect_annexb_in_statement(body, blocked, out);
            blocked.truncate(depth);
        }
        // Every clause of a `switch` shares one lexical scope, so a `let` in any of them blocks
        // a function declaration in all of them.
        StatementKind::Switch { cases, .. } => {
            let depth = blocked.len();
            for case in cases {
                for statement in &case.body {
                    collect_lexical_names(statement, blocked);
                }
            }
            for case in cases {
                for statement in &case.body {
                    if let StatementKind::Function(function) = &statement.kind
                        && let Some(name) = &function.name
                        && !function.generator
                        && !function.asynchronous
                        && !blocked.contains(name)
                    {
                        out.push((name.clone(), function.span));
                    }
                    collect_annexb_in_statement(statement, blocked, out);
                }
            }
            blocked.truncate(depth);
        }
        StatementKind::Try {
            block,
            handler,
            finalizer,
        } => {
            collect_annexb_function_names(block, true, blocked, out);
            if let Some(handler) = handler {
                // B.3.4 removes the early error for `catch (e) { var e; }`, so a catch parameter
                // does not block -- and the HIR carries only the simple form, which is exactly
                // the one the exemption is written for.
                collect_annexb_function_names(&handler.body, true, blocked, out);
            }
            collect_annexb_function_names(finalizer, true, blocked, out);
        }
        _ => {}
    }
}

/// An `if` branch is a statement rather than a list, and a function declaration written directly
/// as one is the form Annex B B.3.4 exists for: `if (x) function f() {}`.
pub(super) fn collect_annexb_branch(
    statement: &Statement,
    blocked: &mut Vec<String>,
    out: &mut Vec<(String, Span)>,
) {
    if let StatementKind::Function(function) = &statement.kind
        && let Some(name) = &function.name
        && !function.generator
        && !function.asynchronous
        && !blocked.contains(name)
    {
        out.push((name.clone(), function.span));
    }
    collect_annexb_in_statement(statement, blocked, out);
}

/// The names one statement declares lexically, which are the ones a `var` of the same name could
/// not join.
pub(super) fn collect_lexical_names(statement: &Statement, names: &mut Vec<String>) {
    match &statement.kind {
        StatementKind::Variable { kind, declarations } if *kind != VariableKind::Var => {
            for declaration in declarations {
                pattern_names_in_order(&declaration.pattern, names);
            }
        }
        StatementKind::Class(class) => {
            if let Some(name) = &class.name {
                names.push(name.clone());
            }
        }
        _ => {}
    }
}

/// Whether a property's value is a *syntactically* anonymous function definition.
///
/// `NamedEvaluation` names one of these after the property key; it does not name a reference to
/// a function that happens to have no name, which is why this asks the shape of the expression
/// rather than the name of the value. `{ [k]: (0, function () {}) }` is a parenthesised
/// reference and keeps the empty name. An arrow is a `Function` with the flag set and is always
/// anonymous, so it needs no arm of its own.
pub(super) fn anonymous_function_value(value: &kiln_hir::Expression) -> bool {
    match &value.kind {
        kiln_hir::ExpressionKind::Function(function) => function.name.is_none(),
        kiln_hir::ExpressionKind::Class(class) => class.name.is_none(),
        _ => false,
    }
}
