use super::*;
use kiln_builtin_surface::WellKnownSymbol;

impl super::Compiler {
    pub(super) fn statement(&mut self, statement: &Statement) {
        // Every statement that normalises an empty completion to `undefined`, named here rather
        // than in nine separate compile functions so the list can be read against the grammar.
        if matches!(
            statement.kind,
            StatementKind::If { .. }
                | StatementKind::While { .. }
                | StatementKind::DoWhile { .. }
                | StatementKind::For { .. }
                | StatementKind::ForIn { .. }
                | StatementKind::ForOf { .. }
                | StatementKind::Switch { .. }
                | StatementKind::With { .. }
                | StatementKind::Try { .. }
        ) {
            self.reset_completion();
        }
        match &statement.kind {
            StatementKind::Variable { kind, declarations } => {
                for declaration in declarations {
                    // A plain name keeps the direct path, which knows about `var` redeclaration
                    // and temporal-dead-zone checks. A pattern evaluates its initialiser once
                    // and binds against the result.
                    match declaration.pattern.as_identifier() {
                        Some(name) => {
                            self.variable(
                                *kind,
                                name,
                                declaration.initializer.as_ref(),
                                declaration.span,
                            );
                            // `using` binds exactly as `const` does; the registration is the
                            // whole of what it adds, and it runs after the binding so that a
                            // disposal method reached through the binding sees it initialised.
                            if let Some(awaiting) = kind.disposal() {
                                self.register_resource(name, awaiting, declaration.span);
                            }
                        }
                        None => {
                            match &declaration.initializer {
                                Some(initializer) => self.expression(initializer),
                                // `const { a } = undefined` throws in JavaScript; there is no
                                // throw from the compiler, so it binds undefined and each
                                // property read yields undefined.
                                None => self.push_constant(Constant::Undefined),
                            }
                            self.bind_pattern(*kind, &declaration.pattern, declaration.span);
                        }
                    }
                }
            }
            StatementKind::Expression(expression) => {
                self.expression(expression);
                // At any depth, not just the top level: `eval("2; switch (1) { case 1: 3; }")`
                // is 3, and the statement that produced it is two blocks down.
                match self.completion_slot() {
                    Some(slot) => self.code.push(Instruction::StoreLocal(slot)),
                    None => self.code.push(Instruction::Pop),
                }
            }
            StatementKind::Block(statements) => {
                self.scopes.push(HashMap::new());
                self.statements(statements);
                self.scopes.pop().expect("block scope was just pushed");
            }
            StatementKind::With { object, body } => {
                self.with_statement(object, body, statement.span);
            }
            StatementKind::If {
                test,
                consequent,
                alternate,
            } => self.if_statement(test, consequent, alternate.as_deref()),
            StatementKind::While { test, body } => self.while_statement(test, body),
            StatementKind::DoWhile { body, test } => self.do_while_statement(body, test),
            StatementKind::Switch {
                discriminant,
                cases,
            } => self.switch_statement(discriminant, cases, statement.span),
            StatementKind::For {
                init,
                test,
                update,
                body,
            } => self.for_statement(
                init.as_deref(),
                test.as_ref(),
                update.as_ref(),
                body,
                statement.span,
            ),
            StatementKind::Labelled { label, body } => self.labelled_statement(label, body),
            StatementKind::ForIn {
                kind,
                name,
                object,
                body,
            } => self.for_in(*kind, name, object, body, statement.span),
            StatementKind::ForOf {
                kind,
                pattern,
                iterable,
                body,
                awaiting,
            } => self.for_of(*kind, pattern, iterable, body, *awaiting, statement.span),
            StatementKind::Throw(value) => {
                self.expression(value);
                self.code.push(Instruction::Throw);
            }
            StatementKind::Try {
                block,
                handler,
                finalizer,
            } => self.try_statement(block, handler.as_ref(), finalizer, statement.span),
            StatementKind::Break { label } => self.jump_out(label.as_deref(), true, statement.span),
            StatementKind::Continue { label } => {
                self.jump_out(label.as_deref(), false, statement.span)
            }
            StatementKind::Empty => {}
            StatementKind::Function(function) => {
                self.emit_function_declaration(function, statement.span)
            }
            StatementKind::Return(value) => self.return_statement(value.as_ref(), statement.span),
            StatementKind::Class(class) => self.class_declaration(class),
            // `export default <expression>` has no name in the source, so it binds the reserved
            // one the module record already uses for an anonymous default. Everything after it
            // -- the export table, the linker -- then sees an ordinary local export.
            StatementKind::ExportDefault(expression) => self.variable(
                VariableKind::Const,
                kiln_hir::DEFAULT_EXPORT_LOCAL,
                Some(expression),
                statement.span,
            ),
            StatementKind::Unsupported { feature } => self.error(
                Some(statement.span),
                format!("unsupported statement: {feature}"),
            ),
        }
    }

    pub(super) fn variable(
        &mut self,
        kind: VariableKind,
        name: &str,
        initializer: Option<&Expression>,
        span: Span,
    ) {
        self.variable_with(kind, name, initializer, span, false);
    }

    /// Bind `pattern` to the value on top of the stack, consuming it.
    pub(super) fn bind_pattern(&mut self, kind: VariableKind, pattern: &Pattern, span: Span) {
        self.destructure(PatternLeaf::Declare(kind), pattern, span);
    }

    /// Assign `pattern` from the value on top of the stack, consuming it.
    ///
    /// Every rule about how a pattern takes a value apart -- defaults applying only to
    /// `undefined`, a hole skipping an index, a rest copying what the named keys did not take --
    /// is the same whether the names are being declared or assigned. Only the leaf differs.
    pub(super) fn assign_pattern(&mut self, pattern: &Pattern, span: Span) {
        self.destructure(PatternLeaf::Assign, pattern, span);
    }

    /// Take the value on top of the stack apart, consuming it.
    ///
    /// A leaf that stores through a property reference expects the receiver -- and the key, for a
    /// computed one -- to be sitting under that value already: [`Compiler::destructure_element`]
    /// puts them there before the value exists, because that is the order the language evaluates
    /// them in. Only that method reaches such a leaf, which is why nothing here re-derives them.
    /// `RequireObjectCoercible` on the value an object pattern is about to take apart.
    ///
    /// Part of the pattern rather than a side effect of reading it: `var {} = null` throws even
    /// though the pattern names nothing to read. A pattern *with* properties only appeared to
    /// obey the rule, because reading its first key off `null` threw for an unrelated reason --
    /// which left `var {} = null`, `({} = null)` and `var { ...rest } = null` accepting a value
    /// no object pattern may take.
    pub(super) fn require_object_coercible(&mut self) {
        self.code.push(Instruction::Dup);
        let coercible = self.emit_jump(Instruction::JumpIfNotNullish(0));
        self.code.push(Instruction::LoadBuiltin(Builtin::TypeError));
        self.push_constant(Constant::String(
            "Cannot destructure 'null' or 'undefined'".to_owned().into(),
        ));
        self.code.push(Instruction::New { argc: 1 });
        self.code.push(Instruction::Throw);
        self.patch_jump(coercible);
    }

    pub(super) fn destructure(&mut self, leaf: PatternLeaf, pattern: &Pattern, span: Span) {
        match pattern {
            Pattern::Identifier(name) => {
                if name.is_empty() {
                    // The frontend already reported why; binding an impossible name here would
                    // add a second, more confusing diagnostic.
                    self.code.push(Instruction::Pop);
                    return;
                }
                match leaf {
                    PatternLeaf::Declare(kind) => self.declare_bound(kind, name, span),
                    PatternLeaf::Assign => self.store_existing(name, span),
                }
            }
            Pattern::Member { property, .. } => {
                let name = self.constant_index(Constant::String(property.clone().into()));
                self.store_through_reference(leaf, self.set_property_instruction(name), span);
            }
            Pattern::Computed { .. } => {
                self.store_through_reference(leaf, self.set_computed_instruction(), span);
            }
            Pattern::PrivateMember { name, .. } => {
                let brand = self.private_brand(name);
                let name = self.constant_index(Constant::String(self.private_key(name).into()));
                // The store is the first thing `store_through_reference` emits, so the brand
                // pushed here lands directly beneath it.
                self.push_private_brand(&brand);
                self.store_through_reference(leaf, Instruction::SetPrivate(name), span);
            }
            Pattern::WithDefault { inner, default } => {
                // A default applies only to `undefined`, not to every falsy value and not to
                // `null` -- which is why this is a nullish-style test against undefined alone
                // rather than a truthiness test.
                self.code.push(Instruction::Dup);
                self.push_constant(Constant::Undefined);
                self.code.push(Instruction::Binary(BinaryOp::StrictEqual));
                let skip = self.emit_jump(Instruction::JumpIfFalse(0));
                self.code.push(Instruction::Pop);
                self.expression(default);
                self.patch_jump(skip);
                self.destructure(leaf, inner, span);
            }
            Pattern::Object { properties, rest } => {
                self.require_object_coercible();
                let spilled = rest.is_some()
                    || properties.iter().any(|property| {
                        stores_through_reference(&property.value)
                            || matches!(property.key, PatternKey::Computed(_))
                    });
                let source = self.pattern_source(spilled, span);

                let mut taken = Vec::with_capacity(properties.len());
                for property in properties {
                    // A computed key is evaluated where the source wrote it: before the target's
                    // reference and before the read, and exactly once even when a rest below
                    // needs the key again.
                    let key = match &property.key {
                        PatternKey::Static(name) => SourceKey::Static(
                            self.constant_index(Constant::String(name.clone().into())),
                        ),
                        PatternKey::Computed(expression) => {
                            self.expression(expression);
                            let held = format!("%key{}", self.hidden_bindings);
                            self.hidden_bindings += 1;
                            self.declare_hidden(&held, span);
                            SourceKey::Computed(held)
                        }
                    };
                    self.destructure_element(leaf, &property.value, span, |compiler| {
                        compiler.load_pattern_source(&source);
                        compiler.read_key(&key);
                    });
                    taken.push(key);
                }

                // Copy everything, then remove what the named properties already took. The other
                // direction -- copy all but a set of names -- would need the exclusion list at
                // runtime; this needs only instructions that already earn their keep.
                if let Some(rest) = rest {
                    self.destructure_element(leaf, rest, span, |compiler| {
                        compiler.code.push(Instruction::NewObject { count: 0 });
                        compiler.load_pattern_source(&source);
                        compiler.code.push(Instruction::SpreadObject);
                        for key in &taken {
                            compiler.code.push(Instruction::Dup);
                            match key {
                                SourceKey::Static(index) => {
                                    compiler
                                        .code
                                        .push(compiler.delete_property_instruction(*index));
                                }
                                SourceKey::Computed(held) => {
                                    compiler.load_hidden(held);
                                    compiler.code.push(compiler.delete_computed_instruction());
                                }
                            }
                            compiler.code.push(Instruction::Pop);
                        }
                    });
                }
                self.finish_pattern_source(&source);
            }
            Pattern::Array { elements, rest } => {
                self.destructure_array(leaf, elements, rest.as_deref(), span);
            }
        }
    }

    /// Take an array pattern apart by stepping its iterator, one element at a time.
    ///
    /// The shorter lowering -- materialise the iterable into an array and read it by index --
    /// gets three separate things wrong, and each of them is something a program can observe:
    ///
    /// * `const [a] = naturals()` must take exactly one step. Draining first never returns.
    /// * An iterator the pattern did not exhaust must be *closed*, by calling its `return`. A
    ///   generator on the other side of one runs its `finally` blocks because of that call.
    /// * How many times `next` ran is visible to whoever wrote the iterator, so `[] = iter` --
    ///   which takes nothing -- must not step it at all.
    ///
    /// So the protocol is walked here, in the same shape `for-of` walks it and for the same
    /// reason: no single instruction can stop part-way through an iterable.
    ///
    /// `done` tracks the specification's `iteratorRecord.[[Done]]`, and the flag is what decides
    /// whether the close happens. It is set *before* each `next` and cleared only once a value
    /// is in hand, because an iterator that threw is finished rather than merely unfinished --
    /// closing one that has already failed would call `return` on it, which the language does
    /// not do.
    pub(super) fn destructure_array(
        &mut self,
        leaf: PatternLeaf,
        elements: &[Option<Pattern>],
        rest: Option<&Pattern>,
        span: Span,
    ) {
        let index = self.hidden_bindings;
        self.hidden_bindings += 1;
        let iterator = format!("%iterated{index}");
        let done = format!("%drained{index}");
        let step = format!("%stepped{index}");
        let next_method = format!("%nextof{index}");

        // `value[Symbol.iterator]()`, which is what makes `const [a] = new Set([1])` bind 1: a
        // pattern reads what the value yields, not what its indices hold.
        self.push_constant(Constant::WellKnownSymbol(WellKnownSymbol::Iterator));
        self.code.push(Instruction::CallComputed { argc: 0 });
        self.declare_hidden(&iterator, span);
        // The rest of the iterator record. `GetIteratorFromMethod` reads `next` once, before any
        // element is taken, and every step calls what it captured -- which is why `const [] = it`
        // reads `next` even though it steps nothing, and why a three-element pattern reads it
        // once rather than three times.
        let next = self.constant_index(Constant::String("next".to_owned().into()));
        self.load_hidden(&iterator);
        self.code.push(Instruction::GetProperty(next));
        self.declare_hidden(&next_method, span);
        self.push_constant(Constant::Boolean(false));
        self.declare_hidden(&done, span);
        // The slot the step results land in, bound once here so each element can store into it.
        self.push_constant(Constant::Undefined);
        self.declare_hidden(&step, span);

        // Everything from here to the close is guarded: an exception out of a default, a nested
        // pattern or a target's own evaluation has to close the iterator on its way past.
        let handler_jump = self.emit_jump(Instruction::PushHandler(0));

        for element in elements {
            match element {
                // A hole steps the iterator and reads nothing -- not even `value`, which is why
                // it is not the same as binding a name no one looks at.
                None => self.emit_iterator_step(&iterator, &next_method, &done, &step, false),
                Some(element) => {
                    self.destructure_element(leaf, element, span, |compiler| {
                        compiler.emit_iterator_step(&iterator, &next_method, &done, &step, true);
                    });
                }
            }
        }
        if let Some(rest) = rest {
            self.destructure_element(leaf, rest, span, |compiler| {
                compiler.emit_iterator_drain(&iterator, &next_method, &done, &step, span);
            });
        }

        self.code.push(Instruction::PopHandler);
        // A pattern that consumed every value has an iterator that closed itself; calling
        // `return` on an exhausted iterator is a step the language does not take.
        self.load_hidden(&done);
        let closed = self.emit_jump(Instruction::JumpIfTruthy(0));
        self.emit_iterator_close_checked(&iterator, span);
        self.patch_jump(closed);
        let finished = self.emit_jump(Instruction::Jump(0));

        // The landing pad. Entering it has already popped the handler and put the thrown value
        // on the stack. The close here is the forgiving flavour: an exception already on its way
        // out is not replaced by whatever the cleanup thought of the iterator.
        self.patch_jump(handler_jump);
        self.load_hidden(&done);
        let raise = self.emit_jump(Instruction::JumpIfTruthy(0));
        self.emit_iterator_close_quietly(&iterator, span);
        self.patch_jump(raise);
        self.code.push(Instruction::Throw);

        self.patch_jump(finished);
    }

