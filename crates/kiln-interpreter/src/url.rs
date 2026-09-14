//! WHATWG URL and `node:url` implementation.
//!
//! Provides the `URL` and `URLSearchParams` objects, URL property reflection,
//! and Node file URL / path conversions (`fileURLToPath`, `pathToFileURL`, legacy `parse`).

use std::cell::RefCell;
use std::rc::Rc;

use crate::{ArrayData, ExecutionError, Native, NodeObject, ObjectData, RealmId, Value};

#[derive(Clone, Debug)]
pub(crate) struct UrlState {
    pub(crate) href: String,
    pub(crate) origin: String,
    pub(crate) protocol: String,
    pub(crate) host: String,
    pub(crate) hostname: String,
    pub(crate) port: String,
    pub(crate) pathname: String,
    pub(crate) search: String,
    pub(crate) hash: String,
    pub(crate) username: String,
    pub(crate) password: String,
    pub(crate) query: Vec<(String, String)>,
}

pub(crate) fn url_object(state: UrlState) -> Value {
    let state = Rc::new(state);
    let object = ObjectData {
        node_object: Some(NodeObject::Url(state)),
        ..Default::default()
    };
    Value::Object(Rc::new(RefCell::new(object)))
}

pub(crate) fn url_search_params_object(state: Rc<UrlState>) -> Value {
    let mut object = ObjectData {
        node_object: Some(NodeObject::UrlSearchParams(state)),
        ..Default::default()
    };
    object.set(
        "get",
        Value::Native(Native::UrlSearchParamsGet, RealmId::PRIMARY),
    );
    object.set(
        "has",
        Value::Native(Native::UrlSearchParamsHas, RealmId::PRIMARY),
    );
    object.set(
        "getAll",
        Value::Native(Native::UrlSearchParamsGetAll, RealmId::PRIMARY),
    );
    object.set(
        "toString",
        Value::Native(Native::UrlSearchParamsToString, RealmId::PRIMARY),
    );
    Value::Object(Rc::new(RefCell::new(object)))
}

pub(crate) fn parse_url(input: &str, base: Option<&str>) -> Result<UrlState, ExecutionError> {
    let parsed = match base {
        Some(base_str) => {
            let base_url =
                ::url::Url::parse(base_str).map_err(|_| ExecutionError::TypeErrorMessage {
                    message: format!("Invalid base URL: {base_str}"),
                })?;
            base_url
                .join(input)
                .map_err(|_| ExecutionError::TypeErrorMessage {
                    message: format!("Invalid URL: {input}"),
                })?
        }
        None => ::url::Url::parse(input).map_err(|_| ExecutionError::TypeErrorMessage {
            message: format!("Invalid URL: {input}"),
        })?,
    };

    let href = parsed.as_str().to_owned();
    let origin = parsed.origin().ascii_serialization();
    let protocol = format!("{}:", parsed.scheme());
    let host = match (parsed.host_str(), parsed.port()) {
        (Some(h), Some(p)) => format!("{h}:{p}"),
        (Some(h), None) => h.to_owned(),
        (None, _) => String::new(),
    };
    let hostname = parsed.host_str().unwrap_or("").to_owned();
    let port = parsed.port().map_or_else(String::new, |p| p.to_string());
    let pathname = parsed.path().to_owned();
    let search = parsed.query().map_or_else(String::new, |q| format!("?{q}"));
    let hash = parsed
        .fragment()
        .map_or_else(String::new, |f| format!("#{f}"));
    let username = parsed.username().to_owned();
    let password = parsed.password().unwrap_or("").to_owned();
    let query = parsed
        .query_pairs()
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();

    Ok(UrlState {
        href,
        origin,
        protocol,
        host,
        hostname,
        port,
        pathname,
        search,
        hash,
        username,
        password,
        query,
    })
}

pub(crate) fn node_legacy_url_parse(input: &str) -> Value {
    let fragment_at = input.find('#');
    let query_at = input
        .find('?')
        .filter(|query| fragment_at.is_none_or(|fragment| *query < fragment));
    let pathname_end = query_at.or(fragment_at).unwrap_or(input.len());
    let pathname = &input[..pathname_end];
    let query_end = fragment_at.unwrap_or(input.len());
    let query = query_at.map_or("", |query| &input[query + 1..query_end]);
    let search = query_at.map_or("", |query| &input[query..query_end]);
    let hash = fragment_at.map_or("", |fragment| &input[fragment..]);
    let mut object = ObjectData::default();
    object.set("href", Value::String(input.into()));
    object.set("pathname", Value::String(pathname.into()));
    object.set("path", Value::String(format!("{pathname}{search}").into()));
    object.set(
        "search",
        if query_at.is_some() {
            Value::String(search.into())
        } else {
            Value::Null
        },
    );
    object.set("query", Value::String(query.into()));
    object.set(
        "hash",
        if fragment_at.is_some() {
            Value::String(hash.into())
        } else {
            Value::Null
        },
    );
    Value::Object(Rc::new(RefCell::new(object)))
}

