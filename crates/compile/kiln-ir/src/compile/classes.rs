use super::*;

impl super::Compiler {
    /// Compile a class into a constructor and a prototype.
    ///
    /// The desugaring is not a shortcut -- a class really is a constructor function whose
    /// `prototype` holds the instance methods -- so it reuses the machinery `new` already needs
    /// rather than introducing a parallel object model for classes.
    /// A class expression leaves its constructor on the stack.
    pub(super) fn class_expression(&mut self, class: &Class, span: Span) {
        // A class expression may be anonymous, and its name -- when it has one -- is visible only
        // inside its own body. The scope pushed below is what keeps it there, so a named class
        // expression can bind under its own name: `class A extends B { m() { return A; } }` as an
        // expression has to find `A` from inside `m`, and nowhere else. An anonymous one gets a
        // hidden binding instead, carrying a character an identifier cannot contain.
        let binding = match &class.name {
            Some(name) => name.clone(),
            None => {
                let hidden = format!("%class{}", self.hidden_bindings);
                self.hidden_bindings += 1;
                hidden
            }
        };

        self.scopes.push(HashMap::new());
        let mut named = class.clone();
        named.name = Some(binding.clone());
        self.class_declaration(&named);
        match self.resolve(&binding) {
            Resolution::Local(local) if local.celled => {
                self.code.push(Instruction::LoadCell(local.slot))
            }
            Resolution::Local(local) => self.code.push(Instruction::LoadLocal(local.slot)),
            _ => {
                self.error(Some(span), "could not bind class expression");
                self.push_constant(Constant::Undefined);
            }
        }
        self.scopes.pop().expect("class scope was just pushed");
    }

    /// A class body is strict code, whatever encloses it.
    ///
    /// Not only its methods -- which are functions and would inherit it anyway -- but the
    /// heritage expression, the computed keys and the field initialisers, all of which are
    /// compiled into whatever code surrounds the class. So the flag is raised around the whole
    /// declaration and lowered again after it.
    pub(super) fn class_declaration(&mut self, class: &Class) {
        let outer_strict = std::mem::replace(&mut self.strict, true);
        self.strict_class_declaration(class);
        self.strict = outer_strict;
    }

