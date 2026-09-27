//! `node:stream` implementation.
//!
//! Provides the Node Readable and Writable stream abstractions and state tracking.

use std::cell::RefCell;
use std::rc::Rc;

use crate::intrinsic_members::NamedMembers;
use crate::{
    ExecutionError, Native, NodeObject, ObjectData, PrototypeTable, RealmId, Value, is_callable,
    own_data_property,
};

#[derive(Debug)]
pub(crate) struct ReadableState {
    pub(crate) buffer: Vec<Value>,
    pub(crate) buffered_bytes: usize,
    pub(crate) high_water_mark: usize,
    pub(crate) ended: bool,
    pub(crate) end_emitted: bool,
    pub(crate) reading: bool,
    pub(crate) flowing: bool,
    pub(crate) destroyed: bool,
    pub(crate) encoding: Option<String>,
    pub(crate) pipes: Vec<Value>,
    pub(crate) public: Rc<RefCell<ObjectData>>,
}

#[derive(Debug)]
pub(crate) struct WritableState {
    pub(crate) ended: bool,
    pub(crate) finished: bool,
    pub(crate) destroyed: bool,
    pub(crate) write: Option<Value>,
}

pub(crate) fn new_readable_state(
    high_water_mark: usize,
    encoding: Option<String>,
) -> Rc<RefCell<ReadableState>> {
    let public = Rc::new(RefCell::new(ObjectData::default()));
    Rc::new(RefCell::new(ReadableState {
        buffer: Vec::new(),
        buffered_bytes: 0,
        high_water_mark,
        ended: false,
        end_emitted: false,
        reading: false,
        flowing: false,
        destroyed: false,
        encoding,
        pipes: Vec::new(),
        public,
    }))
}

pub(crate) fn node_readable(arguments: &[Value], receiver: Option<&Value>) -> Value {
    let options = arguments.first().cloned().unwrap_or(Value::Undefined);
    let high_water_mark = own_data_property(&options, "highWaterMark")
        .map(|value| value.as_number().max(0.0) as usize)
        .unwrap_or(16 * 1024);
    let encoding = own_data_property(&options, "encoding")
        .filter(|value| !matches!(value, Value::Undefined))
        .map(|value| value.to_display_string().to_ascii_lowercase());
    let state = new_readable_state(high_water_mark, encoding);
    let public = Rc::clone(&state.borrow().public);
    let initializes_existing = matches!(receiver, Some(Value::Object(_)));
    let object = match receiver {
        Some(Value::Object(object)) => Rc::clone(object),
        _ => Rc::new(RefCell::new(ObjectData::default())),
    };
    {
        let mut data = object.borrow_mut();
        data.node_events.get_or_insert_with(Vec::new);
        data.node_object = Some(NodeObject::Readable(Rc::clone(&state)));
        if !initializes_existing {
            data.set_prototype(Value::Native(
                Native::PrototypeOf(PrototypeTable::Readable),
                RealmId::PRIMARY,
            ));
        }
        data.set_non_enumerable("_readableState", Value::Object(public));
        data.set_non_enumerable("readableEnded", Value::Boolean(false));
        data.set_non_enumerable("destroyed", Value::Boolean(false));
    }
    sync_readable_public_state(&Value::Object(Rc::clone(&object)), &state.borrow());
    Value::Object(object)
}

pub(crate) fn node_readable_state(
    receiver: &Value,
) -> Result<Rc<RefCell<ReadableState>>, ExecutionError> {
    let Value::Object(object) = receiver else {
        return Err(ExecutionError::TypeErrorMessage {
            message: "Readable method called on an incompatible receiver".into(),
        });
    };
    match &object.borrow().node_object {
        Some(NodeObject::Readable(state)) => Ok(Rc::clone(state)),
        Some(NodeObject::Zlib(state)) => Ok(Rc::clone(&state.readable)),
        _ => Err(ExecutionError::TypeErrorMessage {
            message: "Readable method called on an incompatible receiver".into(),
        }),
    }
}