    /// One `IteratorStep`, leaving the value it produced when `want_value` is set.
    ///
    /// An exhausted iterator yields `undefined` rather than stopping the walk: `var [a, b] = [1]`
    /// binds `b` to undefined, and a default on it applies.
    pub(super) fn emit_iterator_step(
        &mut self,
        iterator: &str,
        next_method: &str,
        done: &str,
        step: &str,
        want_value: bool,
    ) {
        self.load_hidden(done);
        let exhausted = self.emit_jump(Instruction::JumpIfTruthy(0));
        // Marked done *before* the call, and cleared only after a value is in hand: a `next` or
        // a `value` getter that throws leaves the iterator failed, and a failed one is never
        // closed.
        self.push_constant(Constant::Boolean(true));
        self.store_hidden(done);
        self.load_hidden(iterator);
        self.load_hidden(next_method);
        self.code.push(Instruction::NewArray { count: 0 });
        self.code.push(Instruction::Apply);
        self.guard_iterator_result();
        self.store_hidden(step);
        self.load_hidden(step);
        let finished_key = self.constant_index(Constant::String("done".to_owned().into()));
        self.code.push(Instruction::GetProperty(finished_key));
        let finished = self.emit_jump(Instruction::JumpIfTruthy(0));
        if want_value {
            self.load_hidden(step);
            let value_key = self.constant_index(Constant::String("value".to_owned().into()));
            self.code.push(Instruction::GetProperty(value_key));
        }
        self.push_constant(Constant::Boolean(false));
        self.store_hidden(done);
        let ready = self.emit_jump(Instruction::Jump(0));

        self.patch_jump(exhausted);
        self.patch_jump(finished);
        if want_value {
            self.push_constant(Constant::Undefined);
        }
        self.patch_jump(ready);
    }

    /// `...rest`: everything the iterator has left, as a fresh array.
    ///
    /// Written with an index and `SetComputed` rather than with `push`, because
    /// `Array.prototype.push` is a property a program can replace and the language defines this
    /// as a plain data property on a fresh array.
    pub(super) fn emit_iterator_drain(
        &mut self,
        iterator: &str,
        next_method: &str,
        done: &str,
        step: &str,
        span: Span,
    ) {
        let index = self.hidden_bindings;
        self.hidden_bindings += 1;
        let collected = format!("%collected{index}");
        let count = format!("%counted{index}");
        self.code.push(Instruction::NewArray { count: 0 });
        self.declare_hidden(&collected, span);
        self.push_constant(Constant::Number(0.0f64.to_bits()));
        self.declare_hidden(&count, span);

        let start = self.code.len();
        self.load_hidden(done);
        let drained = self.emit_jump(Instruction::JumpIfTruthy(0));
        self.push_constant(Constant::Boolean(true));
        self.store_hidden(done);
        self.load_hidden(iterator);
        self.load_hidden(next_method);
        self.code.push(Instruction::NewArray { count: 0 });
        self.code.push(Instruction::Apply);
        self.guard_iterator_result();
        self.store_hidden(step);
        self.load_hidden(step);
        let finished_key = self.constant_index(Constant::String("done".to_owned().into()));
        self.code.push(Instruction::GetProperty(finished_key));
        let ended = self.emit_jump(Instruction::JumpIfTruthy(0));

        self.load_hidden(&collected);
        self.load_hidden(&count);
        self.load_hidden(step);
        let value_key = self.constant_index(Constant::String("value".to_owned().into()));
        self.code.push(Instruction::GetProperty(value_key));
        // The value survived, so the iterator is unfinished again.
        self.push_constant(Constant::Boolean(false));
        self.store_hidden(done);
        self.code.push(self.set_computed_instruction());
        self.code.push(Instruction::Pop);
        self.load_hidden(&count);
        self.push_constant(Constant::Number(1.0f64.to_bits()));
        self.code.push(Instruction::Binary(BinaryOp::Add));
        self.store_hidden(&count);
        self.code.push(Instruction::Jump(start as u32));

        self.patch_jump(drained);
        self.patch_jump(ended);
        self.load_hidden(&collected);
    }

    /// `IteratorClose` on a normal completion, which is the strict flavour of it.
    ///
    /// A `return` that throws propagates, and one that answers a non-object is a `TypeError`:
    /// nothing is on its way out yet, so this failure is the program's outcome. The pad that
    /// catches an exception uses [`Compiler::emit_iterator_close`] instead, which discards both.
    pub(super) fn emit_iterator_close_checked(&mut self, binding: &str, span: Span) {
        let absent = self.emit_iterator_return(binding, span);
        self.require_object("Iterator result of `return` is not an object");
        self.code.push(Instruction::Pop);
        self.patch_jump(absent);
    }

    /// `IteratorClose` on an abrupt completion, which is the forgiving flavour of it.
    ///
    /// The iterator is given its chance to clean up and anything it has to say about that is
    /// dropped -- its result unread, and an exception out of its `return` swallowed. A value is
    /// already travelling outwards, and the language keeps the one the program threw rather than
    /// replacing it with whatever the cleanup thought.
    pub(super) fn emit_iterator_close_quietly(&mut self, binding: &str, span: Span) {
        let guard = self.emit_jump(Instruction::PushHandler(0));
        let absent = self.emit_iterator_return(binding, span);
        self.code.push(Instruction::Pop);
        self.patch_jump(absent);
        self.code.push(Instruction::PopHandler);
        let closed = self.emit_jump(Instruction::Jump(0));
        // Entering here truncated the stack back to where the guard went up, so the only thing
        // above the exception being carried out is the one the `return` raised.
        self.patch_jump(guard);
        self.code.push(Instruction::Pop);
        self.patch_jump(closed);
    }

    /// Call the iterator's `return`, leaving its result on the stack.
    ///
    /// Returns the jump taken when there is no `return` to call, which the caller patches to
    /// wherever it lands after dealing with the result: an absent `return` leaves nothing.
    ///
    /// Applied rather than called by name, because the language reads `return` once and a second
    /// read would run an accessor twice. The empty argument array is also what `return` sees as
    /// its own `arguments`, whose length one test asserts.
    pub(super) fn emit_iterator_return(&mut self, binding: &str, span: Span) -> usize {
        let name = self.constant_index(Constant::String("return".to_owned().into()));
        let held = format!("%closing{}", self.hidden_bindings);
        self.hidden_bindings += 1;
        self.load_hidden(binding);
        self.code.push(Instruction::GetProperty(name));
        self.declare_hidden(&held, span);
        self.load_hidden(&held);
        let present = self.emit_jump(Instruction::JumpIfNotNullish(0));
        let absent = self.emit_jump(Instruction::Jump(0));

        self.patch_jump(present);
        self.load_hidden(binding);
        self.load_hidden(&held);
        self.code.push(Instruction::NewArray { count: 0 });
        self.code.push(Instruction::Apply);
        absent
    }

    /// Throw a `TypeError` unless the value on top of the stack is an object, leaving it either
    /// way. `typeof` rather than a test of its own, because the two answers that mean "object"
    /// are exactly `"object"` -- excluding `null`, which shares the answer -- and `"function"`.
    pub(super) fn require_object(&mut self, message: &str) {
        self.code.push(Instruction::Dup);
        self.code.push(Instruction::Unary(UnaryOp::TypeOf));
        self.push_constant(Constant::String("object".to_owned().into()));
        self.code.push(Instruction::Binary(BinaryOp::StrictEqual));
        let nullable = self.emit_jump(Instruction::JumpIfTruthy(0));
        self.code.push(Instruction::Dup);
        self.code.push(Instruction::Unary(UnaryOp::TypeOf));
        self.push_constant(Constant::String("function".to_owned().into()));
        self.code.push(Instruction::Binary(BinaryOp::StrictEqual));
        let callable = self.emit_jump(Instruction::JumpIfTruthy(0));
        let refused = self.emit_jump(Instruction::Jump(0));

        self.patch_jump(nullable);
        self.code.push(Instruction::Dup);
        let object = self.emit_jump(Instruction::JumpIfNotNullish(0));

        self.patch_jump(refused);
        self.code.push(Instruction::LoadBuiltin(Builtin::TypeError));
        self.push_constant(Constant::String(message.to_owned().into()));
        self.code.push(Instruction::New { argc: 1 });
        self.code.push(Instruction::Throw);

        self.patch_jump(callable);
        self.patch_jump(object);
    }

    /// Take one element or property apart, given how to push the value it receives.
    ///
    /// The value is emitted by a callback rather than passed in already on the stack, because a
    /// target that stores through a reference has to be evaluated *before* it: `({ a: o.p } = s)`
    /// evaluates `o` and only then runs the getter for `a`, and which of the two throws first is
    /// observable. A nested pattern is not a reference, so `[{ a: o.p }] = xs` evaluates `o`
    /// while taking the inner object apart rather than before reading the element.
    pub(super) fn destructure_element(
        &mut self,
        leaf: PatternLeaf,
        target: &Pattern,
        span: Span,
        value: impl FnOnce(&mut Self),
    ) {
        // In declaration position a reference target is refused rather than evaluated, so
        // nothing is pushed here that the leaf below would not know to pop.
        if matches!(leaf, PatternLeaf::Assign) {
            self.reference_prologue(target);
        }
        value(self);
        self.destructure(leaf, target, span);
    }

    /// Evaluate what a target stores through, looking past any default it carries.
    pub(super) fn reference_prologue(&mut self, target: &Pattern) {
        match target {
            Pattern::Member { object, .. } | Pattern::PrivateMember { object, .. } => {
                self.expression(object);
            }
            Pattern::Computed { object, property } => {
                self.expression(object);
                self.expression(property);
            }
            // `[o.p = 1] = xs` evaluates `o` before the element, not after the default decides
            // whether one is needed: the default is part of the value, not part of the target.
            Pattern::WithDefault { inner, .. } => self.reference_prologue(inner),
            _ => {}
        }
    }

    /// Finish a leaf that stores through a reference, with the value on top of the stack and the
    /// reference under it.
    pub(super) fn store_through_reference(
        &mut self,
        leaf: PatternLeaf,
        store: Instruction,
        span: Span,
    ) {
        match leaf {
            PatternLeaf::Assign => {
                self.code.push(store);
                // The store leaves the value, which is what an assignment expression evaluates
                // to and what a pattern walk has no further use for.
                self.code.push(Instruction::Pop);
            }
            // `let [o.p] = xs` is a syntax error: a declaration introduces names, and a property
            // reference is not one. Nothing was pushed for the reference, so only the value goes.
            PatternLeaf::Declare(_) => {
                self.error(
                    Some(span),
                    "a declaration cannot bind through a property reference",
                );
                self.code.push(Instruction::Pop);
            }
        }
    }

    /// Move the value being taken apart off the stack when the walk needs the room.
    ///
    /// The temporary deliberately goes in the scope that is current, rather than in one pushed
    /// around the walk: the names the pattern binds are declared as it runs, and a scope of the
    /// walk's own would swallow them when it was popped.
    pub(super) fn pattern_source(&mut self, spilled: bool, span: Span) -> PatternSource {
        if !spilled {
            return PatternSource::Stack;
        }
        let name = format!("%source{}", self.hidden_bindings);
        self.hidden_bindings += 1;
        self.declare_hidden(&name, span);
        PatternSource::Hidden(name)
    }

