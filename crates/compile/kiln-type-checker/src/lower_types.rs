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
        let sites = &self.module.type_syntax.signatures;
        // The executable function of a method or an accessor spans the whole definition
        // (`m<T>(this: X) {}`, `get p() {}`, after `static`), because `toString` answers what was
        // written. Its signature site spans Oxc's function node, which starts at `<` or `(`. Both
        // end at the body's `}`, and no function nested in that body can end there too.
        let candidates = self
            .signatures_by_end
            .get(&function.span.end)
            .map(|list| list.as_slice())
            .unwrap_or_default();
        let signature = candidates
            .iter()
            .map(|&i| &sites[i as usize])
            .find(|site| site.owner_span == function.span)
            .or_else(|| {
                candidates
                    .iter()
                    .map(|&i| &sites[i as usize])
                    .filter(|site| site.owner_span.start > function.span.start)
                    .min_by_key(|site| site.owner_span.start)
            })
            .cloned();
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
            .instantiations_by_owner
            .get(&span)
            .map(|&i| {
                self.module.type_syntax.instantiations[i as usize]
                    .arguments
                    .clone()
            })
            .unwrap_or_default();
        self.ty_list(&args)
    }

    pub(super) fn declaration_at(&mut self, span: k::Span) -> Option<b::StmtId> {
        let declarations = &self.module.type_syntax.declarations;
        let declaration = self
            .declarations_by_span
            .get(&span)
            .and_then(|list| list.first())
            .map(|&i| &declarations[i as usize])
            .or_else(|| {
                // `export namespace N {}` / `export enum E {}`: executable lowering keeps these as
                // one statement spanning the `export`, so no marker carries the declaration's
                // own span. The declaration is the exported one ending where the statement does;
                // `export namespace A.B {}` nests `B` with the same end, but only `A` is exported.
                self.exported_declarations_by_end
                    .get(&span.end)
                    .into_iter()
                    .flatten()
                    .map(|&i| &declarations[i as usize])
                    .filter(|d| d.span.start > span.start)
                    .min_by_key(|d| d.span.start)
            })
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
                    // `NodeFlagsAmbient`: written `declare`, or inside a declared namespace or
                    // module.
                    if declaration.declared || self.in_ambient_declaration(declaration.span) {
                        flags |= b::Flags::AMBIENT;
                    }
                    if declaration.exported {
                        flags |= b::Flags::EXPORT;
                    }
                    if declaration.default_exported {
                        flags |= b::Flags::EXPORT | b::Flags::DEFAULT;
                    }
                    if declaration.const_ {
                        flags |= b::Flags::CONST;
                    }
                    let vars = self.variable_decls(*kind, declarations, flags);
                    let stmt = self
                        .file
                        .stmt(b::StmtKind::Var(vars), declaration.span.start);
                    self.file[stmt].modifiers =
                        self.declaration_modifiers(flags, declaration.span.start);
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
        // `NodeFlagsAmbient`: written `declare`, or inside a declared namespace or module.
        if declaration.declared || self.in_ambient_declaration(declaration.span) {
            flags |= b::Flags::AMBIENT;
        }
        if declaration.exported {
            flags |= b::Flags::EXPORT;
        }
        if declaration.default_exported {
            flags |= b::Flags::EXPORT | b::Flags::DEFAULT;
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
                    name_pos: self.name_pos(declaration.span, declaration.name.as_deref()),
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
                    name_pos: self.name_pos(declaration.span, declaration.name.as_deref()),
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
                    name_pos: self.name_pos(declaration.span, declaration.name.as_deref()),
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
                // `export { a as b }` / `import x = require("m")` in the block: executable HIR
                // drops them, so a block holding only these has an empty `declaration.body`.
                // Merge them after the rebuild above, which would otherwise discard them (and
                // leave the namespace looking uninstantiated with no `b` in its exports).
                let items = self.ambient_module_items(declaration.span);
                if !items.is_empty() {
                    let mut all: Vec<(u32, b::StmtId)> = self
                        .file
                        .ids::<b::StmtId>(body)
                        .map(|stmt| (self.file[stmt].start, stmt))
                        .collect();
                    all.extend(items);
                    all.sort_by_key(|(pos, _)| *pos);
                    body = self
                        .file
                        .list(&all.into_iter().map(|(_, stmt)| stmt).collect::<Vec<_>>());
                }
                let module_name = if declaration.global || name.is_none() {
                    b::ModuleName::Global
                } else {
                    b::ModuleName::Ident(name)
                };
                let module = self.file.add_module(b::Module {
                    name: module_name,
                    name_pos: self.name_pos(declaration.span, declaration.name.as_deref()),
                    flags,
                    body,
                    has_body: true,
                    specifies_module: self.namespace_specifies_module(declaration),
                    stmt: b::StmtId::NONE,
                });
                b::StmtKind::Module(module)
            }
            k::TypeDeclarationKind::Module => {
                let mut items: Vec<(u32, b::StmtId)> = (declaration.body.iter())
                    .map(|statement| (statement.span.start, self.statement(statement)))
                    .collect();
                if !declaration.global {
                    // `import`/`export` in the body: executable HIR drops them.
                    items.extend(self.ambient_module_items(declaration.span));
                    items.sort_by_key(|(pos, _)| *pos);
                }
                // `declare module "m";` (shorthand, every import `any`) has no block; `{}` and a
                // block of only imports/exports do.
                let has_body =
                    !items.is_empty() || declaration.source_text.trim_end().ends_with('}');
                let body = if items.is_empty() {
                    b::IdList::EMPTY
                } else {
                    let ids: Vec<_> = items.into_iter().map(|(_, stmt)| stmt).collect();
                    self.file.list(&ids)
                };
                let module_name = if declaration.global {
                    b::ModuleName::Global
                } else {
                    // `declare module "m"`: the frontend records the string's value. A module
                    // with an identifier name is a `Namespace`.
                    b::ModuleName::String(self.atom(declaration.name.as_deref().unwrap_or("")))
                };
                let module = self.file.add_module(b::Module {
                    name: module_name,
                    name_pos: self.name_pos(declaration.span, declaration.name.as_deref()),
                    flags,
                    body,
                    has_body,
                    // `ModuleDeclaration.Keyword` only matters for an identifier name: a string
                    // name or `global` never reports 1540, whatever keyword precedes it.
                    specifies_module: false,
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
                // `declare function`: the parser's `NodeFlagsAmbient` is on the declaration itself.
                self.file[function].flags |= flags;
                self.file[function].name = name;
                self.file[function].name_pos =
                    self.name_pos(declaration.span, declaration.name.as_deref());
                b::StmtKind::Fn(function)
            }
            _ => {
                self.error(declaration.span, "unsupported type declaration");
                return self.file.stmt(b::StmtKind::Empty, declaration.span.start);
            }
        };
        let stmt = self.file.stmt(kind, declaration.span.start);
        self.file[stmt].modifiers = self.declaration_modifiers(flags, declaration.span.start);
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
                // The entity after `typeof`, each name where it is written.
                let source = self.module.source.as_bytes();
                let skip = |mut at: usize, also: &[u8]| {
                    while at < source.len()
                        && (source[at].is_ascii_whitespace() || also.contains(&source[at]))
                    {
                        at += 1;
                    }
                    at
                };
                let mut at = skip(t.span.start as usize + "typeof".len(), b"");
                let entity_start = at as u32;
                let mut expr = b::ExprId::NONE;
                for n in names {
                    let atom = self.atom(n);
                    let (start, end) = (at as u32, (at + n.len()).min(source.len()) as u32);
                    expr = if expr.is_none() && n == "this" {
                        self.file.expr(b::ExprKind::This, start, end)
                    } else if expr.is_none() {
                        self.file.expr(b::ExprKind::Ident(atom), start, end)
                    } else {
                        self.file.expr(
                            b::ExprKind::Dot {
                                obj: expr,
                                name: atom,
                                name_pos: start,
                                chain: b::Chain::No,
                            },
                            entity_start,
                            end,
                        )
                    };
                    at = skip(end as usize, b".");
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
                options,
            } => {
                let spec_pos = self
                    .module
                    .source
                    .get(t.span.start as usize..t.span.end as usize)
                    .and_then(|source| source.find(['\'', '"']))
                    .map_or(t.span.start, |offset| t.span.start + offset as u32);
                // The qualifier follows the specifier's string, which may contain its text.
                let name = self.name(qualifier, spec_pos + specifier.len() as u32 + 2);
                let args = self.ty_list(arguments);
                let (mode, attributes) = self.import_type_attributes(options.as_deref());
                let spec = self.atom(specifier);
                self.file.specifier_uses.push(b::SpecifierUse {
                    spec,
                    pos: spec_pos,
                    kind: b::SpecifierKind::ImportType,
                    mode,
                });
                b::TypeNodeKind::Import {
                    spec,
                    name,
                    args,
                    is_typeof: *is_typeof,
                    mode,
                    attributes,
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

    fn import_type_attributes(
        &mut self,
        options: Option<&k::Expression>,
    ) -> (b::ResolutionMode, b::ImportAttributesToken) {
        let Some(options) = options else {
            return (b::ResolutionMode::None, b::ImportAttributesToken::None);
        };
        let k::ExpressionKind::Object { members } = &options.kind else {
            self.error(options.span, "import type options must be an object");
            return (b::ResolutionMode::None, b::ImportAttributesToken::None);
        };
        let Some(property) = members.iter().find_map(|member| match member {
            k::ObjectMember::Property(property) => Some(property),
            _ => None,
        }) else {
            self.error(options.span, "import type options missing attributes");
            return (b::ResolutionMode::None, b::ImportAttributesToken::None);
        };
        let token = match &property.key {
            k::ObjectKey::Static(key) if key == &k::StringUnits::from("assert") => {
                self.file.error(
                    b::DiagnosticKind::Parse,
                    property.span.start,
                    property.span.end,
                    2880,
                );
                b::ImportAttributesToken::Assert
            }
            _ => b::ImportAttributesToken::With,
        };
        let mode = match &property.value.kind {
            k::ExpressionKind::Object {
                members: attributes,
            } if attributes.len() == 1 => match &attributes[0] {
                k::ObjectMember::Property(attribute) if matches!(&attribute.key, k::ObjectKey::Static(key) if key == &k::StringUnits::from("resolution-mode")) => {
                    match &attribute.value.kind {
                        k::ExpressionKind::Literal(k::Literal::String(value))
                            if value == &k::StringUnits::from("require") =>
                        {
                            b::ResolutionMode::Require
                        }
                        k::ExpressionKind::Literal(k::Literal::String(value))
                            if value == &k::StringUnits::from("import") =>
                        {
                            b::ResolutionMode::Import
                        }
                        _ => b::ResolutionMode::None,
                    }
                }
                _ => b::ResolutionMode::None,
            },
            _ => b::ResolutionMode::None,
        };
        let object = self.expr(&property.value);
        self.file
            .import_attributes
            .push((property.span.start, object));
        (mode, token)
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
                            name_pos: self.name_pos(m.span, None),
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
                        let at = self.index_parameter_span(m.span);
                        let pat = self
                            .file
                            .pat(b::PatKind::Ident(param_name), at.start, at.end);
                        let key_ty = self.ty(key_type);
                        let param = self.file.add_params(&[b::Param {
                            pat,
                            ty: key_ty,
                            default: b::ExprId::NONE,
                            flags: b::Flags::empty(),
                            pos: at.start,
                            loc: Self::loc(at),
                        }]);
                        let ret = self.ty(value_type);
                        ty = ret;
                        func = self.file.add_fn(b::Func {
                            kind: b::FnKind::IndexSignature,
                            flags: b::Flags::empty(),
                            name: Atom::NONE,
                            name_pos: self.name_pos(m.span, None),
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
                    name_pos: self.name_pos(m.span, None),
                    start: m.span.start,
                    loc: Self::loc(m.span),
                    modifiers: b::Span::EMPTY,
                }
            })
            .collect::<Vec<_>>();
        let mut members = members;
        for member in &mut members {
            member.flags |= self.literal_name_flags(member);
            // `1n() {}`: the checker reads a bigint name from its text, as TypeScript does.
            if self.is_bigint_name_at(member.name_pos) {
                member.key = b::PropKey::None;
            }
        }
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
            name_pos: match f.name.as_deref() {
                Some(name) => self.name_pos(f.span, Some(name)),
                None => f.span.start,
            },
            type_params,
            params,
            this_param,
            ret,
            body,
            anchor: f.span.start,
            start: f.span.start,
        })
    }
    /// `node.Loc.Pos()` is the node's full start: the end of the previous token, so that the
    /// range covers the leading trivia (where `checkJSDocComment` finds a node's JSDoc). Kiln's
    /// spans start at the first token, which `Self::loc` records; widen each recorded range.
    fn widen_locs_to_full_start(&mut self) {
        use crate::engine::check::spans::skip_trivia_back;
        let file = &mut self.file;
        let text: &[u8] = &file.text;
        let widen = |loc: &mut b::TextRange| {
            if loc.end != 0 {
                loc.pos = skip_trivia_back(text, loc.pos as usize) as u32;
            }
        };
        file.stmts.iter_mut().for_each(|it| widen(&mut it.loc));
        file.var_decls.iter_mut().for_each(|it| widen(&mut it.loc));
        file.enum_members
            .iter_mut()
            .for_each(|it| widen(&mut it.loc));
        // A node reparsed from a JSDoc tag keeps the tag's position (`scopes_of_reparsed_tag`).
        (file.members.iter_mut())
            .filter(|it| !it.flags.contains(b::Flags::REPARSED))
            .for_each(|it| widen(&mut it.loc));
        (file.params.iter_mut())
            .filter(|it| !it.flags.contains(b::Flags::REPARSED))
            .for_each(|it| widen(&mut it.loc));
    }

    /// Where an identifier spelled like a word that strict mode reserves, or `eval` or
    /// `arguments`, may be (`File::keyword_identifier_positions`): the checker finds each such
    /// identifier from there and reports what binder.go does for it. Every occurrence of the word
    /// is recorded, also in a string or a comment: the checker only takes identifiers.
    fn keyword_identifier_positions(&mut self) {
        const WORDS: [&[u8]; 12] = [
            b"implements",
            b"interface",
            b"let",
            b"package",
            b"private",
            b"protected",
            b"public",
            b"static",
            b"yield",
            b"await",
            b"eval",
            b"arguments",
        ];
        let text = self.module.source.as_bytes();
        let is_word = |b: u8| b.is_ascii_alphanumeric() || b == b'_' || b == b'$' || b >= 0x80;
        let mut at = 0;
        while at < text.len() {
            if !is_word(text[at]) {
                at += 1;
                continue;
            }
            let start = at;
            while at < text.len() && is_word(text[at]) {
                at += 1;
            }
            let preceded_by_dot = start > 0 && text[start - 1] == b'.';
            if !preceded_by_dot && WORDS.contains(&&text[start..at]) {
                self.file.keyword_identifier_positions.push(start as u32);
            }
        }
    }

    /// `node.Modifiers()` of each parameter: its decorators and modifier keywords. A parameter is
    /// found by its end, the same for the parameter and its decorators' owner.
    fn parameter_modifiers(&mut self) {
        for index in 0..self.file.params.len() {
            let (pos, end) = (self.file.params[index].pos, self.file.params[index].loc.end);
            let decorators = self.decorator_exprs(k::TypeDecoratorOwner::Parameter, |span| {
                span.end == end && span.start <= pos
            });
            let start = (self.module.type_syntax.decorators.iter())
                .filter(|site| site.owner == k::TypeDecoratorOwner::Parameter)
                .find(|site| site.owner_span.end == end && site.owner_span.start <= pos)
                .map_or(pos, |site| site.owner_span.start);
            let modifiers = self.written_node_modifiers(start, end, &decorators);
            let list = self.file.add_modifiers(&modifiers);
            let param = b::ParamId::from(index as u32);
            self.file.set_param_modifiers(param, list);
            for (_, e) in decorators {
                self.file
                    .decorators
                    .push((b::DecoratorOwner::Param(param), e));
            }
        }
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
        self.parameter_modifiers();
        self.keyword_identifier_positions();
        // In source order.
        let mut decorators = std::mem::take(&mut self.file.decorators);
        decorators.sort_by_key(|&(_, e)| self.file[e].pos);
        self.file.decorators = decorators;
        self.file
            .import_attributes
            .sort_by_key(|attributes| attributes.0);
        // By expression; for one expression, innermost first, the order they were found in.
        self.file.parens.sort_by_key(|paren| paren.0.0);
        self.file.jsx_expressions.sort_by_key(|braces| braces.0.0);
        self.file
            .specifier_uses
            .sort_by_key(|specifier| specifier.pos);
        self.widen_locs_to_full_start();
        let (mut file, _) = self.file.into_arena(arena, session);
        file.finish_nodes();
        (file, self.unsupported)
    }

    /// `ModuleDeclaration.Keyword == KindModuleKeyword` for `module a {}`, `declare module a {}`
    /// or `export module a {}`. The `b` of `module a.b {}` is a declaration of its own that
    /// starts at `b`, so it takes the keyword of the outermost declaration of the chain: the
    /// one that ends where it does.
    fn namespace_specifies_module(&self, declaration: &k::TypeDeclaration) -> bool {
        let outermost = self
            .module
            .type_syntax
            .declarations
            .iter()
            .filter(|outer| {
                matches!(outer.kind, k::TypeDeclarationKind::Namespace)
                    && outer.span.end == declaration.span.end
                    && outer.span.start <= declaration.span.start
            })
            .min_by_key(|outer| outer.span.start)
            .unwrap_or(declaration);
        let mut rest = outermost.source_text.trim_start();
        for modifier in ["export", "declare"] {
            if let Some(after) = rest.strip_prefix(modifier)
                && after.starts_with(|c: char| c.is_whitespace())
            {
                rest = after.trim_start();
            }
        }
        rest.strip_prefix("module").is_some_and(|after| {
            !after.starts_with(|c: char| c.is_alphanumeric() || c == '_' || c == '$')
        })
    }
}
