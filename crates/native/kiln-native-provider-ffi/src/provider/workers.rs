//! The native realm's half of cross-realm ports, `node:worker_threads` and shared memory (realms
//! issue 06; ADR 0070 primitives A and C).
//!
//! The runtime (`kiln_runtime::workers`) owns ports, workers and the `Atomics.wait` list; this
//! module only carries message tokens across the C seam and hands the runtime what a native
//! realm's values are made of. A transferred `ArrayBuffer`'s block is `malloc` memory the C
//! runtime detached from its buffer: it travels as a [`Token::Moved`] and the receiving realm
//! adopts the same block. A `SharedArrayBuffer`'s bytes are a [`NativeShared`] store every realm
//! that received it points into, freed when the last of them lets go.
//!
//! A worker's realm is started by the graph launcher, which registers the function that runs a
//! module of the program as a worker ([`kiln_worker_set_runner`]); the runtime calls it on the
//! worker's own thread.

use super::{INVALID_ARGUMENT, KilnString, validate_string};
use core::ffi::{c_char, c_void};
use kiln_runtime::signals;
use kiln_runtime::workers::{self, Message, Token, WorkerEntry, WorkerSpec};
use kiln_string::{StringUnits, StringView};
use std::sync::Arc;
use std::sync::atomic::{AtomicI32, AtomicI64, AtomicPtr, AtomicUsize, Ordering};

unsafe extern "C" {
    fn calloc(count: usize, size: usize) -> *mut c_void;
    fn free(pointer: *mut c_void);
}

/// One message token as the C runtime reads and writes it (`kiln_worker_token`).
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct WorkerToken {
    pub kind: u32,
    /// A boolean's value; a string's width (0 Latin-1, 1 UTF-16).
    pub flag: u32,
    pub number: f64,
    /// A string's units or a copied buffer's bytes.
    pub data: *const c_void,
    /// How many units or bytes `data` holds; a moved block's length.
    pub len: u64,
    /// A moved block (`malloc` memory) or a shared store; whoever adopts it clears it here.
    pub handle: *mut c_void,
}

const TOKEN_UNDEFINED: u32 = 0;
const TOKEN_NULL: u32 = 1;
const TOKEN_BOOL: u32 = 2;
const TOKEN_NUMBER: u32 = 3;
const TOKEN_STRING: u32 = 4;
const TOKEN_MOVED: u32 = 5;
const TOKEN_BYTES: u32 = 6;
const TOKEN_SHARED: u32 = 7;

const OP_CHANNEL: u32 = 1;
const OP_POST: u32 = 2;
const OP_CLOSE: u32 = 4;
const OP_STATE: u32 = 5;
const OP_ENQUEUE: u32 = 6;
const OP_BROADCAST_POST: u32 = 8;
const OP_TERMINATE: u32 = 10;
const OP_REAP: u32 = 13;
/// Report the exception ending this worker to its parent (the launcher's, not the program's).
const OP_REPORT_ERROR: u32 = 15;
// Process signals (`kiln_runtime::signals`, realms issue 23).
const OP_SIGNAL_NUMBER: u32 = 16;
const OP_SIGNAL_LISTEN: u32 = 17;
const OP_SIGNAL_UNHANDLED: u32 = 18;
const OP_KILL: u32 = 19;
/// Not the TypeScript's: the thread-mode launcher's signal thread handing a signal to the
/// realms that listen for it.
const OP_SIGNAL_DELIVER: u32 = 20;

/// Borrowed, read-only description of one native heap value. The callback may reuse its
/// scratch on the next call; the provider copies strings before asking for another field.
#[repr(C)]
#[derive(Default)]
pub struct WorkerPlainNode {
    pub kind: u32,
    pub number: f64,
    pub text: *const KilnString,
    pub identity: usize,
    pub length: usize,
}

#[repr(C)]
#[derive(Default)]
pub struct WorkerPlainChild {
    pub key: *const KilnString,
    pub value: u64,
    pub hole: u32,
}

pub type WorkerPlainDescribe = unsafe extern "C" fn(*mut c_void, u64, *mut WorkerPlainNode) -> i32;
pub type WorkerPlainField =
    unsafe extern "C" fn(*mut c_void, u64, usize, *mut WorkerPlainChild) -> i32;

struct NativePlainSource {
    context: *mut c_void,
    describe: WorkerPlainDescribe,
    field: WorkerPlainField,
    compound_count: usize,
    /// Repeated shape keys in a large graph can share their owned string units for this one
    /// synchronous encode. Compare contents: a callback is allowed to reuse its scratch view.
    keys: [Option<StringUnits>; 8],
    key_count: usize,
}

impl workers::PlainSource for NativePlainSource {
    type Handle = u64;