    pub(super) fn load_pattern_source(&mut self, source: &PatternSource) {
        match source {
            PatternSource::Stack => self.code.push(Instruction::Dup),
            PatternSource::Hidden(name) => self.load_hidden(name),
        }
    }

    /// Read one property of the source, which is already on top of the stack.
    pub(super) fn read_key(&mut self, key: &SourceKey) {
        match key {
            SourceKey::Static(index) => self.code.push(Instruction::GetProperty(*index)),
            SourceKey::Computed(held) => {
                self.load_hidden(held);
                self.code.push(Instruction::GetComputed);
            }
        }
    }

    /// Drop the source now the walk is done with it. A spilled one is already off the stack.
    pub(super) fn finish_pattern_source(&mut self, source: &PatternSource) {
        if matches!(source, PatternSource::Stack) {
            self.code.push(Instruction::Pop);
        }
    }

    /// Declare a binding.
    ///
    /// `bound_from_stack` is for bindings the compiler introduces itself, where the value is
    /// already on the stack and the caller stores it: a `for-of` loop variable is `const` and
    /// initialised, but not by an expression this function can see. Without the distinction the
    /// const check would reject a binding that is in fact initialised.
    pub(super) fn variable_with(
        &mut self,
        kind: VariableKind,
        name: &str,
        initializer: Option<&Expression>,
        span: Span,
        bound_from_stack: bool,
    ) {
        if name.is_empty() {
            self.error(Some(span), "destructuring bindings are not supported yet");
            return;
        }
        // `var x = e` inside a `with` initialises through the object environment, because the
        // declaration and the initialisation are two different things: the *declaration* hoists to
        // the function or script scope, and the *initialisation* is an ordinary assignment, so it
        // resolves `x` against the with object first and writes there when that object has the
        // name. The hoisted binding is then left undefined -- `with (o) { var value = "v" }` makes
        // `o.value` "v" and leaves `value` undefined. Storing into the slot got both backwards.
        //
        // Only a plain `var`: a `let` or `const` inside a `with` block declares in the block's own
        // scope, which the object environment does not shadow.
        if kind == VariableKind::Var
            && !bound_from_stack
            && let Some(initializer) = initializer
        {
            let scopes = self.with_scopes_for(name);
            if !scopes.is_empty() {
                self.with_assignment(&scopes, AssignmentOperator::Assign, name, initializer, span);
                // An assignment leaves its value; a declaration is a statement and wants none.
                self.code.push(Instruction::Pop);
                return;
            }
        }
        let scope_index = if kind == VariableKind::Var {
            0
        } else {
            self.scopes.len() - 1
        };
        // A `var` at the top of global code is a property of the global object, and declaration
        // instantiation already created it. All that is left is the initializer.
        // `global_var_scope` and not merely the name being in `global_bindings`: a function
        // written in a script whose top level declares `var x` still has its own `var x`, and
        // that one is a slot of the call's.
        if self.global_var_scope
            && kind == VariableKind::Var
            && scope_index == 0
            && self.global_bindings.contains(name)
        {
            if bound_from_stack {
                // The value is on the stack and `declare_bound` emits the store, because only it
                // knows which instruction the binding needs.
                return;
            }
            // `var x;` with no initializer keeps whatever the binding already holds -- which is
            // the same rule a slot-backed `var` follows, and the reason a second `var x;` does
            // not reset it to `undefined`.
            if let Some(initializer) = initializer {
                self.expression(initializer);
                self.store_global_object_binding(name);
            }
            return;
        }
        if let Some(existing) = self.scopes[scope_index].get(name).copied() {
            // A `var` re-declaration is an assignment, and the hoisting pass has already created
            // the slot for every declaration in the block -- so reaching an existing binding is
            // the normal path now, and a *second* declaration of the same `let` is the error.
            // Two `var`s of one name are an assignment, not a redeclaration. Every other pairing
            // is an error -- including `var` after `let`, which is why the kinds of *both* are
            // consulted rather than only the one being declared now.
            let redeclaration = existing.initialized
                && !(kind == VariableKind::Var && existing.kind == VariableKind::Var);
            if redeclaration {
                self.error(
                    Some(span),
                    format!("redeclaration of local `{name}` is not supported"),
                );
                return;
            }
            if kind == VariableKind::Const && initializer.is_none() && !bound_from_stack {
                self.error(
                    Some(span),
                    format!("const local `{name}` requires an initializer"),
                );
            }
            if bound_from_stack {
                self.initialize(name);
                return;
            }
            // The initializer is compiled *before* the name leaves its dead zone, because it is
            // inside that zone: `let x = x` is a run-time `ReferenceError`, not a read of an
            // undefined slot and not a refusal of the source.
            if let Some(initializer) = initializer {
                self.expression(initializer);
            } else if kind != VariableKind::Var {
                // A `let` with no initializer is undefined; a `var` keeps what it already had,
                // which is what makes `var x = 1; var x;` leave 1 in place.
                self.push_constant(Constant::Undefined);
            } else {
                self.initialize(name);
                return;
            }
            self.initialize(name);
            self.code.push(if existing.celled {
                Instruction::StoreCell(existing.slot)
            } else {
                Instruction::StoreLocal(existing.slot)
            });
            return;
        }
        if kind == VariableKind::Const && initializer.is_none() && !bound_from_stack {
            self.error(
                Some(span),
                format!("const local `{name}` requires an initializer"),
            );
        }
        let slot = self.local_count;
        self.local_count += 1;
        // A captured binding is a cell at every scope, including module scope. `LoadGlobal`
        // reaches the currently executing module frame, which is correct only while a callable
        // stays in its declaring module. Exported functions and classes travel; their cells travel
        // with them. `predeclare` applies the same rule to hoisted bindings, and keeping this
        // non-hoisted path weaker lets hidden superclass bindings index the importer by slot.
        let celled = self.cell_everything || self.captured.contains(name);
        // A `let` or `const` whose initializer has not run yet is in its dead zone, and the
        // initializer is part of that zone. A `var` never is, and a binding the caller is about
        // to store into from the stack has no initializer of its own to wait for.
        let in_dead_zone = kind != VariableKind::Var && !bound_from_stack;
        self.scopes[scope_index].insert(
            name.to_owned(),
            Local {
                slot,
                kind,
                imported: false,
                initialized: !in_dead_zone,
                celled,
                eval_declaration: None,
            },
        );
        if celled {
            self.code.push(Instruction::NewCell(slot));
            // A captured binding can be read from a function called before the declaration runs,
            // where the compiler cannot see the read. The marker is what makes that a
            // `ReferenceError` rather than an undefined the program never wrote. Uncaptured
            // bindings use a scratch marker below when their initializer reads them directly.
            if in_dead_zone {
                self.code.push(Instruction::MarkUninitialized(slot));
            }
        }
        if bound_from_stack {
            // The value is already on the stack; the caller emits the store, because it may need
            // to know which instruction was used.
            return;
        }
        if let Some(initializer) = initializer {
            self.expression(initializer);
        } else {
            self.push_constant(Constant::Undefined);
        }
        self.initialize(name);
        self.code.push(if celled {
            Instruction::StoreCell(slot)
        } else {
            Instruction::StoreLocal(slot)
        });
    }

    /// Compile a run of statements sharing one scope, hoisting what the language hoists.
    ///
    /// `f(); function f() {}` is ordinary JavaScript, so a function declaration's value has to
    /// exist before the line that writes it. Emitting those first is not enough on its own: a
    /// hoisted body would then compile before the `const`s below it are in scope, and a closure
    /// legitimately sees those. So the block's *names* are all created first and only the
    /// function declarations' *values* are emitted early -- which is what the language does, and
    /// is where its temporal dead zone comes from.
    pub(super) fn statements(&mut self, list: &[Statement]) {
        let resources = resource_declarations(list);
        if resources.is_empty() {
            self.plain_statements(list);
            return;
        }
        self.resource_scope(list, &resources);
    }

    /// Compile a statement list whose `using` declarations have to be disposed of when control
    /// leaves it, whichever way it leaves.
    ///
    /// The shape is `try`/`finally` with no `catch`, and for the same reason: a disposal has to
    /// run on every way out, and re-emitting it at each exit is what keeps `break`, `continue`
    /// and `return` ordinary jumps. It goes on the same cleanup stack a `finally` uses, so a
    /// non-local exit crossing both runs them in the right order without either knowing about
    /// the other.
    pub(super) fn resource_scope(&mut self, list: &[Statement], resources: &[bool]) {
        let span = list
            .first()
            .map_or(Span::new(0, 0), |statement| statement.span);
        self.with_resource_scope(resources, span, |compiler| compiler.plain_statements(list));
    }

    /// Compile `body` with `resources` disposed of on every way out of it.
    pub(super) fn with_resource_scope(
        &mut self,
        resources: &[bool],
        span: Span,
        body: impl FnOnce(&mut Self),
    ) {
        let mut slots = Vec::with_capacity(resources.len());
        for awaiting in resources {
            let index = self.hidden_bindings;
            self.hidden_bindings += 1;
            let slot = ResourceSlot {
                value: format!("%resource{index}"),
                method: format!("%dispose{index}"),
                awaiting: *awaiting,
            };
            // Cleared on entry rather than only written on use, because a loop body is one scope
            // entered many times: without this, a pass that never reached a declaration would
            // dispose of what the previous pass left in the slot.
            self.push_constant(Constant::Undefined);
            self.declare_hidden(&slot.value, span);
            self.push_constant(Constant::Undefined);
            self.declare_hidden(&slot.method, span);
            slots.push(slot);
        }
        let cleanup = Cleanup::DisposeResources(slots.clone());
        self.resource_scopes.push(ResourceScope { slots, next: 0 });
        self.finalizers.push(cleanup.clone());

        let handler_jump = self.emit_jump(Instruction::PushHandler(0));
        self.handler_depth += 1;
        body(self);
        self.code.push(Instruction::PopHandler);
        self.handler_depth -= 1;

        self.finalizers.pop();
        self.emit_finalizer(&cleanup);
        let done = self.emit_jump(Instruction::Jump(0));

        // The exception is still on the stack for `Throw` to take; the disposals below it push
        // and pop in balance, so it is still on top when they are done.
        self.patch_jump(handler_jump);
        self.emit_finalizer(&cleanup);
        self.code.push(Instruction::Throw);
        self.patch_jump(done);
        self.resource_scopes
            .pop()
            .expect("this scope pushed its own resource state");
    }

    pub(super) fn plain_statements(&mut self, list: &[Statement]) {
        for statement in list {
            self.hoist_names(statement);
        }
        for statement in list {
            if let StatementKind::Function(function) = &statement.kind {
                self.initialize_function_declaration(function, statement.span);
            }
        }
        for statement in list {
            // The lexical closure already exists. Only the Annex B copy belongs at the
            // declaration's execution point, so an earlier break or return skips that copy.
            if let StatementKind::Function(function) = &statement.kind {
                self.emit_annexb_function_write(function);
            } else {
                self.statement(statement);
            }
        }
    }

    /// `with (object) body`: an object environment record around the body's scope.
    ///
    /// The object goes into a hidden binding rather than staying on the stack, because the body
    /// may create a function that outlives the statement -- and that function's names still
    /// resolve against the object when it is called. A binding is the only thing a closure can
    /// capture, so the object becomes one, and it is boxed like any other captured binding.
    pub(super) fn with_statement(&mut self, object: &Expression, body: &Statement, span: Span) {
        // Each level costs one lookup chain at every name inside it, and an assignment repeats
        // its value expression once per level. Four is far past anything written on purpose, and
        // refusing beyond it is a limit that says so rather than emitting code nobody can read.
        if self.with_scopes.len() >= 4 {
            self.error(
                Some(span),
                "unsupported statement: `with` nested more than four deep",
            );
            return;
        }
        let name = format!("%with{}", self.hidden_bindings);
        self.hidden_bindings += 1;
        self.scopes.push(HashMap::new());
        self.captured.insert(name.clone());
        self.expression(object);
        self.code.push(Instruction::ToObject);
        self.declare_bound(VariableKind::Let, &name, span);
        self.with_scopes.push(WithScope {
            name,
            functions: self.enclosing.len(),
            scopes: self.scopes.len(),
        });
        self.statement(body);
        self.with_scopes.pop().expect("just pushed");
        self.scopes.pop().expect("with scope was just pushed");
    }

    /// The `with` objects `name` has to be looked for on, innermost first.
    ///
    /// Empty for every name in a program with no `with` in it, and for a name whose binding is
    /// written *inside* the body: `with (o) { let x = 1; x }` is the `let`, whatever `o` holds,
    /// because the block's scope is nested within the object environment record.
    pub(super) fn with_scopes_for(&self, name: &str) -> Vec<WithScope> {
        if self.with_scopes.is_empty() {
            return Vec::new();
        }
        // `this` is not a binding an environment record answers for, and a compiler-introduced
        // name is not one any object can have: neither is ever looked for on a `with` object.
        if name == "this" || name.starts_with('%') || name.starts_with('*') {
            return Vec::new();
        }
        if self.eval_program && self.eval_lexical_names.contains(name) {
            return Vec::new();
        }
        let bound = self
            .scopes
            .iter()
            .rposition(|scope| scope.contains_key(name));
        let functions = self.enclosing.len();
        self.with_scopes
            .iter()
            .rev()
            .filter(|with| {
                if with.functions == functions {
                    bound.is_none_or(|at| at < with.scopes)
                } else {
                    // The `with` is in an enclosing function, so every binding this function
                    // declares -- a parameter, a local -- is written inside it and shadows it.
                    bound.is_none()
                }
            })
            .cloned()
            .collect()
    }

