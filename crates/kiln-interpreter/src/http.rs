//! `node:http`, `node:https`, and `node:http2` implementation.
//!
//! Provides HTTP server and client request/response state tracking and methods.

use std::cell::RefCell;
use std::rc::{Rc, Weak};

use crate::net::{NetSocketState, NodeServerState};
use crate::{
    is_callable, ExecutionError, Native, NodeObject, ObjectData, Property, RealmId, Value,
};

#[derive(Debug)]
pub(crate) struct HttpClientRequestState {
    pub(crate) host: String,
    pub(crate) port: u16,
    pub(crate) method: String,
    pub(crate) path: String,
    pub(crate) headers: Vec<(String, String)>,
    pub(crate) body: Vec<u8>,
    pub(crate) callback: Value,
    pub(crate) socket: Option<Rc<RefCell<NetSocketState>>>,
    pub(crate) object: Option<Rc<RefCell<ObjectData>>>,
    pub(crate) ended: bool,
    pub(crate) started: bool,
    pub(crate) request_id: u64,
}

#[derive(Debug)]
pub(crate) struct HttpResponseState {
    pub(crate) stream: kiln_host_abi::ResourceId,
    pub(crate) request_id: Option<u64>,
    pub(crate) status: u16,
    pub(crate) headers: Vec<(String, String)>,
    pub(crate) streaming: bool,
    pub(crate) ended: bool,
    pub(crate) object: Option<Weak<RefCell<ObjectData>>>,
}

#[derive(Debug)]
pub(crate) struct HttpIncomingState {
    pub(crate) stream: kiln_host_abi::ResourceId,
    pub(crate) readable: Rc<RefCell<crate::ReadableState>>,
}