    fn describe(&mut self, handle: u64) -> Option<workers::PlainNode> {
        let mut node = WorkerPlainNode::default();
        // SAFETY: the C callback is live for this synchronous call and writes one descriptor.
        if unsafe { (self.describe)(self.context, handle, &mut node) } != 1 {
            return None;
        }
        Some(match node.kind {
            0 => workers::PlainNode::Undefined,
            1 => workers::PlainNode::Null,
            2 => workers::PlainNode::Bool(false),
            3 => workers::PlainNode::Bool(true),
            4 => workers::PlainNode::Number(node.number),
            5 => {
                if node.text.is_null() {
                    return None;
                }
                // SAFETY: callback lends a live kiln_string through this call.
                let view = unsafe { validate_string(&*node.text) }?;
                workers::PlainNode::String(StringUnits::from_view(view))
            }
            8 => {
                self.compound_count += 1;
                workers::PlainNode::Object {
                    identity: node.identity,
                    length: node.length,
                }
            }
            9 => {
                self.compound_count += 1;
                workers::PlainNode::Array {
                    identity: node.identity,
                    length: node.length,
                }
            }
            _ => return None,
        })
    }

    fn child(&mut self, handle: u64, index: usize) -> Option<workers::PlainChild<u64>> {
        let mut child = WorkerPlainChild::default();
        // SAFETY: the callback borrows one field from the live, guarded container.
        if unsafe { (self.field)(self.context, handle, index, &mut child) } != 1 {
            return None;
        }
        let key = if child.key.is_null() {
            None
        } else {
            // SAFETY: copy before the next callback can reuse its scratch. The bounded cache
            // begins only once the graph is large enough to repeat keys across many objects.
            let view = unsafe { validate_string(&*child.key) }?;
            if self.compound_count < 16 {
                Some(StringUnits::from_view(view))
            } else if let Some(cached) = self.keys[..self.key_count]
                .iter()
                .flatten()
                .find(|key| key.as_view().unit_len() == view.unit_len() && key.as_view() == view)
            {
                Some(cached.clone())
            } else {
                let owned = StringUnits::from_view(view);
                if self.key_count < self.keys.len() {
                    self.keys[self.key_count] = Some(owned.clone());
                    self.key_count += 1;
                }
                Some(owned)
            }
        };
        Some(workers::PlainChild {
            key,
            value: (child.hole == 0).then_some(child.value),
        })
    }
}

/// A transferred `ArrayBuffer`'s block, owned by the message until a realm adopts it.
struct NativeBlock {
    pointer: *mut u8,
    length: usize,
}

// SAFETY: the block is plain `malloc` memory with one owner at a time: the realm that detached
// it gave it up before it was boxed, and only the realm that adopts it touches it afterwards.
unsafe impl Send for NativeBlock {}

impl Drop for NativeBlock {
    fn drop(&mut self) {
        if !self.pointer.is_null() {
            // SAFETY: the C runtime allocated the block with `malloc` and gave up ownership.
            unsafe { free(self.pointer.cast()) };
        }
    }
}

/// A `SharedArrayBuffer`'s store: `capacity` zeroed bytes that never move, of which `length`
/// are in the buffer (a growable one grows into the rest).
pub struct NativeShared {
    pointer: *mut u8,
    length: AtomicUsize,
    capacity: usize,
    growable: bool,
}

// SAFETY: the bytes are shared memory by definition; every access to them is either one of the
// language's unordered accesses (a data race the language permits) or an atomic operation.
unsafe impl Send for NativeShared {}
// SAFETY: as above.
unsafe impl Sync for NativeShared {}

impl Drop for NativeShared {
    fn drop(&mut self) {
        if !self.pointer.is_null() {
            // SAFETY: allocated with `calloc` in `kiln_shared_create`, freed once, by the last
            // holder.
            unsafe { free(self.pointer.cast()) };
        }
    }
}

fn ffi<F: FnOnce() -> i32>(body: F) -> i32 {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(body)).unwrap_or(INVALID_ARGUMENT)
}

/// The tokens the C runtime wrote for a message it is sending. A moved block and a shared store
/// are taken over (the token's handle is cleared); everything else is copied.
unsafe fn tokens_from_c(tokens: *mut WorkerToken, count: u64) -> Option<Vec<Token>> {
    let count = usize::try_from(count).ok()?;
    if count == 0 {
        return Some(Vec::new());
    }
    if tokens.is_null() {
        return None;
    }
    // SAFETY: the caller supplies `count` tokens, writable for this call.
    let tokens = unsafe { std::slice::from_raw_parts_mut(tokens, count) };
    let mut out = Vec::with_capacity(count);
    for token in tokens {
        out.push(match token.kind {
            TOKEN_UNDEFINED => Token::Undefined,
            TOKEN_NULL => Token::Null,
            TOKEN_BOOL => Token::Bool(token.flag != 0),
            TOKEN_NUMBER => Token::Number(token.number),
            TOKEN_STRING => {
                let view = KilnString {
                    unit_len: u32::try_from(token.len).ok()?,
                    flags: token.flag,
                    data: token.data,
                };
                // SAFETY: the C runtime passes a live string's units for this call.
                let view = unsafe { validate_string(&view) }?;
                Token::String(StringUnits::from_view(view))
            }
            TOKEN_MOVED => {
                let block = NativeBlock {
                    pointer: token.handle.cast(),
                    length: usize::try_from(token.len).ok()?,
                };
                token.handle = std::ptr::null_mut();
                Token::Moved(Box::new(block))
            }
            TOKEN_BYTES => {
                let length = usize::try_from(token.len).ok()?;
                let bytes: Arc<[u8]> = if length == 0 {
                    Arc::from(&[][..])
                } else if token.data.is_null() {
                    return None;
                } else {
                    // SAFETY: the C runtime passes `length` live bytes for this call.
                    Arc::from(unsafe {
                        std::slice::from_raw_parts(token.data.cast::<u8>(), length)
                    })
                };
                Token::Bytes(bytes)
            }
            TOKEN_SHARED => {
                if token.handle.is_null() {
                    return None;
                }
                let store = token.handle.cast::<NativeShared>().cast_const();
                // SAFETY: the handle is a strong reference the sending realm's buffer holds;
                // the message takes one of its own.
                let store = unsafe {
                    Arc::increment_strong_count(store);
                    Arc::from_raw(store)
                };
                Token::Shared(store)
            }
            _ => return None,
        });
    }
    Some(out)
}

