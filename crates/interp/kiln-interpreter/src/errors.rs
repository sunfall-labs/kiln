use super::*;

#[derive(Clone, Debug, PartialEq)]
pub enum ExecutionError {
    ImportCountMismatch {
        expected: usize,
        actual: usize,
    },
    StepLimitExceeded {
        steps: u64,
        limit: u64,
    },
    /// This agent's group is shutting down and it must stop where it is.
    ///
    /// Never seen by the main agent's program: it is how an agent thread unwinds when the case
    /// that started it has finished, so that the thread can be joined rather than leaked.
    AgentTerminated,
    /// `process.exit(code)` was called to terminate execution.
    ProcessExited(i32),
    InvalidInstructionPointer {
        instruction: usize,
    },
    ConstantOutOfBounds {
        index: usize,
    },
    LocalOutOfBounds {
        index: usize,
    },
    StackUnderflow {
        instruction: usize,
    },
    InvalidJump {
        instruction: usize,
        target: usize,
    },
    Host {
        instruction: usize,
        message: String,
    },
    ModuleNotFound {
        instruction: usize,
        message: String,
    },
    /// A `MakeFunction` or `Call` naming a proto the program does not contain.
    UnknownFunction {
        function: u32,
    },
    /// A property name that is not a string constant, which the compiler never emits.
    InvalidPropertyKey {
        instruction: usize,
        value: &'static str,
    },
    /// `new` on something that is not a constructor.
    NotConstructable {
        instruction: usize,
        value: String,
    },
    /// Arithmetic mixing a BigInt with a number.
    ///
    /// A catchable `TypeError` in the specification, and refused rather than widened because
    /// silently converting either way loses exactly what a BigInt is for.
    MixedBigInt,
    /// A radix or digit count outside the range its method accepts.
    ///
    /// A catchable `RangeError`, in the specification and here -- see [`Self::CyclicJson`].
    RadixOutOfRange,
    InvalidTimeout,
    InvalidPort,
    InvalidHttpStatus,
    /// A `size` a set-like reported as a negative number, which no collection can have.
    SetSizeOutOfRange,
    /// An index outside the array `with` was asked to copy.
    ArrayIndexOutOfRange,
    /// An instant with no ISO spelling reaching `toISOString`.
    ///
    /// Its own variant rather than [`Self::RadixOutOfRange`]: both are `RangeError`s, and a
    /// program that catches one and prints it would otherwise be told an invalid date was a bad
    /// radix.
    DateOutOfRange,
    /// `reduce` on an empty array with no initial value.
    ///
    /// A catchable `TypeError`, in the specification and here -- see [`Self::CyclicJson`].
    EmptyReduce,
    /// `JSON.parse` was given text that is not JSON.
    ///
    /// A catchable `SyntaxError`, in the specification and here -- see [`Self::CyclicJson`].
    InvalidJson,
    /// A pattern the regexp engine will not take.
    ///
    /// Catchable, because a program can build one out of user input -- and the frontend cannot
    /// refuse it, because `new RegExp(text)` has no text to look at until it runs.
    InvalidRegExp,
    /// `JSON.stringify` reached a value that contains itself.
    ///
    /// A catchable `TypeError` in the specification, and catchable here.
    ///
    /// This is the anchor the variants around it point at, so the mechanism is written down once:
    /// [`Vm::catchable`] turns any error whose [`Self::as_javascript_error`] answers a kind into a
    /// thrown value and hands control to the handler, and every variant in that mapping is
    /// catchable provided its raising site is reached through `catchable` rather than through a
    /// bare `?`.
    ///
    /// The comment that stood here for a long time said the opposite -- that a native could not
    /// yet raise a value a program could catch. That was true once and outlived the mechanism,
    /// and it is worth knowing that it cost a reader most of an afternoon: the claim is exactly
    /// the kind a careful codebase gets trusted on. If you add a variant a program should be able
    /// to catch, put it in the mapping *and* check that whatever raises it is reached through
    /// `catchable`; a bare `?` from inside `run_frames` is what makes one fatal by accident.
    CyclicJson,
    /// `JSON.stringify` reached a BigInt.
    ///
    /// A catchable `TypeError`, in the specification and here -- see [`Self::CyclicJson`]. It has
    /// its own variant rather than borrowing the cycle's because the two share nothing but their
    /// error kind, and a program that catches this should not be told its value contains itself.
    BigIntNotSerialisable,
    /// `eval("...")` or `Function("...")`: building a function from source at run time.
    ///
    /// A catchable error, in the specification and here -- see [`Self::CyclicJson`]. It is not a
    /// missing builtin but a missing *realm capability*: compiling source during execution needs
    /// the compiler present at run time, which is a different kind of realm from the one an
    /// ahead-of-time compiler targets. `Error` rather than `Type`, because nothing about the
    /// argument is wrong -- this realm cannot do it.
    ///
    /// Non-string arguments are unaffected: `eval(1)` is specified to hand back its argument, and
    /// still does.
    FunctionFromSource,
    /// ToPrimitive reached the end without a primitive: no `@@toPrimitive`, and `valueOf` and
    /// `toString` either absent or both answering objects.
    ///
    /// A catchable `TypeError`, in the specification and here -- see [`Self::CyclicJson`].
    ///
    /// This used to answer `[object Object]` and carry on, which is not one wrong value but a
    /// wrong program: every line after the fault ran, and the process exited 0 where the language
    /// ends it.
    NotPrimitive,
    /// A `.return()` signal finished unwinding a generator. Never surfaces to a caller: it is
    /// caught by the `return` that raised it.
    GeneratorReturned,
    /// An exception that reached the top of the program.
    Uncaught {
        instruction: usize,
        message: String,
        /// The thrown value's constructor name, if it has one.
        ///
        /// JavaScript permits throwing primitives and arbitrary objects whose string
        /// representation happens to start with `TypeError:`. Conformance must judge the
        /// constructor rather than parsing that representation. This deliberately records the
        /// nearest constructor, so `class E extends TypeError {}` remains `E` rather than being
        /// mistaken for `TypeError`.
        error_constructor: Option<String>,
        /// The functions that were on the stack, innermost first.
        ///
        /// Not `Error.prototype.stack`, which is a property the program can read and which this
        /// deliberately is not: there are no line numbers to put in one, and inventing a
        /// node-shaped string that disagrees with node's would be worse than having none. This
        /// is for the human reading the failure, and it is the difference between "somewhere in
        /// six thousand lines" and a name to grep for.
        trace: Vec<String>,
        /// The thrown value's `stack` property, when it is an error that has one, exactly as
        /// the program would read it: header, then one `    at function (file:line:column)`
        /// line per frame. This is what a host prints for an uncaught error, and it is the
        /// same text the native backend prints from the same property, so the two reports are
        /// the same bytes. `message` stays the short description conformance runners judge.
        stack: Option<String>,
    },
    /// An exception reached a handler the *host* installed rather than one the program wrote.
    ///
    /// Never escapes the call that installed it: [`Vm::call_catching`] turns it back into the
    /// thrown value. It exists because a rejection carries a value and an `ExecutionError`
    /// carries a message, and a promise needs the value.
    CaughtByHost,
    /// A throw whose handler is below the current Rust re-entry.
    ///
    /// The value rides in [`Vm::caught`], like `CaughtByHost`, and for the same reason. It is
    /// distinct because the two want opposite things: `CaughtByHost` means a built-in asked for
    /// the value and will keep it, and this means nobody has taken it yet and the *same*
    /// handler must still receive it once the Rust stack is flat.
    ThrownAcrossNative,
    /// Any operation on a proxy whose `revoke` has been called.
    RevokedProxy,
    /// A proxy nested inside a value being serialised.
    ProxyNotSerialisable,
    /// `array.length = x` where `x` is not a whole number in range.
    InvalidArrayLength,
    /// `String.fromCodePoint` handed something that is not a code point.
    ///
    /// Carries what was written rather than a fixed sentence: the specification refuses a
    /// fraction, a `NaN`, an infinity and anything above `U+10FFFF` under one rule, and the
    /// value is the only thing that says which of them this was.
    InvalidCodePoint {
        value: String,
    },
    /// A `TypeError` the specification states with wording of its own.
    ///
    /// Every other variant here stands for one condition and builds its message from it. The
    /// property-definition checks cannot: `ValidateAndApplyPropertyDescriptor` refuses for six
    /// different reasons and a program that catches one reads the message to tell them apart,
    /// so the message travels with the error rather than being reconstructed from a name.
    TypeErrorMessage {
        message: String,
    },
    /// Invalid external key material is an ordinary catchable `Error`, as Node's crypto API
    /// reports parser failures; argument-shape errors remain `TypeErrorMessage`.
    ErrorMessage {
        message: String,
    },
    /// A `RangeError` with wording of its own, for the same reason as the pair around it: an
    /// index outside a view is refused for several different reasons and the message is what
    /// tells them apart.
    RangeErrorMessage {
        message: String,
    },
    /// A `ReferenceError` with wording of its own, for the same reason as the pair above.
    ///
    /// Rare, because almost every reference error names an identifier the machine already has.
    /// A disposed `DisposableStack` is the exception: the receiver is the right kind of thing,
    /// and what is missing is the resources it used to name.
    ReferenceErrorMessage {
        message: String,
    },
    /// `next()`, `return()` or `throw()` on a generator that is already executing.
    GeneratorAlreadyRunning,
    /// A byte buffer larger than [`MAX_BUFFER_BYTES`]. Like [`Self::ArrayTooLarge`] this is a
    /// limit of the implementation and is deliberately not catchable -- and unlike an oversized
    /// `Vec`, an oversized buffer request is an `abort` rather than a panic, which no amount of
    /// `catch_unwind` in the harness can contain.
    BufferTooLarge {
        bytes: f64,
    },
    /// `resize` on a buffer created without a `maxByteLength`. A `TypeError` rather than a
    /// no-op: a fixed-length buffer is not a resizable one that happens to refuse, and a
    /// program that cannot tell the two apart will read the wrong bytes afterwards.
    BufferNotResizable,
    /// `grow` on a shared buffer created without a `maxByteLength`, for the same reason
    /// [`Self::BufferNotResizable`] exists: a fixed-length buffer is not a growable one that
    /// happens to refuse.
    BufferNotGrowable,
    /// An operation on a buffer whose bytes have been transferred away, or through a view onto
    /// one. A `TypeError` and not an empty read: a detached buffer has no bytes at all, and
    /// handing back zeroes would let a program carry on believing it still holds its data.
    BufferDetached,
    /// A write through a buffer that promised never to change again -- or a detach, or a resize,
    /// which are the two largest writes there are. A `TypeError` rather than a silent no-op,
    /// because the promise is what the type is *for*: a program handed an immutable buffer is
    /// entitled to be told when it tried to break it rather than to watch the write vanish.
    BufferImmutable,
    /// A buffer length outside what the specification accepts -- negative, unrepresentable, or
    /// above the ceiling the buffer was created with.
    BufferLengthOutOfRange {
        length: f64,
    },
    /// An `ArrayBuffer.prototype` method reached through `call` with a receiver that is not a
    /// buffer. The specification checks the internal slot before it does anything else, and
    /// answering `undefined` instead would let the program carry on as though it had a buffer.
    NotABuffer,
    /// A `DataView.prototype` accessor reached with a receiver that is not a view, for the same
    /// reason and with the same consequence as [`Self::NotABuffer`].
    NotADataView,
    /// An access through a `DataView` whose window a resize has left outside its buffer. A
    /// `TypeError`, because there is nothing left to read *through* -- distinct from an index
    /// past the end of a window that is still valid, which is a `RangeError`.
    DataViewOutOfBounds,
    /// A `DataView` offset or length that does not fit: an element read past the end of the
    /// view, or a view built past the end of its buffer.
    DataViewOutOfRange,
    /// `ToNumber` of a symbol, which the specification refuses outright.
    SymbolIsNotANumber,
    /// A symbol reaching `ToString`, which has no spelling to give back. `String(symbol)` is the
    /// deliberate conversion and the only one the specification allows.
    SymbolIsNotAString,
    /// A calendar-engine failure that arrives already classified, so a `RangeError` from
    /// `temporal_rs` reaches the program as a `RangeError` rather than as a generic fault.
    TemporalFailure {
        kind: ErrorKind,
        message: String,
    },
    /// An `Intl` refusal, carrying the constructor ECMA-402 names for it.
    ///
    /// The kind travels with the message because the specification is unusually precise
    /// about which of the two it is -- an out-of-range option value is a `RangeError` and a
    /// wrong-typed one is a `TypeError` -- and a large part of the corpus tests only that.
    IntlFailure {
        kind: ErrorKind,
        message: String,
    },
    /// `ToNumber` of a BigInt. Refused for the same reason the other direction is: the two
    /// numeric types never cross implicitly.
    BigIntIsNotANumber,
    /// `ToBigInt` of something that is not one -- a number, `undefined`, a symbol.
    NotABigInt,
    /// `ToBigInt` of a string that is not an integer literal. A `SyntaxError`, because a BigInt
    /// has no NaN to become.
    InvalidBigIntString,
    /// A copy between two typed arrays whose elements are in different numeric towers.
    ///
    /// Its own refusal rather than a per-element one, because the specification makes it before
    /// looking at a single element: copying an empty `BigInt64Array` into a `Uint8Array` throws
    /// just as a full one does, and there is no element there to blame it on.
    TypedArrayContentTypeMismatch,
    /// An iterator method reached with a receiver that cannot be stepped, or a `flatMap` mapper
    /// that answered something that is not iterable.
    NotAnIterator,
    /// A `next` that answered a primitive. The specification requires an object, and reading
    /// `done` off a primitive would quietly see `undefined` and loop forever.
    IteratorResultNotAnObject,
    /// A count for `take` or `drop` that is NaN or negative.
    IteratorLimitOutOfRange,
    /// A valid array length this dense representation cannot materialise. See
    /// [`MAX_DENSE_ARRAY`]. Deliberately absent from `as_javascript_error`: it is a limit of
    /// this implementation rather than anything the specification defines, so a program must
    /// not be able to catch it and carry on as though the array had been created.
    ArrayTooLarge {
        length: usize,
    },
    /// A string operation whose result would exceed [`MAX_STRING_STORAGE_BYTES`]. Unlike the two limits
    /// above this one *is* in `as_javascript_error`, because it is not only an implementation
    /// limit: the specification bounds a string's length, V8 raises `RangeError: Invalid string
    /// length` at its own bound, and Test262 has tests that build a string toward 2^36 units and
    /// accept the throw as the answer. Refusing uncatchably would fail those on a technicality;
    /// not refusing at all takes the process down and reports nothing for the rest of the run.
    StringTooLong {
        bytes: usize,
    },
    /// A percent-escape that is not two hex digits, or that decodes to invalid UTF-8.
    MalformedUri,
    /// `push` and its siblings on a frozen array.
    FrozenArray,
    /// `eval` or `new Function` reached with no compiler installed for this realm.
    ///
    /// Deliberately absent from `as_javascript_error`: this is the *engine* saying it has no
    /// frontend, not the language saying anything, and a program that caught it would carry on
    /// as though the source had been rejected on its merits.
    NoSourceCompiler,
    /// `import()` reached with no module loader installed for this realm.
    ///
    /// Absent from `as_javascript_error` for the same reason [`Self::NoSourceCompiler`] is: a
    /// rejection here would be indistinguishable, to the program, from a module that genuinely
    /// could not be found.
    NoModuleLoader,
    /// A module `import()` reached that this compiler cannot build yet.
    ///
    /// Kept apart from a rejection for the same reason [`Self::EvalUnsupported`] is kept apart
    /// from a `SyntaxError`: a program told the module failed will take the branch it wrote for
    /// a failing module rather than failing itself.
    DynamicImportUnsupported {
        message: String,
    },
    /// Source handed to `eval` or `new Function` that is not a program.
    SyntaxErrorMessage {
        message: String,
    },
    /// Source that parses but that this compiler cannot lower yet.
    ///
    /// Absent from `as_javascript_error` for the same reason [`Self::NoSourceCompiler`] is: a
    /// program told `SyntaxError` would believe its own source was rejected, and take the branch
    /// it wrote for that instead of failing.
    EvalUnsupported {
        message: String,
    },
    /// A valid Node crypto input requires an algorithm or encoding this runtime has not built.
    ///
    /// This is an uncatchable capability refusal rather than a JavaScript `TypeError`: reporting
    /// a malformed key would let the program take the branch it wrote for bad caller input.
    CryptoUnsupported {
        message: String,
    },
    /// A property assignment on something that is not an object.
    NotAnObject {
        instruction: usize,
        value: &'static str,
    },
    /// A capture descriptor pointing past the creating closure's upvalues.
    MissingUpvalue {
        index: u32,
    },
    /// A capture or cell access on a slot that was never boxed.
    NotACell {
        slot: usize,
    },
    NotCallable {
        instruction: usize,
        value: &'static str,
    },
    CallDepthExceeded {
        limit: usize,
    },
    GlobalOutOfBounds {
        index: usize,
        available: usize,
        function: Option<String>,
    },
}

