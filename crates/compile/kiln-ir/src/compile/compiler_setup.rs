use super::*;

impl super::Compiler {
    pub(super) fn note_call_site(&mut self, span: Span) {
        self.debug_call_sites
            .push((self.active_function_span, self.code.len() as u32, span));
    }

    pub(super) fn set_property_instruction(&self, name: u32) -> Instruction {
        Instruction::SetPropertyStrict {
            name,
            strict: self.strict,
        }
    }

    pub(super) fn set_computed_instruction(&self) -> Instruction {
        Instruction::SetComputedStrict {
            strict: self.strict,
        }
    }

    pub(super) fn set_super_property_instruction(&self, name: u32) -> Instruction {
        Instruction::SetSuperPropertyStrict {
            name,
            strict: self.strict,
        }
    }

    pub(super) fn set_super_element_instruction(&self) -> Instruction {
        Instruction::SetSuperElementStrict {
            strict: self.strict,
        }
    }

    pub(super) fn delete_property_instruction(&self, name: u32) -> Instruction {
        Instruction::DeletePropertyStrict {
            name,
            strict: self.strict,
        }
    }

    pub(super) fn delete_computed_instruction(&self) -> Instruction {
        Instruction::DeleteComputedStrict {
            strict: self.strict,
        }
    }

    pub(super) fn new(module: &Module) -> Self {
        let mut compiler = Self {
            source: Arc::clone(&module.source),
            source_provenance: module.static_source_provenance.clone(),
            source_origin: module.source_origin.clone(),
            constants: Vec::new(),
            constant_indices: rustc_hash::FxHashMap::default(),
            code: Vec::new(),
            functions: Vec::new(),
            template_sites: Vec::new(),
            debug_call_sites: Vec::new(),
            active_function_span: None,
            strict_functions: Vec::new(),
            removable_declarations: Vec::new(),
            module_instantiation_end: 0,
            enclosing: Vec::new(),
            upvalues: Vec::new(),
            upvalue_names: HashMap::new(),
            // The module body has nested functions of its own, so its block bindings can be
            // captured. Computed here rather than in each entry point, because there are two --
            // `compile` and `compile_module` -- and one of them would have been forgotten.
            captured: names_captured_by_nested_functions(&module.body),
            strict: false,
            completion: None,
            eval_program: false,
            module_body: false,
            parameter_dead_zone: false,
            parameter_scope: None,
            blocked_var_names: Vec::new(),
            outer_names: HashSet::new(),
            annexb_var_functions: HashSet::new(),
            global_bindings: HashSet::new(),
            global_var_scope: false,
            global_bindings_configurable: false,
            // Module scope is boxed too when the module contains a direct eval: a script's
            // top-level bindings are exactly the ones an eval written there can see.
            cell_everything: body_contains_direct_eval(&module.body),
            loops: Vec::new(),
            hidden_bindings: 0,
            resource_scopes: Vec::new(),
            async_generator: false,
            // The module body has no parameters and no `arguments`; the flag is a function's.
            mapped_arguments: false,
            with_scopes: Vec::new(),
            eval_lexical_names: HashSet::new(),
            private_scopes: Vec::new(),
            private_scope_modules: Vec::new(),
            private_module_id: module.module_id.to_string(),
            private_brands: Vec::new(),
            next_private_scope: 0,
            private_declarations: Vec::new(),
            finalizers: Vec::new(),
            pending_finalizers: Vec::new(),
            function_finalizer_depth: 0,
            handler_depth: 0,
            pending_labels: Vec::new(),
            chains: Vec::new(),
            super_bindings: Vec::new(),
            pending_elements: Vec::new(),
            super_binding_count: 0,
            home_bindings: Vec::new(),
            home_binding_count: 0,
            in_function: false,
            super_property: false,
            this_binding: None,
            derived_constructor: false,
            this_binding_count: 0,
            next_is_method: false,
            field_initialiser: false,
            scopes: vec![HashMap::new()],
            local_count: 0,
            diagnostics: Vec::new(),
            current_function_span: None,
            local_spans: BTreeMap::new(),
            new_object_spans: BTreeMap::new(),
        };
        if module.panicked {
            compiler.error(None, "frontend could not produce an executable module");
        }
        for diagnostic in &module.diagnostics {
            compiler.diagnostics.push(CompileDiagnostic {
                message: diagnostic.message.clone(),
                span: diagnostic.span,
            });
        }
        compiler
    }