/// A received message as the C runtime reads it: the tokens, and what keeps their storage alive
/// until [`kiln_worker_message_free`].
struct Received {
    tokens: Vec<WorkerToken>,
    _message: Vec<Token>,
}

fn token_to_c(token: &mut Token) -> WorkerToken {
    let mut out = WorkerToken {
        kind: TOKEN_UNDEFINED,
        flag: 0,
        number: 0.0,
        data: std::ptr::null(),
        len: 0,
        handle: std::ptr::null_mut(),
    };
    match token {
        Token::Undefined => {}
        Token::Null => out.kind = TOKEN_NULL,
        Token::Bool(value) => {
            out.kind = TOKEN_BOOL;
            out.flag = u32::from(*value);
        }
        Token::Number(value) => {
            out.kind = TOKEN_NUMBER;
            out.number = *value;
        }
        Token::String(text) => {
            out.kind = TOKEN_STRING;
            match text.as_view() {
                StringView::Latin1(bytes) => {
                    out.data = bytes.as_ptr().cast();
                    out.len = bytes.len() as u64;
                }
                StringView::Utf16(units) => {
                    out.flag = 1;
                    out.data = units.as_ptr().cast();
                    out.len = units.len() as u64;
                }
            }
        }
        Token::Moved(store) => {
            if let Some(block) = store.downcast_mut::<NativeBlock>() {
                out.kind = TOKEN_MOVED;
                out.handle = block.pointer.cast();
                out.len = block.length as u64;
                // The C runtime adopts the block, or hands it back through the token.
                block.pointer = std::ptr::null_mut();
            }
        }
        Token::Bytes(bytes) => {
            out.kind = TOKEN_BYTES;
            out.data = bytes.as_ptr().cast();
            out.len = bytes.len() as u64;
        }
        Token::Shared(store) => {
            if let Some(shared) = Arc::clone(store).downcast::<NativeShared>().ok() {
                out.kind = TOKEN_SHARED;
                out.flag = u32::from(shared.growable);
                out.len = shared.length.load(Ordering::Acquire) as u64;
                out.number = shared.capacity as f64;
                out.data = shared.pointer.cast_const().cast();
                out.handle = Arc::into_raw(shared).cast_mut().cast();
            }
        }
    }
    out
}

/// Post, queue or broadcast a message the C runtime flattened, or report the exception ending
/// this worker. 1 when a port received it, 0 when it was dropped, negative on a bad call.
///
/// # Safety
///
/// `tokens` must hold `count` writable tokens for the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_worker_post(
    op: u32,
    port: u64,
    kind: u32,
    tokens: *mut WorkerToken,
    count: u64,
) -> i32 {
    ffi(|| {
        // SAFETY: the caller's contract.
        let Some(tokens) = (unsafe { tokens_from_c(tokens, count) }) else {
            return INVALID_ARGUMENT;
        };
        let message = Message::new(kind, tokens);
        let delivered = match op {
            OP_POST => workers::post(port, message),
            OP_ENQUEUE => workers::enqueue(port, message),
            OP_BROADCAST_POST => workers::broadcast_post(port, message) != 0,
            OP_REPORT_ERROR => match workers::current() {
                Some(worker) => {
                    worker.report_error(message.tokens);
                    true
                }
                None => false,
            },
            _ => return INVALID_ARGUMENT,
        };
        i32::from(delivered)
    })
}

