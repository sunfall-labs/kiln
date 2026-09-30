use super::*;

impl super::Compiler {
    pub(super) fn expression(&mut self, expression: &Expression) {
        match &expression.kind {
            ExpressionKind::Literal(literal) => self.push_literal(literal),
            ExpressionKind::Identifier(name) => {
                self.with_scoped_load(name, |compiler| compiler.load_lexically(name));
            }
            // `typeof x` inside a `with` has to ask the object first, and only then fall back
            // to the rule below -- the name may be absent from every binding around it and still
            // be a property of the object.
            ExpressionKind::Unary {
                operator: UnaryOperator::TypeOf,
                argument,
            } if matches!(
                &argument.kind,
                ExpressionKind::Identifier(name) if !self.with_scopes_for(name).is_empty()
            ) =>
            {
                let ExpressionKind::Identifier(name) = &argument.kind else {
                    unreachable!("the guard above matched an identifier")
                };
                let unbound = self.names_nothing(name)
                    && Builtin::from_name(name).is_none()
                    && name != "undefined"
                    && !(name == "arguments" && self.compiling_function());
                self.with_scoped_load(name, |compiler| {
                    // The *value*, not the string: the `typeof` below is what turns it into one,
                    // and pushing an answer here would report `"string"`. A name no scope bound
                    // is asked of the global object at run time rather than answered now,
                    // because the realm may have gained it since this was compiled.
                    if unbound {
                        let name =
                            compiler.constant_index(Constant::String(name.to_owned().into()));
                        compiler
                            .code
                            .push(Instruction::LoadGlobalBindingOrUndefined(name));
                    } else {
                        compiler.load_lexically(name);
                    }
                });
                self.code.push(Instruction::Unary(UnaryOp::TypeOf));
            }
            // `typeof undeclaredName` is "undefined" rather than a throw -- the one place a name
            // may be read without existing, and the whole reason feature detection works.
            ExpressionKind::Unary {
                operator: UnaryOperator::TypeOf,
                argument,
            } if matches!(
                &argument.kind,
                ExpressionKind::Identifier(name)
                    if self.names_nothing(name)
                        && Builtin::from_name(name).is_none()
                        && name != "undefined"
                        && name != "this"
                        && !(name == "arguments" && self.compiling_function())
            ) =>
            {
                let ExpressionKind::Identifier(name) = &argument.kind else {
                    unreachable!("the guard above matched an identifier")
                };
                // Asked of the global object rather than folded: `typeof x` where a `var` in
                // global code, an `eval`, or a plain `globalThis.x = 1` created `x` is not
                // `"undefined"`, and only the running realm knows.
                let name = self.constant_index(Constant::String(name.to_owned().into()));
                self.code
                    .push(Instruction::LoadGlobalBindingOrUndefined(name));
                self.code.push(Instruction::Unary(UnaryOp::TypeOf));
            }
            ExpressionKind::Unary { operator, argument } => {
                self.expression(argument);
                self.code.push(Instruction::Unary(match operator {
                    UnaryOperator::Plus => UnaryOp::Plus,
                    UnaryOperator::Negate => UnaryOp::Negate,
                    UnaryOperator::Not => UnaryOp::Not,
                    UnaryOperator::TypeOf => UnaryOp::TypeOf,
                    UnaryOperator::BitwiseNot => UnaryOp::BitwiseNot,
                    UnaryOperator::Void => UnaryOp::Void,
                }));
            }
            ExpressionKind::Binary {
                operator,
                left,
                right,
            } => {
                self.expression(left);
                self.expression(right);
                self.code.push(Instruction::Binary(match operator {
                    BinaryOperator::Add => BinaryOp::Add,
                    BinaryOperator::Subtract => BinaryOp::Subtract,
                    BinaryOperator::Multiply => BinaryOp::Multiply,
                    BinaryOperator::Divide => BinaryOp::Divide,
                    BinaryOperator::Remainder => BinaryOp::Remainder,
                    BinaryOperator::Exponentiate => BinaryOp::Exponentiate,
                    BinaryOperator::StrictEqual => BinaryOp::StrictEqual,
                    BinaryOperator::In => BinaryOp::In,
                    BinaryOperator::InstanceOf => BinaryOp::InstanceOf,
                    BinaryOperator::StrictNotEqual => BinaryOp::StrictNotEqual,
                    BinaryOperator::LooseEqual => BinaryOp::LooseEqual,
                    BinaryOperator::LooseNotEqual => BinaryOp::LooseNotEqual,
                    BinaryOperator::BitwiseAnd => BinaryOp::BitwiseAnd,
                    BinaryOperator::BitwiseOr => BinaryOp::BitwiseOr,
                    BinaryOperator::BitwiseXor => BinaryOp::BitwiseXor,
                    BinaryOperator::ShiftLeft => BinaryOp::ShiftLeft,
                    BinaryOperator::ShiftRight => BinaryOp::ShiftRight,
                    BinaryOperator::UnsignedShiftRight => BinaryOp::UnsignedShiftRight,
                    BinaryOperator::LessThan => BinaryOp::LessThan,
                    BinaryOperator::LessThanOrEqual => BinaryOp::LessThanOrEqual,
                    BinaryOperator::GreaterThan => BinaryOp::GreaterThan,
                    BinaryOperator::GreaterThanOrEqual => BinaryOp::GreaterThanOrEqual,
                }));
            }
            ExpressionKind::Logical {
                operator,
                left,
                right,
            } => self.logical(*operator, left, right),
            ExpressionKind::Assignment {
                operator,
                target,
                value,
            } => self.assignment(*operator, target, value, expression.span),
            ExpressionKind::PatternAssignment { pattern, value } => {
                // An assignment is an expression whose value is the right-hand side, so a copy
                // is kept back before the pattern consumes it.
                self.expression(value);
                self.code.push(Instruction::Dup);
                self.assign_pattern(pattern, expression.span);
            }
            ExpressionKind::Conditional {
                test,
                consequent,
                alternate,
            } => {
                self.expression(test);
                let false_jump = self.emit_jump(Instruction::JumpIfFalse(0));
                self.expression(consequent);
                let end_jump = self.emit_jump(Instruction::Jump(0));
                self.patch_jump(false_jump);
                self.expression(alternate);
                self.patch_jump(end_jump);
            }
            ExpressionKind::Call {
                callee,
                arguments,
                optional,
            } => self.call(callee, arguments, *optional, expression.span),
            ExpressionKind::InlineLiteralEval {
                callee: _,
                argument,
                expression: inline,
            } => {
                let name = self.constant_index(Constant::String("eval".into()));
                self.code
                    .push(Instruction::LoadGlobalBindingOrUndefined(name));
                self.code.push(Instruction::Dup);
                self.code
                    .push(Instruction::LoadIntrinsicBuiltin(Builtin::Eval));
                self.code.push(Instruction::Binary(BinaryOp::StrictEqual));
                let fallback = self.emit_jump(Instruction::JumpIfFalse(0));
                self.code.push(Instruction::Pop);
                self.expression(inline);
                let end = self.emit_jump(Instruction::Jump(0));
                self.patch_jump(fallback);
                self.expression(argument);
                self.code.push(Instruction::Call { argc: 1 });
                self.patch_jump(end);
            }
            ExpressionKind::This => self.load_this(),
            // `undefined` in a field initialiser: the initialiser is its own function to the
            // language, and reading the frame's `new.target` there answered `C` for
            // `class C { x = new.target }` because Kiln emits the initialiser into the
            // constructor.
            ExpressionKind::NewTarget => {
                if self.field_initialiser {
                    self.push_constant(Constant::Undefined);
                } else {
                    self.code.push(Instruction::LoadNewTarget);
                }
            }
            ExpressionKind::Yield { delegate, value } if !*delegate => {
                match value {
                    Some(value) => self.expression(value),
                    None => self.push_constant(Constant::Undefined),
                }
                self.code.push(Instruction::Yield);
            }
            ExpressionKind::Yield { value, .. } => {
                self.yield_delegate(value.as_deref(), expression.span)
            }
            ExpressionKind::Class(class) => self.class_expression(class, expression.span),
            ExpressionKind::Update {
                increment,
                prefix,
                target,
            } => self.update(*increment, *prefix, target, expression.span),
            ExpressionKind::UpdateMember {
                increment,
                prefix,
                object,
                property,
            } => self.update_member(*increment, *prefix, object, property, expression.span),
            ExpressionKind::SuperCall { arguments } => {
                if Self::has_spread(arguments) {
                    // The count is the length of an array the program computes, which is exactly
                    // what `argc` cannot say. The parent is reached through the same runtime-list
                    // shape `Apply` uses, but through `SuperApply`, because running the parent
                    // against the instance being built is not an ordinary call.
                    self.load_super();
                    self.array_literal(arguments, expression.span);
                    self.code.push(Instruction::SuperApply);
                    self.store_this_binding();
                    self.emit_elements_after_super(expression.span);
                    return;
                }
                if arguments.len() > u16::MAX as usize {
                    self.error(Some(expression.span), "too many arguments to `super`");
                    return;
                }
                self.load_super();
                for argument in arguments {
                    let (ListElement::Item(value) | ListElement::Spread(value)) = argument else {
                        continue;
                    };
                    self.expression(value);
                }
                // The parent's return value is this expression's value, so `SuperCall` leaves
                // exactly one thing on the stack and needs no epilogue here.
                self.code.push(Instruction::SuperCall {
                    argc: arguments.len() as u16,
                });
                self.store_this_binding();
                self.emit_elements_after_super(expression.span);
            }
            // `#x in o` compiles to the ordinary `in`: a private field is stored under a key no
            // program can write, so asking whether the key is there *is* the brand check.
            ExpressionKind::PrivateIn { name, object } => {
                // The key alone is not the answer: two evaluations of one class share it, so an
                // instance of a *sibling* evaluation carries the name and must still answer
                // `false`. Unlike an access, this reports rather than throws -- `#x in o` exists
                // to ask the question without raising.
                let brand = self.private_brand(name);
                let brand_key =
                    self.constant_index(Constant::String(self.private_brand_key(name).into()));
                let key = self.private_key(name);
                let held = format!("%pin{}", self.hidden_bindings);
                self.hidden_bindings += 1;
                self.scopes.push(HashMap::new());
                self.expression(object);
                self.declare_hidden(&held, expression.span);
                self.push_constant(Constant::String(key.into()));
                self.load_hidden(&held);
                self.code.push(Instruction::Binary(BinaryOp::In));
                let absent = self.emit_jump(Instruction::JumpIfFalse(0));
                self.load_hidden(&held);
                self.code.push(Instruction::GetProperty(brand_key));
                self.push_private_brand(&brand);
                self.code.push(Instruction::Binary(BinaryOp::StrictEqual));
                let done = self.emit_jump(Instruction::Jump(0));
                self.patch_jump(absent);
                self.push_constant(Constant::Boolean(false));
                self.patch_jump(done);
                self.scopes.pop().expect("private-in scope was just pushed");
            }
            ExpressionKind::SuperMember { property } => {
                // `super.m` resolves on the prototype of the object this method was defined on,
                // not on `this` and not on the parent constructor -- but it is still a reference
                // *with* a `this`, so writing one before `super()` is the same ReferenceError.
                self.require_this_binding();
                self.load_home_prototype();
                let name = self.constant_index(Constant::String(property.clone().into()));
                self.code.push(Instruction::LoadSuperProperty(name));
            }
            ExpressionKind::SuperComputedMember { property } => {
                // The key first, then the prototype: `super[f()]` runs `f` before it asks the
                // home object what it inherits from.
                self.require_this_binding();
                self.expression(property);
                self.load_home_prototype();
                self.code.push(Instruction::LoadSuperElement);
            }
            ExpressionKind::Function(function) => {
                // A function expression the source gave a name to can call itself by that name,
                // and the name is visible nowhere else. The scope pushed here is what keeps it
                // there -- the same shape a named class expression uses, and for the same reason.
                let self_bound = function
                    .binds_own_name
                    .then(|| function.name.clone())
                    .flatten();
                if let Some(name) = &self_bound {
                    self.scopes.push(HashMap::new());
                    self.predeclare(VariableKind::Const, name, false);
                }
                if let Some(index) = self.compile_function(function, expression.span) {
                    self.code.push(Instruction::MakeFunction(index));
                }
                if let Some(name) = &self_bound {
                    // The closure is both the expression's value and what the binding holds, so
                    // it is duplicated rather than loaded back.
                    self.code.push(Instruction::Dup);
                    match self.resolve(name) {
                        Resolution::Local(local) if local.celled => {
                            self.code.push(Instruction::StoreCell(local.slot));
                        }
                        Resolution::Local(local) => {
                            self.code.push(Instruction::StoreLocal(local.slot));
                        }
                        _ => {
                            self.error(Some(expression.span), "could not bind function expression");
                            self.code.push(Instruction::Pop);
                        }
                    }
                    self.scopes.pop().expect("function scope was just pushed");
                }
            }
            ExpressionKind::Array { elements } => self.array_literal(elements, expression.span),
            ExpressionKind::Sequence { operands } => {
                let Some((last, rest)) = operands.split_last() else {
                    self.push_constant(Constant::Undefined);
                    return;
                };
                // Every operand is evaluated for its effects; only the last one is the value.
                for operand in rest {
                    self.expression(operand);
                    self.code.push(Instruction::Pop);
                }
                self.expression(last);
            }
            // `await` is a `yield` whose resumption the async driver supplies rather than the
            // program: the same suspension, resumed by a promise settling instead of a `next`.
            ExpressionKind::Await(value) => {
                if !self.compiling_function() && !self.module_body {
                    // A script has nowhere to put the suspension: only a module body is an
                    // evaluation the host can stop and resume, which is what makes top-level
                    // `await` a module-only form rather than a general one.
                    self.error(
                        Some(expression.span),
                        "top-level `await` is only available in a module",
                    );
                    self.push_constant(Constant::Undefined);
                    return;
                }
                self.expression(value);
                self.code.push(Instruction::Await);
            }
            // The specifier expression is evaluated like any other, so a `throw` inside it
            // throws. Only what happens *after* it -- coercing it to a string, resolving it,
            // evaluating the module -- rejects the promise instead, which is why the coercion
            // is the instruction's job and not something emitted around it.
            ExpressionKind::ImportCall { specifier, options } => {
                self.expression(specifier);
                // The second argument is evaluated in source order and then discarded: nothing
                // reads an import attribute yet, but a getter written in one still runs.
                if let Some(options) = options {
                    self.expression(options);
                    self.code.push(Instruction::DynamicImportWithOptions);
                } else {
                    self.code.push(Instruction::DynamicImport);
                }
            }
            ExpressionKind::Chain { expression: inner } => {
                self.chains.push(Vec::new());
                self.expression(inner);
                let exits = self.chains.pop().expect("this chain pushed its own frame");
                // Every `?.` in the chain lands here with `undefined` already on the stack, so
                // the chain's value is one thing either way.
                for jump in exits {
                    self.patch_jump(jump);
                }
            }
            ExpressionKind::ComputedMember {
                object,
                property,
                optional,
            } => {
                self.expression(object);
                self.short_circuit_if_nullish(*optional, expression.span);
                self.expression(property);
                self.code.push(Instruction::GetComputed);
            }
            ExpressionKind::ComputedAssignment {
                operator,
                object,
                property,
                value,
            } => self.computed_assignment(*operator, object, property, value),
            ExpressionKind::New { callee, arguments } => {
                if arguments.len() > u16::MAX as usize {
                    self.error(Some(expression.span), "too many constructor arguments");
                    return;
                }
                if Self::has_spread(arguments) {
                    self.expression(callee);
                    self.array_literal(arguments, expression.span);
                    self.code.push(Instruction::ConstructSpread);
                    return;
                }
                self.expression(callee);
                for argument in arguments {
                    let ListElement::Item(value) = argument else {
                        unreachable!("just checked that there are no spreads");
                    };
                    self.expression(value);
                }
                self.code.push(Instruction::New {
                    argc: arguments.len() as u16,
                });
            }
            ExpressionKind::MemberAssignment {
                operator,
                object,
                property,
                value,
            } => self.member_assignment(*operator, object, property, value),
            ExpressionKind::SuperMemberAssignment {
                operator,
                property,
                value,
            } => self.super_member_assignment(*operator, property, value, expression.span),
            ExpressionKind::SuperComputedAssignment {
                operator,
                property,
                value,
            } => self.super_computed_assignment(*operator, property, value, expression.span),
            ExpressionKind::PrivateAssignment {
                operator,
                object,
                name,
                value,
            } => self.private_assignment(*operator, object, name, value),
            ExpressionKind::Object { members } => self.object_literal(members, expression.span),
            // Concatenation, starting from a string so that every `+` after it is a string
            // append rather than a numeric add. `${1}${2}` is "12" for exactly that reason.
            ExpressionKind::Template {
                quasis,
                expressions,
            } => {
                let mut quasis = quasis.iter();
                self.push_constant(Constant::String(
                    quasis
                        .next()
                        .cloned()
                        .unwrap_or_else(|| StringUnits::from(""))
                        .into(),
                ));
                for (expression, quasi) in expressions.iter().zip(quasis) {
                    // `${x}` is ToString(x), which reduces an object with the *string* hint --
                    // `toString` before `valueOf`. Concatenation reduces with the default hint,
                    // the other way round, so a template is not `"" + x`: for an object with a
                    // `valueOf` and no own `toString` the two genuinely disagree, and desugaring
                    // to `+` silently produced the wrong one.
                    //
                    // A literal is already a primitive and converts to itself, so it keeps the
                    // plain concatenation -- which is also what leaves a template of constants
                    // compilable by a backend that has no calls.
                    if matches!(expression.kind, ExpressionKind::Literal(_)) {
                        self.expression(expression);
                    } else {
                        self.code.push(Instruction::LoadBuiltin(Builtin::String));
                        self.expression(expression);
                        self.code.push(Instruction::Call { argc: 1 });
                    }
                    self.code.push(Instruction::Binary(BinaryOp::Add));
                    // A run of text between two holes is skipped when it is empty, which is the
                    // common shape: `${a}${b}` has two empty quasis in the middle and the end.
                    if quasi.unit_len() != 0 {
                        self.push_constant(Constant::String(quasi.clone().into()));
                        self.code.push(Instruction::Binary(BinaryOp::Add));
                    }
                }
            }
            ExpressionKind::TemplateStrings { cooked, raw } => {
                if cooked.len() > u32::MAX as usize || raw.len() > u32::MAX as usize {
                    self.error(Some(expression.span), "template has too many segments");
                    return;
                }
                let site_index = u32::try_from(self.template_sites.len()).unwrap_or(u32::MAX);
                self.template_sites.push(TemplateSite {
                    cooked: cooked.clone(),
                    raw: raw.clone(),
                });
                self.code.push(Instruction::LoadTemplateObject(site_index));
            }
            ExpressionKind::RegExp { pattern, flags } => {
                let pattern = self.constant_index(Constant::String(pattern.clone().into()));
                let flags = self.constant_index(Constant::String(flags.clone().into()));
                self.code.push(Instruction::NewRegExp { pattern, flags });
            }
            ExpressionKind::PrivateMember {
                object,
                name,
                optional,
            } => {
                self.expression(object);
                // `o?.#x` on a nullish `o` leaves the chain with `undefined`, exactly as an
                // ordinary optional member read does. Without this the brand check ran on
                // `null` and raised the TypeError the `?.` was written to avoid.
                self.short_circuit_if_nullish(*optional, expression.span);
                self.load_private_brand(name);
                let name = self.constant_index(Constant::String(self.private_key(name).into()));
                self.code.push(Instruction::GetPrivate(name));
            }
            ExpressionKind::DeleteValue { argument } => {
                // The operand is evaluated and discarded: `delete f()` calls `f`, and the
                // answer is `true` whatever it returned.
                self.expression(argument);
                self.code.push(Instruction::Pop);
                self.push_constant(Constant::Boolean(true));
            }
            ExpressionKind::DeleteBinding { name } => self.delete_binding(name),
            ExpressionKind::Delete { object, property } => {
                self.expression(object);
                match property.as_ref() {
                    // `delete obj.#x` is a syntax error, so the frontend never builds one.
                    ObjectKey::Private(name) => self.error(
                        Some(expression.span),
                        format!("`delete` of the private name `#{name}`"),
                    ),
                    ObjectKey::Static(key) => {
                        let name = self.constant_index(Constant::String(key.clone().into()));
                        self.code.push(self.delete_property_instruction(name));
                    }
                    ObjectKey::Computed(key) => {
                        self.expression(key);
                        self.code.push(self.delete_computed_instruction());
                    }
                }
            }
            ExpressionKind::StaticMember {
                object,
                property,
                optional,
            } => {
                if let Some(method) = self.lone_method_read(object, property) {
                    // `{ m() {} }.m`: the method and nothing else (see `lone_method_read`).
                    self.next_is_method = true;
                    self.expression(method);
                    return;
                }
                self.expression(object);
                self.short_circuit_if_nullish(*optional, expression.span);
                let name = self.constant_index(Constant::String(property.clone().into()));
                self.code.push(Instruction::GetProperty(name));
            }
            ExpressionKind::Unsupported { feature } => {
                self.error(
                    Some(expression.span),
                    format!("unsupported expression: {feature}"),
                );
                self.push_constant(Constant::Undefined);
            }
        }
    }

