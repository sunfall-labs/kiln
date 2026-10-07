//! Static module syntax, retained independently of executable erasure.
use crate::{
    engine::{atom::Atom, hir as b},
    lower::Lower,
};
use kiln_hir as k;
use std::collections::BTreeMap;
impl Lower<'_, '_> {
    fn module_attributes_object(&mut self, attributes: &k::TypeModuleAttributes) -> b::ExprId {
        let props: Vec<_> = attributes
            .attributes
            .iter()
            .map(|attribute| {
                let value = self.file.expr(
                    b::ExprKind::String(self.atom_units(&attribute.value)),
                    attribute.value_span.start,
                    attribute.value_span.end,
                );
                b::Prop {
                    kind: b::PropKind::Init,
                    key: b::PropKey::Name(self.atom_units(&attribute.key)),
                    name_kind: match attribute.key_kind {
                        k::TypeModuleAttributeKeyKind::Identifier => b::NameKind::Identifier,
                        k::TypeModuleAttributeKeyKind::StringLiteral => b::NameKind::StringLiteral,
                    },
                    value,
                    pos: attribute.key_span.start,
                    start: attribute.span.start,
                    end: attribute.span.end,
                    postfix_token: 0,
                }
            })
            .collect();
        let properties = self.file.add_props(&props);
        let object = self.file.expr(
            b::ExprKind::Object(properties),
            attributes.attributes_span.start,
            attributes.attributes_span.end,
        );
        self.file
            .import_attributes
            .push((attributes.keyword_span.start, object));
        if attributes.keyword == k::TypeModuleAttributesKeyword::Assert {
            self.file.error(
                b::DiagnosticKind::Parse,
                attributes.keyword_span.start,
                attributes.keyword_span.end,
                2880,
            );
        }
        object
    }

    fn module_resolution_mode(
        &self,
        statement_span: k::Span,
        declaration_type_only: bool,
    ) -> b::ResolutionMode {
        if !declaration_type_only {
            return b::ResolutionMode::None;
        }
        let Some(attributes) = self
            .module
            .type_syntax
            .module_attributes
            .iter()
            .find(|attributes| attributes.statement_span == statement_span)
        else {
            return b::ResolutionMode::None;
        };
        if attributes.attributes.len() != 1 {
            return b::ResolutionMode::None;
        }
        let attribute = &attributes.attributes[0];
        if attribute.key_kind != k::TypeModuleAttributeKeyKind::StringLiteral
            || attribute.key != k::StringUnits::from("resolution-mode")
        {
            return b::ResolutionMode::None;
        }
        match &attribute.value {
            value if value == &k::StringUnits::from("require") => b::ResolutionMode::Require,
            value if value == &k::StringUnits::from("import") => b::ResolutionMode::Import,
            _ => b::ResolutionMode::None,
        }
    }

    fn declaration_is_type_only(
        &self,
        statement_span: k::Span,
        kind: k::ModuleRequestKind,
    ) -> bool {
        self.module
            .requested_modules
            .iter()
            .flat_map(|request| &request.occurrences)
            .any(|occurrence| {
                occurrence.statement_span == statement_span
                    && occurrence.kind == kind
                    && occurrence.type_only
            })
    }

