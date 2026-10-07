//! Lower executable values and statements from the public, owned HIR into the
//! checker engine's AST-shaped HIR.

use crate::engine::{atom::Atom, hir as b};
use crate::lower::Lower;
use kiln_hir as k;

impl<'a, 's> Lower<'a, 's> {
    /// Attach the source modifiers represented by retained declaration metadata. The checker
    /// grammar gates declaration files on the statement modifier list, even when the declaration
    /// itself also stores flags (for example, on a VarDecl or Class).
    pub(super) fn declaration_modifiers(
        &mut self,
        flags: b::Flags,
        pos: u32,
    ) -> b::Span<b::ModifierId> {
        // Keep the canonical TypeScript modifier order: export precedes declare, and default
        // follows both. The checker validates this sequence and declaration emit uses it too.
        let modifiers = [b::Flags::EXPORT, b::Flags::AMBIENT, b::Flags::DEFAULT]
            .into_iter()
            .filter(|flag| flags.contains(*flag))
            .map(|flag| b::Modifier {
                kind: b::ModifierKind::Keyword(flag),
                pos,
            })
            .collect::<Vec<_>>();
        self.file.add_modifiers(&modifiers)
    }

    /// Oxc removes `export` from executable statements in namespace bodies. The
    /// module-record export table only describes file-level exports, so recover
    /// that modifier from the retained declaration span for namespace members.
    pub(super) fn namespace_export_flag(&self, span: k::Span) -> b::Flags {
        let nested = self
            .module
            .type_syntax
            .declarations
            .iter()
            .any(|declaration| {
                matches!(
                    declaration.kind,
                    k::TypeDeclarationKind::Namespace | k::TypeDeclarationKind::Module
                ) && declaration.span.start <= span.start
                    && declaration.span.end >= span.end
                    && declaration.span != span
            });
        let exported = self
            .module
            .type_syntax
            .exported_declarations
            .iter()
            .any(|declaration| {
                declaration.end == span.end
                    && declaration.start >= span.start
                    && declaration.start <= span.end
            });
        if nested && exported {
            b::Flags::EXPORT
        } else {
            b::Flags::empty()
        }
    }

    pub(super) fn binding_span(&self, name: &str, span: k::Span) -> k::Span {
        let sites = &self.module.type_syntax.binding_identifiers;
        let first = sites.partition_point(|site| site.span.start < span.start);
        sites[first..]
            .iter()
            .take_while(|site| site.span.start < span.end)
            .find(|site| site.name == name && site.span.end <= span.end)
            .map_or(span, |site| site.span)
    }

    pub(super) fn pattern(&mut self, p: &k::Pattern, span: k::Span) -> b::PatId {
        let span = if let k::Pattern::Identifier(name) = p {
            self.binding_span(name, span)
        } else {
            span
        };
        let kind = match p {
            k::Pattern::Identifier(name) => b::PatKind::Ident(self.atom(name)),
            k::Pattern::WithDefault { inner, .. } => return self.pattern(inner, span),
            k::Pattern::Object { properties, rest } => {
                let mut props: Vec<_> = properties
                    .iter()
                    .map(|prop| {
                        let key = match &prop.key {
                            k::PatternKey::Static(text) => b::PropKey::Name(self.atom_units(text)),
                            k::PatternKey::Computed(expr) => b::PropKey::Computed(self.expr(expr)),
                        };
                        let value = self.pattern(&prop.value, span);
                        let default = match &prop.value {
                            k::Pattern::WithDefault { default, .. } => self.expr(default),
                            _ => b::ExprId::NONE,
                        };
                        b::PatProp {
                            key,
                            name_kind: b::NameKind::Identifier,
                            value,
                            default,
                            is_rest: false,
                            pos: span.start,
                            key_pos: span.start,
                            end: span.end,
                        }
                    })
                    .collect();
                if let Some(rest) = rest {
                    let value = self.pattern(rest, span);
                    props.push(b::PatProp {
                        key: b::PropKey::None,
                        name_kind: b::NameKind::Identifier,
                        value,
                        default: b::ExprId::NONE,
                        is_rest: true,
                        pos: span.start,
                        key_pos: span.start,
                        end: span.end,
                    });
                }
                b::PatKind::Object(self.file.add_pat_props(&props))
            }
            k::Pattern::Array { elements, rest } => {
                let mut elems: Vec<_> = elements
                    .iter()
                    .map(|element| {
                        let pat = match element {
                            Some(p) => self.pattern(p, span),
                            None => self.file.pat(b::PatKind::Missing, span.start, span.end),
                        };
                        let default = match element {
                            Some(k::Pattern::WithDefault { default, .. }) => self.expr(default),
                            _ => b::ExprId::NONE,
                        };
                        b::PatElem {
                            pat,
                            default,
                            is_rest: false,
                            start: span.start,
                            end: span.end,
                        }
                    })
                    .collect();
                if let Some(rest) = rest {
                    let pat = self.pattern(rest, span);
                    elems.push(b::PatElem {
                        pat,
                        default: b::ExprId::NONE,
                        is_rest: true,
                        start: span.start,
                        end: span.end,
                    });
                }
                b::PatKind::Array(self.file.add_pat_elems(&elems))
            }
            k::Pattern::Member { .. }
            | k::Pattern::Computed { .. }
            | k::Pattern::PrivateMember { .. } => {
                self.error(span, "member binding pattern");
                b::PatKind::Missing
            }
        };
        self.file.pat(kind, span.start, span.end)
    }

    pub(super) fn key(&mut self, key: &k::ObjectKey) -> b::PropKey {
        match key {
            k::ObjectKey::Static(text) => b::PropKey::Name(self.atom_units(text)),
            k::ObjectKey::Computed(expr) => b::PropKey::Computed(self.expr(expr)),
            k::ObjectKey::Private(name) => b::PropKey::Private(self.atom(name)),
        }
    }

    pub(super) fn args(&mut self, args: &[k::ListElement], span: k::Span) -> b::IdList<b::ExprId> {
        let ids: Vec<_> = args
            .iter()
            .map(|element| match element {
                k::ListElement::Item(expr) => self.expr(expr),
                k::ListElement::Hole => {
                    self.file.expr(b::ExprKind::Missing, span.start, span.start)
                }
                k::ListElement::Spread(expr) => {
                    let value = self.expr(expr);
                    self.file
                        .expr(b::ExprKind::Spread(value), expr.span.start, expr.span.end)
                }
            })
            .collect();
        self.file.list(&ids)
    }

