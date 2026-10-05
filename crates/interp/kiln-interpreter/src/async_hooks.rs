//! `node:async_hooks` implementation.
//!
//! Provides `AsyncLocalStorage` and `AsyncResource` tracking and state.

use std::cell::Cell;
use std::rc::Rc;

use crate::{ExecutionError, Native, NodeObject, ObjectData, RealmId, Value, tracked_object};

#[derive(Debug)]
pub(crate) struct AsyncStorage {
    pub(crate) disabled: Cell<bool>,
}

/// Two storages are equal only when they are the same one.
///
/// Written out rather than derived for the reason [`Closure`]'s is: `disabled` is state, not
/// identity, and a derive over it would make any two untouched storages compare equal.
impl PartialEq for AsyncStorage {
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self, other)
    }
}

/// Every `AsyncLocalStorage` that currently holds a store, and what it holds.
///
/// Shared rather than copied: this is captured every time a job is queued -- once per promise
/// reaction, which is the hottest scheduling path there is -- and is empty in every program that
/// never asks for a storage, which is nearly all of them. A `Vec` rather than a map because it
/// is searched by pointer identity and is almost always empty or a single entry.
pub(crate) type AsyncContext = Rc<Vec<(Rc<AsyncStorage>, Value)>>;

/// `new AsyncLocalStorage()`.
///
/// The instance is an ordinary object carrying its own identity in `node_object`; the methods are
/// shared natives that read that identity off their receiver, exactly as `StringDecoder`'s do.
pub(crate) fn node_async_local_storage() -> Value {
    let mut data = ObjectData {
        node_object: Some(NodeObject::AsyncLocalStorage(Rc::new(AsyncStorage {
            disabled: Cell::new(false),
        }))),
        ..Default::default()
    };
    data.set(
        "run",
        Value::Native(Native::NodeAsyncLocalStorageRun, RealmId::PRIMARY),
    );
    data.set(
        "getStore",
        Value::Native(Native::NodeAsyncLocalStorageGetStore, RealmId::PRIMARY),
    );
    data.set(
        "enterWith",
        Value::Native(Native::NodeAsyncLocalStorageEnterWith, RealmId::PRIMARY),
    );
    data.set(
        "exit",
        Value::Native(Native::NodeAsyncLocalStorageExit, RealmId::PRIMARY),
    );
    data.set(
        "disable",
        Value::Native(Native::NodeAsyncLocalStorageDisable, RealmId::PRIMARY),
    );
    Value::Object(tracked_object(data))
}

/// A context with one storage's binding replaced, or removed when `store` is `None`.
///
/// A fresh context rather than a mutation, because contexts already captured by queued work must
/// not change underneath it: that captured copy *is* the store those jobs will see.
pub(crate) fn bind_store(
    context: &AsyncContext,
    storage: &Rc<AsyncStorage>,
    store: Option<Value>,
) -> AsyncContext {
    let mut bindings: Vec<(Rc<AsyncStorage>, Value)> = context
        .iter()
        .filter(|(held, _)| !Rc::ptr_eq(held, storage))
        .cloned()
        .collect();
    if let Some(store) = store {
        bindings.push((Rc::clone(storage), store));
    }
    Rc::new(bindings)
}

/// The storage a method was called on, or a `TypeError` naming the mistake.
pub(crate) fn node_async_storage(receiver: &Value) -> Result<Rc<AsyncStorage>, ExecutionError> {
    if let Value::Object(object) = receiver
        && let Some(NodeObject::AsyncLocalStorage(storage)) = &object.borrow().node_object
    {
        return Ok(Rc::clone(storage));
    }
    Err(ExecutionError::TypeErrorMessage {
        message: "AsyncLocalStorage method called on an incompatible receiver".into(),
    })
}

pub(crate) fn property(prop: &str) -> Option<Value> {
    match prop {
        "AsyncResource" => Some(Value::Native(Native::NodeAsyncResource, RealmId::PRIMARY)),
        "AsyncLocalStorage" => Some(Value::Native(
            Native::NodeAsyncLocalStorage,
            RealmId::PRIMARY,
        )),
        "default" => Some(Value::Native(Native::NodeAsyncHooks, RealmId::PRIMARY)),
        _ => None,
    }
}

pub(crate) const fn js_name(native: Native) -> Option<&'static str> {
    match native {
        Native::NodeAsyncHooks => Some("async_hooks"),
        Native::NodeAsyncResource => Some("AsyncResource"),
        Native::NodeAsyncResourceBind => Some("bind"),
        Native::NodeAsyncResourceEmitDestroy => Some("emitDestroy"),
        Native::NodeAsyncLocalStorage => Some("AsyncLocalStorage"),
        Native::NodeAsyncLocalStorageRun => Some("run"),
        Native::NodeAsyncLocalStorageGetStore => Some("getStore"),
        Native::NodeAsyncLocalStorageEnterWith => Some("enterWith"),
        Native::NodeAsyncLocalStorageExit => Some("exit"),
        Native::NodeAsyncLocalStorageDisable => Some("disable"),
        _ => None,
    }
}