    /// Lower a TypeScript `import X = ...` declaration at its source position.
    /// OXC erases it from the executable statement tree, so this metadata is
    /// also the authoritative bridge back to the original lexical statement.
    pub(super) fn import_equals_at(&mut self, span: k::Span) -> Option<b::StmtId> {
        let import = self
            .module
            .type_syntax
            .import_equals
            .iter()
            .find(|import| import.span == span)?
            .clone();
        let source = self
            .module
            .source
            .get(span.start as usize..span.end as usize)?;
        let mut name_rel = source.find("import").map_or(0, |pos| pos + "import".len());
        while source
            .as_bytes()
            .get(name_rel)
            .is_some_and(u8::is_ascii_whitespace)
        {
            name_rel += 1;
        }
        if import.type_only && source[name_rel..].starts_with("type") {
            name_rel += "type".len();
            while source
                .as_bytes()
                .get(name_rel)
                .is_some_and(u8::is_ascii_whitespace)
            {
                name_rel += 1;
            }
        }
        let name_pos = span.start + name_rel as u32;
        let mut flags = b::Flags::empty();
        if import.exported {
            flags |= b::Flags::EXPORT;
        }
        if import.declared {
            flags |= b::Flags::AMBIENT;
        }
        if import.type_only {
            flags |= b::Flags::TYPE_ONLY;
        }
        let target = match &import.reference {
            k::TypeImportEqualsReference::ExternalModule(specifier) => {
                let spec = self.atom(specifier);
                let spec_pos = source
                    .find("require")
                    .and_then(|require| {
                        source[require..]
                            .find(['\'', '"', '`'])
                            .map(|p| require + p)
                    })
                    .map_or(span.start, |p| span.start + p as u32);
                self.file.specifier_uses.push(b::SpecifierUse {
                    spec,
                    pos: spec_pos,
                    kind: b::SpecifierKind::Require,
                    mode: b::ResolutionMode::None,
                });
                b::ImportEqualsTarget::Require(spec)
            }
            k::TypeImportEqualsReference::EntityName(parts) => {
                let mut cursor = name_rel + import.name.len();
                let names: Vec<_> = parts
                    .iter()
                    .map(|part| {
                        let rel = source[cursor..].find(part).map_or(cursor, |p| cursor + p);
                        cursor = rel.saturating_add(part.len());
                        (self.atom(part), span.start + rel as u32)
                    })
                    .collect();
                b::ImportEqualsTarget::Entity(self.file.entity_name(names.into_iter()))
            }
        };
        let id = self.file.add_import_equals(b::ImportEquals {
            name: self.atom(&import.name),
            name_pos,
            target,
            expression: b::ExprId::NONE,
            flags,
            stmt: b::StmtId::NONE,
        });
        let stmt = self.file.stmt(b::StmtKind::ImportEquals(id), span.start);
        self.file[stmt].loc = Self::loc(span);
        Some(stmt)
    }