pub(crate) fn property(prop: &str) -> Option<Value> {
    match prop {
        "URL" => Some(Value::Native(Native::UrlConstructor, RealmId::PRIMARY)),
        "parse" => Some(Value::Native(Native::NodeUrlParse, RealmId::PRIMARY)),
        "fileURLToPath" => Some(Value::Native(
            Native::NodeUrlFileUrlToPath,
            RealmId::PRIMARY,
        )),
        "pathToFileURL" => Some(Value::Native(
            Native::NodeUrlPathToFileUrl,
            RealmId::PRIMARY,
        )),
        "default" => Some(Value::Native(Native::NodeUrl, RealmId::PRIMARY)),
        _ => None,
    }
}

pub(crate) fn read_property(object: &ObjectData, state: &Rc<UrlState>, name: &str) -> Value {
    match name {
        "href" => Value::String(state.href.clone().into()),
        "origin" => Value::String(state.origin.clone().into()),
        "protocol" => Value::String(state.protocol.clone().into()),
        "host" => Value::String(state.host.clone().into()),
        "hostname" => Value::String(state.hostname.clone().into()),
        "port" => Value::String(state.port.clone().into()),
        "pathname" => Value::String(state.pathname.clone().into()),
        "search" => Value::String(state.search.clone().into()),
        "hash" => Value::String(state.hash.clone().into()),
        "username" => Value::String(state.username.clone().into()),
        "password" => Value::String(state.password.clone().into()),
        "searchParams" => url_search_params_object(Rc::clone(state)),
        // Both answer `href`. A `URL` is one of the few objects real code hands
        // straight to string concatenation -- `` `imported from ${url}` `` --
        // and without them `String(url)` was "Cannot convert object to primitive
        // value": postcss's `CssSyntaxError` constructor stringifies the file URL
        // it built, so a *CSS syntax error* surfaced as a TypeError from the
        // reporter rather than as the located error it had already worked out.
        "toString" | "toJSON" => Value::Native(Native::UrlToString, RealmId::PRIMARY),
        _ => object.get(name).unwrap_or(Value::Undefined),
    }
}

pub(crate) const fn js_name(native: Native) -> Option<&'static str> {
    match native {
        Native::NodeUrl => Some("url"),
        Native::UrlConstructor => Some("URL"),
        Native::NodeUrlParse => Some("parse"),
        Native::NodeUrlFileUrlToPath => Some("fileURLToPath"),
        Native::NodeUrlPathToFileUrl => Some("pathToFileURL"),
        Native::UrlToString => Some("toString"),
        Native::UrlSearchParamsGet => Some("get"),
        Native::UrlSearchParamsHas => Some("has"),
        Native::UrlSearchParamsGetAll => Some("getAll"),
        Native::UrlSearchParamsToString => Some("toString"),
        _ => None,
    }
}