    /// Leave the enclosing optional chain with `undefined` when the value on top of the stack is
    /// nullish, or fall through with it untouched.
    ///
    /// Does nothing when the access is not optional, so every member read can call it and only
    /// the ones written with `?.` cost anything.
    pub(super) fn short_circuit_if_nullish(&mut self, optional: bool, span: Span) {
        if !optional {
            return;
        }
        if self.chains.is_empty() {
            // The frontend only produces an optional access inside a chain, so this means the
            // two have drifted apart rather than that a program did something unusual.
            self.error(Some(span), "`?.` outside of an optional chain");
            return;
        }
        self.code.push(Instruction::Dup);
        let present = self.emit_jump(Instruction::JumpIfNotNullish(0));
        // Nullish: drop the object and hand the whole chain `undefined`.
        self.code.push(Instruction::Pop);
        self.push_constant(Constant::Undefined);
        let exit = self.emit_jump(Instruction::Jump(0));
        self.chains
            .last_mut()
            .expect("checked just above")
            .push(exit);
        self.patch_jump(present);
    }

    /// An object literal.
    ///
    /// With no spread this is one `NewObject` over the whole literal, which is the common case
    /// and the cheap one. A spread splits the literal into segments: each run of properties
    /// becomes its own object and is merged into the accumulator in source order, so a property
    /// written after a spread overwrites what the spread contributed and one written before it
    /// is overwritten. Merging a segment uses the same instruction the spread does, rather than
    /// a second way to set a property on an object already on the stack.
    pub(super) fn object_literal(&mut self, members: &[ObjectMember], span: Span) {
        if members.len() > u32::MAX as usize {
            self.error(Some(span), "object literal has too many properties");
            return;
        }
        let mut pending = 0u32;
        let mut accumulating = false;
        // A method written in a literal is defined *on the literal*, so `super.x` inside one
        // reads the literal's own prototype. The binding is reserved before any member is
        // compiled, because a method closes over it and is compiled before the object it will
        // name exists; it is filled in once the literal is built, below.
        //
        // Only a method or an accessor gets one. `{ x: v }` and `{ ...o }` cannot contain a
        // `super` that belongs to this literal -- a `super` written in a value position belongs
        // to whatever method encloses the literal -- so a plain object literal emits exactly
        // what it emitted before.
        let home = members
            .iter()
            .any(|member| {
                matches!(
                    member,
                    ObjectMember::Accessor { .. }
                        | ObjectMember::Property(kiln_hir::ObjectProperty {
                            form: kiln_hir::PropertyForm::Method,
                            ..
                        })
                )
            })
            .then(|| self.declare_home_binding(span));
        // The hidden local a pending pair's computed key converts through, in a scope of its own
        // that is opened by the first key needing it and closed with the literal.
        let mut key_slot: Option<String> = None;

        for member in members {
            match member {
                // `{ __proto__: v }` is not a property at all: it links the object to `v`. Only
                // this spelling does -- `{ __proto__ }` and `{ __proto__() {} }` are ordinary own
                // properties, and so is `{ ["__proto__"]: v }`, because the specification asks
                // whether the key was computed rather than what it evaluated to.
                ObjectMember::Property(property)
                    if matches!(&property.key, ObjectKey::Static(key) if key == &StringUnits::from("__proto__"))
                        && property.form == kiln_hir::PropertyForm::KeyValue =>
                {
                    // The prototype is set on an object, so the run of pairs written before it
                    // has to become one first -- the same reason an accessor flushes the run.
                    self.emit_object_segment(&mut pending, accumulating);
                    accumulating = true;
                    self.expression(&property.value);
                    self.code.push(Instruction::SetPrototype);
                }
                ObjectMember::Property(property) => {
                    // `{ [k]: function () {} }` names the function after the key, which is only
                    // knowable once the key has been evaluated. The pairs waiting to become an
                    // object are flushed first so the object exists to define on -- the shape a
                    // spread already uses -- and the key is then evaluated in its own place, so
                    // the source order of its side effects is unchanged.
                    //
                    // The same path serves every pair written after a spread: the accumulator
                    // exists by then, so the pair is defined on it directly rather than built
                    // into a second object and spread in -- one literal, not two, and the
                    // spread's per-key copy is not run twice for `{ ...base, f: i }`.
                    let named_after_key = matches!(&property.key, ObjectKey::Computed(_))
                        && anonymous_function_value(&property.value);
                    if accumulating || named_after_key {
                        let method = property.form == kiln_hir::PropertyForm::Method;
                        self.emit_object_segment(&mut pending, accumulating);
                        accumulating = true;
                        match &property.key {
                            ObjectKey::Private(name) => self.error(
                                Some(span),
                                format!("private name `#{name}` in an object literal"),
                            ),
                            ObjectKey::Static(key) => {
                                self.push_constant(Constant::String(key.clone().into()));
                            }
                            // The object is beneath the key, which is the pair `ToPropertyKey`
                            // reads.
                            ObjectKey::Computed(key) => {
                                self.expression(key);
                                self.code.push(Instruction::ToPropertyKey);
                            }
                        }
                        if method && let Some(home) = &home {
                            self.home_bindings.push(home.clone());
                        }
                        self.next_is_method = method;
                        self.expression(&property.value);
                        if method && home.is_some() {
                            self.home_bindings.pop();
                        }
                        // Only an anonymous function definition takes the key as its name: a
                        // pair here only because it follows a spread defines without renaming
                        // what its value already is (`{ ...o, f: foo }` keeps `foo.name`).
                        self.code.push(Instruction::DefineNamedProperty {
                            name_function: method || anonymous_function_value(&property.value),
                        });
                        continue;
                    }
                    match &property.key {
                        // Only a class body can declare one, so a literal never has one.
                        ObjectKey::Private(name) => self.error(
                            Some(span),
                            format!("private name `#{name}` in an object literal"),
                        ),
                        ObjectKey::Static(key) => {
                            self.push_constant(Constant::String(key.clone().into()));
                        }
                        // Evaluated in source order, between the members around it, because a
                        // computed key is an arbitrary expression and may have side effects.
                        ObjectKey::Computed(key) => {
                            self.expression(key);
                            self.pending_literal_key(key, &mut key_slot, span);
                        }
                    }
                    // A method's body is the only part of a member the literal's own home
                    // covers. A value's is not: `{ v: super.m() }` inside a class method reads
                    // that method's home, not this literal's.
                    let method = property.form == kiln_hir::PropertyForm::Method;
                    if method && let Some(home) = &home {
                        self.home_bindings.push(home.clone());
                    }
                    // The body about to be compiled is a method.
                    self.next_is_method = method;
                    self.expression(&property.value);
                    if method && home.is_some() {
                        self.home_bindings.pop();
                    }
                    pending += 1;
                }
                ObjectMember::Spread(source) => {
                    // The first segment becomes the accumulator; later ones merge into it.
                    self.emit_object_segment(&mut pending, accumulating);
                    accumulating = true;
                    self.expression(source);
                    self.code.push(Instruction::SpreadObject);
                }
                ObjectMember::Accessor {
                    key,
                    kind,
                    function,
                } => {
                    // An accessor is defined on the object rather than pushed as a pair, so the
                    // pending run has to become an object first.
                    self.emit_object_segment(&mut pending, accumulating);
                    accumulating = true;
                    match key {
                        ObjectKey::Private(name) => self.error(
                            Some(span),
                            format!("private name `#{name}` in an object literal"),
                        ),
                        ObjectKey::Static(name) => {
                            self.push_constant(Constant::String(name.clone().into()));
                        }
                        ObjectKey::Computed(key) => {
                            self.expression(key);
                            self.code.push(Instruction::ToPropertyKey);
                        }
                    }
                    if let Some(home) = &home {
                        self.home_bindings.push(home.clone());
                    }
                    self.next_is_method = true;
                    let compiled = self.compile_function(function, span);
                    if home.is_some() {
                        self.home_bindings.pop();
                    }
                    let Some(index) = compiled else {
                        return;
                    };
                    self.code.push(Instruction::MakeFunction(index));
                    self.code.push(Instruction::DefineAccessor {
                        setter: matches!(kind, kiln_hir::MethodKind::Set),
                        enumerable: true,
                    });
                }
            }
        }
        self.emit_object_segment(&mut pending, accumulating);
        if key_slot.is_some() {
            self.scopes
                .pop()
                .expect("the literal's key scope was pushed");
        }
        // The literal is the home object its methods were defined on, so it is what the binding
        // they captured has to hold -- duplicated rather than reloaded, because the literal is
        // also this expression's value.
        if let Some(home) = &home {
            self.code.push(Instruction::Dup);
            let home = home.clone();
            self.store_home_binding(&home);
        }
    }