    pub(super) fn compile_body(&mut self, module: &Module) {
        // A module body is a variable environment like a function's, so its `var`s are hoisted
        // to the top of it rather than to the top of whichever block they were written in.
        self.hoist_var_names(&module.body);
        // Script code, so B.3.3.2 rather than B.3.2.1: there are no parameters to decline for.
        // A module body is strict and `hoist_annexb_function_names` returns immediately for it.
        self.hoist_annexb_function_names(&module.body, &HashSet::new());
        if self.module_body {
            self.plain_module_statements(&module.body);
        } else {
            self.plain_statements(&module.body);
        }
        self.push_constant(Constant::Undefined);
        self.code.push(Instruction::Return);
    }

    pub(super) fn plain_module_statements(&mut self, list: &[Statement]) {
        for statement in list {
            self.hoist_names(statement);
        }
        for statement in list {
            if let StatementKind::Function(function) = &statement.kind {
                self.emit_function_declaration(function, statement.span);
            }
        }
        // Everything above this boundary is part of ModuleDeclarationInstantiation: all
        // module bindings have been created and every hoisted function declaration has its
        // closure. Evaluation begins with the first ordinary statement below.
        self.module_instantiation_end = self.code.len() as u32;
        for statement in list {
            if matches!(statement.kind, StatementKind::Function(_)) {
                continue;
            }
            let removable_names = removable_declaration_names(statement);
            let pc_start = self.code.len();
            self.statement(statement);
            if let Some(names) = removable_names {
                let local_slots = names
                    .iter()
                    .filter_map(|name| self.resolve_module_local(name))
                    .collect::<Vec<_>>();
                if local_slots.len() == names.len() {
                    self.removable_declarations.push(RemovableDeclaration {
                        local_slots,
                        pc_start: pc_start as u32,
                        pc_end: self.code.len() as u32,
                    });
                }
            }
        }
    }

    /// The body of an eval program, which returns its completion value.
    ///
    /// `eval("1 + 1")` is 2, and a module body is not: it always completes with `undefined`, so
    /// the ordinary path throws every statement's value away. The completion value is the last
    /// *expression* statement's, held in a slot of its own so that a statement which produces
    /// nothing -- a declaration, a loop -- leaves the previous one standing, which is what the
    /// specification's completion records do.
    ///
    /// Tracked at every depth, because a completion value is usually produced below the top
    /// level: `eval("2; switch (1) { case 1: 3; break; }")` is 3, and `eval("1; if (true) { }")`
    /// is `undefined` rather than 1. See `reset_completion` for which statements normalise an
    /// empty completion and which pass one through.
    pub(super) fn compile_eval_body(&mut self, module: &Module) {
        let completion = self.local_count;
        self.local_count += 1;
        self.push_constant(Constant::Undefined);
        self.code.push(Instruction::StoreLocal(completion));
        // The same three passes `statements` makes -- names, then hoisted function values, then
        // the rest -- with the expression statements diverted into the completion slot.
        self.hoist_var_names(&module.body);
        self.hoist_annexb_function_names(&module.body, &HashSet::new());
        for statement in &module.body {
            self.hoist_names(statement);
        }
        for statement in &module.body {
            if let StatementKind::Function(function) = &statement.kind {
                self.emit_function_declaration(function, statement.span);
            }
        }
        self.completion = Some(completion);
        for statement in &module.body {
            if !matches!(statement.kind, StatementKind::Function(_)) {
                self.statement(statement);
            }
        }
        self.completion = None;
        self.code.push(Instruction::LoadLocal(completion));
        self.code.push(Instruction::Return);
    }

