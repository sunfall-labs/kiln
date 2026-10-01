use super::*;

impl super::Compiler {
    /// Push a brand resolved earlier, for the access instruction that follows.
    pub(super) fn push_private_brand(&mut self, brand: &Option<String>) {
        match brand {
            Some(binding) => self.load_captured_binding(binding),
            None => self.push_constant(Constant::Undefined),
        }
    }

    /// Push the brand of the class declaring `name`, for the access instruction that follows.
    pub(super) fn load_private_brand(&mut self, name: &str) {
        match self.private_brand(name) {
            Some(binding) => self.load_captured_binding(&binding),
            // A private name outside any class is a syntax error reported elsewhere; the access
            // still needs a value here so the stack stays balanced.
            None => self.push_constant(Constant::Undefined),
        }
    }

    /// The brand key of the class currently being compiled, for the store that installs it.
    pub(super) fn own_private_brand_key(&self) -> String {
        let depth = self.private_scopes.len().saturating_sub(1);
        private_key(
            self.private_scope_modules
                .get(depth)
                .map_or(&self.private_module_id, String::as_str),
            self.private_scopes.get(depth).copied().unwrap_or(0),
            "%brand",
        )
    }

    /// Read `name` from the bindings around it, ignoring any `with` object.
    ///
    /// The whole of Kiln's ordinary identifier resolution: a slot, a cell, an upvalue, a module
    /// slot, or -- when nothing bound it -- a built-in or a `ReferenceError` where the read is.
    pub(super) fn load_lexically(&mut self, name: &str) {
        match self.resolve_or_capture(name) {
            // Reading a binding whose declaration has not run yet is the temporal dead zone.
            // Keep the read in the instruction stream: if control never reaches it, compilation
            // succeeds, and if it does, the marker produces the language's `ReferenceError`.
            // A nested function reading the same name resolves as an upvalue and checks the
            // shared cell when it runs.
            Resolution::Local(local) if !local.initialized && local.celled && self.eval_program => {
                self.code.push(Instruction::LoadCell(local.slot))
            }
            // Any read of a binding whose declaration has not run yet: `x; let x = 1`,
            // `let x = x`, or a parameter read from another parameter's default in
            // `function f(x = y, y) {}`. The check has to be a *run-time* `ReferenceError`,
            // because the source is well formed and only reaching the read fails -- exactly what
            // `assert.throws(ReferenceError, () => f())` asks for. A cell that is uninitialised by
            // construction is that check, spelled with instructions that already exist rather
            // than with an opcode of its own; a scratch one rather than the binding's own,
            // because a plain local has no cell at this point in the code and an uncelled slot is
            // not one to load from.
            Resolution::Local(local) if !local.initialized => {
                let _ = local;
                let scratch = self.local_count;
                self.local_count += 1;
                self.code.push(Instruction::NewCell(scratch));
                self.code.push(Instruction::MarkUninitialized(scratch));
                self.code.push(Instruction::LoadCell(scratch));
            }
            Resolution::Local(local) if local.celled => {
                self.code.push(Instruction::LoadCell(local.slot))
            }
            Resolution::Local(local) => self.code.push(Instruction::LoadLocal(local.slot)),
            Resolution::Global(slot) => self.code.push(Instruction::LoadGlobal(slot)),
            Resolution::Upvalue(index) => self.code.push(Instruction::LoadUpvalue(index)),
            Resolution::GlobalObject => self.load_global_object_binding(name),
            Resolution::Unresolved if name == "this" => self.code.push(Instruction::LoadThis),
            // `arguments` is not a declared binding, so it only resolves when nothing else
            // bound the name -- a parameter or local called `arguments` shadows it, as it
            // does in JavaScript.
            Resolution::Unresolved if name == "arguments" && self.compiling_function() => {
                self.code.push(Instruction::LoadArguments)
            }
            Resolution::Unresolved if name == "undefined" => {
                self.push_constant(Constant::Undefined)
            }
            // A built-in is only reachable when nothing else bound the name, so a program
            // that declares its own `Error` shadows it exactly as it would any other global.
            Resolution::Unresolved => match Builtin::from_name(name) {
                Some(builtin) => self.code.push(Instruction::LoadBuiltin(builtin)),
                // A `ReferenceError` where the read happens, which is what JavaScript does
                // and what lets a module compile when a branch it never takes mentions a
                // global this realm does not provide. See ADR 0017.
                None => {
                    let name = self.constant_index(Constant::String(name.to_owned().into()));
                    self.code.push(Instruction::ThrowReference(name));
                }
            },
        }
    }