    /// `ToPropertyKey` of a computed key in a pending run of pairs, where it happens: after the
    /// key and before the value, so `{ [k]: v() }` runs `k`'s `toString` before `v` and a throw
    /// from it stops the literal there. Converting when the object is built ran it after every
    /// value, and the native backend did not run it at all (issue 24).
    ///
    /// `ToPropertyKey` reads the object beneath its key, and a pending pair has none -- only the
    /// previous pair's value, which may be nullish. The key itself stands in for it (a nullish
    /// key is a primitive and needs no conversion), and a hidden local carries the converted key
    /// past the original. A key that is syntactically a primitive is left alone.
    fn pending_literal_key(&mut self, key: &Expression, slot: &mut Option<String>, span: Span) {
        if matches!(
            key.kind,
            kiln_hir::ExpressionKind::Literal(_) | kiln_hir::ExpressionKind::Template { .. }
        ) {
            return;
        }
        let name = match slot {
            Some(name) => name.clone(),
            None => {
                let name = format!("%okey{}", self.hidden_bindings);
                self.hidden_bindings += 1;
                self.scopes.push(HashMap::new());
                *slot = Some(name.clone());
                name
            }
        };
        self.code.push(Instruction::Dup);
        self.code.push(Instruction::ToPropertyKey);
        self.declare_hidden(&name, span);
        self.code.push(Instruction::Pop);
        self.load_hidden(&name);
    }