    pub(super) fn expr(&mut self, expr: &k::Expression) -> b::ExprId {
        use k::ExpressionKind as E;
        let span = expr.span;
        let kind = match &expr.kind {
            E::Identifier(name) => b::ExprKind::Ident(self.atom(name)),
            E::Literal(literal) => match literal {
                k::Literal::Undefined => b::ExprKind::Ident(self.atom("undefined")),
                k::Literal::Null => b::ExprKind::Null,
                k::Literal::Boolean(true) => b::ExprKind::True,
                k::Literal::Boolean(false) => b::ExprKind::False,
                k::Literal::Number(bits) => {
                    b::ExprKind::Number(self.file.number(f64::from_bits(*bits)))
                }
                k::Literal::String(text) => b::ExprKind::String(self.atom_units(text)),
                k::Literal::BigInt(digits) => b::ExprKind::BigInt(self.atom(digits)),
            },
            E::This => b::ExprKind::This,
            E::SuperCall { arguments } => {
                let args = self.args(arguments, span);
                let callee = self
                    .file
                    .expr(b::ExprKind::Super, span.start, span.start + 5);
                let call = self.file.add_call(b::Call {
                    callee,
                    args,
                    type_args: b::IdList::EMPTY,
                    close_pos: span.end.saturating_sub(1),
                    chain: b::Chain::No,
                    template: b::ExprId::NONE,
                });
                b::ExprKind::Call(call)
            }
            E::SuperMember { property } => b::ExprKind::Dot {
                obj: self
                    .file
                    .expr(b::ExprKind::Super, span.start, span.start + 5),
                name: self.atom(property),
                name_pos: span.end.saturating_sub(property.len() as u32),
                chain: b::Chain::No,
            },
            E::SuperComputedMember { property } => {
                let obj = self
                    .file
                    .expr(b::ExprKind::Super, span.start, span.start + 5);
                let index = self.expr(property);
                b::ExprKind::Index {
                    obj,
                    index,
                    chain: b::Chain::No,
                }
            }
            E::Function(function) => b::ExprKind::Fn(self.function(function)),
            E::Class(class) => b::ExprKind::Class(self.class(class)),
            E::Unary { operator, argument } => {
                let operand = self.expr(argument);
                let op = match operator {
                    k::UnaryOperator::Plus => b::UnOp::Plus,
                    k::UnaryOperator::Negate => b::UnOp::Minus,
                    k::UnaryOperator::Not => b::UnOp::Not,
                    k::UnaryOperator::TypeOf => b::UnOp::Typeof,
                    k::UnaryOperator::BitwiseNot => b::UnOp::BitNot,
                    k::UnaryOperator::Void => b::UnOp::Void,
                };
                b::ExprKind::Unary { op, operand }
            }
            E::Binary {
                operator,
                left,
                right,
            } => {
                let left = self.expr(left);
                let right = self.expr(right);
                b::ExprKind::Binary {
                    op: bin(*operator),
                    left,
                    right,
                }
            }
            E::Logical {
                operator,
                left,
                right,
            } => {
                let left = self.expr(left);
                let right = self.expr(right);
                let op = match operator {
                    k::LogicalOperator::And => b::BinOp::And,
                    k::LogicalOperator::Or => b::BinOp::Or,
                    k::LogicalOperator::NullishCoalesce => b::BinOp::Nullish,
                };
                b::ExprKind::Binary { op, left, right }
            }
            E::Assignment {
                operator,
                target,
                value,
            } => {
                let target_id = self.file.expr(
                    b::ExprKind::Ident(self.atom(target)),
                    span.start,
                    span.start + target.len() as u32,
                );
                let value = self.expr(value);
                b::ExprKind::Assign {
                    op: assign(*operator),
                    target: target_id,
                    value,
                }
            }
            E::MemberAssignment {
                operator,
                object,
                property,
                value,
            } => {
                let obj = self.expr(object);
                let name = self.atom(property);
                let target = self.file.expr(
                    b::ExprKind::Dot {
                        obj,
                        name,
                        name_pos: span.end.saturating_sub(property.len() as u32),
                        chain: b::Chain::No,
                    },
                    object.span.start,
                    span.end,
                );
                let value = self.expr(value);
                b::ExprKind::Assign {
                    op: assign(*operator),
                    target,
                    value,
                }
            }
            E::ComputedAssignment {
                operator,
                object,
                property,
                value,
            } => {
                let obj = self.expr(object);
                let index = self.expr(property);
                let target = self.file.expr(
                    b::ExprKind::Index {
                        obj,
                        index,
                        chain: b::Chain::No,
                    },
                    object.span.start,
                    span.end,
                );
                let value = self.expr(value);
                b::ExprKind::Assign {
                    op: assign(*operator),
                    target,
                    value,
                }
            }
            E::PrivateAssignment {
                operator,
                object,
                name,
                value,
            } => {
                let obj = self.expr(object);
                let name_atom = self.atom(name);
                let target = self.file.expr(
                    b::ExprKind::Dot {
                        obj,
                        name: name_atom,
                        name_pos: private_key_pos(&self.file.text, object.span.end, span.end, name),
                        chain: b::Chain::No,
                    },
                    object.span.start,
                    span.end,
                );
                let value = self.expr(value);
                b::ExprKind::Assign {
                    op: assign(*operator),
                    target,
                    value,
                }
            }
            E::SuperMemberAssignment {
                operator,
                property,
                value,
            } => {
                let obj = self
                    .file
                    .expr(b::ExprKind::Super, span.start, span.start + 5);
                let name = self.atom(property);
                let target = self.file.expr(
                    b::ExprKind::Dot {
                        obj,
                        name,
                        name_pos: span.end.saturating_sub(property.len() as u32),
                        chain: b::Chain::No,
                    },
                    span.start,
                    span.end,
                );
                let value = self.expr(value);
                b::ExprKind::Assign {
                    op: assign(*operator),
                    target,
                    value,
                }
            }
            E::SuperComputedAssignment {
                operator,
                property,
                value,
            } => {
                let obj = self
                    .file
                    .expr(b::ExprKind::Super, span.start, span.start + 5);
                let index = self.expr(property);
                let target = self.file.expr(
                    b::ExprKind::Index {
                        obj,
                        index,
                        chain: b::Chain::No,
                    },
                    span.start,
                    span.end,
                );
                let value = self.expr(value);
                b::ExprKind::Assign {
                    op: assign(*operator),
                    target,
                    value,
                }
            }
            E::PrivateIn { name, object } => {
                let name_atom = self.atom(name);
                let left = self.file.expr(
                    b::ExprKind::PrivateIdentifier(name_atom),
                    span.start,
                    span.start + name.len() as u32 + 1,
                );
                let right = self.expr(object);
                b::ExprKind::Binary {
                    op: b::BinOp::In,
                    left,
                    right,
                }
            }
            E::NewTarget => b::ExprKind::NewTarget(self.atom("target")),
            E::PatternAssignment { pattern, value } => {
                let target = self.pattern_target(pattern, span);
                let value = self.expr(value);
                b::ExprKind::Assign {
                    op: None,
                    target,
                    value,
                }
            }
            E::Conditional {
                test,
                consequent,
                alternate,
            } => {
                let test = self.expr(test);
                let yes = self.expr(consequent);
                let no = self.expr(alternate);
                b::ExprKind::Cond { test, yes, no }
            }
            E::Call {
                callee,
                arguments,
                optional,
            } => {
                let callee = self.expr(callee);
                let tagged = arguments.first().and_then(|argument| match argument {
                    k::ListElement::Item(template) => match &template.kind {
                        E::TemplateStrings { cooked, raw } => Some((cooked, raw)),
                        _ => None,
                    },
                    _ => None,
                });
                let mut template = b::ExprId::NONE;
                let args = if let Some((cooked, raw)) = tagged {
                    let substitutions: Vec<_> = arguments
                        .iter()
                        .skip(1)
                        .map(|arg| match arg {
                            k::ListElement::Item(expr) => self.expr(expr),
                            k::ListElement::Hole => {
                                self.file.expr(b::ExprKind::Missing, span.start, span.end)
                            }
                            k::ListElement::Spread(expr) => {
                                let value = self.expr(expr);
                                self.file.expr(
                                    b::ExprKind::Spread(value),
                                    expr.span.start,
                                    expr.span.end,
                                )
                            }
                        })
                        .collect();
                    let texts: Vec<_> = cooked
                        .iter()
                        .enumerate()
                        .map(|(i, text)| {
                            self.atom_units(
                                text.as_ref()
                                    .or_else(|| raw.get(i))
                                    .expect("raw and cooked template lengths agree"),
                            )
                        })
                        .collect();
                    let substitutions = self.file.list(&substitutions);
                    self.file.ids.extend(texts.iter().map(|atom| atom.0));
                    template = self.file.expr(
                        b::ExprKind::Template {
                            exprs: substitutions,
                        },
                        span.start,
                        span.end,
                    );
                    substitutions
                } else {
                    self.args(arguments, span)
                };
                let type_args = self.call_type_arguments(span);
                let call = self.file.add_call(b::Call {
                    callee,
                    args,
                    type_args,
                    close_pos: if template.is_none() {
                        span.end.saturating_sub(1)
                    } else {
                        u32::MAX
                    },
                    chain: if *optional {
                        b::Chain::Start
                    } else {
                        b::Chain::No
                    },
                    template,
                });
                if template.is_none() {
                    b::ExprKind::Call(call)
                } else {
                    b::ExprKind::TaggedTemplate(call)
                }
            }
            E::New { callee, arguments } => {
                let callee = self.expr(callee);
                let args = self.args(arguments, span);
                let type_args = self.call_type_arguments(span);
                let call = self.file.add_call(b::Call {
                    callee,
                    args,
                    type_args,
                    close_pos: u32::MAX,
                    chain: b::Chain::No,
                    template: b::ExprId::NONE,
                });
                b::ExprKind::New(call)
            }
            E::StaticMember {
                object,
                property,
                optional,
            } => {
                let obj = self.expr(object);
                let name = self.atom(property);
                b::ExprKind::Dot {
                    obj,
                    name,
                    name_pos: span.end.saturating_sub(property.len() as u32),
                    chain: if *optional {
                        b::Chain::Start
                    } else {
                        b::Chain::No
                    },
                }
            }
            E::ComputedMember {
                object,
                property,
                optional,
            } => {
                let obj = self.expr(object);
                let index = self.expr(property);
                b::ExprKind::Index {
                    obj,
                    index,
                    chain: if *optional {
                        b::Chain::Start
                    } else {
                        b::Chain::No
                    },
                }
            }
            E::PrivateMember {
                object,
                name,
                optional,
            } => {
                let obj = self.expr(object);
                let name_atom = self.atom(name);
                b::ExprKind::Dot {
                    obj,
                    name: name_atom,
                    name_pos: private_key_pos(&self.file.text, object.span.end, span.end, name),
                    chain: if *optional {
                        b::Chain::Start
                    } else {
                        b::Chain::No
                    },
                }
            }
            E::DeleteBinding { name } => {
                let target = self.file.expr(
                    b::ExprKind::Ident(self.atom(name)),
                    span.start + 7,
                    span.end,
                );
                b::ExprKind::Unary {
                    op: b::UnOp::Delete,
                    operand: target,
                }
            }
            E::DeleteValue { argument } => {
                let operand = self.expr(argument);
                b::ExprKind::Unary {
                    op: b::UnOp::Delete,
                    operand,
                }
            }
            E::Delete { object, property } => {
                let target = match property.as_ref() {
                    k::ObjectKey::Static(name) => {
                        let obj = self.expr(object);
                        let name_pos = span.end.saturating_sub(name.len() as u32);
                        let name = self.atom_units(name);
                        self.file.expr(
                            b::ExprKind::Dot {
                                obj,
                                name,
                                name_pos,
                                chain: b::Chain::No,
                            },
                            object.span.start,
                            span.end,
                        )
                    }
                    k::ObjectKey::Computed(key) => {
                        let obj = self.expr(object);
                        let index = self.expr(key);
                        self.file.expr(
                            b::ExprKind::Index {
                                obj,
                                index,
                                chain: b::Chain::No,
                            },
                            object.span.start,
                            span.end,
                        )
                    }
                    k::ObjectKey::Private(name) => {
                        let obj = self.expr(object);
                        let name_atom = self.atom(name);
                        self.file.expr(
                            b::ExprKind::Dot {
                                obj,
                                name: name_atom,
                                name_pos: private_key_pos(
                                    &self.file.text,
                                    object.span.end,
                                    span.end,
                                    name,
                                ),
                                chain: b::Chain::No,
                            },
                            object.span.start,
                            span.end,
                        )
                    }
                };
                b::ExprKind::Unary {
                    op: b::UnOp::Delete,
                    operand: target,
                }
            }
            E::Update {
                increment,
                prefix,
                target,
            } => {
                let operand =
                    self.file
                        .expr(b::ExprKind::Ident(self.atom(target)), span.start, span.end);
                b::ExprKind::Unary {
                    op: match (*increment, *prefix) {
                        (true, true) => b::UnOp::PreInc,
                        (false, true) => b::UnOp::PreDec,
                        (true, false) => b::UnOp::PostInc,
                        (false, false) => b::UnOp::PostDec,
                    },
                    operand,
                }
            }
            E::UpdateMember {
                increment,
                prefix,
                object,
                property,
            } => {
                let operand = match property.as_ref() {
                    k::ObjectKey::Static(name) => {
                        let obj = self.expr(object);
                        let atom = self.atom_units(name);
                        let name_pos = span.end.saturating_sub(name.len() as u32);
                        self.file.expr(
                            b::ExprKind::Dot {
                                obj,
                                name: atom,
                                name_pos,
                                chain: b::Chain::No,
                            },
                            object.span.start,
                            span.end,
                        )
                    }
                    k::ObjectKey::Computed(key) => {
                        let obj = self.expr(object);
                        let index = self.expr(key);
                        self.file.expr(
                            b::ExprKind::Index {
                                obj,
                                index,
                                chain: b::Chain::No,
                            },
                            object.span.start,
                            span.end,
                        )
                    }
                    k::ObjectKey::Private(name) => {
                        let obj = self.expr(object);
                        let atom = self.atom(name);
                        let name_pos =
                            private_key_pos(&self.file.text, object.span.end, span.end, name);
                        self.file.expr(
                            b::ExprKind::Dot {
                                obj,
                                name: atom,
                                name_pos,
                                chain: b::Chain::No,
                            },
                            object.span.start,
                            span.end,
                        )
                    }
                };
                b::ExprKind::Unary {
                    op: match (*increment, *prefix) {
                        (true, true) => b::UnOp::PreInc,
                        (false, true) => b::UnOp::PreDec,
                        (true, false) => b::UnOp::PostInc,
                        (false, false) => b::UnOp::PostDec,
                    },
                    operand,
                }
            }
            E::Chain { expression } => {
                let root = self.expr(expression);
                let mut active = false;
                self.mark_optional_chain(root, &mut active);
                return root;
            }
            E::Yield { delegate, value } => b::ExprKind::Yield {
                value: value.as_ref().map_or(b::ExprId::NONE, |v| self.expr(v)),
                star: *delegate,
            },
            E::Await(value) => b::ExprKind::Await(self.expr(value)),
            E::ImportCall { specifier, options } => {
                let mut args = vec![self.expr(specifier)];
                if let Some(options) = options {
                    args.push(self.expr(options));
                }
                let args = self.file.list(&args);
                b::ExprKind::ImportCall { args }
            }
            E::Array { elements } => b::ExprKind::Array(self.args(elements, span)),
            E::Sequence { operands } => {
                let mut ids: Vec<_> = operands.iter().map(|e| self.expr(e)).collect();
                if ids.is_empty() {
                    b::ExprKind::Missing
                } else {
                    let mut value = ids.remove(0);
                    for right in ids {
                        value = self.file.expr(
                            b::ExprKind::Binary {
                                op: b::BinOp::Comma,
                                left: value,
                                right,
                            },
                            span.start,
                            span.end,
                        );
                    }
                    return value;
                }
            }
            E::Object { members } => {
                let mut props = Vec::with_capacity(members.len());
                for member in members {
                    match member {
                        k::ObjectMember::Property(property) => {
                            let key = self.key(&property.key);
                            let value = self.expr(&property.value);
                            let kind = match property.form {
                                k::PropertyForm::KeyValue => b::PropKind::Init,
                                k::PropertyForm::Shorthand => b::PropKind::Shorthand,
                                k::PropertyForm::Method => b::PropKind::Method,
                            };
                            props.push(b::Prop {
                                kind,
                                key,
                                name_kind: b::NameKind::Identifier,
                                value,
                                pos: property.span.start,
                                start: property.span.start,
                                end: property.span.end,
                                postfix_token: 0,
                            });
                        }
                        k::ObjectMember::Spread(expr) => {
                            let value = self.expr(expr);
                            props.push(b::Prop {
                                kind: b::PropKind::Spread,
                                key: b::PropKey::None,
                                name_kind: b::NameKind::Identifier,
                                value,
                                pos: expr.span.start,
                                start: expr.span.start,
                                end: expr.span.end,
                                postfix_token: 0,
                            });
                        }
                        k::ObjectMember::Accessor {
                            key,
                            kind,
                            function,
                        } => {
                            let key = self.key(key);
                            let func = self.function(function);
                            self.file[func].kind = match kind {
                                k::MethodKind::Get => b::FnKind::Getter,
                                _ => b::FnKind::Setter,
                            };
                            let value = self.file.expr(
                                b::ExprKind::Fn(func),
                                function.span.start,
                                function.span.end,
                            );
                            props.push(b::Prop {
                                kind: match kind {
                                    k::MethodKind::Get => b::PropKind::Getter,
                                    _ => b::PropKind::Setter,
                                },
                                key,
                                name_kind: b::NameKind::Identifier,
                                value,
                                pos: function.span.start,
                                start: function.span.start,
                                end: function.span.end,
                                postfix_token: 0,
                            });
                        }
                    }
                }
                b::ExprKind::Object(self.file.add_props(&props))
            }
            E::Template {
                quasis,
                expressions,
            } => self.template(quasis, expressions, span),
            E::TemplateStrings { cooked, raw } => {
                // This syntax node is used only to represent a tagged-template site. The engine
                // stores its cooked/raw text in the template-text column and the substitutions
                // in the call's argument list.
                let quasis: Vec<_> = cooked
                    .iter()
                    .enumerate()
                    .map(|(i, text)| {
                        text.as_ref()
                            .or_else(|| raw.get(i))
                            .cloned()
                            .unwrap_or_else(|| kiln_string::StringUnits::from_utf8(b"").unwrap())
                    })
                    .collect();
                self.template(&quasis, &[], span)
            }
            E::RegExp { .. } => b::ExprKind::Regex,
            E::Unsupported { feature } => {
                self.error(span, feature.clone());
                b::ExprKind::Missing
            }
        };
        let expression = self.file.expr(kind, span.start, span.end);
        // The frontend erases TypeScript wrappers from executable HIR. Follow the exact
        // operand-span chain so nested assertions survive without wrapping child expressions.
        #[derive(Clone)]
        enum WrapperKind {
            As(k::TypeExpr),
            Satisfies(k::TypeExpr),
            NonNull,
            AsConst,
            Instantiation(Vec<k::TypeExpr>),
        }
        #[derive(Clone)]
        struct Wrapper {
            owner: k::Span,
            operand: k::Span,
            kind: WrapperKind,
        }
        let syntax = &self.module.type_syntax;
        let mut wrappers: Vec<_> = syntax
            .annotations
            .iter()
            .filter_map(|site| {
                let operand = site.operand_span?;
                let kind = match site.site {
                    k::TypeSite::AsExpression => WrapperKind::As(site.ty.clone()),
                    k::TypeSite::SatisfiesExpression => WrapperKind::Satisfies(site.ty.clone()),
                    _ => return None,
                };
                Some(Wrapper {
                    owner: site.owner_span,
                    operand,
                    kind,
                })
            })
            .chain(syntax.expression_wrappers.iter().map(|site| Wrapper {
                owner: site.owner_span,
                operand: site.operand_span,
                kind: match site.kind {
                    k::TypeExpressionWrapperKind::NonNull => WrapperKind::NonNull,
                    k::TypeExpressionWrapperKind::AsConst => WrapperKind::AsConst,
                },
            }))
            .chain(syntax.instantiations.iter().filter_map(|site| {
                (site.kind == k::TypeInstantiationKind::Expression).then_some(Wrapper {
                    owner: site.owner_span,
                    operand: site.operand_span?,
                    kind: WrapperKind::Instantiation(site.arguments.clone()),
                })
            }))
            .collect();
        let mut expression = expression;
        let mut operand_span = span;
        loop {
            let Some(index) = wrappers
                .iter()
                .position(|site| site.operand == operand_span)
            else {
                break;
            };
            let site = wrappers.remove(index);
            let kind = match site.kind {
                WrapperKind::As(ty) => b::ExprKind::As {
                    expr: expression,
                    ty: self.ty(&ty),
                },
                WrapperKind::Satisfies(ty) => b::ExprKind::Satisfies {
                    expr: expression,
                    ty: self.ty(&ty),
                },
                WrapperKind::NonNull => b::ExprKind::NonNull(expression),
                WrapperKind::AsConst => b::ExprKind::AsConst(expression),
                WrapperKind::Instantiation(arguments) => b::ExprKind::Instantiation {
                    expr: expression,
                    type_args: self.ty_list(&arguments),
                },
            };
            expression = self.file.expr(kind, site.owner.start, site.owner.end);
            operand_span = site.owner;
        }
        expression
    }

