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
        Self {
            file,
            module,
            atoms,
            unsupported: Vec::new(),
        }
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
    pub(super) fn name(&mut self, names: &[String], pos: u32) -> b::Span<b::NameId> {
        let names: Vec<_> = names.iter().map(|n| (self.atom(n), pos)).collect();
        self.file.entity_name(names.into_iter())
    }
}
impl<'a, 's> Lower<'a, 's> {
    pub(super) fn annotation(
        &mut self,
        span: k::Span,
        site: fn(&k::TypeSite) -> bool,
    ) -> b::TypeNodeId {
        let ty = self
            .module
            .type_syntax
            .annotations
            .iter()
            .find(|a| a.owner_span == span && site(&a.site))
            .map(|a| a.ty.clone());
        self.optional_ty(ty.as_ref())
    }
}
