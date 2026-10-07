//! Static module syntax, retained independently of executable erasure.
use crate::{
    engine::{atom::Atom, hir as b},
    lower::Lower,
};
use kiln_hir as k;
use std::collections::BTreeMap;
impl Lower<'_, '_> {
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
            let type_only = !imports.is_empty() && imports.iter().all(|i| i.type_only);
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
            self.file.specifier_uses.push(b::SpecifierUse {
                spec,
                pos: span.start,
                kind: if default.is_none() && namespace.is_none() && named.is_empty() {
                    b::SpecifierKind::SideEffect
                } else {
                    b::SpecifierKind::Import
                },
                mode: b::ResolutionMode::None,
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
                mode: b::ResolutionMode::None,
                stmt: b::StmtId::NONE,
            });
            let stmt = self.file.stmt(b::StmtKind::Import(id), span.start);
            self.file[stmt].loc = Self::loc(span);
            result.push((span.start, stmt));
        }
        for e in &self.module.exports {
            // Declaration export flags already describe these bindings. A second
            // export clause would duplicate the declaration and its type-only flag.
            if matches!(&e.origin, k::ExportOrigin::Local { local } if self.module.type_syntax.declarations.iter().any(|d| d.exported && d.name.as_ref() == Some(local) && d.span.start >= e.statement_span.start && d.span.end <= e.statement_span.end))
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
                            mode: b::ResolutionMode::None,
                        });
                    }
                    b::StmtKind::ExportNamed(self.file.add_export(b::Export {
                        spec,
                        items,
                        type_only: e.type_only,
                        mode: b::ResolutionMode::None,
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
                        mode: b::ResolutionMode::None,
                    });
                    b::StmtKind::ExportStar {
                        spec,
                        alias: exported,
                        type_only: e.type_only,
                        mode: b::ResolutionMode::None,
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
                        mode: b::ResolutionMode::None,
                    });
                    b::StmtKind::ExportNamed(self.file.add_export(b::Export {
                        spec,
                        items,
                        type_only: e.type_only,
                        mode: b::ResolutionMode::None,
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