    fn template(
        &mut self,
        quasis: &[kiln_string::StringUnits],
        expressions: &[k::Expression],
        _span: k::Span,
    ) -> b::ExprKind {
        let exprs: Vec<_> = expressions.iter().map(|expr| self.expr(expr)).collect();
        let ids = self.file.list(&exprs);
        let texts: Vec<_> = quasis.iter().map(|text| self.atom_units(text)).collect();
        self.file.ids.extend(texts.iter().map(|atom| atom.0));
        b::ExprKind::Template { exprs: ids }
    }

    /// Marks accesses after the optional hop as continuations of the same chain.
    fn mark_optional_chain(&mut self, expression: b::ExprId, active: &mut bool) {
        let kind = self.file[expression].kind;
        match kind {
            b::ExprKind::Dot { obj, chain, .. } => {
                self.mark_optional_chain(obj, active);
                let chain = if chain == b::Chain::Start {
                    *active = true;
                    b::Chain::Start
                } else if *active {
                    b::Chain::Continue
                } else {
                    chain
                };
                if let b::ExprKind::Dot { chain: slot, .. } = &mut self.file[expression].kind {
                    *slot = chain;
                }
            }
            b::ExprKind::Index { obj, chain, .. } => {
                self.mark_optional_chain(obj, active);
                let chain = if chain == b::Chain::Start {
                    *active = true;
                    b::Chain::Start
                } else if *active {
                    b::Chain::Continue
                } else {
                    chain
                };
                if let b::ExprKind::Index { chain: slot, .. } = &mut self.file[expression].kind {
                    *slot = chain;
                }
            }
            b::ExprKind::Call(call)
            | b::ExprKind::New(call)
            | b::ExprKind::TaggedTemplate(call) => {
                let callee = self.file[call].callee;
                self.mark_optional_chain(callee, active);
                let chain = if self.file[call].chain == b::Chain::Start {
                    *active = true;
                    b::Chain::Start
                } else if *active {
                    b::Chain::Continue
                } else {
                    self.file[call].chain
                };
                self.file[call].chain = chain;
            }
            b::ExprKind::As { expr, .. }
            | b::ExprKind::Satisfies { expr, .. }
            | b::ExprKind::NonNull(expr) => self.mark_optional_chain(expr, active),
            _ => {}
        }
    }