/// Encode and post a guarded plain graph through the shared Rust token builder. A zero answer
/// means the representation adapter declined the graph and the caller must use the full clone.
/// One means the graph was encoded (including when the endpoint has closed and drops it).
///
/// # Safety
///
/// `describe` and `field` must remain callable throughout this call, lend live descriptors, and
/// perform no JavaScript-visible effects. `root` and every child handle must remain rooted.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_worker_post_plain(
    port: u64,
    root: u64,
    context: *mut c_void,
    describe: WorkerPlainDescribe,
    field: WorkerPlainField,
) -> i32 {
    ffi(|| {
        let mut source = NativePlainSource {
            context,
            describe,
            field,
            compound_count: 0,
            keys: std::array::from_fn(|_| None),
            key_count: 0,
        };
        let Some(tokens) = workers::encode_plain(&mut source, root) else {
            return 0;
        };
        let _ = workers::post(port, Message::new(workers::kind::USER, tokens));
        1
    })
}

/// Take the oldest message queued for `port`: 1 with its kind and tokens, 0 when there is none.
/// The tokens stay valid until `kiln_worker_message_free(*out_message)`; a moved block or shared
/// store the C runtime adopts, it clears from its token first.
///
/// # Safety
///
/// Every out-pointer must be writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_worker_take(
    port: u64,
    out_kind: *mut u32,
    out_tokens: *mut *mut WorkerToken,
    out_count: *mut u64,
    out_message: *mut *mut c_void,
) -> i32 {
    ffi(|| {
        if out_kind.is_null()
            || out_tokens.is_null()
            || out_count.is_null()
            || out_message.is_null()
        {
            return INVALID_ARGUMENT;
        }
        let Some(mut message) = workers::take(port) else {
            return 0;
        };
        let tokens: Vec<WorkerToken> = message.tokens.iter_mut().map(token_to_c).collect();
        let mut received = Box::new(Received {
            tokens,
            _message: message.tokens,
        });
        // SAFETY: the out-pointers were checked above.
        unsafe {
            out_kind.write(message.kind);
            out_tokens.write(received.tokens.as_mut_ptr());
            out_count.write(received.tokens.len() as u64);
            out_message.write(Box::into_raw(received).cast());
        }
        1
    })
}

/// Release a message [`kiln_worker_take`] handed out, and whatever in it was not adopted.
///
/// # Safety
///
/// `message` must come from `kiln_worker_take` and be released once.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_worker_message_free(message: *mut c_void) {
    if message.is_null() {
        return;
    }
    // SAFETY: produced by `Box::into_raw` in `kiln_worker_take`.
    let received = unsafe { Box::from_raw(message.cast::<Received>()) };
    for token in &received.tokens {
        if token.handle.is_null() {
            continue;
        }
        match token.kind {
            // SAFETY: a block nobody adopted is still the message's.
            TOKEN_MOVED => unsafe { free(token.handle) },
            // SAFETY: the strong reference `token_to_c` made and nobody adopted.
            TOKEN_SHARED => unsafe {
                drop(Arc::from_raw(
                    token.handle.cast::<NativeShared>().cast_const(),
                ))
            },
            _ => {}
        }
    }
}

/// The scalar operations: a new channel (`out[0]`, `out[1]`), closing a port, a port's state,
/// terminating and forgetting a worker (by its runtime key); and process signals: a signal's
/// number by its portable index, subscribing (`out[0]` the endpoint, or 0 and the errno in
/// `out[1]`), giving a subscription up unhandled, `kill` (`a` is the pid in its low 32 bits and
/// the signal above them; `out[0]` the errno or 0), and delivering a signal (`out[0]` how many
/// realms took it).
///
/// # Safety
///
/// `out` must have room for two words when `op` writes any.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_worker_op(op: u32, a: u64, out: *mut u64) -> i32 {
    ffi(|| {
        let write = |index: usize, value: u64| {
            if !out.is_null() {
                // SAFETY: the caller gave room for two words.
                unsafe { out.add(index).write(value) };
            }
        };
        match op {
            OP_CHANNEL => {
                let (first, second) = workers::channel();
                write(0, first);
                write(1, second);
            }
            OP_CLOSE => workers::close(a),
            OP_STATE => write(0, u64::from(workers::state(a))),
            OP_TERMINATE => write(0, u64::from(workers::terminate(a))),
            OP_REAP => workers::reap(a),
            OP_SIGNAL_NUMBER => {
                let number = signals::number(u32::try_from(a).unwrap_or(u32::MAX));
                write(0, u64::try_from(number).unwrap_or(0));
            }
            OP_SIGNAL_LISTEN => match signals::listen(i32::try_from(a).unwrap_or(-1)) {
                Ok(endpoint) => {
                    write(0, endpoint);
                    write(1, 0);
                }
                Err(errno) => {
                    write(0, 0);
                    write(1, u64::try_from(errno).unwrap_or(0));
                }
            },
            OP_SIGNAL_UNHANDLED => signals::unhandled(a),
            OP_KILL => {
                let pid = a as u32 as i32;
                let signal = (a >> 32) as u32 as i32;
                write(0, u64::try_from(signals::kill(pid, signal)).unwrap_or(0));
            }
            OP_SIGNAL_DELIVER => {
                let delivered = signals::deliver(i32::try_from(a).unwrap_or(-1));
                write(0, delivered as u64);
            }
            _ => return INVALID_ARGUMENT,
        }
        0
    })
}