    /// Push the object of one active `with`.
    pub(super) fn load_with_object(&mut self, with: &WithScope) {
        match self.resolve_or_capture(&with.name) {
            Resolution::Local(local) if local.celled => {
                self.code.push(Instruction::LoadCell(local.slot))
            }
            Resolution::Local(local) => self.code.push(Instruction::LoadLocal(local.slot)),
            Resolution::Upvalue(index) => self.code.push(Instruction::LoadUpvalue(index)),
            Resolution::Global(slot) => self.code.push(Instruction::LoadGlobal(slot)),
            // A hidden binding, never a name a program could have declared on the object.
            Resolution::GlobalObject | Resolution::Unresolved => {
                self.error(None, "a `with` object went out of scope");
                self.push_constant(Constant::Undefined);
            }
        }
    }

    pub(super) fn with_scoped_load(&mut self, name: &str, fallback: impl FnOnce(&mut Self)) {
        let scopes = self.with_scopes_for(name);
        if scopes.is_empty() {
            fallback(self);
            return;
        }
        let key = self.constant_index(Constant::String(name.to_owned().into()));
        let mut found = Vec::with_capacity(scopes.len());
        for with in &scopes {
            self.load_with_object(with);
            self.code.push(Instruction::WithHas(key));
            let miss = self.emit_jump(Instruction::JumpIfFalse(0));
            self.load_with_object(with);
            self.code.push(Instruction::GetProperty(key));
            found.push(self.emit_jump(Instruction::Jump(0)));
            self.patch_jump(miss);
        }
        fallback(self);
        for jump in found {
            self.patch_jump(jump);
        }
    }

    /// Create the bindings one statement declares, without running its initializer.
    pub(super) fn hoist_names(&mut self, statement: &Statement) {
        match &statement.kind {
            // A function declaration is usable immediately; a class is not, which is the whole
            // difference between the two for hoisting purposes.
            StatementKind::Function(function) => {
                if let Some(name) = &function.name {
                    // At the top of global code a function declaration is a *var*-scoped
                    // binding -- `CreateGlobalFunctionBinding` -- and so a property of the
                    // global object. Anywhere else it is block-scoped like a `let`.
                    let kind = if self.global_var_scope && self.scopes.len() == 1 {
                        VariableKind::Var
                    } else {
                        VariableKind::Let
                    };
                    self.predeclare(kind, name, true);
                }
            }
            StatementKind::Class(class) => {
                if let Some(name) = &class.name {
                    self.predeclare(VariableKind::Let, name, false);
                }
            }
            StatementKind::Variable { kind, declarations } => {
                for declaration in declarations {
                    let mut names = Vec::new();
                    pattern_names_in_order(&declaration.pattern, &mut names);
                    for name in names {
                        // `var` is initialised to undefined when the scope is entered; `let` and
                        // `const` are not, and reading one before its declaration is an error.
                        self.predeclare(*kind, &name, *kind == VariableKind::Var);
                    }
                }
            }
            _ => {}
        }
    }

    /// Predeclare every `var` a body contains, wherever inside it the declaration is written.
    ///
    /// `hoist_names` runs once per *block*, which is right for a lexical declaration and wrong
    /// for a `var`: a `var` belongs to the function, not to the block it appears in. Its slot
    /// already went to scope 0, but the instructions `predeclare` emits went wherever the block
    /// was compiled -- so a captured `var` written inside one arm of an `if` had its `NewCell`
    /// inside that arm, and the *other* arm's write found an unboxed slot. That is
    /// `local slot 460 does not hold a captured cell`, and `import "react-dom/server"` is
    /// exactly it -- `if (typeof performance === "object") { var getCurrentTime = ... } else {
    /// getCurrentTime = ... }`, with the closure that reads `getCurrentTime` being what makes
    /// the binding boxed and the else path fatal.
    ///
    /// The initialisation is the same story told quietly: a `var` reads as `undefined` from the
    /// top of its function, not from the top of the block it was written in.
    ///
    /// Walks into every statement that shares this variable environment and into none that
    /// starts a new one, which is the specification's `VarScopedDeclarations`: a nested function
    /// owns its own `var`s, and a class body is a function's worth of scopes away.
    pub(super) fn hoist_var_names(&mut self, body: &[Statement]) {
        for statement in body {
            self.hoist_var_names_in(statement);
        }
    }

    pub(super) fn hoist_var_names_in(&mut self, statement: &Statement) {
        match &statement.kind {
            StatementKind::Variable {
                kind: VariableKind::Var,
                declarations,
            } => {
                for declaration in declarations {
                    let mut names = Vec::new();
                    pattern_names_in_order(&declaration.pattern, &mut names);
                    for name in names {
                        self.predeclare(VariableKind::Var, &name, true);
                    }
                }
            }
            StatementKind::Block(body) => self.hoist_var_names(body),
            StatementKind::If {
                consequent,
                alternate,
                ..
            } => {
                self.hoist_var_names_in(consequent);
                if let Some(alternate) = alternate {
                    self.hoist_var_names_in(alternate);
                }
            }
            StatementKind::While { body, .. }
            | StatementKind::DoWhile { body, .. }
            | StatementKind::Labelled { body, .. }
            | StatementKind::With { body, .. } => self.hoist_var_names_in(body),
            StatementKind::For { init, body, .. } => {
                if let Some(init) = init {
                    self.hoist_var_names_in(init);
                }
                self.hoist_var_names_in(body);
            }
            StatementKind::ForIn {
                kind, name, body, ..
            } => {
                if *kind == VariableKind::Var {
                    self.predeclare(VariableKind::Var, name, true);
                }
                self.hoist_var_names_in(body);
            }
            StatementKind::ForOf {
                kind,
                pattern,
                body,
                ..
            } => {
                if *kind == VariableKind::Var {
                    let mut names = Vec::new();
                    pattern_names_in_order(pattern, &mut names);
                    for name in names {
                        self.predeclare(VariableKind::Var, &name, true);
                    }
                }
                self.hoist_var_names_in(body);
            }
            StatementKind::Switch { cases, .. } => {
                for case in cases {
                    self.hoist_var_names(&case.body);
                }
            }
            StatementKind::Try {
                block,
                handler,
                finalizer,
            } => {
                self.hoist_var_names(block);
                if let Some(handler) = handler {
                    self.hoist_var_names(&handler.body);
                }
                self.hoist_var_names(finalizer);
            }
            // A function owns its own `var`s, and everything else declares nothing.
            _ => {}
        }
    }

    /// Give every block-level function declaration the var-scoped binding Annex B promises it.
    ///
    /// B.3.2.1 for a function body and B.3.3.2 for a script: in sloppy code a `FunctionDeclaration`
    /// directly inside a `Block`, a `CaseClause`, a `DefaultClause` or an `if` branch also
    /// declares a `var` of the same name in the enclosing variable environment, initialised to
    /// `undefined` and written when the declaration is evaluated. It is the reason
    /// `{ function f() {} } typeof f` is `"function"` in a browser and `"undefined"` under a
    /// reading of the main grammar alone, and bundlers still emit code that depends on it.
    ///
    /// A name is skipped when replacing the declaration with a `var` would be an early error --
    /// a `let`, `const`, `class` or `using` of the same name anywhere between the variable
    /// environment and the block, a destructured catch parameter -- and, in a function, when it
    /// is a formal parameter or `arguments`. `blocked` carries exactly those names down.
    pub(super) fn hoist_annexb_function_names(
        &mut self,
        body: &[Statement],
        parameters: &HashSet<String>,
    ) {
        if self.strict {
            return;
        }
        let mut blocked: Vec<String> = parameters.iter().cloned().collect();
        let mut names = Vec::new();
        collect_annexb_function_names(body, false, &mut blocked, &mut names);
        for (name, span) in names {
            // A `var` or a top-level function declaration of the same name already owns the
            // binding; Annex B only adds the write, never a second slot.
            self.predeclare(VariableKind::Var, &name, true);
            self.annexb_var_functions.insert(span);
        }
    }

    /// Allocate a slot for a name the block declares, leaving its value for the declaration.
    ///
    /// Silent when the name is already there: a genuine redeclaration is reported by the
    /// declaration itself, where the span points at the second one rather than at the block.
    pub(super) fn predeclare(&mut self, kind: VariableKind, name: &str, initialized: bool) {
        if name.is_empty() {
            return;
        }
        let scope_index = if kind == VariableKind::Var {
            0
        } else {
            self.scopes.len() - 1
        };
        // An outer binding is the *caller's*, borrowed into scope 0 so a direct eval can read and
        // write it. A declaration in the eval'd source either takes it over or shadows it, and
        // which of the two is the whole difference strictness makes to an eval: a sloppy `var` or
        // function declaration belongs to the caller's variable environment and so becomes that
        // binding, while a `let`, a `const`, a class, and every declaration in strict code belong
        // to the eval's own scope and get a slot of their own.
        // `EvalDeclarationInstantiation` refuses a `var` that names a binding in any record
        // between this eval's lexical environment and its variable one. A block's `let` arrives
        // as an outer binding and is caught by the redeclaration rule below; a parameter
        // environment does not, because a function's parameters and its body share one scope
        // here -- so its names travelled from the call site instead.
        if kind == VariableKind::Var
            && scope_index == 0
            && self.blocked_var_names.iter().any(|blocked| blocked == name)
        {
            self.error(
                None,
                format!(
                    "a direct `eval` may not declare `var {name}` where the enclosing parameter                      list already binds that name"
                ),
            );
            return;
        }
        if scope_index == 0 && self.outer_names.remove(name) {
            if initialized && !self.strict {
                // `var x;` over an existing binding keeps its value -- which is why this returns
                // rather than falling through to the `undefined` store below.
                return;
            }
            self.scopes[0].remove(name);
        }
        if self.scopes[scope_index].contains_key(name) {
            return;
        }
        // Global code's variable environment is the global object, so a `var` or a function
        // declaration at the top of it is a property rather than a slot. Lexical declarations
        // are not: `let` and `const` at the top of a script go in the global *declarative*
        // record, where `globalThis` cannot see them, and they keep their slots.
        if self.global_var_scope && scope_index == 0 && kind == VariableKind::Var {
            if self.global_bindings.insert(name.to_owned()) {
                let index = self.constant_index(Constant::String(name.to_owned().into()));
                self.code.push(Instruction::DeclareGlobalBinding {
                    name: index,
                    configurable: self.global_bindings_configurable,
                });
            }
            return;
        }
        let slot = self.local_count;
        self.local_count += 1;
        // Module scope boxes a captured binding exactly like any other scope. It used to be
        // exempt, on the reasoning that a function reaches a module binding through the module
        // frame -- true within one module, and wrong across two: the frame a `LoadGlobal` finds
        // belongs to whichever module is *running*, not to the one the function was written in.
        // An exported function reading its own module's `const` read the importer's slot of the
        // same number instead, which is a wrong answer rather than a missing one.
        let captured = self.captured.contains(name) || self.cell_everything;
        let celled = captured;
        self.scopes[scope_index].insert(
            name.to_owned(),
            Local {
                slot,
                kind,
                imported: false,
                initialized,
                celled,
                eval_declaration: None,
            },
        );
        // The cell has to exist before a hoisted function captures it, which is why this is here
        // rather than at the declaration.
        if celled {
            self.code.push(Instruction::NewCell(slot));
        }
        if initialized {
            // `var` reads as undefined before its declaration rather than throwing.
            self.push_constant(Constant::Undefined);
            self.code.push(if celled {
                Instruction::StoreCell(slot)
            } else {
                Instruction::StoreLocal(slot)
            });
        } else {
            // Keep the run-time state on every lexical slot, not only captured ones. A switch
            // shares one scope between all of its cases, so compiler-time `initialized` state
            // can be true after compiling one case even though another case jumps straight to a
            // read before the declaration. The marker lets the machine preserve the TDZ across
            // that control-flow join; captured cells need it for the same reason when a closure
            // is called too early.
            self.code.push(Instruction::MarkUninitialized(slot));
        }
    }

    /// Mark a name as past its dead zone, now that its declaration has run.
    pub(super) fn initialize(&mut self, name: &str) {
        for scope in self.scopes.iter_mut().rev() {
            if let Some(local) = scope.get_mut(name) {
                local.initialized = true;
                return;
            }
        }
    }

    pub(super) fn if_statement(
        &mut self,
        test: &Expression,
        consequent: &Statement,
        alternate: Option<&Statement>,
    ) {
        self.expression(test);
        let false_jump = self.emit_jump(Instruction::JumpIfFalse(0));
        self.branch_statement(consequent);
        if let Some(alternate) = alternate {
            let end_jump = self.emit_jump(Instruction::Jump(0));
            self.patch_jump(false_jump);
            self.branch_statement(alternate);
            self.patch_jump(end_jump);
        } else {
            self.patch_jump(false_jump);
        }
    }

