//! `node:events` and EventEmitter implementation.
//!
//! Provides the Node EventEmitter object constructor, listener tracking, and property lookups.

use std::cell::RefCell;
use std::rc::Rc;

use crate::{Native, ObjectData, PrototypeTable, RealmId, Value, same_value};

#[derive(Clone, Debug)]
pub(crate) struct NodeEventBucket {
    pub(crate) name: String,
    pub(crate) listeners: Vec<NodeEventListener>,
}

#[derive(Clone, Debug)]
pub(crate) struct NodeEventListener {
    pub(crate) callback: Value,
    pub(crate) once: bool,
}

pub(crate) fn node_event_emitter() -> Value {
    let mut data = ObjectData::default();
    data.set_prototype(Value::Native(
        Native::PrototypeOf(PrototypeTable::EventEmitter),
        RealmId::PRIMARY,
    ));
    data.node_events = Some(Vec::new());
    Value::Object(Rc::new(RefCell::new(data)))
}

pub(crate) fn node_listeners(object: &RefCell<ObjectData>, event: &str) -> Vec<NodeEventListener> {
    object
        .borrow()
        .node_events
        .as_ref()
        .and_then(|buckets| buckets.iter().find(|bucket| bucket.name == event))
        .map(|bucket| bucket.listeners.clone())
        .unwrap_or_default()
}

pub(crate) fn remove_node_listener(object: &RefCell<ObjectData>, event: &str, listener: &Value) {
    let mut data = object.borrow_mut();
    let Some(buckets) = data.node_events.as_mut() else {
        return;
    };
    let Some(bucket) = buckets.iter_mut().find(|bucket| bucket.name == event) else {
        return;
    };
    if let Some(index) = bucket
        .listeners
        .iter()
        .position(|registered| same_value(&registered.callback, listener))
    {
        bucket.listeners.remove(index);
    }
}

pub(crate) fn remove_last_node_listener(
    object: &RefCell<ObjectData>,
    event: &str,
    listener: &Value,
) {
    let mut data = object.borrow_mut();
    let Some(buckets) = data.node_events.as_mut() else {
        return;
    };
    let Some(bucket) = buckets.iter_mut().find(|bucket| bucket.name == event) else {
        return;
    };
    if let Some(index) = bucket
        .listeners
        .iter()
        .rposition(|registered| same_value(&registered.callback, listener))
    {
        bucket.listeners.remove(index);
    }
    if bucket.listeners.is_empty() {
        buckets.retain(|bucket| bucket.name != event);
    }
}

pub(crate) fn property(prop: &str) -> Option<Value> {
    match prop {
        "EventEmitter" | "default" => {
            Some(Value::Native(Native::NodeEventEmitter, RealmId::PRIMARY))
        }
        _ => None,
    }
}

pub(crate) fn event_emitter_property(prop: &str) -> Option<Value> {
    match prop {
        "prototype" => Some(Value::Native(
            Native::PrototypeOf(PrototypeTable::EventEmitter),
            RealmId::PRIMARY,
        )),
        "EventEmitter" | "default" => {
            Some(Value::Native(Native::NodeEventEmitter, RealmId::PRIMARY))
        }
        _ => None,
    }
}

pub(crate) fn prototype_property(prop: &str) -> Option<Value> {
    match prop {
        "constructor" => Some(Value::Native(Native::NodeEventEmitter, RealmId::PRIMARY)),
        "on" | "addListener" => Some(Value::Native(Native::NodeEventOn, RealmId::PRIMARY)),
        "once" => Some(Value::Native(Native::NodeEventOnce, RealmId::PRIMARY)),
        "emit" => Some(Value::Native(Native::NodeEventEmit, RealmId::PRIMARY)),
        "listeners" => Some(Value::Native(Native::NodeEventListeners, RealmId::PRIMARY)),
        "eventNames" => Some(Value::Native(Native::NodeEventNames, RealmId::PRIMARY)),
        "removeListener" | "off" => Some(Value::Native(
            Native::NodeEventRemoveListener,
            RealmId::PRIMARY,
        )),
        "removeAllListeners" => Some(Value::Native(
            Native::NodeEventRemoveAllListeners,
            RealmId::PRIMARY,
        )),
        "setMaxListeners" => Some(Value::Native(
            Native::NodeEventSetMaxListeners,
            RealmId::PRIMARY,
        )),
        _ => None,
    }
}

pub(crate) const fn js_name(native: Native) -> Option<&'static str> {
    match native {
        Native::NodeEvents => Some("events"),
        Native::NodeEventEmitter => Some("EventEmitter"),
        Native::NodeEventOn => Some("on"),
        Native::NodeEventOnce => Some("once"),
        Native::NodeEventEmit => Some("emit"),
        Native::NodeEventListeners => Some("listeners"),
        Native::NodeEventNames => Some("eventNames"),
        Native::NodeEventRemoveListener => Some("removeListener"),
        Native::NodeEventRemoveAllListeners => Some("removeAllListeners"),
        Native::NodeEventSetMaxListeners => Some("setMaxListeners"),
        _ => None,
    }
}
