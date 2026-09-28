//! Interpreter facades for ADR 0073's sealed graphs. A facade belongs to one reading realm; the
//! backing node is realm-neutral. The cache holds weak entries so it never prolongs graph life.

use super::*;
use kiln_shared_graph::{
    Descriptor, Handle, InputValue, Inspector, Key, OpaqueHandle, Property as GraphProperty,
    Snapshot, SourceId, SourceNode, UnsupportedKind, Value as GraphValue,
};
use std::collections::HashMap;

#[derive(Default, Debug)]
pub(super) struct SharedCache {
    facades: HashMap<(RealmId, OpaqueHandle), Weak<SharedFacade>>,
}

#[derive(Debug)]
pub struct SharedFacade {
    pub(super) handle: Handle,
    pub(super) realm: RealmId,
    cache: Rc<RefCell<SharedCache>>,
}

impl SharedFacade {
    pub(super) fn obtain(
        handle: Handle,
        realm: RealmId,
        cache: &Rc<RefCell<SharedCache>>,
    ) -> Rc<Self> {
        let key = (realm, handle.opaque());
        if let Some(existing) = cache.borrow().facades.get(&key).and_then(Weak::upgrade) {
            return existing;
        }
        if cache.borrow().facades.len() > 1024 {
            cache
                .borrow_mut()
                .facades
                .retain(|_, value| value.strong_count() != 0);
        }
        let facade = Rc::new(Self {
            handle,
            realm,
            cache: Rc::clone(cache),
        });
        cache
            .borrow_mut()
            .facades
            .insert(key, Rc::downgrade(&facade));
        facade
    }

    pub(super) fn child(&self, handle: Handle) -> Rc<Self> {
        Self::obtain(handle, self.realm, &self.cache)
    }

    pub(super) fn value(&self, value: GraphValue) -> Value {
        match value {
            GraphValue::Undefined => Value::Undefined,
            GraphValue::Null => Value::Null,
            GraphValue::Boolean(flag) => Value::Boolean(flag),
            GraphValue::Number(number) => Value::Number(number),
            GraphValue::BigInt(text) => Value::BigInt(Rc::new(
                text.parse()
                    .expect("the sealed graph stored a validated BigInt"),
            )),
            GraphValue::String(text) => Value::String(text),
            GraphValue::Node(index) => Value::Shared(
                self.child(
                    self.handle
                        .child(index)
                        .expect("the sealed graph checked its local edge"),
                ),
            ),
            GraphValue::Shared(handle) => Value::Shared(self.child(handle)),
        }
    }

    pub(super) fn own_fields(&self) -> Vec<(StringUnits, Property, Attributes)> {
        let graph = self.handle.graph();
        let Some((_, _, _, count)) = graph.node_info(self.handle.index()) else {
            return Vec::new();
        };
        (0..count)
            .filter_map(|index| graph.field(self.handle.index(), index))
            .map(|field| {
                (
                    field.key,
                    Property::Value(self.value(field.value)),
                    Attributes {
                        writable: false,
                        enumerable: field.enumerable,
                        configurable: false,
                    },
                )
            })
            .collect()
    }

    pub(super) fn is_array(&self) -> bool {
        self.handle
            .graph()
            .node_info(self.handle.index())
            .is_some_and(|info| info.0)
    }

    pub(super) fn array_length(&self) -> Option<usize> {
        self.handle
            .graph()
            .node_info(self.handle.index())
            .and_then(|info| info.0.then_some(info.2 as usize))
    }

    pub(super) fn own_property(&self, key: PropertyKeyRef<'_>) -> Option<(Property, Attributes)> {
        // Sealed graphs contain only string keys. A Symbol whose private spelling happens to
        // match one must never read that string property.
        if key.is_symbol() {
            return None;
        }
        if key.matches_ascii("length")
            && let Some(length) = self.array_length()
        {
            return Some((
                Property::Value(Value::Number(length as f64)),
                Attributes::NONE,
            ));
        }
        let field = self
            .handle
            .graph()
            .find_field(self.handle.index(), &key.owned())?;
        Some((
            Property::Value(self.value(field.value)),
            Attributes {
                writable: false,
                enumerable: field.enumerable,
                configurable: false,
            },
        ))
    }

    pub(super) fn own_keys(&self) -> Vec<StringUnits> {
        let mut keys: Vec<_> = self
            .own_fields()
            .into_iter()
            .map(|(key, _, _)| key)
            .collect();
        if self.is_array() {
            keys.push("length".into());
        }
        keys
    }

    pub(super) fn prototype(&self) -> Value {
        match self.handle.graph().node_info(self.handle.index()) {
            Some((_, true, _, _)) => Value::Null,
            Some((true, _, _, _)) => {
                Value::Native(Native::PrototypeOf(PrototypeTable::Array), self.realm)
            }
            _ => Value::Native(Native::PrototypeOf(PrototypeTable::Object), self.realm),
        }
    }
}

struct InterpreterInspector {
    values: HashMap<SourceId, Value>,
    identities: HashMap<(u8, usize), SourceId>,
    next: SourceId,
    realm: RealmId,
    intrinsics: Rc<Intrinsics>,
}

impl InterpreterInspector {
    fn new(realm: RealmId, intrinsics: Rc<Intrinsics>) -> Self {
        Self {
            values: HashMap::new(),
            identities: HashMap::new(),
            next: 1,
            realm,
            intrinsics,
        }
    }

