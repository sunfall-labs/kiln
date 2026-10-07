//! Lower owned source type syntax into the checker’s arena representation.
use crate::{
    engine::{atom::Atom, hir as b, session::Session},
    lower::Lower,
};
use kiln_hir as k;

impl<'a, 's> Lower<'a, 's> {
    pub(super) fn function_type_parts(
        &mut self,
        function: &k::Function,
    ) -> (
        b::Span<b::TypeParamId>,
        b::Span<b::ParamId>,
        b::TypeNodeId,
        bool,
    ) {
        let signature = self
            .module
            .type_syntax
            .signatures
            .iter()
            .find(|site| site.owner_span == function.span)
            .map(|site| site.clone());
        let (type_params, signature) = if let Some(site) = signature {
            (
                self.type_parameters(&site.signature.type_parameters),
                Some(site),
            )
        } else {
            (b::Span::EMPTY, None)
        };
        let mut params = Vec::new();
        if let Some(parameter) = signature
            .as_ref()
            .and_then(|site| site.signature.this_parameter.as_ref())
        {
            params.push(self.signature_parameter(parameter));
        }
        for (index, parameter) in function.parameters.iter().enumerate() {
            let pat = self.pattern(&parameter.pattern, parameter.span);
            let parameter_ty = signature
                .as_ref()
                .and_then(|site| site.signature.parameters.get(index))
                .and_then(|p| p.ty.as_ref())
                .map(|t| self.ty(t))
                .unwrap_or_else(|| {
                    self.annotation(parameter.span, |site| {
                        matches!(site, k::TypeSite::Parameter { .. })
                    })
                });
            let default = match &parameter.pattern {
                k::Pattern::WithDefault { default, .. } => self.expr(default),
                _ => b::ExprId::NONE,
            };
            let flags = signature
                .as_ref()
                .and_then(|site| site.signature.parameters.get(index))
                .map_or(b::Flags::empty(), |p| {
                    let mut flags = b::Flags::empty();
                    if p.optional {
                        flags |= b::Flags::OPTIONAL
                    }
                    if p.rest {
                        flags |= b::Flags::REST
                    }
                    flags
                });
            params.push(b::Param {
                pat,
                ty: parameter_ty,
                default,
                flags,
                pos: parameter.span.start,
                loc: Self::loc(parameter.span),
            });
        }
        if let Some(rest) = &function.rest {
            let span = signature
                .as_ref()
                .and_then(|site| {
                    site.signature
                        .parameters
                        .iter()
                        .find(|parameter| parameter.rest)
                })
                .map_or(function.span, |parameter| parameter.span);
            let pat = self.pattern(rest, span);
            let rest_ty = signature
                .as_ref()
                .and_then(|site| site.signature.parameters.iter().find(|p| p.rest))
                .and_then(|p| p.ty.as_ref())
                .map(|t| self.ty(t))
                .unwrap_or(b::TypeNodeId::NONE);
            params.push(b::Param {
                pat,
                ty: rest_ty,
                default: b::ExprId::NONE,
                flags: b::Flags::REST,
                pos: span.start,
                loc: Self::loc(span),
            });
        }
        let params = self.file.add_params(&params);
        let ret = signature
            .as_ref()
            .and_then(|site| site.signature.return_type.as_ref())
            .map(|t| self.ty(t))
            .unwrap_or_else(|| {
                self.annotation(function.span, |site| matches!(site, k::TypeSite::Return))
            });
        let has_body = signature.as_ref().is_none_or(|site| site.has_body);
        (type_params, params, ret, has_body)
    }

    pub(super) fn call_type_arguments(&mut self, span: k::Span) -> b::IdList<b::TypeNodeId> {
        let args = self
            .module
            .type_syntax
            .instantiations
            .iter()
            .find(|site| site.owner_span == span)
            .map(|site| site.arguments.clone())
            .unwrap_or_default();
        self.ty_list(&args)
    }

    pub(super) fn declaration_at(&mut self, span: k::Span) -> Option<b::StmtId> {
        let declaration = self
            .module
            .type_syntax
            .declarations
            .iter()
            .find(|d| d.span == span && !matches!(d.kind, k::TypeDeclarationKind::Import))
            .cloned()?;
        if matches!(declaration.kind, k::TypeDeclarationKind::Class) {
            return self.class_declaration_at(span);
        }
        Some(self.declaration(&declaration))
    }