    fn pattern_target(&mut self, pattern: &k::Pattern, span: k::Span) -> b::ExprId {
        match pattern {
            k::Pattern::Identifier(name) => {
                self.file
                    .expr(b::ExprKind::Ident(self.atom(name)), span.start, span.end)
            }
            k::Pattern::Member { object, property } => {
                let obj = self.expr(object);
                let name = self.atom(property);
                self.file.expr(
                    b::ExprKind::Dot {
                        obj,
                        name,
                        name_pos: span.start,
                        chain: b::Chain::No,
                    },
                    span.start,
                    span.end,
                )
            }
            k::Pattern::Computed { object, property } => {
                let obj = self.expr(object);
                let index = self.expr(property);
                self.file.expr(
                    b::ExprKind::Index {
                        obj,
                        index,
                        chain: b::Chain::No,
                    },
                    span.start,
                    span.end,
                )
            }
            k::Pattern::PrivateMember { object, name } => {
                let obj = self.expr(object);
                let name_atom = self.atom(name);
                let name_pos = private_key_pos(&self.file.text, object.span.end, span.end, name);
                self.file.expr(
                    b::ExprKind::Dot {
                        obj,
                        name: name_atom,
                        name_pos,
                        chain: b::Chain::No,
                    },
                    span.start,
                    span.end,
                )
            }
            k::Pattern::WithDefault { inner, default } => {
                let target = self.pattern_target(inner, span);
                let value = self.expr(default);
                self.file.expr(
                    b::ExprKind::Assign {
                        op: None,
                        target,
                        value,
                    },
                    span.start,
                    span.end,
                )
            }
            k::Pattern::Array { elements, rest } => {
                let mut ids: Vec<_> = elements
                    .iter()
                    .map(|item| match item {
                        Some(item) => self.pattern_target(item, span),
                        None => self.file.expr(b::ExprKind::Missing, span.start, span.end),
                    })
                    .collect();
                if let Some(rest) = rest {
                    let id = self.pattern_target(rest, span);
                    ids.push(
                        self.file
                            .expr(b::ExprKind::Spread(id), span.start, span.end),
                    );
                }
                let list = self.file.list(&ids);
                self.file
                    .expr(b::ExprKind::Array(list), span.start, span.end)
            }
            k::Pattern::Object { properties, rest } => {
                let mut props = Vec::with_capacity(properties.len() + usize::from(rest.is_some()));
                for p in properties {
                    let key = match &p.key {
                        k::PatternKey::Static(s) => b::PropKey::Name(self.atom_units(s)),
                        k::PatternKey::Computed(e) => b::PropKey::Computed(self.expr(e)),
                    };
                    let value = self.pattern_target(&p.value, span);
                    props.push(b::Prop {
                        kind: b::PropKind::Init,
                        key,
                        name_kind: b::NameKind::Identifier,
                        value,
                        pos: span.start,
                        start: span.start,
                        end: span.end,
                        postfix_token: 0,
                    });
                }
                if let Some(rest) = rest {
                    let value = self.pattern_target(rest, span);
                    props.push(b::Prop {
                        kind: b::PropKind::Spread,
                        key: b::PropKey::None,
                        name_kind: b::NameKind::Identifier,
                        value,
                        pos: span.start,
                        start: span.start,
                        end: span.end,
                        postfix_token: 0,
                    });
                }
                let properties = self.file.add_props(&props);
                self.file
                    .expr(b::ExprKind::Object(properties), span.start, span.end)
            }
        }
    }

