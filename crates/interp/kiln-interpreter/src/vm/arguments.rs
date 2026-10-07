use super::*;
use kiln_graph::{ArgumentsAnalysis, ArrowArgumentsCaptureFacts, build_function_graph};
use std::collections::HashMap;

impl<H: HostOps + ?Sized> Vm<'_, H> {
    pub(super) fn reset_arguments_marker_for_current_frame(&mut self) {
        let depth = self.frames.len();
        if self.arguments_length_markers.len() <= depth {
            self.arguments_length_markers.resize(depth + 1, None);
        }
        self.arguments_length_markers[depth] = None;
    }

    /// Return the arrow-capture and `arguments.length` facts for one closure. Facts are keyed by
    /// exact Program identity and cached weakly in the realm's ModuleMap.
    pub(super) fn argument_object_proofs(&mut self, function: u32) -> (bool, bool) {
        let program = Rc::clone(&self.program);
        let key = Rc::as_ptr(&program) as usize;
        let module_map = self.gc_module_map();
        let cached = {
            let map = module_map.borrow();
            map.arguments_facts.get(&key).and_then(|entry| {
                entry
                    .program
                    .upgrade()
                    .filter(|cached| Rc::ptr_eq(cached, &program))
                    .map(|_| {
                        (
                            entry.arrow_captures.arrow_captures_arguments(function),
                            entry.length_only.get(&function).copied(),
                            Rc::clone(&entry.arrow_captures),
                        )
                    })
            })
        };
        if let Some((captures, Some(length_only), _)) = &cached {
            return (*captures, *length_only);
        }

        let arrow_captures = cached
            .as_ref()
            .map(|(_, _, facts)| Rc::clone(facts))
            .unwrap_or_else(|| Rc::new(ArrowArgumentsCaptureFacts::for_program(&program)));
        let analysis = ArgumentsAnalysis::with_arrow_capture_facts(&program, &arrow_captures)
            .unwrap_or_else(|| ArgumentsAnalysis::new(&program));
        let captures = arrow_captures.arrow_captures_arguments(function);
        let length_only = cached.and_then(|(_, value, _)| value).unwrap_or_else(|| {
            match program.functions.get(function as usize) {
                Some(proto)
                    if !proto.arrow
                        && !proto.generator
                        && !proto.asynchronous
                        && !proto.async_generator
                        && proto.code.iter().any(|instruction| {
                            matches!(instruction, Instruction::LoadArguments)
                        })
                        && !proto.code.iter().any(|instruction| {
                            matches!(instruction, Instruction::DirectEval(_))
                        })
                        && !proto.code.iter().any(|instruction| {
                            let Instruction::MakeFunction(child_index) = instruction else {
                                return false;
                            };
                            program.functions.get(*child_index as usize).is_some_and(
                                |child_proto| {
                                    child_proto.arrow
                                        && arrow_captures.arrow_captures_arguments(*child_index)
                                },
                            )
                        }) =>
                {
                    build_function_graph(proto, &program)
                        .ok()
                        .map(|graph| {
                            let fact = analysis.argument_length_fact(function as usize, &graph);
                            !fact.values.is_empty() && fact.reads_elided
                        })
                        .unwrap_or(false)
                }
                Some(_) | None => false,
            }
        });

        let mut map = module_map.borrow_mut();
        if map.arguments_facts.len() >= map.arguments_facts_prune_at.max(64) {
            map.arguments_facts
                .retain(|_, entry| entry.program.strong_count() > 0);
            map.arguments_facts_prune_at = map
                .arguments_facts
                .len()
                .saturating_add(map.arguments_facts.len().max(64));
        }
        let entry = map.arguments_facts.entry(key).or_insert_with(|| {
            crate::module_map::ArgumentsFactsCache {
                program: Rc::downgrade(&program),
                arrow_captures: Rc::clone(&arrow_captures),
                length_only: HashMap::new(),
            }
        });
        if entry
            .program
            .upgrade()
            .is_none_or(|cached| !Rc::ptr_eq(&cached, &program))
        {
            *entry = crate::module_map::ArgumentsFactsCache {
                program: Rc::downgrade(&program),
                arrow_captures: Rc::clone(&arrow_captures),
                length_only: HashMap::new(),
            };
        }
        entry.length_only.insert(function, length_only);
        (captures, length_only)
    }

    /// What `arguments` refers to in the running function.
    ///
    /// An arrow has none of its own and sees the enclosing function's, which it captured when it
    /// was created. Everything else sees the call it is in.
    /// The one `arguments` object this call hands out, built on first mention.
    ///
    /// Cached rather than rebuilt because `arguments` names an *object*: a write through it has
    /// to be visible to the next read, and `Object.defineProperty(arguments, "0", …)` has to
    /// survive the statement that made it. A fresh array per mention lost both silently, and
    /// `arguments === arguments` was false.
    ///
    /// An arrow has none of its own and answers with the one it captured, which is the enclosing
    /// call's -- the same object that call hands out, so a write through either is seen by both.
    pub(super) fn arguments_object(&mut self) -> Value {
        if let Some(captured) = self
            .function
            .as_ref()
            .and_then(|closure| closure.captured_arguments.clone())
        {
            return captured;
        }
        if let Some(existing) = &self.arguments_object {
            return existing.clone();
        }
        let built = self.build_arguments_object();
        self.arguments_object = Some(built.clone());
        built
    }

    /// `CreateUnmappedArgumentsObject`: an ordinary object carrying the call's arguments.
    ///
    /// It used to be an array, which is the wrong answer to four questions a program actually
    /// asks. `Array.isArray(arguments)` said true, `Object.prototype.toString.call(arguments)`
    /// said `[object Array]`, `Object.getPrototypeOf(arguments)` was `Array.prototype`, and
    /// `length` was the array's own -- so `arguments.length = 100` either threw or allocated a
    /// hundred slots, where the language says it is an ordinary writable property that nothing
    /// keeps in step with the indices. Every array-generic idiom that reaches for `arguments`
    /// -- `slice.call`, `apply`, `Array.from`, spread, `for-of` -- goes through the array-like
    /// path instead, which is the path the language defines them on.
    ///
    /// `@@iterator` is `%Array.prototype.values%` as an *own* property rather than something
    /// inherited: `arguments` inherits from `Object.prototype`, which has no iterator, so
    /// without it `[...arguments]` would stop working the moment this stopped being an array.
    ///
    /// The one thing missing is the *parameter map* of a mapped `arguments`: in a sloppy
    /// function with a simple parameter list, `arguments[0]` and the first parameter are two
    /// names for one binding. That needs an exotic `[[GetOwnProperty]]`, `[[DefineOwnProperty]]`
    /// and `[[Delete]]` reading through a live cell, in both engines, and is deliberately not
    /// attempted here. `callee` still tells the two apart, because that much is decidable from
    /// the function alone.
    pub(super) fn build_arguments_object(&mut self) -> Value {
        let mut data = ObjectData {
            arguments_brand: true,
            ..ObjectData::default()
        };
        let length = self.arguments.len();
        data.define_value("length", Value::Number(length as f64), Attributes::BUILT_IN);
        for index in 0..length {
            let value = self.arguments[index].clone();
            data.define_value(&index.to_string(), value, Attributes::DEFAULT);
        }
        // An unmapped `arguments` refuses to name its function: reading `callee` is a
        // `TypeError` rather than `undefined`, because a program that reaches for it is asking
        // for the caller identity strict mode exists to withhold. Which of the two objects this
        // is was decided where the parameter list could be seen, and travels on the instruction.
        if !self.mapped_arguments {
            data.define_accessor(
                "callee",
                Some(Value::Native(Native::ThrowTypeError, RealmId::PRIMARY)),
                Some(Value::Native(Native::ThrowTypeError, RealmId::PRIMARY)),
                Attributes::NONE,
            );
        } else {
            let callee = self
                .function
                .clone()
                .map_or(Value::Undefined, Value::Function);
            data.define_value("callee", callee, Attributes::BUILT_IN);
        }
        data.define_value(
            WellKnown::Iterator.property_key(),
            Value::Native(Native::ArrayValues, RealmId::PRIMARY),
            Attributes::BUILT_IN,
        );
        Value::Object(tracked_object(data))
    }
}