    pub(super) fn strict_class_declaration(&mut self, class: &Class) {
        let Some(name) = class.name.clone() else {
            self.error(Some(class.span), "a class declaration requires a name");
            return;
        };
        // Everything compiled from here to the end of this function -- fields, methods, and any
        // class nested inside one -- names its private fields in this class's own scope.
        self.private_scopes.push(self.next_private_scope);
        self.next_private_scope += 1;
        self.private_scope_modules
            .push(self.private_module_id.clone());
        // Declared here so every method compiled below captures it; what it holds is minted
        // once, where the class is evaluated.
        let brand = self.declare_home_binding(class.span);
        self.private_brands.push(brand);
        self.private_declarations.push(
            class
                .initialisers
                .iter()
                .filter_map(ClassInitialiser::as_field)
                .map(|field| &field.key)
                .chain(class.methods.iter().map(|method| &method.key))
                .filter_map(|key| match key {
                    ObjectKey::Private(name) => Some(name.clone()),
                    _ => None,
                })
                .collect(),
        );
        // A derived class evaluates its parent once, into a binding its members capture.
        //
        // The name carries a character an identifier cannot contain, so it can never collide
        // with one the program wrote, and a counter so that two derived classes in one scope get
        // separate bindings. `super` is syntax rather than a variable, but it resolves exactly
        // like a captured one -- giving it a real binding lets it reuse the closure machinery
        // instead of needing a parallel path through every scope.
        if let Some(superclass) = &class.superclass {
            let binding = format!("%super{}", self.super_binding_count);
            self.super_binding_count += 1;
            self.captured.insert(binding.clone());
            self.variable(VariableKind::Const, &binding, Some(superclass), class.span);
            self.super_bindings.push(binding);
        }

        // The two home objects a class has. An instance method, the constructor and every field
        // initialiser are defined on `C.prototype`; a static method and a static field are
        // defined on `C` itself. Declared here, before anything that could capture one is
        // compiled, and filled in below once the constructor exists -- a method closes over the
        // binding, not over what it held at the time.
        let instance_home = self.declare_home_binding(class.span);
        let static_home = self.declare_home_binding(class.span);

        // Bound before the body is compiled, so a method can refer to the class by name.
        self.variable(VariableKind::Let, &name, None, class.span);

        // A class expression with no written name still needs a binding to hold itself, and
        // `class_expression` invents one carrying a `%` that no identifier can contain. That
        // binding is scaffolding: `(class {}).name` is the empty string, and reporting the
        // scaffolding instead put `"%class0"` where the program could read it.
        // A `%` carries the scaffolding binding an anonymous class expression is held in, which
        // the program cannot spell and must never read back. What it *does* read back is the name
        // a named evaluation supplied -- `var C = class {}` is `"C"` -- and the empty string only
        // when nothing named it.
        let (written_name, inferred_name) = if name.starts_with('%') {
            // Scaffolding is never the name: an anonymous class reads back "" unless a
            // named evaluation inferred one. `Some("")`, not `None`, so the constructor
            // carries the empty spelling rather than no spelling.
            (Some(String::new()), class.inferred_name.clone())
        } else {
            (Some(name.clone()), None)
        };
        let constructor = match &class.constructor {
            // The constructor *is* the class as a value, so it carries the class's name: that is
            // what `new.target.name` and a console's `C {}` both read.
            Some(constructor) => Function {
                name: written_name.clone(),
                inferred_name: inferred_name.clone(),
                // The class's span, not the constructor's. The constructor *is* the class as a
                // value, and `Function.prototype.toString` on a class answers with the whole
                // `class C { ... }` text -- the methods included -- never with the one member
                // that happens to back it.
                span: class.span,
                ..constructor.clone()
            },
            // A class with no constructor gets an empty one, so `new` needs no special case for
            // classes that declared none.
            None => Function {
                rest: None,
                return_type: None,
                asynchronous: false,
                name: written_name,
                inferred_name,
                // A class binds its own name through `class_expression`, not through this.
                binds_own_name: false,
                arrow: false,
                generator: false,
                parameters: Vec::new(),
                body: Vec::new(),
                strict: true,
                span: class.span,
            },
        };
        let synthesized_derived = class.constructor.is_none() && class.superclass.is_some();
        // A private instance method or accessor is installed on each *instance*, not on the
        // prototype: that is what carries the brand, and what makes `D.f()` on a sibling class a
        // TypeError rather than a call. The kind travels with it, because an accessor read is a
        // call and a method write is a refusal -- neither of which a plain slot can express.
        //
        // Installed *before* the field initialisers rather than after, which is the order the
        // language specifies and an observable one: a field initialiser may call `this.#m()`.
        let private_elements: Vec<(String, PrivateKind, Function)> = class
            .methods
            .iter()
            .filter(|method| !method.is_static)
            .filter_map(|method| {
                let ObjectKey::Private(name) = &method.key else {
                    return None;
                };
                Some((
                    name.clone(),
                    private_kind(method.kind),
                    method.function.clone(),
                ))
            })
            .collect();
        let instance_fields: Vec<&ClassField> = class
            .initialisers
            .iter()
            .filter_map(ClassInitialiser::as_field)
            .filter(|field| !field.is_static)
            .collect();
        self.home_bindings.push(instance_home.clone());
        // Each private method and accessor is created *once*, here, where the class is defined,
        // and kept in a binding the constructor captures. Creating it inside the constructor
        // made a fresh function per instance, so `a.#m === b.#m` was false and a brand check
        // written as an identity comparison failed. It also closed over the constructor's
        // locals rather than the class, which is not the environment the specification gives it.
        // The brand: one fresh object per *evaluation* of this class. An empty array is the
        // cheapest thing with an identity, and nothing ever looks inside it -- only whether an
        // instance's brand is this one.
        if let Some(brand) = self.private_brands.last().cloned() {
            self.code.push(Instruction::NewArray { count: 0 });
            self.store_home_binding(&brand);
        }
        let mut private_bindings: Vec<(String, PrivateKind, String)> =
            Vec::with_capacity(private_elements.len());
        for (name, kind, function) in &private_elements {
            self.next_is_method = true;
            let Some(body) = self.compile_function(function, class.span) else {
                self.home_bindings.pop();
                return;
            };
            let binding = self.declare_home_binding(class.span);
            self.code.push(Instruction::MakeFunction(body));
            self.store_home_binding(&binding);
            private_bindings.push((name.clone(), *kind, binding));
        }
        // A field's *name* is evaluated where the class is defined, once. The binding is
        // declared here so the constructor compiled below captures it; what it holds is computed
        // further down, once the class binding exists to be the receiver `ToPropertyKey` reads.
        let field_bindings: Vec<(&ClassField, Option<String>)> = instance_fields
            .iter()
            .map(|field| {
                let binding = matches!(field.key, ObjectKey::Computed(_))
                    .then(|| self.declare_home_binding(class.span));
                (*field, binding)
            })
            .collect();
        let compiled = self.compile_constructor(
            &constructor,
            class.span,
            Some(ClassBody {
                derived: class.superclass.is_some(),
                // A *derived* class with no constructor is not empty: it forwards its arguments
                // to the parent. Without that, `class F extends Error {}` would construct an
                // error with no message.
                forwards_to_super: synthesized_derived,
                private_elements: &private_bindings,
                fields: &field_bindings,
            }),
        );
        self.home_bindings.pop();
        let Some(index) = compiled else {
            return;
        };

        let Resolution::Local(local) = self.resolve(&name) else {
            self.error(Some(class.span), format!("could not bind class `{name}`"));
            return;
        };
        self.code.push(Instruction::MakeFunction(index));
        self.code.push(if local.celled {
            Instruction::StoreCell(local.slot)
        } else {
            Instruction::StoreLocal(local.slot)
        });

        // The constructor carries the brand too: a static private member lives on it, and the
        // check that reads one has to tell this evaluation's constructor from another's.
        if let Some(brand) = self.private_brands.last().cloned() {
            let key = self.constant_index(Constant::String(self.own_private_brand_key().into()));
            self.load_local_or_cell(local);
            self.load_captured_binding(&brand);
            self.code.push(self.set_property_instruction(key));
            self.code.push(Instruction::Pop);
        }

        let prototype = self.constant_index(Constant::String("prototype".to_owned().into()));

        // A computed field name -- instance or static -- is evaluated and converted once, here,
        // where the class is defined: one whose `toString` has an effect has it once rather than
        // once per instance, and one that refuses does so when the class is written. Every
        // element's key is evaluated in source order, a field's between the methods around it
        // (issue 28): evaluating the static fields' keys with their values, before every method
        // key, ran `static [b] = 1` ahead of an earlier `[a]() {}`. A static field's value still
        // runs after every element, below. The class is pushed beneath the key because that is
        // the pair `ToPropertyKey` reads; storing the name consumes it, and the receiver is
        // dropped afterwards. A static field's name is refused as `prototype` where it is
        // evaluated, before the next key, which is where node refuses it.
        //
        // Every key is evaluated before the heritage is checked (`link_heritage`, below the
        // elements): node runs `k` in `class U extends 1 { [k()]() {} }` and then throws.
        let mut static_field_keys: Vec<Option<String>> = Vec::new();
        let mut field_keys: Vec<(u32, &Expression, bool, String)> = Vec::new();
        {
            let mut instance = field_bindings.iter();
            for initialiser in &class.initialisers {
                let ClassInitialiser::Field(field) = initialiser else {
                    static_field_keys.push(None);
                    continue;
                };
                if field.is_static {
                    let binding = match &field.key {
                        ObjectKey::Computed(key) => {
                            let binding = self.declare_home_binding(class.span);
                            field_keys.push((key.span.start, key, true, binding.clone()));
                            Some(binding)
                        }
                        _ => None,
                    };
                    static_field_keys.push(binding);
                    continue;
                }
                static_field_keys.push(None);
                if let Some((instance_field, binding)) = instance.next()
                    && let (ObjectKey::Computed(key), Some(binding)) =
                        (&instance_field.key, binding)
                {
                    field_keys.push((key.span.start, key, false, binding.clone()));
                }
            }
        }
        let mut field_keys = field_keys.into_iter().peekable();

        for method in &class.methods {
            // Every field key written before this method's key is evaluated first. Only a
            // computed key has an order anyone can observe, so a method with a literal key is
            // defined where it falls.
            if let ObjectKey::Computed(key) = &method.key {
                while let Some((_, key_expression, is_static, binding)) =
                    field_keys.next_if(|(start, _, _, _)| *start < key.span.start)
                {
                    self.emit_class_field_key(local, key_expression, is_static, &binding);
                }
            }
            self.home_bindings.push(if method.is_static {
                static_home.clone()
            } else {
                instance_home.clone()
            });
            self.next_is_method = true;
            let compiled = self.compile_function(&method.function, class.span);
            self.home_bindings.pop();
            let Some(body) = compiled else {
                return;
            };

            // Static methods hang off the constructor; instance methods off its prototype.
            self.load_local_or_cell(local);
            if !method.is_static {
                self.code.push(Instruction::GetProperty(prototype));
            }

            let accessor = !matches!(method.kind, kiln_hir::MethodKind::Normal);
            match &method.key {
                // Installed on each instance by the constructor prologue instead -- an accessor
                // as much as a method, because both carry the brand and the brand is per object.
                ObjectKey::Private(_) if !method.is_static => {
                    self.code.push(Instruction::Pop);
                    continue;
                }
                // A static private element lives on the constructor, and with its kind: a static
                // private getter read as a slot answered with the getter *function*, which is a
                // wrong answer rather than a missing feature.
                ObjectKey::Private(name) => {
                    let key = self.private_key(name);
                    let name = self.constant_index(Constant::String(key.into()));
                    self.code.push(Instruction::MakeFunction(body));
                    self.code.push(Instruction::DefinePrivate {
                        name,
                        kind: private_kind(method.kind),
                    });
                }
                ObjectKey::Static(key) if !accessor => {
                    self.push_constant(Constant::String(key.clone().into()));
                    self.code.push(Instruction::MakeFunction(body));
                    self.code.push(Instruction::DefineMethod);
                }
                ObjectKey::Static(key) => {
                    self.push_constant(Constant::String(key.clone().into()));
                    self.code.push(Instruction::MakeFunction(body));
                    self.code.push(Instruction::DefineAccessor {
                        setter: matches!(method.kind, kiln_hir::MethodKind::Set),
                        enumerable: false,
                    });
                }
                // The key converts where it is written, before the next element's key is
                // evaluated; the define that follows converted it too late, and natively not at
                // all (issue 24). The constructor or prototype is beneath it.
                ObjectKey::Computed(key) => {
                    self.expression(key);
                    self.code.push(Instruction::ToPropertyKey);
                    self.code.push(Instruction::MakeFunction(body));
                    if accessor {
                        self.code.push(Instruction::DefineAccessor {
                            setter: matches!(method.kind, kiln_hir::MethodKind::Set),
                            enumerable: false,
                        });
                    } else {
                        self.code.push(Instruction::DefineMethod);
                    }
                }
            }
            // Each definition leaves something; nothing consumes it.
            self.code.push(Instruction::Pop);
        }
        for (_, key_expression, is_static, binding) in field_keys {
            self.emit_class_field_key(local, key_expression, is_static, &binding);
        }

        // Link the two prototypes so instances inherit the parent's methods, and the two
        // constructors so static methods are inherited too -- after every element key, which is
        // node's order, and before any static field initialiser runs.
        if class.superclass.is_some() {
            self.link_heritage(local, prototype);
        }

        // Filled after the prototype link, so that reading `C.prototype`'s own prototype from
        // inside a method answers the parent's -- and before the static fields, because a static
        // field initialiser runs now and may be the first thing to read one.
        self.load_local_or_cell(local);
        self.store_home_binding(&static_home);
        self.load_local_or_cell(local);
        self.code.push(Instruction::GetProperty(prototype));
        self.store_home_binding(&instance_home);

        // Static fields are ordinary properties of the constructor, assigned once here -- after
        // every element has been defined, so `static x = C.m()` finds a static method written
        // below it -- and a `static { }` block is a body run once, here, with `this` bound to
        // the constructor. One loop over one list because the two interleave and the order is
        // observable.
        self.home_bindings.push(static_home.clone());
        for (initialiser, key_binding) in class.initialisers.iter().zip(&static_field_keys) {
            let field = match initialiser {
                ClassInitialiser::Field(field) if field.is_static => field,
                ClassInitialiser::Field(_) => continue,
                // Compiled as the function it is and called immediately with the constructor as
                // its receiver -- which is the whole of the block's semantics: `this` is the
                // class, `var` stays inside, and `super.x` reads through the same home binding a
                // static method does.
                ClassInitialiser::StaticBlock(body) => {
                    self.next_is_method = true;
                    let Some(block) = self.compile_function(body, class.span) else {
                        self.home_bindings.pop();
                        return;
                    };
                    self.load_local_or_cell(local);
                    self.code.push(Instruction::MakeFunction(block));
                    self.code.push(Instruction::NewArray { count: 0 });
                    self.code.push(Instruction::Apply);
                    self.code.push(Instruction::Pop);
                    continue;
                }
            };
            self.load_local_or_cell(local);
            match (&field.key, key_binding) {
                (ObjectKey::Private(name), _) => {
                    let name = self.constant_index(Constant::String(self.private_key(name).into()));
                    self.emit_field_value(field, Some(&static_home));
                    self.code.push(self.set_property_instruction(name));
                }
                (ObjectKey::Static(key), _) => {
                    let name = self.constant_index(Constant::String(key.clone().into()));
                    self.emit_field_value(field, Some(&static_home));
                    self.code.push(Instruction::DefineOwnProperty(name));
                }
                // The name was evaluated, converted and checked with the other element keys.
                (ObjectKey::Computed(_), Some(binding)) => {
                    self.load_captured_binding(binding);
                    self.emit_field_value(field, Some(&static_home));
                    self.code.push(Instruction::DefineOwnComputed);
                }
                (ObjectKey::Computed(key), None) => {
                    self.expression(key);
                    self.emit_field_value(field, Some(&static_home));
                    self.code.push(Instruction::DefineOwnComputed);
                }
            }
            self.code.push(Instruction::Pop);
        }
        self.home_bindings.pop();

        if class.superclass.is_some() {
            self.super_bindings.pop();
        }
        self.private_scopes
            .pop()
            .expect("this class pushed its own private scope");
        self.private_scope_modules
            .pop()
            .expect("this class pushed its module identity");
        self.private_brands
            .pop()
            .expect("this class pushed its own brand");
        self.private_declarations
            .pop()
            .expect("this class pushed its own private names");
    }