    /// One arm of an `if`, which is a statement rather than a block -- except when it is a
    /// function declaration, which Annex B B.3.4 says to read as though a block were written
    /// around it.
    ///
    /// The block matters: without it the declaration binds in whatever scope encloses the `if`,
    /// so `function(f) { if (true) function f() {} }` overwrote the *parameter* instead of
    /// creating a binding of its own and leaving the parameter alone, which is what B.3.2.1
    /// declines to do for a formal.
    pub(super) fn branch_statement(&mut self, statement: &Statement) {
        if !matches!(statement.kind, StatementKind::Function(_)) {
            self.statement(statement);
            return;
        }
        self.scopes.push(HashMap::new());
        self.statement(statement);
        self.scopes.pop().expect("branch scope was just pushed");
    }

    /// `try`/`catch`/`finally`.
    pub(super) fn statement_always_terminates(stmt: &Statement) -> bool {
        match &stmt.kind {
            StatementKind::Return(_) | StatementKind::Throw(_) => true,
            StatementKind::Block(stmts) => Self::statements_always_terminate(stmts),
            StatementKind::If {
                consequent,
                alternate: Some(alt),
                ..
            } => {
                Self::statement_always_terminates(consequent)
                    && Self::statement_always_terminates(alt)
            }
            StatementKind::Try {
                block,
                handler,
                finalizer,
            } => {
                if !finalizer.is_empty() && Self::statements_always_terminate(finalizer) {
                    return true;
                }
                if let Some(clause) = handler {
                    Self::statements_always_terminate(block)
                        && Self::statements_always_terminate(&clause.body)
                } else {
                    false
                }
            }
            _ => false,
        }
    }

    pub(super) fn statements_always_terminate(stmts: &[Statement]) -> bool {
        stmts.iter().any(Self::statement_always_terminates)
    }

    ///
    /// The cleanup block is emitted once per exit path rather than once with a dispatcher.
    /// A `finally` has to run on every way out -- falling off the end, `return`, `break`,
    /// `continue`, and an exception -- and duplicating the block is what lets each of those stay
    /// an ordinary jump. It costs code size in exchange for no pending-completion register and
    /// no second dispatch mechanism; if the size ever matters, the shape here is what a
    /// dispatcher would replace.
    pub(super) fn try_statement(
        &mut self,
        block: &[Statement],
        handler: Option<&CatchClause>,
        finalizer: &[Statement],
        span: Span,
    ) {
        let has_finalizer = !finalizer.is_empty();
        if has_finalizer {
            self.finalizers.push(Cleanup::Finally(finalizer.to_vec()));
        }

        let handler_jump = self.emit_jump(Instruction::PushHandler(0));
        self.handler_depth += 1;

        self.scopes.push(HashMap::new());
        self.statements(block);
        self.scopes.pop().expect("try scope was just pushed");
        self.code.push(Instruction::PopHandler);
        self.handler_depth -= 1;

        // Normal completion runs the cleanup and skips the handler.
        if has_finalizer {
            self.finalizers.pop();
            self.emit_finalizer(&Cleanup::Finally(finalizer.to_vec()));
        }
        let done_jump = self.emit_jump(Instruction::Jump(0));

        // The handler is entered with the thrown value on the stack.
        // Entering the handler pops it at runtime, so the count is already back to what it was.
        self.patch_jump(handler_jump);
        if has_finalizer {
            self.finalizers.push(Cleanup::Finally(finalizer.to_vec()));
        }
        match handler {
            Some(clause) => {
                let skip_catch_jump = self.emit_jump(Instruction::JumpIfReturnSignal(0));

                self.scopes.push(HashMap::new());
                match &clause.binding {
                    Some(name) => self.declare_bound(VariableKind::Let, name, span),
                    // `catch { }` discards the value rather than binding it.
                    None => self.code.push(Instruction::Pop),
                }

                let catch_handler_jump = if has_finalizer {
                    let j = self.emit_jump(Instruction::PushHandler(0));
                    self.handler_depth += 1;
                    Some(j)
                } else {
                    None
                };

                self.statements(&clause.body);

                if catch_handler_jump.is_some() {
                    self.code.push(Instruction::PopHandler);
                    self.handler_depth -= 1;
                }

                self.scopes.pop().expect("catch scope was just pushed");
                if has_finalizer {
                    self.finalizers.pop();
                    self.emit_finalizer(&Cleanup::Finally(finalizer.to_vec()));
                }

                if let Some(j) = catch_handler_jump {
                    let catch_done_jump = self.emit_jump(Instruction::Jump(0));

                    // Catch block threw: landing pad for `catch_handler_jump`.
                    // The thrown value from catch is on the value stack.
                    self.patch_jump(j);
                    self.emit_finalizer(&Cleanup::Finally(finalizer.to_vec()));
                    if !Self::statements_always_terminate(finalizer) {
                        self.code.push(Instruction::Throw);
                    }
                    let catch_landing_done = self.emit_jump(Instruction::Jump(0));

                    // Return signal skipped catch: landing pad for `skip_catch_jump`.
                    self.patch_jump(skip_catch_jump);
                    self.emit_finalizer(&Cleanup::Finally(finalizer.to_vec()));
                    if !Self::statements_always_terminate(finalizer) {
                        self.code.push(Instruction::Throw);
                    }
                    self.patch_jump(catch_landing_done);

                    self.patch_jump(catch_done_jump);
                } else {
                    let catch_done_jump = self.emit_jump(Instruction::Jump(0));
                    self.patch_jump(skip_catch_jump);
                    self.code.push(Instruction::Throw);
                    self.patch_jump(catch_done_jump);
                }
            }
            None => {
                // `try`/`finally` with no catch: run the cleanup, then let the exception
                // continue outwards. The value is still on the stack for `Throw` to take.
                if has_finalizer {
                    self.finalizers.pop();
                    self.emit_finalizer(&Cleanup::Finally(finalizer.to_vec()));
                }
                if !Self::statements_always_terminate(finalizer) {
                    self.code.push(Instruction::Throw);
                }
            }
        }

        self.patch_jump(done_jump);
    }

    /// Emit a cleanup block inline, with the enclosing cleanups temporarily out of scope.
    ///
    /// A `return` inside a `finally` must not re-run the same `finally`, which is what would
    /// happen if it were still on the stack while its own body compiled.
    pub(super) fn emit_finalizer(&mut self, cleanup: &Cleanup) {
        match cleanup {
            Cleanup::Finally(statements) => {
                self.scopes.push(HashMap::new());
                let body = statements.clone();
                self.without_completion(|compiler| compiler.statements(&body));
                self.scopes.pop().expect("finalizer scope was just pushed");
            }
            Cleanup::CloseIterator { binding } => self.emit_iterator_close(&binding.clone()),
            Cleanup::DisposeResources(slots) => self.emit_disposals(&slots.clone()),
        }
    }

    /// Record the value a `using` declaration just bound, so the end of the scope disposes of it.
    ///
    /// The method is read here rather than at the disposal, which is what the specification's
    /// `GetDisposeMethod` says and is observable twice over: an accessor runs once, and a value
    /// with no disposal method is refused where it was declared instead of much later, when the
    /// program has already used it and cannot do anything about it.
    pub(super) fn register_resource(&mut self, name: &str, awaiting: bool, span: Span) {
        let Some(scope) = self.resource_scopes.last_mut() else {
            self.error(Some(span), "a `using` declaration outside any scope");
            return;
        };
        let Some(slot) = scope.slots.get(scope.next).cloned() else {
            self.error(
                Some(span),
                "more `using` declarations than this scope counted",
            );
            return;
        };
        scope.next += 1;
        debug_assert_eq!(slot.awaiting, awaiting);

        self.load_lexically(name);
        self.code.push(Instruction::Dup);
        let present = self.emit_jump(Instruction::JumpIfNotNullish(0));
        // `using x = null` binds and disposes of nothing, and never looks for a method: a
        // missing resource is the one case the language lets pass.
        self.code.push(Instruction::Pop);
        let done = self.emit_jump(Instruction::Jump(0));

        self.patch_jump(present);
        self.store_hidden(&slot.value);
        let dispose = self.constant_index(Constant::String("@@dispose".to_owned().into()));
        if awaiting {
            let asynchronous =
                self.constant_index(Constant::String("@@asyncDispose".to_owned().into()));
            self.load_hidden(&slot.value);
            self.code.push(Instruction::GetProperty(asynchronous));
            self.code.push(Instruction::Dup);
            let found = self.emit_jump(Instruction::JumpIfNotNullish(0));
            // `await using` falls back to the synchronous method, whose result is awaited like
            // any other -- which is what lets one object serve both forms.
            self.code.push(Instruction::Pop);
            self.load_hidden(&slot.value);
            self.code.push(Instruction::GetProperty(dispose));
            self.patch_jump(found);
        } else {
            self.load_hidden(&slot.value);
            self.code.push(Instruction::GetProperty(dispose));
        }

        self.code.push(Instruction::Dup);
        let callable = self.emit_jump(Instruction::JumpIfNotNullish(0));
        self.code.push(Instruction::Pop);
        self.emit_type_error(if awaiting {
            "the value is not asynchronously disposable"
        } else {
            "the value is not disposable"
        });
        self.patch_jump(callable);
        self.store_hidden(&slot.method);
        self.patch_jump(done);
    }

    /// Throw a `TypeError` the compiler decided on, built from the realm's own constructor.
    ///
    /// Through `LoadBuiltin` rather than by naming `TypeError`, because a program is free to
    /// shadow that name and the error a language rule raises is not the shadowing program's to
    /// choose.
    pub(super) fn emit_type_error(&mut self, message: &str) {
        self.code.push(Instruction::LoadBuiltin(Builtin::TypeError));
        self.push_constant(Constant::String(message.to_owned().into()));
        self.code.push(Instruction::New { argc: 1 });
        self.code.push(Instruction::Throw);
    }

    /// Dispose of a scope's `using` resources, in reverse declaration order.
    ///
    /// A slot whose method binding is still empty belongs to a declaration this exit never
    /// reached -- an early `return` above it, or a throw from its own initialiser -- and there is
    /// nothing to dispose of.
    pub(super) fn emit_disposals(&mut self, slots: &[ResourceSlot]) {
        for slot in slots.iter().rev() {
            self.load_hidden(&slot.method);
            let skip = self.emit_jump(Instruction::JumpIfFalse(0));
            self.load_hidden(&slot.value);
            self.load_hidden(&slot.method);
            self.code.push(Instruction::NewArray { count: 0 });
            self.code.push(Instruction::Apply);
            if slot.awaiting {
                self.code.push(Instruction::Await);
            }
            // The specification discards what a disposal returns; only whether it threw matters.
            self.code.push(Instruction::Pop);
            self.patch_jump(skip);
        }
    }

    /// `if (it.return) it.return()` against a hidden `for-of` binding.
    ///
    /// The method is optional -- an array iterator has none -- so it is tested before it is
    /// called. The binding is loaded twice rather than duplicated on the stack because the test
    /// consumes what it reads, and a reload of a local is cheaper than the shuffling that
    /// keeping the receiver around would need.
    pub(super) fn emit_iterator_close(&mut self, binding: &str) {
        let name = self.constant_index(Constant::String("return".to_owned().into()));
        self.load_hidden(binding);
        self.code.push(Instruction::GetProperty(name));
        let skip = self.emit_jump(Instruction::JumpIfFalse(0));
        self.load_hidden(binding);
        self.code.push(Instruction::CallMethod { name, argc: 0 });
        // The specification ignores the result unless it is not an object; nothing observes it
        // here, so it is discarded.
        self.code.push(Instruction::Pop);
        self.patch_jump(skip);
    }

    /// Run every cleanup between here and `depth`, innermost first.
    pub(super) fn unwind_finalizers(&mut self, depth: usize) {
        while self.finalizers.len() > depth {
            let finalizer = self
                .finalizers
                .pop()
                .expect("checked by the loop condition");
            self.emit_finalizer(&finalizer);
            self.pending_finalizers.push(finalizer);
        }
        while let Some(finalizer) = self.pending_finalizers.pop() {
            self.finalizers.push(finalizer);
        }
    }

    /// `label: body`.
    ///
    /// A label on a loop names that loop, so it is handed to the loop's own context and nothing
    /// is emitted here. On anything else it names a block, which needs a context of its own for
    /// `break label` to have somewhere to land.
    pub(super) fn labelled_statement(&mut self, label: &str, body: &Statement) {
        // A chain such as `outer: inner: while (...)` gives both labels the same iteration
        // target. Keep them together on that loop context so a continue to either label can
        // resolve there; treating the outer label as a non-loop block incorrectly rejects it.
        let mut target = body;
        let mut labels = vec![label.to_owned()];
        while let StatementKind::Labelled {
            label: nested_label,
            body: nested_body,
        } = &target.kind
        {
            labels.push(nested_label.clone());
            target = nested_body;
        }
        if matches!(
            target.kind,
            StatementKind::While { .. }
                | StatementKind::DoWhile { .. }
                | StatementKind::For { .. }
                | StatementKind::ForIn { .. }
                | StatementKind::ForOf { .. }
                | StatementKind::Switch { .. }
        ) {
            let previous = std::mem::replace(&mut self.pending_labels, labels);
            self.statement(target);
            // Restore the outer pending state even if a future target form fails to claim it.
            self.pending_labels = previous;
            return;
        }

        self.loops.push(LoopContext {
            kind: BreakableKind::Labelled,
            labels: vec![label.to_owned()],
            finalizer_depth: self.finalizers.len(),
            handler_depth: self.handler_depth,
            ..LoopContext::default()
        });
        self.statement(body);
        let context = self.loops.pop().expect("this label pushed its own context");
        let exit = self.code.len() as u32;
        for jump in context.breaks {
            self.patch_jump_to(jump, exit);
        }
    }