    pub(super) fn finish_with_strictness(
        self,
    ) -> Result<(Program, Strictness), Vec<CompileDiagnostic>> {
        if self.diagnostics.is_empty() {
            let mut raw_sites = self.debug_call_sites;
            raw_sites.sort_by_key(|(_, _, span)| span.start);
            let mut generated_offsets = Vec::with_capacity(raw_sites.len());
            let mut original_offsets = Vec::with_capacity(raw_sites.len());
            for (index, (_, _, span)) in raw_sites.iter().enumerate() {
                let mapped = self
                    .source_provenance
                    .as_ref()
                    .and_then(|origin| origin.original_offset(span.start));
                if let Some(offset) = mapped {
                    original_offsets.push((offset, index));
                } else {
                    generated_offsets.push((span.start, index));
                }
            }
            let mut positions = vec![(0, 0); raw_sites.len()];
            fill_source_positions(&self.source, &mut generated_offsets, &mut positions);
            if let Some(origin) = &self.source_provenance {
                fill_source_positions(
                    &origin.original_source,
                    &mut original_offsets,
                    &mut positions,
                );
            }
            let mut debug_call_sites = raw_sites
                .into_iter()
                .enumerate()
                .map(|(index, (function_span, instruction, _))| {
                    let (line, column) = positions[index];
                    DebugCallSite {
                        function_span,
                        instruction,
                        line,
                        column,
                    }
                })
                .collect::<Vec<_>>();
            debug_call_sites.sort_by_key(|site| (site.function_span, site.instruction));
            let source_origin = self.source_origin;
            let static_source_provenance = self.source_provenance;
            let static_source_identity = static_source_provenance
                .as_ref()
                .map(|provenance| Arc::clone(&provenance.identity));
            Ok((
                Program {
                    version: BYTECODE_VERSION,
                    constants: self.constants,
                    code: self.code,
                    local_count: self.local_count,
                    functions: self.functions,
                    source: self.source,
                    source_origin,
                    static_source_identity,
                    static_source_provenance,
                    template_sites: self.template_sites,
                    debug_call_sites,
                },
                Strictness {
                    body: self.strict,
                    functions: self.strict_functions,
                },
            ))
        } else {
            let mut diagnostics = self.diagnostics;
            if let Some(origin) = &self.source_provenance {
                for diagnostic in &mut diagnostics {
                    if let Some(span) = &mut diagnostic.span {
                        if let Some(start) = origin.original_offset(span.start) {
                            span.start = start;
                        }
                        if let Some(end) = origin.original_offset(span.end) {
                            span.end = end.max(span.start);
                        }
                    }
                }
            }
            Err(diagnostics)
        }
    }

    pub(super) fn declare_import(&mut self, name: &str, span: Span) -> Option<u32> {
        if self.scopes[0].contains_key(name) {
            self.error(Some(span), format!("duplicate module binding `{name}`"));
            return None;
        }
        let slot = self.local_count;
        self.local_count += 1;
        self.scopes[0].insert(
            name.to_owned(),
            Local {
                slot,
                kind: VariableKind::Const,
                imported: true,
                // An import is bound before the module body runs, so it is never in a dead zone.
                initialized: true,
                // *Always* a cell. An import is a live view of the exporter's binding, not a
                // copy of its value at the moment the exporter finished -- `export let count`
                // that a function later increments has to read as the new number here. The cell
                // is the exporter's own, installed by the host before the first instruction, so
                // there is nothing for the module prologue to box.
                celled: true,
                eval_declaration: None,
            },
        );
        Some(slot)
    }

    /// The reserved key a private field is stored under, in the class being compiled.
    ///
    /// Scoped to the class rather than to the name: `class A { #v } class B { #v }` declare two
    /// different fields, and `a.#v` on a `B` is a TypeError rather than a read of B's. One key
    /// per name would have made those the same slot, which is a wrong answer -- and the one
    /// `#v in b` would have reported as a match.
    pub(super) fn private_key(&self, name: &str) -> String {
        // The innermost class that *declares* the name, which is not always the innermost class:
        // a nested class does not shadow a name it never mentioned.
        for (depth, declared) in self.private_declarations.iter().enumerate().rev() {
            if declared.contains(name) {
                return private_key(
                    &self.private_scope_modules[depth],
                    self.private_scopes[depth],
                    name,
                );
            }
        }
        // Undeclared anywhere. A syntax error in its own right, reported where the name is used
        // rather than invented into some other class's scope.
        let depth = self.private_scopes.len().saturating_sub(1);
        private_key(
            self.private_scope_modules
                .get(depth)
                .map_or(&self.private_module_id, String::as_str),
            self.private_scopes.get(depth).copied().unwrap_or(0),
            name,
        )
    }