    fn input(&mut self, value: Value) -> InputValue {
        match value {
            Value::Undefined => InputValue::Undefined,
            Value::Null => InputValue::Null,
            Value::Boolean(flag) => InputValue::Boolean(flag),
            Value::Number(number) => InputValue::Number(number),
            Value::String(text) => InputValue::String(text),
            Value::BigInt(number) => InputValue::BigInt(number.to_string()),
            Value::Shared(facade) => InputValue::Shared(facade.handle.clone()),
            other => {
                let identity = match &other {
                    Value::Object(object) => Some((0, Rc::as_ptr(object) as usize)),
                    Value::Array(array) => Some((1, Rc::as_ptr(array) as usize)),
                    _ => None,
                };
                if let Some(id) = identity.and_then(|key| self.identities.get(&key).copied()) {
                    return InputValue::Object(id);
                }
                let id = self.next;
                self.next = self
                    .next
                    .checked_add(1)
                    .expect("snapshot object ID overflow");
                if let Some(identity) = identity {
                    self.identities.insert(identity, id);
                }
                self.values.insert(id, other);
                InputValue::Object(id)
            }
        }
    }

    fn properties(&mut self, properties: Vec<Slot>) -> Vec<GraphProperty> {
        properties
            .into_iter()
            .map(|slot| GraphProperty {
                key: if slot.symbol {
                    Key::Symbol
                } else {
                    Key::String(slot.key)
                },
                descriptor: match slot.property {
                    Property::Value(value) => Descriptor::Data(self.input(value)),
                    Property::Accessor { .. } => Descriptor::Accessor,
                },
                enumerable: slot.attributes.enumerable,
            })
            .collect()
    }

    fn plain_store(&self, object: &ObjectData, array: bool) -> Result<bool, UnsupportedKind> {
        if object.error_brand
            || object.arguments_brand
            || object.module_namespace
            || !object.namespace_cells.is_empty()
            || object.primitive.is_some()
            || object.node_events.is_some()
            || object.node_object.is_some()
            || object.buffer_brand
            || object.shadow_realm.is_some()
            || object.weak_shared_target.is_some()
            || !object.finalization_cells.is_empty()
        {
            return Err(UnsupportedKind::InternalSlots);
        }
        match &object.prototype {
            None => Ok(false),
            Some(Value::Native(Native::PrototypeOf(table), realm))
                if *realm == self.realm
                    && *table
                        == if array {
                            PrototypeTable::Array
                        } else {
                            PrototypeTable::Object
                        } =>
            {
                Ok(false)
            }
            Some(Value::Null) if !array => Ok(true),
            _ => Err(UnsupportedKind::CustomPrototype),
        }
    }
}

impl Inspector for InterpreterInspector {
    type Error = &'static str;

    fn inspect(&mut self, id: SourceId) -> Result<SourceNode, Self::Error> {
        let value = self
            .values
            .get(&id)
            .cloned()
            .ok_or("snapshot source disappeared")?;
        Ok(match value {
            Value::Object(object) => {
                let object = object.borrow();
                match self.plain_store(&object, false) {
                    Ok(null_prototype) => SourceNode::Record {
                        null_prototype,
                        properties: self.properties(object.properties.clone()),
                    },
                    Err(kind) => SourceNode::Unsupported(kind),
                }
            }
            Value::Array(array) => {
                let store = array.properties.borrow();
                if let Err(kind) = self.plain_store(&store, true) {
                    SourceNode::Unsupported(kind)
                } else {
                    drop(store);
                    let mut properties = Vec::new();
                    for key in
                        own_property_keys_units(&self.intrinsics, &Value::Array(array.clone()))
                    {
                        if key == "length" {
                            continue;
                        }
                        if let Some((property, attributes)) = own_property_state_key(
                            &self.intrinsics,
                            &Value::Array(array.clone()),
                            PropertyKeyRef::Units(&key),
                        ) {
                            properties.push(Slot {
                                key,
                                symbol: false,
                                property,
                                attributes,
                            });
                        }
                    }
                    properties.extend(
                        array
                            .properties
                            .borrow()
                            .properties
                            .iter()
                            .filter(|slot| slot.symbol)
                            .cloned(),
                    );
                    SourceNode::Array {
                        length: u32::try_from(array.length()).map_err(|_| "array too long")?,
                        properties: self.properties(properties),
                    }
                }
            }
            Value::Proxy(_) => SourceNode::Unsupported(UnsupportedKind::Proxy),
            Value::Symbol(_) => SourceNode::Unsupported(UnsupportedKind::Symbol),
            Value::Function(_)
            | Value::Native(..)
            | Value::Callback(..)
            | Value::Bound(_)
            | Value::Wrapped(_)
            | Value::CommonJsRequire(_)
            | Value::CommonJsResolve(_) => SourceNode::Unsupported(UnsupportedKind::Function),
            _ => SourceNode::Unsupported(UnsupportedKind::InternalSlots),
        })
    }
}

impl<H: HostOps + ?Sized> Vm<'_, H> {
    pub(super) fn share_value(&mut self, value: Value) -> Result<Value, ExecutionError> {
        if matches!(value, Value::Shared(_)) {
            return Ok(value);
        }
        let mut inspector =
            InterpreterInspector::new(self.current_realm, Rc::clone(&self.intrinsics));
        let input = inspector.input(value.clone());
        match kiln_shared_graph::share(input, &mut inspector) {
            Ok(Snapshot::Primitive(_)) => Ok(value),
            Ok(Snapshot::Shared(handle)) => Ok(Value::Shared(SharedFacade::obtain(
                handle,
                self.current_realm,
                &self.shared_facades,
            ))),
            Err(error) => Err(ExecutionError::TypeErrorMessage {
                message: error.to_string(),
            }),
        }
    }
}