    pub(super) fn declaration(&mut self, declaration: &k::TypeDeclaration) -> b::StmtId {
        if matches!(declaration.kind, k::TypeDeclarationKind::Class) {
            if let Some(statement) = self.class_declaration_at(declaration.span) {
                return statement;
            }
            self.error(declaration.span, "missing ambient class metadata");
            return self.file.stmt(b::StmtKind::Empty, declaration.span.start);
        }
        if matches!(declaration.kind, k::TypeDeclarationKind::Variable) {
            if let Some(statement) = declaration.body.first() {
                if let k::StatementKind::Variable { kind, declarations } = &statement.kind {
                    let mut flags = b::Flags::empty();
                    if declaration.declared {
                        flags |= b::Flags::AMBIENT;
                    }
                    if declaration.exported {
                        flags |= b::Flags::EXPORT;
                    }
                    if declaration.default_exported {
                        flags |= b::Flags::DEFAULT;
                    }
                    if declaration.const_ {
                        flags |= b::Flags::CONST;
                    }
                    let vars = self.variable_decls(*kind, declarations, flags);
                    let stmt = self
                        .file
                        .stmt(b::StmtKind::Var(vars), declaration.span.start);
                    self.file[stmt].loc = Self::loc(declaration.span);
                    return stmt;
                }
            }
            self.error(declaration.span, "missing ambient variable declaration");
            return self.file.stmt(b::StmtKind::Empty, declaration.span.start);
        }
        let name = declaration
            .name
            .as_deref()
            .map_or(Atom::NONE, |name| self.atom(name));
        let mut flags = b::Flags::empty();
        if declaration.declared {
            flags |= b::Flags::AMBIENT;
        }
        if declaration.exported {
            flags |= b::Flags::EXPORT;
        }
        if declaration.default_exported {
            flags |= b::Flags::DEFAULT;
        }
        if declaration.const_ {
            flags |= b::Flags::CONST;
        }
        let kind = match &declaration.kind {
            k::TypeDeclarationKind::Alias(ty) => {
                let type_params = self.type_parameters(&declaration.type_parameters);
                let ty = self.ty(ty);
                b::StmtKind::TypeAlias(self.file.add_alias(b::Alias {
                    name,
                    name_pos: declaration.span.start,
                    flags,
                    type_params,
                    ty,
                    stmt: b::StmtId::NONE,
                }))
            }
            k::TypeDeclarationKind::Interface => {
                let type_params = self.type_parameters(&declaration.type_parameters);
                let extends = self.ty_list(&declaration.extends);
                let members = self.members(&declaration.members);
                b::StmtKind::Interface(self.file.add_interface(b::Interface {
                    name,
                    name_pos: declaration.span.start,
                    flags,
                    type_params,
                    extends,
                    other_heritage: b::IdList::EMPTY,
                    members,
                    stmt: b::StmtId::NONE,
                }))
            }
            k::TypeDeclarationKind::Enum => {
                let members: Vec<_> = declaration
                    .enum_members
                    .iter()
                    .map(|member| b::EnumMember {
                        name: self.atom_units(&member.name),
                        name_kind: match member.name_kind {
                            k::EnumMemberNameKind::Identifier => b::NameKind::Identifier,
                            k::EnumMemberNameKind::StringLiteral => b::NameKind::StringLiteral,
                            k::EnumMemberNameKind::ComputedString => b::NameKind::ComputedString,
                        },
                        computed_name: b::ExprId::NONE,
                        init: member
                            .initializer
                            .as_ref()
                            .map_or(b::ExprId::NONE, |e| self.expr(e)),
                        pos: member.span.start,
                        loc: Self::loc(member.span),
                    })
                    .collect();
                let members = self.file.add_enum_members(&members);
                b::StmtKind::Enum(self.file.add_enum(b::Enum {
                    name,
                    name_pos: declaration.span.start,
                    flags,
                    members,
                    stmt: b::StmtId::NONE,
                }))
            }
            k::TypeDeclarationKind::Namespace => {
                let mut body = self.statements(&declaration.body);
                if declaration.body.is_empty() {
                    let child_declarations = self
                        .module
                        .type_syntax
                        .declarations
                        .iter()
                        .filter(|child| {
                            child.span != declaration.span
                                && child.span.start >= declaration.span.start
                                && child.span.end <= declaration.span.end
                        })
                        .cloned()
                        .collect::<Vec<_>>();
                    let mut children: Vec<_> = child_declarations
                        .iter()
                        .filter(|child| {
                            !child_declarations.iter().any(|middle| {
                                middle.span != child.span
                                    && middle.span.start >= declaration.span.start
                                    && middle.span.end <= declaration.span.end
                                    && middle.span.start <= child.span.start
                                    && middle.span.end >= child.span.end
                            })
                        })
                        .map(|child| (child.span.start, self.declaration(child)))
                        .collect();
                    children.sort_by_key(|(pos, _)| *pos);
                    body = self.file.list(
                        &children
                            .into_iter()
                            .map(|(_, stmt)| stmt)
                            .collect::<Vec<_>>(),
                    );
                }
                let module_name = if declaration.global || name.is_none() {
                    b::ModuleName::Global
                } else {
                    b::ModuleName::Ident(name)
                };
                let module = self.file.add_module(b::Module {
                    name: module_name,
                    name_pos: declaration.span.start,
                    flags,
                    body,
                    has_body: true,
                    specifies_module: declaration.source_text.trim_start().starts_with("module "),
                    stmt: b::StmtId::NONE,
                });
                b::StmtKind::Module(module)
            }
            k::TypeDeclarationKind::Module => {
                let body = if declaration.body.is_empty() {
                    b::IdList::EMPTY
                } else {
                    self.statements(&declaration.body)
                };
                let module_name = if declaration.global {
                    b::ModuleName::Global
                } else {
                    let text = declaration.name.as_deref().unwrap_or("");
                    if text.starts_with('"') || text.starts_with('\'') {
                        b::ModuleName::String(self.atom(text.trim_matches(['"', '\''])))
                    } else {
                        b::ModuleName::Ident(self.atom(text))
                    }
                };
                let module = self.file.add_module(b::Module {
                    name: module_name,
                    name_pos: declaration.span.start,
                    flags,
                    body,
                    has_body: !declaration.body.is_empty(),
                    specifies_module: true,
                    stmt: b::StmtId::NONE,
                });
                b::StmtKind::Module(module)
            }
            k::TypeDeclarationKind::Overload => {
                let Some(site) = self
                    .module
                    .type_syntax
                    .signatures
                    .iter()
                    .find(|site| site.owner_span == declaration.span)
                    .cloned()
                else {
                    self.error(declaration.span, "missing overload signature");
                    return self.file.stmt(b::StmtKind::Empty, declaration.span.start);
                };
                let function = self.signature(&site.signature, b::FnKind::FunctionType);
                self.file[function].kind = b::FnKind::Decl;
                self.file[function].name = name;
                self.file[function].name_pos = declaration.span.start;
                b::StmtKind::Fn(function)
            }
            _ => {
                self.error(declaration.span, "unsupported type declaration");
                return self.file.stmt(b::StmtKind::Empty, declaration.span.start);
            }
        };
        let stmt = self.file.stmt(kind, declaration.span.start);
        self.file[stmt].loc = Self::loc(declaration.span);
        stmt
    }
    pub(super) fn ty_list(&mut self, types: &[k::TypeExpr]) -> b::IdList<b::TypeNodeId> {
        let ids: Vec<_> = types.iter().map(|t| self.ty(t)).collect();
        self.file.list(&ids)
    }
    pub(super) fn optional_ty(&mut self, t: Option<&k::TypeExpr>) -> b::TypeNodeId {
        t.map_or(b::TypeNodeId::NONE, |t| self.ty(t))
    }
    pub(super) fn type_parameter(&mut self, t: &k::TypeParameter) -> b::TypeParam {
        let constraint = self.optional_ty(t.constraint.as_deref());
        let default = self.optional_ty(t.default.as_deref());
        let mut flags = b::Flags::empty();
        if t.const_ {
            flags |= b::Flags::CONST
        }
        if t.in_ {
            flags |= b::Flags::IN
        }
        if t.out {
            flags |= b::Flags::OUT
        }
        b::TypeParam {
            name: self.atom(&t.name),
            pos: t.span.start,
            start: t.span.start,
            end: t.span.end,
            constraint,
            default,
            flags,
            modifiers: b::Span::EMPTY,
        }
    }
    pub(super) fn type_parameters(
        &mut self,
        params: &[k::TypeParameter],
    ) -> b::Span<b::TypeParamId> {
        let params: Vec<_> = params.iter().map(|p| self.type_parameter(p)).collect();
        self.file.add_type_params(&params)
    }
    pub(super) fn ty(&mut self, t: &k::TypeExpr) -> b::TypeNodeId {
        use k::TypeExprKind as T;
        let kind = match &t.kind {
            T::Keyword(kw) => b::TypeNodeKind::Keyword(match kw {
                k::TypeKeyword::Any => b::Keyword::Any,
                k::TypeKeyword::Unknown => b::Keyword::Unknown,
                k::TypeKeyword::Never => b::Keyword::Never,
                k::TypeKeyword::Void => b::Keyword::Void,
                k::TypeKeyword::Undefined => b::Keyword::Undefined,
                k::TypeKeyword::Null => b::Keyword::Null,
                k::TypeKeyword::Boolean => b::Keyword::Boolean,
                k::TypeKeyword::Number => b::Keyword::Number,
                k::TypeKeyword::BigInt => b::Keyword::BigInt,
                k::TypeKeyword::String => b::Keyword::String,
                k::TypeKeyword::Symbol => b::Keyword::Symbol,
                k::TypeKeyword::Object => b::Keyword::Object,
                k::TypeKeyword::Intrinsic => b::Keyword::Intrinsic,
                k::TypeKeyword::This => b::Keyword::This,
            }),
            T::Reference { name, arguments } => {
                let name = self.name(name, t.span.start);
                let args = self.ty_list(arguments);
                b::TypeNodeKind::Ref { name, args }
            }
            T::Union(types) => b::TypeNodeKind::Union(self.ty_list(types)),
            T::Intersection(types) => b::TypeNodeKind::Intersection(self.ty_list(types)),
            T::Array(ty) => b::TypeNodeKind::Array(self.ty(ty)),
            T::Literal(lit) => match lit {
                k::TypeLiteral::String(s) => b::TypeNodeKind::StringLit(self.atom_units(s)),
                k::TypeLiteral::Number(n) => match n.parse() {
                    Ok(n) => b::TypeNodeKind::NumberLit(self.file.number(n)),
                    Err(_) => {
                        self.error(t.span, "numeric type literal");
                        b::TypeNodeKind::Error
                    }
                },
                k::TypeLiteral::BigInt(n) => b::TypeNodeKind::BigIntLit {
                    text: self.atom(n),
                    negative: n.starts_with('-'),
                },
                k::TypeLiteral::Boolean(v) => b::TypeNodeKind::BoolLit(*v),
                k::TypeLiteral::Null => b::TypeNodeKind::Keyword(b::Keyword::Null),
            },
            T::Conditional {
                check,
                extends,
                when_true,
                when_false,
            } => {
                let check = self.ty(check);
                let extends = self.ty(extends);
                let yes = self.ty(when_true);
                let no = self.ty(when_false);
                b::TypeNodeKind::Cond {
                    check,
                    extends,
                    yes,
                    no,
                }
            }
            T::Keyof(ty) => b::TypeNodeKind::Keyof(self.ty(ty)),
            T::Operator { operator, operand } => {
                // The engine represents valid `unique symbol` syntax separately from
                // a `unique` operator with an invalid operand. Preserve that distinction
                // so readonly symbol members retain their declaration identity.
                if matches!(operator, k::TypeOperator::Unique)
                    && matches!(operand.kind, T::Keyword(k::TypeKeyword::Symbol))
                {
                    return self
                        .file
                        .ty(b::TypeNodeKind::UniqueSymbol, t.span.start, t.span.end);
                }
                let ty = self.ty(operand);
                match operator {
                    k::TypeOperator::Readonly => b::TypeNodeKind::Readonly(ty),
                    k::TypeOperator::Unique => b::TypeNodeKind::Unique(ty),
                    k::TypeOperator::Keyof => b::TypeNodeKind::Keyof(ty),
                }
            }
            T::Infer(param) => {
                let p = self.type_parameter(param);
                b::TypeNodeKind::Infer(self.file.add_type_param(p))
            }
            T::IndexedAccess { object, index } => {
                let obj = self.ty(object);
                let index = self.ty(index);
                b::TypeNodeKind::IndexedAccess { obj, index }
            }
            T::Parenthesized(ty) => return self.ty(ty),
            T::Typeof {
                name: names,
                arguments,
            } => {
                let name = self.name(names, t.span.start);
                let args = self.ty_list(arguments);
                let mut expr = b::ExprId::NONE;
                for n in names {
                    let atom = self.atom(n);
                    expr = if expr.is_none() {
                        self.file
                            .expr(b::ExprKind::Ident(atom), t.span.start, t.span.end)
                    } else {
                        self.file.expr(
                            b::ExprKind::Dot {
                                obj: expr,
                                name: atom,
                                name_pos: t.span.start,
                                chain: b::Chain::No,
                            },
                            t.span.start,
                            t.span.end,
                        )
                    }
                }
                b::TypeNodeKind::Typeof {
                    name,
                    args,
                    has_type_arguments: !arguments.is_empty(),
                    expr,
                }
            }
            T::Template { quasis, types } => {
                let types = self.ty_list(types);
                let texts: Vec<_> = quasis.iter().map(|s| self.atom_units(s)).collect();
                let texts = self.file.list(&texts);
                b::TypeNodeKind::Template { types, texts }
            }
            T::Import {
                specifier,
                qualifier,
                arguments,
                is_typeof,
            } => {
                let name = self.name(qualifier, t.span.start);
                let args = self.ty_list(arguments);
                b::TypeNodeKind::Import {
                    spec: self.atom(specifier),
                    name,
                    args,
                    is_typeof: *is_typeof,
                    mode: b::ResolutionMode::None,
                    attributes: b::ImportAttributesToken::None,
                }
            }
            T::Predicate {
                parameter,
                asserts,
                ty,
            } => {
                let ty = self.optional_ty(ty.as_deref());
                b::TypeNodeKind::Predicate {
                    param: self.atom(parameter),
                    asserts: *asserts,
                    ty,
                }
            }
            T::Tuple(elements) => {
                let elements: Vec<_> = elements
                    .iter()
                    .map(|e| {
                        let ty = self.ty(&e.ty);
                        b::TupleElem {
                            ty,
                            written: ty,
                            member_type: b::TupleMemberType::Plain,
                            name: e.name.as_deref().map_or(Atom::NONE, |n| self.atom(n)),
                            optional: e.optional,
                            rest: e.rest,
                            has_dots: e.rest,
                            start: e.ty.span.start,
                            end: e.ty.span.end,
                        }
                    })
                    .collect();
                b::TypeNodeKind::Tuple(self.file.add_tuple_elems(&elements))
            }
            T::Mapped {
                parameter,
                constraint,
                name_type,
                value,
                readonly,
                optional,
            } => {
                let mut param = self.type_parameter(parameter);
                param.constraint = self.ty(constraint);
                let param = self.file.add_type_param(param);
                let name_ty = self.optional_ty(name_type.as_deref());
                let ty = self.ty(value);
                let modifier = |m: &k::TypeModifier| match m {
                    k::TypeModifier::None => b::MappedModifier::None,
                    k::TypeModifier::Add => b::MappedModifier::Add,
                    k::TypeModifier::Remove => b::MappedModifier::Remove,
                };
                let mapped = self.file.add_mapped(b::Mapped {
                    param,
                    name_ty,
                    ty,
                    readonly: modifier(readonly),
                    optional: modifier(optional),
                    is_readonly_with_plus: false,
                    is_optional_with_plus: false,
                    members: b::Span::EMPTY,
                });
                b::TypeNodeKind::Mapped(mapped)
            }
            T::Object(members) => b::TypeNodeKind::Object(self.members(members)),
            T::Function(sig) => b::TypeNodeKind::Fn(self.signature(sig, b::FnKind::FunctionType)),
            T::Constructor(sig) => {
                b::TypeNodeKind::Fn(self.signature(sig, b::FnKind::ConstructorType))
            }
            T::Unsupported { syntax } | T::Raw { syntax } => {
                self.error(t.span, format!("type syntax: {syntax}"));
                b::TypeNodeKind::Error
            }
        };
        self.file.ty(kind, t.span.start, t.span.end)
    }

