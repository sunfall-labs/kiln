//! `node:http`, `node:https`, and `node:http2` implementation.
//!
//! Provides HTTP server and client request/response state tracking and methods.

use std::cell::RefCell;
use std::rc::{Rc, Weak};

use crate::net::{NetSocketState, NodeServerState};
use crate::{
    ExecutionError, Native, NodeObject, ObjectData, Property, RealmId, Value, is_callable,
};

#[derive(Debug)]
pub(crate) struct HttpClientRequestState {
    pub(crate) host: String,
    pub(crate) port: u16,
    pub(crate) method: String,
    pub(crate) path: String,
    pub(crate) headers: Vec<(String, String)>,
    pub(crate) body: Vec<u8>,
    pub(crate) socket: Option<Rc<RefCell<NetSocketState>>>,
    pub(crate) object: Option<Rc<RefCell<ObjectData>>>,
    pub(crate) ended: bool,
    pub(crate) started: bool,
    /// The ended body has been handed to the host request slot. Socket/read events may
    /// revisit `flush_http_client_request`; `ended` alone is not an instruction to end twice.
    pub(crate) end_sent: bool,
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
    data.set("socket", Value::Null);
    data.set("connection", Value::Null);
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
        "addListener",
        Value::Native(Native::NodeEventOn, RealmId::PRIMARY),
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
    data.set(
        "listeners",
        Value::Native(Native::NodeEventListeners, RealmId::PRIMARY),
    );
    data.set(
        "listenerCount",
        Value::Native(Native::NodeEventListenerCount, RealmId::PRIMARY),
    );
    data.set(
        "eventNames",
        Value::Native(Native::NodeEventNames, RealmId::PRIMARY),
    );
    data.set(
        "setMaxListeners",
        Value::Native(Native::NodeEventSetMaxListeners, RealmId::PRIMARY),
    );
    let object = Rc::new(RefCell::new(data));
    state.borrow_mut().object = Some(Rc::clone(&object));
    Value::Object(object)
}

pub(crate) type HttpRequestArguments = (String, u16, String, String, Vec<(String, String)>, Value);

pub(crate) fn node_http_request_arguments(
    arguments: &[Value],
) -> Result<HttpRequestArguments, ExecutionError> {
    // `http.request(url[, options][, callback])`: a string first argument is a URL whose host,
    // port and path the options object -- if there is one -- overrides. `node/http.ts` reduces
    // the same three spellings for the native backend; this is the interpreter's copy of the rule,
    // because its `node:http` dispatches here without passing through that module.
    let mut from_url: Option<(String, f64, String)> = None;
    let mut at = 0;
    if let Some(Value::String(text)) = arguments.first() {
        let parsed = ::url::Url::parse(&text.to_lossy_string()).map_err(|_| {
            ExecutionError::TypeErrorMessage {
                message: format!("Invalid URL: {}", text.to_lossy_string()),
            }
        })?;
        if parsed.scheme() != "http" && parsed.scheme() != "https" {
            return Err(ExecutionError::TypeErrorMessage {
                message: format!(
                    "Protocol \"{}:\" not supported. Expected \"http:\"",
                    parsed.scheme()
                ),
            });
        }
        let host = parsed.host_str().unwrap_or("localhost").to_owned();
        let port = f64::from(parsed.port_or_known_default().unwrap_or(80));
        let mut path = parsed.path().to_owned();
        if let Some(query) = parsed.query() {
            path.push('?');
            path.push_str(query);
        }
        from_url = Some((host, port, path));
        at = 1;
    }
    let options = match arguments.get(at) {
        Some(Value::Object(options)) => Some(options.borrow()),
        _ if from_url.is_some() => None,
        _ => {
            return Err(ExecutionError::TypeErrorMessage {
                message: "HTTP request options must be an object".into(),
            });
        }
    };
    let own = |name: &str| options.as_ref().and_then(|options| options.get_own(name));
    let (url_host, url_port, url_path) = from_url
        .clone()
        .unwrap_or_else(|| ("localhost".into(), 80.0, "/".into()));
    let host = own("host")
        .or_else(|| own("hostname"))
        .map(|value| value.to_display_string())
        .unwrap_or(url_host);
    let port = own("port")
        .map(|value| value.as_number())
        .unwrap_or(url_port);
    if !port.is_finite() || port.fract() != 0.0 || !(1.0..=65535.0).contains(&port) {
        return Err(ExecutionError::InvalidPort);
    }
    let method = own("method")
        .map(|value| value.to_display_string().to_ascii_uppercase())
        .unwrap_or_else(|| "GET".into());
    let path = own("path")
        .map(|value| value.to_display_string())
        .unwrap_or(url_path);
    let mut headers = Vec::new();
    if let Some(Value::Object(header_object)) = own("headers") {
        let header_object = header_object.borrow();
        for property in header_object
            .properties
            .iter()
            .filter(|property| property.attributes.enumerable)
        {
            if let Property::Value(value) = &property.property {
                headers.push((property.key.to_lossy_string(), value.to_display_string()));
            }
        }
    }
    let callback_at = if options.is_some() { at + 1 } else { at };
    let callback = arguments
        .get(callback_at)
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

/// Reduce Node's `ServerResponse.end([chunk[, encoding]][, callback])` overloads to their body
/// bytes and completion callback. A callback passed as the only argument is not a body chunk.
pub(crate) fn node_response_end_arguments(
    arguments: &[Value],
) -> Result<(Vec<u8>, Option<Value>), ExecutionError> {
    let callback = arguments
        .iter()
        .rev()
        .find(|value| is_callable(value))
        .cloned();
    let chunk = arguments
        .first()
        .filter(|value| !is_callable(value) && !matches!(value, Value::Undefined));
    let bytes = node_body_bytes(chunk)?;
    Ok((bytes, callback))
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
        "createFetchServer" => Some(Value::Native(
            Native::NodeHttpCreateFetchServer,
            RealmId::PRIMARY,
        )),
        "fetchRespond" => Some(Value::Native(
            Native::NodeHttpFetchRespond,
            RealmId::PRIMARY,
        )),
        "ServerResponse" => Some(Value::Native(
            Native::NodeHttpServerResponse,
            RealmId::PRIMARY,
        )),
        "default" => Some(Value::Native(Native::NodeHttp, RealmId::PRIMARY)),
        _ => None,
    }
}

/// Host capabilities captured by the TypeScript HTTP and web facades. Keep them separate from
/// the public namespace: its complete materialized export store excludes these private hooks.
pub(crate) fn native_capabilities() -> Value {
    let mut module = ObjectData::default();
    for name in [
        "createServer",
        "Server",
        "IncomingMessage",
        "request",
        "get",
        "ServerResponse",
    ] {
        if let Some(value) = property(name) {
            module.set(name, value);
        }
    }
    for name in ["createFetchServer", "fetchRespond"] {
        if let Some(value) = property(name) {
            module.set_non_enumerable(name, value);
        }
    }
    Value::Object(Rc::new(RefCell::new(module)))
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
        Native::NodeHttpCreateFetchServer => Some("createFetchServer"),
        Native::NodeHttpFetchRespond => Some("fetchRespond"),
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
        Native::NodeClientRequestAssignSocket => Some("assignSocket"),
        _ => None,
    }
}