/// Join the broadcast group `name`; its new endpoint in `out`.
///
/// # Safety
///
/// `name` must be a live string view and `out` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_worker_broadcast_join(name: *const KilnString, out: *mut u64) -> i32 {
    ffi(|| {
        // SAFETY: the caller's contract.
        let Some(name) = (unsafe { validate_string(name) }) else {
            return INVALID_ARGUMENT;
        };
        if out.is_null() {
            return INVALID_ARGUMENT;
        }
        let name = StringUnits::from_view(name).to_lossy_string();
        // SAFETY: checked above.
        unsafe { out.write(workers::broadcast_join(&name)) };
        0
    })
}

/// What runs a module of the program as a worker's realm: the graph launcher's, on the worker's
/// thread. `entry` is the module's path, or source text when `eval` is nonzero; answers the exit
/// code.
pub type WorkerRunner = unsafe extern "C" fn(
    entry: *const u8,
    entry_len: u64,
    eval: i32,
    argc: u64,
    argv: *const *const c_char,
) -> i32;

static RUNNER: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());

/// Register the launcher's [`WorkerRunner`]. A program without one cannot start workers.
#[unsafe(no_mangle)]
pub extern "C" fn kiln_worker_set_runner(runner: Option<WorkerRunner>) -> i32 {
    let pointer = runner.map_or(std::ptr::null_mut(), |runner| runner as *mut c_void);
    RUNNER.store(pointer, Ordering::Release);
    0
}

fn string_of(view: *const KilnString) -> Option<String> {
    // SAFETY: the callers' contracts supply live views.
    let view = unsafe { validate_string(view) }?;
    Some(StringUnits::from_view(view).to_lossy_string())
}

/// Start a worker running `entry` (a module path, or source text when `eval` is nonzero) with
/// `argv` as its `process.argv`, its parent channel's worker end `port`, its control endpoint
/// and what its `node:worker_threads` reads about itself (`info`, JSON). Its runtime key and
/// `threadId` in `out_key` and `out_thread`. -2 when the program registered no runner.
///
/// # Safety
///
/// The views must be live for the call, `argv` must hold `argc` of them, and both out-pointers
/// must be writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_worker_spawn(
    entry: *const KilnString,
    eval: i32,
    argv: *const KilnString,
    argc: u64,
    port: u64,
    control: u64,
    info: *const KilnString,
    out_key: *mut u64,
    out_thread: *mut u64,
) -> i32 {
    ffi(|| {
        let runner = RUNNER.load(Ordering::Acquire);
        if runner.is_null() {
            return -2;
        }
        // SAFETY: registered by `kiln_worker_set_runner` from a `WorkerRunner`.
        let runner: WorkerRunner =
            unsafe { std::mem::transmute::<*mut c_void, WorkerRunner>(runner) };
        let (Some(entry), Some(info), Ok(argc)) =
            (string_of(entry), string_of(info), usize::try_from(argc))
        else {
            return INVALID_ARGUMENT;
        };
        if out_key.is_null() || out_thread.is_null() || (argc != 0 && argv.is_null()) {
            return INVALID_ARGUMENT;
        }
        let mut arguments = Vec::with_capacity(argc);
        for index in 0..argc {
            // SAFETY: `argv` holds `argc` live views.
            let Some(argument) = string_of(unsafe { argv.add(index) }) else {
                return INVALID_ARGUMENT;
            };
            arguments.push(argument);
        }
        let spec = WorkerSpec {
            entry: if eval != 0 {
                WorkerEntry::Eval(entry)
            } else {
                WorkerEntry::File(entry)
            },
            argv: arguments,
            info,
            port,
            control,
            pipe_stdio: true,
        };
        let started = workers::spawn(spec, move |worker| {
            let (text, eval) = match &worker.spec.entry {
                WorkerEntry::File(path) => (path.clone(), 0),
                WorkerEntry::Eval(source) => (source.clone(), 1),
            };
            let argv: Vec<std::ffi::CString> = worker
                .spec
                .argv
                .iter()
                .map(|argument| {
                    std::ffi::CString::new(argument.replace('\0', "")).unwrap_or_default()
                })
                .collect();
            let pointers: Vec<*const c_char> =
                argv.iter().map(|argument| argument.as_ptr()).collect();
            // SAFETY: the launcher's runner, called on the worker's own thread with buffers that
            // outlive the call.
            unsafe {
                runner(
                    text.as_ptr(),
                    text.len() as u64,
                    eval,
                    pointers.len() as u64,
                    pointers.as_ptr(),
                )
            }
        });
        match started {
            Ok((key, thread)) => {
                // SAFETY: checked above.
                unsafe {
                    out_key.write(key);
                    out_thread.write(thread);
                }
                0
            }
            Err(_) => super::RUNTIME_ERROR,
        }
    })
}