    fn signature_parameter(&mut self, p: &k::TypeSignatureParameter) -> b::Param {
        let pat = if let Some(pattern) = &p.pattern {
            self.pattern(pattern, p.pattern_span)
        } else if let Some(name) = p.name.as_deref() {
            let name_span = self.binding_span(name, p.span);
            let name = self.atom(name);
            self.file
                .pat(b::PatKind::Ident(name), name_span.start, name_span.end)
        } else {
            self.error(p.span, "signature parameter without a binding pattern");
            self.file.pat(b::PatKind::Missing, p.span.start, p.span.end)
        };
        let ty = p.ty.as_ref().map_or(b::TypeNodeId::NONE, |t| self.ty(t));
        let mut flags = b::Flags::empty();
        if p.optional {
            flags |= b::Flags::OPTIONAL;
        }
        if p.rest {
            flags |= b::Flags::REST;
        }
        b::Param {
            pat,
            ty,
            default: b::ExprId::NONE,
            flags,
            pos: p.span.start,
            loc: Self::loc(p.span),
        }
    }

    pub(super) fn signature(&mut self, sig: &k::TypeSignature, kind: b::FnKind) -> b::FnId {
        let type_params = self.type_parameters(&sig.type_parameters);
        let mut params = Vec::new();
        if let Some(parameter) = &sig.this_parameter {
            params.push(self.signature_parameter(parameter));
        }
        params.extend(sig.parameters.iter().map(|p| self.signature_parameter(p)));
        let params = self.file.add_params(&params);
        let (this_param, params) = self.file.split_this_parameter(params);
        let ret = sig
            .return_type
            .as_ref()
            .map_or(b::TypeNodeId::NONE, |t| self.ty(t));
        self.file.add_fn(b::Func {
            kind,
            flags: b::Flags::empty(),
            name: Atom::NONE,
            name_pos: sig.span.start,
            type_params,
            params,
            this_param,
            ret,
            body: b::FnBody::None,
            anchor: sig.span.start,
            start: sig.span.start,
        })
    }
    pub(super) fn members(&mut self, members: &[k::TypeMember]) -> b::Span<b::MemberId> {
        let members: Vec<_> = members
            .iter()
            .map(|m| {
                let key = if let Some(key) = &m.key {
                    self.key(key)
                } else {
                    m.name
                        .as_deref()
                        .map_or(b::PropKey::None, |n| b::PropKey::Name(self.atom(n)))
                };
                let mut flags = b::Flags::empty();
                if m.optional {
                    flags |= b::Flags::OPTIONAL
                }
                if m.readonly {
                    flags |= b::Flags::READONLY
                }
                let mut ty = b::TypeNodeId::NONE;
                let init = b::ExprId::NONE;
                let mut func = b::FnId::NONE;
                let kind = match &m.kind {
                    k::TypeMemberKind::Property(t) => {
                        ty = self.ty(t);
                        b::MemberKind::Property
                    }
                    k::TypeMemberKind::Getter(t) => {
                        let ret = t.as_ref().map_or(b::TypeNodeId::NONE, |t| self.ty(t));
                        let this_param = if let Some(parameter) = &m.this_parameter {
                            let parameter = self.signature_parameter(parameter);
                            let params = self.file.add_params(&[parameter]);
                            self.file.split_this_parameter(params).0
                        } else {
                            b::ParamId::NONE
                        };
                        func = self.file.add_fn(b::Func {
                            kind: b::FnKind::Getter,
                            flags: b::Flags::empty(),
                            name: Atom::NONE,
                            name_pos: m.span.start,
                            type_params: b::Span::EMPTY,
                            params: b::Span::EMPTY,
                            this_param,
                            ret,
                            body: b::FnBody::None,
                            anchor: m.span.start,
                            start: m.span.start,
                        });
                        b::MemberKind::Getter
                    }
                    k::TypeMemberKind::Index {
                        parameter,
                        key_type,
                        value_type,
                    } => {
                        let param_name = self.atom(parameter);
                        let pat =
                            self.file
                                .pat(b::PatKind::Ident(param_name), m.span.start, m.span.end);
                        let key_ty = self.ty(key_type);
                        let param = self.file.add_params(&[b::Param {
                            pat,
                            ty: key_ty,
                            default: b::ExprId::NONE,
                            flags: b::Flags::empty(),
                            pos: m.span.start,
                            loc: Self::loc(m.span),
                        }]);
                        let ret = self.ty(value_type);
                        func = self.file.add_fn(b::Func {
                            kind: b::FnKind::IndexSignature,
                            flags: b::Flags::empty(),
                            name: Atom::NONE,
                            name_pos: m.span.start,
                            type_params: b::Span::EMPTY,
                            params: param,
                            this_param: b::ParamId::NONE,
                            ret,
                            body: b::FnBody::None,
                            anchor: m.span.start,
                            start: m.span.start,
                        });
                        b::MemberKind::IndexSignature
                    }
                    k::TypeMemberKind::Method(sig)
                    | k::TypeMemberKind::Call(sig)
                    | k::TypeMemberKind::Construct(sig)
                    | k::TypeMemberKind::Setter(sig) => {
                        let (kind, fkind) = match &m.kind {
                            k::TypeMemberKind::Method(_) => {
                                (b::MemberKind::Method, b::FnKind::Method)
                            }
                            k::TypeMemberKind::Call(_) => {
                                (b::MemberKind::CallSignature, b::FnKind::CallSignature)
                            }
                            k::TypeMemberKind::Construct(_) => (
                                b::MemberKind::ConstructSignature,
                                b::FnKind::ConstructSignature,
                            ),
                            _ => (b::MemberKind::Setter, b::FnKind::Setter),
                        };
                        func = self.signature(sig, fkind);
                        kind
                    }
                    _ => {
                        self.error(m.span, "type member adapter coverage");
                        b::MemberKind::Property
                    }
                };
                b::Member {
                    kind,
                    key,
                    flags,
                    ty,
                    init,
                    func,
                    name_pos: m.span.start,
                    start: m.span.start,
                    loc: Self::loc(m.span),
                    modifiers: b::Span::EMPTY,
                }
            })
            .collect();
        self.file.add_members(&members)
    }
    pub(super) fn function(&mut self, f: &k::Function) -> b::FnId {
        let (type_params, params, ret, has_body) = self.function_type_parts(f);
        let (this_param, params) = self.file.split_this_parameter(params);
        let body = if has_body {
            b::FnBody::Block(self.statements(&f.body))
        } else {
            b::FnBody::None
        };
        let mut flags = b::Flags::empty();
        if f.asynchronous {
            flags |= b::Flags::ASYNC
        }
        if f.generator {
            flags |= b::Flags::GENERATOR
        }
        self.file.add_fn(b::Func {
            kind: if f.arrow {
                b::FnKind::Arrow
            } else {
                b::FnKind::Expr
            },
            flags,
            name: f.name.as_deref().map_or(Atom::NONE, |n| self.atom(n)),
            name_pos: f.span.start,
            type_params,
            params,
            this_param,
            ret,
            body,
            anchor: f.span.start,
            start: f.span.start,
        })
    }
    pub fn finish(
        mut self,
        arena: &'s crate::engine::session::Arena,
        session: &'s Session,
    ) -> (b::File<'s>, Vec<(k::Span, String)>) {
        let source_body = match self.module.source_kind {
            k::SourceKind::JavaScript | k::SourceKind::Jsx => &self.module.body,
            k::SourceKind::TypeScript | k::SourceKind::Tsx => &self.module.type_syntax.body,
        };
        let mut body: Vec<(u32, b::StmtId)> = source_body
            .iter()
            .map(|s| (s.span.start, self.statement(s)))
            .collect();
        body.extend(self.module_items());
        body.sort_by_key(|(pos, _)| *pos);
        let ids: Vec<_> = body.into_iter().map(|(_, stmt)| stmt).collect();
        self.file.body = self.file.list(&ids);
        for reference in &self.module.type_syntax.references {
            self.file.references.push((
                match reference.kind {
                    k::TypeReferenceKind::Path => b::ReferenceKind::Path,
                    k::TypeReferenceKind::Types => b::ReferenceKind::Types,
                    k::TypeReferenceKind::Lib => b::ReferenceKind::Lib,
                },
                self.atom(&reference.value),
                reference.position,
                match reference.resolution_mode {
                    None => b::ResolutionMode::None,
                    Some(k::TypeResolutionMode::Import) => b::ResolutionMode::Import,
                    Some(k::TypeResolutionMode::Require) => b::ResolutionMode::Require,
                },
            ));
        }
        let (mut file, _) = self.file.into_arena(arena, session);
        file.finish_nodes();
        (file, self.unsupported)
    }
}