    /// Evaluate a class field's computed name and convert it with `ToPropertyKey`, into the
    /// hidden binding the field's definition reads; a static field's is refused as `prototype`
    /// there too. The class sits beneath the key because that is the pair `ToPropertyKey` reads;
    /// it is dropped once the name is stored.
    fn emit_class_field_key(
        &mut self,
        local: Local,
        key: &Expression,
        is_static: bool,
        binding: &str,
    ) {
        self.load_local_or_cell(local);
        self.expression(key);
        if is_static {
            self.guard_static_prototype_key();
        } else {
            self.code.push(Instruction::ToPropertyKey);
        }
        self.store_home_binding(binding);
        self.code.push(Instruction::Pop);
    }

    /// ClassDefinitionEvaluation's heritage steps, in their order, for the class in `local`.
    ///
    /// `null` links the prototype object to `null` and leaves the constructor's own link at
    /// `Function.prototype`. Anything else must be a constructor -- `SetPrototype` with a
    /// function target is where both engines check that, first, before `prototype` is read --
    /// and its `prototype` must be an object or `null`. Reading `prototype` first and linking
    /// whatever came back let `class U extends 1 {}` define a class natively, and made
    /// `extends null` a `TypeError` in both engines (issue 25).
    fn link_heritage(&mut self, local: Local, prototype: u32) {
        self.load_super();
        self.push_constant(Constant::Null);
        self.code.push(Instruction::Binary(BinaryOp::StrictEqual));
        let not_null = self.emit_jump(Instruction::JumpIfFalse(0));
        self.load_local_or_cell(local);
        self.code.push(Instruction::GetProperty(prototype));
        self.push_constant(Constant::Null);
        self.code.push(Instruction::SetPrototype);
        self.code.push(Instruction::Pop);
        let done = self.emit_jump(Instruction::Jump(0));

        self.patch_jump(not_null);
        self.load_local_or_cell(local);
        self.load_super();
        self.code.push(Instruction::SetPrototype);
        self.code.push(Instruction::Pop);

        self.load_local_or_cell(local);
        self.code.push(Instruction::GetProperty(prototype));
        self.load_super();
        self.code.push(Instruction::GetProperty(prototype));
        // `typeof` is "object" for an object and for `null`, and "function" for a callable
        // object; every other answer is a primitive, which cannot be a prototype.
        let mut valid = Vec::new();
        for kind in ["object", "function"] {
            self.code.push(Instruction::Dup);
            self.code.push(Instruction::Unary(UnaryOp::TypeOf));
            self.push_constant(Constant::String(kind.to_owned().into()));
            self.code.push(Instruction::Binary(BinaryOp::StrictEqual));
            valid.push(self.emit_jump(Instruction::JumpIfTruthy(0)));
        }
        self.code.push(Instruction::LoadBuiltin(Builtin::TypeError));
        self.push_constant(Constant::String(
            "class heritage does not have a valid prototype property"
                .to_owned()
                .into(),
        ));
        self.code.push(Instruction::New { argc: 1 });
        self.code.push(Instruction::Throw);
        for jump in valid {
            self.patch_jump(jump);
        }
        self.code.push(Instruction::SetPrototype);
        self.code.push(Instruction::Pop);
        self.patch_jump(done);
    }