    /// `for (const name in object) body`.
    ///
    /// Walks a snapshot of the keys by index rather than through the iterator protocol, because
    /// there is no iterator involved: the keys are produced once and the loop steps through them.
    /// Each key is rechecked against the object before the body runs, so one the body deleted is
    /// skipped -- the reason this is not simply a `for-of` over `Object.keys(object)`.
    pub(super) fn for_in(
        &mut self,
        kind: VariableKind,
        name: &str,
        object: &Expression,
        body: &Statement,
        span: Span,
    ) {
        let subject = format!("%in{}", self.hidden_bindings);
        let keys = format!("%keys{}", self.hidden_bindings);
        let cursor = format!("%at{}", self.hidden_bindings);
        self.hidden_bindings += 1;

        self.scopes.push(HashMap::new());
        self.expression(object);
        self.declare_hidden(&subject, span);
        self.load_hidden(&subject);
        self.code.push(Instruction::EnumerateKeys);
        self.declare_hidden(&keys, span);
        self.push_constant(Constant::Number(0f64.to_bits()));
        self.declare_hidden(&cursor, span);

        let start = self.code.len();
        self.load_hidden(&cursor);
        self.load_hidden(&keys);
        let length = self.constant_index(Constant::String("length".to_owned().into()));
        self.code.push(Instruction::GetProperty(length));
        self.code.push(Instruction::Binary(BinaryOp::LessThan));
        let exit_jump = self.emit_jump(Instruction::JumpIfFalse(0));

        // Read this step's key, then advance the cursor, so a `continue` in the body cannot
        // re-read it.
        let key = format!("%key{}", self.hidden_bindings);
        self.hidden_bindings += 1;
        self.load_hidden(&keys);
        self.load_hidden(&cursor);
        self.code.push(Instruction::GetComputed);
        self.declare_hidden(&key, span);
        self.load_hidden(&cursor);
        self.push_constant(Constant::Number(1f64.to_bits()));
        self.code.push(Instruction::Binary(BinaryOp::Add));
        self.store_hidden(&cursor);

        // A key the body has already deleted is not visited.
        self.load_hidden(&key);
        self.load_hidden(&subject);
        self.code.push(Instruction::Binary(BinaryOp::In));
        let skip = self.emit_jump(Instruction::JumpIfFalse(0));

        // A fresh binding each iteration, so a closure made in the body captures that step's key.
        self.scopes.push(HashMap::new());
        self.load_hidden(&key);
        self.declare_bound(kind, name, span);

        self.loops.push(LoopContext {
            labels: std::mem::take(&mut self.pending_labels),
            finalizer_depth: self.finalizers.len(),
            handler_depth: self.handler_depth,
            ..LoopContext::default()
        });
        self.statement(body);
        let context = self.loops.pop().expect("this loop pushed its own context");
        self.scopes.pop().expect("iteration scope was just pushed");

        let continue_target = self.code.len() as u32;
        for jump in context.continues {
            self.patch_jump_to(jump, continue_target);
        }
        // A skipped key rejoins here, having already advanced the cursor.
        self.patch_jump(skip);
        self.code.push(Instruction::Jump(start as u32));

        self.patch_jump(exit_jump);
        let exit = self.code.len() as u32;
        for jump in context.breaks {
            self.patch_jump_to(jump, exit);
        }
        self.scopes.pop().expect("loop scope was just pushed");
    }

    /// `for (const name of iterable) body`.
    ///
    /// Lowered to the protocol rather than to indexed access: the subject may be any iterable,
    /// and indexing an array directly would be wrong for everything else and would miss the
    /// re-read of `length` that mutation during iteration depends on.
    ///
    /// The iterator and each step's result live in hidden locals whose names carry a character an
    /// identifier cannot contain, so they can never collide with the program's own bindings.
    pub(super) fn for_of(
        &mut self,
        kind: VariableKind,
        pattern: &Pattern,
        iterable: &Expression,
        body: &Statement,
        awaiting: bool,
        span: Span,
    ) {
        if awaiting && !self.compiling_function() && !self.module_body {
            // `for await` at top level is top-level `await` under another name, and a script is
            // not an evaluation anything can resume.
            self.error(
                Some(span),
                "top-level `for await` is only available in a module",
            );
            return;
        }
        let iterator_binding = format!("%iter{}", self.hidden_bindings);
        let result_binding = format!("%step{}", self.hidden_bindings);
        let asynchronous_binding = format!("%async{}", self.hidden_bindings);
        let next_binding = format!("%next{}", self.hidden_bindings);
        let method_binding = format!("%method{}", self.hidden_bindings);
        self.hidden_bindings += 1;

        self.scopes.push(HashMap::new());

        if awaiting {
            // `for await` prefers the async iterator and falls back to the sync one, and the two
            // are awaited in different places: an async iterator's `next` returns a promise of
            // the result, while a sync one returns the result directly and it is each *value*
            // that may be a promise. Which was found is remembered rather than re-derived.
            let subject = format!("%iterable{}", self.hidden_bindings);
            self.hidden_bindings += 1;
            self.expression(iterable);
            self.declare_hidden(&subject, span);

            // `GetMethod(obj, @@asyncIterator)` is *one* read, and the method it answers with is
            // the one that gets called. Reading the symbol to decide and then reading it again to
            // call it asked an accessor twice and let a program hand back two different methods,
            // one of which was never invoked. It also missed `null`, which `GetMethod` treats as
            // absent exactly as it treats `undefined`.
            self.load_hidden(&subject);
            self.push_constant(Constant::WellKnownSymbol(WellKnownSymbol::AsyncIterator));
            self.code.push(Instruction::GetComputed);
            self.declare_hidden(&method_binding, span);

            self.load_hidden(&method_binding);
            let use_async = self.emit_jump(Instruction::JumpIfNotNullish(0));

            self.load_hidden(&subject);
            self.push_constant(Constant::WellKnownSymbol(WellKnownSymbol::Iterator));
            self.code.push(Instruction::CallComputed { argc: 0 });
            self.push_constant(Constant::Boolean(false));
            let chosen = self.emit_jump(Instruction::Jump(0));

            self.patch_jump(use_async);
            self.load_hidden(&subject);
            self.load_hidden(&method_binding);
            self.code.push(Instruction::NewArray { count: 0 });
            self.code.push(Instruction::Apply);
            self.push_constant(Constant::Boolean(true));

            self.patch_jump(chosen);
            self.declare_hidden(&asynchronous_binding, span);
            self.declare_hidden(&iterator_binding, span);
        } else {
            // `iterable[Symbol.iterator]()`, which is what makes any iterable work rather than
            // only an array.
            self.expression(iterable);
            self.push_constant(Constant::WellKnownSymbol(WellKnownSymbol::Iterator));
            self.code.push(Instruction::CallComputed { argc: 0 });
            self.declare_hidden(&iterator_binding, span);
        }

        // `GetIteratorFromMethod` reads `next` once and the record keeps it, so every step calls
        // the method the opening captured. Reading it per step is a `Get` the language does not
        // perform: a three-element loop asked a proxy four times where it asks once, and an
        // accessor that answers differently the second time was followed rather than pinned.
        let next = self.constant_index(Constant::String("next".to_owned().into()));
        self.load_hidden(&iterator_binding);
        self.code.push(Instruction::GetProperty(next));
        self.declare_hidden(&next_binding, span);

        // One handler for the whole loop rather than one per iteration: the guarded region is
        // the same every time round, and pushing it once keeps `continue` an ordinary jump.
        let handler_jump = self.emit_jump(Instruction::PushHandler(0));

        let start = self.code.len();
        self.load_hidden(&iterator_binding);
        self.load_hidden(&next_binding);
        self.code.push(Instruction::NewArray { count: 0 });
        self.code.push(Instruction::Apply);
        if awaiting {
            // An async iterator hands back a promise of the result object; a sync one hands back
            // the object itself and has nothing to wait for here.
            self.load_hidden(&asynchronous_binding);
            let ready = self.emit_jump(Instruction::JumpIfFalse(0));
            self.code.push(Instruction::Await);
            self.patch_jump(ready);
        }
        self.guard_iterator_result();
        self.declare_hidden(&result_binding, span);

        self.load_hidden(&result_binding);
        let done = self.constant_index(Constant::String("done".to_owned().into()));
        self.code.push(Instruction::GetProperty(done));
        let exit_jump = self.emit_jump(Instruction::JumpIfTruthy(0));

        // A fresh binding each iteration, so a closure made in the body captures that step's
        // value rather than the last one.
        self.scopes.push(HashMap::new());
        self.load_hidden(&result_binding);
        let value = self.constant_index(Constant::String("value".to_owned().into()));
        self.code.push(Instruction::GetProperty(value));
        if awaiting {
            // The other half of the rule: a sync iterator's values are awaited one by one, which
            // is what makes `for await (const x of promises)` work on a plain array.
            self.load_hidden(&asynchronous_binding);
            let ready = self.emit_jump(Instruction::JumpIfTruthy(0));
            self.code.push(Instruction::Await);
            self.patch_jump(ready);
        }
        // A pattern, so `for (const [key, value] of entries)` binds both -- through the same
        // `bind_pattern` a declaration uses, defaults and nesting included.
        self.bind_pattern(kind, pattern, span);

        // Leaving the loop early has to close the iterator. Registering that as a cleanup is
        // what gets `return` right for free; `break` and an exception are handled by the two
        // landing pads below, which is why the loop's own context is recorded *above* it.
        self.finalizers.push(Cleanup::CloseIterator {
            binding: iterator_binding.clone(),
        });
        self.handler_depth += 1;
        self.loops.push(LoopContext {
            labels: std::mem::take(&mut self.pending_labels),
            finalizer_depth: self.finalizers.len(),
            handler_depth: self.handler_depth,
            ..LoopContext::default()
        });
        self.statement(body);
        let context = self.loops.pop().expect("this loop pushed its own context");
        self.handler_depth -= 1;
        self.finalizers.pop().expect("the close was just pushed");
        self.scopes.pop().expect("iteration scope was just pushed");

        // `continue` stays inside the loop, so it neither closes the iterator nor pops the guard.
        let continue_target = self.code.len() as u32;
        for jump in context.continues {
            self.patch_jump_to(jump, continue_target);
        }
        self.code.push(Instruction::Jump(start as u32));

        // Running out of values is a normal completion: the iterator closes itself, and closing
        // it again here would call `return` on an exhausted iterator.
        self.patch_jump(exit_jump);
        self.code.push(Instruction::PopHandler);
        let normal_exit = self.emit_jump(Instruction::Jump(0));

        if !context.breaks.is_empty() {
            let break_pad = self.code.len() as u32;
            for jump in context.breaks {
                self.patch_jump_to(jump, break_pad);
            }
            self.code.push(Instruction::PopHandler);
            self.emit_iterator_close(&iterator_binding);
            let leave = self.emit_jump(Instruction::Jump(0));
            self.emit_catch_pad(handler_jump, &iterator_binding);
            self.patch_jump(leave);
        } else {
            self.emit_catch_pad(handler_jump, &iterator_binding);
        }
        self.patch_jump(normal_exit);
        self.scopes.pop().expect("loop scope was just pushed");
    }

    /// The landing pad an exception thrown inside a `for-of` body reaches: close the iterator,
    /// then let the exception carry on outwards. Entering it has already popped the handler.
    pub(super) fn emit_catch_pad(&mut self, handler_jump: usize, binding: &str) {
        self.patch_jump(handler_jump);
        self.emit_iterator_close(binding);
        self.code.push(Instruction::Throw);
    }

