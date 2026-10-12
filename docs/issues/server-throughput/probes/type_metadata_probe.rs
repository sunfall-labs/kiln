fn main() {
    for (path, source) in [
        (
            "typed.ts",
            "export function increment(value: number): number { return value + 1; }",
        ),
        ("node:stream.ts", kiln_builtins::STREAM_SOURCE),
    ] {
        let generated = kiln_frontend_oxc::parse(path, source);
        let authored = generated
            .type_syntax
            .original_module
            .as_ref()
            .expect("authored TypeScript retained");
        for (stage, hir) in [
            ("authored", authored.as_ref()),
            ("raw executable HIR (before source facts)", &generated),
        ] {
            let ir = kiln_ir::compile_module(hir).expect("compiles");
            let annotations = ir
                .program
                .functions
                .iter()
                .filter(|f| f.type_annotations.is_some())
                .count();
            let checked = ir
                .program
                .functions
                .iter()
                .filter(|f| {
                    f.type_annotations
                        .as_ref()
                        .is_some_and(|t| t.source_types.is_some())
                })
                .count();
            println!(
                "{path} {stage}: {} functions, {annotations} annotated, {checked} checker facts",
                ir.program.functions.len()
            );
        }
    }
}