    pub(super) fn statements(&mut self, statements: &[k::Statement]) -> b::IdList<b::StmtId> {
        let ids: Vec<_> = statements
            .iter()
            .map(|statement| self.statement(statement))
            .collect();
        self.file.list(&ids)
    }

    pub(super) fn variable_decls(
        &mut self,
        kind: k::VariableKind,
        declarations: &[k::VariableDeclarator],
        flags: b::Flags,
    ) -> b::Span<b::VarDeclId> {
        let vars: Vec<_> = declarations
            .iter()
            .map(|decl| {
                let pat = self.pattern(&decl.pattern, decl.span);
                let ty = self.annotation(decl.span, |site| {
                    matches!(site, k::TypeSite::Variable { .. })
                });
                let init = decl
                    .initializer
                    .as_ref()
                    .map_or(b::ExprId::NONE, |expr| self.expr(expr));
                b::VarDecl {
                    pat,
                    ty,
                    init,
                    kind: var_kind(kind),
                    flags,
                    loc: Self::loc(decl.span),
                }
            })
            .collect();
        self.file.add_var_decls(&vars)
    }

    pub(super) fn block(&mut self, statements: &[k::Statement], pos: u32) -> b::StmtId {
        let list = self.statements(statements);
        self.file.stmt(b::StmtKind::Block(list), pos)
    }

    pub(super) fn statement(&mut self, statement: &k::Statement) -> b::StmtId {
        if let Some(lowered) = self.import_equals_at(statement.span) {
            return lowered;
        }
        if let Some(lowered) = self.class_declaration_at(statement.span) {
            return lowered;
        }
        if let Some(lowered) = self.declaration_at(statement.span) {
            return lowered;
        }
        use k::StatementKind as S;
        let kind = match &statement.kind {
            S::Empty => b::StmtKind::Empty,
            S::Unsupported { feature } => {
                self.error(statement.span, feature.clone());
                b::StmtKind::Empty
            }
            S::Expression(expr) => b::StmtKind::Expr(self.expr(expr)),
            S::Variable { kind, declarations } => {
                let flags = self.namespace_export_flag(statement.span);
                let vars = self.variable_decls(*kind, declarations, flags);
                b::StmtKind::Var(vars)
            }
            S::Function(function) => {
                let func = self.function(function);
                self.file[func].kind = b::FnKind::Decl;
                let flags = self.namespace_export_flag(statement.span);
                self.file[func].flags |= flags;
                b::StmtKind::Fn(func)
            }
            S::Class(class) => b::StmtKind::Class(self.class(class)),
            S::Block(statements) => b::StmtKind::Block(self.statements(statements)),
            S::Return(expr) => b::StmtKind::Return(
                expr.as_ref()
                    .map_or(b::ExprId::NONE, |expr| self.expr(expr)),
            ),
            S::Throw(expr) => b::StmtKind::Throw(self.expr(expr)),
            S::If {
                test,
                consequent,
                alternate,
            } => {
                let test = self.expr(test);
                let yes = self.statement(consequent);
                let no = alternate
                    .as_ref()
                    .map_or(b::StmtId::NONE, |stmt| self.statement(stmt));
                b::StmtKind::If { test, yes, no }
            }
            S::While { test, body } => {
                let test = self.expr(test);
                let body = self.statement(body);
                b::StmtKind::While { test, body }
            }
            S::DoWhile { test, body } => {
                let body = self.statement(body);
                let test = self.expr(test);
                b::StmtKind::DoWhile { body, test }
            }
            S::For {
                init,
                test,
                update,
                body,
            } => {
                let init = init
                    .as_ref()
                    .map_or(b::StmtId::NONE, |stmt| self.statement(stmt));
                let test = test
                    .as_ref()
                    .map_or(b::ExprId::NONE, |expr| self.expr(expr));
                let update = update
                    .as_ref()
                    .map_or(b::ExprId::NONE, |expr| self.expr(expr));
                let body = self.statement(body);
                b::StmtKind::For {
                    init,
                    test,
                    update,
                    body,
                }
            }
            S::ForIn {
                kind,
                name,
                object,
                body,
            } => {
                let pat = self.file.pat(
                    b::PatKind::Ident(self.atom(name)),
                    statement.span.start,
                    statement.span.end,
                );
                let var = b::VarDecl {
                    pat,
                    ty: b::TypeNodeId::NONE,
                    init: b::ExprId::NONE,
                    kind: var_kind(*kind),
                    flags: b::Flags::empty(),
                    loc: Self::loc(statement.span),
                };
                let declarations = self.file.add_var_decls(&[var]);
                let left = self
                    .file
                    .stmt(b::StmtKind::Var(declarations), statement.span.start);
                let expr = self.expr(object);
                let body = self.statement(body);
                b::StmtKind::ForIn { left, expr, body }
            }
            S::ForOf {
                kind,
                pattern,
                iterable,
                body,
                awaiting,
            } => {
                let pat = self.pattern(pattern, statement.span);
                let var = b::VarDecl {
                    pat,
                    ty: b::TypeNodeId::NONE,
                    init: b::ExprId::NONE,
                    kind: var_kind(*kind),
                    flags: b::Flags::empty(),
                    loc: Self::loc(statement.span),
                };
                let declarations = self.file.add_var_decls(&[var]);
                let left = self
                    .file
                    .stmt(b::StmtKind::Var(declarations), statement.span.start);
                let expr = self.expr(iterable);
                let body = self.statement(body);
                b::StmtKind::ForOf {
                    left,
                    expr,
                    body,
                    is_await: *awaiting,
                }
            }
            S::Switch {
                discriminant,
                cases,
            } => {
                let expr = self.expr(discriminant);
                let cases: Vec<_> = cases
                    .iter()
                    .map(|case| b::Case {
                        test: case
                            .test
                            .as_ref()
                            .map_or(b::ExprId::NONE, |test| self.expr(test)),
                        body: self.statements(&case.body),
                        pos: case
                            .test
                            .as_ref()
                            .map_or(statement.span.start, |test| test.span.start),
                        end: case.body.last().map_or(statement.span.end, |s| s.span.end),
                    })
                    .collect();
                b::StmtKind::Switch {
                    expr,
                    cases: self.file.add_cases(&cases),
                }
            }
            S::With { object, body } => {
                let object_expr = self.expr(object);
                self.file.with_bodies.push((body.span.start, body.span.end));
                let object_stmt = self
                    .file
                    .stmt(b::StmtKind::Expr(object_expr), object.span.start);
                let body_stmt = self.statement(body);
                b::StmtKind::Block(self.file.list(&[object_stmt, body_stmt]))
            }
            S::Try {
                block,
                handler,
                finalizer,
            } => {
                let block = self.block(block, statement.span.start);
                let (param, handler) = if let Some(handler) = handler {
                    let param = handler.binding.as_ref().map_or(b::VarDeclId::NONE, |name| {
                        let pat = self.file.pat(
                            b::PatKind::Ident(self.atom(name)),
                            statement.span.start,
                            statement.span.end,
                        );
                        self.file.add_var_decl(b::VarDecl {
                            pat,
                            ty: b::TypeNodeId::NONE,
                            init: b::ExprId::NONE,
                            kind: b::VarKind::Let,
                            flags: b::Flags::empty(),
                            loc: Self::loc(statement.span),
                        })
                    });
                    (param, self.block(&handler.body, statement.span.start))
                } else {
                    (b::VarDeclId::NONE, b::StmtId::NONE)
                };
                let finalizer = if finalizer.is_empty() {
                    b::StmtId::NONE
                } else {
                    self.block(finalizer, statement.span.start)
                };
                b::StmtKind::Try {
                    block,
                    param,
                    handler,
                    finalizer,
                }
            }
            S::Break { label } => {
                b::StmtKind::Break(label.as_deref().map_or(Atom::NONE, |name| self.atom(name)))
            }
            S::Continue { label } => {
                b::StmtKind::Continue(label.as_deref().map_or(Atom::NONE, |name| self.atom(name)))
            }
            S::Labelled { label, body } => {
                let body = self.statement(body);
                b::StmtKind::Labeled {
                    label: self.atom(label),
                    body,
                }
            }
            S::ExportDefault(expr) => b::StmtKind::ExportDefault(self.expr(expr)),
        };
        let id = self.file.stmt(kind, statement.span.start);
        self.file[id].loc = Self::loc(statement.span);
        id
    }