    /// `yield* iterable`.
    ///
    /// Compiled as a loop over the iterator protocol rather than as one instruction. That is not
    /// an optimisation, it is the only way to get the semantics: each value has to be yielded
    /// *and suspended on*, and the value the caller sends must be handed to the delegate's own
    /// `next`. A single instruction cannot do that, because the run loop cannot suspend part-way
    /// through evaluating one -- an earlier attempt drained the iterable up front, which is
    /// wrong for a two-way protocol and never terminates on an infinite one.
    ///
    /// The shape is the specification's:
    ///
    /// ```text
    /// iterator = iterable[Symbol.iterator]()
    /// sent = undefined
    /// loop { step = iterator.next(sent); if step.done { break step.value } sent = yield step.value }
    /// ```
    pub(super) fn yield_delegate(&mut self, value: Option<&Expression>, span: Span) {
        let iterator_binding = format!("%delegate{}", self.hidden_bindings);
        let sent_binding = format!("%sent{}", self.hidden_bindings);
        let step_binding = format!("%step{}", self.hidden_bindings);
        let wrapped_binding = format!("%wrapped{}", self.hidden_bindings);
        let subject_binding = format!("%subject{}", self.hidden_bindings);
        let next_binding = format!("%next{}", self.hidden_bindings);
        let method_binding = format!("%method{}", self.hidden_bindings);
        self.hidden_bindings += 1;

        self.scopes.push(HashMap::new());

        // Whether each step has to be awaited, and where. Fixed at compile time by the kind of
        // generator this `yield*` was written in, exactly as `GetGeneratorKind` fixes it.
        let asynchronous = self.async_generator;
        let value_key = self.constant_index(Constant::String("value".to_owned().into()));

        match value {
            Some(value) => self.expression(value),
            None => self.push_constant(Constant::Undefined),
        }
        if asynchronous {
            // `GetIterator(value, async)`: `@@asyncIterator` decides, and `@@iterator` is only
            // reached when it is absent. A present-but-not-callable `@@asyncIterator` is a
            // TypeError raised *without* ever touching `@@iterator` -- which is what the two
            // must not share a path for.
            self.declare_hidden(&subject_binding, span);
            // Read once and kept: `GetMethod` performs a single `Get`, and the method it answers
            // with is the one that is called. Reading the symbol to decide and again to call it
            // ran an accessor twice and let a program hand back a method that was never invoked.
            self.load_hidden(&subject_binding);
            self.push_constant(Constant::WellKnownSymbol(WellKnownSymbol::AsyncIterator));
            self.code.push(Instruction::GetComputed);
            self.declare_hidden(&method_binding, span);
            self.load_hidden(&method_binding);
            let is_async = self.emit_jump(Instruction::JumpIfNotNullish(0));

            // Absent: the sync iterator, whose steps the specification wraps in an
            // async-from-sync iterator. The wrapper is not materialised -- nothing here hands
            // it to the program -- but the two places it awaits are, which is the whole of what
            // it does that the async protocol does not.
            self.load_hidden(&subject_binding);
            self.push_constant(Constant::WellKnownSymbol(WellKnownSymbol::Iterator));
            self.code.push(Instruction::CallComputed { argc: 0 });
            self.push_constant(Constant::Boolean(true));
            let chosen = self.emit_jump(Instruction::Jump(0));

            self.patch_jump(is_async);
            self.load_hidden(&subject_binding);
            self.load_hidden(&method_binding);
            self.code.push(Instruction::NewArray { count: 0 });
            self.code.push(Instruction::Apply);
            self.push_constant(Constant::Boolean(false));

            self.patch_jump(chosen);
            self.declare_hidden(&wrapped_binding, span);
            self.declare_hidden(&iterator_binding, span);
        } else {
            self.push_constant(Constant::WellKnownSymbol(WellKnownSymbol::Iterator));
            self.code.push(Instruction::CallComputed { argc: 0 });
            self.declare_hidden(&iterator_binding, span);
        }

        // The iterator record's `next`, read once when the delegate was opened. Every step calls
        // what was captured, which is what the specification does and what a re-read per step
        // could not be: `yield*` over three values asked for `next` four times.
        let next = self.constant_index(Constant::String("next".to_owned().into()));
        self.load_hidden(&iterator_binding);
        self.code.push(Instruction::GetProperty(next));
        self.declare_hidden(&next_binding, span);

        self.push_constant(Constant::Undefined);
        self.declare_hidden(&sent_binding, span);

        let start = self.code.len();
        self.load_hidden(&iterator_binding);
        self.load_hidden(&next_binding);
        self.load_hidden(&sent_binding);
        self.code.push(Instruction::NewArray { count: 1 });
        self.code.push(Instruction::Apply);
        if asynchronous {
            // An async iterator answers with a promise of the step, so the step object itself
            // is what has to be awaited. A wrapped sync iterator answers with the step directly
            // and it is each *value* that may be a promise, which is awaited further down.
            self.load_hidden(&wrapped_binding);
            let sync_step = self.emit_jump(Instruction::JumpIfTruthy(0));
            self.code.push(Instruction::Await);
            self.patch_jump(sync_step);
        }
        self.guard_iterator_result();
        self.declare_hidden(&step_binding, span);

        self.load_hidden(&step_binding);
        let done = self.constant_index(Constant::String("done".to_owned().into()));
        self.code.push(Instruction::GetProperty(done));
        let not_done = self.emit_jump(Instruction::JumpIfFalse(0));

        // Done: load step.value and jump to finished
        self.load_hidden(&step_binding);
        self.code.push(Instruction::GetProperty(value_key));
        let finished_jump = self.emit_jump(Instruction::Jump(0));

        // Not done: yield the inner step object out directly, delegating return and throw completions.
        self.patch_jump(not_done);
        if asynchronous {
            self.load_hidden(&wrapped_binding);
        } else {
            self.push_constant(Constant::Boolean(false));
        }
        self.load_hidden(&iterator_binding);
        self.load_hidden(&step_binding);
        let yield_star_idx = self.emit_jump(Instruction::YieldStar(0));
        self.store_hidden(&sent_binding);
        self.code.push(Instruction::Jump(start as u32));

        // Finished: either normal completion from done step, or inner throw() returning done: true.
        // The final value of yield* is left on top of the stack.
        let finished_target = self.code.len() as u32;
        self.patch_jump(finished_jump);
        self.patch_jump_to(yield_star_idx, finished_target);
        if asynchronous {
            self.await_wrapped_value(&wrapped_binding);
        }

        self.scopes.pop().expect("delegate scope was just pushed");
    }

    /// Await the value on top of the stack, but only when the delegate is a wrapped sync
    /// iterator.
    ///
    /// `AsyncFromSyncIteratorContinuation` resolves each value before it becomes the step, so a
    /// sync iterator of promises yielded through an async `yield*` hands on their values. A real
    /// async iterator has already settled its step by this point and awaiting again would cost a
    /// tick the specification does not spend.
    pub(super) fn await_wrapped_value(&mut self, wrapped_binding: &str) {
        self.load_hidden(wrapped_binding);
        let plain = self.emit_jump(Instruction::JumpIfFalse(0));
        self.code.push(Instruction::Await);
        self.patch_jump(plain);
    }

    /// Throw a `TypeError` unless the value on top of the stack is an object.
    ///
    /// `IteratorNext` requires the result to be an Object, and the two compiled walks -- `for...of`
    /// and `yield*` -- read `done` off it with `GetProperty`. A primitive answers `undefined`
    /// there, which is falsy, which reads as "not done": `for (const x of { [Symbol.iterator]:
    /// () => ({ next: () => 1 }) })` looped for ever, with no diagnostic and no way out. A hang
    /// is the worst answer available, and the one shape that gives a program no thread to pull.
    ///
    /// Emitted rather than made an instruction, because an instruction has to be implemented in
    /// the C backend too, and everything this needs already exists there.
    ///
    /// The value is left on the stack.
    pub(super) fn guard_iterator_result(&mut self) {
        // `typeof v === "object"` is true for null as well, so the null case is excluded by the
        // second test rather than by the first.
        self.code.push(Instruction::Dup);
        self.code.push(Instruction::Unary(UnaryOp::TypeOf));
        self.push_constant(Constant::String("object".to_owned().into()));
        self.code.push(Instruction::Binary(BinaryOp::StrictEqual));
        let object_like = self.emit_jump(Instruction::JumpIfTruthy(0));
        self.code.push(Instruction::Dup);
        self.code.push(Instruction::Unary(UnaryOp::TypeOf));
        self.push_constant(Constant::String("function".to_owned().into()));
        self.code.push(Instruction::Binary(BinaryOp::StrictEqual));
        let callable = self.emit_jump(Instruction::JumpIfTruthy(0));
        self.emit_iterator_result_type_error();
        self.patch_jump(object_like);
        self.patch_jump(callable);
        // `typeof null` is "object", so the null case reaches here and is rejected separately.
        self.code.push(Instruction::Dup);
        self.push_constant(Constant::Null);
        self.code.push(Instruction::Binary(BinaryOp::StrictEqual));
        let not_null = self.emit_jump(Instruction::JumpIfFalse(0));
        self.emit_iterator_result_type_error();
        self.patch_jump(not_null);
    }

    /// `return v` from a *derived* constructor, which `[[Construct]]` does not simply hand back.
    ///
    /// An object is the override and wins outright. `undefined` yields `this` instead -- and
    /// reading `this` is what makes returning before `super()` a ReferenceError, exactly as the
    /// bare `return` above relies on. Anything else is a TypeError: the specification allows a
    /// derived constructor to replace its instance with an object, not to answer a primitive.
    /// `null` is a primitive here, which is why the `typeof` test alone will not do.
    ///
    /// Emitted rather than made an instruction, for the reason `this_binding` itself is a
    /// binding: the C backend implements nothing new, so the two engines cannot disagree.
    pub(super) fn guard_derived_return(&mut self, binding: &str) {
        self.code.push(Instruction::Dup);
        self.code.push(Instruction::Unary(UnaryOp::TypeOf));
        self.push_constant(Constant::String("function".to_owned().into()));
        self.code.push(Instruction::Binary(BinaryOp::StrictEqual));
        let callable = self.emit_jump(Instruction::JumpIfTruthy(0));

        self.code.push(Instruction::Dup);
        self.code.push(Instruction::Unary(UnaryOp::TypeOf));
        self.push_constant(Constant::String("object".to_owned().into()));
        self.code.push(Instruction::Binary(BinaryOp::StrictEqual));
        let primitive = self.emit_jump(Instruction::JumpIfFalse(0));

        // `typeof null` is "object", so null reaches here and falls through to the primitive
        // handling below rather than counting as the object override.
        self.code.push(Instruction::Dup);
        self.push_constant(Constant::Null);
        self.code.push(Instruction::Binary(BinaryOp::StrictEqual));
        let object_like = self.emit_jump(Instruction::JumpIfFalse(0));

        self.patch_jump(primitive);
        self.code.push(Instruction::Dup);
        self.push_constant(Constant::Undefined);
        self.code.push(Instruction::Binary(BinaryOp::StrictEqual));
        let refused = self.emit_jump(Instruction::JumpIfFalse(0));
        self.code.push(Instruction::Pop);
        self.load_this_binding(binding);
        let yielded_this = self.emit_jump(Instruction::Jump(0));

        self.patch_jump(refused);
        self.code.push(Instruction::LoadBuiltin(Builtin::TypeError));
        self.push_constant(Constant::String(
            "a derived constructor may only return an object or undefined"
                .to_owned()
                .into(),
        ));
        self.code.push(Instruction::New { argc: 1 });
        self.code.push(Instruction::Throw);

        self.patch_jump(callable);
        self.patch_jump(object_like);
        self.patch_jump(yielded_this);
    }

    /// A static class element may not be named `prototype`, which is a TypeError rather than a
    /// quiet overwrite: the constructor's `prototype` is what its instances inherit from, and a
    /// field that replaced it would leave every instance of the class descending from something
    /// the class never made.
    ///
    /// The name is converted first, so a key object whose `toString` answers "prototype" is
    /// refused for the same reason a literal one is. The constructor is already beneath the key
    /// on the stack, which is the shape `ToPropertyKey` reads.
    pub(super) fn guard_static_prototype_key(&mut self) {
        self.code.push(Instruction::ToPropertyKey);
        self.code.push(Instruction::Dup);
        self.push_constant(Constant::String("prototype".to_owned().into()));
        self.code.push(Instruction::Binary(BinaryOp::StrictEqual));
        let usable = self.emit_jump(Instruction::JumpIfFalse(0));
        self.code.push(Instruction::LoadBuiltin(Builtin::TypeError));
        self.push_constant(Constant::String(
            "a static class element may not be named `prototype`"
                .to_owned()
                .into(),
        ));
        self.code.push(Instruction::New { argc: 1 });
        self.code.push(Instruction::Throw);
        self.patch_jump(usable);
    }

    /// Read a hidden binding that an inner function captured, the way the home object is read.
    ///
    /// `load_hidden` resolves a local and nothing else, which is right where the binding and the
    /// reader are the same function. A private method is made where the class is defined and
    /// installed from inside the constructor, so the read crosses a function boundary and has to
    /// go through `resolve_or_capture`.
    pub(super) fn load_captured_binding(&mut self, binding: &str) {
        match self.resolve_or_capture(binding) {
            Resolution::Local(local) if local.celled => {
                self.code.push(Instruction::LoadCell(local.slot));
            }
            Resolution::Local(local) => self.code.push(Instruction::LoadLocal(local.slot)),
            Resolution::Upvalue(index) => self.code.push(Instruction::LoadUpvalue(index)),
            Resolution::Global(slot) => self.code.push(Instruction::LoadGlobal(slot)),
            Resolution::GlobalObject | Resolution::Unresolved => {
                self.error(None, format!("`{binding}` went out of scope"));
                self.push_constant(Constant::Undefined);
            }
        }
    }

    /// `throw new TypeError("...")`, spelled in the instructions that already exist.
    pub(super) fn emit_iterator_result_type_error(&mut self) {
        self.code.push(Instruction::LoadBuiltin(Builtin::TypeError));
        self.push_constant(Constant::String(
            "iterator result is not an object".to_owned().into(),
        ));
        self.code.push(Instruction::New { argc: 1 });
        self.code.push(Instruction::Throw);
    }

    pub(super) fn store_hidden(&mut self, name: &str) {
        match self.resolve(name) {
            Resolution::Local(local) if local.celled => {
                self.code.push(Instruction::StoreCell(local.slot))
            }
            Resolution::Local(local) => self.code.push(Instruction::StoreLocal(local.slot)),
            _ => self.error(None, format!("`{name}` went out of scope")),
        }
    }

    /// Bind a hidden local to the value on top of the stack.
    ///
    /// Reuses the slot when the name is already bound in this scope, because a compiler-
    /// introduced binding inside a loop body is declared once and assigned on every pass.
    pub(super) fn declare_hidden(&mut self, name: &str, span: Span) {
        if matches!(self.resolve(name), Resolution::Local(_)) {
            self.store_hidden(name);
            return;
        }
        self.declare_bound(VariableKind::Let, name, span);
    }