impl fmt::Display for ExecutionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ImportCountMismatch { expected, actual } => write!(
                formatter,
                "module expected {expected} imported values but received {actual}"
            ),
            Self::StepLimitExceeded { steps, limit } => {
                write!(
                    formatter,
                    "execution exceeded {limit} steps after {steps} steps"
                )
            }
            Self::AgentTerminated => write!(formatter, "the agent was stopped"),
            Self::ProcessExited(code) => write!(formatter, "process exited with code {code}"),
            Self::InvalidInstructionPointer { instruction } => {
                write!(formatter, "invalid instruction pointer {instruction}")
            }
            Self::ConstantOutOfBounds { index } => write!(formatter, "constant {index} is missing"),
            Self::LocalOutOfBounds { index } => write!(formatter, "local {index} is missing"),
            Self::StackUnderflow { instruction } => {
                write!(formatter, "stack underflow at instruction {instruction}")
            }
            Self::InvalidJump {
                instruction,
                target,
            } => {
                write!(
                    formatter,
                    "invalid jump from instruction {instruction} to {target}"
                )
            }
            Self::InvalidPropertyKey { instruction, value } => write!(
                formatter,
                "instruction {instruction} used a {value} as a property name"
            ),
            Self::NotConstructable { instruction, value } => write!(
                formatter,
                "instruction {instruction} used `new` on a {value}, which is not a constructor"
            ),
            Self::MixedBigInt => write!(
                formatter,
                "cannot mix BigInt and other types, use explicit conversions"
            ),
            Self::RadixOutOfRange => {
                write!(
                    formatter,
                    "argument is out of the range this method accepts"
                )
            }
            Self::InvalidTimeout => write!(formatter, "timeout is out of range"),
            Self::InvalidPort => write!(formatter, "port is outside 0..=65535"),
            Self::InvalidHttpStatus => write!(formatter, "invalid HTTP status code"),
            Self::DateOutOfRange => write!(formatter, "date has no ISO spelling"),
            Self::SetSizeOutOfRange => write!(formatter, "set size is negative"),
            Self::ArrayIndexOutOfRange => write!(formatter, "index is outside the array"),
            Self::EmptyReduce => write!(formatter, "reduce of empty array with no initial value"),
            Self::InvalidJson => write!(formatter, "unexpected token in JSON"),
            Self::InvalidRegExp => write!(formatter, "invalid regular expression"),
            Self::RevokedProxy => write!(
                formatter,
                "cannot perform this operation on a revoked proxy"
            ),
            Self::GeneratorAlreadyRunning => {
                write!(formatter, "generator is already running")
            }
            Self::InvalidArrayLength => write!(formatter, "invalid array length"),
            Self::InvalidCodePoint { value } => write!(formatter, "invalid code point {value}"),
            Self::TemporalFailure { message, .. } => write!(formatter, "{message}"),
            Self::IntlFailure { message, .. } => write!(formatter, "{message}"),
            Self::BufferTooLarge { bytes } => write!(
                formatter,
                "buffer of {bytes} bytes exceeds what this implementation will allocate"
            ),
            Self::BufferNotResizable => {
                write!(formatter, "cannot resize a non-resizable ArrayBuffer")
            }
            Self::BufferNotGrowable => {
                write!(formatter, "cannot grow a non-growable SharedArrayBuffer")
            }
            Self::BufferDetached => {
                write!(formatter, "the ArrayBuffer is detached")
            }
            Self::BufferImmutable => {
                write!(formatter, "the ArrayBuffer is immutable")
            }
            Self::BufferLengthOutOfRange { length } => {
                write!(formatter, "invalid ArrayBuffer length {length}")
            }
            Self::NotABuffer => {
                write!(formatter, "receiver is not an ArrayBuffer")
            }
            Self::NotADataView => {
                write!(formatter, "receiver is not a DataView")
            }
            Self::DataViewOutOfBounds => {
                write!(formatter, "DataView is out of bounds for its buffer")
            }
            Self::DataViewOutOfRange => {
                write!(formatter, "offset is outside the bounds of the DataView")
            }
            Self::SymbolIsNotANumber => {
                write!(formatter, "cannot convert a Symbol value to a number")
            }
            Self::SymbolIsNotAString => {
                write!(formatter, "cannot convert a Symbol value to a string")
            }
            Self::BigIntIsNotANumber => {
                write!(formatter, "cannot convert a BigInt value to a number")
            }
            Self::NotABigInt => write!(formatter, "cannot convert this value to a BigInt"),
            Self::InvalidBigIntString => {
                write!(formatter, "cannot convert this string to a BigInt")
            }
            Self::TypedArrayContentTypeMismatch => {
                write!(formatter, "cannot mix BigInt and non-BigInt typed arrays")
            }
            Self::NotAnIterator => write!(formatter, "receiver is not an iterator"),
            Self::IteratorResultNotAnObject => {
                write!(formatter, "iterator result is not an object")
            }
            Self::IteratorLimitOutOfRange => {
                write!(formatter, "argument is not a valid iterator limit")
            }
            Self::ArrayTooLarge { length } => write!(
                formatter,
                "array of length {length} exceeds this implementation's dense representation"
            ),
            Self::StringTooLong { bytes } => {
                write!(
                    formatter,
                    "string of {bytes} bytes exceeds the maximum length"
                )
            }
            Self::MalformedUri => write!(formatter, "URI malformed"),
            Self::FrozenArray => write!(formatter, "cannot change the length of a frozen array"),
            Self::NoSourceCompiler => write!(
                formatter,
                "this realm has no source compiler capability, so `eval` and `Function` cannot compile at run time"
            ),
            Self::NoModuleLoader => write!(
                formatter,
                "no module loader is installed, so `import()` cannot reach a module at run time"
            ),
            Self::DynamicImportUnsupported { message } => {
                write!(formatter, "unsupported dynamic import: {message}")
            }
            Self::SyntaxErrorMessage { message } => write!(formatter, "{message}"),
            Self::EvalUnsupported { message } => write!(formatter, "{message}"),
            Self::CryptoUnsupported { message } => write!(formatter, "{message}"),
            Self::ProxyNotSerialisable => {
                write!(formatter, "cannot serialise a proxy nested inside a value")
            }
            Self::CyclicJson => write!(formatter, "converting circular structure to JSON"),
            Self::BigIntNotSerialisable => {
                write!(formatter, "do not know how to serialize a BigInt")
            }
            Self::FunctionFromSource => write!(
                formatter,
                "building a function from source needs the compiler at run time"
            ),
            Self::NotPrimitive => {
                write!(formatter, "cannot convert object to primitive value")
            }
            Self::GeneratorReturned => write!(formatter, "generator returned"),
            Self::Uncaught {
                message,
                trace,
                stack,
                ..
            } => {
                // `(in promise)` is a prefix of the description, and the stack starts with the
                // bare error: keep the marker in front of whichever is printed.
                let marker = if message.starts_with("(in promise) ") {
                    "(in promise) "
                } else {
                    ""
                };
                match stack {
                    Some(stack) => write!(formatter, "uncaught {marker}{stack}"),
                    None => {
                        write!(formatter, "uncaught {message}")?;
                        for frame in trace {
                            write!(formatter, "\n    at {frame}")?;
                        }
                        Ok(())
                    }
                }
            }
            Self::CaughtByHost => write!(formatter, "exception delivered to the host"),
            Self::ThrownAcrossNative => {
                write!(formatter, "exception travelling out of a built-in callback")
            }
            Self::NotAnObject { instruction, value } => write!(
                formatter,
                "instruction {instruction} set a property on a {value}, which is not an object"
            ),
            Self::MissingUpvalue { index } => {
                write!(formatter, "no captured cell at upvalue index {index}")
            }
            Self::NotACell { slot } => {
                write!(formatter, "local slot {slot} does not hold a captured cell")
            }
            Self::UnknownFunction { function } => {
                write!(formatter, "no compiled function at index {function}")
            }
            Self::NotCallable { instruction, value } => write!(
                formatter,
                "instruction {instruction} tried to call a {value}, which is not a function"
            ),
            Self::CallDepthExceeded { limit } => {
                write!(formatter, "call depth exceeded {limit} frames")
            }
            Self::GlobalOutOfBounds {
                index,
                available,
                function,
            } => {
                write!(
                    formatter,
                    "module slot {index} is out of bounds for {available} slots"
                )?;
                if let Some(function) = function {
                    write!(formatter, " while running {function}")?;
                }
                Ok(())
            }
            Self::TypeErrorMessage { message }
            | Self::ErrorMessage { message }
            | Self::RangeErrorMessage { message }
            | Self::ReferenceErrorMessage { message } => {
                write!(formatter, "{message}")
            }
            Self::Host {
                instruction,
                message,
            } => {
                write!(
                    formatter,
                    "host operation failed at instruction {instruction}: {message}"
                )
            }
            Self::ModuleNotFound { message, .. } => write!(formatter, "{message}"),
        }
    }
}

