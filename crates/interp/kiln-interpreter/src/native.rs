use super::*;
use crate::PrimitiveHint;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WebStubKind {
    ReadableStream,
    WritableStream,
    TransformStream,
    ByteLengthQueuingStrategy,
    CountQueuingStrategy,
    MessagePort,
    MessageChannel,
    BroadcastChannel,
    Event,
    EventTarget,
    CustomEvent,
}

impl WebStubKind {
    pub const ALL: &'static [(Self, &'static str)] = &[
        (Self::ReadableStream, "ReadableStream"),
        (Self::WritableStream, "WritableStream"),
        (Self::TransformStream, "TransformStream"),
        (Self::ByteLengthQueuingStrategy, "ByteLengthQueuingStrategy"),
        (Self::CountQueuingStrategy, "CountQueuingStrategy"),
        (Self::MessagePort, "MessagePort"),
        (Self::MessageChannel, "MessageChannel"),
        (Self::BroadcastChannel, "BroadcastChannel"),
        (Self::Event, "Event"),
        (Self::EventTarget, "EventTarget"),
        (Self::CustomEvent, "CustomEvent"),
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::ReadableStream => "ReadableStream",
            Self::WritableStream => "WritableStream",
            Self::TransformStream => "TransformStream",
            Self::ByteLengthQueuingStrategy => "ByteLengthQueuingStrategy",
            Self::CountQueuingStrategy => "CountQueuingStrategy",
            Self::MessagePort => "MessagePort",
            Self::MessageChannel => "MessageChannel",
            Self::BroadcastChannel => "BroadcastChannel",
            Self::Event => "Event",
            Self::EventTarget => "EventTarget",
            Self::CustomEvent => "CustomEvent",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FixedOwnValue {
    Native(Native),
    Symbol(WellKnown),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Native {
    /// A `Temporal` namespace, constructor, prototype or method; the payload says which.
    ///
    /// One variant with a payload rather than ninety flat ones: `Temporal`'s eight types answer
    /// almost the same set of methods, so the receiver decides what a method *means*.
    Temporal(temporal::Op),
    /// An `Intl` namespace, constructor, prototype or method; the payload says which.
    ///
    /// One variant with a payload for the reason `Temporal` has one: the six services answer
    /// to overlapping method names and the receiver decides what a name means.
    Intl(intl::Op),
    /// `Error.isError(value)`: whether a value really is an error rather than an object shaped
    /// like one. The distinction cannot be made from JavaScript, which is why it exists.
    ErrorIsError,
    /// Make a second realm and answer with its global object.
    ///
    /// Not a language built-in: it is reachable only from a realm whose host asked for it
    /// (`ExecutionCapabilities::with_host_realms`), which is how ADR 0031 says a realm takes
    /// what the host gives it. Test262's `$262.createRealm` is the only caller there is, and
    /// the `$262` object it hangs off is assembled by the runner rather than here, because what
    /// a host can do is the host's statement to make.
    HostCreateRealm,
    /// Evaluate source as a *Script* in this realm, the way Test262's `$262.evalScript` does.
    ///
    /// Not `eval`. An indirect `eval` gives strict code a variable environment of its own and
    /// makes every global binding it does create configurable; a Script does neither, and both
    /// differences are what the tests reaching for this are about. Reachable only from a realm
    /// whose host asked for it, for the same reason [`Native::HostCreateRealm`] is.
    HostEvalScript,
    /// One operation of Test262's agent protocol, published on the global object of a realm
    /// whose host asked for agents (`ExecutionCapabilities::with_host_agents`).
    ///
    /// Eight hooks rather than one object, for the same reason `$262CreateRealm` is a hook: the
    /// `$262.agent` a program sees is the runner's, assembled from what it found published. What
    /// the engine owes is the operations themselves.
    HostAgent(AgentOp),
    /// `Map.prototype.getOrInsert` and its three siblings. `weak` says which collection this is
    /// reached through and `computed` whether the value is a callback's result, because the two
    /// differ only in where the value comes from and when it is asked for.
    CollectionGetOrInsert {
        weak: bool,
        computed: bool,
    },
    /// The `Atomics` namespace, and one operation on it; the payload says which.
    ///
    /// A namespace rather than a constructor: there is nothing to make one of. Every operation
    /// takes the view it works on as its first argument, which is why they share one variant.
    Atomics,
    AtomicsOp(AtomicsOp),
    /// The `node:fs/promises` namespace and its first asynchronous methods.
    NodeFsPromises,
    NodeFs,
    NodeFsExistsSync,
    NodeFsReadFileSync,
    NodeFsMkdirSync,
    NodeFsWriteFileSync,
    NodeFsStatSync,
    NodeFsLstatSync,
    NodeFsFstatSync,
    NodeFsReaddirSync,
    NodeFsRmSync,
    NodeFsUnlinkSync,
    NodeFsRealpathSync,
    NodeFsRenameSync,
    NodeFsCopyFileSync,
    NodeFsAccessSync,
    NodeFsAppendFileSync,
    NodeFsChmodSync,
    NodeFsFchmodSync,
    NodeFsOpenSync,
    NodeFsCloseSync,
    NodeFsReadlinkSync,
    NodeFsRmdirSync,
    NodeFsUtimesSync,
    NodeFsFutimesSync,
    NodeFsWriteSync,
    NodeFsFtruncateSync,
    NodeFsFsyncSync,
    /// `AbortController`, and the `AbortSignal` namespace its statics hang off.
    AbortControllerConstructor,
    AbortSignalConstructor,
    /// `controller.abort(reason)`, and the two statics that build an already-settled signal.
    AbortControllerAbort,
    AbortSignalAbort,
    AbortSignalTimeout,
    AbortSignalAny,
    /// `signal.addEventListener` / `removeEventListener` / `throwIfAborted`.
    AbortSignalListen {
        remove: bool,
    },
    AbortSignalThrowIfAborted,
    /// Settle a particular signal later: the receiver is the signal, bound into a timer.
    AbortSignalSettle,
    /// Mirror another signal's abort onto this one, for `AbortSignal.any`.
    AbortSignalFollow,
    /// `new TextEncoder().encode(text)`.
    TextEncoderConstructor,
    TextEncoderEncode,
    /// Any of `node:fs`'s synchronous entry points. One variant because they all end the same
    /// way: this realm's filesystem is Tokio-owned and asynchronous, so none of them can answer,
    /// and which one was asked for adds nothing to that.
    NodeFsSync,
    NodeFsReadFile,
    NodeFsWriteFile,
    NodeFsAppendFile,
    NodeFsStat,
    NodeFsMkdir,
    NodeFsReaddir,
    NodeFsCopyFile,
    NodeFsRename,
    NodeFsRm,
    NodeFsUnlink,
    NodeFsLstat,
    NodeFsRealpath,
    NodeFsAccess,
    NodeFsChmod,
    NodeFsRmdir,
    NodeFsReadlink,
    NodeStatIsFile,
    NodeStatIsDirectory,
    NodeStatIsSymbolicLink,
    NodeEvents,
    NodeDiagnosticsChannel,
    NodeDiagnosticsChannelCreate,
    NodeDiagnosticsTracingChannel,
    NodeDiagnosticsPublish,
    NodeDiagnosticsRunStores,
    NodeUtil,
    NodeUtilInherits,
    NodeUtilDebuglog,
    NodeUtilDebugNoop,
    NodeUtilDeprecate,
    NodeUtilInspect,
    NodeUtilFormat,
    ConsoleConstructor,
    ConsoleLog,
    ConsoleInfo,
    ConsoleDebug,
    ConsoleDir,
    ConsoleWarn,
    ConsoleError,
    ConsoleAssert,
    ConsoleClear,
    ConsoleCount,
    ConsoleCountReset,
    ConsoleGroup,
    ConsoleGroupEnd,
    ConsoleGroupCollapsed,
    ConsoleTable,
    ConsoleTime,
    ConsoleTimeEnd,
    ConsoleTimeLog,
    ConsoleTrace,
    ConsoleDirxml,
    ConsoleTimeStamp,
    ConsoleProfile,
    ConsoleProfileEnd,
    NodeConsole,
    NodeAssert,
    NodeAssertOk,
    NodeAsyncHooks,
    NodeAsyncResource,
    NodeAsyncResourceBind,
    NodeAsyncResourceEmitDestroy,
    NodeAsyncLocalStorage,
    NodeAsyncLocalStorageRun,
    NodeAsyncLocalStorageGetStore,
    NodeAsyncLocalStorageEnterWith,
    NodeAsyncLocalStorageExit,
    NodeAsyncLocalStorageDisable,
    NodeCrypto,
    NodeCryptoRandomUuid,
    NodeCryptoRandomBytes,
    NodeCryptoRandomFillSync,
    NodeCryptoGetHashes,
    NodeCryptoGetRandomValues,
    NodeCryptoCreateHash,
    NodeCryptoCreateHmac,
    NodeCryptoTimingSafeEqual,
    NodeCryptoKeyImport,
    NodeCryptoKeyPublic,
    NodeCryptoKeyInfo,
    NodeCryptoKeyExport,
    NodeCryptoKeyFree,
    WebCryptoPrimitive,
    WebCompressionPrimitive,
    NodeHashUpdate,
    NodeHashDigest,
    NodeStringDecoder,
    NodeStringDecoderConstructor,
    NodeStringDecoderWrite,
    NodeStringDecoderEnd,
    NodeStream,
    NodeReadable,
    NodeReadablePush,
    NodeReadableRead,
    NodeReadableDestroy,
    NodeStreamAddAbortSignal,
    NodeWritable,
    NodeWritableWrite,
    NodeWritableEnd,
    NodeWritableDestroy,
    NodeStreamFinished,
    NodeDns,
    NodeDnsLookup,
    NodeOs,
    NodeOsNetworkInterfaces,
    NodeOsHostname,
    /// One fact about the machine this realm runs on; the payload says which.
    ///
    /// One variant because the interesting part is not the arithmetic, it is *where the answer
    /// comes from*, and that is the same question for all of them. See [`OsFact`].
    NodeOsFact(OsFact),
    NodeTty,
    NodeTtyIsatty,
    NodeZlib,
    NodeZlibCreateDecompress,
    NodeZlibCreateGunzip,
    NodeZlibCreateInflate,
    NodeZlibCreateGzip,
    NodeZlibCreateDeflate,
    NodeZlibCreateUnzip,
    NodeZlibCreateBrotliDecompress,
    NodeZlibGunzipSync,
    NodeZlibInflateSync,
    NodeZlibGzipSync,
    NodeZlibDeflateSync,
    NodeZlibUnzipSync,
    NodePerfHooks,
    NodeHttps,
    NodeHttpsCreateServer,
    NodeHttp2,
    NodeHttp2CreateServer,
    NodeHttp2CreateSecureServer,
    NodeModule,
    NodeModuleCreateRequire,
    /// Private runner-only access to the realm's module export cells for Vitest spy overlays.
    NodeModuleLoader,
    NodePath,
    NodePathJoin,
    NodePathResolve,
    NodePathIsAbsolute,
    /// One of the four pure decompositions of a path: `dirname`, `basename`, `extname` and
    /// `normalize`. One variant because they are one scan read four ways -- find the last
    /// separator and the last dot, and answer with a different slice.
    NodePathPart(PathPart),
    NodePathRelative,
    NodePathParse,
    NodePathFormat,
    NodeWorkerThreads,
    NodeWorkerConstructor,
    NodeQueryString,
    NodeQueryStringParse,
    NodeUrl,
    NodeUrlParse,
    NodeUrlFileUrlToPath,
    NodeUrlPathToFileUrl,
    TextDecoderConstructor,
    TextDecoderDecode,
    NodeEventEmitter,
    NodeEventOn,
    NodeEventOnce,
    NodeEventEmit,
    NodeEventListeners,
    NodeEventListenerCount,
    NodeEventNames,
    NodeEventRemoveListener,
    NodeEventRemoveAllListeners,
    NodeEventSetMaxListeners,
    NodeNet,
    NodeNetCreateServer,
    NodeNetConnect,
    NodeSocketWrite,
    NodeSocketEnd,
    NodeSocketDestroy,
    NodeSocketCork,
    NodeSocketUncork,
    NodeReadablePause,
    NodeReadableResume,
    NodeReadableSetEncoding,
    NodeReadablePipe,
    NodeReadableUnpipe,
    NodeHttp,
    NodeHttpServer,
    NodeHttpCreateServer,
    NodeHttpRequest,
    NodeHttpGet,
    /// The private host half of `serve()` (`__kilnNativeModules.http`).
    NodeHttpCreateFetchServer,
    NodeHttpFetchRespond,
    NodeHttpServerResponse,
    NodeClientRequestWrite,
    NodeClientRequestEnd,
    NodeClientRequestAssignSocket,
    NodeBuffer,
    /// The *global* `Buffer`, as distinct from the `node:buffer` namespace above.
    ///
    /// They publish the same members and differ in exactly one answer: node reports
    /// `typeof require("buffer")` as "object" and `typeof Buffer` as "function", because the
    /// first is a module namespace and the second is a constructor. One value for both made the
    /// global read as an object here while the native backend read it as a function -- the same
    /// bytecode, two answers -- and made this engine disagree with node about a name every Node
    /// bundle feature-detects on.
    NodeBufferGlobal,
    NodeBufferFrom,
    NodeBufferAlloc,
    NodeBufferIsBuffer,
    NodeBufferByteLength,
    NodeBufferConcat,
    NodeBufferToString,
    /// `process.hrtime`, whose callable and `.bigint()` forms both read the realm clock.
    ProcessHrtime,
    ProcessHrtimeBigInt,
    ProcessCwd,
    ProcessKill,
    ProcessExit,
    ProcessStartRaw,
    ProcessCancelRaw,
    /// Private realm status access used by the shared TypeScript `process.exitCode` policy.
    ProcessExitCodeGet,
    ProcessExitCodeSet,
    /// The interpreter releases unreachable acyclic values through Rc; this request is a
    /// lifecycle boundary hook, not a claim to trace or collect cycles.
    GcRequest,
    /// The four bridges the shared TypeScript `URL` captures (`kiln-builtins/src/url.ts`).
    UrlParseRaw,
    UrlSetRaw,
    UrlFastRaw,
    UrlAdoptRaw,
    ProcessStdinSetRawMode,
    UrlConstructor,
    UrlSearchParamsGet,
    UrlSearchParamsHas,
    UrlSearchParamsGetAll,
    UrlSearchParamsToString,
    /// `URL.prototype.toString` and `toJSON`, which are one method under two names.
    UrlToString,
    NodeServerListen,
    NodeServerClose,
    NodeServerAddress,
    NodeServerIsListening,
    NodeServerSetTimeout,
    NodeResponseSetHeader,
    NodeResponseGetHeader,
    NodeResponseGetHeaders,
    NodeResponseHasHeader,
    NodeResponseRemoveHeader,
    NodeResponseWriteHead,
    NodeResponseWrite,
    NodeResponseEnd,
    ProcessGetBuiltinModule,
    Error(ErrorKind),
    SymbolConstructor,
    ArrayConstructor,
    ObjectConstructor,
    Json,
    ArrayIsArray,
    ArrayFrom,
    /// `Array.fromAsync`: `Array.from` with an await at every element.
    ObjectKeys,
    ObjectSetPrototypeOf,
    ObjectGetPrototypeOf,
    StringConstructor,
    JsonParse,
    JsonStringify,
    /// The `Math` namespace, and the functions reached through it.
    ///
    /// `Math.random` is here and is *deterministic*: a fixed seed and a fixed generator, the same
    /// in both engines, for the same reason the clock is virtual. See ADR 0026.
    Math,
    MathRandom,
    MathSinh,
    MathCosh,
    MathTanh,
    MathAsinh,
    MathAcosh,
    MathAtanh,
    MathLog1p,
    MathExpm1,
    MathFround,
    MathF16Round,
    ArrayBufferTransferToImmutable,
    ArrayBufferSliceToImmutable,
    /// One of the six `Uint8Array` conversions between bytes and text.
    Uint8Text(ByteText),
    MathClz32,
    MathImul,
    MathAbs,
    MathFloor,
    MathHypot,
    MathAtan2,
    MathSin,
    MathCos,
    MathTan,
    MathAsin,
    MathAcos,
    MathAtan,
    MathCeil,
    MathRound,
    MathTrunc,
    MathSign,
    MathSqrt,
    MathCbrt,
    MathPow,
    MathMin,
    MathMax,
    MathLog,
    MathLog2,
    MathLog10,
    MathExp,
    NumberConstructor,
    NumberIsInteger,
    NumberIsSafeInteger,
    /// `Number.isNaN`, which does not coerce -- unlike the global `isNaN`, which does.
    NumberIsNaN,
    NumberIsFinite,
    /// `Number.prototype.toString`, `toFixed` and `valueOf`.
    NumberToString,
    NumberToFixed,
    NumberValueOf,
    StringValueOf,
    SymbolValueOf,
    BigIntValueOf,
    StringAt,
    StringCharAt,
    StringCharCodeAt,
    StringPadStart,
    StringPadEnd,
    StringReplace,
    StringReplaceAll,
    StringLastIndexOf,
    StringTrimStart,
    StringTrimEnd,
    StringConcat,
    ArrayAt,
    ArrayConcat,
    ArrayReverse,
    ArrayShift,
    ArrayUnshift,
    ArrayLastIndexOf,
    ArrayFill,
    ArraySplice,
    ArrayCopyWithin,
    /// `toString`, which is `join` with the separator fixed rather than defaulted -- a program can
    /// pass one to `join` and cannot to `toString`.
    ArrayJoinDefault,
    SetTimeout,
    SetInterval,
    SetImmediate,
    /// `clearTimeout` and `clearInterval` are one operation: both remove a scheduled callback by
    /// the id it was given, and the language does not distinguish which kind it was.
    /// `clearTimeout` and `clearInterval` do the same thing and are two variants because they
    /// are two functions: a program can print either name, and `f === g` must be false for them.
    ClearTimeout,
    ClearInterval,
    ClearImmediate,
    QueueMicrotask,
    ProcessNextTick,
    DateConstructor,
    DateNow,
    DateUtc,
    DateParse,
    /// One method on `Date.prototype`, named by the payload. Every one of them is a separate
    /// function object in the specification, so every one of them is a separate marker here.
    Date(DateMethod),
    /// The getter half of one accessor on `RegExp.prototype`. There is no setter half: the
    /// specification defines none, which is what makes `re.global = true` silently do nothing.
    RegExpAccessor(RegExpAccessor),
    /// One of the seven set operations. They share a marker because they share a shape: each
    /// takes one *set-like* argument and answers a new `Set` or a boolean.
    SetOperation(SetOperation),
    /// The four copying counterparts of the mutating array methods: `with`, `toReversed`,
    /// `toSorted` and `toSpliced`. Each answers a new array and leaves the receiver alone, which
    /// is the whole reason the language grew them.
    ArrayCopy(ArrayCopy),
    /// One of Annex B's HTML wrappers on `String.prototype`.
    /// `Number.prototype.toExponential` and `toPrecision`, which differ only in what the digit
    /// count means.
    NumberToExponential,
    NumberToPrecision,
    /// `BigInt.asIntN` and `asUintN`: the same truncation, read as signed or unsigned.
    BigIntAsIntN,
    BigIntAsUintN,
    /// The two halves of `Object.prototype.__proto__`, which is an accessor rather than a
    /// property -- Annex B's spelling of `Object.getPrototypeOf` and `setPrototypeOf`.
    ProtoGetter,
    ProtoSetter,
    /// One of the accessors a built-in prototype publishes about its instances.
    ///
    /// Each of these used to be a `match` arm on the *instance*, which answered a read and
    /// nothing else: the prototype had no such property to describe, to list or to take apart,
    /// `Object.getOwnPropertyDescriptor(ArrayBuffer.prototype, "byteLength")` was `undefined`,
    /// and a collection's `size` turned up as a bogus own property of every instance. As
    /// accessors they are what the specification says they are -- one own accessor on the
    /// prototype, non-enumerable and configurable, refusing a receiver of the wrong kind.
    BuiltinAccessor(BuiltinAccessor),
    /// `%ThrowTypeError%`: the realm's one anonymous function whose whole behaviour is to throw.
    ///
    /// It is both halves of `Function.prototype.caller` and of `Function.prototype.arguments`,
    /// and both halves of `callee` on a strict function's arguments object. One object fills all
    /// six slots because the specification says so and because a program can *see* it: Test262
    /// compares the getter it takes off `arguments` with the one it takes off `Function.prototype`
    /// and requires them to be the same value.
    ThrowTypeError,
    /// `Object.groupBy` and `Map.groupBy`: one walk, two shapes of result.
    ObjectGroupBy,
    MapGroupBy,
    /// `Promise.withResolvers`.
    PromiseWithResolvers,
    /// One of the five symbol-keyed methods on `RegExp.prototype`. These are what
    /// `String.prototype.match` and its four siblings are *defined* to call, which is what lets a
    /// program hand any of them an object of its own that answers the same symbol.
    RegExpSymbol(RegExpSymbol),
    /// `Iterator.from`, and the `next` of the wrapper it may have to build.
    IteratorFrom,
    IteratorWrapNext,
    WeakMapConstructor,
    WeakSetConstructor,
    /// `JSON.rawJSON(text)` and `JSON.isRawJSON(value)`.
    ///
    /// A raw JSON object is a hole punched in the serialiser: it carries JSON *text* that
    /// `stringify` emits verbatim instead of re-encoding. Its whole reason to exist is the value
    /// JavaScript cannot hold -- a big integer, a decimal with more digits than a double keeps --
    /// which a program can now put into a document without the round trip through `Number`
    /// silently rewriting it.
    JsonRawJson,
    JsonIsRawJson,
    /// `WeakRef`, and the one method it has.
    ///
    /// This realm never collects, so a reference held here is never cleared and `deref` always
    /// answers the target. That is not a stub: the specification requires only that a target be
    /// *kept* until it is unreachable, and leaves when -- or whether -- to clear entirely to the
    /// implementation. An engine that never clears is a conforming one, and a program cannot
    /// tell it apart from one that has simply not collected yet.
    WeakRefConstructor,
    WeakRefDeref,
    /// `ShadowRealm`, and the two methods that are its entire surface.
    ///
    /// A `ShadowRealm` is a realm a program can evaluate in but cannot otherwise reach: no object
    /// crosses the boundary in either direction. What makes that true is not the constructor --
    /// which only allocates a realm the machine already knows how to make -- but
    /// [`Value::Wrapped`], the membrane every callable is handed through. The constructor is the
    /// easy half; the wrapping is the specification's entire point.
    ShadowRealmConstructor,
    ShadowRealmEvaluate,
    ShadowRealmImportValue,
    /// `DisposableStack` and everything it can do.
    ///
    /// A stack is an ordinary object carrying two reserved slots: the disposers it has been
    /// given, and whether it has run them. That is the whole of its state, and the brand check
    /// every method starts with is "does this object have the first slot" -- the same shape
    /// `WeakRef` and a collection use, and for the same reason.
    DisposableStackConstructor,
    DisposableStackUse,
    DisposableStackAdopt,
    DisposableStackDefer,
    DisposableStackMove,
    /// `dispose()` and `[Symbol.dispose]`, which are one function: the specification says the
    /// two properties hold the same object, and a program compares them.
    DisposableStackDispose,
    /// `AsyncDisposableStack`: the same object driven by promises.
    ///
    /// It carries the same pair of slots under names of its own, so the brand check tells the
    /// two apart -- `DisposableStack.prototype.use.call(asyncStack)` has to be refused, and the
    /// slot it looks for is the whole of that refusal. `use` prefers `[Symbol.asyncDispose]`
    /// and falls back to `[Symbol.dispose]`, and every disposer's result is awaited, which is
    /// what makes `disposeAsync` a state machine where `dispose` is a loop.
    AsyncDisposableStackConstructor,
    AsyncDisposableStackUse,
    AsyncDisposableStackAdopt,
    AsyncDisposableStackDefer,
    AsyncDisposableStackMove,
    /// `disposeAsync()` and `[Symbol.asyncDispose]`, one function for the reason above.
    AsyncDisposableStackDisposeAsync,
    /// `FinalizationRegistry` and its two methods, for the same reason: nothing is ever
    /// collected, so no cleanup callback ever becomes due. What the registry still has to get
    /// right is everything a program can observe synchronously -- which targets it accepts, what
    /// `unregister` answers, and that `register` returns nothing.
    FinalizationRegistryConstructor,
    FinalizationRegistryRegister,
    FinalizationRegistryUnregister,
    /// `performance`, whose `now` reads the same virtual clock `Date.now` does.
    Performance,
    PerformanceNow,
    SymbolFor,
    SymbolKeyFor,
    ObjectAssign,
    ObjectEntries,
    ObjectValues,
    ObjectFreeze,
    ObjectIsFrozen,
    ObjectCreate,
    ObjectDefineProperty,
    /// `Reflect.defineProperty`: the same operation as [`Self::ObjectDefineProperty`], reporting
    /// a refusal as `false` where `Object.defineProperty` raises it.
    ReflectDefineProperty,
    ObjectFromEntries,
    ArrayOf,
    StringFromCharCode,
    StringFromCodePoint,
    Reflect,
    ReflectOwnKeys,
    ReflectGet,
    ReflectSet,
    ReflectApply,
    /// The seven `Reflect` operations that are *not* their `Object` namesakes.
    ///
    /// Each was aliased to the `Object` one, and three of the aliases were wrong answers rather
    /// than shared code: `Reflect.has` is `in` where `Object.hasOwn` is not, `Reflect.
    /// setPrototypeOf` answers a boolean where `Object.setPrototypeOf` answers the object, and
    /// `Reflect.preventExtensions` the same. The other four differ in what they do with a
    /// non-object: `Reflect` refuses one, where `Object` coerces it.
    ReflectConstruct,
    ReflectHas,
    ReflectGetPrototypeOf,
    ReflectSetPrototypeOf,
    ReflectGetOwnPropertyDescriptor,
    ReflectDeleteProperty,
    ReflectPreventExtensions,
    ObjectHasOwn,
    BigIntConstructor,
    BigIntToString,
    ObjectIs,
    ObjectHasOwnProperty,
    ObjectIsPrototypeOf,
    ObjectPropertyIsEnumerable,
    ObjectToStringTag,
    /// `Object.prototype.toLocaleString`, which is `Invoke(ToObject(this), "toString")`.
    ///
    /// Distinct from `ObjectToStringTag`, which the two shared: this one *calls* whatever
    /// `toString` the receiver's chain answers with, so a class that overrides `toString` is the
    /// one that runs. Sharing the variant meant `Object.prototype.toLocaleString` ignored the
    /// override, answered `[object Object]`, and reported its own name as `toString`.
    ObjectToLocaleString,
    ObjectValueOf,
    /// `ToString` of the receiver, which is what a primitive's `toString` returns.
    PrimitiveToString,
    /// The receiver itself, once a wrapper has been opened: `String.prototype.valueOf`,
    /// `Symbol.prototype.valueOf`, `BigInt.prototype.valueOf`.
    ///
    /// Distinct from `ObjectValueOf`, which is `Object.prototype.valueOf` and must hand back the
    /// *box*: `Object.prototype.valueOf.call(new String("x"))` is the wrapper and
    /// `String.prototype.valueOf.call(new String("x"))` is "x".
    PrimitiveValueOf,
    /// `Object.prototype`, `Array.prototype` and the rest, as method tables.
    ///
    /// Not real prototype objects: the realm has no chain to hang built-ins on yet (design.md
    /// §5). A marker is enough for what programs actually do with them, which is reach for one
    /// method and `call` it onto a receiver of their own.
    PrototypeOf(PrototypeTable),
    StringRaw,
    RegExpToString,
    Escape,
    Unescape,
    EncodeUri,
    EncodeUriComponent,
    DecodeUri,
    DecodeUriComponent,
    StringSubstring,
    StringSubstr,
    /// `clear`, `forEach` and the three iteration methods, each carrying whether it is the Map
    /// one or the Set one.
    ///
    /// The behaviour is shared -- both walk the same entry vector -- but the *identity* is not:
    /// `Map.prototype.forEach` and `Set.prototype.forEach` are two functions, and the difference
    /// is observable as the brand check each performs. Sharing one variant made
    /// `Map.prototype.clear.call(new Set())` succeed where it must throw.
    CollectionKeys(bool),
    CollectionForEach(bool),
    CollectionEntries(bool),
    /// `Function`, which programs reach for through `Function.prototype`.
    FunctionConstructor,
    /// `AsyncFunction`, `GeneratorFunction` and `AsyncGeneratorFunction` constructors and
    /// prototypes.
    AsyncFunctionConstructor,
    AsyncFunctionPrototype,
    GeneratorFunctionConstructor,
    GeneratorFunctionPrototype,
    AsyncGeneratorFunctionConstructor,
    AsyncGeneratorFunctionPrototype,
    /// `eval` as a *value*: the global function, reached by any route but a direct call.
    ///
    /// A direct `eval(...)` never becomes this. It compiles to [`Instruction::DirectEval`],
    /// because it evaluates in the caller's scope and no value can carry a scope. Everything
    /// else -- `(0, eval)(s)`, `globalThis.eval(s)`, a saved reference -- is an indirect eval
    /// and runs in global scope, which is exactly what a plain callable can do.
    Eval,
    /// `Proxy`, which is only ever reached through `new`.
    ProxyConstructor,
    /// `ArrayBuffer`, and one constructor per view kind.
    ArrayBufferConstructor,
    /// `SharedArrayBuffer`.
    ///
    /// A separate constructor rather than a flag on the one above, because a program tells the
    /// two apart by identity: `sab instanceof ArrayBuffer` is false and the prototypes are
    /// different objects. One constructor answering both would make those agree when the
    /// language says they disagree.
    SharedArrayBufferConstructor,
    /// `ArrayBuffer.prototype.slice`. `SharedArrayBuffer.prototype.slice`, `resize` and `grow`
    /// are the shared bootstrap's (`kiln-builtins/src/arraybuffer.ts`).
    ArrayBufferSlice,
    /// `__kilnArrayBufferDetach(buffer, length, preserveResizability)`: move the bytes into a new
    /// buffer and detach this one -- the primitive under `transfer`, `transferToFixedLength` and
    /// every transfer list, which are the shared bootstrap's.
    ArrayBufferDetachRaw,
    /// `__kilnArrayBufferByteLength(buffer)`: the internal-slot reader for the shared TypeScript
    /// `ArrayBuffer.prototype.byteLength` accessor.
    ArrayBufferByteLengthRaw,
    /// `__kilnArrayBufferResize(buffer, length)`: move a resizable buffer's length, or grow a
    /// growable shared one, in place (ADR 0065, representation). `resize` and `grow` are the
    /// shared TypeScript over it (`kiln-builtins/src/arraybuffer.ts`), which makes every check a
    /// program can observe first.
    ArrayBufferResizeRaw,
    /// `__kilnTypedArrayOutOfBounds(view)`: `IsTypedArrayOutOfBounds` (ADR 0065, representation)
    /// -- detached, or a resizable buffer shrunk under the window. The shared `%TypedArray%`
    /// methods ask it in `ValidateTypedArray` (`kiln-builtins/src/typedarray.ts`).
    TypedArrayOutOfBoundsRaw,
    /// `__kilnCollectionSize(collection, isMap)`: internal Map/Set storage read used by the shared
    /// TypeScript accessor so its descriptor and brand behavior have one owner.
    CollectionSizeRaw,
    /// `__kilnTypedArrayCopy(target, targetIndex, source, sourceIndex, count)`: a same-type element
    /// copy as bytes (ADR 0065, representation), `false` when the TypeScript's loop must run.
    TypedArrayCopyRaw,
    /// `__kilnTypedArraySort(view)`: the default-order sort in place (ADR 0065, representation),
    /// over the order `kiln_number_core::typed_order` owns for both engines.
    TypedArraySortRaw,
    /// `__kilnDataViewState(view)`: 0 for a non-view, 1 for a mutable view, 2 for an immutable
    /// view. A side-effect-free internal-slot read, checked before any argument coercion.
    DataViewStateRaw,
    /// Side-effect-free ECMAScript IsConstructor predicate for shared built-ins.
    IsConstructorRaw,
    /// `__kilnCloneData(value)`: the structured clone's plain-data fast path, which this engine
    /// never takes (it answers `undefined`, "not handled").
    CloneDataRaw,
    /// Engine-backed immutable graph snapshot, installed only through `kiln:shared`.
    ShareRaw,
    /// Test whether a value is a shared graph facade without inspecting user properties.
    SharedIsRaw,
    /// `__kilnJobsPending()`: whether a microtask checkpoint would run anything now, and whether a
    /// rejection nothing handled is waiting to be reported.
    JobsPendingRaw,
    /// `__kilnQueueIo(callback, argument)`: `callback(argument)` as an I/O callback, node's poll
    /// phase (`kiln_runtime::Realm::queue_io`), which is where a message port delivers.
    QueueIoRaw,
    /// `__kilnTimerRef(handle, op)`: what the methods of a timer handle ask of the engine, which
    /// made the handle and holds its identity (`op` 0 the identity, 1 `hasRef`, 2 `ref`, 3
    /// `unref`, 4 `refresh`). Whether a timer keeps the loop alive is a flag the realm's timer
    /// queue owns (`kiln_runtime::Realm::set_timer_referenced`), and a refresh restarts the
    /// queue's own entry (`refresh_timer`). Captured and deleted by
    /// `kiln-builtins/src/timers.ts`.
    TimerRefRaw,
    /// `__kilnWorkerOp(op, ...)`: the runtime's cross-realm ports and workers
    /// (`kiln_runtime::workers`), for `kiln-builtins/src/web_events_streams.ts`.
    WorkerOpRaw,
    /// `__kilnPortWait(endpoint, referenced)`: a host operation done once a cross-realm port has
    /// something for this realm.
    PortWaitRaw,
    /// `__kilnPromiseRejections(process)`: registers the shared unhandled-rejection tracking
    /// (`kiln-builtins/src/rejections.ts`), which the end of every microtask checkpoint calls.
    PromiseRejectionsRaw,
    ArrayBufferIsView,
    /// `%TypedArray%`: the abstract constructor the nine view constructors inherit from.
    ///
    /// Not reachable by name -- the specification gives it no global binding -- but reachable as
    /// `Object.getPrototypeOf(Int8Array)`, which is how every program that wants to reach the
    /// shared methods gets at them, and how Test262's own typed-array harness does it.
    TypedArrayIntrinsic,
    TypedArrayConstructor(TypedKind),
    TypedArrayOf(TypedKind),
    TypedArrayFrom(TypedKind),
    /// `DataView`, and the accessors that read and write one element through it.
    ///
    /// The element type rides in the variant rather than on the receiver because a `DataView`
    /// has none of its own: `getInt8` and `getFloat64` are two functions over the same view.
    DataViewConstructor,
    DataViewGet(TypedKind),
    DataViewSet(TypedKind),
    /// The 64-bit integer accessors, which stand apart from the eight above because their value
    /// does: a `BigInt`, not a number, and the reason they exist at all.
    DataViewGetBig {
        signed: bool,
    },
    /// `setBigInt64` and `setBigUint64`. Both write the low 64 bits of whatever they are given
    /// and the signedness only ever shows on the way back out, so it does not change what the
    /// method *does* -- but a program can see which one it is holding: they are two function
    /// objects with two names, and one variant for both gave them one name and one identity.
    DataViewSetBig {
        signed: bool,
    },
    /// `Iterator`: abstract, like `%TypedArray%`, and reachable by name so that a program can
    /// hang methods on its prototype and ask whether something is one.
    IteratorConstructor,
    /// One of the lazy helpers. Calling it builds an iterator; it walks nothing.
    IteratorLazy(HelperKind),
    /// One of the methods that consumes an iterator and answers a value.
    IteratorTerminal(TerminalKind),
    /// `next` on the iterator `RegExp.prototype[@@matchAll]` produced.
    RegExpStringIteratorNext,
    /// `next` and `return` on a helper this realm made.
    IteratorHelperNext(HelperKind),
    IteratorHelperReturn,
    ProxyRevocable,
    /// The `revoke` half of a revocable pair. Its receiver is the proxy it turns off.
    ProxyRevoke,
    /// `Function.prototype.toString`.
    FunctionToString,
    FunctionCall,
    FunctionApply,
    FunctionBind,
    ObjectPreventExtensions,
    ObjectIsExtensible,
    ObjectGetOwnPropertySymbols,
    ObjectDefineProperties,
    ObjectGetOwnPropertyDescriptor,
    ObjectGetOwnPropertyDescriptors,
    ArrayFlat,
    ArrayEntries,
    ArrayKeys,
    StringCodePointAt,
    StringLocaleCompare,
    StringNormalize,
    RegExpConstructor,
    RegExpTest,
    RegExpExec,
    RegExpLegacyCaptureRaw,
    StringMatch,
    StringMatchAll,
    StringSearch,
    PromiseConstructor,
    PromiseResolve,
    PromiseReject,
    /// `Function.prototype[Symbol.hasInstance]`, which is `OrdinaryHasInstance` published as a
    /// method. It is the one built-in the specification makes neither writable nor configurable,
    /// because it is what decides `instanceof` for every ordinary function in the realm.
    FunctionHasInstance,
    /// `Symbol.prototype[Symbol.toPrimitive]`: `thisSymbolValue`, and the reason a symbol refuses
    /// to be coerced. It exists as its own variant rather than reusing `PrimitiveValueOf` because
    /// a program can read it, and what it reads must be named `[Symbol.toPrimitive]`.
    SymbolToPrimitive,
    /// `%TypedArray%.prototype[Symbol.toStringTag]`'s getter. It answers the concrete view's name
    /// -- `"Uint8Array"` -- and `undefined` for anything that is not a view, which is what makes
    /// it usable as a brand check: real serializers capture this getter once and call it on every
    /// value they meet.
    TypedArrayToStringTag,
    /// `String.prototype.toString`. Its behaviour is `valueOf`'s exactly -- both hand back the
    /// string -- but they are two function objects with two names, and publishing one variant for
    /// both made `String.prototype.toString.name` answer `"valueOf"` and the two compare equal.
    StringToStringMethod,
    PromiseAll,
    PromiseAllSettled,
    /// `Promise.allKeyed` and `Promise.allSettledKeyed`: the same two combinators, walking an
    /// object's own enumerable keys instead of an iterable, and settling with an object keyed the
    /// same way rather than an array indexed by position.
    PromiseRace,
    PromiseAny,
    PromiseThen,
    PromiseCatch,
    PromiseFinally,
    BooleanConstructor,
    /// `Boolean.prototype.toString` and `Boolean.prototype.valueOf`.
    ///
    /// Their own variants rather than the shared `PrimitiveToString`/`ObjectValueOf`, because
    /// what makes them these functions rather than those is the brand check: neither is generic,
    /// and `Boolean.prototype.valueOf.call({})` is a `TypeError` the specification requires.
    BooleanToString,
    BooleanValueOf,
    ParseInt,
    ParseFloat,
    IsNaN,
    IsFinite,
    /// `Array.prototype.join`.
    ArrayJoin,
    /// `next` on an array iterator.
    ArrayIteratorNext,
    /// `String.prototype[Symbol.iterator]`, which walks code points rather than UTF-16 units.
    StringValues,
    StringIteratorNext,
    StringToUpperCase,
    StringToLowerCase,
    StringIncludes,
    StringIndexOf,
    StringSlice,
    StringSplit,
    StringTrim,
    StringStartsWith,
    StringEndsWith,
    StringRepeat,
    ArrayPush,
    ArrayPop,
    ArrayIndexOf,
    /// `Array.prototype.toLocaleString`, which is `join` with each element reduced by *calling*
    /// its own `toLocaleString` rather than its `toString`. On the machine because of that call:
    /// the corpus counts it, and an array of objects renders through it.
    ArrayToLocaleString,
    ArrayIncludes,
    ArraySlice,
    SetConstructor,
    MapConstructor,
    SetAdd,
    SetHas,
    SetDelete,
    CollectionClear(bool),
    ObjectOwnNames,
    /// The weak collections' methods. They do exactly what their strong counterparts do on
    /// exactly the same layout; they are separate only so that the brand check has something to
    /// check, which is the whole of the difference the specification draws between
    /// `WeakSet.prototype.add` and `Set.prototype.add`.
    WeakSetAdd,
    WeakSetHas,
    WeakSetDelete,
    WeakMapSet,
    WeakMapGet,
    WeakMapHas,
    WeakMapDelete,
    MapSet,
    MapGet,
    MapHas,
    MapDelete,
    CollectionValues(bool),
    CollectionIteratorNext,
    /// An absent element of an array: the hole in `[1, , 3]`.
    ///
    /// Not `undefined`, because the two are distinguishable: `1 in [1, , 3]` is false while
    /// `1 in [1, undefined, 3]` is true, and every method that walks elements skips a hole and
    /// visits an undefined. Reading one still *yields* undefined, which is why the marker never
    /// escapes into an expression.
    Hole,
    /// What a `let` or `const` slot holds before its declaration runs.
    ///
    /// A marker rather than `undefined`, because the two are different answers: reading a binding
    /// in its dead zone is an error, and reading one initialised to `undefined` is not. The
    /// compiler catches a *direct* read at compile time; this catches the read a nested function
    /// makes when it is called too early, which no compile-time check can see.
    Uninitialized,
    /// The value thrown into a generator by `.return()`.
    ///
    /// A marker rather than a flag because the unwinding it needs is exactly the unwinding an
    /// exception needs -- through every `finally` on the way out -- and reusing the exception
    /// path means there is one implementation of that rather than two. No expression produces
    /// it, so a program cannot catch it by accident.
    ReturnSignal,
    /// A deleted slot in a Set or Map.
    ///
    /// Deletion leaves a hole rather than compacting, because compacting a dense array under a
    /// live iterator shifts the elements past the cursor and skips one. That is the exact bug
    /// g13 exists to catch. Never reachable from JavaScript: no expression produces it.
    Tombstone,
    /// Returns its own receiver, so an iterator is iterable.
    IteratorSelf,
    /// `Array.prototype.values`, which is also `Array.prototype[Symbol.iterator]` -- the same
    /// function object, which is what `[][Symbol.iterator] === [].values` asserts.
    ArrayValues,
    /// Private activation-snapshot primitive used by the TypeScript CallSite facade.
    ErrorCaptureStackTrace,
    /// `Error.prototype.toString`, which renders `name: message`.
    ErrorToString,
    /// Standard Web/Node globals provided as stubs or baseline classes.
    WebStub(WebStubKind),
}

impl Native {
    /// Whether this built-in *changes* the array it is given, rather than only reading it.
    ///
    /// Redirected onto an array-like, such a method has nowhere to write; onto a proxy it does,
    /// through the `set` trap, and the write is the whole point of calling it.
    pub(crate) fn mutates_an_array(self) -> bool {
        matches!(
            self,
            Self::ArrayPush
                | Self::ArrayPop
                | Self::ArrayShift
                | Self::ArrayUnshift
                | Self::ArrayReverse
                | Self::ArrayFill
        )
    }

    /// Whether this built-in changes how many elements its array has.
    ///
    /// A typed array's length is fixed, so these are the ones it does not have. The other
    /// mutators -- `reverse`, `fill` -- keep the length and are fine on one.
    pub(crate) fn changes_array_length(self) -> bool {
        if matches!(self, Self::ArrayCopy(ArrayCopy::ToSpliced)) {
            return true;
        }
        matches!(
            self,
            Self::ArrayPush
                | Self::ArrayPop
                | Self::ArrayShift
                | Self::ArrayUnshift
                | Self::ArraySplice
        )
    }

    /// Whether this built-in reduces its arguments to primitives first, and with which hint.
    ///
    /// `None` means the built-in takes its arguments as they arrive. The hint has to be carried
    /// rather than collapsed into a flag: for an object defining both methods, `Math.abs` reaches
    /// `valueOf` and `parseInt` reaches `toString`, so the two disagree on the same value -- and
    /// `new Date(x)` asks for neither, which is a third answer rather than a shade of one of
    /// these two, and is why the count is a parameter here.
    ///
    /// Only the built-ins whose pure implementations cannot do this themselves are listed.
    /// `String`, the global `isNaN` and the global `isFinite` already agree with the
    /// specification by another route, and naming them here would reduce twice -- which a
    /// `valueOf` that counts its calls can see.
    pub(crate) fn primitive_argument_hint(self, argument_count: usize) -> Option<PrimitiveHint> {
        match self {
            // `Math.random` takes no argument, so it is deliberately not in this list.
            Self::MathAbs
            | Self::MathFloor
            | Self::MathCeil
            | Self::MathRound
            | Self::MathTrunc
            | Self::MathSign
            | Self::MathSqrt
            | Self::MathCbrt
            | Self::MathPow
            | Self::MathMin
            | Self::MathMax
            | Self::MathLog
            | Self::MathLog2
            | Self::MathLog10
            | Self::MathExp
            | Self::MathHypot
            | Self::MathAtan2
            | Self::MathSin
            | Self::MathCos
            | Self::MathTan
            | Self::MathAsin
            | Self::MathAcos
            | Self::MathAtan
            | Self::MathSinh
            | Self::MathCosh
            | Self::MathTanh
            | Self::MathAsinh
            | Self::MathAcosh
            | Self::MathAtanh
            | Self::MathLog1p
            | Self::MathExpm1
            | Self::MathFround
            | Self::MathF16Round
            | Self::MathClz32
            | Self::MathImul
            | Self::NumberConstructor
            // The *global* pair, which coerces. `Number.isNaN` and `Number.isFinite` ask whether
            // the value already is that number and are a different built-in; they must not be
            // added here, or `Number.isNaN({valueOf: () => NaN})` would answer true.
            | Self::IsNaN
            | Self::IsFinite
            // The field forms of both take seven numbers, and the *order* the conversions
            // happen in is observable through a `valueOf`: the specification converts year
            // through millisecond left to right, before it decides anything about the date.
            | Self::DateUtc
            // Both take code *units*, and both read them with the pure `as_number`, which
            // answers NaN for an object: `String.fromCharCode({valueOf: () => 65})` was NUL
            // rather than "A", and `fromCodePoint` refused the same argument outright.
            | Self::StringFromCharCode
            | Self::StringFromCodePoint => Some(PrimitiveHint::Number),
            Self::ParseInt | Self::ParseFloat => Some(PrimitiveHint::String),
            // `new Date(value)` is `ToPrimitive(value)` with no preferred type at all: it decides
            // between parsing a date string and reading an instant only once the value has
            // reduced, so it cannot have asked for a number. Every other arity is the field form,
            // which takes seven numbers left to right.
            Self::DateConstructor if argument_count == 1 => Some(PrimitiveHint::Default),
            Self::DateConstructor => Some(PrimitiveHint::Number),
            _ => None,
        }
    }

    /// Whether this built-in rearranges its array in place, keeping the length.
    pub(crate) fn rearranges_in_place(self) -> bool {
        matches!(self, Self::ArrayReverse)
    }

    /// Whether this method creates an iterator that must keep observing its original receiver.
    ///
    /// Most generic array methods can operate on a temporary dense copy. Iterators cannot: a
    /// write after iterator creation but before `next()` has to be visible, especially for a
    /// typed-array view whose whole purpose is sharing its backing bytes.
    pub(crate) fn creates_array_iterator(self) -> bool {
        matches!(
            self,
            Self::ArrayEntries | Self::ArrayKeys | Self::ArrayValues
        )
    }

    /// Whether this built-in reads its receiver as an array, and so should accept an array-like
    /// one when it is redirected through `call` or `apply`.
    ///
    /// The constructor and the statics are excluded: `Array.from` and `Array.isArray` take their
    /// subject as an argument, and coercing a receiver they never read would be noise.
    pub(crate) fn walks_an_array(self) -> bool {
        matches!(
            self,
            Self::ArrayCopy(_)
                | Self::ArrayAt
                | Self::ArrayConcat
                | Self::ArrayEntries
                | Self::ArrayFill
                | Self::ArrayFlat
                | Self::ArrayIncludes
                | Self::ArrayIndexOf
                | Self::ArrayJoin
                | Self::ArrayKeys
                | Self::ArrayLastIndexOf
                | Self::ArrayPop
                | Self::ArrayPush
                | Self::ArrayReverse
                | Self::ArrayShift
                | Self::ArraySlice
                | Self::ArrayUnshift
                | Self::ArrayValues
        )
    }

    /// Whether this `Array.prototype` method has a generic implementation that walks the
    /// receiver the program supplied rather than a copy of it.
    ///
    /// The specification defines every one of them on an arbitrary object -- `HasProperty`,
    /// `Get`, `Set` and `DeletePropertyOrThrow` against the receiver, with the bound read
    /// through `Get(O, "length")` -- and a copy answers differently in three ways at once: a
    /// mutating method writes nothing back, an index the walk deleted or that only the prototype
    /// carries is visited anyway, and the receiver a callback is handed is an array the program
    /// never saw. An array receiver keeps the dense path, which is the same walk without the
    /// indirection.
    pub(crate) fn walks_a_live_receiver(self) -> bool {
        matches!(
            self,
            Self::ArrayAt
                | Self::ArrayConcat
                | Self::ArrayCopy(_)
                | Self::ArrayCopyWithin
                | Self::ArrayFill
                | Self::ArrayFlat
                | Self::ArrayIncludes
                | Self::ArrayIndexOf
                | Self::ArrayJoin
                | Self::ArrayJoinDefault
                | Self::ArrayLastIndexOf
                | Self::ArrayPop
                | Self::ArrayPush
                | Self::ArrayReverse
                | Self::ArrayShift
                | Self::ArraySlice
                | Self::ArraySplice
                | Self::ArrayUnshift
                | Self::ArrayEntries
                | Self::ArrayKeys
                | Self::ArrayValues
        )
    }

    /// Whether this is one of the generic `String.prototype` methods.
    ///
    /// Each begins `RequireObjectCoercible(this)` then `ToString(this)`: they are defined on the
    /// *value*, not on the string type, which is why `[].join.call` has a `String.prototype`
    /// counterpart at all. The coercion may call the program's own `toString`, so it cannot
    /// happen in the pure table -- see [`Vm::dispatch_native`].
    pub(crate) fn walks_a_string(self) -> bool {
        matches!(
            self,
            Self::StringToUpperCase
                | Self::StringToLowerCase
                | Self::StringTrim
                | Self::StringIncludes
                | Self::StringIndexOf
                | Self::StringSlice
                | Self::StringSplit
                | Self::StringStartsWith
                | Self::StringEndsWith
                | Self::StringRepeat
                | Self::StringAt
                | Self::StringCharAt
                | Self::StringCharCodeAt
                | Self::StringPadStart
                | Self::StringPadEnd
                | Self::StringReplace
                | Self::StringReplaceAll
                | Self::StringLastIndexOf
                | Self::StringTrimStart
                | Self::StringTrimEnd
                | Self::StringConcat
                | Self::StringSubstring
                | Self::StringSubstr
                | Self::StringCodePointAt
                | Self::StringLocaleCompare
                | Self::StringNormalize
        )
    }

    /// What a built-in prototype table publishes itself, before the step up to `Object.prototype`.
    pub(crate) fn prototype_table_value(table: PrototypeTable, name: &str) -> Value {
        match table {
            PrototypeTable::Object => object_property(name),
            PrototypeTable::Array => array_property(name),
            PrototypeTable::String => string_property(name),
            PrototypeTable::Number => number_property(name),
            PrototypeTable::Function => tabulated_function_property(name),
            PrototypeTable::RegExp => regexp_method(name),
            PrototypeTable::Date => date_method(name),
            PrototypeTable::ArrayBuffer => array_buffer_method(false, name),
            PrototypeTable::SharedArrayBuffer => array_buffer_method(true, name),
            // Every method on `%TypedArray%.prototype` is the shared bootstrap's
            // (`kiln-builtins/src/typedarray.ts`), in the realm's store.
            PrototypeTable::TypedArray => Value::Undefined,
            // The per-view table carries only what differs between the views; everything
            // else is inherited from `%TypedArray%.prototype` one link up the chain, which
            // is why reading it here rather than there would be the wrong shape.
            PrototypeTable::TypedArrayFor(kind) => match name {
                "BYTES_PER_ELEMENT" => Value::Number(kind.width() as f64),
                _ => typed_array_for_method(kind, name),
            },
            PrototypeTable::DataView => data_view_method(name),
            PrototypeTable::Iterator => iterator_method(name),
            PrototypeTable::IteratorFor(kind) => iterator_prototype_method(kind, name),
            PrototypeTable::Generator => generator_prototype_method(false, name),
            PrototypeTable::AsyncGenerator => generator_prototype_method(true, name),
            PrototypeTable::AsyncIterator => match name {
                key if key == WellKnown::AsyncIterator.property_key() => {
                    Value::Native(Native::IteratorSelf, RealmId::PRIMARY)
                }
                key if key == WellKnown::ToStringTag.property_key() => {
                    Value::String("AsyncIterator".to_owned().into())
                }
                _ => Value::Undefined,
            },
            PrototypeTable::Boolean => boolean_method(name),
            PrototypeTable::Symbol => {
                if let Some(member) = kiln_builtin_surface::installed_prototype_member(
                    kiln_builtin_surface::PrototypeId::Symbol,
                    name,
                ) {
                    match member.binding {
                        kiln_builtin_surface::SemanticBinding::PrimitiveToString => {
                            Value::Native(Native::PrimitiveToString, RealmId::PRIMARY)
                        }
                        kiln_builtin_surface::SemanticBinding::PrimitiveValueOf => {
                            Value::Native(Native::SymbolValueOf, RealmId::PRIMARY)
                        }
                        kiln_builtin_surface::SemanticBinding::StringTag(tag) => {
                            Value::String(tag.to_owned().into())
                        }
                        kiln_builtin_surface::SemanticBinding::SymbolToPrimitive => {
                            Value::Native(Native::SymbolToPrimitive, RealmId::PRIMARY)
                        }
                        kiln_builtin_surface::SemanticBinding::Constructor => {
                            Value::Native(Native::SymbolConstructor, RealmId::PRIMARY)
                        }
                        _ => Value::Undefined,
                    }
                } else {
                    Value::Undefined
                }
            }
            PrototypeTable::BigInt => bigint_method(name),
            PrototypeTable::Promise => promise_method(name),
            PrototypeTable::Map => tabulated_map_property(name),
            PrototypeTable::Set => tabulated_set_property(name),
            PrototypeTable::WeakMap => tabulated_weak_map_property(name),
            PrototypeTable::WeakSet => tabulated_weak_set_property(name),
            PrototypeTable::WeakRef => weak_ref_method(name),
            PrototypeTable::ShadowRealm => shadow_realm_method(name),
            PrototypeTable::DisposableStack => disposable_stack_method(name),
            PrototypeTable::AsyncDisposableStack => async_disposable_stack_method(name),
            PrototypeTable::FinalizationRegistry => finalization_registry_method(name),
            PrototypeTable::EventEmitter => Value::Undefined,
            PrototypeTable::Console => Value::Undefined,
            PrototypeTable::Readable => Self::PrototypeOf(PrototypeTable::EventEmitter)
                .property(name)
                .unwrap_or(Value::Undefined),
            PrototypeTable::Writable => Self::PrototypeOf(PrototypeTable::EventEmitter)
                .property(name)
                .unwrap_or(Value::Undefined),
            PrototypeTable::WebStub(_) => Value::Undefined,
        }
    }

    /// Fixed own rows that are not described by the shared surface tables. Names and values are
    /// one authority; property() converts the selected row to a Value.
    pub(crate) fn own_property_rows(self) -> &'static [(&'static str, FixedOwnValue)] {
        match self {
            Self::AsyncFunctionPrototype => &[(
                "constructor",
                FixedOwnValue::Native(Self::AsyncFunctionConstructor),
            )],
            Self::AbortSignalConstructor => &[
                ("abort", FixedOwnValue::Native(Self::AbortSignalAbort)),
                ("timeout", FixedOwnValue::Native(Self::AbortSignalTimeout)),
                ("any", FixedOwnValue::Native(Self::AbortSignalAny)),
            ],
            Self::AsyncGeneratorFunctionPrototype => &[
                (
                    "constructor",
                    FixedOwnValue::Native(Self::AsyncGeneratorFunctionConstructor),
                ),
                (
                    "prototype",
                    FixedOwnValue::Native(Self::PrototypeOf(PrototypeTable::AsyncGenerator)),
                ),
            ],
            Self::GeneratorFunctionPrototype => &[
                (
                    "constructor",
                    FixedOwnValue::Native(Self::GeneratorFunctionConstructor),
                ),
                (
                    "prototype",
                    FixedOwnValue::Native(Self::PrototypeOf(PrototypeTable::Generator)),
                ),
            ],
            Self::NodeFsSync => &[("native", FixedOwnValue::Native(Self::NodeFsSync))],
            Self::NodeFsWriteSync => &[("native", FixedOwnValue::Native(Self::NodeFsWriteSync))],
            Self::NodeFsRealpathSync => {
                &[("native", FixedOwnValue::Native(Self::NodeFsRealpathSync))]
            }
            Self::NodeUtilInspect => &[(
                "custom",
                FixedOwnValue::Symbol(WellKnown::NodeInspectCustom),
            )],
            Self::ProcessHrtime => &[("bigint", FixedOwnValue::Native(Self::ProcessHrtimeBigInt))],
            Self::Error(_) => &[(
                "captureStackTrace",
                FixedOwnValue::Native(Self::ErrorCaptureStackTrace),
            )],
            _ => &[],
        }
    }

    pub(crate) fn property(self, name: &str) -> Option<Value> {
        if let Some((_, value)) = self
            .own_property_rows()
            .iter()
            .find(|(key, _)| *key == name)
        {
            return Some(match value {
                FixedOwnValue::Native(native) => Value::Native(*native, RealmId::PRIMARY),
                FixedOwnValue::Symbol(symbol) => Value::Symbol(Symbol::WellKnown(*symbol)),
            });
        }
        match (self, name) {
            // `realpathSync.native` is the only member any of these publishes, and it is the
            // same refusal reached one level down.
            (Self::NodeFsPromises, prop) => crate::fs::promises_property(prop),
            (Self::NodeFs, prop) => crate::fs::property(prop),
            (Self::NodeDiagnosticsChannel, prop) => crate::diagnostics_channel::property(prop),
            (Self::NodeUtil, prop) => crate::util::property(prop),
            // `default` binds to the module namespace unless an arm below overrides it, and an
            // override is only correct where node's own default is callable. Measured against
            // node v25.9.0, exactly four are: `assert`, `events`, `module` and `stream`.
            // `async_hooks`, `string_decoder` and `perf_hooks` used to override too, which bound
            // their defaults to `AsyncResource`, `StringDecoder` and `performance` and made
            // `typeof` answer "function" where node answers "object".
            (Self::NodeAssert, prop) => crate::assert::property(prop),
            (Self::NodeAsyncHooks, prop) => crate::async_hooks::property(prop),
            (Self::NodeCrypto, prop) => crate::crypto::property(prop),
            (Self::NodeStringDecoder, prop) => crate::string_decoder::property(prop),
            (Self::NodeStream, prop) => crate::stream::property(prop),
            (Self::NodeReadable, prop) => crate::stream::readable_constructor_property(prop),
            (Self::NodeWritable, prop) => crate::stream::writable_constructor_property(prop),
            (Self::NodeHttpServerResponse, "prototype") => Some(Value::Native(
                Self::PrototypeOf(PrototypeTable::Writable),
                RealmId::PRIMARY,
            )),
            (Self::NodeDns, prop) => crate::dns::property(prop),
            (Self::NodeOs, prop) => crate::os::property(prop),
            (Self::NodeTty, prop) => crate::tty::property(prop),
            (Self::NodeZlib, prop) => crate::zlib::property(prop),
            (Self::NodePerfHooks, prop) => crate::perf_hooks::property(prop),
            (Self::NodeHttps, prop) => crate::http::https_property(prop),
            (Self::NodeHttp2, prop) => crate::http::http2_property(prop),
            (Self::NodeModule, "createRequire") => Some(Value::Native(
                Self::NodeModuleCreateRequire,
                RealmId::PRIMARY,
            )),
            (Self::NodeModule, "Module" | "default") => {
                Some(Value::Native(Self::NodeModule, RealmId::PRIMARY))
            }
            (Self::NodeModule, "builtinModules") => {
                Some(Value::Array(crate::ArrayData::new(Vec::new())))
            }
            (Self::NodeModule, "isBuiltin") => {
                Some(Value::Native(Self::NodeModule, RealmId::PRIMARY))
            }
            (Self::NodePath, prop) => crate::path::property(prop),
            (Self::NodeWorkerThreads, "Worker") => {
                Some(Value::Native(Self::NodeWorkerConstructor, RealmId::PRIMARY))
            }
            (Self::NodeQueryString, prop) => crate::querystring::property(prop),
            (Self::NodeUrl, prop) => crate::url::property(prop),
            (Self::NodeEvents, prop) => crate::events::property(prop),
            (Self::NodeNet, prop) => crate::net::property(prop),
            (Self::NodeHttp, prop) => crate::http::property(prop),
            (Self::NodeBuffer | Self::NodeBufferGlobal, prop) => {
                crate::buffer::property(self, prop)
            }
            (Self::WebStub(kind), "prototype") => Some(Value::Native(
                Self::PrototypeOf(PrototypeTable::WebStub(kind)),
                RealmId::PRIMARY,
            )),
            (Self::WebStub(kind), "name") => Some(Value::String(kind.name().to_owned().into())),
            (Self::ConsoleConstructor, prop) => crate::console::constructor_property(prop),
            (Self::PrototypeOf(PrototypeTable::Console), prop) => {
                crate::console::prototype_property(prop).or_else(|| {
                    Some(inherit_from_object(
                        PrototypeTable::Console,
                        Value::Undefined,
                        prop,
                    ))
                })
            }
            (Self::NodeConsole, prop) => crate::console::property(prop),
            (Self::NodeEventEmitter, prop) => crate::events::event_emitter_property(prop),
            (Self::PrototypeOf(PrototypeTable::EventEmitter), prop) => {
                crate::events::prototype_property(prop).or_else(|| {
                    Some(inherit_from_object(
                        PrototypeTable::EventEmitter,
                        Value::Undefined,
                        prop,
                    ))
                })
            }
            (Self::PrototypeOf(PrototypeTable::Readable), prop) => {
                crate::stream::readable_prototype_property(prop)
                    .or_else(|| Self::PrototypeOf(PrototypeTable::EventEmitter).property(prop))
            }
            (Self::PrototypeOf(PrototypeTable::Writable), prop) => {
                crate::stream::writable_prototype_property(prop)
                    .or_else(|| Self::PrototypeOf(PrototypeTable::EventEmitter).property(prop))
            }
            // Every `Temporal` namespace, constructor and prototype answers from one place, so
            // the eight types cannot drift on what they expose.
            (Self::Temporal(op), name) => temporal::static_property(op, name),
            // The same arrangement for `Intl`: one table for the namespace, the six
            // constructors and their prototypes, so none of them can drift from the others.
            (Self::Intl(op), name) => intl::static_property(op, name),
            // `e.constructor.name` reaches this: the instance finds its constructor through the
            // prototype, and the constructor knows which kind it is.
            (Self::Error(kind), "name") => Some(Value::String(kind.name().to_owned().into())),
            (Self::Error(ErrorKind::Error), "isError") => {
                Some(Value::Native(Self::ErrorIsError, RealmId::PRIMARY))
            }
            (Self::ArrayConstructor, "isArray") => {
                Some(Value::Native(Self::ArrayIsArray, RealmId::PRIMARY))
            }
            (Self::ArrayConstructor, "from") => {
                Some(Value::Native(Self::ArrayFrom, RealmId::PRIMARY))
            }
            (Self::ObjectConstructor, "keys") => {
                Some(Value::Native(Self::ObjectKeys, RealmId::PRIMARY))
            }
            // Not `Object.keys`: this reports every own property and that one reports only
            // the enumerated ones. They were the same function until an error object gave them
            // different answers.
            (Self::ObjectConstructor, "getOwnPropertyNames") => {
                Some(Value::Native(Self::ObjectOwnNames, RealmId::PRIMARY))
            }
            (Self::ObjectConstructor, "assign") => {
                Some(Value::Native(Self::ObjectAssign, RealmId::PRIMARY))
            }
            (Self::ObjectConstructor, "entries") => {
                Some(Value::Native(Self::ObjectEntries, RealmId::PRIMARY))
            }
            (Self::ObjectConstructor, "values") => {
                Some(Value::Native(Self::ObjectValues, RealmId::PRIMARY))
            }
            (Self::ObjectConstructor, "freeze") => {
                Some(Value::Native(Self::ObjectFreeze, RealmId::PRIMARY))
            }
            (Self::ObjectConstructor, "isFrozen") => {
                Some(Value::Native(Self::ObjectIsFrozen, RealmId::PRIMARY))
            }
            (Self::ObjectConstructor, "create") => {
                Some(Value::Native(Self::ObjectCreate, RealmId::PRIMARY))
            }
            (Self::ObjectConstructor, "defineProperty") => {
                Some(Value::Native(Self::ObjectDefineProperty, RealmId::PRIMARY))
            }
            (Self::ObjectConstructor, "fromEntries") => {
                Some(Value::Native(Self::ObjectFromEntries, RealmId::PRIMARY))
            }
            (Self::ArrayConstructor, "of") => Some(Value::Native(Self::ArrayOf, RealmId::PRIMARY)),
            (Self::StringConstructor, "raw") => {
                Some(Value::Native(Self::StringRaw, RealmId::PRIMARY))
            }
            (Self::StringConstructor, "fromCharCode") => {
                Some(Value::Native(Self::StringFromCharCode, RealmId::PRIMARY))
            }
            (Self::StringConstructor, "fromCodePoint") => {
                Some(Value::Native(Self::StringFromCodePoint, RealmId::PRIMARY))
            }
            // `Reflect` is mostly the `Object` statics with a different failure convention, and
            // the ones a program actually reaches are the ones that behave identically here.
            (Self::Reflect, "ownKeys") => {
                Some(Value::Native(Self::ReflectOwnKeys, RealmId::PRIMARY))
            }
            (Self::Reflect, "get") => Some(Value::Native(Self::ReflectGet, RealmId::PRIMARY)),
            (Self::Reflect, "set") => Some(Value::Native(Self::ReflectSet, RealmId::PRIMARY)),
            (Self::Reflect, "has") => Some(Value::Native(Self::ReflectHas, RealmId::PRIMARY)),
            (Self::Reflect, "apply") => Some(Value::Native(Self::ReflectApply, RealmId::PRIMARY)),
            (Self::Reflect, "construct") => {
                Some(Value::Native(Self::ReflectConstruct, RealmId::PRIMARY))
            }
            (Self::Reflect, "getPrototypeOf") => {
                Some(Value::Native(Self::ReflectGetPrototypeOf, RealmId::PRIMARY))
            }
            (Self::Reflect, "defineProperty") => {
                Some(Value::Native(Self::ReflectDefineProperty, RealmId::PRIMARY))
            }
            (Self::Reflect, "getOwnPropertyDescriptor") => Some(Value::Native(
                Self::ReflectGetOwnPropertyDescriptor,
                RealmId::PRIMARY,
            )),
            (Self::Reflect, "deleteProperty") => {
                Some(Value::Native(Self::ReflectDeleteProperty, RealmId::PRIMARY))
            }
            (Self::ObjectConstructor, "hasOwn") => {
                Some(Value::Native(Self::ObjectHasOwn, RealmId::PRIMARY))
            }
            (Self::ObjectConstructor, "groupBy") => {
                Some(Value::Native(Self::ObjectGroupBy, RealmId::PRIMARY))
            }
            (Self::MapConstructor, "groupBy") => {
                Some(Value::Native(Self::MapGroupBy, RealmId::PRIMARY))
            }
            (Self::PromiseConstructor, "withResolvers") => {
                Some(Value::Native(Self::PromiseWithResolvers, RealmId::PRIMARY))
            }
            (Self::ObjectConstructor, "is") => {
                Some(Value::Native(Self::ObjectIs, RealmId::PRIMARY))
            }
            (Self::ObjectConstructor, "prototype") => Some(Value::Native(
                Self::PrototypeOf(PrototypeTable::Object),
                RealmId::PRIMARY,
            )),
            (Self::ArrayConstructor, "prototype") => Some(Value::Native(
                Self::PrototypeOf(PrototypeTable::Array),
                RealmId::PRIMARY,
            )),
            (Self::StringConstructor, "prototype") => Some(Value::Native(
                Self::PrototypeOf(PrototypeTable::String),
                RealmId::PRIMARY,
            )),
            (Self::NumberConstructor, "prototype") => Some(Value::Native(
                Self::PrototypeOf(PrototypeTable::Number),
                RealmId::PRIMARY,
            )),
            (Self::RegExpConstructor, "prototype") => Some(Value::Native(
                Self::PrototypeOf(PrototypeTable::RegExp),
                RealmId::PRIMARY,
            )),
            (Self::DateConstructor, "prototype") => Some(Value::Native(
                Self::PrototypeOf(PrototypeTable::Date),
                RealmId::PRIMARY,
            )),
            (Self::ArrayBufferConstructor, "prototype") => Some(Value::Native(
                Self::PrototypeOf(PrototypeTable::ArrayBuffer),
                RealmId::PRIMARY,
            )),
            (Self::ArrayBufferConstructor, "isView") => {
                Some(Value::Native(Self::ArrayBufferIsView, RealmId::PRIMARY))
            }
            (Self::SharedArrayBufferConstructor, "prototype") => Some(Value::Native(
                Self::PrototypeOf(PrototypeTable::SharedArrayBuffer),
                RealmId::PRIMARY,
            )),
            (Self::DataViewConstructor, "prototype") => Some(Value::Native(
                Self::PrototypeOf(PrototypeTable::DataView),
                RealmId::PRIMARY,
            )),
            (Self::IteratorConstructor, "prototype") => Some(Value::Native(
                Self::PrototypeOf(PrototypeTable::Iterator),
                RealmId::PRIMARY,
            )),
            (Self::IteratorConstructor, "from") => {
                Some(Value::Native(Self::IteratorFrom, RealmId::PRIMARY))
            }
            (Self::BooleanConstructor, "prototype") => Some(Value::Native(
                Self::PrototypeOf(PrototypeTable::Boolean),
                RealmId::PRIMARY,
            )),
            (Self::SymbolConstructor, "prototype") => Some(Value::Native(
                Self::PrototypeOf(PrototypeTable::Symbol),
                RealmId::PRIMARY,
            )),
            (Self::BigIntConstructor, "prototype") => Some(Value::Native(
                Self::PrototypeOf(PrototypeTable::BigInt),
                RealmId::PRIMARY,
            )),
            (Self::BigIntConstructor, "asIntN") => {
                Some(Value::Native(Self::BigIntAsIntN, RealmId::PRIMARY))
            }
            (Self::BigIntConstructor, "asUintN") => {
                Some(Value::Native(Self::BigIntAsUintN, RealmId::PRIMARY))
            }
            (Self::PromiseConstructor, "prototype") => Some(Value::Native(
                Self::PrototypeOf(PrototypeTable::Promise),
                RealmId::PRIMARY,
            )),
            (Self::MapConstructor, "prototype") => Some(Value::Native(
                Self::PrototypeOf(PrototypeTable::Map),
                RealmId::PRIMARY,
            )),
            (Self::SetConstructor, "prototype") => Some(Value::Native(
                Self::PrototypeOf(PrototypeTable::Set),
                RealmId::PRIMARY,
            )),
            (Self::WeakMapConstructor, "prototype") => Some(Value::Native(
                Self::PrototypeOf(PrototypeTable::WeakMap),
                RealmId::PRIMARY,
            )),
            (Self::WeakSetConstructor, "prototype") => Some(Value::Native(
                Self::PrototypeOf(PrototypeTable::WeakSet),
                RealmId::PRIMARY,
            )),
            (Self::WeakRefConstructor, "prototype") => Some(Value::Native(
                Self::PrototypeOf(PrototypeTable::WeakRef),
                RealmId::PRIMARY,
            )),
            (Self::ShadowRealmConstructor, "prototype") => Some(Value::Native(
                Self::PrototypeOf(PrototypeTable::ShadowRealm),
                RealmId::PRIMARY,
            )),
            (Self::DisposableStackConstructor, "prototype") => Some(Value::Native(
                Self::PrototypeOf(PrototypeTable::DisposableStack),
                RealmId::PRIMARY,
            )),
            (Self::AsyncDisposableStackConstructor, "prototype") => Some(Value::Native(
                Self::PrototypeOf(PrototypeTable::AsyncDisposableStack),
                RealmId::PRIMARY,
            )),
            (Self::FinalizationRegistryConstructor, "prototype") => Some(Value::Native(
                Self::PrototypeOf(PrototypeTable::FinalizationRegistry),
                RealmId::PRIMARY,
            )),
            (Self::TypedArrayIntrinsic, "prototype") => Some(Value::Native(
                Self::PrototypeOf(PrototypeTable::TypedArray),
                RealmId::PRIMARY,
            )),
            // The shared constructor owns these methods; each concrete constructor inherits the
            // same callable through its [[Prototype]] link and supplies itself as `this`.
            (Self::TypedArrayIntrinsic, "of") => Some(Value::Native(
                Self::TypedArrayOf(TypedKind::Int8),
                RealmId::PRIMARY,
            )),
            (Self::TypedArrayIntrinsic, "from") => Some(Value::Native(
                Self::TypedArrayFrom(TypedKind::Int8),
                RealmId::PRIMARY,
            )),
            (Self::TypedArrayConstructor(kind), "prototype") => Some(Value::Native(
                Self::PrototypeOf(PrototypeTable::TypedArrayFor(kind)),
                RealmId::PRIMARY,
            )),
            // A method read off one of these tables is unbound, exactly as reading it off an
            // instance is: `call` and `apply` are what supply the receiver.
            // `Proxy.revocable(target, handler)` hands back the proxy and the switch that turns
            // it off, as a pair.
            (Self::ProxyConstructor, "revocable") => {
                Some(Value::Native(Self::ProxyRevocable, RealmId::PRIMARY))
            }
            (Self::TypedArrayConstructor(_), "of") => Some(Value::Native(
                Self::TypedArrayOf(TypedKind::Int8),
                RealmId::PRIMARY,
            )),
            (Self::TypedArrayConstructor(_), "from") => Some(Value::Native(
                Self::TypedArrayFrom(TypedKind::Int8),
                RealmId::PRIMARY,
            )),
            (Self::TypedArrayConstructor(kind), "BYTES_PER_ELEMENT") => {
                Some(Value::Number(kind.width() as f64))
            }
            // The two that *build* a view are statics on `Uint8Array`; the four that read or
            // fill one are methods on its prototype, and both lists come from one place.
            (Self::TypedArrayConstructor(kind), _) => byte_text_method(kind, name, true),
            // Every function inherits from `Function.prototype`, which is what the interop
            // shims test when they clone a module's shape.
            (Self::FunctionConstructor, "prototype") => Some(Value::Native(
                Self::PrototypeOf(PrototypeTable::Function),
                RealmId::PRIMARY,
            )),
            (Self::AsyncFunctionConstructor, "prototype") => Some(Value::Native(
                Self::AsyncFunctionPrototype,
                RealmId::PRIMARY,
            )),
            (Self::AsyncFunctionConstructor, "name") => {
                Some(Value::String("AsyncFunction".to_owned().into()))
            }
            (Self::AsyncFunctionConstructor, "length") => Some(Value::Number(1.0)),
            (Self::AsyncFunctionPrototype, key) if key == WellKnown::ToStringTag.property_key() => {
                Some(Value::String("AsyncFunction".to_owned().into()))
            }
            (Self::AsyncGeneratorFunctionConstructor, "prototype") => Some(Value::Native(
                Self::AsyncGeneratorFunctionPrototype,
                RealmId::PRIMARY,
            )),
            (Self::AsyncGeneratorFunctionConstructor, "name") => {
                Some(Value::String("AsyncGeneratorFunction".to_owned().into()))
            }
            (Self::AsyncGeneratorFunctionConstructor, "length") => Some(Value::Number(1.0)),
            (Self::AsyncGeneratorFunctionPrototype, key)
                if key == WellKnown::ToStringTag.property_key() =>
            {
                Some(Value::String("AsyncGeneratorFunction".to_owned().into()))
            }
            (Self::GeneratorFunctionConstructor, "prototype") => Some(Value::Native(
                Self::GeneratorFunctionPrototype,
                RealmId::PRIMARY,
            )),
            (Self::GeneratorFunctionConstructor, "name") => {
                Some(Value::String("GeneratorFunction".to_owned().into()))
            }
            (Self::GeneratorFunctionConstructor, "length") => Some(Value::Number(1.0)),
            (Self::GeneratorFunctionPrototype, key)
                if key == WellKnown::ToStringTag.property_key() =>
            {
                Some(Value::String("GeneratorFunction".to_owned().into()))
            }
            // A method table's `constructor` is the built-in it belongs to, which is what
            // `Object.prototype.constructor` has to answer for a plain-object check to work.
            // `%GeneratorPrototype%.constructor` is `%GeneratorFunction.prototype%`.
            // `%AsyncGeneratorPrototype%.constructor` is `%AsyncGeneratorFunction.prototype%`.
            // `%ArrayIteratorPrototype%` and its siblings publish none: the specification gives them
            // no `constructor`, and the tag is how a program tells one kind of iterator from another.
            (
                Self::PrototypeOf(PrototypeTable::AsyncIterator | PrototypeTable::IteratorFor(_)),
                "constructor",
            ) => None,
            (Self::PrototypeOf(table), "constructor") => Some(Value::Native(
                match table {
                    // Caught by the arm above, which publishes no `constructor` for them.
                    PrototypeTable::AsyncIterator | PrototypeTable::IteratorFor(_) => {
                        unreachable!("these tables answer `constructor` above")
                    }
                    PrototypeTable::Generator => Self::GeneratorFunctionPrototype,
                    PrototypeTable::AsyncGenerator => Self::AsyncGeneratorFunctionPrototype,
                    PrototypeTable::Object => Self::ObjectConstructor,
                    PrototypeTable::Array => Self::ArrayConstructor,
                    PrototypeTable::String => Self::StringConstructor,
                    PrototypeTable::Number => Self::NumberConstructor,
                    PrototypeTable::Function => Self::FunctionConstructor,
                    PrototypeTable::RegExp => Self::RegExpConstructor,
                    PrototypeTable::Date => Self::DateConstructor,
                    PrototypeTable::ArrayBuffer => Self::ArrayBufferConstructor,
                    PrototypeTable::SharedArrayBuffer => Self::SharedArrayBufferConstructor,
                    PrototypeTable::TypedArray => Self::TypedArrayIntrinsic,
                    PrototypeTable::TypedArrayFor(kind) => Self::TypedArrayConstructor(kind),
                    PrototypeTable::DataView => Self::DataViewConstructor,
                    PrototypeTable::Iterator => Self::IteratorConstructor,
                    PrototypeTable::Boolean => Self::BooleanConstructor,
                    PrototypeTable::Symbol => Self::SymbolConstructor,
                    PrototypeTable::BigInt => Self::BigIntConstructor,
                    PrototypeTable::Promise => Self::PromiseConstructor,
                    PrototypeTable::Map => Self::MapConstructor,
                    PrototypeTable::Set => Self::SetConstructor,
                    PrototypeTable::WeakMap => Self::WeakMapConstructor,
                    PrototypeTable::WeakSet => Self::WeakSetConstructor,
                    PrototypeTable::WeakRef => Self::WeakRefConstructor,
                    PrototypeTable::ShadowRealm => Self::ShadowRealmConstructor,
                    PrototypeTable::DisposableStack => Self::DisposableStackConstructor,
                    PrototypeTable::AsyncDisposableStack => Self::AsyncDisposableStackConstructor,
                    PrototypeTable::FinalizationRegistry => Self::FinalizationRegistryConstructor,
                    PrototypeTable::EventEmitter => Self::NodeEventEmitter,
                    PrototypeTable::Console => Self::ConsoleConstructor,
                    PrototypeTable::Readable => Self::NodeReadable,
                    PrototypeTable::Writable => Self::NodeWritable,
                    PrototypeTable::WebStub(kind) => Self::WebStub(kind),
                },
                RealmId::PRIMARY,
            )),
            // A read falls through to `Object.prototype`; `own_property` does not, which is the
            // difference between what this table *has* and what a read through it *finds*.
            (Self::PrototypeOf(table), _) => Some(inherit_from_object(
                table,
                Self::prototype_table_value(table, name),
                name,
            )),
            (Self::ObjectConstructor, "preventExtensions") => Some(Value::Native(
                Self::ObjectPreventExtensions,
                RealmId::PRIMARY,
            )),
            (Self::ObjectConstructor, "isExtensible") => {
                Some(Value::Native(Self::ObjectIsExtensible, RealmId::PRIMARY))
            }
            (Self::ObjectConstructor, "getOwnPropertySymbols") => Some(Value::Native(
                Self::ObjectGetOwnPropertySymbols,
                RealmId::PRIMARY,
            )),
            (Self::ObjectConstructor, "defineProperties") => Some(Value::Native(
                Self::ObjectDefineProperties,
                RealmId::PRIMARY,
            )),
            (Self::ObjectConstructor, "getOwnPropertyDescriptor") => Some(Value::Native(
                Self::ObjectGetOwnPropertyDescriptor,
                RealmId::PRIMARY,
            )),
            (Self::ObjectConstructor, "getOwnPropertyDescriptors") => Some(Value::Native(
                Self::ObjectGetOwnPropertyDescriptors,
                RealmId::PRIMARY,
            )),
            (Self::ObjectConstructor, "setPrototypeOf") => {
                Some(Value::Native(Self::ObjectSetPrototypeOf, RealmId::PRIMARY))
            }
            (Self::ObjectConstructor, "getPrototypeOf") => {
                Some(Value::Native(Self::ObjectGetPrototypeOf, RealmId::PRIMARY))
            }
            (Self::Math, "abs") => Some(Value::Native(Self::MathAbs, RealmId::PRIMARY)),
            (Self::Math, "floor") => Some(Value::Native(Self::MathFloor, RealmId::PRIMARY)),
            (Self::Math, "ceil") => Some(Value::Native(Self::MathCeil, RealmId::PRIMARY)),
            (Self::Math, "round") => Some(Value::Native(Self::MathRound, RealmId::PRIMARY)),
            (Self::Math, "trunc") => Some(Value::Native(Self::MathTrunc, RealmId::PRIMARY)),
            (Self::Math, "sign") => Some(Value::Native(Self::MathSign, RealmId::PRIMARY)),
            (Self::Math, "sqrt") => Some(Value::Native(Self::MathSqrt, RealmId::PRIMARY)),
            (Self::Math, "cbrt") => Some(Value::Native(Self::MathCbrt, RealmId::PRIMARY)),
            (Self::Math, "random") => Some(Value::Native(Self::MathRandom, RealmId::PRIMARY)),
            (Self::Math, "sinh") => Some(Value::Native(Self::MathSinh, RealmId::PRIMARY)),
            (Self::Math, "cosh") => Some(Value::Native(Self::MathCosh, RealmId::PRIMARY)),
            (Self::Math, "tanh") => Some(Value::Native(Self::MathTanh, RealmId::PRIMARY)),
            (Self::Math, "asinh") => Some(Value::Native(Self::MathAsinh, RealmId::PRIMARY)),
            (Self::Math, "acosh") => Some(Value::Native(Self::MathAcosh, RealmId::PRIMARY)),
            (Self::Math, "atanh") => Some(Value::Native(Self::MathAtanh, RealmId::PRIMARY)),
            (Self::Math, "log1p") => Some(Value::Native(Self::MathLog1p, RealmId::PRIMARY)),
            (Self::Math, "expm1") => Some(Value::Native(Self::MathExpm1, RealmId::PRIMARY)),
            (Self::Math, "fround") => Some(Value::Native(Self::MathFround, RealmId::PRIMARY)),
            (Self::Math, "f16round") => Some(Value::Native(Self::MathF16Round, RealmId::PRIMARY)),
            (Self::Math, "clz32") => Some(Value::Native(Self::MathClz32, RealmId::PRIMARY)),
            (Self::Math, "imul") => Some(Value::Native(Self::MathImul, RealmId::PRIMARY)),
            (Self::Math, "pow") => Some(Value::Native(Self::MathPow, RealmId::PRIMARY)),
            (Self::Math, "min") => Some(Value::Native(Self::MathMin, RealmId::PRIMARY)),
            (Self::Math, "max") => Some(Value::Native(Self::MathMax, RealmId::PRIMARY)),
            (Self::Math, "log") => Some(Value::Native(Self::MathLog, RealmId::PRIMARY)),
            (Self::Math, "log2") => Some(Value::Native(Self::MathLog2, RealmId::PRIMARY)),
            (Self::Math, "log10") => Some(Value::Native(Self::MathLog10, RealmId::PRIMARY)),
            (Self::Math, "exp") => Some(Value::Native(Self::MathExp, RealmId::PRIMARY)),
            (Self::Math, "PI") => Some(Value::Number(std::f64::consts::PI)),
            (Self::Math, "E") => Some(Value::Number(std::f64::consts::E)),
            (Self::Math, "LN2") => Some(Value::Number(std::f64::consts::LN_2)),
            (Self::Math, "LN10") => Some(Value::Number(std::f64::consts::LN_10)),
            (Self::Math, "SQRT2") => Some(Value::Number(std::f64::consts::SQRT_2)),
            (Self::Math, "SQRT1_2") => Some(Value::Number(std::f64::consts::FRAC_1_SQRT_2)),
            (Self::Math, "LOG2E") => Some(Value::Number(std::f64::consts::LOG2_E)),
            (Self::Math, "LOG10E") => Some(Value::Number(std::f64::consts::LOG10_E)),
            (Self::Math, "hypot") => Some(Value::Native(Self::MathHypot, RealmId::PRIMARY)),
            (Self::Math, "atan2") => Some(Value::Native(Self::MathAtan2, RealmId::PRIMARY)),
            (Self::Math, "sin") => Some(Value::Native(Self::MathSin, RealmId::PRIMARY)),
            (Self::Math, "cos") => Some(Value::Native(Self::MathCos, RealmId::PRIMARY)),
            (Self::Math, "tan") => Some(Value::Native(Self::MathTan, RealmId::PRIMARY)),
            (Self::Math, "asin") => Some(Value::Native(Self::MathAsin, RealmId::PRIMARY)),
            (Self::Math, "acos") => Some(Value::Native(Self::MathAcos, RealmId::PRIMARY)),
            (Self::Math, "atan") => Some(Value::Native(Self::MathAtan, RealmId::PRIMARY)),
            (Self::NumberConstructor, "isInteger") => {
                Some(Value::Native(Self::NumberIsInteger, RealmId::PRIMARY))
            }
            (Self::NumberConstructor, "isSafeInteger") => {
                Some(Value::Native(Self::NumberIsSafeInteger, RealmId::PRIMARY))
            }
            // `Number.isNaN` is not the global `isNaN`: it does not coerce, so
            // `Number.isNaN("x")` is false where `isNaN("x")` is true. Same code, different
            // entry points would be wrong -- these are genuinely two functions.
            (Self::NumberConstructor, "isNaN") => {
                Some(Value::Native(Self::NumberIsNaN, RealmId::PRIMARY))
            }
            (Self::NumberConstructor, "isFinite") => {
                Some(Value::Native(Self::NumberIsFinite, RealmId::PRIMARY))
            }
            (Self::NumberConstructor, "parseInt") => {
                Some(Value::Native(Self::ParseInt, RealmId::PRIMARY))
            }
            (Self::NumberConstructor, "parseFloat") => {
                Some(Value::Native(Self::ParseFloat, RealmId::PRIMARY))
            }
            (Self::NumberConstructor, "MAX_SAFE_INTEGER") => {
                Some(Value::Number(9_007_199_254_740_991.0))
            }
            (Self::NumberConstructor, "MIN_SAFE_INTEGER") => {
                Some(Value::Number(-9_007_199_254_740_991.0))
            }
            (Self::NumberConstructor, "MAX_VALUE") => Some(Value::Number(f64::MAX)),
            (Self::NumberConstructor, "MIN_VALUE") => Some(Value::Number(5e-324)),
            (Self::NumberConstructor, "EPSILON") => Some(Value::Number(f64::EPSILON)),
            (Self::NumberConstructor, "POSITIVE_INFINITY") => Some(Value::Number(f64::INFINITY)),
            (Self::NumberConstructor, "NEGATIVE_INFINITY") => {
                Some(Value::Number(f64::NEG_INFINITY))
            }
            (Self::NumberConstructor, "NaN") => Some(Value::Number(f64::NAN)),
            (Self::Performance, "now") => {
                Some(Value::Native(Self::PerformanceNow, RealmId::PRIMARY))
            }
            (Self::DateConstructor, "now") => Some(Value::Native(Self::DateNow, RealmId::PRIMARY)),
            (Self::DateConstructor, "UTC") => Some(Value::Native(Self::DateUtc, RealmId::PRIMARY)),
            (Self::DateConstructor, "parse") => {
                Some(Value::Native(Self::DateParse, RealmId::PRIMARY))
            }
            (Self::PromiseConstructor, "resolve") => {
                Some(Value::Native(Self::PromiseResolve, RealmId::PRIMARY))
            }
            (Self::PromiseConstructor, "reject") => {
                Some(Value::Native(Self::PromiseReject, RealmId::PRIMARY))
            }
            (Self::PromiseConstructor, "all") => {
                Some(Value::Native(Self::PromiseAll, RealmId::PRIMARY))
            }
            (Self::PromiseConstructor, "allSettled") => {
                Some(Value::Native(Self::PromiseAllSettled, RealmId::PRIMARY))
            }
            (Self::PromiseConstructor, "race") => {
                Some(Value::Native(Self::PromiseRace, RealmId::PRIMARY))
            }
            (Self::PromiseConstructor, "any") => {
                Some(Value::Native(Self::PromiseAny, RealmId::PRIMARY))
            }
            (Self::Json, "parse") => Some(Value::Native(Self::JsonParse, RealmId::PRIMARY)),
            (Self::Json, "rawJSON") => Some(Value::Native(Self::JsonRawJson, RealmId::PRIMARY)),
            // The tag `Object.prototype.toString` reads. A namespace has no prototype to hang it
            // on, so it is the object's own -- which is exactly how the specification defines it.
            (Self::Math | Self::Json | Self::Reflect, key)
                if key == WellKnown::ToStringTag.property_key() =>
            {
                Some(Value::String(
                    match self {
                        Self::Math => "Math",
                        Self::Reflect => "Reflect",
                        _ => "JSON",
                    }
                    .to_owned()
                    .into(),
                ))
            }
            // The tag `Object.prototype.toString` reads, and the operations themselves. Like
            // `Math`, the namespace *is* the object: there is no prototype to hang either on.
            (Self::Atomics, key) if key == WellKnown::ToStringTag.property_key() => {
                Some(Value::String("Atomics".to_owned().into()))
            }
            (Self::Atomics, _) => AtomicsOp::ALL
                .into_iter()
                .find(|op| op.property_key() == name)
                .map(|op| Value::Native(Self::AtomicsOp(op), RealmId::PRIMARY)),
            (Self::Json, "isRawJSON") => Some(Value::Native(Self::JsonIsRawJson, RealmId::PRIMARY)),
            (Self::Json, "stringify") => Some(Value::Native(Self::JsonStringify, RealmId::PRIMARY)),
            (Self::SymbolConstructor, "for") => {
                Some(Value::Native(Self::SymbolFor, RealmId::PRIMARY))
            }
            (Self::SymbolConstructor, "keyFor") => {
                Some(Value::Native(Self::SymbolKeyFor, RealmId::PRIMARY))
            }
            // The well-known symbols a program can name. Each is the *same* symbol every time,
            // which is the whole point of them -- and each is published even where nothing here
            // consults it yet, because a program that feature-detects one and finds it missing
            // takes a different path rather than failing loudly.
            (Self::SymbolConstructor, _) => WellKnown::ALL
                .into_iter()
                .filter(|symbol| symbol.published_by_symbol())
                .find(|symbol| symbol.js_name() == name)
                .map(|symbol| Value::Symbol(Symbol::WellKnown(symbol))),
            _ => None,
        }
    }
}
