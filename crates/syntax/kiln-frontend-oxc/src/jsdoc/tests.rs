use super::*;
use kiln_hir::jsdoc::*;
use kiln_hir::{TypeExprKind, TypeKeyword};

fn parse(source: &str) -> JsDocSyntax {
    let mut ranges = Vec::new();
    let mut at = 0;
    while let Some(found) = source[at..].find("/*") {
        let start = at + found;
        let end = source[start + 2..]
            .find("*/")
            .map_or(source.len(), |e| start + 2 + e + 2);
        ranges.push((start as u32, end as u32));
        at = end;
    }
    parse_comments(source, ranges)
}

#[test]
fn type_tag() {
    let syntax = parse("/** @type {string | number[]} */\nvar x;");
    let doc = &syntax.docs[0];
    assert!(doc.diagnostics.is_empty(), "{:?}", doc.diagnostics);
    let JsDocTagKind::Type(expr) = &doc.tags[0].kind else {
        panic!("{:?}", doc.tags)
    };
    let ty = expr.ty.as_ref().unwrap();
    assert!(
        matches!(&ty.kind, TypeExprKind::Union(types) if types.len() == 2),
        "{ty:?}"
    );
}

#[test]
fn param_and_returns() {
    let syntax = parse(
        "/**\n * @param {number} a the a\n * @param {string=} [b] b\n * @returns {boolean}\n */\nfunction f(a, b) {}",
    );
    let doc = &syntax.docs[0];
    assert!(doc.diagnostics.is_empty(), "{:?}", doc.diagnostics);
    assert_eq!(doc.tags.len(), 3, "{:?}", doc.tags);
    let JsDocTagKind::Param(a) = &doc.tags[0].kind else {
        panic!()
    };
    assert_eq!(a.name[0].text, "a");
    let JsDocTagKind::Param(b) = &doc.tags[1].kind else {
        panic!()
    };
    assert!(b.is_bracketed);
    assert!(matches!(&doc.tags[2].kind, JsDocTagKind::Return(Some(_))));
}

#[test]
fn jsdoc_only_types() {
    let syntax = parse(
        "/** @type {?string} */ /** @type {*} */ /** @type {Array.<number>} */ /** @type {!Object} */ /** @type {function(string): number} */",
    );
    let kinds: Vec<_> = syntax
        .docs
        .iter()
        .map(|doc| match &doc.tags[0].kind {
            JsDocTagKind::Type(expr) => (expr.ty.clone().unwrap(), doc.diagnostics.clone()),
            other => panic!("{other:?}"),
        })
        .collect();
    assert!(matches!(kinds[0].0.kind, TypeExprKind::JsDoc { .. }));
    assert!(matches!(
        kinds[1].0.kind,
        TypeExprKind::Keyword(TypeKeyword::Any)
    ));
    assert!(
        matches!(&kinds[2].0.kind, TypeExprKind::Reference { arguments, .. } if arguments.len() == 1)
    );
    assert!(matches!(kinds[3].0.kind, TypeExprKind::JsDoc { .. }));
    // `function(` is not a type: `'}' expected.` at the `(`.
    assert_eq!(kinds[4].1.len(), 1, "{:?}", kinds[4]);
    assert_eq!(kinds[4].1[0].code, 1005);
}

#[test]
fn typedef_with_properties() {
    let syntax = parse(
        "/**\n * @typedef {Object} Point\n * @property {number} x\n * @property {number} y\n */",
    );
    let doc = &syntax.docs[0];
    let JsDocTagKind::Typedef(typedef) = &doc.tags[0].kind else {
        panic!("{:?}", doc.tags)
    };
    assert_eq!(typedef.name.name.text, "Point");
    let JsDocTagType::Literal { properties, .. } = &typedef.ty else {
        panic!("{:?}", typedef.ty)
    };
    assert_eq!(properties.len(), 2);
}

#[test]
fn template_and_callback() {
    let syntax = parse(
        "/**\n * @template {object} T, U\n * @callback Cb\n * @param {T} x\n * @returns {U}\n */",
    );
    let doc = &syntax.docs[0];
    assert!(doc.diagnostics.is_empty(), "{:?}", doc.diagnostics);
    let JsDocTagKind::Template(template) = &doc.tags[0].kind else {
        panic!("{:?}", doc.tags)
    };
    assert_eq!(template.params.len(), 2);
    assert!(template.constraint.is_some());
    let JsDocTagKind::Callback(callback) = &doc.tags[1].kind else {
        panic!()
    };
    assert_eq!(callback.signature.params.len(), 1);
    assert!(callback.signature.ret.is_some());
}

#[test]
fn import_tag() {
    let syntax = parse("/** @import { A, type B as C } from './x' */");
    let doc = &syntax.docs[0];
    assert!(doc.diagnostics.is_empty(), "{:?}", doc.diagnostics);
    let JsDocTagKind::Import(import) = &doc.tags[0].kind else {
        panic!("{:?}", doc.tags)
    };
    assert_eq!(import.named.len(), 2);
    assert_eq!(import.named[1].imported, "B");
    assert_eq!(import.named[1].local, "C");
    assert!(import.named[1].is_type_only);
    assert_eq!(import.specifier.as_ref().unwrap().0, "./x");
}