    /// Evaluate a function declaration whose lexical value was initialized on scope entry.
    pub(super) fn emit_annexb_function_write(&mut self, function: &Function) {
        if let Some(name) = &function.name
            && let Resolution::Local(local) = self.resolve(name)
        {
            self.emit_annexb_var_write(name, local);
        }
    }

    /// The var-scoped write Annex B adds to a block-level function declaration's evaluation.
    ///
    /// Emitted after the block-scoped binding has been given its value, and reading that binding
    /// rather than rebuilding the closure: the specification copies what the block holds, so the
    /// two bindings start as the same function object and diverge from there --
    /// `{ function f() {} f = 123; }` leaves the outer `f` a function.
    pub(super) fn emit_annexb_var_write(&mut self, name: &str, block: Local) {
        if !self.annexb_var_functions.contains(name) {
            return;
        }
        let Some(target) = self.scopes[0].get(name).copied() else {
            // The var-scoped binding is a property of the global object, so the copy is a write
            // to it rather than to a slot.
            if self.global_var_scope && self.global_bindings.contains(name) {
                self.code.push(if block.celled {
                    Instruction::LoadCell(block.slot)
                } else {
                    Instruction::LoadLocal(block.slot)
                });
                self.store_global_object_binding(name);
            }
            return;
        };
        // The declaration is at the top level of the variable environment after all, so the
        // binding it just wrote *is* the var-scoped one.
        if target.slot == block.slot {
            return;
        }
        self.code.push(if block.celled {
            Instruction::LoadCell(block.slot)
        } else {
            Instruction::LoadLocal(block.slot)
        });
        self.code.push(if target.celled {
            Instruction::StoreCell(target.slot)
        } else {
            Instruction::StoreLocal(target.slot)
        });
    }

    /// The bindings a direct `eval` written here can see, innermost first.
    ///
    /// Every one is handed over as a *cell*, so the eval'd source shares the binding rather than
    /// a copy of its value: `function f() { var x = 1; eval("x = 2"); return x; }` is 2, and a
    /// copy would make it 1. That is what [`Compiler::cell_everything`] guarantees -- a function
    /// containing an eval boxes all of its bindings, and so does every function around it.
    ///
    /// Resolving each name is what *captures* it, so an enclosing function's binding gains its
    /// upvalue here rather than at a use the eval'd source has not been written yet.
    pub(super) fn eval_site_bindings(&mut self, span: Span) -> Vec<EvalBinding> {
        // Names first: resolving one mutates the upvalue lists, which cannot happen while the
        // scopes it reads are borrowed. Innermost scope first, so a shadowed name never wins;
        // sorted within a scope, because two bindings of one scope cannot shadow each other and
        // a hash map's order would otherwise make compilation non-reproducible.
        // The declaring `Local` is taken here too. An upvalue cannot say how its binding was
        // declared -- a capture carries no kind -- and the eval'd source needs to know: `var x`
        // over a caller's `var` is that binding, and over a caller's `let` it is a SyntaxError.
        let mut order: Vec<(String, EvalDeclaration)> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
        let levels = self.scopes.iter().rev().chain(
            self.enclosing
                .iter()
                .rev()
                .flat_map(|level| level.scopes.iter().rev()),
        );
        for scope in levels {
            let mut names: Vec<&String> = scope.keys().collect();
            names.sort();
            for name in names {
                // A compiler-introduced binding carries a character an identifier cannot, so it
                // is never a name the eval'd source could write.
                if name.starts_with('%') || !seen.insert(name.clone()) {
                    continue;
                }
                let local = scope[name];
                // An import is the exporter's binding, and writing one is refused wherever it is
                // written. `const` is the nearest thing this vocabulary has to that.
                let declaration = match local.eval_declaration {
                    Some(declaration) => declaration,
                    None => match local.kind {
                        _ if local.imported => EvalDeclaration::Const,
                        VariableKind::Var => EvalDeclaration::Var,
                        VariableKind::Let => EvalDeclaration::Let,
                        // A `using` binding cannot be reassigned either, and what an eval'd source
                        // may do with it is exactly what it may do with a `const`.
                        VariableKind::Const | VariableKind::Using | VariableKind::AwaitUsing => {
                            EvalDeclaration::Const
                        }
                    },
                };
                order.push((name.clone(), declaration));
            }
        }

        let mut bindings = Vec::with_capacity(order.len());
        for (name, declaration) in order {
            let access = match self.resolve_or_capture(&name) {
                Resolution::Local(local) if local.celled => EvalAccess::Cell(local.slot),
                Resolution::Upvalue(index) => EvalAccess::Upvalue(index),
                // Unreachable: `cell_everything` boxed every binding in every scope that can
                // reach this call. Refusing rather than dropping the name, because a dropped
                // binding does not disappear -- the eval'd source would resolve it to a built-in
                // or to nothing, and answer with something plausible from the wrong scope.
                _ => {
                    self.error(
                        Some(span),
                        format!("a direct `eval` cannot reach the binding `{name}`"),
                    );
                    continue;
                }
            };
            bindings.push(EvalBinding {
                name,
                access,
                declaration,
            });
        }
        bindings
    }