/// 1 inside a worker, with its `threadId`, parent-channel end, control endpoint and `info`
/// (valid while the worker runs); 0 on a main thread.
///
/// # Safety
///
/// Every out-pointer must be writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_worker_self(
    out_thread: *mut u64,
    out_port: *mut u64,
    out_control: *mut u64,
    out_info: *mut *const u8,
    out_info_len: *mut u64,
) -> i32 {
    ffi(|| {
        let Some(worker) = workers::current() else {
            return 0;
        };
        if out_thread.is_null()
            || out_port.is_null()
            || out_control.is_null()
            || out_info.is_null()
            || out_info_len.is_null()
        {
            return INVALID_ARGUMENT;
        }
        // SAFETY: checked above; `info` lives in the context, which outlives the realm.
        unsafe {
            out_thread.write(worker.thread_id);
            out_port.write(worker.spec.port);
            out_control.write(worker.spec.control);
            out_info.write(worker.spec.info.as_ptr());
            out_info_len.write(worker.spec.info.len() as u64);
        }
        1
    })
}

/// Text a worker wrote to its standard output (`stream` 1) or error (2), for its parent.
///
/// # Safety
///
/// `bytes` must hold `len` readable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_worker_stdio(stream: u32, bytes: *const u8, len: u64) -> i32 {
    ffi(|| {
        let Some(worker) = workers::current() else {
            return 0;
        };
        let Ok(len) = usize::try_from(len) else {
            return INVALID_ARGUMENT;
        };
        if len == 0 {
            return 0;
        }
        if bytes.is_null() {
            return INVALID_ARGUMENT;
        }
        // SAFETY: the caller's contract.
        let text = String::from_utf8_lossy(unsafe { std::slice::from_raw_parts(bytes, len) });
        worker.write_stdio(stream == 2, StringUnits::from(text.as_ref()));
        1
    })
}

/// `process.exit(code)`: in a worker, the realm unwinds and the launcher ends it with `code` --
/// 0 is answered and the caller unwinds. On a main thread, 1: the caller ends the process.
#[unsafe(no_mangle)]
pub extern "C" fn kiln_worker_exit(code: i32) -> i32 {
    if workers::request_exit(code) {
        // A realm parked while a connection's callback asked to end must see it now.
        super::realm::wake_active_drive();
        0
    } else {
        1
    }
}

/// The code a worker's realm asked to end with (`process.exit`, `terminate`): 1 and the code in
/// `out`, or 0 when it did not ask.
///
/// # Safety
///
/// `out` must be writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_worker_exit_code(out: *mut i32) -> i32 {
    if out.is_null() {
        return INVALID_ARGUMENT;
    }
    match workers::take_exit_request() {
        Some(code) => {
            // SAFETY: checked above.
            unsafe { out.write(code) };
            1
        }
        None => 0,
    }
}

/// A new `SharedArrayBuffer` store of `length` zeroed bytes that can grow to `capacity` (a fixed
/// one when `capacity` is `u64::MAX`): its bytes in `out_bytes` and a strong reference in
/// `out_handle`, which the buffer holding it releases with [`kiln_shared_release`].
///
/// # Safety
///
/// Both out-pointers must be writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_shared_create(
    length: u64,
    capacity: u64,
    out_bytes: *mut *mut u8,
    out_handle: *mut *mut c_void,
) -> i32 {
    ffi(|| {
        let growable = capacity != u64::MAX;
        let capacity = if growable { capacity } else { length };
        let (Ok(length), Ok(capacity)) = (usize::try_from(length), usize::try_from(capacity))
        else {
            return INVALID_ARGUMENT;
        };
        let capacity = capacity.max(length);
        if out_bytes.is_null() || out_handle.is_null() || capacity > isize::MAX as usize {
            return INVALID_ARGUMENT;
        }
        // SAFETY: a zeroed block; one byte at least so the pointer is never null.
        let pointer = unsafe { calloc(capacity.max(1), 1) }.cast::<u8>();
        if pointer.is_null() {
            return super::RUNTIME_ERROR;
        }
        let store = Arc::new(NativeShared {
            pointer,
            length: AtomicUsize::new(length),
            capacity,
            growable,
        });
        // SAFETY: checked above.
        unsafe {
            out_bytes.write(pointer);
            out_handle.write(Arc::into_raw(store).cast_mut().cast());
        }
        0
    })
}

/// Let go of a buffer's strong reference to its shared store.
///
/// # Safety
///
/// `handle` must be a strong reference from `kiln_shared_create` or a received message, released
/// once.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_shared_release(handle: *mut c_void) {
    if !handle.is_null() {
        // SAFETY: the caller's contract.
        drop(unsafe { Arc::from_raw(handle.cast::<NativeShared>().cast_const()) });
    }
}

