//! `node:net` implementation.
//!
//! Provides TCP server and socket state tracking and methods.

use std::cell::RefCell;
use std::rc::{Rc, Weak};

use kiln_runtime::node_net::TcpServerDriver;
use kiln_runtime::node_stream::StreamDriver;

use crate::{
    ExecutionError, Native, NodeObject, ObjectData, RealmId, Value, is_callable, own_data_property,
};

#[derive(Debug)]
pub(crate) struct NetSocketState {
    pub(crate) owner: Option<Weak<RefCell<NodeServerState>>>,
    pub(crate) resource: Option<kiln_host_abi::ResourceId>,
    pub(crate) object: Option<Weak<RefCell<ObjectData>>>,
    pub(crate) readable: Rc<RefCell<crate::ReadableState>>,
}

#[derive(Debug)]
pub(crate) struct NodeServerState {
    pub(crate) driver: TcpServerDriver,
    pub(crate) handler: Value,
    pub(crate) protocol: ServerProtocol,
    pub(crate) timeout_milliseconds: f64,
    pub(crate) streams: Vec<NodeConnection>,
    pub(crate) object: Option<Weak<RefCell<ObjectData>>>,
    /// A fetch-style (`serve`) server: its connections take requests whole and its handler is
    /// called with strings and bytes instead of an IncomingMessage and a ServerResponse.
    pub(crate) fetch: bool,
    /// The listener has closed while connections were still open: node's `close` event (and the
    /// `server.close(callback)` callback) waits for the last of them to end.
    pub(crate) close_deferred: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ServerProtocol {
    Net,
    Http,
    HttpClient,
}

#[derive(Debug)]
pub(crate) struct NodeConnection {
    pub(crate) driver: StreamDriver,
    pub(crate) runtime_http: bool,
    pub(crate) upgrade_request_id: Option<u64>,
    pub(crate) upgrading: bool,
    pub(crate) socket: Option<Rc<RefCell<NetSocketState>>>,
    pub(crate) socket_object: Option<Value>,
    pub(crate) request: Option<Value>,
    pub(crate) response: Option<Value>,
    pub(crate) client_requests: std::collections::BTreeMap<u64, Value>,
    pub(crate) client_responses: std::collections::BTreeMap<u64, Value>,
    pub(crate) http_responses: Vec<(u64, Value)>,
    pub(crate) close_when_response_ends: bool,
    pub(crate) closed: bool,
    pub(crate) applying_read_completion: bool,
}

impl NodeConnection {
    pub(crate) fn resource(&self) -> kiln_host_abi::ResourceId {
        self.driver.resource()
    }
}

pub(crate) fn node_server_object(state: &Rc<RefCell<NodeServerState>>) -> Value {
    let mut data = ObjectData {
        node_events: Some(Vec::new()),
        node_object: Some(NodeObject::NodeServer(Rc::clone(state))),
        ..Default::default()
    };
    data.set(
        "listen",
        Value::Native(Native::NodeServerListen, RealmId::PRIMARY),
    );
    data.set(
        "close",
        Value::Native(Native::NodeServerClose, RealmId::PRIMARY),
    );
    data.set(
        "address",
        Value::Native(Native::NodeServerAddress, RealmId::PRIMARY),
    );
    data.set(
        "setTimeout",
        Value::Native(Native::NodeServerSetTimeout, RealmId::PRIMARY),
    );
    data.set("timeout", Value::Number(0.0));
    data.set("keepAliveTimeout", Value::Number(5_000.0));
    data.set("requestTimeout", Value::Number(300_000.0));
    data.set("httpAllowHalfOpen", Value::Boolean(false));
    data.set("on", Value::Native(Native::NodeEventOn, RealmId::PRIMARY));
    data.set(
        "once",
        Value::Native(Native::NodeEventOnce, RealmId::PRIMARY),
    );
    data.set(
        "emit",
        Value::Native(Native::NodeEventEmit, RealmId::PRIMARY),
    );
    data.set(
        "removeListener",
        Value::Native(Native::NodeEventRemoveListener, RealmId::PRIMARY),
    );
    data.set(
        "off",
        Value::Native(Native::NodeEventRemoveListener, RealmId::PRIMARY),
    );
    data.set(
        "removeAllListeners",
        Value::Native(Native::NodeEventRemoveAllListeners, RealmId::PRIMARY),
    );
    let object = Rc::new(RefCell::new(data));
    state.borrow_mut().object = Some(Rc::downgrade(&object));
    Value::Object(object)
}

pub(crate) fn node_socket_object(state: &Rc<RefCell<NetSocketState>>) -> Value {
    let mut data = ObjectData {
        node_events: Some(Vec::new()),
        node_object: Some(NodeObject::NetSocket(Rc::clone(state))),
        ..Default::default()
    };
    data.set("on", Value::Native(Native::NodeEventOn, RealmId::PRIMARY));
    data.set(
        "once",
        Value::Native(Native::NodeEventOnce, RealmId::PRIMARY),
    );
    data.set(
        "emit",
        Value::Native(Native::NodeEventEmit, RealmId::PRIMARY),
    );
    data.set(
        "write",
        Value::Native(Native::NodeSocketWrite, RealmId::PRIMARY),
    );
    data.set(
        "end",
        Value::Native(Native::NodeSocketEnd, RealmId::PRIMARY),
    );
    data.set(
        "destroy",
        Value::Native(Native::NodeSocketDestroy, RealmId::PRIMARY),
    );
    data.set("readyState", Value::String("open".into()));
    data.set(
        "setEncoding",
        Value::Native(Native::NodeReadableSetEncoding, RealmId::PRIMARY),
    );
    let object = Rc::new(RefCell::new(data));
    state.borrow_mut().object = Some(Rc::downgrade(&object));
    Value::Object(object)
}

pub(crate) fn sync_node_socket_ready_state(
    server: &Rc<RefCell<NodeServerState>>,
    resource: kiln_host_abi::ResourceId,
) {
    let ready_state = server
        .borrow()
        .streams
        .iter()
        .find(|stream| stream.resource() == resource)
        .map(|stream| stream.driver.state().ready_state().as_str());
    let socket = node_net_socket_value(server, resource);
    if let (Some(ready_state), Some(Value::Object(socket))) = (ready_state, socket) {
        socket
            .borrow_mut()
            .set("readyState", Value::String(ready_state.into()));
    }
}

pub(crate) fn node_net_socket_value(
    server: &Rc<RefCell<NodeServerState>>,
    resource: kiln_host_abi::ResourceId,
) -> Option<Value> {
    server
        .borrow()
        .streams
        .iter()
        .find(|stream| stream.resource() == resource)
        .and_then(|stream| stream.request.clone())
}

pub(crate) fn node_net_socket_state(
    receiver: &Value,
) -> Result<Rc<RefCell<NetSocketState>>, ExecutionError> {
    let Value::Object(object) = receiver else {
        return Err(ExecutionError::TypeErrorMessage {
            message: "Socket method called on an incompatible receiver".into(),
        });
    };
    match &object.borrow().node_object {
        Some(NodeObject::NetSocket(state)) => Ok(Rc::clone(state)),
        _ => Err(ExecutionError::TypeErrorMessage {
            message: "Socket method called on an incompatible receiver".into(),
        }),
    }
}

pub(crate) fn node_listen_arguments(
    arguments: &[Value],
) -> Result<(u16, String, Option<Value>), ExecutionError> {
    let options = arguments.first();
    let port = match options {
        Some(Value::Object(_)) => options
            .and_then(|options| own_data_property(options, "port"))
            .map_or(f64::NAN, |value| value.as_number()),
        Some(value) => value.as_number(),
        None => f64::NAN,
    };
    if !port.is_finite() || port.fract() != 0.0 || !(0.0..=65535.0).contains(&port) {
        return Err(ExecutionError::InvalidPort);
    }
    let mut host = match options {
        Some(options @ Value::Object(_)) => own_data_property(options, "host")
            .and_then(|value| match value {
                Value::String(host) => Some(host),
                _ => None,
            })
            .unwrap_or_else(|| "0.0.0.0".into()),
        _ => "0.0.0.0".into(),
    };
    let mut callback = None;
    for argument in arguments.iter().skip(1) {
        if is_callable(argument) {
            callback = Some(argument.clone());
        } else if let Value::String(value) = argument {
            host = value.to_lossy_string().into();
        }
    }
    Ok((port as u16, host.to_lossy_string(), callback))
}

pub(crate) fn property(prop: &str) -> Option<Value> {
    match prop {
        "createServer" => Some(Value::Native(Native::NodeNetCreateServer, RealmId::PRIMARY)),
        "connect" | "createConnection" => {
            Some(Value::Native(Native::NodeNetConnect, RealmId::PRIMARY))
        }
        "default" => Some(Value::Native(Native::NodeNet, RealmId::PRIMARY)),
        _ => None,
    }
}

pub(crate) const fn js_name(native: Native) -> Option<&'static str> {
    match native {
        Native::NodeNet => Some("net"),
        Native::NodeNetCreateServer => Some("createServer"),
        Native::NodeNetConnect => Some("connect"),
        Native::NodeSocketWrite => Some("write"),
        Native::NodeSocketEnd => Some("end"),
        Native::NodeSocketDestroy => Some("destroy"),
        Native::NodeServerListen => Some("listen"),
        Native::NodeServerClose => Some("close"),
        Native::NodeServerAddress => Some("address"),
        Native::NodeServerSetTimeout => Some("setTimeout"),
        _ => None,
    }
}