    /// Push the `[[Prototype]]` of the home object of the method currently being compiled.
    ///
    /// Read at each `super.x` rather than fixed when the method was defined, because that is
    /// where the specification reads it: `Object.setPrototypeOf(C.prototype, X)` changes what
    /// `super` means inside `C`'s methods from that moment on, and a link recorded at class
    /// definition would keep answering the old one.
    pub(super) fn load_home_prototype(&mut self) {
        let Some(binding) = self.home_bindings.last().cloned() else {
            // Unreachable from any program the frontend accepts -- `super.x` outside a method is
            // a syntax error before it reaches here -- so this reports rather than invents.
            self.error(None, "`super` outside of a method");
            self.push_constant(Constant::Undefined);
            return;
        };
        match self.resolve_or_capture(&binding) {
            Resolution::Local(local) if local.celled => {
                self.code.push(Instruction::LoadCell(local.slot));
            }
            Resolution::Local(local) => self.code.push(Instruction::LoadLocal(local.slot)),
            Resolution::Upvalue(index) => self.code.push(Instruction::LoadUpvalue(index)),
            Resolution::Global(slot) => self.code.push(Instruction::LoadGlobal(slot)),
            Resolution::GlobalObject | Resolution::Unresolved => {
                self.error(None, "`super` outside of a method");
                self.push_constant(Constant::Undefined);
                return;
            }
        }
        self.code.push(Instruction::LoadPrototype);
    }

    /// Reserve a hidden binding for a home object, celled so a method body can capture it.
    ///
    /// The value is stored later, by [`Compiler::store_home_binding`], because the object it
    /// names does not exist yet when the methods that capture it are compiled -- a method
    /// closes over the *binding*, and reads it when it runs.
    /// Reserve the hidden binding a derived constructor's `this` lives in, in its dead zone.
    ///
    /// Not `variable`, which would store `undefined` into it and so take it straight back out of
    /// the zone. The cell is created and marked uninitialised and nothing else: the only thing
    /// that fills it is the `super()` that brings the instance into existence.
    pub(super) fn declare_this_binding(&mut self) -> String {
        let binding = format!("%this{}", self.this_binding_count);
        self.this_binding_count += 1;
        self.captured.insert(binding.clone());
        let slot = self.local_count;
        self.local_count += 1;
        let scope = self.scopes.len() - 1;
        self.scopes[scope].insert(
            binding.clone(),
            Local {
                slot,
                kind: VariableKind::Let,
                imported: false,
                // Initialised as far as *resolution* is concerned: the check is the cell's, at
                // run time, because `super()` may be written in a branch or in an arrow and no
                // compile-time flag can say whether it ran.
                initialized: true,
                celled: true,
                eval_declaration: None,
            },
        );
        self.code.push(Instruction::NewCell(slot));
        self.code.push(Instruction::MarkUninitialized(slot));
        binding
    }

    /// Push `this` from the hidden binding, raising the dead zone's `ReferenceError` if `super()`
    /// has not run.
    pub(super) fn load_this_binding(&mut self, binding: &str) {
        match self.resolve_or_capture(binding) {
            Resolution::Local(local) => self.code.push(Instruction::LoadCell(local.slot)),
            Resolution::Upvalue(index) => self.code.push(Instruction::LoadUpvalue(index)),
            _ => {
                self.error(None, "a derived constructor's `this` binding lost its cell");
                self.code.push(Instruction::LoadThis);
            }
        }
    }

    /// Record that `super()` has returned, so `this` may be read from here on.
    ///
    /// A second `super()` therefore does *not* raise -- the binding is simply written again.
    /// That is a narrower fix than the specification's, which makes the second call a
    /// `ReferenceError`, and it is named rather than hidden: what is fixed here is reading `this`
    /// too early, not calling `super()` twice.
    pub(super) fn store_this_binding(&mut self) {
        let Some(binding) = self.this_binding.clone() else {
            return;
        };
        self.code.push(Instruction::LoadThis);
        match self.resolve_or_capture(&binding) {
            Resolution::Local(local) => self.code.push(Instruction::StoreCell(local.slot)),
            Resolution::Upvalue(index) => self.code.push(Instruction::StoreUpvalue(index)),
            _ => {
                self.error(None, "a derived constructor's `this` binding lost its cell");
                self.code.push(Instruction::Pop);
            }
        }
    }

    /// Push `this`, from wherever this context keeps it.
    pub(super) fn load_this(&mut self) {
        match self.this_binding.clone() {
            Some(binding) => self.load_this_binding(&binding),
            None => self.code.push(Instruction::LoadThis),
        }
    }

    /// `GetThisBinding()`, for its effect only.
    ///
    /// `super.x` resolves against `this` even though it does not read it, so writing one before
    /// `super()` in a derived constructor is a `ReferenceError` just as writing `this` is.
    pub(super) fn require_this_binding(&mut self) {
        if self.this_binding.is_some() {
            self.load_this();
            self.code.push(Instruction::Pop);
        }
    }