/// Grow a growable store's length to `length` (never shrinks, never past its capacity): 0, or
/// -1 when it does not fit. The new length in `out`.
///
/// # Safety
///
/// `handle` must be a live strong reference and `out` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_shared_grow(handle: *mut c_void, length: u64, out: *mut u64) -> i32 {
    if handle.is_null() || out.is_null() {
        return INVALID_ARGUMENT;
    }
    // SAFETY: the caller's contract.
    let store = unsafe { &*handle.cast::<NativeShared>().cast_const() };
    let Ok(length) = usize::try_from(length) else {
        return INVALID_ARGUMENT;
    };
    if length > store.capacity {
        return INVALID_ARGUMENT;
    }
    let grown = store.length.fetch_max(length, Ordering::AcqRel).max(length);
    // SAFETY: checked above.
    unsafe { out.write(grown as u64) };
    0
}

/// `Atomics.wait` on the `width`-byte cell at `offset` of a shared store: 0 "ok", 1
/// "not-equal", 2 "timed-out", 3 when the waiting worker was terminated (the caller unwinds).
/// `timeout` in milliseconds; infinite or NaN waits for ever.
///
/// # Safety
///
/// `handle` must be a live strong reference and the cell inside its length.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_atomics_wait(
    handle: *mut c_void,
    offset: u64,
    width: u32,
    expected: i64,
    timeout: f64,
) -> i32 {
    ffi(|| {
        if handle.is_null() {
            return INVALID_ARGUMENT;
        }
        // SAFETY: the caller's contract.
        let store = unsafe { &*handle.cast::<NativeShared>().cast_const() };
        let Ok(offset) = usize::try_from(offset) else {
            return INVALID_ARGUMENT;
        };
        let size = if width == 8 { 8 } else { 4 };
        if offset
            .checked_add(size)
            .is_none_or(|end| end > store.capacity)
            || offset % size != 0
        {
            return INVALID_ARGUMENT;
        }
        // SAFETY: in bounds and aligned (the view's elements are), checked above.
        let cell = unsafe { store.pointer.add(offset) };
        let still_expected = || {
            if size == 8 {
                // SAFETY: an aligned, in-bounds 8-byte cell of shared memory.
                unsafe { AtomicI64::from_ptr(cell.cast()) }.load(Ordering::SeqCst) == expected
            } else {
                // SAFETY: an aligned, in-bounds 4-byte cell of shared memory.
                i64::from(unsafe { AtomicI32::from_ptr(cell.cast()) }.load(Ordering::SeqCst))
                    == expected
            }
        };
        let limit = (timeout.is_finite() && timeout >= 0.0)
            .then(|| std::time::Duration::from_secs_f64(timeout / 1_000.0));
        let key = (handle as usize, offset);
        match workers::wait(key, still_expected, limit) {
            workers::WaitOutcome::Ok => 0,
            workers::WaitOutcome::NotEqual => 1,
            workers::WaitOutcome::TimedOut => 2,
            workers::WaitOutcome::Interrupted => 3,
        }
    })
}

/// `Atomics.notify`: wake up to `count` waiters on the cell at `offset` of a shared store; how
/// many were woken.
///
/// # Safety
///
/// `handle` must be a live strong reference.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_atomics_notify(handle: *mut c_void, offset: u64, count: u64) -> u64 {
    if handle.is_null() {
        return 0;
    }
    let (Ok(offset), Ok(count)) = (usize::try_from(offset), usize::try_from(count)) else {
        return 0;
    };
    workers::notify((handle as usize, offset), count) as u64
}

macro_rules! assert_worker_export {
    ($(($group:ident, $name:ident, $signature:ident);)*) => {
        $(assert_worker_export!($group, $name, $signature);)*
    };
    (realm, kiln_worker_post, worker_post) => {
        const _: unsafe extern "C" fn(u32, u64, u32, *mut WorkerToken, u64) -> i32 = kiln_worker_post;
    };
    (realm, kiln_worker_post_plain, worker_post_plain) => {
        const _: unsafe extern "C" fn(u64, u64, *mut c_void, WorkerPlainDescribe, WorkerPlainField) -> i32 = kiln_worker_post_plain;
    };
    (realm, kiln_worker_take, worker_take) => {
        const _: unsafe extern "C" fn(u64, *mut u32, *mut *mut WorkerToken, *mut u64, *mut *mut c_void) -> i32 = kiln_worker_take;
    };
    (realm, kiln_worker_message_free, worker_message_free) => {
        const _: unsafe extern "C" fn(*mut c_void) = kiln_worker_message_free;
    };
    (realm, kiln_worker_op, worker_op) => {
        const _: unsafe extern "C" fn(u32, u64, *mut u64) -> i32 = kiln_worker_op;
    };
    (realm, kiln_worker_broadcast_join, worker_broadcast_join) => {
        const _: unsafe extern "C" fn(*const KilnString, *mut u64) -> i32 = kiln_worker_broadcast_join;
    };
    (realm, kiln_worker_set_runner, worker_set_runner) => {
        const _: extern "C" fn(Option<WorkerRunner>) -> i32 = kiln_worker_set_runner;
    };
    (realm, kiln_worker_spawn, worker_spawn) => {
        const _: unsafe extern "C" fn(*const KilnString, i32, *const KilnString, u64, u64, u64, *const KilnString, *mut u64, *mut u64) -> i32 = kiln_worker_spawn;
    };
    (realm, kiln_worker_self, worker_self) => {
        const _: unsafe extern "C" fn(*mut u64, *mut u64, *mut u64, *mut *const u8, *mut u64) -> i32 = kiln_worker_self;
    };
    (realm, kiln_worker_stdio, worker_stdio) => {
        const _: unsafe extern "C" fn(u32, *const u8, u64) -> i32 = kiln_worker_stdio;
    };
    (realm, kiln_worker_exit, worker_exit) => {
        const _: extern "C" fn(i32) -> i32 = kiln_worker_exit;
    };
    (realm, kiln_worker_exit_code, worker_exit_code) => {
        const _: unsafe extern "C" fn(*mut i32) -> i32 = kiln_worker_exit_code;
    };
    (realm, kiln_shared_create, shared_create) => {
        const _: unsafe extern "C" fn(u64, u64, *mut *mut u8, *mut *mut c_void) -> i32 = kiln_shared_create;
    };
    (realm, kiln_shared_release, shared_release) => {
        const _: unsafe extern "C" fn(*mut c_void) = kiln_shared_release;
    };
    (realm, kiln_shared_grow, shared_grow) => {
        const _: unsafe extern "C" fn(*mut c_void, u64, *mut u64) -> i32 = kiln_shared_grow;
    };
    (realm, kiln_atomics_wait, atomics_wait) => {
        const _: unsafe extern "C" fn(*mut c_void, u64, u32, i64, f64) -> i32 = kiln_atomics_wait;
    };
    (realm, kiln_atomics_notify, atomics_notify) => {
        const _: unsafe extern "C" fn(*mut c_void, u64, u64) -> u64 = kiln_atomics_notify;
    };
    ($group:ident, $name:ident, $signature:ident) => {};
}