impl ExecutionError {
    /// The JavaScript error this corresponds to, when it is one a program could catch.
    ///
    /// Everything else is a compiler defect or a resource limit -- a program cannot meaningfully
    /// catch "bytecode jumped out of range", and letting it try would turn a bug in kiln into an
    /// exception the program silently swallows. So the list is short on purpose.
    pub(super) fn as_javascript_error(&self) -> Option<(ErrorKind, String)> {
        Some(match self {
            Self::NotCallable { value, .. } => {
                (ErrorKind::Type, format!("a {value} is not a function"))
            }
            Self::NotConstructable { value, .. } => {
                (ErrorKind::Type, format!("a {value} is not a constructor"))
            }
            Self::NotAnObject { value, .. } => (
                ErrorKind::Type,
                format!("cannot set a property on a {value}"),
            ),
            Self::RevokedProxy => (
                ErrorKind::Type,
                "Cannot perform this operation on a revoked proxy".to_owned(),
            ),
            Self::InvalidArrayLength => (ErrorKind::Range, "Invalid array length".to_owned()),
            Self::InvalidCodePoint { value } => {
                (ErrorKind::Range, format!("Invalid code point {value}"))
            }
            // Catchable, and with the engine's own classification kept: a program that asks for
            // an impossible date is told `RangeError` because that is what the specification
            // says, not because this is the nearest kind to hand.
            Self::TemporalFailure { kind, message } => (*kind, message.clone()),
            Self::IntlFailure { kind, message } => (*kind, message.clone()),
            Self::TypeErrorMessage { message } => (ErrorKind::Type, message.clone()),
            Self::ErrorMessage { message } => (ErrorKind::Error, message.clone()),
            Self::RangeErrorMessage { message } => (ErrorKind::Range, message.clone()),
            Self::ReferenceErrorMessage { message } => (ErrorKind::Reference, message.clone()),
            Self::BufferNotResizable => (
                ErrorKind::Type,
                "Cannot resize a non-resizable ArrayBuffer".to_owned(),
            ),
            Self::BufferNotGrowable => (
                ErrorKind::Type,
                "Cannot grow a non-growable SharedArrayBuffer".to_owned(),
            ),
            Self::BufferImmutable => (
                ErrorKind::Type,
                "Cannot perform this operation on an immutable ArrayBuffer".to_owned(),
            ),
            Self::BufferDetached => (
                ErrorKind::Type,
                "Cannot perform this operation on a detached ArrayBuffer".to_owned(),
            ),
            Self::BufferLengthOutOfRange { .. } => {
                (ErrorKind::Range, "Invalid ArrayBuffer length".to_owned())
            }
            Self::NotABuffer => (
                ErrorKind::Type,
                "Method called on an incompatible receiver".to_owned(),
            ),
            Self::NotADataView => (
                ErrorKind::Type,
                "Method called on an incompatible receiver".to_owned(),
            ),
            Self::DataViewOutOfBounds => (
                ErrorKind::Type,
                "DataView is out of bounds for its buffer".to_owned(),
            ),
            Self::DataViewOutOfRange => (
                ErrorKind::Range,
                "Offset is outside the bounds of the DataView".to_owned(),
            ),
            Self::SymbolIsNotANumber => (
                ErrorKind::Type,
                "Cannot convert a Symbol value to a number".to_owned(),
            ),
            Self::SymbolIsNotAString => (
                ErrorKind::Type,
                "Cannot convert a Symbol value to a string".to_owned(),
            ),
            Self::BigIntIsNotANumber => (
                ErrorKind::Type,
                "Cannot convert a BigInt value to a number".to_owned(),
            ),
            Self::NotABigInt => (
                ErrorKind::Type,
                "Cannot convert this value to a BigInt".to_owned(),
            ),
            Self::InvalidBigIntString => (
                ErrorKind::Syntax,
                "Cannot convert this string to a BigInt".to_owned(),
            ),
            Self::TypedArrayContentTypeMismatch => (
                ErrorKind::Type,
                "Cannot mix BigInt and non-BigInt typed arrays".to_owned(),
            ),
            Self::NotAnIterator => (
                ErrorKind::Type,
                "Method called on an incompatible receiver".to_owned(),
            ),
            Self::IteratorResultNotAnObject => (
                ErrorKind::Type,
                "Iterator result is not an object".to_owned(),
            ),
            Self::IteratorLimitOutOfRange => {
                (ErrorKind::Range, "Invalid iterator limit".to_owned())
            }
            // Word for word what V8 says, because a test that catches this reads the message.
            Self::StringTooLong { .. } => (ErrorKind::Range, "Invalid string length".to_owned()),
            Self::GeneratorAlreadyRunning => {
                (ErrorKind::Type, "Generator is already running".to_owned())
            }
            // What a real engine raises when the stack runs out, and catchable for the same
            // reason: a program that recurses on user input has to be able to say so.
            Self::CallDepthExceeded { .. } => (
                ErrorKind::Range,
                "Maximum call stack size exceeded".to_owned(),
            ),
            // `URIError` is the whole reason that kind exists, and it is the one a program
            // catches: `decodeURI` on user input is a routine thing to guard, and the guard is
            // written as `catch (e) { if (e instanceof URIError) ... }`. Reported as a TypeError,
            // every one of those guards fell through to a rethrow.
            Self::MalformedUri => (ErrorKind::Uri, "URI malformed".to_owned()),
            Self::FrozenArray => (
                ErrorKind::Type,
                "Cannot add property, object is not extensible".to_owned(),
            ),
            Self::SyntaxErrorMessage { message } => (ErrorKind::Syntax, message.clone()),
            Self::ProxyNotSerialisable => (
                ErrorKind::Type,
                "Cannot serialise a proxy nested inside a value".to_owned(),
            ),
            Self::CyclicJson => (
                ErrorKind::Type,
                "Converting circular structure to JSON".to_owned(),
            ),
            Self::BigIntNotSerialisable => (
                ErrorKind::Type,
                "Do not know how to serialize a BigInt".to_owned(),
            ),
            Self::FunctionFromSource => (
                ErrorKind::Error,
                "Building a function from source is not supported in this realm".to_owned(),
            ),
            Self::NotPrimitive => (
                ErrorKind::Type,
                "Cannot convert object to primitive value".to_owned(),
            ),
            Self::InvalidJson => (ErrorKind::Syntax, "Unexpected token in JSON".to_owned()),
            Self::MixedBigInt => (
                ErrorKind::Type,
                "Cannot mix BigInt and other types, use explicit conversions".to_owned(),
            ),
            Self::RadixOutOfRange => (
                ErrorKind::Range,
                "toString() radix must be between 2 and 36".to_owned(),
            ),
            Self::InvalidTimeout => (
                ErrorKind::Range,
                "The value of \"msecs\" is out of range".to_owned(),
            ),
            Self::InvalidPort => (
                ErrorKind::Range,
                "Port should be >= 0 and < 65536".to_owned(),
            ),
            Self::InvalidHttpStatus => (ErrorKind::Range, "Invalid status code".to_owned()),
            Self::DateOutOfRange => (ErrorKind::Range, "Invalid time value".to_owned()),
            Self::SetSizeOutOfRange => (ErrorKind::Range, "Invalid set size".to_owned()),
            Self::ArrayIndexOutOfRange => (ErrorKind::Range, "Invalid index".to_owned()),
            Self::InvalidRegExp => (ErrorKind::Syntax, "Invalid regular expression".to_owned()),
            Self::EmptyReduce => (
                ErrorKind::Type,
                "Reduce of empty array with no initial value".to_owned(),
            ),
            _ => return None,
        })
    }
}

impl std::error::Error for ExecutionError {}