    /// Names visible to the function being compiled, and how each is reached.
    pub(super) fn resolve(&self, name: &str) -> Resolution {
        if let Some(local) = self.scopes.iter().rev().find_map(|scope| scope.get(name)) {
            return Resolution::Local(*local);
        }
        let Some(module) = self.enclosing.first() else {
            // Compiling the module body: its scopes were searched above, so the global object is
            // the only place left, and only for a name this program declared there.
            return self.global_object_binding(name);
        };
        if let Some(local) = module.scopes[0].get(name) {
            return Resolution::Global(local.slot);
        }
        self.global_object_binding(name)
    }

    /// Whether `name` is one this program declared on the global object.
    ///
    /// Checked last, after every scope: a `let` at the top of the script, a parameter, a block
    /// binding all shadow it, exactly as they shadow any other global. Checked *before* the
    /// built-in table, though, because `var Object = 1` at the top of a script is that script's
    /// binding and not the intrinsic.
    pub(super) fn global_object_binding(&self, name: &str) -> Resolution {
        if self.global_bindings.contains(name) {
            Resolution::GlobalObject
        } else {
            Resolution::Unresolved
        }
    }

    /// Resolve `name` and, if it lives in an enclosing function, capture it.
    ///
    /// Split from [`Compiler::resolve`] because capturing mutates the upvalue lists of every
    /// function between the binding and here, and a read-only lookup is wanted in places that
    /// must not have that side effect -- the `console` check in [`Compiler::call`] above all,
    /// which asks whether a name exists and would otherwise capture one by asking.
    pub(super) fn resolve_or_capture(&mut self, name: &str) -> Resolution {
        if let Some(local) = self.scopes.iter().rev().find_map(|scope| scope.get(name)) {
            return Resolution::Local(*local);
        }
        // The capture is tried before the global, because an enclosing function's binding
        // shadows a module binding of the same name. Asking `resolve` first would jump from
        // here to module scope and skip every function in between -- reading the wrong binding
        // whenever a name is reused, and reusing a name like `value` is not unusual. That was
        // invisible for as long as a module binding was only created at the line that declared
        // it, because the name did not exist yet when the inner function was compiled.
        if let Some(index) = self.capture(name) {
            return Resolution::Upvalue(index);
        }
        self.resolve(name)
    }

    /// Whether `name` binds nothing anywhere in scope, without creating a capture to find out.
    ///
    /// `resolve` alone is not this question: it looks in the current function and then jumps to
    /// module scope, skipping every function in between. Asking it directly made
    /// `typeof enclosingParameter` answer "undefined" -- which is the wrong answer for a name
    /// that plainly exists, and is exactly what every CommonJS shim tests when it writes
    /// `typeof exports == "object"` inside a nested function.
    pub(super) fn names_nothing(&self, name: &str) -> bool {
        if self.scopes.iter().any(|scope| scope.contains_key(name)) {
            return false;
        }
        if self
            .enclosing
            .iter()
            .any(|level| level.scopes.iter().any(|scope| scope.contains_key(name)))
        {
            return false;
        }
        matches!(self.resolve(name), Resolution::Unresolved)
    }

    /// Emit the read of a name this program put on the global object.
    ///
    /// `ThrowReference` is the whole of it: it looks the name up on the global object and only
    /// throws when it is not there, which is what a read of a global `var` binding is -- the
    /// property exists from declaration instantiation onwards, so the throw is unreachable for
    /// as long as nothing deletes it, and correct the moment something does.
    pub(super) fn load_global_object_binding(&mut self, name: &str) {
        let name = self.constant_index(Constant::String(name.to_owned().into()));
        self.code.push(Instruction::ThrowReference(name));
    }

    pub(super) fn store_global_object_binding(&mut self, name: &str) {
        let name = self.constant_index(Constant::String(name.to_owned().into()));
        self.code.push(Instruction::StoreGlobalBinding(name));
    }