    pub(super) fn class_declaration_at(&mut self, span: k::Span) -> Option<b::StmtId> {
        let site = self
            .module
            .type_syntax
            .classes
            .iter()
            .find(|site| site.span == span && site.declared)?
            .clone();
        let name = site
            .name
            .as_deref()
            .map_or(Atom::NONE, |name| self.atom(name));
        let mut flags = b::Flags::AMBIENT;
        if site.exported {
            flags |= b::Flags::EXPORT;
        }
        if site.default_exported {
            flags |= b::Flags::DEFAULT;
        }
        if site.abstract_ {
            flags |= b::Flags::ABSTRACT;
        }
        let type_params = self.type_parameters(&site.type_parameters);
        let extends = site.extends.as_ref().map_or(b::ExprId::NONE, |heritage| {
            self.class_heritage_expr(heritage)
        });
        let extends_args = site
            .extends
            .as_ref()
            .and_then(|heritage| match &heritage.kind {
                k::TypeExprKind::Reference { arguments, .. } => Some(arguments.as_slice()),
                _ => None,
            })
            .map_or(b::IdList::EMPTY, |args| self.ty_list(args));
        let implements = self.ty_list(&site.implements);
        let mut members = Vec::new();
        for member in &site.members {
            let key = self.class_site_key(member);
            match member.kind {
                k::TypeClassMemberKind::Field | k::TypeClassMemberKind::Accessor => {
                    if member.name.is_none() && member.key.is_none() {
                        continue;
                    }
                    let annotation = self
                        .module
                        .type_syntax
                        .annotations
                        .iter()
                        .find(|annotation| {
                            annotation.owner_span == member.span
                                && matches!(annotation.site, k::TypeSite::ClassField { .. })
                        })
                        .cloned();
                    let ty = annotation
                        .as_ref()
                        .map_or(b::TypeNodeId::NONE, |annotation| self.ty(&annotation.ty));
                    let mut flags = class_member_flags(Some(member), false);
                    if member.kind == k::TypeClassMemberKind::Accessor {
                        flags |= b::Flags::ACCESSOR;
                    }
                    members.push(b::Member {
                        kind: b::MemberKind::Property,
                        key,
                        flags,
                        ty,
                        init: b::ExprId::NONE,
                        func: b::FnId::NONE,
                        name_pos: member.span.start,
                        start: member.span.start,
                        loc: Self::loc(member.span),
                        modifiers: b::Span::EMPTY,
                    });
                }
                k::TypeClassMemberKind::Method
                | k::TypeClassMemberKind::Getter
                | k::TypeClassMemberKind::Setter
                | k::TypeClassMemberKind::Constructor => {
                    let signature = self
                        .module
                        .type_syntax
                        .signatures
                        .iter()
                        .find(|signature| {
                            signature.owner_span.start >= member.span.start
                                && signature.owner_span.end <= member.span.end
                        })
                        .map(|signature| signature.signature.clone());
                    let Some(signature) = signature else { continue };
                    let (member_kind, fn_kind) = match member.kind {
                        k::TypeClassMemberKind::Getter => {
                            (b::MemberKind::Getter, b::FnKind::Getter)
                        }
                        k::TypeClassMemberKind::Setter => {
                            (b::MemberKind::Setter, b::FnKind::Setter)
                        }
                        k::TypeClassMemberKind::Constructor => {
                            (b::MemberKind::Constructor, b::FnKind::Constructor)
                        }
                        _ => (b::MemberKind::Method, b::FnKind::Method),
                    };
                    let func = self.signature(&signature, fn_kind);
                    let member_name = member
                        .name
                        .as_deref()
                        .map_or(Atom::NONE, |name| self.atom(name));
                    self.file[func].name = member_name;
                    self.file[func].name_pos = member.span.start;
                    self.file[func].anchor = member.span.start;
                    self.file[func].start = member.span.start;
                    members.push(b::Member {
                        kind: member_kind,
                        key: if member_kind == b::MemberKind::Constructor {
                            b::PropKey::None
                        } else {
                            key
                        },
                        flags: class_member_flags(Some(member), false),
                        ty: b::TypeNodeId::NONE,
                        init: b::ExprId::NONE,
                        func,
                        name_pos: member.span.start,
                        start: member.span.start,
                        loc: Self::loc(member.span),
                        modifiers: b::Span::EMPTY,
                    });
                }
                k::TypeClassMemberKind::StaticBlock => {}
                k::TypeClassMemberKind::Index => {
                    if let Some(index) = self.class_index_member(member) {
                        members.push(index);
                    }
                }
                k::TypeClassMemberKind::ParameterProperty => {
                    let annotation = self
                        .module
                        .type_syntax
                        .annotations
                        .iter()
                        .find(|annotation| annotation.owner_span == member.span)
                        .cloned();
                    let ty = annotation
                        .as_ref()
                        .map_or(b::TypeNodeId::NONE, |annotation| self.ty(&annotation.ty));
                    members.push(b::Member {
                        kind: b::MemberKind::Property,
                        key,
                        flags: class_member_flags(Some(member), false),
                        ty,
                        init: b::ExprId::NONE,
                        func: b::FnId::NONE,
                        name_pos: member.span.start,
                        start: member.span.start,
                        loc: Self::loc(member.span),
                        modifiers: b::Span::EMPTY,
                    });
                }
            }
        }
        members.sort_by_key(|member| member.start);
        let members = self.file.add_members(&members);
        let class_modifiers = self.declaration_modifiers(flags, span.start);
        let class = self.file.add_class(b::Class {
            name,
            name_pos: site.name_pos,
            flags,
            type_params,
            extends,
            extends_args,
            other_extends: b::IdList::EMPTY,
            implements,
            other_implements: b::IdList::EMPTY,
            members,
            start: span.start,
            modifiers: class_modifiers,
        });
        let stmt = self.file.stmt(b::StmtKind::Class(class), span.start);
        self.file[stmt].modifiers = class_modifiers;
        self.file[stmt].loc = Self::loc(span);
        Some(stmt)
    }

