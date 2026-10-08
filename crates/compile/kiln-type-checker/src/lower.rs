//! Adapt Kiln-owned syntax to the private checker representation. No second parser.
use crate::engine::{
    atom::{Atom, Interner},
    hir as b,
};
use kiln_hir as k;
use std::borrow::Cow;

pub(crate) struct Lower<'a, 's> {
    pub file: b::FileBuilder,
    pub module: &'a k::Module,
    pub(super) atoms: &'a Interner<'s>,
    pub unsupported: Vec<(k::Span, String)>,
    /// The type-syntax sites that wrap an expression, by the span of the operand they wrap.
    pub(super) wrappers: rustc_hash::FxHashMap<k::Span, smallvec::SmallVec<[Wrapper; 1]>>,
    /// `type_syntax.names` by the end of their owner: the name and where it starts.
    names: rustc_hash::FxHashMap<u32, smallvec::SmallVec<[(&'a str, u32); 1]>>,
    /// `type_syntax.signatures` and `type_syntax.classes` by the end of their owner.
    pub(super) signatures_by_end: rustc_hash::FxHashMap<u32, smallvec::SmallVec<[u32; 1]>>,
    pub(super) classes_by_end: rustc_hash::FxHashMap<u32, smallvec::SmallVec<[u32; 1]>>,
    /// The spans of constructor parameter properties. The frontend runs `constructor(public x)`
    /// as TypeScript emits it, with a `this.x = x` statement of the parameter's span in the body;
    /// the checker reads the parameter, and that statement is not in the source.
    pub(super) parameter_properties: rustc_hash::FxHashSet<k::Span>,
    /// The `import x = ...` declarations lowered so far, by start.
    pub(super) lowered_import_equals: rustc_hash::FxHashSet<u32>,
    /// Indexes of the sidecar lists, which lowering would otherwise search once per node.
    pub(super) annotations_by_owner: rustc_hash::FxHashMap<k::Span, smallvec::SmallVec<[u32; 2]>>,
    pub(super) instantiations_by_owner: rustc_hash::FxHashMap<k::Span, u32>,
    /// Declarations other than imports, by span, and the exported ones by end.
    pub(super) declarations_by_span: rustc_hash::FxHashMap<k::Span, smallvec::SmallVec<[u32; 1]>>,
    pub(super) exported_declarations_by_end:
        rustc_hash::FxHashMap<u32, smallvec::SmallVec<[u32; 1]>>,
    /// The namespaces and modules: span, and whether `declare`d.
    pub(super) containers: Vec<(k::Span, bool)>,
    /// `type_syntax.exported_declarations` starts, by end.
    pub(super) export_wrapped_by_end: rustc_hash::FxHashMap<u32, smallvec::SmallVec<[u32; 1]>>,
}

/// A source-level wrapper that executable HIR erased: an index into the site list it came from.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Wrapper {
    /// `as` or `satisfies`: `type_syntax.annotations`.
    Annotation(u32),
    /// `!`, `as const` or parentheses: `type_syntax.expression_wrappers`.
    Expression(u32),
    /// `f<T>` without a call: `type_syntax.instantiations`.
    Instantiation(u32),
}
impl<'a, 's> Lower<'a, 's> {
    pub fn new(module: &'a k::Module, atoms: &'a Interner<'s>) -> Self {
        // Static TS execution HIR has been transformed to JavaScript. The checker must consume
        // the authored module sidecar so all declaration nodes and spans address its text.
        let module = module
            .type_syntax
            .original_module
            .as_deref()
            .unwrap_or(module);
        let mut file = b::FileBuilder::default();
        file.source_len = module.source.len() as u32;
        file.text = Cow::Owned(module.source.as_bytes().to_vec());
        file.is_js = matches!(
            module.source_kind,
            k::SourceKind::JavaScript | k::SourceKind::Jsx
        );
        file.has_module_syntax = module
            .items
            .iter()
            .any(|i| matches!(i.kind, k::ItemKind::Import | k::ItemKind::Export));
        let syntax = &module.type_syntax;
        let mut wrappers: rustc_hash::FxHashMap<k::Span, smallvec::SmallVec<[Wrapper; 1]>> =
            rustc_hash::FxHashMap::default();
        for (index, site) in syntax.annotations.iter().enumerate() {
            if let Some(operand) = site.operand_span
                && matches!(
                    site.site,
                    k::TypeSite::AsExpression | k::TypeSite::SatisfiesExpression
                )
            {
                wrappers
                    .entry(operand)
                    .or_default()
                    .push(Wrapper::Annotation(index as u32));
            }
        }
        for (index, site) in syntax.expression_wrappers.iter().enumerate() {
            wrappers
                .entry(site.operand_span)
                .or_default()
                .push(Wrapper::Expression(index as u32));
        }
        for (index, site) in syntax.instantiations.iter().enumerate() {
            if let Some(operand) = site.operand_span
                && site.kind == k::TypeInstantiationKind::Expression
            {
                wrappers
                    .entry(operand)
                    .or_default()
                    .push(Wrapper::Instantiation(index as u32));
            }
        }
        let mut names: rustc_hash::FxHashMap<u32, smallvec::SmallVec<[(&str, u32); 1]>> =
            rustc_hash::FxHashMap::default();
        for site in &syntax.names {
            names
                .entry(site.owner_end)
                .or_default()
                .push((site.name.as_str(), site.start));
        }
        let mut signatures_by_end: rustc_hash::FxHashMap<u32, smallvec::SmallVec<[u32; 1]>> =
            rustc_hash::FxHashMap::default();
        for (index, site) in syntax.signatures.iter().enumerate() {
            signatures_by_end
                .entry(site.owner_span.end)
                .or_default()
                .push(index as u32);
        }
        let mut classes_by_end: rustc_hash::FxHashMap<u32, smallvec::SmallVec<[u32; 1]>> =
            rustc_hash::FxHashMap::default();
        for (index, site) in syntax.classes.iter().enumerate() {
            classes_by_end
                .entry(site.span.end)
                .or_default()
                .push(index as u32);
        }
        let mut annotations_by_owner: rustc_hash::FxHashMap<k::Span, smallvec::SmallVec<[u32; 2]>> =
            rustc_hash::FxHashMap::default();
        for (index, site) in syntax.annotations.iter().enumerate() {
            annotations_by_owner
                .entry(site.owner_span)
                .or_default()
                .push(index as u32);
        }
        let mut instantiations_by_owner = rustc_hash::FxHashMap::default();
        for (index, site) in syntax.instantiations.iter().enumerate() {
            instantiations_by_owner
                .entry(site.owner_span)
                .or_insert(index as u32);
        }
        let mut declarations_by_span: rustc_hash::FxHashMap<k::Span, smallvec::SmallVec<[u32; 1]>> =
            rustc_hash::FxHashMap::default();
        let mut exported_declarations_by_end: rustc_hash::FxHashMap<
            u32,
            smallvec::SmallVec<[u32; 1]>,
        > = rustc_hash::FxHashMap::default();
        let mut containers = Vec::new();
        for (index, declaration) in syntax.declarations.iter().enumerate() {
            if matches!(declaration.kind, k::TypeDeclarationKind::Import) {
                continue;
            }
            declarations_by_span
                .entry(declaration.span)
                .or_default()
                .push(index as u32);
            if declaration.exported {
                exported_declarations_by_end
                    .entry(declaration.span.end)
                    .or_default()
                    .push(index as u32);
            }
            if matches!(
                declaration.kind,
                k::TypeDeclarationKind::Namespace | k::TypeDeclarationKind::Module
            ) {
                containers.push((declaration.span, declaration.declared));
            }
        }
        let mut export_wrapped_by_end: rustc_hash::FxHashMap<u32, smallvec::SmallVec<[u32; 1]>> =
            rustc_hash::FxHashMap::default();
        for span in &syntax.exported_declarations {
            export_wrapped_by_end
                .entry(span.end)
                .or_default()
                .push(span.start);
        }
        let parameter_properties = (module.type_syntax.classes.iter())
            .flat_map(|class| &class.members)
            .filter(|member| member.kind == k::TypeClassMemberKind::ParameterProperty)
            .map(|member| member.span)
            .collect();
        Self {
            parameter_properties,
            file,
            module,
            atoms,
            unsupported: Vec::new(),
            wrappers,
            names,
            signatures_by_end,
            classes_by_end,
            lowered_import_equals: rustc_hash::FxHashSet::default(),
            annotations_by_owner,
            instantiations_by_owner,
            declarations_by_span,
            exported_declarations_by_end,
            containers,
            export_wrapped_by_end,
        }
    }
    /// `node.Name().Pos()` of the declaration, member or property that ends at `owner.end`: the
    /// one named `name`, or else the first, if one starts inside `owner`. `owner.start` without.
    pub(super) fn name_pos(&self, owner: k::Span, name: Option<&str>) -> u32 {
        let candidates = self.names.get(&owner.end).into_iter().flatten();
        let inside = candidates.filter(|(_, start)| *start >= owner.start);
        let found = match name {
            Some(name) => inside
                .filter(|(text, _)| *text == name)
                .map(|(_, s)| *s)
                .min(),
            None => inside.map(|(_, start)| *start).min(),
        };
        found.unwrap_or(owner.start)
    }
    /// The syntactic form of the name that starts at `pos`, when it is not in brackets:
    /// `"a"` is a `StringLiteral`, `0` and `.5` are `NumericLiteral`s (`getLiteralTypeFromPropertyName`:
    /// `keyof { 0: 1 }` is `0`, `keyof { "0": 1 }` is `"0"`). Executable HIR keeps only the
    /// key's value, so the source text decides, as Bun's `Mark::StringLiteralName` does.
    pub(super) fn literal_name_kind(&self, pos: u32) -> b::NameKind {
        match self.module.source.as_bytes().get(pos as usize) {
            Some(b'"' | b'\'') => b::NameKind::StringLiteral,
            Some(b'0'..=b'9' | b'.') => b::NameKind::NumericLiteral,
            _ => b::NameKind::Identifier,
        }
    }
    /// The same as the flags of a member: `LITERAL_NAME`, and `STRING_NAME` for a string.
    pub(super) fn literal_name_flags(&self, member: &b::Member) -> b::Flags {
        if !matches!(member.key, b::PropKey::Name(_)) {
            return b::Flags::empty();
        }
        match self.literal_name_kind(member.name_pos) {
            b::NameKind::StringLiteral => b::Flags::LITERAL_NAME | b::Flags::STRING_NAME,
            b::NameKind::NumericLiteral => b::Flags::LITERAL_NAME,
            _ => b::Flags::empty(),
        }
    }
    /// The class site of the class that spans `span`, which may start before it at `export`
    /// or after it, past decorators: the outermost that ends with it and starts inside it.
    pub(super) fn class_site(&self, span: k::Span) -> Option<&'a k::TypeClassSite> {
        let classes = &self.module.type_syntax.classes;
        let candidates = self.classes_by_end.get(&span.end)?;
        let exact = candidates
            .iter()
            .map(|&i| &classes[i as usize])
            .find(|site| site.span == span);
        exact.or_else(|| {
            candidates
                .iter()
                .map(|&i| &classes[i as usize])
                .filter(|site| site.span.start >= span.start)
                .min_by_key(|site| site.span.start)
        })
    }
    pub(super) fn atom(&self, name: &str) -> Atom {
        self.atoms.intern(name.as_bytes())
    }
    pub(super) fn atom_units(&self, units: &k::StringUnits) -> Atom {
        let mut bytes = Vec::with_capacity(units.len());
        for scalar in char::decode_utf16(units.units()) {
            match scalar {
                Ok(c) => {
                    let mut buf = [0; 4];
                    bytes.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
                }
                Err(e) => {
                    let n = u32::from(e.unpaired_surrogate());
                    bytes.extend_from_slice(&[
                        (0xe0 | (n >> 12)) as u8,
                        (0x80 | ((n >> 6) & 0x3f)) as u8,
                        (0x80 | (n & 0x3f)) as u8,
                    ]);
                }
            }
        }
        self.atoms.intern(&bytes)
    }
    pub(super) fn error(&mut self, span: k::Span, what: impl Into<String>) {
        self.unsupported.push((span, what.into()));
    }
    pub(super) fn loc(span: k::Span) -> b::TextRange {
        b::TextRange {
            pos: span.start,
            end: span.end,
        }
    }
    /// `A.B.C`, written at or after `pos`: each name at its own position, as the parser gives
    /// it. The checker reports a qualified name's missing member at, and with the text of, its
    /// right identifier (`Namespace '"m"' has no exported member 'B'`). A name not found as a
    /// whole identifier keeps the position the previous name ended at.
    pub(super) fn name(&mut self, names: &[String], pos: u32) -> b::Span<b::NameId> {
        let module = self.module;
        let source = module.source.as_bytes();
        let identifier = |c: u8| c.is_ascii_alphanumeric() || c == b'_' || c == b'$' || c >= 0x80;
        let mut cursor = pos as usize;
        let names: Vec<_> = names
            .iter()
            .map(|n| {
                let mut from = cursor;
                let mut at = cursor;
                while let Some(found) = module
                    .source
                    .get(from..)
                    .and_then(|rest| rest.find(n.as_str()))
                {
                    let start = from + found;
                    let end = start + n.len();
                    if (start == 0 || !identifier(source[start - 1]))
                        && source.get(end).is_none_or(|&c| !identifier(c))
                    {
                        at = start;
                        break;
                    }
                    from = start + n.chars().next().map_or(1, char::len_utf8);
                }
                cursor = at + n.len();
                (self.atom(n), at as u32)
            })
            .collect();
        self.file.entity_name(names.into_iter())
    }
}
impl<'a, 's> Lower<'a, 's> {
    pub(super) fn annotation(
        &mut self,
        span: k::Span,
        site: fn(&k::TypeSite) -> bool,
    ) -> b::TypeNodeId {
        let annotations = &self.module.type_syntax.annotations;
        let ty = self
            .annotations_by_owner
            .get(&span)
            .into_iter()
            .flatten()
            .map(|&i| &annotations[i as usize])
            .find(|a| site(&a.site))
            .map(|a| a.ty.clone());
        self.optional_ty(ty.as_ref())
    }
}