    /// Thread a binding from the function that owns it down to the one being compiled.
    ///
    /// Every function in between gains an upvalue too, because a closure can only reach cells
    /// its immediately enclosing frame holds -- a two-level capture is two hops, not one long
    /// one. Levels are indices into `enclosing` followed by the current function.
    ///
    /// Module scope is included. It was skipped while a module binding was reached through the
    /// module frame instead, which cannot survive a function value leaving its module.
    pub(super) fn capture(&mut self, name: &str) -> Option<u32> {
        // Scope 0 of level 0 is module scope, whose bindings are globals rather than captures --
        // but a *block* inside the module body is an ordinary scope, and a closure created there
        // captures from it like any other. Skipping level 0 entirely missed exactly that: a
        // `for-of` at module level whose body creates a closure over the loop variable.
        let (owner, local) = (0..self.enclosing.len()).rev().find_map(|level| {
            self.enclosing[level]
                .scopes
                .iter()
                .rev()
                .find_map(|scope| scope.get(name))
                .map(|local| (level, *local))
        })?;
        if !local.celled {
            // The capture analysis said no nested function referred to this name, yet here we
            // are. Compiling it anyway would read an uninitialised slot as though it were a
            // cell, so refuse rather than emit that.
            return None;
        }

        let name_index = self.constant_index(Constant::String(name.to_owned().into()));
        let mut source = Upvalue::NamedParentLocal {
            slot: local.slot,
            name: name_index,
        };
        for level in (owner + 1)..=self.enclosing.len() {
            let index = self.add_upvalue(level, name, source);
            source = Upvalue::NamedParentUpvalue {
                index,
                name: name_index,
            };
        }
        // The last index added belongs to the function being compiled.
        self.upvalue_names.get(name).copied()
    }

    /// Add one upvalue to the function at `level`, reusing an existing capture of the same name.
    pub(super) fn add_upvalue(&mut self, level: usize, name: &str, source: Upvalue) -> u32 {
        let (upvalues, names) = if level == self.enclosing.len() {
            (&mut self.upvalues, &mut self.upvalue_names)
        } else {
            let state = &mut self.enclosing[level];
            (&mut state.upvalues, &mut state.upvalue_names)
        };
        if let Some(existing) = names.get(name) {
            return *existing;
        }
        let index = upvalues.len() as u32;
        upvalues.push(source);
        names.insert(name.to_owned(), index);
        index
    }

    pub(super) fn compiling_function(&self) -> bool {
        !self.enclosing.is_empty()
    }

    pub(super) fn push_literal(&mut self, literal: &Literal) {
        self.push_constant(match literal {
            Literal::Undefined => Constant::Undefined,
            Literal::Null => Constant::Null,
            Literal::Boolean(value) => Constant::Boolean(*value),
            Literal::Number(value) => Constant::Number(*value),
            Literal::String(value) => Constant::String(value.clone().into()),
            Literal::BigInt(digits) => Constant::BigInt(digits.clone()),
        });
    }

    pub(super) fn push_constant(&mut self, constant: Constant) {
        let index = self.constant_index(constant);
        self.code.push(Instruction::Const(index));
    }

    /// Intern a constant, returning its table index.
    ///
    /// Property names repeat heavily -- every `.length` in a program is the same string -- and
    /// an instruction that names one by index rather than carrying it inline is what lets the
    /// table hold one copy.
    pub(super) fn constant_index(&mut self, constant: Constant) -> u32 {
        if let Some(&existing) = self.constant_indices.get(&constant) {
            return existing;
        }
        let index = self.constants.len() as u32;
        self.constant_indices.insert(constant.clone(), index);
        self.constants.push(constant);
        index
    }

    pub(super) fn emit_jump(&mut self, instruction: Instruction) -> usize {
        let index = self.code.len();
        self.code.push(instruction);
        index
    }

    pub(super) fn patch_jump(&mut self, index: usize) {
        let target = self.code.len() as u32;
        match &mut self.code[index] {
            Instruction::Jump(offset)
            | Instruction::JumpIfFalse(offset)
            | Instruction::JumpIfTruthy(offset)
            | Instruction::JumpIfNotNullish(offset)
            // A handler's target is patched exactly like a jump's: neither is known until the
            // code it points past has been emitted.
            | Instruction::PushHandler(offset)
            | Instruction::YieldStar(offset)
            | Instruction::JumpIfReturnSignal(offset) => *offset = target,
            other => unreachable!("patch_jump called for {other:?}, which carries no target"),
        }
    }
}