pub(crate) fn dispatch(
    native: Native,
    receiver: &Value,
    arguments: &[Value],
) -> Result<Value, ExecutionError> {
    match native {
        Native::UrlConstructor => {
            let input = arguments
                .first()
                .map(Value::to_display_string)
                .unwrap_or_default();
            let base = arguments.get(1).and_then(|val| match val {
                Value::Undefined => None,
                Value::Object(obj) => {
                    if let Some(NodeObject::Url(state)) = &obj.borrow().node_object {
                        Some(state.href.clone())
                    } else {
                        Some(val.to_display_string())
                    }
                }
                _ => Some(val.to_display_string()),
            });
            Ok(url_object(parse_url(&input, base.as_deref())?))
        }
        Native::NodeUrlParse => Ok(node_legacy_url_parse(
            &arguments
                .first()
                .map(Value::to_display_string)
                .unwrap_or_default(),
        )),
        Native::NodeUrlFileUrlToPath => {
            let url_string = match arguments.first() {
                Some(Value::Object(obj)) => {
                    if let Some(NodeObject::Url(state)) = &obj.borrow().node_object {
                        state.href.clone()
                    } else {
                        arguments
                            .first()
                            .map(Value::to_display_string)
                            .unwrap_or_default()
                    }
                }
                Some(val) => val.to_display_string(),
                None => String::new(),
            };
            let parsed =
                ::url::Url::parse(&url_string).map_err(|_| ExecutionError::TypeErrorMessage {
                    message: "fileURLToPath requires a file: URL".to_owned(),
                })?;
            if parsed.scheme() != "file" {
                return Err(ExecutionError::TypeErrorMessage {
                    message: "fileURLToPath requires a file: URL".to_owned(),
                });
            }
            let path = parsed
                .to_file_path()
                .map_err(|_| ExecutionError::TypeErrorMessage {
                    message: "fileURLToPath could not convert file URL to path".to_owned(),
                })?;
            Ok(Value::String(path.to_string_lossy().into_owned().into()))
        }
        Native::NodeUrlPathToFileUrl => {
            let path = arguments
                .first()
                .map(Value::to_display_string)
                .unwrap_or_default();
            let path_buf = std::path::PathBuf::from(&path);
            let url_res = if path_buf.is_absolute() {
                ::url::Url::from_file_path(&path_buf)
            } else if let Ok(cwd) = std::env::current_dir() {
                ::url::Url::from_file_path(cwd.join(&path_buf))
            } else {
                ::url::Url::from_file_path(format!("/{path}"))
            };
            let parsed_url = match url_res {
                Ok(u) => u,
                Err(_) => {
                    let abs = if path.starts_with('/') {
                        path
                    } else {
                        format!("/{path}")
                    };
                    ::url::Url::parse(&format!("file://{abs}")).map_err(|_| {
                        ExecutionError::TypeErrorMessage {
                            message: "pathToFileURL failed".to_owned(),
                        }
                    })?
                }
            };
            Ok(url_object(parse_url(parsed_url.as_str(), None)?))
        }
        Native::UrlToString => {
            let Value::Object(object) = receiver else {
                return Ok(Value::Undefined);
            };
            let href = match &object.borrow().node_object {
                Some(NodeObject::Url(state)) => state.href.clone(),
                _ => String::new(),
            };
            Ok(Value::String(href.into()))
        }
        Native::UrlSearchParamsGet => {
            let Value::Object(object) = receiver else {
                return Err(ExecutionError::TypeErrorMessage {
                    message: "URLSearchParams.get called on an incompatible receiver".into(),
                });
            };
            let state = match &object.borrow().node_object {
                Some(NodeObject::UrlSearchParams(state)) => Rc::clone(state),
                _ => {
                    return Err(ExecutionError::TypeErrorMessage {
                        message: "URLSearchParams.get called on an incompatible receiver".into(),
                    });
                }
            };
            let name = arguments
                .first()
                .map(Value::to_display_string)
                .unwrap_or_default();
            Ok(state
                .query
                .iter()
                .find(|(key, _)| key == &name)
                .map(|(_, value)| Value::String(value.clone().into()))
                .unwrap_or(Value::Null))
        }
        Native::UrlSearchParamsHas => {
            let Value::Object(object) = receiver else {
                return Err(ExecutionError::TypeErrorMessage {
                    message: "URLSearchParams.has called on an incompatible receiver".into(),
                });
            };
            let state = match &object.borrow().node_object {
                Some(NodeObject::UrlSearchParams(state)) => Rc::clone(state),
                _ => {
                    return Err(ExecutionError::TypeErrorMessage {
                        message: "URLSearchParams.has called on an incompatible receiver".into(),
                    });
                }
            };
            let name = arguments
                .first()
                .map(Value::to_display_string)
                .unwrap_or_default();
            Ok(Value::Boolean(
                state.query.iter().any(|(key, _)| key == &name),
            ))
        }
        Native::UrlSearchParamsGetAll => {
            let Value::Object(object) = receiver else {
                return Err(ExecutionError::TypeErrorMessage {
                    message: "URLSearchParams.getAll called on an incompatible receiver".into(),
                });
            };
            let state = match &object.borrow().node_object {
                Some(NodeObject::UrlSearchParams(state)) => Rc::clone(state),
                _ => {
                    return Err(ExecutionError::TypeErrorMessage {
                        message: "URLSearchParams.getAll called on an incompatible receiver".into(),
                    });
                }
            };
            let name = arguments
                .first()
                .map(Value::to_display_string)
                .unwrap_or_default();
            let values: Vec<Value> = state
                .query
                .iter()
                .filter(|(key, _)| key == &name)
                .map(|(_, value)| Value::String(value.clone().into()))
                .collect();
            Ok(Value::Array(ArrayData::new(values)))
        }
        Native::UrlSearchParamsToString => {
            let Value::Object(object) = receiver else {
                return Err(ExecutionError::TypeErrorMessage {
                    message: "URLSearchParams.toString called on an incompatible receiver".into(),
                });
            };
            let state = match &object.borrow().node_object {
                Some(NodeObject::UrlSearchParams(state)) => Rc::clone(state),
                _ => {
                    return Err(ExecutionError::TypeErrorMessage {
                        message: "URLSearchParams.toString called on an incompatible receiver"
                            .into(),
                    });
                }
            };
            let encoded = ::url::form_urlencoded::Serializer::new(String::new())
                .extend_pairs(&state.query)
                .finish();
            Ok(Value::String(encoded.into()))
        }
        _ => Err(ExecutionError::TypeErrorMessage {
            message: "Invalid native URL operation".into(),
        }),
    }
}