pub(crate) fn sync_readable_public_state(receiver: &Value, state: &ReadableState) {
    {
        let mut public = state.public.borrow_mut();
        public.set("reading", Value::Boolean(state.reading));
        public.set("ended", Value::Boolean(state.ended));
        public.set("endEmitted", Value::Boolean(state.end_emitted));
        public.set("flowing", Value::Boolean(state.flowing));
        public.set("length", Value::Number(state.buffered_bytes as f64));
        public.set("highWaterMark", Value::Number(state.high_water_mark as f64));
    }
    if let Value::Object(object) = receiver {
        let mut object = object.borrow_mut();
        object.set_non_enumerable("readableEnded", Value::Boolean(state.end_emitted));
        object.set_non_enumerable("destroyed", Value::Boolean(state.destroyed));
    }
}

pub(crate) fn node_writable(arguments: &[Value], receiver: Option<&Value>) -> Value {
    let options = arguments.first().cloned().unwrap_or(Value::Undefined);
    let write = own_data_property(&options, "write").filter(is_callable);
    let initializes_existing = matches!(receiver, Some(Value::Object(_)));
    let object = match receiver {
        Some(Value::Object(object)) => Rc::clone(object),
        _ => Rc::new(RefCell::new(ObjectData::default())),
    };
    let state = Rc::new(RefCell::new(WritableState {
        ended: false,
        finished: false,
        destroyed: false,
        write,
    }));
    {
        let mut data = object.borrow_mut();
        data.node_events.get_or_insert_with(Vec::new);
        data.node_object = Some(NodeObject::Writable(state));
        if !initializes_existing {
            data.set_prototype(Value::Native(
                Native::PrototypeOf(PrototypeTable::Writable),
                RealmId::PRIMARY,
            ));
        }
        data.set_non_enumerable("writableEnded", Value::Boolean(false));
        data.set_non_enumerable("writableFinished", Value::Boolean(false));
        data.set_non_enumerable("destroyed", Value::Boolean(false));
    }
    Value::Object(object)
}

pub(crate) fn node_writable_state(
    receiver: &Value,
) -> Result<Rc<RefCell<WritableState>>, ExecutionError> {
    let Value::Object(object) = receiver else {
        return Err(ExecutionError::TypeErrorMessage {
            message: "Writable method called on an incompatible receiver".into(),
        });
    };
    match &object.borrow().node_object {
        Some(NodeObject::Writable(state)) => Ok(Rc::clone(state)),
        Some(NodeObject::Zlib(state)) => Ok(Rc::clone(&state.writable)),
        _ => Err(ExecutionError::TypeErrorMessage {
            message: "Writable method called on an incompatible receiver".into(),
        }),
    }
}

pub(crate) fn sync_writable_public_state(receiver: &Value, state: &WritableState) {
    if let Value::Object(object) = receiver {
        let mut object = object.borrow_mut();
        object.set_non_enumerable("writableEnded", Value::Boolean(state.ended));
        object.set_non_enumerable("writableFinished", Value::Boolean(state.finished));
        object.set_non_enumerable("destroyed", Value::Boolean(state.destroyed));
    }
}

pub(crate) fn property(prop: &str) -> Option<Value> {
    match prop {
        "Readable" => Some(Value::Native(Native::NodeReadable, RealmId::PRIMARY)),
        "Writable" => Some(Value::Native(Native::NodeWritable, RealmId::PRIMARY)),
        "addAbortSignal" => Some(Value::Native(
            Native::NodeStreamAddAbortSignal,
            RealmId::PRIMARY,
        )),
        "finished" => Some(Value::Native(Native::NodeStreamFinished, RealmId::PRIMARY)),
        "default" => Some(Value::Native(Native::NodeStream, RealmId::PRIMARY)),
        _ => None,
    }
}