    /// Declare `name` and store the value on top of the stack into it.
    pub(super) fn declare_bound(&mut self, kind: VariableKind, name: &str, span: Span) {
        self.variable_with(kind, name, None, span, true);
        match self.resolve(name) {
            Resolution::Local(local) if local.celled => {
                self.code.push(Instruction::StoreCell(local.slot))
            }
            Resolution::Local(local) => self.code.push(Instruction::StoreLocal(local.slot)),
            Resolution::GlobalObject => self.store_global_object_binding(name),
            _ => self.error(Some(span), format!("could not bind `{name}`")),
        }
    }

    pub(super) fn load_hidden(&mut self, name: &str) {
        match self.resolve(name) {
            Resolution::Local(local) if local.celled => {
                self.code.push(Instruction::LoadCell(local.slot))
            }
            Resolution::Local(local) => self.code.push(Instruction::LoadLocal(local.slot)),
            _ => self.error(None, format!("`{name}` went out of scope")),
        }
    }

    /// `for (init; test; update) body`.
    ///
    /// The initialiser gets its own scope so a `let` there belongs to the loop rather than to
    /// the enclosing block, and `continue` jumps to the update rather than to the test -- which
    /// is the difference a `while` desugaring would silently lose.
    ///
    /// A `let` in the head is also re-bound on every pass, which is what makes
    /// `for (let i = 0; i < 3; i++) fs.push(() => i)` collect three closures over 0, 1 and 2
    /// rather than three over the value the loop ended on.
    pub(super) fn for_statement(
        &mut self,
        init: Option<&Statement>,
        test: Option<&Expression>,
        update: Option<&Expression>,
        body: &Statement,
        span: Span,
    ) {
        // A `using` in a `for` head belongs to the loop rather than to one pass through it: the
        // head runs once, and what it declares is disposed of when the loop is left -- by the
        // test failing, by a `break`, by a `return`, or by a throw.
        let resources = init.map_or_else(Vec::new, |init| {
            resource_declarations(std::slice::from_ref(init))
        });
        if resources.is_empty() {
            self.for_statement_body(init, test, update, body);
            return;
        }
        self.with_resource_scope(&resources, span, |compiler| {
            compiler.for_statement_body(init, test, update, body);
        });
    }

    pub(super) fn for_statement_body(
        &mut self,
        init: Option<&Statement>,
        test: Option<&Expression>,
        update: Option<&Expression>,
        body: &Statement,
    ) {
        self.scopes.push(HashMap::new());
        if let Some(init) = init {
            // The head of a `for` is not a statement of the program, whatever it is spelled as
            // here: `eval("for (false; false; false);")` is `undefined`, not `false`. Only the
            // body can contribute a completion value.
            self.without_completion(|compiler| compiler.statement(init));
        }
        // Only a captured binding can tell one iteration's copy from another's, and only a
        // captured binding is boxed -- so the boxed `let`s of the head are exactly the set that
        // needs re-binding. Sorted by slot because the scope is a hash map and the bytecode is a
        // cache key: the same source must always produce the same instructions.
        let mut per_iteration: Vec<u32> = self
            .scopes
            .last()
            .expect("the loop scope was just pushed")
            .values()
            .filter(|local| local.celled && !matches!(local.kind, VariableKind::Var))
            .map(|local| local.slot)
            .collect();
        per_iteration.sort_unstable();

        let start = self.code.len();
        let exit_jump = test.map(|test| {
            self.expression(test);
            self.emit_jump(Instruction::JumpIfFalse(0))
        });

        self.loops.push(LoopContext {
            labels: std::mem::take(&mut self.pending_labels),
            finalizer_depth: self.finalizers.len(),
            handler_depth: self.handler_depth,
            ..LoopContext::default()
        });
        self.statement(body);
        let context = self.loops.pop().expect("this loop pushed its own context");

        // `continue` lands here rather than at the test, so the update still runs.
        let update_target = self.code.len() as u32;
        for jump in context.continues {
            self.patch_jump_to(jump, update_target);
        }
        // Re-bind before the update, not after: the update belongs to the *next* iteration, so
        // it must write the next iteration's cell and leave this one holding what the body saw.
        for slot in per_iteration {
            self.code.push(Instruction::LoadCell(slot));
            self.code.push(Instruction::NewCell(slot));
            self.code.push(Instruction::StoreCell(slot));
        }
        if let Some(update) = update {
            self.expression(update);
            self.code.push(Instruction::Pop);
        }
        self.code.push(Instruction::Jump(start as u32));

        if let Some(exit_jump) = exit_jump {
            self.patch_jump(exit_jump);
        }
        let exit = self.code.len() as u32;
        for jump in context.breaks {
            self.patch_jump_to(jump, exit);
        }
        self.scopes.pop().expect("loop scope was just pushed");
    }

    /// `break` or `continue`, recorded for the enclosing loop to patch once its shape is known.
    pub(super) fn jump_out(&mut self, label: Option<&str>, is_break: bool, span: Span) {
        // Only the cleanups entered since the target began: an outer `finally` is not left by
        // this jump, so running it here would run it twice.
        //
        // Unlabelled, `break` leaves the innermost breakable and `continue` steps over any
        // `switch` in the way, because a `switch` is not something a `continue` can continue.
        // Labelled, both look for that label instead -- which is the point of a label, and is
        // what lets one `break` leave several loops at once.
        let target = match label {
            None if is_break => self.loops.len().checked_sub(1),
            None => self
                .loops
                .iter()
                .rposition(|context| context.kind == BreakableKind::Loop),
            Some(label) => self.loops.iter().rposition(|context| {
                context.labels.iter().any(|candidate| candidate == label)
                    && (is_break || context.kind == BreakableKind::Loop)
            }),
        };
        if target.is_none()
            && let Some(label) = label
        {
            // The label is not there, or a `continue` named something that is not a loop.
            self.error(
                Some(span),
                format!("no enclosing loop or statement labelled `{label}`"),
            );
            return;
        }
        let (depth, handler_depth) = target.map_or((0, 0), |index| {
            let context = &self.loops[index];
            (context.finalizer_depth, context.handler_depth)
        });
        // Before the cleanups, not after: a `finally` reached by `break` must not still be
        // guarded by the handler of the `try` it belongs to, or an exception it raises would be
        // caught by its own `catch`.
        for _ in handler_depth..self.handler_depth {
            self.code.push(Instruction::PopHandler);
        }
        self.unwind_finalizers(depth);

        let jump = self.emit_jump(Instruction::Jump(0));
        let Some(context) = target.map(|index| &mut self.loops[index]) else {
            self.error(
                Some(span),
                if is_break {
                    "`break` outside of a loop"
                } else {
                    "`continue` outside of a loop"
                },
            );
            return;
        };
        if is_break {
            context.breaks.push(jump);
        } else {
            context.continues.push(jump);
        }
    }

    pub(super) fn patch_jump_to(&mut self, jump: usize, target: u32) {
        match &mut self.code[jump] {
            Instruction::Jump(slot)
            | Instruction::JumpIfFalse(slot)
            | Instruction::JumpIfTruthy(slot)
            | Instruction::JumpIfNotNullish(slot)
            // A handler's target is patched the same way a jump's is: it is not known until the
            // handler body has been emitted.
            | Instruction::PushHandler(slot)
            | Instruction::YieldStar(slot)
            | Instruction::JumpIfReturnSignal(slot) => *slot = target,
            other => unreachable!("patched a {other:?}, which is not a jump"),
        }
    }

    pub(super) fn while_statement(&mut self, test: &Expression, body: &Statement) {
        let loop_start = self.code.len();
        self.expression(test);
        let exit_jump = self.emit_jump(Instruction::JumpIfFalse(0));

        self.loops.push(LoopContext {
            labels: std::mem::take(&mut self.pending_labels),
            finalizer_depth: self.finalizers.len(),
            handler_depth: self.handler_depth,
            ..LoopContext::default()
        });
        self.statement(body);
        let context = self.loops.pop().expect("this loop pushed its own context");

        // `continue` re-tests, which for a `while` is the top of the loop.
        for jump in context.continues {
            self.patch_jump_to(jump, loop_start as u32);
        }
        self.code.push(Instruction::Jump(loop_start as u32));
        self.patch_jump(exit_jump);
        let exit = self.code.len() as u32;
        for jump in context.breaks {
            self.patch_jump_to(jump, exit);
        }
    }

    /// `do body while (test)`, whose body always runs once before the test is reached.
    pub(super) fn do_while_statement(&mut self, body: &Statement, test: &Expression) {
        let loop_start = self.code.len();

        self.loops.push(LoopContext {
            labels: std::mem::take(&mut self.pending_labels),
            finalizer_depth: self.finalizers.len(),
            handler_depth: self.handler_depth,
            ..LoopContext::default()
        });
        self.statement(body);
        let context = self.loops.pop().expect("this loop pushed its own context");

        // `continue` skips the rest of the body but still runs the test, so it lands here rather
        // than at the top -- the one place a do-while differs from a while for a jump.
        let test_target = self.code.len() as u32;
        for jump in context.continues {
            self.patch_jump_to(jump, test_target);
        }
        self.expression(test);
        let repeat = self.emit_jump(Instruction::JumpIfTruthy(0));
        self.patch_jump_to(repeat, loop_start as u32);

        let exit = self.code.len() as u32;
        for jump in context.breaks {
            self.patch_jump_to(jump, exit);
        }
    }

    /// `switch (discriminant) { case … }`.
    ///
    /// Two passes over the arms, because a `switch` is not a set of alternatives: the tests are
    /// evaluated in order until one matches with `===`, and control then enters that arm's body
    /// and *keeps going* through the arms after it until a `break`. So the tests are emitted as a
    /// dispatch chain and the bodies as one straight run entered at different offsets.
    ///
    /// `default` is not special beyond being the arm with no test: it may sit anywhere among
    /// them, it is only reached once every test has failed, and it falls through like the rest.
    pub(super) fn switch_statement(
        &mut self,
        discriminant: &Expression,
        cases: &[SwitchCase],
        span: Span,
    ) {
        // The discriminant is evaluated once, however many tests are compared against it.
        let subject = format!("%switch{}", self.hidden_bindings);
        self.hidden_bindings += 1;
        self.scopes.push(HashMap::new());
        self.expression(discriminant);
        self.declare_hidden(&subject, span);

        // Every arm shares the switch's scope, so the hoisting pass spans all of them -- a
        // function declared in one case is callable from another, which is what one scope means.
        //
        // Emitted here, before the dispatch chain, because the chain's jumps land *inside* the
        // bodies: a prologue placed after them is skipped by every path that reaches an arm.
        // That is invisible while hoisting only allocates slots, and is a wrong answer the moment
        // it emits anything -- a `NewCell` for a captured declaration, or the dead-zone marker a
        // `let` in a case needs.
        for case in cases {
            for statement in &case.body {
                self.hoist_names(statement);
            }
        }
        // CaseBlock declaration instantiation initializes every function before any case test
        // runs, including declarations in unselected clauses. Annex B's outer var write is a
        // separate operation that occurs only when execution reaches the declaration below.
        for case in cases {
            for statement in &case.body {
                if let StatementKind::Function(function) = &statement.kind {
                    self.initialize_function_declaration(function, statement.span);
                }
            }
        }

        let mut entries = Vec::with_capacity(cases.len());
        let mut fallback = None;
        for case in cases {
            let Some(test) = &case.test else {
                // Emitted with the other bodies below; only its entry point is recorded here.
                fallback = Some(entries.len());
                entries.push(None);
                continue;
            };
            self.load_hidden(&subject);
            self.expression(test);
            self.code.push(Instruction::Binary(BinaryOp::StrictEqual));
            entries.push(Some(self.emit_jump(Instruction::JumpIfTruthy(0))));
        }
        // No test matched, so control goes to `default` if there is one and past the whole
        // statement if there is not.
        let no_match = self.emit_jump(Instruction::Jump(0));

        self.loops.push(LoopContext {
            kind: BreakableKind::Switch,
            labels: std::mem::take(&mut self.pending_labels),
            finalizer_depth: self.finalizers.len(),
            handler_depth: self.handler_depth,
            ..LoopContext::default()
        });
        let mut starts = Vec::with_capacity(cases.len());
        for case in cases {
            starts.push(self.code.len() as u32);
            for statement in &case.body {
                if let StatementKind::Function(function) = &statement.kind {
                    self.emit_annexb_function_write(function);
                } else {
                    self.statement(statement);
                }
            }
        }
        let context = self
            .loops
            .pop()
            .expect("this switch pushed its own context");

        for (entry, start) in entries.iter().zip(&starts) {
            if let Some(jump) = entry {
                self.patch_jump_to(*jump, *start);
            }
        }
        match fallback.and_then(|index| starts.get(index)) {
            Some(start) => self.patch_jump_to(no_match, *start),
            None => self.patch_jump(no_match),
        }

        let exit = self.code.len() as u32;
        // A `default` in the middle still falls through, so this is the only jump that leaves
        // early -- and it is the same jump `break` uses.
        for jump in context.breaks {
            self.patch_jump_to(jump, exit);
        }
        self.scopes.pop().expect("switch scope was just pushed");
    }
}