    pub(super) fn module_items(&mut self) -> Vec<(u32, b::StmtId)> {
        let mut result = Vec::new();
        let module_attributes = self.module.type_syntax.module_attributes.clone();
        for attributes in &module_attributes {
            self.module_attributes_object(attributes);
        }
        let mut groups: BTreeMap<k::Span, Vec<&k::Import>> = BTreeMap::new();
        for i in &self.module.imports {
            groups.entry(i.statement_span).or_default().push(i);
        }
        for request in &self.module.requested_modules {
            for occurrence in &request.occurrences {
                if occurrence.kind == k::ModuleRequestKind::Import {
                    groups.entry(occurrence.statement_span).or_default();
                }
            }
        }
        for (span, imports) in groups {
            let source = imports.first().map(|i| i.source.as_str()).or_else(|| {
                self.module
                    .requested_modules
                    .iter()
                    .find(|r| r.occurrences.iter().any(|o| o.statement_span == span))
                    .map(|r| r.specifier.as_str())
            });
            let Some(source) = source else { continue };
            let id = b::ImportId(self.file.imports.len() as u32);
            let mut default = Atom::NONE;
            let mut default_pos = 0;
            let mut namespace = Atom::NONE;
            let mut namespace_pos = 0;
            let mut specs = Vec::new();
            let type_only = self.declaration_is_type_only(span, k::ModuleRequestKind::Import);
            for i in imports {
                match &i.imported {
                    k::ImportName::Default => {
                        default = self.atom(&i.local);
                        default_pos = i.span.start
                    }
                    k::ImportName::Namespace => {
                        namespace = self.atom(&i.local);
                        namespace_pos = i.span.start
                    }
                    k::ImportName::Name(name) => specs.push(b::ImportSpec {
                        start: i.span.start,
                        imported: self.atom(name),
                        local: self.atom(&i.local),
                        pos: i.span.start,
                        type_only: i.type_only && !type_only,
                        imported_pos: i.span.start,
                        end: i.span.end,
                        import: id,
                    }),
                }
            }
            let named = self.file.add_import_specs(&specs);
            let spec = self.atom(source);
            let mode = self.module_resolution_mode(span, type_only);
            self.file.specifier_uses.push(b::SpecifierUse {
                spec,
                pos: span.start,
                kind: if default.is_none() && namespace.is_none() && named.is_empty() {
                    b::SpecifierKind::SideEffect
                } else {
                    b::SpecifierKind::Import
                },
                mode,
            });
            let id = self.file.add_import(b::Import {
                spec,
                default,
                default_pos,
                namespace,
                namespace_pos,
                clause_start: span.start,
                clause_end: span.end,
                namespace_start: namespace_pos,
                named,
                type_only,
                is_deferred: false,
                mode,
                stmt: b::StmtId::NONE,
            });
            let stmt = self.file.stmt(b::StmtKind::Import(id), span.start);
            self.file[stmt].loc = Self::loc(span);
            result.push((span.start, stmt));
        }
        for e in &self.module.exports {
            let declaration_type_only =
                self.declaration_is_type_only(e.statement_span, k::ModuleRequestKind::Export);
            let mode = self.module_resolution_mode(e.statement_span, declaration_type_only);
            // Inline exported declarations (including `export declare const x, y`) already carry
            // export flags on the declarations. The module-record pass also reports each binding
            // as a local export, but lowering that second representation duplicates the symbols.
            if matches!(&e.origin, k::ExportOrigin::Local { .. })
                && self
                    .module
                    .type_syntax
                    .declarations
                    .iter()
                    .any(|declaration| {
                        declaration.exported
                            && declaration.span.start >= e.statement_span.start
                            && declaration.span.end <= e.statement_span.end
                    })
            {
                continue;
            }
            let exported = e
                .exported
                .as_ref()
                .map_or(Atom::NONE, |n| self.atom(n.as_str()));
            let kind = match &e.origin {
                k::ExportOrigin::DefaultExpression => continue,
                k::ExportOrigin::Local { local }
                | k::ExportOrigin::ReExport {
                    imported: k::ImportName::Name(local),
                    ..
                } => {
                    let spec = match &e.origin {
                        k::ExportOrigin::ReExport { source, .. } => self.atom(source),
                        _ => Atom::NONE,
                    };
                    let items = self.file.add_export_specs(&[b::ExportSpec {
                        start: e.span.start,
                        local: self.atom(local),
                        exported,
                        pos: e.span.start,
                        type_only: false,
                        local_pos: e.span.start,
                        end: e.span.end,
                        export: b::ExportId(self.file.exports.len() as u32),
                    }]);
                    if spec.is_some() {
                        self.file.specifier_uses.push(b::SpecifierUse {
                            spec,
                            pos: e.statement_span.start,
                            kind: b::SpecifierKind::Import,
                            mode,
                        });
                    }
                    b::StmtKind::ExportNamed(self.file.add_export(b::Export {
                        spec,
                        items,
                        type_only: e.type_only,
                        mode,
                        stmt: b::StmtId::NONE,
                    }))
                }
                k::ExportOrigin::Star { source }
                | k::ExportOrigin::NamespaceReExport { source } => {
                    let spec = self.atom(source);
                    self.file.specifier_uses.push(b::SpecifierUse {
                        spec,
                        pos: e.statement_span.start,
                        kind: b::SpecifierKind::Import,
                        mode,
                    });
                    b::StmtKind::ExportStar {
                        spec,
                        alias: exported,
                        type_only: e.type_only,
                        mode,
                        star_pos: e.span.start,
                        alias_pos: e.span.start,
                    }
                }
                k::ExportOrigin::ReExport {
                    source,
                    imported: k::ImportName::Default,
                } => {
                    let spec = self.atom(source);
                    let local = self.atom("default");
                    let items = self.file.add_export_specs(&[b::ExportSpec {
                        start: e.span.start,
                        local,
                        exported,
                        pos: e.span.start,
                        type_only: false,
                        local_pos: e.span.start,
                        end: e.span.end,
                        export: b::ExportId(self.file.exports.len() as u32),
                    }]);
                    self.file.specifier_uses.push(b::SpecifierUse {
                        spec,
                        pos: e.statement_span.start,
                        kind: b::SpecifierKind::Import,
                        mode,
                    });
                    b::StmtKind::ExportNamed(self.file.add_export(b::Export {
                        spec,
                        items,
                        type_only: e.type_only,
                        mode,
                        stmt: b::StmtId::NONE,
                    }))
                }
                _ => {
                    self.error(e.span, "export adapter coverage");
                    continue;
                }
            };
            let stmt = self.file.stmt(kind, e.statement_span.start);
            self.file[stmt].loc = Self::loc(e.statement_span);
            result.push((e.statement_span.start, stmt));
        }
        result
    }
}