pub(crate) fn readable_constructor_property(prop: &str) -> Option<Value> {
    match prop {
        "prototype" => Some(Value::Native(
            Native::PrototypeOf(PrototypeTable::Readable),
            RealmId::PRIMARY,
        )),
        "Readable" | "default" => Some(Value::Native(Native::NodeReadable, RealmId::PRIMARY)),
        _ => None,
    }
}

macro_rules! readable_prototype_members {
    ($(($name:literal, $native:ident)),* $(,)?) => {
        // One declaration feeds both marker lookup and materialized own-key enumeration.
        const READABLE_PROTOTYPE_MEMBERS: NamedMembers =
            NamedMembers::new(&[$($name),*]);

        pub(crate) const fn readable_prototype_member_names() -> &'static [&'static str] {
            READABLE_PROTOTYPE_MEMBERS.names()
        }

        pub(crate) fn readable_prototype_property(prop: &str) -> Option<Value> {
            match prop {
                "constructor" => Some(Value::Native(Native::NodeReadable, RealmId::PRIMARY)),
                $($name => Some(Value::Native(Native::$native, RealmId::PRIMARY)),)*
                _ => None,
            }
        }
    };
}

readable_prototype_members! {
    ("push", NodeReadablePush),
    ("read", NodeReadableRead),
    ("destroy", NodeReadableDestroy),
    ("pause", NodeReadablePause),
    ("resume", NodeReadableResume),
    ("setEncoding", NodeReadableSetEncoding),
    ("pipe", NodeReadablePipe),
    ("unpipe", NodeReadableUnpipe),
    // Preserve the interpreter's existing Readable-own EventEmitter surface.
    ("on", NodeEventOn),
    ("addListener", NodeEventOn),
    ("once", NodeEventOnce),
    ("emit", NodeEventEmit),
    ("listeners", NodeEventListeners),
    ("eventNames", NodeEventNames),
    ("removeAllListeners", NodeEventRemoveAllListeners),
    ("setMaxListeners", NodeEventSetMaxListeners),
}

pub(crate) fn writable_constructor_property(prop: &str) -> Option<Value> {
    match prop {
        "prototype" => Some(Value::Native(
            Native::PrototypeOf(PrototypeTable::Writable),
            RealmId::PRIMARY,
        )),
        "Writable" | "default" => Some(Value::Native(Native::NodeWritable, RealmId::PRIMARY)),
        _ => None,
    }
}

pub(crate) fn writable_prototype_property(prop: &str) -> Option<Value> {
    match prop {
        "constructor" => Some(Value::Native(Native::NodeWritable, RealmId::PRIMARY)),
        "write" => Some(Value::Native(Native::NodeWritableWrite, RealmId::PRIMARY)),
        "end" => Some(Value::Native(Native::NodeWritableEnd, RealmId::PRIMARY)),
        "destroy" => Some(Value::Native(Native::NodeWritableDestroy, RealmId::PRIMARY)),
        _ => None,
    }
}

pub(crate) const fn js_name(native: Native) -> Option<&'static str> {
    match native {
        Native::NodeStream => Some("stream"),
        Native::NodeReadable => Some("Readable"),
        Native::NodeWritable => Some("Writable"),
        Native::NodeReadablePush => Some("push"),
        Native::NodeReadableRead => Some("read"),
        Native::NodeReadableDestroy => Some("destroy"),
        Native::NodeReadablePause => Some("pause"),
        Native::NodeReadableResume => Some("resume"),
        Native::NodeReadableSetEncoding => Some("setEncoding"),
        Native::NodeReadablePipe => Some("pipe"),
        Native::NodeReadableUnpipe => Some("unpipe"),
        Native::NodeStreamAddAbortSignal => Some("addAbortSignal"),
        Native::NodeStreamFinished => Some("finished"),
        Native::NodeWritableWrite => Some("write"),
        Native::NodeWritableEnd => Some("end"),
        Native::NodeWritableDestroy => Some("destroy"),
        _ => None,
    }
}