kiln_native_provider_contract::provider_abi_registry!(assert_worker_export);

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn a_moved_block_and_a_shared_store_cross_a_channel_without_copying() {
        let mut ports = [0u64; 2];
        // SAFETY: room for two words.
        assert_eq!(
            unsafe { kiln_worker_op(OP_CHANNEL, 0, ports.as_mut_ptr()) },
            0
        );
        // SAFETY: plain allocations for the test.
        let block = unsafe { calloc(4, 1) }.cast::<u8>();
        let mut bytes = std::ptr::null_mut();
        let mut handle = std::ptr::null_mut();
        // SAFETY: writable out-pointers.
        assert_eq!(
            unsafe { kiln_shared_create(8, 8, &mut bytes, &mut handle) },
            0
        );
        let text = "héllo";
        let units: Vec<u16> = text.encode_utf16().collect();
        let mut tokens = [
            WorkerToken {
                kind: TOKEN_NUMBER,
                flag: 0,
                number: 4.5,
                data: std::ptr::null(),
                len: 0,
                handle: std::ptr::null_mut(),
            },
            WorkerToken {
                kind: TOKEN_STRING,
                flag: 1,
                number: 0.0,
                data: units.as_ptr().cast(),
                len: units.len() as u64,
                handle: std::ptr::null_mut(),
            },
            WorkerToken {
                kind: TOKEN_MOVED,
                flag: 0,
                number: 0.0,
                data: std::ptr::null(),
                len: 4,
                handle: block.cast(),
            },
            WorkerToken {
                kind: TOKEN_SHARED,
                flag: 0,
                number: 0.0,
                data: std::ptr::null(),
                len: 8,
                handle,
            },
        ];
        // SAFETY: four writable tokens.
        assert_eq!(
            unsafe { kiln_worker_post(OP_POST, ports[0], 0, tokens.as_mut_ptr(), 4) },
            1
        );
        assert!(tokens[2].handle.is_null(), "the message took the block");
        let (mut kind, mut out, mut count, mut message) =
            (9u32, std::ptr::null_mut(), 0u64, std::ptr::null_mut());
        // SAFETY: writable out-pointers.
        assert_eq!(
            unsafe { kiln_worker_take(ports[1], &mut kind, &mut out, &mut count, &mut message) },
            1
        );
        assert_eq!((kind, count), (0, 4));
        // SAFETY: `count` tokens valid until the free below.
        let received = unsafe { std::slice::from_raw_parts_mut(out, 4) };
        assert_eq!(received[0].number, 4.5);
        assert_eq!(received[1].flag, 1);
        assert_eq!(
            received[2].handle,
            block.cast(),
            "the same block, not a copy"
        );
        assert_eq!(
            received[3].data,
            bytes.cast_const().cast(),
            "the same shared bytes"
        );
        // SAFETY: from `kiln_worker_take`; the block and shared reference go back with it.
        unsafe { kiln_worker_message_free(message) };
        // SAFETY: the buffer's own reference.
        unsafe { kiln_shared_release(handle) };
        for port in ports {
            // SAFETY: no out-pointer needed.
            unsafe { kiln_worker_op(OP_CLOSE, port, std::ptr::null_mut()) };
        }
    }
}