pub(crate) fn node_http_socket_facade() -> Value {
    let mut data = ObjectData {
        node_events: Some(Vec::new()),
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
    data.set("destroyed", Value::Boolean(false));
    data.set("encrypted", Value::Boolean(false));
    Value::Object(Rc::new(RefCell::new(data)))
}

pub(crate) fn node_http_server(
    receiver: &Value,
) -> Result<Rc<RefCell<NodeServerState>>, ExecutionError> {
    let Value::Object(object) = receiver else {
        return Err(ExecutionError::TypeErrorMessage {
            message: "Server method called on an incompatible receiver".into(),
        });
    };
    match &object.borrow().node_object {
        Some(NodeObject::NodeServer(state)) => Ok(Rc::clone(state)),
        _ => Err(ExecutionError::TypeErrorMessage {
            message: "Server method called on an incompatible receiver".into(),
        }),
    }
}

pub(crate) fn node_http_response(
    receiver: &Value,
) -> Result<Rc<RefCell<HttpResponseState>>, ExecutionError> {
    let Value::Object(object) = receiver else {
        return Err(ExecutionError::TypeErrorMessage {
            message: "ServerResponse method called on an incompatible receiver".into(),
        });
    };
    match &object.borrow().node_object {
        Some(NodeObject::HttpResponse(state)) => Ok(Rc::clone(state)),
        _ => Err(ExecutionError::TypeErrorMessage {
            message: "ServerResponse method called on an incompatible receiver".into(),
        }),
    }
}

pub(crate) fn node_http_incoming(
    receiver: &Value,
) -> Result<Rc<RefCell<HttpIncomingState>>, ExecutionError> {
    let Value::Object(object) = receiver else {
        return Err(ExecutionError::TypeErrorMessage {
            message: "IncomingMessage method called on an incompatible receiver".into(),
        });
    };
    match &object.borrow().node_object {
        Some(NodeObject::HttpIncoming(state)) => Ok(Rc::clone(state)),
        _ => Err(ExecutionError::TypeErrorMessage {
            message: "IncomingMessage method called on an incompatible receiver".into(),
        }),
    }
}

pub(crate) fn node_http_client_request(
    receiver: &Value,
) -> Result<Rc<RefCell<HttpClientRequestState>>, ExecutionError> {
    let Value::Object(object) = receiver else {
        return Err(ExecutionError::TypeErrorMessage {
            message: "ClientRequest method called on an incompatible receiver".into(),
        });
    };
    match &object.borrow().node_object {
        Some(NodeObject::HttpClientRequest(state)) => Ok(Rc::clone(state)),
        _ => Err(ExecutionError::TypeErrorMessage {
            message: "ClientRequest method called on an incompatible receiver".into(),
        }),
    }
}

pub(crate) fn node_http_client_request_object(
    state: &Rc<RefCell<HttpClientRequestState>>,
) -> Value {
    let mut data = ObjectData {
        node_events: Some(Vec::new()),
        node_object: Some(NodeObject::HttpClientRequest(Rc::clone(state))),
        ..Default::default()
    };
    data.set(
        "write",
        Value::Native(Native::NodeClientRequestWrite, RealmId::PRIMARY),
    );
    data.set(
        "end",
        Value::Native(Native::NodeClientRequestEnd, RealmId::PRIMARY),
    );
    data.set("on", Value::Native(Native::NodeEventOn, RealmId::PRIMARY));
    data.set(
        "once",
        Value::Native(Native::NodeEventOnce, RealmId::PRIMARY),
    );
    data.set(
        "emit",
        Value::Native(Native::NodeEventEmit, RealmId::PRIMARY),
    );
    let object = Rc::new(RefCell::new(data));
    state.borrow_mut().object = Some(Rc::clone(&object));
    Value::Object(object)
}

pub(crate) type HttpRequestArguments = (String, u16, String, String, Vec<(String, String)>, Value);

pub(crate) fn node_http_request_arguments(
    arguments: &[Value],
) -> Result<HttpRequestArguments, ExecutionError> {
    let options = arguments
        .first()
        .ok_or_else(|| ExecutionError::TypeErrorMessage {
            message: "HTTP request options must be an object".into(),
        })?;
    let Value::Object(options) = options else {
        return Err(ExecutionError::TypeErrorMessage {
            message: "HTTP request options must be an object".into(),
        });
    };
    let options = options.borrow();
    let own = |name: &str| options.get_own(name);
    let host = own("host")
        .or_else(|| own("hostname"))
        .map(|value| value.to_display_string())
        .unwrap_or_else(|| "localhost".into());
    let port = own("port").map(|value| value.as_number()).unwrap_or(80.0);
    if !port.is_finite() || port.fract() != 0.0 || !(1.0..=65535.0).contains(&port) {
        return Err(ExecutionError::InvalidPort);
    }
    let method = own("method")
        .map(|value| value.to_display_string().to_ascii_uppercase())
        .unwrap_or_else(|| "GET".into());
    let path = own("path")
        .map(|value| value.to_display_string())
        .unwrap_or_else(|| "/".into());
    let mut headers = Vec::new();
    if let Some(Value::Object(header_object)) = own("headers") {
        let header_object = header_object.borrow();
        for property in header_object
            .properties
            .iter()
            .filter(|property| property.attributes.enumerable)
        {
            if let Property::Value(value) = &property.property {
                headers.push((property.key.clone(), value.to_display_string()));
            }
        }
    }
    let callback = arguments
        .get(1)
        .cloned()
        .filter(is_callable)
        .unwrap_or(Value::Undefined);
    Ok((host, port as u16, method, path, headers, callback))
}

pub(crate) fn node_body_bytes(value: Option<&Value>) -> Result<Vec<u8>, ExecutionError> {
    match value.unwrap_or(&Value::Undefined) {
        Value::Undefined | Value::Null => Ok(Vec::new()),
        Value::String(value) => Ok(value.to_lossy_string().into_bytes()),
        Value::TypedArray(value) => Ok(value.bytes()),
        value => Err(ExecutionError::TypeErrorMessage {
            message: format!(
                "The chunk argument must be a string or typed array, received {}",
                value.type_name()
            ),
        }),
    }
}

pub(crate) fn readable_chunk_length(value: &Value) -> usize {
    match value {
        Value::String(value) => value.len(),
        Value::TypedArray(value) => value.bytes().len(),
        _ => 1,
    }
}

pub(crate) fn property(prop: &str) -> Option<Value> {
    match prop {
        "createServer" => Some(Value::Native(
            Native::NodeHttpCreateServer,
            RealmId::PRIMARY,
        )),
        "Server" => Some(Value::Native(Native::NodeHttpServer, RealmId::PRIMARY)),
        "IncomingMessage" => Some(Value::Native(Native::NodeReadable, RealmId::PRIMARY)),
        "request" => Some(Value::Native(Native::NodeHttpRequest, RealmId::PRIMARY)),
        "get" => Some(Value::Native(Native::NodeHttpGet, RealmId::PRIMARY)),
        "ServerResponse" => Some(Value::Native(
            Native::NodeHttpServerResponse,
            RealmId::PRIMARY,
        )),
        "default" => Some(Value::Native(Native::NodeHttp, RealmId::PRIMARY)),
        _ => None,
    }
}

pub(crate) fn https_property(prop: &str) -> Option<Value> {
    match prop {
        "createServer" => Some(Value::Native(
            Native::NodeHttpsCreateServer,
            RealmId::PRIMARY,
        )),
        "default" => Some(Value::Native(Native::NodeHttps, RealmId::PRIMARY)),
        _ => None,
    }
}

pub(crate) fn http2_property(prop: &str) -> Option<Value> {
    match prop {
        "createServer" => Some(Value::Native(
            Native::NodeHttp2CreateServer,
            RealmId::PRIMARY,
        )),
        "createSecureServer" => Some(Value::Native(
            Native::NodeHttp2CreateSecureServer,
            RealmId::PRIMARY,
        )),
        "default" => Some(Value::Native(Native::NodeHttp2, RealmId::PRIMARY)),
        _ => None,
    }
}

pub(crate) const fn js_name(native: Native) -> Option<&'static str> {
    match native {
        Native::NodeHttp => Some("http"),
        Native::NodeHttpServer => Some("Server"),
        Native::NodeHttpCreateServer => Some("createServer"),
        Native::NodeHttpRequest => Some("request"),
        Native::NodeHttpGet => Some("get"),
        Native::NodeHttpServerResponse => Some("ServerResponse"),
        Native::NodeHttps => Some("https"),
        Native::NodeHttpsCreateServer => Some("createServer"),
        Native::NodeHttp2 => Some("http2"),
        Native::NodeHttp2CreateServer => Some("createServer"),
        Native::NodeHttp2CreateSecureServer => Some("createSecureServer"),
        Native::NodeResponseSetHeader => Some("setHeader"),
        Native::NodeResponseGetHeader => Some("getHeader"),
        Native::NodeResponseGetHeaders => Some("getHeaders"),
        Native::NodeResponseHasHeader => Some("hasHeader"),
        Native::NodeResponseRemoveHeader => Some("removeHeader"),
        Native::NodeResponseWriteHead => Some("writeHead"),
        Native::NodeResponseWrite => Some("write"),
        Native::NodeResponseEnd => Some("end"),
        Native::NodeClientRequestWrite => Some("write"),
        Native::NodeClientRequestEnd => Some("end"),
        _ => None,
    }
}