    /// The brand binding of the class that *declares* `name`, mirroring [`Self::private_key`].
    pub(super) fn private_brand(&self, name: &str) -> Option<String> {
        for (depth, declared) in self.private_declarations.iter().enumerate().rev() {
            if declared.contains(name) {
                return self.private_brands.get(depth).cloned();
            }
        }
        self.private_brands.last().cloned()
    }

    /// The reserved name an instance stores its class's brand under.
    ///
    /// One per class, not one per member: the brand says which *evaluation* an instance came
    /// from, and every private member of that class shares the answer.
    pub(super) fn private_brand_key(&self, name: &str) -> String {
        for (depth, declared) in self.private_declarations.iter().enumerate().rev() {
            if declared.contains(name) {
                return private_key(
                    &self.private_scope_modules[depth],
                    self.private_scopes[depth],
                    "%brand",
                );
            }
        }
        let depth = self.private_scopes.len().saturating_sub(1);
        private_key(
            self.private_scope_modules
                .get(depth)
                .map_or(&self.private_module_id, String::as_str),
            self.private_scopes.get(depth).copied().unwrap_or(0),
            "%brand",
        )
    }

    pub(super) fn resolve_module_local(&self, name: &str) -> Option<u32> {
        self.scopes[0].get(name).map(|local| local.slot)
    }

    pub(super) fn error(&mut self, span: Option<Span>, message: impl Into<String>) {
        self.diagnostics.push(CompileDiagnostic {
            message: message.into(),
            span,
        });
    }

    /// The completion slot, when the statement being compiled can contribute to one.
    ///
    /// Inside a function body it cannot: `enclosing` is non-empty there, the slot index belongs
    /// to another frame's locals, and the value is the function's to return.
    pub(super) fn completion_slot(&self) -> Option<u32> {
        self.enclosing
            .is_empty()
            .then_some(self.completion)
            .flatten()
    }

    /// `UpdateEmpty(stmtCompletion, undefined)`, which is how most statements end.
    ///
    /// `if`, the five loops, `switch`, `with` and `try` each normalise an empty body completion
    /// to `undefined` rather than passing the previous statement's value along -- so
    /// `eval("1; if (true) { }")` is `undefined`, not 1. Seeding the slot before the body is the
    /// same thing said forwards: whatever the body produces overwrites it, and a body that
    /// produces nothing leaves the `undefined` standing.
    ///
    /// A block and a labelled statement are deliberately *not* in that list. They pass an empty
    /// completion through, which is why `eval("1; { }")` and `eval("1; lbl: { }")` are both 1.
    pub(super) fn reset_completion(&mut self) {
        if let Some(slot) = self.completion_slot() {
            self.push_constant(Constant::Undefined);
            self.code.push(Instruction::StoreLocal(slot));
        }
    }

    /// Compile `body` with the completion slot detached, and put it back afterwards.
    ///
    /// A `finally` block's completion is discarded unless it is abrupt, so its statements must
    /// not touch the slot: `eval("try { 2; } finally { 3; }")` is 2.
    pub(super) fn without_completion(&mut self, body: impl FnOnce(&mut Self)) {
        let saved = self.completion.take();
        body(self);
        self.completion = saved;
    }

    /// Read `name`, looking on each active `with` object before the bindings around it.
    ///
    /// `fallback` emits the read for the case no object has it -- the ordinary lexical one, or
    /// the `"undefined"` that a `typeof` of an unbound name produces.
    /// `delete name` where `name` is an unqualified identifier, which sloppy code alone can
    /// write.
    ///
    /// A `with` object is asked first, exactly as a *read* of the same name asks it: the name may
    /// be absent from every binding around the statement and still be a property of the object,
    /// and it is that property `delete` removes.
    pub(super) fn delete_binding(&mut self, name: &str) {
        let scopes = self.with_scopes_for(name);
        if scopes.is_empty() {
            self.delete_binding_lexically(name);
            return;
        }
        let key = self.constant_index(Constant::String(name.to_owned().into()));
        let mut found = Vec::with_capacity(scopes.len());
        for with in &scopes {
            self.load_with_object(with);
            self.code.push(Instruction::WithHas(key));
            let miss = self.emit_jump(Instruction::JumpIfFalse(0));
            self.load_with_object(with);
            self.code.push(Instruction::DeleteProperty(key));
            found.push(self.emit_jump(Instruction::Jump(0)));
            self.patch_jump(miss);
        }
        self.delete_binding_lexically(name);
        for jump in found {
            self.patch_jump(jump);
        }
    }