    pub(super) fn declare_home_binding(&mut self, span: Span) -> String {
        let binding = format!("%home{}", self.home_binding_count);
        self.home_binding_count += 1;
        self.captured.insert(binding.clone());
        self.variable(VariableKind::Let, &binding, None, span);
        binding
    }

    /// Store the value on top of the stack into a hidden home binding, consuming it.
    pub(super) fn store_home_binding(&mut self, binding: &str) {
        match self.resolve(binding) {
            Resolution::Local(local) => self.code.push(if local.celled {
                Instruction::StoreCell(local.slot)
            } else {
                Instruction::StoreLocal(local.slot)
            }),
            _ => {
                self.error(None, "a home object binding lost its slot");
                self.code.push(Instruction::Pop);
            }
        }
    }

    /// Push the parent constructor of the class currently being compiled.
    pub(super) fn load_super(&mut self) {
        let Some(binding) = self.super_bindings.last().cloned() else {
            self.error(None, "`super` outside of a derived class");
            self.push_constant(Constant::Undefined);
            return;
        };
        match self.resolve_or_capture(&binding) {
            Resolution::Local(local) if local.celled => {
                self.code.push(Instruction::LoadCell(local.slot))
            }
            Resolution::Local(local) => self.code.push(Instruction::LoadLocal(local.slot)),
            Resolution::Upvalue(index) => self.code.push(Instruction::LoadUpvalue(index)),
            Resolution::Global(slot) => self.code.push(Instruction::LoadGlobal(slot)),
            Resolution::GlobalObject | Resolution::Unresolved => {
                self.error(None, "`super` outside of a derived class");
                self.push_constant(Constant::Undefined);
            }
        }
    }

    /// A field with no initialiser is `undefined`, not absent: the property exists.
    /// Emit a field's initialiser, in the context the language gives it.
    ///
    /// An initialiser is a function of its own to the specification even though Kiln emits it
    /// inline: `super.x` reads through the class's home object -- which the caller has already
    /// pushed -- and `new.target` is `undefined` rather than whatever constructed the instance.
    /// Both are restored afterwards, because the code around the initialiser is not in one.
    pub(super) fn emit_field_value(&mut self, field: &ClassField, static_this: Option<&str>) {
        let outer_super_property = std::mem::replace(&mut self.super_property, true);
        let outer_field_initialiser = std::mem::replace(&mut self.field_initialiser, true);
        // An initialiser is its own function to the language, so its `this` is the instance
        // being built rather than any enclosing constructor's dead-zoned binding. An instance
        // field only runs after `super()` anyway; a *static* one is emitted where the class is
        // defined, which may be inside a derived constructor that has not called it yet.
        let outer_this_binding = self.this_binding.take();
        if let Some(binding) = static_this {
            self.this_binding = Some(binding.to_owned());
        }
        match &field.value {
            Some(value) => self.expression(value),
            None => self.push_constant(Constant::Undefined),
        }
        self.super_property = outer_super_property;
        self.field_initialiser = outer_field_initialiser;
        self.this_binding = outer_this_binding;
    }

    pub(super) fn load_local_or_cell(&mut self, local: Local) {
        self.code.push(if local.celled {
            Instruction::LoadCell(local.slot)
        } else {
            Instruction::LoadLocal(local.slot)
        });
    }

    /// Compile a function body into its own proto and bind the name to a callable.
    ///
    /// Declarations are not hoisted yet: the binding exists from this point on, so calling a
    /// function textually above its declaration is an unresolved identifier rather than a call
    /// to `undefined`. That is a smaller lie than hoisting half-way would be, and it is a
    /// diagnostic rather than a wrong answer.
    /// Compile a function declaration and store it in its already-created binding.
    ///
    /// The binding exists before the body is compiled, which is what makes a function able to
    /// call itself and to be called from above its own text: the body resolves its own name
    /// through the enclosing scope, and the slot it reads is filled by the `MakeFunction` here
    /// before any call can reach it.
    pub(super) fn emit_function_declaration(&mut self, function: &Function, span: Span) {
        let Some(name) = function.name.clone() else {
            self.error(Some(span), "a function declaration requires a name");
            return;
        };
        // Normally created by the hoisting pass. A function declaration can also appear where a
        // single statement is expected -- `if (x) function f() {}` -- and there is no block to
        // have hoisted it.
        //
        // The test is the *innermost* scope rather than resolution, and the difference is the
        // whole of Annex B B.3.4: an `if` arm is read as though a block were written around it,
        // so `function (f) { if (true) function f() {} }` declares a binding of its own there.
        // Asking `resolve` instead found the enclosing parameter and overwrote it, which is
        // exactly what B.3.2.1 declines to do for a formal.
        let innermost = self.scopes.len() - 1;
        let global_here =
            self.global_var_scope && innermost == 0 && self.global_bindings.contains(&name);
        if !self.scopes[innermost].contains_key(&name) && !global_here {
            self.predeclare(VariableKind::Let, &name, true);
        }

        let Some(index) = self.compile_function(function, span) else {
            return;
        };

        match self.resolve(&name) {
            Resolution::Local(local) => {
                self.code.push(Instruction::MakeFunction(index));
                self.code.push(if local.celled {
                    Instruction::StoreCell(local.slot)
                } else {
                    Instruction::StoreLocal(local.slot)
                });
                self.emit_annexb_var_write(&name, local);
            }
            // A function declaration at the top of global code: the binding is a property of the
            // global object, created by `DeclareGlobalBinding` when the scope was entered and
            // given its value here.
            Resolution::GlobalObject => {
                self.code.push(Instruction::MakeFunction(index));
                self.store_global_object_binding(&name);
            }
            // The binding was just created in the current function's innermost scope, so any
            // other resolution means the scope bookkeeping is inconsistent.
            _ => self.error(Some(span), format!("could not bind function `{name}`")),
        }
    }

    /// Compile one function body, returning its index in [`Program::functions`].
    ///
    /// The body is compiled against a fresh state: its own code buffer, its own slot numbering
    /// starting at the parameters, and its own scope stack. Module scope stays reachable through
    /// `enclosing[0]`; anything between is a capture and is refused by `resolve`.
    pub(super) fn compile_function(&mut self, function: &Function, span: Span) -> Option<u32> {
        self.compile_constructor(function, span, None)
    }

