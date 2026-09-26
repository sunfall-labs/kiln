//! The WHATWG URL record the shared TypeScript `URL` class is built on.
//!
//! `record` answers `new URL(input[, base])` as the eleven components -- href, origin, protocol,
//! username, password, host, hostname, port, pathname, search, hash -- NUL-separated in that
//! order, and `set` answers one setter with the record after it. A component's spelling follows
//! node: `search` and `hash` are empty rather than `?` or `#` when there is nothing after the
//! mark, `port` is empty for the scheme's default, `origin` is `null` for an opaque origin.
//!
//! One implementation under both engines (`kiln-native-provider-ffi` spills it across the
//! private ABI; the interpreter calls it), so that `href` cannot come out spelled two ways.

/// The input did not parse: the constructor's `TypeError`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Invalid;

/// The setter fields, as `__kilnUrlSet` numbers them.
pub const SET_HREF: u32 = 0;
pub const SET_PROTOCOL: u32 = 1;
pub const SET_USERNAME: u32 = 2;
pub const SET_PASSWORD: u32 = 3;
pub const SET_HOST: u32 = 4;
pub const SET_HOSTNAME: u32 = 5;
pub const SET_PORT: u32 = 6;
pub const SET_PATHNAME: u32 = 7;
pub const SET_SEARCH: u32 = 8;
pub const SET_HASH: u32 = 9;

/// `new URL(input[, base])`.
pub fn record(input: &str, base: Option<&str>) -> Result<String, Invalid> {
    let parsed = match base {
        Some(base) => match url::Url::parse(base) {
            Ok(base) => base.join(input),
            Err(_) => return Err(Invalid),
        },
        None => url::Url::parse(input),
    };
    parsed.map(|parsed| serialize(&parsed)).map_err(|_| Invalid)
}

/// One setter over `href`; a setter the specification ignores -- a scheme change from special
/// to non-special, a port on a URL with no host -- leaves the URL as it was, which is what node
/// does. `href` itself must parse. An unknown `field` is `Err(Invalid)` too.
pub fn set(href: &str, field: u32, value: &str) -> Result<String, Invalid> {
    let Ok(mut parsed) = url::Url::parse(href) else {
        return Err(Invalid);
    };
    match field {
        SET_HREF => match url::Url::parse(value) {
            Ok(next) => parsed = next,
            Err(_) => return Err(Invalid),
        },
        SET_PROTOCOL => {
            let scheme = value.split(':').next().unwrap_or("");
            let _ = parsed.set_scheme(scheme);
        }
        SET_USERNAME => {
            let _ = parsed.set_username(value);
        }
        SET_PASSWORD => {
            let _ = parsed.set_password(if value.is_empty() { None } else { Some(value) });
        }
        SET_HOST => {
            // `host` takes `hostname[:port]`; a bare hostname leaves the port alone.
            let (hostname, port) = match value.rsplit_once(':') {
                Some((h, p))
                    if !p.is_empty()
                        && p.bytes().all(|b| b.is_ascii_digit())
                        && !h.ends_with(']')
                        || (h.contains('[') && h.ends_with(']')) =>
                {
                    (h, Some(p))
                }
                _ => (value, None),
            };
            if parsed.set_host(Some(hostname)).is_ok()
                && let Some(port) = port
            {
                let _ = parsed.set_port(port.parse::<u16>().ok());
            }
        }
        SET_HOSTNAME => {
            let _ = parsed.set_host(Some(value));
        }
        SET_PORT => {
            let digits: String = value.chars().take_while(char::is_ascii_digit).collect();
            if digits.is_empty() {
                if value.is_empty() {
                    let _ = parsed.set_port(None);
                }
            } else if let Ok(port) = digits.parse::<u16>() {
                let _ = parsed.set_port(Some(port));
            }
        }
        SET_PATHNAME => {
            if !parsed.cannot_be_a_base() {
                parsed.set_path(value);
            }
        }
        SET_SEARCH => {
            let query = value.strip_prefix('?').unwrap_or(value);
            parsed.set_query(if query.is_empty() { None } else { Some(query) });
        }
        SET_HASH => {
            let fragment = value.strip_prefix('#').unwrap_or(value);
            parsed.set_fragment(if fragment.is_empty() {
                None
            } else {
                Some(fragment)
            });
        }
        _ => return Err(Invalid),
    }
    Ok(serialize(&parsed))
}

fn serialize(parsed: &url::Url) -> String {
    use url::Position;
    let host = &parsed[Position::BeforeHost..Position::AfterPort];
    let port = parsed.port().map(|p| p.to_string()).unwrap_or_default();
    let search = match parsed.query() {
        Some(q) if !q.is_empty() => format!("?{q}"),
        _ => String::new(),
    };
    let hash = match parsed.fragment() {
        Some(f) if !f.is_empty() => format!("#{f}"),
        _ => String::new(),
    };
    [
        parsed.as_str(),
        &parsed.origin().ascii_serialization(),
        &format!("{}:", parsed.scheme()),
        parsed.username(),
        parsed.password().unwrap_or(""),
        host,
        parsed.host_str().unwrap_or(""),
        &port,
        parsed.path(),
        &search,
        &hash,
    ]
    .join("\0")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_the_eleven_components() {
        let full = record("https://u:p@example.com:8080/a/b?x=1#frag", None).unwrap();
        let parts: Vec<&str> = full.split('\0').collect();
        assert_eq!(
            parts,
            [
                "https://u:p@example.com:8080/a/b?x=1#frag",
                "https://example.com:8080",
                "https:",
                "u",
                "p",
                "example.com:8080",
                "example.com",
                "8080",
                "/a/b",
                "?x=1",
                "#frag"
            ]
        );
        assert_eq!(record("nope", None), Err(Invalid));
        assert!(
            record("/x", Some("http://h/"))
                .unwrap()
                .starts_with("http://h/x\0")
        );
    }

    #[test]
    fn setters_follow_node() {
        let href = "http://h/p?q#f";
        assert!(
            set(href, SET_SEARCH, "a=1")
                .unwrap()
                .starts_with("http://h/p?a=1#f\0")
        );
        assert!(
            set(href, SET_HASH, "")
                .unwrap()
                .starts_with("http://h/p?q\0")
        );
        assert!(
            set(href, SET_PORT, "81x")
                .unwrap()
                .starts_with("http://h:81/p?q#f\0")
        );
        assert!(
            set(href, SET_HOST, "k:9")
                .unwrap()
                .starts_with("http://k:9/p?q#f\0")
        );
        assert_eq!(set(href, 42, ""), Err(Invalid));
    }
}