    pub(super) fn delete_binding_lexically(&mut self, name: &str) {
        match self.resolve_or_capture(name) {
            // Anything a scope declared is undeletable -- `var`, `let`, a parameter, a
            // function declaration, an import -- so the answer is known here and nothing
            // needs to run.
            Resolution::Local(_) | Resolution::Upvalue(_) | Resolution::Global(_) => {
                self.push_constant(Constant::Boolean(false));
            }
            // A global `var` is a property like any other: whether it can be deleted is the
            // property's own business, and a script's is non-configurable while an `eval`'s is
            // not. So the object is asked, exactly as it is for a name nothing declared.
            Resolution::GlobalObject => {
                let name = self.constant_index(Constant::String(name.to_owned().into()));
                self.code.push(Instruction::DeleteGlobalBinding(name));
            }
            // Nothing placed it, so the global object's environment record is the last one
            // an unqualified reference reaches and its `[[Delete]]` is the answer.  That is
            // also right for the names the compiler otherwise short-circuits -- `undefined`
            // and the intrinsics -- because the realm registers those as properties whose
            // attributes are exactly what decides `delete`.
            // `arguments` inside a function is a binding the call created, not a global: it is
            // undeletable like any other, and asking the global object would answer `true` for a
            // name that is right there.
            Resolution::Unresolved if name == "arguments" && self.compiling_function() => {
                self.push_constant(Constant::Boolean(false));
            }
            Resolution::Unresolved => {
                let name = self.constant_index(Constant::String(name.to_owned().into()));
                self.code.push(Instruction::DeleteGlobalBinding(name));
            }
        }
    }

    /// `object.#name = value`, and the compound forms.
    ///
    /// The same shape as a member assignment with a different pair of instructions, which is the
    /// whole difference: the private ones check that the name is already there.
    pub(super) fn private_assignment(
        &mut self,
        operator: AssignmentOperator,
        object: &Expression,
        name: &str,
        value: &Expression,
    ) {
        let brand = self.private_brand(name);
        let name = self.constant_index(Constant::String(self.private_key(name).into()));
        if operator.is_short_circuiting() {
            self.short_circuiting_private(operator, object, name, &brand, value);
            return;
        }
        self.expression(object);
        if operator != AssignmentOperator::Assign {
            self.code.push(Instruction::Dup);
            self.push_private_brand(&brand);
            self.code.push(Instruction::GetPrivate(name));
            self.expression(value);
            self.code
                .push(Instruction::Binary(compound_operator(operator)));
        } else {
            self.expression(value);
        }
        self.push_private_brand(&brand);
        self.code.push(Instruction::SetPrivate(name));
    }
}

/// Compute one-based line and UTF-16 column coordinates for sorted byte offsets.
fn fill_source_positions(source: &str, offsets: &mut [(u32, usize)], output: &mut [(u32, u32)]) {
    offsets.sort_unstable_by_key(|(offset, _)| *offset);
    let mut chars = source.char_indices().peekable();
    let (mut line, mut column) = (1u32, 1u32);
    for &(offset, index) in offsets.iter() {
        while chars
            .peek()
            .is_some_and(|(byte_offset, _)| *byte_offset < offset as usize)
        {
            let (_, ch) = chars.next().expect("peeked source character");
            match ch {
                '\n' => {
                    line += 1;
                    column = 1;
                }
                '\r' => {
                    if chars.peek().is_some_and(|(_, next)| *next == '\n') {
                        chars.next();
                    }
                    line += 1;
                    column = 1;
                }
                '\u{2028}' | '\u{2029}' => {
                    line += 1;
                    column = 1;
                }
                _ => column += ch.len_utf16() as u32,
            }
        }
        output[index] = (line, column);
    }
}