    /// Install a class's private methods and instance fields on `this`.
    ///
    /// Methods and accessors first, then fields: a field initialiser may call `this.#m()`, and
    /// the language installs the elements that make that call possible before it runs any
    /// initialiser.
    pub(super) fn emit_instance_elements(
        &mut self,
        private_elements: &[(String, PrivateKind, String)],
        fields: &[(&ClassField, Option<String>)],
        _span: Span,
    ) -> Option<()> {
        // Every instance carries the brand of the evaluation that made it, stored beside its
        // private members and read by each access to tell one evaluation from another.
        if let Some(brand) = self.private_brands.last().cloned() {
            let key = self.constant_index(Constant::String(self.own_private_brand_key().into()));
            self.code.push(Instruction::LoadThis);
            self.load_captured_binding(&brand);
            self.code.push(self.set_property_instruction(key));
            self.code.push(Instruction::Pop);
        }
        for (name, kind, binding) in private_elements {
            let key = self.private_key(name);
            let name = self.constant_index(Constant::String(key.into()));
            // The class made this function once; every instance is given *that* one, which is
            // what makes `a.#m === b.#m` true.
            self.code.push(Instruction::LoadThis);
            self.load_captured_binding(binding);
            self.code
                .push(Instruction::DefinePrivate { name, kind: *kind });
            self.code.push(Instruction::Pop);
        }
        for (field, name_binding) in fields {
            self.code.push(Instruction::LoadThis);
            if let Some(binding) = name_binding {
                // The name was computed and converted once, where the class is defined.
                self.load_captured_binding(binding);
                self.emit_field_value(field, None);
                self.code.push(Instruction::DefineOwnComputed);
                self.code.push(Instruction::Pop);
                continue;
            }
            match &field.key {
                ObjectKey::Private(name) => {
                    let name = self.constant_index(Constant::String(self.private_key(name).into()));
                    self.emit_field_value(field, None);
                    self.code.push(self.set_property_instruction(name));
                }
                ObjectKey::Static(key) => {
                    let name = self.constant_index(Constant::String(key.clone().into()));
                    self.emit_field_value(field, None);
                    self.code.push(Instruction::DefineOwnProperty(name));
                }
                ObjectKey::Computed(key) => {
                    self.expression(key);
                    self.emit_field_value(field, None);
                    self.code.push(Instruction::DefineOwnComputed);
                }
            }
            self.code.push(Instruction::Pop);
        }
        Some(())
    }

    /// The elements a derived constructor owes its instance, emitted where `super()` just
    /// returned.
    ///
    /// The `super()` expression has left the parent's answer on the stack and every sequence
    /// below is stack-balanced, so the expression's own value survives being followed by this.
    pub(super) fn emit_elements_after_super(&mut self, span: Span) {
        let Some(Some(elements)) = self.pending_elements.last().cloned() else {
            return;
        };
        let fields: Vec<(&ClassField, Option<String>)> = elements
            .fields
            .iter()
            .map(|(field, binding)| (field, binding.clone()))
            .collect();
        self.emit_instance_elements(&elements.private, &fields, span);
    }