    /// The method of `{ m() { ... } }.m`, when reading it back is all the literal is for.
    ///
    /// That spelling is how a built-in is written to be a method -- no `prototype`, no
    /// [[Construct]] -- rather than a function declaration (`date.ts`, `typedarray.ts` and the
    /// rest of the bootstrap: 139 of them ran in every realm's start, each leaving the literal and
    /// its home-object cell as garbage, realms issue 08). The literal is a fresh ordinary object
    /// with one own data property, so the read finds exactly the method the definition made and
    /// runs nothing else; the method's name and kind come from its definition, not the object.
    /// What the literal could still be observed through is the method's [[HomeObject]], which
    /// only `super` reads -- written in the body, or reached by a direct `eval` there. So the
    /// literal is dropped only when the method's text never spells `super` (a keyword cannot be
    /// written with escapes, so the letters are there whenever the reference is) and its body
    /// holds no direct `eval`.
    fn lone_method_read<'e>(
        &self,
        object: &'e Expression,
        property: &str,
    ) -> Option<&'e Expression> {
        let ExpressionKind::Object { members } = &object.kind else {
            return None;
        };
        let [ObjectMember::Property(member)] = members.as_slice() else {
            return None;
        };
        if member.form != kiln_hir::PropertyForm::Method {
            return None;
        }
        let ObjectKey::Static(key) = &member.key else {
            return None;
        };
        if *key != StringUnits::from(property) {
            return None;
        }
        let ExpressionKind::Function(function) = &member.value.kind else {
            return None;
        };
        let text = self
            .source
            .get(function.span.start as usize..function.span.end as usize)?;
        if text.is_empty() || text.contains("super") || function_contains_direct_eval(function) {
            return None;
        }
        Some(&member.value)
    }

    /// Emit the pending run of key/value pairs as an object, merging it into the accumulator
    /// beneath when one exists.
    ///
    /// Skipped when the run is empty and something is already accumulating, so `{ ...a, ...b }`
    /// does not build two throwaway objects.
    pub(super) fn emit_object_segment(&mut self, pending: &mut u32, merge: bool) {
        if merge && *pending == 0 {
            return;
        }
        self.code.push(Instruction::NewObject { count: *pending });
        *pending = 0;
        if merge {
            self.code.push(Instruction::SpreadObject);
        }
    }

    pub(super) fn logical(
        &mut self,
        operator: LogicalOperator,
        left: &Expression,
        right: &Expression,
    ) {
        self.expression(left);
        self.code.push(Instruction::Dup);
        let jump = self.emit_jump(match operator {
            LogicalOperator::And => Instruction::JumpIfFalse(0),
            LogicalOperator::Or => Instruction::JumpIfTruthy(0),
            LogicalOperator::NullishCoalesce => Instruction::JumpIfNotNullish(0),
        });
        self.code.push(Instruction::Pop);
        self.expression(right);
        self.patch_jump(jump);
    }

    pub(super) fn assignment(
        &mut self,
        operator: AssignmentOperator,
        target: &str,
        value: &Expression,
        span: Span,
    ) {
        let scopes = self.with_scopes_for(target);
        if !scopes.is_empty() {
            self.with_assignment(&scopes, operator, target, value, span);
            return;
        }
        self.assign_lexically(operator, target, value, span);
    }

    /// `name = value` inside a `with`, written to the innermost object that has the name.
    ///
    /// The value expression is compiled once per object plus once for the lexical case. Only one
    /// of those copies ever runs, and the alternative -- spilling the value into a temporary
    /// first -- would evaluate it before the reference is resolved, which is the opposite of the
    /// order the specification gives.
    pub(super) fn with_assignment(
        &mut self,
        scopes: &[WithScope],
        operator: AssignmentOperator,
        target: &str,
        value: &Expression,
        span: Span,
    ) {
        let key = self.constant_index(Constant::String(target.to_owned().into()));
        let mut found = Vec::with_capacity(scopes.len());
        for with in scopes {
            self.load_with_object(with);
            self.code.push(Instruction::WithHas(key));
            let miss = self.emit_jump(Instruction::JumpIfFalse(0));
            if operator.is_short_circuiting() {
                self.load_with_object(with);
                self.code.push(Instruction::GetProperty(key));
                let skip = self.emit_jump(short_circuit_jump(operator));
                self.load_with_object(with);
                self.expression(value);
                self.code.push(self.set_property_instruction(key));
                let done = self.emit_jump(Instruction::Jump(0));
                self.patch_jump(skip);
                self.load_with_object(with);
                self.code.push(Instruction::GetProperty(key));
                self.patch_jump(done);
            } else if operator == AssignmentOperator::Assign {
                self.load_with_object(with);
                self.expression(value);
                self.code.push(self.set_property_instruction(key));
            } else {
                self.load_with_object(with);
                self.load_with_object(with);
                self.code.push(Instruction::GetProperty(key));
                self.expression(value);
                self.code
                    .push(Instruction::Binary(compound_operator(operator)));
                self.code.push(self.set_property_instruction(key));
            }
            found.push(self.emit_jump(Instruction::Jump(0)));
            self.patch_jump(miss);
        }
        self.assign_lexically(operator, target, value, span);
        for jump in found {
            self.patch_jump(jump);
        }
    }

    /// `name = value` against the bindings around it, ignoring any `with` object.
    pub(super) fn assign_lexically(
        &mut self,
        operator: AssignmentOperator,
        target: &str,
        value: &Expression,
        span: Span,
    ) {
        let Some((load, store)) = self.assignment_place(target, span) else {
            return;
        };
        if operator.is_short_circuiting() {
            // `a ??= b` reads `a`, and assigns only if it is the wrong one -- so `b` may never
            // be evaluated and the store may never happen. The current value is left on the
            // stack as the expression's result when the store is skipped.
            self.code.push(load.clone());
            let skip = self.emit_jump(short_circuit_jump(operator));
            self.expression(value);
            self.code.push(Instruction::Dup);
            self.code.push(store);
            let done = self.emit_jump(Instruction::Jump(0));
            self.patch_jump(skip);
            self.code.push(load);
            self.patch_jump(done);
            return;
        }
        if operator == AssignmentOperator::Assign {
            self.expression(value);
        } else {
            self.code.push(load);
            self.expression(value);
            self.code
                .push(Instruction::Binary(compound_operator(operator)));
        }
        self.code.push(Instruction::Dup);
        self.code.push(store);
    }

    /// Store the value on top of the stack into an existing binding, consuming it.
    ///
    /// The leaf of a destructuring assignment. Unlike [`Compiler::assignment`] there is no
    /// operator and no result to leave behind: the pattern walk already holds what it needs.
    pub(super) fn store_existing(&mut self, name: &str, span: Span) {
        if let Some((_, store)) = self.assignment_place(name, span) {
            self.code.push(store);
        }
    }

    /// How to read and write `target`, or `None` when it names nothing.
    ///
    /// Reports const and imported-binding assignments, and compiles an unresolved name to a
    /// `ReferenceError` raised where the assignment happens -- so a caller that gets `None` has
    /// already had its diagnostic or its throw emitted and should emit nothing more.
    pub(super) fn assignment_place(
        &mut self,
        target: &str,
        span: Span,
    ) -> Option<(Instruction, Instruction)> {
        // Whether the target is a frame local or a module slot decides the store instruction,
        // so it has to be carried alongside the slot rather than assumed.
        enum Place {
            Local(u32),
            Cell(u32),
            Upvalue(u32),
            Global(u32),
        }
        let (place, kind, imported) = match self.resolve_or_capture(target) {
            Resolution::Local(local) if local.celled => {
                (Place::Cell(local.slot), local.kind, local.imported)
            }
            Resolution::Local(local) => (Place::Local(local.slot), local.kind, local.imported),
            Resolution::Upvalue(index) => {
                // The owning declaration decides const-ness, and it was checked where the name
                // was captured; an upvalue carries no separate mutability of its own.
                (Place::Upvalue(index), VariableKind::Let, false)
            }
            Resolution::Global(slot) => {
                let module = &self.enclosing[0].scopes[0];
                let local = module
                    .get(target)
                    .copied()
                    .expect("Global resolution just found this name in module scope");
                (Place::Global(slot), local.kind, local.imported)
            }
            // A `var` this program declared on the global object. `ThrowReference` reads it and
            // `StoreGlobalBinding` writes it, exactly as for a name nothing declared -- the
            // difference is that declaration instantiation already created the property, so the
            // read cannot throw and the write cannot be the one that creates it.
            Resolution::GlobalObject => {
                let name = self.constant_index(Constant::String(target.to_owned().into()));
                return Some((
                    Instruction::ThrowReference(name),
                    Instruction::StoreGlobalBinding(name),
                ));
            }
            Resolution::Unresolved => {
                let _ = span;
                let name = self.constant_index(Constant::String(target.to_owned().into()));
                // Sloppy code settles an unresolvable reference against the global object, which
                // creates the property rather than refusing the write. Strict code raises the
                // `ReferenceError` where the assignment happens rather than when it is compiled.
                //
                // Only the *store* differs. A compound assignment reads first, and reading a name
                // nothing bound is still a `ReferenceError` in both modes -- `undeclared += 1`
                // throws in sloppy code too -- so the load stays `ThrowReference`, which is also
                // the instruction that finds a global the program installed earlier.
                if self.strict {
                    // Where the reference is *resolved*, which is before the right-hand side is
                    // evaluated. Strict mode refuses to create a global by assignment, and
                    // whether this name is one it would have to create can only be answered at
                    // run time -- a `var` in global code, or a `globalThis.x = 1` a line above,
                    // makes it a perfectly ordinary write.
                    self.code.push(Instruction::RequireGlobalBinding(name));
                }
                return Some((
                    Instruction::ThrowReference(name),
                    if self.strict {
                        Instruction::StoreGlobalBindingStrict(name)
                    } else {
                        Instruction::StoreGlobalBinding(name)
                    },
                ));
            }
        };
        let (load, store): (Instruction, Instruction) = match place {
            Place::Local(slot) => (Instruction::LoadLocal(slot), Instruction::StoreLocal(slot)),
            Place::Cell(slot) => (Instruction::LoadCell(slot), Instruction::StoreCell(slot)),
            Place::Upvalue(index) => (
                Instruction::LoadUpvalue(index),
                Instruction::StoreUpvalue(index),
            ),
            Place::Global(slot) => (
                Instruction::LoadGlobal(slot),
                Instruction::StoreGlobal(slot),
            ),
        };
        // Assignment is also a read of the binding's state: `x = 1; let x;` is a ReferenceError,
        // not a write that makes the later declaration appear to have run. A switch shares one
        // scope between its cases, so compiler-time `initialized` can be true here after another
        // case's declaration even when this path reaches the assignment first. Probe the
        // run-time slot before every lexical write; initialized bindings pay only a load/pop.
        if kind != VariableKind::Var || matches!(&load, Instruction::LoadUpvalue(_)) {
            self.code.push(load.clone());
            self.code.push(Instruction::Pop);
        }
        if imported {
            self.error(
                Some(span),
                format!("assignment to imported binding `{target}` is not allowed"),
            );
        } else if kind == VariableKind::Const {
            self.error(
                Some(span),
                format!("assignment to const local `{target}` is not supported"),
            );
        }
        Some((load, store))
    }

    pub(super) fn return_statement(&mut self, value: Option<&Expression>, span: Span) {
        if !self.compiling_function() {
            self.error(Some(span), "`return` outside of a function");
            return;
        }
        // `return;` in a derived constructor yields `this`, and reading it is what makes
        // returning before `super()` a ReferenceError. But the read happens where `[[Construct]]`
        // does it -- *after* the cleanups -- because a `finally` is allowed to be where `super()`
        // is called: `constructor() { try { return; } finally { super(); } }` constructs, and
        // reading `this` at the `return` would have refused it.
        let deferred_this = value.is_none() && self.derived_constructor;
        match value {
            Some(value) => {
                self.expression(value);
                if self.async_generator {
                    self.code.push(Instruction::Await);
                }
            }
            None if deferred_this => {}
            // Anywhere else a bare `return` is `undefined`, and reaching for `this` there would
            // have made every one of them answer with the receiver.
            None => self.push_constant(Constant::Undefined),
        }
        if !self.finalizers.is_empty() {
            if deferred_this {
                self.unwind_finalizers(self.function_finalizer_depth);
            } else {
                // The value is computed before the cleanups run -- that is the observable order
                // -- but the cleanups may use the stack, so it is parked in a local across them.
                let slot = format!("%ret{}", self.hidden_bindings);
                self.hidden_bindings += 1;
                self.scopes.push(HashMap::new());
                self.declare_bound(VariableKind::Let, &slot, span);
                self.unwind_finalizers(self.function_finalizer_depth);
                self.load_hidden(&slot);
                self.scopes.pop().expect("return scope was just pushed");
            }
        }
        if deferred_this {
            let binding = self
                .this_binding
                .clone()
                .expect("a deferred `this` has a binding to read");
            self.load_this_binding(&binding);
        } else if self.derived_constructor {
            // `return v` in a derived constructor, where the value is not simply handed back.
            let binding = self
                .this_binding
                .clone()
                .expect("a derived constructor has a hidden `this` binding");
            self.guard_derived_return(&binding);
        }
        self.code.push(Instruction::Return);
    }

    /// `object.property = value`, and the compound forms.
    ///
    /// The object expression is evaluated once. `o().x += 1` must not call `o` twice, so the
    /// reference is duplicated on the stack rather than the expression being compiled twice --
    /// which would be the easy mistake and would double any side effect in the receiver.
    /// `x++` and friends.
    ///
    /// The two forms differ only in when the value is duplicated: a prefix update yields the new
    /// value, a postfix one the old. That is the whole reason this is not lowered to `x = x + 1`.
    pub(super) fn update(&mut self, increment: bool, prefix: bool, target: &str, span: Span) {
        let scopes = self.with_scopes_for(target);
        if !scopes.is_empty() {
            self.with_update(&scopes, increment, prefix, target, span);
            return;
        }
        self.update_lexically(increment, prefix, target, span);
    }

    /// `name++` inside a `with`, updating the innermost object that has the name.
    ///
    /// The old value is spilled into a hidden binding rather than juggled on the stack: the
    /// object has to sit beneath the new value for the store, and the old one has to survive
    /// past it for a postfix update, which is one value too many to keep in order otherwise.
    pub(super) fn with_update(
        &mut self,
        scopes: &[WithScope],
        increment: bool,
        prefix: bool,
        target: &str,
        span: Span,
    ) {
        let key = self.constant_index(Constant::String(target.to_owned().into()));
        let held = format!("%update{}", self.hidden_bindings);
        self.hidden_bindings += 1;
        let mut found = Vec::with_capacity(scopes.len());
        for with in scopes {
            self.load_with_object(with);
            self.code.push(Instruction::WithHas(key));
            let miss = self.emit_jump(Instruction::JumpIfFalse(0));
            self.load_with_object(with);
            self.code.push(Instruction::GetProperty(key));
            // `ToNumeric` first, for the same reason the lexical form does it: `x++` where `x`
            // is "5" is 5 then 6, not "5" then "51".
            self.code.push(Instruction::Unary(UnaryOp::ToNumeric));
            self.declare_hidden(&held, span);
            self.load_with_object(with);
            self.load_hidden(&held);
            self.code.push(Instruction::Unary(if increment {
                UnaryOp::Increment
            } else {
                UnaryOp::Decrement
            }));
            self.code.push(self.set_property_instruction(key));
            if !prefix {
                self.code.push(Instruction::Pop);
                self.load_hidden(&held);
            }
            found.push(self.emit_jump(Instruction::Jump(0)));
            self.patch_jump(miss);
        }
        self.update_lexically(increment, prefix, target, span);
        for jump in found {
            self.patch_jump(jump);
        }
    }

    /// `name++` against the bindings around it, ignoring any `with` object.
    pub(super) fn update_lexically(
        &mut self,
        increment: bool,
        prefix: bool,
        target: &str,
        span: Span,
    ) {
        let (load, store) = match self.resolve_or_capture(target) {
            Resolution::Local(local) if local.celled => (
                Instruction::LoadCell(local.slot),
                Instruction::StoreCell(local.slot),
            ),
            Resolution::Local(local) => (
                Instruction::LoadLocal(local.slot),
                Instruction::StoreLocal(local.slot),
            ),
            Resolution::Upvalue(index) => (
                Instruction::LoadUpvalue(index),
                Instruction::StoreUpvalue(index),
            ),
            Resolution::Global(slot) => (
                Instruction::LoadGlobal(slot),
                Instruction::StoreGlobal(slot),
            ),
            Resolution::GlobalObject => {
                let name = self.constant_index(Constant::String(target.to_owned().into()));
                (
                    Instruction::ThrowReference(name),
                    Instruction::StoreGlobalBinding(name),
                )
            }
            // `x++` on a name nothing bound is settled against the global object, exactly as a
            // read and a write of it separately would be. The read comes first and is
            // `ThrowReference`, so an unresolvable name is the `ReferenceError` the language
            // promises without the compiler having to decide it -- which it cannot: a global the
            // program installed earlier, or one global code declared with `var`, is resolvable.
            Resolution::Unresolved => {
                let _ = span;
                let name = self.constant_index(Constant::String(target.to_owned().into()));
                (
                    Instruction::ThrowReference(name),
                    if self.strict {
                        Instruction::StoreGlobalBindingStrict(name)
                    } else {
                        Instruction::StoreGlobalBinding(name)
                    },
                )
            }
        };

        self.code.push(load);
        // `ToNumeric` first: `x++` where `x` is "5" is 5 then 6, not "5" then "51". Without a
        // conversion the `+ 1` below concatenates, which is the trap the member form fell into --
        // and `ToNumber` is not the conversion an update applies: it narrows a BigInt, so `1n++`
        // left a *number* behind and the addition would have refused to mix the two anyway.
        self.code.push(Instruction::Unary(UnaryOp::ToNumeric));
        if !prefix {
            // Keep the old value beneath the arithmetic; the store below consumes the new one.
            self.code.push(Instruction::Dup);
        }
        self.code.push(Instruction::Unary(if increment {
            UnaryOp::Increment
        } else {
            UnaryOp::Decrement
        }));
        if prefix {
            self.code.push(Instruction::Dup);
        }
        self.code.push(store);
    }

    /// An array literal, which may contain spreads.
    ///
    /// Without a spread this is one `NewArray` over the elements. With one, the array is built
    /// incrementally: the run of literal elements before each spread becomes its own `NewArray`
    /// and the spread appends to it, because a spread's length is not known until it runs.
    pub(super) fn array_literal(&mut self, elements: &[ListElement], span: Span) {
        if elements.len() > u32::MAX as usize {
            self.error(Some(span), "array literal has too many elements");
            return;
        }

        if !elements
            .iter()
            .any(|element| matches!(element, ListElement::Spread(_)))
        {
            for element in elements {
                match element {
                    ListElement::Item(value) => self.expression(value),
                    // A hole occupies its position without contributing a value, which is what
                    // makes it different from an element that happens to be undefined.
                    ListElement::Hole => self.code.push(Instruction::PushHole),
                    ListElement::Spread(_) => {
                        unreachable!("just checked that there are no spreads")
                    }
                }
            }
            self.code.push(Instruction::NewArray {
                count: elements.len() as u32,
            });
            return;
        }

        let mut pending = 0u32;
        let mut started = false;
        for element in elements {
            match element {
                ListElement::Item(value) => {
                    self.expression(value);
                    pending += 1;
                }
                ListElement::Hole => {
                    self.code.push(Instruction::PushHole);
                    pending += 1;
                }
                ListElement::Spread(value) => {
                    self.flush_array_run(&mut pending, &mut started);
                    self.expression(value);
                    self.code.push(Instruction::SpreadInto);
                }
            }
        }
        self.flush_array_run(&mut pending, &mut started);
    }

    /// Turn the pending literal elements into an array, or append them to the one being built.
    pub(super) fn flush_array_run(&mut self, pending: &mut u32, started: &mut bool) {
        if !*started {
            self.code.push(Instruction::NewArray { count: *pending });
            *started = true;
            *pending = 0;
            return;
        }
        if *pending > 0 {
            self.code.push(Instruction::NewArray { count: *pending });
            self.code.push(Instruction::SpreadInto);
            *pending = 0;
        }
    }

    /// `object[key] = value`, and the compound forms.
    ///
    /// Both the object and the key are evaluated once, in that order, which is the order the
    /// language specifies and the one a reader assumes. A compound form has to re-read through
    /// the same pair rather than re-evaluating either, so both are duplicated on the stack.
    ///
    /// Evaluating the key once is not enough on its own: `ToPropertyKey` is the visible half,
    /// and leaving it to `GetComputed` and `SetComputed` ran it twice. The compound form converts
    /// once up front, which is what the specification's Reference Record memoises. The plain form
    /// must not: there the key converts in `PutValue`, after the right-hand side has run.
    pub(super) fn computed_assignment(
        &mut self,
        operator: AssignmentOperator,
        object: &Expression,
        property: &Expression,
        value: &Expression,
    ) {
        if operator.is_short_circuiting() {
            self.short_circuiting_member(operator, object, None, Some(property), value);
            return;
        }
        self.expression(object);
        self.expression(property);

        if operator != AssignmentOperator::Assign {
            self.code.push(Instruction::ToPropertyKey);
            self.code.push(Instruction::Dup2);
            self.code.push(Instruction::GetComputed);
            self.expression(value);
            self.code
                .push(Instruction::Binary(compound_operator(operator)));
        } else {
            self.expression(value);
        }

        self.code.push(self.set_computed_instruction());
    }

    /// `object.property ??= value`, and the `&&=`/`||=` forms, static or computed.
    ///
    /// These read the property and assign only if it is the wrong one, so the right-hand side may
    /// never be evaluated and the store may never happen -- and the receiver, the key and the
    /// current value each have to survive that decision without being computed twice. Hidden
    /// locals hold them, for the same reason an update does.
    pub(super) fn short_circuiting_member(
        &mut self,
        operator: AssignmentOperator,
        object: &Expression,
        name: Option<u32>,
        key: Option<&Expression>,
        value: &Expression,
    ) {
        let span = object.span;
        let receiver = format!("%sc{}", self.hidden_bindings);
        let key_binding = format!("%sckey{}", self.hidden_bindings);
        let current = format!("%sccur{}", self.hidden_bindings);
        self.hidden_bindings += 1;
        self.scopes.push(HashMap::new());

        self.expression(object);
        self.declare_hidden(&receiver, span);
        if let Some(key) = key {
            self.expression(key);
            self.declare_hidden(&key_binding, span);
        }

        self.load_hidden(&receiver);
        match name {
            Some(name) => self.code.push(Instruction::GetProperty(name)),
            None => {
                // The key converts once and the binding keeps the converted one, so the write
                // below reads the same key the read did rather than running `toString` again.
                self.load_hidden(&key_binding);
                self.code.push(Instruction::ToPropertyKey);
                self.code.push(Instruction::Dup);
                self.store_hidden(&key_binding);
                self.code.push(Instruction::GetComputed);
            }
        }
        self.declare_hidden(&current, span);

        self.load_hidden(&current);
        let skip = self.emit_jump(short_circuit_jump(operator));

        self.load_hidden(&receiver);
        if name.is_none() {
            self.load_hidden(&key_binding);
        }
        self.expression(value);
        match name {
            Some(name) => self.code.push(self.set_property_instruction(name)),
            None => self.code.push(self.set_computed_instruction()),
        }
        let done = self.emit_jump(Instruction::Jump(0));

        // Skipped: the expression evaluates to the value that was already there.
        self.patch_jump(skip);
        self.load_hidden(&current);
        self.patch_jump(done);
        self.scopes.pop().expect("assignment scope was just pushed");
    }

    /// `object.property++`, and the other three spellings.
    ///
    /// The object and the key are each evaluated once and held in hidden locals: `o().x++` must
    /// not call `o` twice, and `a[i()]++` must not call `i` twice. That is the same rule a
    /// compound member assignment follows, and the reason neither can be lowered to
    /// `o.x = o.x + 1`.
    pub(super) fn update_member(
        &mut self,
        increment: bool,
        prefix: bool,
        object: &Expression,
        property: &ObjectKey,
        span: Span,
    ) {
        // A private name has no computed half and no prototype to fall through to, so the general
        // path below -- which exists to evaluate an object and a key exactly once each -- reduces
        // to holding the receiver. Refusing it outright made `this.#n++` a compile error, which
        // is the ordinary way a private counter is incremented.
        if let ObjectKey::Private(name) = property {
            let brand = self.private_brand(name);
            let key = self.private_key(name);
            let name = self.constant_index(Constant::String(key.into()));
            let receiver = format!("%upd{}", self.hidden_bindings);
            let old_binding = format!("%old{}", self.hidden_bindings);
            self.hidden_bindings += 1;
            self.scopes.push(HashMap::new());

            self.expression(object);
            self.declare_hidden(&receiver, span);

            // `ToNumeric` for the same reason the general path uses it: the value a postfix
            // update yields is the conversion's result, and a BigInt must not narrow.
            self.load_hidden(&receiver);
            self.push_private_brand(&brand);
            self.code.push(Instruction::GetPrivate(name));
            self.code.push(Instruction::Unary(UnaryOp::ToNumeric));
            self.declare_hidden(&old_binding, span);

            self.load_hidden(&receiver);
            self.load_hidden(&old_binding);
            self.code.push(Instruction::Unary(if increment {
                UnaryOp::Increment
            } else {
                UnaryOp::Decrement
            }));
            self.push_private_brand(&brand);
            self.code.push(Instruction::SetPrivate(name));
            if !prefix {
                self.code.push(Instruction::Pop);
                self.load_hidden(&old_binding);
            }
            self.scopes.pop().expect("update scope was just pushed");
            return;
        }

        let receiver = format!("%upd{}", self.hidden_bindings);
        let key_binding = format!("%updkey{}", self.hidden_bindings);
        let old_binding = format!("%old{}", self.hidden_bindings);
        self.hidden_bindings += 1;
        self.scopes.push(HashMap::new());

        self.expression(object);
        self.declare_hidden(&receiver, span);
        let name = match property {
            ObjectKey::Static(name) => {
                Some(self.constant_index(Constant::String(name.clone().into())))
            }
            ObjectKey::Computed(key) => {
                self.expression(key);
                self.declare_hidden(&key_binding, span);
                None
            }
            ObjectKey::Private(_) => unreachable!("refused above"),
        };

        // Read the current value, as a *numeric*: the old value a postfix update yields is the
        // conversion's result rather than whatever was stored.
        self.load_hidden(&receiver);
        match name {
            Some(name) => self.code.push(Instruction::GetProperty(name)),
            None => {
                // The key converts once and the binding keeps the converted one, so the write
                // below reads the same key the read did rather than running `toString` again.
                self.load_hidden(&key_binding);
                self.code.push(Instruction::ToPropertyKey);
                self.code.push(Instruction::Dup);
                self.store_hidden(&key_binding);
                self.code.push(Instruction::GetComputed);
            }
        }
        // `ToNumeric`, not `+ 0` and not unary plus: `+` on a string concatenates, so `o.s++`
        // where `s` is "5" would produce "50", and unary plus narrows a BigInt to a number, which
        // an update must not.
        self.code.push(Instruction::Unary(UnaryOp::ToNumeric));
        self.declare_hidden(&old_binding, span);

        // Write the new value through the same reference the read used.
        self.load_hidden(&receiver);
        if name.is_none() {
            self.load_hidden(&key_binding);
        }
        self.load_hidden(&old_binding);
        self.code.push(Instruction::Unary(if increment {
            UnaryOp::Increment
        } else {
            UnaryOp::Decrement
        }));
        match name {
            Some(name) => self.code.push(self.set_property_instruction(name)),
            None => self.code.push(self.set_computed_instruction()),
        }

        // The store leaves the new value, which is what a prefix update evaluates to; a postfix
        // one discards it and hands back what was there before.
        if !prefix {
            self.code.push(Instruction::Pop);
            self.load_hidden(&old_binding);
        }
        self.scopes.pop().expect("update scope was just pushed");
    }

    /// `object.#name ??= value`, and the `&&=`/`||=` forms.
    ///
    /// The same shape as [`Compiler::short_circuiting_member`] and for the same reason: the
    /// receiver has to survive a decision that may skip both the right-hand side and the store,
    /// so `o().#x ||= v` calls `o` once. Without this the operator fell through to the compound
    /// path, which asks `compound_operator` for the arithmetic a short-circuiting assignment
    /// does not have -- and that is unreachable, so the compiler panicked on valid code.
    pub(super) fn short_circuiting_private(
        &mut self,
        operator: AssignmentOperator,
        object: &Expression,
        name: u32,
        brand: &Option<String>,
        value: &Expression,
    ) {
        let span = object.span;
        let receiver = format!("%scp{}", self.hidden_bindings);
        let current = format!("%scpcur{}", self.hidden_bindings);
        self.hidden_bindings += 1;
        self.scopes.push(HashMap::new());

        self.expression(object);
        self.declare_hidden(&receiver, span);

        self.load_hidden(&receiver);
        self.push_private_brand(brand);
        self.code.push(Instruction::GetPrivate(name));
        self.declare_hidden(&current, span);

        self.load_hidden(&current);
        let skip = self.emit_jump(short_circuit_jump(operator));

        self.load_hidden(&receiver);
        self.expression(value);
        self.push_private_brand(brand);
        self.code.push(Instruction::SetPrivate(name));
        let done = self.emit_jump(Instruction::Jump(0));

        // Skipped: the expression evaluates to the value that was already there.
        self.patch_jump(skip);
        self.load_hidden(&current);
        self.patch_jump(done);
        self.scopes.pop().expect("assignment scope was just pushed");
    }

    pub(super) fn member_assignment(
        &mut self,
        operator: AssignmentOperator,
        object: &Expression,
        property: &str,
        value: &Expression,
    ) {
        let name = self.constant_index(Constant::String(property.to_owned().into()));
        if operator.is_short_circuiting() {
            self.short_circuiting_member(operator, object, Some(name), None, value);
            return;
        }
        self.expression(object);

        if operator != AssignmentOperator::Assign {
            // Read the current value from the same object reference the write will use.
            self.code.push(Instruction::Dup);
            self.code.push(Instruction::GetProperty(name));
            self.expression(value);
            self.code
                .push(Instruction::Binary(compound_operator(operator)));
        } else {
            self.expression(value);
        }

        // `SetProperty` leaves the assigned value, which is what an assignment expression
        // evaluates to.
        self.code.push(self.set_property_instruction(name));
    }

    pub(super) fn super_member_assignment(
        &mut self,
        operator: AssignmentOperator,
        property: &str,
        value: &Expression,
        span: Span,
    ) {
        self.require_this_binding();
        let name = self.constant_index(Constant::String(property.to_owned().into()));
        if operator.is_short_circuiting() {
            let cur_binding = format!("%scur{}", self.hidden_bindings);
            self.hidden_bindings += 1;
            self.scopes.push(HashMap::new());

            self.load_home_prototype();
            self.code.push(Instruction::Dup);
            self.code.push(Instruction::LoadSuperProperty(name));
            self.declare_hidden(&cur_binding, span);

            self.load_hidden(&cur_binding);
            let skip = self.emit_jump(short_circuit_jump(operator));
            self.code.push(Instruction::Pop);
            self.load_home_prototype();
            self.expression(value);
            self.code.push(self.set_super_property_instruction(name));
            let done = self.emit_jump(Instruction::Jump(0));

            self.patch_jump(skip);
            self.code.push(Instruction::Pop);
            self.load_hidden(&cur_binding);
            self.patch_jump(done);
            self.scopes.pop().expect("scope was just pushed");
            return;
        }

        self.load_home_prototype();
        if operator != AssignmentOperator::Assign {
            self.code.push(Instruction::Dup);
            self.code.push(Instruction::LoadSuperProperty(name));
            self.expression(value);
            self.code
                .push(Instruction::Binary(compound_operator(operator)));
        } else {
            self.expression(value);
        }
        self.code.push(self.set_super_property_instruction(name));
    }

    pub(super) fn super_computed_assignment(
        &mut self,
        operator: AssignmentOperator,
        property: &Expression,
        value: &Expression,
        span: Span,
    ) {
        self.require_this_binding();
        if operator.is_short_circuiting() {
            let key_binding = format!("%sckey{}", self.hidden_bindings);
            let cur_binding = format!("%sccur{}", self.hidden_bindings);
            self.hidden_bindings += 1;
            self.scopes.push(HashMap::new());

            self.expression(property);
            self.declare_hidden(&key_binding, span);

            self.load_hidden(&key_binding);
            self.load_home_prototype();
            self.code.push(Instruction::LoadSuperElement);
            self.declare_hidden(&cur_binding, span);

            self.load_hidden(&cur_binding);
            let skip = self.emit_jump(short_circuit_jump(operator));
            self.load_hidden(&key_binding);
            self.load_home_prototype();
            self.expression(value);
            self.code.push(self.set_super_element_instruction());
            let done = self.emit_jump(Instruction::Jump(0));

            self.patch_jump(skip);
            self.load_hidden(&cur_binding);
            self.patch_jump(done);
            self.scopes.pop().expect("scope was just pushed");
            return;
        }

        if operator != AssignmentOperator::Assign {
            let key_binding = format!("%sckey{}", self.hidden_bindings);
            self.hidden_bindings += 1;
            self.scopes.push(HashMap::new());

            self.expression(property);
            self.declare_hidden(&key_binding, span);

            self.load_hidden(&key_binding);
            self.load_home_prototype();
            self.code.push(Instruction::LoadSuperElement);
            self.expression(value);
            self.code
                .push(Instruction::Binary(compound_operator(operator)));

            let val_binding = format!("%scval{}", self.hidden_bindings);
            self.hidden_bindings += 1;
            self.declare_hidden(&val_binding, span);

            self.load_hidden(&key_binding);
            self.load_home_prototype();
            self.load_hidden(&val_binding);
            self.code.push(self.set_super_element_instruction());
            self.scopes.pop().expect("scope was just pushed");
        } else {
            self.expression(property);
            self.load_home_prototype();
            self.expression(value);
            self.code.push(self.set_super_element_instruction());
        }
    }

    /// `f?.()` and `o.m?.()`: call only when the callee is there.
    ///
    /// The callee is read into a hidden local so the nullish test has something to look at
    /// without disturbing the receiver beneath it, and the receiver into another so a method call
    /// keeps its `this` across the jump. Skipping means the arguments are never evaluated, which
    /// is the point: `maybe?.(expensive())` does not compute the argument when `maybe` is absent.
    pub(super) fn optional_call(
        &mut self,
        callee: &Expression,
        arguments: &[ListElement],
        span: Span,
    ) {
        if self.chains.is_empty() {
            self.error(Some(span), "`?.()` outside of an optional chain");
            self.push_constant(Constant::Undefined);
            return;
        }
        let receiver = format!("%oc{}", self.hidden_bindings);
        let method = format!("%ocfn{}", self.hidden_bindings);
        self.hidden_bindings += 1;
        self.scopes.push(HashMap::new());

        // A method call keeps its receiver; anything else has none and gets `undefined`.
        let has_receiver = match &callee.kind {
            ExpressionKind::StaticMember {
                object,
                property,
                optional,
            } => {
                self.expression(object);
                self.short_circuit_if_nullish(*optional, span);
                self.declare_hidden(&receiver, span);
                self.load_hidden(&receiver);
                let name = self.constant_index(Constant::String(property.clone().into()));
                self.code.push(Instruction::GetProperty(name));
                true
            }
            ExpressionKind::ComputedMember {
                object,
                property,
                optional,
            } => {
                self.expression(object);
                self.short_circuit_if_nullish(*optional, span);
                self.declare_hidden(&receiver, span);
                self.load_hidden(&receiver);
                self.expression(property);
                self.code.push(Instruction::GetComputed);
                true
            }
            _ => {
                self.expression(callee);
                false
            }
        };
        self.declare_hidden(&method, span);

        self.load_hidden(&method);
        let present = self.emit_jump(Instruction::JumpIfNotNullish(0));
        // Absent: the whole chain is undefined, and the arguments are never touched.
        self.push_constant(Constant::Undefined);
        let exit = self.emit_jump(Instruction::Jump(0));
        self.chains
            .last_mut()
            .expect("checked just above")
            .push(exit);

        self.patch_jump(present);
        if has_receiver {
            self.load_hidden(&receiver);
        } else {
            self.push_constant(Constant::Undefined);
        }
        self.load_hidden(&method);
        self.array_literal(arguments, span);
        self.code.push(Instruction::Apply);
        self.scopes.pop().expect("call scope was just pushed");
    }

    /// Whether an argument list contains a spread, and so needs its length computed at runtime.
    pub(super) fn has_spread(arguments: &[ListElement]) -> bool {
        arguments
            .iter()
            .any(|argument| matches!(argument, ListElement::Spread(_)))
    }

    /// Emit the arguments of a call with no spread, in order, returning the count.
    pub(super) fn emit_arguments(&mut self, arguments: &[ListElement]) -> u16 {
        for argument in arguments {
            let ListElement::Item(value) = argument else {
                unreachable!("callers check for spreads first");
            };
            self.expression(value);
        }
        arguments.len() as u16
    }

    pub(super) fn call(
        &mut self,
        callee: &Expression,
        arguments: &[ListElement],
        optional: bool,
        span: Span,
    ) {
        if optional {
            self.optional_call(callee, arguments, span);
            return;
        }
        let is_console_log = !Self::has_spread(arguments)
            && matches!(self.resolve("console"), Resolution::Unresolved)
            && matches!(
                &callee.kind,
                ExpressionKind::StaticMember { object, property, optional: false }
                    if property == "log"
                        && matches!(&object.kind, ExpressionKind::Identifier(name) if name == "console")
            );
        if arguments.len() > u16::MAX as usize {
            self.error(Some(span), "too many call arguments");
            return;
        }

        if is_console_log {
            // Still a fast compile-time intrinsic when there are no spreads. When spreading,
            // the call falls through to the normal method dispatch on the `console` object.
            let argc = self.emit_arguments(arguments);
            self.code.push(Instruction::HostCall {
                op: HostOp::WriteStdout,
                argc,
            });
            return;
        }

        // A method call keeps its receiver rather than reading the function out first. With a
        // spread the receiver has to be kept explicitly instead, because `Apply` takes it as an
        // operand -- so each shape below has a fixed-arity form and a spread form.
        let spreading = Self::has_spread(arguments);

        // `eval(...)`, written as that name, with nothing shadowing it: a *direct* eval, which
        // evaluates in this scope rather than in the global one. Every other way of reaching the
        // same function -- `(0, eval)(s)`, `globalThis.eval(s)`, a saved reference -- is indirect
        // and compiles to an ordinary call of `Builtin::Eval`. The difference is the language's,
        // not an optimisation: only the direct form can see the names around it.
        if let ExpressionKind::Identifier(name) = &callee.kind
            && name == "eval"
            && self.names_nothing("eval")
        {
            let mut bindings = self.eval_site_bindings(span);
            // The home object travels as an ordinary binding. `eval_site_bindings` skips every
            // name beginning with `%` -- none of them can be written by the eval'd source -- so
            // this is added afterwards rather than found there, and only where the language
            // allows `super.x` to be written at all.
            let home = self
                .super_property
                .then(|| self.home_bindings.last().cloned())
                .flatten();
            if let Some(name) = &home {
                match self.resolve_or_capture(name) {
                    Resolution::Local(local) if local.celled => bindings.push(EvalBinding {
                        name: name.clone(),
                        access: EvalAccess::Cell(local.slot),
                        declaration: EvalDeclaration::Const,
                    }),
                    Resolution::Upvalue(index) => bindings.push(EvalBinding {
                        name: name.clone(),
                        access: EvalAccess::Upvalue(index),
                        declaration: EvalDeclaration::Const,
                    }),
                    // Unreachable: a home binding is declared captured, and `cell_everything`
                    // boxes every binding of a function holding a direct eval. Reported rather
                    // than dropped, because a dropped home would make `super.x` in the eval'd
                    // source read some other slot by number.
                    _ => self.error(Some(span), "a home object binding lost its cell"),
                }
            }
            // A field initialiser is its own function boundary for `new.target` only; `super.x`
            // there reads the class's home object, which is why the two are decided separately.
            let new_target = if self.field_initialiser {
                EvalNewTarget::Undefined
            } else if self.in_function {
                EvalNewTarget::Caller
            } else {
                EvalNewTarget::Forbidden
            };
            let private_scopes = self
                .private_scopes
                .iter()
                .zip(&self.private_scope_modules)
                .zip(&self.private_declarations)
                .map(|((scope, module_id), names)| {
                    let mut names: Vec<String> = names.iter().cloned().collect();
                    names.sort();
                    PrivateScope {
                        module_id: module_id.clone(),
                        scope: *scope,
                        names,
                    }
                })
                .collect();
            let argc = if spreading {
                let eval_name = self.constant_index(Constant::String("eval".into()));
                self.code
                    .push(Instruction::LoadGlobalBindingOrUndefined(eval_name));
                self.array_literal(arguments, span);
                None
            } else {
                let eval_name = self.constant_index(Constant::String("eval".into()));
                self.code
                    .push(Instruction::LoadGlobalBindingOrUndefined(eval_name));
                Some(self.emit_arguments(arguments))
            };
            self.code.push(Instruction::DirectEval(Box::new(EvalSite {
                module_id: self.private_module_id.clone(),
                source_offset: span.start,
                bindings,
                strict: self.strict,
                argc,
                global_var_scope: self.global_var_scope && !self.compiling_function(),
                home,
                new_target,
                private_scopes,
                blocked_var_names: self.parameter_scope.clone().unwrap_or_default(),
            })));
            return;
        }
        match &callee.kind {
            // `super.m(...)` resolves the method on the home object's prototype but runs it
            // against the current receiver, which is precisely the `[this, callee, arguments]`
            // shape `Apply` takes -- no dedicated instruction is needed for either half.
            //
            // The spread-free spelling used `SuperCall` instead, which is *construction*: it
            // treats the value it pops as a parent constructor. That was invisible while
            // `super.x` could only reach a compiled method, and stopped being so the moment it
            // could reach `Object.prototype` -- `super.toString()` in a base class reported that
            // `Object.prototype.toString` is not a constructor. `SuperCall` now means `super()`
            // and nothing else.
            ExpressionKind::SuperMember { .. } | ExpressionKind::SuperComputedMember { .. } => {
                self.load_this();
                self.expression(callee);
                self.array_literal(arguments, span);
                self.code.push(Instruction::Apply);
                return;
            }
            // `this.#m()` is a method call: the private name is read off the receiver and the
            // receiver becomes `this`. Reached through `Apply` rather than a `CallPrivate`
            // instruction, because the shape `Apply` already takes -- receiver, callee,
            // arguments -- is exactly this one.
            ExpressionKind::PrivateMember {
                object,
                name,
                optional,
            } => {
                let brand = self.private_brand(name);
                let name = self.constant_index(Constant::String(self.private_key(name).into()));
                self.expression(object);
                self.short_circuit_if_nullish(*optional, span);
                self.code.push(Instruction::Dup);
                self.push_private_brand(&brand);
                self.code.push(Instruction::GetPrivate(name));
                self.array_literal(arguments, span);
                self.code.push(Instruction::Apply);
                return;
            }
            ExpressionKind::StaticMember {
                object,
                property,
                optional,
            } => {
                let name = self.constant_index(Constant::String(property.clone().into()));
                self.expression(object);
                // `a?.b()` short-circuits before the method is even read, so the receiver is the
                // thing tested and the call never happens.
                self.short_circuit_if_nullish(*optional, span);
                if spreading {
                    // The receiver stays beneath the method, which is what `Apply` expects.
                    self.code.push(Instruction::Dup);
                    self.code.push(Instruction::GetProperty(name));
                    self.array_literal(arguments, span);
                    self.code.push(Instruction::Apply);
                } else {
                    let argc = self.emit_arguments(arguments);
                    self.code.push(Instruction::CallMethod { name, argc });
                }
                return;
            }
            ExpressionKind::ComputedMember {
                object,
                property,
                optional,
            } => {
                self.expression(object);
                self.short_circuit_if_nullish(*optional, span);
                if spreading {
                    self.code.push(Instruction::Dup);
                    self.expression(property);
                    self.code.push(Instruction::GetComputed);
                    self.array_literal(arguments, span);
                    self.code.push(Instruction::Apply);
                } else {
                    self.expression(property);
                    let argc = self.emit_arguments(arguments);
                    self.code.push(Instruction::CallComputed { argc });
                }
                return;
            }
            _ => {}
        }

        // Everything else is a real call. The callee is evaluated first so it sits below its
        // arguments, which is both the evaluation order the language requires and the layout
        // the VM needs to find it at a fixed depth.
        if spreading {
            // A plain call has no receiver, and `Apply` takes one, so `undefined` stands in.
            self.push_constant(Constant::Undefined);
            self.expression(callee);
            self.array_literal(arguments, span);
            self.code.push(Instruction::Apply);
            return;
        }
        self.expression(callee);
        let argc = self.emit_arguments(arguments);
        self.code.push(Instruction::Call { argc });
    }
}