    fn class_heritage_expr(&mut self, heritage: &k::TypeExpr) -> b::ExprId {
        let k::TypeExprKind::Reference { name, .. } = &heritage.kind else {
            return self
                .file
                .expr(b::ExprKind::Missing, heritage.span.start, heritage.span.end);
        };
        let mut names = name.iter();
        let Some(first) = names.next() else {
            return self
                .file
                .expr(b::ExprKind::Missing, heritage.span.start, heritage.span.end);
        };
        let mut pos = heritage.span.start;
        let first_atom = self.atom(first);
        let mut current = self.file.expr(
            b::ExprKind::Ident(first_atom),
            pos,
            pos + first.len() as u32,
        );
        pos += first.len() as u32;
        for name in names {
            let next_pos = self.module.source[pos as usize..heritage.span.end as usize]
                .find(name)
                .map_or(pos, |offset| pos + offset as u32);
            let atom = self.atom(name);
            current = self.file.expr(
                b::ExprKind::Dot {
                    obj: current,
                    name: atom,
                    name_pos: next_pos,
                    chain: b::Chain::No,
                },
                heritage.span.start,
                next_pos + name.len() as u32,
            );
            pos = next_pos + name.len() as u32;
        }
        current
    }

    fn class_site_key(&mut self, member: &k::TypeClassMemberSite) -> b::PropKey {
        if let Some(key) = &member.key {
            return self.key(key);
        }
        let Some(name) = member.name.as_deref() else {
            return b::PropKey::None;
        };
        if name.starts_with('#') {
            b::PropKey::Private(self.atom(name.trim_start_matches('#')))
        } else {
            b::PropKey::Name(self.atom(name))
        }
    }

    fn class_index_member(&mut self, member: &k::TypeClassMemberSite) -> Option<b::Member> {
        let (Some(key_type), Some(value_type)) = (&member.key_type, &member.value_type) else {
            return None;
        };
        let param_name = self.atom(member.name.as_deref().unwrap_or("key"));
        let pat = self.file.pat(
            b::PatKind::Ident(param_name),
            member.span.start,
            member.span.end,
        );
        let key_ty = self.ty(key_type);
        let param = self.file.add_params(&[b::Param {
            pat,
            ty: key_ty,
            default: b::ExprId::NONE,
            flags: b::Flags::empty(),
            pos: member.span.start,
            loc: Self::loc(member.span),
        }]);
        let ret = self.ty(value_type);
        let func = self.file.add_fn(b::Func {
            kind: b::FnKind::IndexSignature,
            flags: b::Flags::empty(),
            name: Atom::NONE,
            name_pos: member.span.start,
            type_params: b::Span::EMPTY,
            params: param,
            this_param: b::ParamId::NONE,
            ret,
            body: b::FnBody::None,
            anchor: member.span.start,
            start: member.span.start,
        });
        Some(b::Member {
            kind: b::MemberKind::IndexSignature,
            key: b::PropKey::None,
            flags: if member.readonly {
                b::Flags::READONLY
            } else {
                b::Flags::empty()
            },
            ty: ret,
            init: b::ExprId::NONE,
            func,
            name_pos: member.span.start,
            start: member.span.start,
            loc: Self::loc(member.span),
            modifiers: b::Span::EMPTY,
        })
    }

    fn class(&mut self, class: &k::Class) -> b::ClassId {
        let name = class.name.as_deref().map_or(Atom::NONE, |n| self.atom(n));
        let site = self
            .module
            .type_syntax
            .classes
            .iter()
            .find(|site| site.span == class.span)
            .cloned();
        let class_flags = site.as_ref().map_or_else(b::Flags::empty, |site| {
            let mut flags = b::Flags::empty();
            if site.exported {
                flags |= b::Flags::EXPORT;
            }
            if site.default_exported {
                flags |= b::Flags::DEFAULT;
            }
            if site.abstract_ {
                flags |= b::Flags::ABSTRACT;
            }
            if site.declared {
                flags |= b::Flags::AMBIENT;
            }
            flags
        });
        let type_params = site.as_ref().map_or(b::Span::EMPTY, |site| {
            self.type_parameters(&site.type_parameters)
        });
        let extends = class
            .superclass
            .as_ref()
            .map_or(b::ExprId::NONE, |expr| self.expr(expr));
        let extends_args = site
            .as_ref()
            .and_then(|site| site.extends.as_ref())
            .and_then(|ty| match &ty.kind {
                k::TypeExprKind::Reference { arguments, .. } => Some(arguments.as_slice()),
                _ => None,
            })
            .map_or(b::IdList::EMPTY, |args| self.ty_list(args));
        let implements = site
            .as_ref()
            .map_or(b::IdList::EMPTY, |site| self.ty_list(&site.implements));
        let mut members = Vec::new();
        if let Some(constructor) = &class.constructor {
            let member_site = site.as_ref().and_then(|site| {
                site.members.iter().find(|member| {
                    member.kind == k::TypeClassMemberKind::Constructor
                        && member.span.start <= constructor.span.start
                        && member.span.end >= constructor.span.end
                })
            });
            let func = self.function(constructor);
            self.file[func].kind = b::FnKind::Constructor;
            members.push(b::Member {
                kind: b::MemberKind::Constructor,
                key: b::PropKey::None,
                flags: class_member_flags(member_site, false),
                ty: b::TypeNodeId::NONE,
                init: b::ExprId::NONE,
                func,
                name_pos: constructor.span.start,
                start: constructor.span.start,
                loc: Self::loc(constructor.span),
                modifiers: b::Span::EMPTY,
            });
        }
        if let Some(site) = &site {
            for property in site
                .members
                .iter()
                .filter(|member| member.kind == k::TypeClassMemberKind::ParameterProperty)
            {
                let annotation = self
                    .module
                    .type_syntax
                    .annotations
                    .iter()
                    .find(|annotation| annotation.owner_span == property.span)
                    .cloned();
                let ty = annotation
                    .as_ref()
                    .map_or(b::TypeNodeId::NONE, |annotation| self.ty(&annotation.ty));
                members.push(b::Member {
                    kind: b::MemberKind::Property,
                    key: self.class_site_key(property),
                    flags: class_member_flags(Some(property), false),
                    ty,
                    init: b::ExprId::NONE,
                    func: b::FnId::NONE,
                    name_pos: property.span.start,
                    start: property.span.start,
                    loc: Self::loc(property.span),
                    modifiers: b::Span::EMPTY,
                });
            }
        }
        for method in &class.methods {
            let member_site = site.as_ref().and_then(|site| {
                site.members
                    .iter()
                    .find(|member| member.span == method.span)
            });
            let func = self.function(&method.function);
            self.file[func].kind = match method.kind {
                k::MethodKind::Normal => b::FnKind::Method,
                k::MethodKind::Get => b::FnKind::Getter,
                k::MethodKind::Set => b::FnKind::Setter,
            };
            let key = self.key(&method.key);
            members.push(b::Member {
                kind: match method.kind {
                    k::MethodKind::Normal => b::MemberKind::Method,
                    k::MethodKind::Get => b::MemberKind::Getter,
                    k::MethodKind::Set => b::MemberKind::Setter,
                },
                key,
                flags: class_member_flags(member_site, method.is_static),
                ty: b::TypeNodeId::NONE,
                init: b::ExprId::NONE,
                func,
                name_pos: method.span.start,
                start: method.span.start,
                loc: Self::loc(method.span),
                modifiers: b::Span::EMPTY,
            });
        }
        if let Some(site) = &site {
            for member in site.members.iter().filter(|member| {
                matches!(
                    member.kind,
                    k::TypeClassMemberKind::Method
                        | k::TypeClassMemberKind::Getter
                        | k::TypeClassMemberKind::Setter
                        | k::TypeClassMemberKind::Constructor
                )
            }) {
                let already_lowered = match member.kind {
                    k::TypeClassMemberKind::Constructor => {
                        class.constructor.as_ref().is_some_and(|constructor| {
                            member.span.start <= constructor.span.start
                                && member.span.end >= constructor.span.end
                        })
                    }
                    _ => class
                        .methods
                        .iter()
                        .any(|method| method.span == member.span),
                };
                if already_lowered {
                    continue;
                }
                let signature = self
                    .module
                    .type_syntax
                    .signatures
                    .iter()
                    .find(|signature| {
                        signature.owner_span.start >= member.span.start
                            && signature.owner_span.end <= member.span.end
                            && !signature.has_body
                    })
                    .map(|signature| signature.signature.clone());
                let Some(signature) = signature else { continue };
                let (member_kind, fn_kind) = match member.kind {
                    k::TypeClassMemberKind::Getter => (b::MemberKind::Getter, b::FnKind::Getter),
                    k::TypeClassMemberKind::Setter => (b::MemberKind::Setter, b::FnKind::Setter),
                    k::TypeClassMemberKind::Constructor => {
                        (b::MemberKind::Constructor, b::FnKind::Constructor)
                    }
                    _ => (b::MemberKind::Method, b::FnKind::Method),
                };
                let func = self.signature(&signature, fn_kind);
                let member_name = member
                    .name
                    .as_deref()
                    .map_or(Atom::NONE, |name| self.atom(name));
                self.file[func].name = member_name;
                self.file[func].name_pos = member.span.start;
                self.file[func].anchor = member.span.start;
                self.file[func].start = member.span.start;
                members.push(b::Member {
                    kind: member_kind,
                    key: if member_kind == b::MemberKind::Constructor {
                        b::PropKey::None
                    } else {
                        self.class_site_key(member)
                    },
                    flags: class_member_flags(Some(member), false),
                    ty: b::TypeNodeId::NONE,
                    init: b::ExprId::NONE,
                    func,
                    name_pos: member.span.start,
                    start: member.span.start,
                    loc: Self::loc(member.span),
                    modifiers: b::Span::EMPTY,
                });
            }
        }
        for init in &class.initialisers {
            match init {
                k::ClassInitialiser::Field(field) => {
                    let key = self.key(&field.key);
                    let field_annotation = self
                        .module
                        .type_syntax
                        .annotations
                        .iter()
                        .find(|site| {
                            site.owner_span == field.span
                                && matches!(&site.site, k::TypeSite::ClassField { .. })
                        })
                        .cloned();
                    let member_site = site.as_ref().and_then(|site| {
                        site.members.iter().find(|member| member.span == field.span)
                    });
                    let ty = field_annotation
                        .as_ref()
                        .map_or(b::TypeNodeId::NONE, |site| self.ty(&site.ty));
                    let value = field
                        .value
                        .as_ref()
                        .map_or(b::ExprId::NONE, |expr| self.expr(expr));
                    members.push(b::Member {
                        kind: b::MemberKind::Property,
                        key,
                        flags: class_member_flags(member_site, field.is_static),
                        ty,
                        init: value,
                        func: b::FnId::NONE,
                        name_pos: field.span.start,
                        start: field.span.start,
                        loc: Self::loc(field.span),
                        modifiers: b::Span::EMPTY,
                    });
                }
                k::ClassInitialiser::StaticBlock(function) => {
                    let func = self.function(function);
                    self.file[func].kind = b::FnKind::StaticBlock;
                    members.push(b::Member {
                        kind: b::MemberKind::StaticBlock,
                        key: b::PropKey::None,
                        flags: b::Flags::empty(),
                        ty: b::TypeNodeId::NONE,
                        init: b::ExprId::NONE,
                        func,
                        name_pos: function.span.start,
                        start: function.span.start,
                        loc: Self::loc(function.span),
                        modifiers: b::Span::EMPTY,
                    });
                }
            }
        }
        if let Some(site) = &site {
            for index in site
                .members
                .iter()
                .filter(|member| member.kind == k::TypeClassMemberKind::Index)
            {
                if let Some(index) = self.class_index_member(index) {
                    members.push(index);
                }
            }
        }
        let mut members = members;
        members.sort_by_key(|member| member.start);
        let members = self.file.add_members(&members);
        self.file.add_class(b::Class {
            name,
            name_pos: site.as_ref().map_or(class.span.start, |site| site.name_pos),
            flags: class_flags,
            type_params,
            extends,
            extends_args,
            other_extends: b::IdList::EMPTY,
            implements,
            other_implements: b::IdList::EMPTY,
            members,
            start: class.span.start,
            modifiers: b::Span::EMPTY,
        })
    }
}