    /// Compile a constructor, whose prologue initialises the class's instance fields.
    ///
    /// Fields run after `super()` and before the body, which is the order the language specifies
    /// and the only one that works: a field initialiser may read `this`, and in a derived class
    /// `this` does not exist until the parent has run.
    pub(super) fn compile_constructor(
        &mut self,
        function: &Function,
        span: Span,
        class: Option<ClassBody<'_>>,
    ) -> Option<u32> {
        if function.parameters.len() > u32::MAX as usize {
            self.error(Some(span), "too many parameters");
            return None;
        }
        if self.functions.len() >= u32::MAX as usize {
            self.error(Some(span), "too many functions in one module");
            return None;
        }

        // Which of this function's own bindings its nested functions refer to. Computed before
        // the body is compiled because a declaration has to know, at the point it allocates its
        // slot, whether that slot holds a value or a cell.
        let mut captured = names_captured_by_this_function(function);
        // `super` is syntax, not an identifier, so the ordinary name walk cannot discover the
        // hidden binding that represents it. Every callable compiled while a derived class is
        // active may emit `load_super` (an explicit call/member access, or a synthesized
        // forwarding constructor) and must carry that binding with the closure. Otherwise an
        // exported derived class reads the caller module's slot by number when constructed.
        if let Some(super_binding) = self.super_bindings.last() {
            captured.insert(super_binding.clone());
        }
        // A `with` object is a hidden binding for the same reason, and the name walk cannot see
        // it either. A function written inside a `with` body still resolves its names against
        // that object when it is called, which may be long after the statement has ended -- so
        // the closure has to carry it.
        for with in &self.with_scopes {
            captured.insert(with.name.clone());
        }
        // A direct `eval` anywhere in this function -- including inside one nested in it -- means
        // the eval'd source may name any binding here, so every one of them has to be a cell.
        let cell_everything = function_contains_direct_eval(function);

        // Every parameter owns a positional slot, whether or not it binds a name there. A plain
        // identifier binds the slot itself, which keeps the common call free of any prologue; a
        // pattern leaves the slot anonymous and is destructured out of it below, into ordinary
        // locals that behave like any other binding.
        let mut parameters = HashMap::with_capacity(function.parameters.len());
        let mut celled_parameters = Vec::new();
        let mut binding_steps = Vec::new();
        // Whether this list has a dead zone to keep at all. Without a default there is nothing
        // that can run while a parameter is unbound, so a function with none pays nothing and
        // behaves exactly as it did.
        let parameters_have_defaults = function
            .parameters
            .iter()
            .any(|parameter| matches!(parameter.pattern, Pattern::WithDefault { .. }));
        for (slot, parameter) in function.parameters.iter().enumerate() {
            let slot = slot as u32;
            let identifier = match &parameter.pattern {
                Pattern::Identifier(name) => Some(name),
                Pattern::WithDefault { inner, .. } => match inner.as_ref() {
                    Pattern::Identifier(name) => Some(name),
                    _ => None,
                },
                _ => None,
            };
            let Some(name) = identifier else {
                binding_steps.push(ParameterStep::Destructure(
                    slot,
                    &parameter.pattern,
                    parameter.span,
                ));
                continue;
            };
            if parameters.contains_key(name) {
                self.error(
                    Some(parameter.span),
                    format!("duplicate parameter `{name}`"),
                );
                return None;
            }
            let celled = captured.contains(name) || cell_everything;
            if celled {
                celled_parameters.push(slot);
            }
            parameters.insert(
                name.clone(),
                Local {
                    slot,
                    // Parameters share a function's variable environment with `var`.
                    // Their initialization rules are still handled below, but `var x` in a
                    // body whose parameter is `x` must reuse this slot.
                    kind: VariableKind::Var,
                    imported: false,
                    // Uninitialised until the binding list reaches it, which is what puts the
                    // parameters to a default's right in a dead zone.
                    initialized: !parameters_have_defaults,
                    celled,
                    eval_declaration: None,
                },
            );
            binding_steps.push(
                if matches!(parameter.pattern, Pattern::WithDefault { .. }) {
                    ParameterStep::Defaulted(slot, celled, &parameter.pattern, parameter.span, name)
                } else {
                    ParameterStep::Plain(name)
                },
            );
        }

        let parameter_count = function.parameters.len() as u32;
        // Annex B declines to give a block-level function declaration a var-scoped binding when
        // the name is a formal parameter or `arguments`: the call already owns that binding, and
        // the block's function must not take it over. `arguments` is here even when the function
        // never mentions it, because the object exists whether or not it is named.
        let mut parameter_names: HashSet<String> = parameters.keys().cloned().collect();
        if let Some(rest) = &function.rest {
            let mut rest_names = Vec::new();
            pattern_names_in_order(rest, &mut rest_names);
            parameter_names.extend(rest_names);
        }
        if !function.arrow {
            parameter_names.insert("arguments".to_owned());
        }
        // A rest parameter is already excluded: it lives in `function.rest` rather than among
        // the positional ones, so counting stops at the first default on its own.
        let declared_length = function
            .parameters
            .iter()
            .take_while(|parameter| !matches!(parameter.pattern, Pattern::WithDefault { .. }))
            .count() as u32;
        let outer_finalizer_depth =
            std::mem::replace(&mut self.function_finalizer_depth, self.finalizers.len());
        // A nested body cannot jump out to an enclosing loop, so it starts with no handlers of
        // its own to account for.
        let outer_handler_depth = std::mem::replace(&mut self.handler_depth, 0);
        // An arrow cannot contain a `yield`, so it never reads this; every other function body
        // establishes its own generator kind, and restores the enclosing one on the way out.
        let outer_async_generator = std::mem::replace(
            &mut self.async_generator,
            function.generator && function.asynchronous,
        );
        // Strictness is inherited and never given up: a body written inside strict code is
        // strict whether or not it says so, and a `"use strict"` of its own turns it on for
        // this body and everything nested in it. Restored on the way out, because a sibling
        // written after a strict function is not strict because of it.
        let inherited_strict = self.strict || function.strict;
        let outer_strict = std::mem::replace(&mut self.strict, inherited_strict);
        // `IsSimpleParameterList`, and the strictness beside it: together they are the
        // specification's own test for which of the two `arguments` objects this function gets.
        //
        // An arrow inherits rather than deciding, because the `arguments` it sees is not its
        // own: it is the enclosing call's, and so is the kind.
        let mapped_arguments = if function.arrow {
            self.mapped_arguments
        } else {
            !inherited_strict
                && function.rest.is_none()
                && function
                    .parameters
                    .iter()
                    .all(|parameter| matches!(parameter.pattern, Pattern::Identifier(_)))
        };
        let outer_mapped_arguments =
            std::mem::replace(&mut self.mapped_arguments, mapped_arguments);
        // An arrow has no boundary of its own for any of these: `super.x`, `new.target` and a
        // field initialiser's `undefined` new.target all reach through it to where it was
        // written. Every other function is a boundary, and `next_is_method` -- set by whichever
        // caller is compiling a method body -- is what says whether this one was written as a method.
        // It is taken either way, so it cannot leak into the next function compiled.
        let method = std::mem::take(&mut self.next_is_method);
        let is_method = method && !function.arrow && class.is_none();
        let is_class = class.is_some();
        let outer_in_function = self.in_function;
        let outer_super_property = self.super_property;
        let outer_field_initialiser = self.field_initialiser;
        let outer_this_binding = self.this_binding.take();
        let outer_derived_constructor = std::mem::replace(
            &mut self.derived_constructor,
            class.as_ref().is_some_and(|class| class.derived),
        );
        if !function.arrow {
            self.in_function = true;
            self.super_property =
                (method && self.home_bindings.last().is_some()) || class.is_some();
            self.field_initialiser = false;
        } else {
            // An arrow has no `this` of its own, so it reads the enclosing one -- including a
            // derived constructor's, which is what makes `constructor() { const go = () =>
            // super(); go(); }` legal and its `this` still dead until `go` runs.
            self.this_binding.clone_from(&outer_this_binding);
        }
        // A function body has a variable environment of its own, whatever encloses it: `var i`
        // inside a function written at the top of a script is that call's binding and not a
        // property of the global object. `global_bindings` stays, because the names the *program*
        // put on the object are still resolvable from in here -- it is only new declarations that
        // stop going there.
        let outer_global_var_scope = std::mem::replace(&mut self.global_var_scope, false);
        let outer = FunctionState {
            code: std::mem::take(&mut self.code),
            scopes: std::mem::replace(&mut self.scopes, vec![parameters]),
            local_count: std::mem::replace(&mut self.local_count, parameter_count),
            upvalues: std::mem::take(&mut self.upvalues),
            upvalue_names: std::mem::take(&mut self.upvalue_names),
            captured: std::mem::replace(&mut self.captured, captured),
            cell_everything: std::mem::replace(&mut self.cell_everything, cell_everything),
        };
        self.enclosing.push(outer);

        // A class constructor is not callable. `[[Call]]` on one throws before it looks at an
        // argument, which is why this is the first thing in the body rather than part of the
        // parameter prologue: `Derived.apply(obj, new Array(100))` must throw without binding a
        // hundred parameters, and `C()` on a class whose constructor has a default must throw
        // without running it.
        //
        // Emitted as ordinary bytecode rather than a flag on the proto, because `new.target` is
        // exactly the question being asked and every backend already answers it -- so the C
        // backend gets the same refusal from the same instructions rather than a second
        // implementation that could disagree with this one.
        if class.is_some() {
            self.code.push(Instruction::LoadNewTarget);
            let constructed = self.emit_jump(Instruction::JumpIfNotNullish(0));
            self.code.push(Instruction::LoadBuiltin(Builtin::TypeError));
            self.push_constant(Constant::String(
                match &function.name {
                    Some(name) if !name.is_empty() => {
                        format!("Class constructor {name} cannot be invoked without 'new'")
                    }
                    _ => "Class constructor cannot be invoked without 'new'".to_owned(),
                }
                .into(),
            ));
            self.code.push(Instruction::New { argc: 1 });
            self.code.push(Instruction::Throw);
            self.patch_jump(constructed);
        }

        // A derived constructor's `this` starts in a dead zone that only `super()` ends.
        if class.as_ref().is_some_and(|class| class.derived) {
            self.this_binding = Some(self.declare_this_binding());
        }

        // A captured parameter arrives as a plain value in its slot, so the cell has to be
        // created and seeded before the body can read it.
        for slot in celled_parameters {
            self.code.push(Instruction::LoadLocal(slot));
            self.code.push(Instruction::NewCell(slot));
            self.code.push(Instruction::StoreCell(slot));
        }

        // Bind the parameters in declaration order, before anything in the body can read them.
        // In *declaration* order rather than patterns-then-defaults, because the order is
        // observable: a default is an expression, and it runs after everything to its left has
        // been bound and before anything to its right has. `bind_pattern` is the same code a
        // `const { a } = x` uses, so defaults, renaming, nesting and rest behave identically
        // wherever they are written.
        let outer_dead_zone =
            std::mem::replace(&mut self.parameter_dead_zone, parameters_have_defaults);
        // A direct `eval` written in one of these defaults declares into the *body's* variable
        // environment, with this parameter record sitting between the two -- so a `var` naming
        // anything the record holds is an early error. Only a list with defaults has a record of
        // its own; without one there is nowhere for an `eval` to be written anyway.
        //
        // `parameter_names` is the set the record holds: the formals, the rest pattern's names,
        // and `arguments` for anything but an arrow. Sorted, because the bytecode is a cache key.
        let outer_parameter_scope = std::mem::replace(
            &mut self.parameter_scope,
            parameters_have_defaults.then(|| {
                let mut names: Vec<String> = parameter_names.iter().cloned().collect();
                names.sort();
                names
            }),
        );
        for step in binding_steps {
            match step {
                ParameterStep::Destructure(slot, pattern, span) => {
                    self.code.push(Instruction::LoadLocal(slot));
                    self.bind_pattern(VariableKind::Let, pattern, span);
                }
                ParameterStep::Defaulted(slot, celled, pattern, span, name) => {
                    self.code.push(if celled {
                        Instruction::LoadCell(slot)
                    } else {
                        Instruction::LoadLocal(slot)
                    });
                    self.assign_pattern(pattern, span);
                    // Past its own default, and so out of the dead zone for everything after it.
                    self.initialize(name);
                }
                // Already in its slot: the call put it there, and there is nothing to emit.
                ParameterStep::Plain(name) => self.initialize(name),
            }
        }
        self.parameter_dead_zone = outer_dead_zone;
        // The body's variable environment is the one an `eval` written there declares into, so
        // there is nothing between the two and nothing to block. Restored on the way out.
        self.parameter_scope = None;

        // `...rest` is the arguments beyond the declared parameters. Taken from the call rather
        // than from slots, because its length is decided by the caller.
        if let Some(rest) = &function.rest {
            self.code.push(Instruction::RestArguments(parameter_count));
            self.bind_pattern(VariableKind::Let, rest, function.span);
        }

        // Everything above binds parameters, and so is what the language calls
        // FunctionDeclarationInstantiation; everything below is the body. A generator has to be
        // able to run the first without the second, so the seam is recorded rather than left
        // implicit in the instruction stream.
        let prologue_end = self.code.len() as u32;

        // Every `var` in the body belongs to *this* function, so its slot and its `undefined`
        // are established here rather than wherever the block containing it happens to be
        // compiled.
        self.hoist_var_names(&function.body);
        // After the `var`s, because Annex B only adds a binding a `var` did not already make --
        // and after the parameters, whose names it must not take over.
        self.hoist_annexb_function_names(&function.body, &parameter_names);
        if let Some(class) = &class {
            if class.forwards_to_super {
                // `super(...arguments)`, and nothing else can run before it -- so by the line
                // after, the instance is already the parent's and the elements go in there.
                self.load_super();
                self.code.push(Instruction::SuperForward);
                self.code.push(Instruction::Pop);
                self.store_this_binding();
            }
            // A derived class with a constructor of its own installs its elements when *that*
            // constructor's `super()` returns, which is a place only the body knows. Anywhere
            // else -- a base class, or the forward just emitted -- `this` is already the
            // instance, so here is the place.
            if class.installs_elements_at_super() {
                self.pending_elements.push(Some(class.elements()));
            } else {
                self.emit_instance_elements(class.private_elements, class.fields, span)?;
                // Pushed even with nothing owed: that is what stops a `super()` in a *nested*
                // class's constructor from reaching past it to this one's elements.
                self.pending_elements.push(None);
            }
        }
        self.statements(&function.body);
        // An ordinary function never took an entry, because an arrow is where a derived
        // constructor's `super()` is allowed to be written -- `constructor() { const go = () =>
        // super(); go(); }` is legal -- and an entry there would hide the constructor's own
        // elements from it.
        if class.is_some() {
            self.pending_elements.pop();
        }
        // Falling off the end of a function returns undefined -- except a derived constructor,
        // which returns `this`, and so raises the dead zone's ReferenceError when it reaches
        // here without having called `super()`. Emitted unconditionally rather than only when
        // the body cannot fall through: proving that needs flow analysis, and an unreachable
        // epilogue costs two instructions.
        if self.derived_constructor {
            let binding = self
                .this_binding
                .clone()
                .expect("a derived constructor has a hidden `this` binding");
            self.load_this_binding(&binding);
        } else {
            self.push_constant(Constant::Undefined);
        }
        self.code.push(Instruction::Return);

        let restored = self
            .enclosing
            .pop()
            .expect("this function pushed its own enclosing state");
        self.global_var_scope = outer_global_var_scope;
        let strict = std::mem::replace(&mut self.strict, outer_strict);
        self.mapped_arguments = outer_mapped_arguments;
        self.in_function = outer_in_function;
        self.super_property = outer_super_property;
        self.field_initialiser = outer_field_initialiser;
        self.this_binding = outer_this_binding;
        self.derived_constructor = outer_derived_constructor;
        let parameter_types: Vec<Option<TypeAnnotation>> = function
            .parameters
            .iter()
            .map(|p| p.type_annotation)
            .collect();
        let type_annotations =
            if parameter_types.iter().any(|p| p.is_some()) || function.return_type.is_some() {
                Some(crate::TypeAnnotations {
                    parameters: parameter_types,
                    return_type: function.return_type,
                })
            } else {
                None
            };
        let proto = FunctionProto {
            name: function.name.clone(),
            inferred_name: function.inferred_name.clone(),
            arrow: function.arrow,
            asynchronous: function.asynchronous,
            // An async body suspends at every `await`, which is a `yield` in every way the
            // machine cares about -- so it is compiled as a generator and driven by the async
            // driver rather than by the program.
            generator: function.generator || function.asynchronous,
            async_generator: function.generator && function.asynchronous,
            is_method,
            is_class,
            parameter_count,
            mapped_arguments,
            declared_length,
            local_count: std::mem::replace(&mut self.local_count, restored.local_count),
            prologue_end,
            code: std::mem::replace(&mut self.code, restored.code),
            upvalues: std::mem::replace(&mut self.upvalues, restored.upvalues),
            span: function.span,
            type_annotations,
        };
        self.scopes = restored.scopes;
        self.function_finalizer_depth = outer_finalizer_depth;
        self.handler_depth = outer_handler_depth;
        self.async_generator = outer_async_generator;
        self.upvalue_names = restored.upvalue_names;
        self.captured = restored.captured;
        self.cell_everything = restored.cell_everything;
        self.parameter_scope = outer_parameter_scope;

        let index = self.functions.len() as u32;
        self.functions.push(proto);
        self.strict_functions.push(strict);
        Some(index)
    }
}
