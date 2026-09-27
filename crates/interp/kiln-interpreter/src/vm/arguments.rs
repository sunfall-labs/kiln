use super::*;

impl<H: HostOps + ?Sized> Vm<'_, H> {
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
        Value::Object(Rc::new(RefCell::new(data)))
    }
}