fn class_member_flags(site: Option<&k::TypeClassMemberSite>, static_fallback: bool) -> b::Flags {
    let mut flags = b::Flags::empty();
    if site.map_or(static_fallback, |site| site.static_) {
        flags |= b::Flags::STATIC;
    }
    if let Some(site) = site {
        if site.optional {
            flags |= b::Flags::OPTIONAL;
        }
        if site.readonly {
            flags |= b::Flags::READONLY;
        }
        if site.abstract_ {
            flags |= b::Flags::ABSTRACT;
        }
        if site.override_ {
            flags |= b::Flags::OVERRIDE;
        }
        match site.accessibility {
            Some(k::TypeAccessibility::Private) => flags |= b::Flags::PRIVATE,
            Some(k::TypeAccessibility::Protected) => flags |= b::Flags::PROTECTED,
            Some(k::TypeAccessibility::Public) => flags |= b::Flags::PUBLIC,
            None => {}
        }
    }
    flags
}

fn var_kind(kind: k::VariableKind) -> b::VarKind {
    match kind {
        k::VariableKind::Var => b::VarKind::Var,
        k::VariableKind::Let => b::VarKind::Let,
        k::VariableKind::Const => b::VarKind::Const,
        k::VariableKind::Using => b::VarKind::Using,
        k::VariableKind::AwaitUsing => b::VarKind::AwaitUsing,
    }
}

fn bin(op: k::BinaryOperator) -> b::BinOp {
    use b::BinOp as B;
    use k::BinaryOperator as K;
    match op {
        K::In => B::In,
        K::InstanceOf => B::Instanceof,
        K::Add => B::Add,
        K::Subtract => B::Sub,
        K::Multiply => B::Mul,
        K::Divide => B::Div,
        K::Remainder => B::Rem,
        K::Exponentiate => B::Pow,
        K::StrictEqual => B::EqEqEq,
        K::StrictNotEqual => B::NotEqEq,
        K::LooseEqual => B::EqEq,
        K::LooseNotEqual => B::NotEq,
        K::BitwiseAnd => B::BitAnd,
        K::BitwiseOr => B::BitOr,
        K::BitwiseXor => B::BitXor,
        K::ShiftLeft => B::Shl,
        K::ShiftRight => B::Shr,
        K::UnsignedShiftRight => B::UShr,
        K::LessThan => B::Lt,
        K::LessThanOrEqual => B::Le,
        K::GreaterThan => B::Gt,
        K::GreaterThanOrEqual => B::Ge,
    }
}

fn assign(op: k::AssignmentOperator) -> Option<b::BinOp> {
    use b::BinOp as B;
    use k::AssignmentOperator as K;
    Some(match op {
        K::Assign => return None,
        K::Add => B::Add,
        K::Subtract => B::Sub,
        K::Multiply => B::Mul,
        K::Divide => B::Div,
        K::Remainder => B::Rem,
        K::Exponentiate => B::Pow,
        K::BitwiseAnd => B::BitAnd,
        K::BitwiseOr => B::BitOr,
        K::BitwiseXor => B::BitXor,
        K::ShiftLeft => B::Shl,
        K::ShiftRight => B::Shr,
        K::UnsignedShiftRight => B::UShr,
        K::LogicalAnd => B::And,
        K::LogicalOr => B::Or,
        K::Nullish => B::Nullish,
    })
}

/// Position of a private identifier's `#`, used by the engine to distinguish it from `"#x"`.
fn private_key_pos(source: &[u8], start: u32, end: u32, name: &str) -> u32 {
    let needle = [b"#".as_slice(), name.as_bytes()].concat();
    let start = start as usize;
    let end = (end as usize).min(source.len());
    if start <= end {
        if let Some(offset) = source[start..end]
            .windows(needle.len())
            .position(|window| window == needle)
        {
            return (start + offset) as u32;
        }
    }
    end.saturating_sub(needle.len()) as u32
}
