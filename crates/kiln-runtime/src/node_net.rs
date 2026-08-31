//! Provider-neutral state for Node TCP servers.
//!
//! The listener owns exactly one accept request at a time. Accepted sockets become independent
//! [`StreamDriver`](crate::node_stream::StreamDriver)s; HTTP and raw `node:net` differ only in
//! what consumes their byte/event actions.

use kiln_host_abi::{
    AcceptedTcpStream, AsyncHostError, AsyncHostOp, AsyncHostValue, Completion, RequestId,
    ResourceInfo,
};

/// One side effect selected by [`TcpServerDriver`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServerAction {
    Start(AsyncHostOp),
    Cancel(RequestId),
    Listening(ResourceInfo),
    Connection(AcceptedTcpStream),
    Close,
    Error(AsyncHostError),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PendingKind {
    Listen,
    Accept,
    Close,
    Ref,
}

/// Request-correlating state for one TCP server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TcpServerDriver {
    listener: Option<ResourceInfo>,
    pending: Vec<(RequestId, PendingKind)>,
    accepting: bool,
    closing: bool,
    closed: bool,
    referenced: bool,
}

impl Default for TcpServerDriver {
    fn default() -> Self {
        Self {
            listener: None,
            pending: Vec::new(),
            accepting: false,
            closing: false,
            closed: false,
            referenced: true,
        }
    }
}

impl TcpServerDriver {
    pub fn listen(&self, host: String, port: u16) -> ServerAction {
        ServerAction::Start(AsyncHostOp::TcpListen { host, port })
    }

    pub fn listener(&self) -> Option<&ResourceInfo> {
        self.listener.as_ref()
    }

    pub const fn referenced(&self) -> bool {
        self.referenced
    }

    pub const fn closed(&self) -> bool {
        self.closed
    }

    pub fn set_referenced(&self, referenced: bool) -> Option<ServerAction> {
        self.listener.as_ref().map(|listener| {
            ServerAction::Start(AsyncHostOp::SetResourceReferenced {
                resource: listener.id,
                referenced,
            })
        })
    }

    pub fn close(&mut self) -> Vec<ServerAction> {
        let Some(listener) = self.listener.as_ref() else {
            return Vec::new();
        };
        if self.closing || self.closed {
            return Vec::new();
        }
        self.closing = true;
        self.accepting = false;
        let mut actions: Vec<ServerAction> = self
            .pending
            .iter()
            .filter(|(_, kind)| matches!(kind, PendingKind::Accept))
            .map(|(request, _)| ServerAction::Cancel(*request))
            .collect();
        self.pending
            .retain(|(_, kind)| !matches!(kind, PendingKind::Accept));
        actions.push(ServerAction::Start(AsyncHostOp::CloseResource {
            resource: listener.id,
        }));
        actions
    }

    pub fn started(&mut self, operation: &AsyncHostOp, request: RequestId) -> bool {
        let kind = match operation {
            AsyncHostOp::TcpListen { .. } if self.listener.is_none() && !self.closing => {
                PendingKind::Listen
            }
            AsyncHostOp::TcpAccept { listener }
                if self
                    .listener
                    .as_ref()
                    .is_some_and(|known| known.id == *listener)
                    && !self.closing =>
            {
                self.accepting = true;
                PendingKind::Accept
            }
            AsyncHostOp::CloseResource { resource }
                if self
                    .listener
                    .as_ref()
                    .is_some_and(|known| known.id == *resource) =>
            {
                PendingKind::Close
            }
            AsyncHostOp::SetResourceReferenced { resource, .. }
                if self
                    .listener
                    .as_ref()
                    .is_some_and(|known| known.id == *resource) =>
            {
                PendingKind::Ref
            }
            _ => return false,
        };
        if self.pending.iter().any(|(known, _)| *known == request) {
            return false;
        }
        self.pending.push((request, kind));
        true
    }

    pub fn completed(&mut self, completion: Completion) -> Option<Vec<ServerAction>> {
        let index = self
            .pending
            .iter()
            .position(|(request, _)| *request == completion.request)?;
        let (_, kind) = self.pending.remove(index);
        let value = match completion.result {
            Ok(value) => value,
            Err(error) => {
                if matches!(kind, PendingKind::Accept) {
                    self.accepting = false;
                }
                return Some(vec![ServerAction::Error(error)]);
            }
        };
        let actions = match kind {
            PendingKind::Listen => match value {
                AsyncHostValue::Resource(listener) => {
                    self.listener = Some(listener.clone());
                    vec![
                        ServerAction::Listening(listener.clone()),
                        ServerAction::Start(AsyncHostOp::TcpAccept {
                            listener: listener.id,
                        }),
                    ]
                }
                _ => vec![ServerAction::Error(unexpected_value("listen"))],
            },
            PendingKind::Accept => {
                self.accepting = false;
                match value {
                    AsyncHostValue::AcceptedTcpStream(stream) => {
                        let mut actions = vec![ServerAction::Connection(stream)];
                        if !self.closing {
                            let listener = self.listener.as_ref().expect("accept has a listener");
                            actions.push(ServerAction::Start(AsyncHostOp::TcpAccept {
                                listener: listener.id,
                            }));
                        }
                        actions
                    }
                    _ => vec![ServerAction::Error(unexpected_value("accept"))],
                }
            }
            PendingKind::Close => {
                if matches!(value, AsyncHostValue::Undefined) {
                    self.closed = true;
                    self.listener = None;
                    vec![ServerAction::Close]
                } else {
                    vec![ServerAction::Error(unexpected_value("close"))]
                }
            }
            PendingKind::Ref => {
                if matches!(value, AsyncHostValue::Undefined) {
                    Vec::new()
                } else {
                    vec![ServerAction::Error(unexpected_value("ref"))]
                }
            }
        };
        Some(actions)
    }

    pub fn referenced_completed(&mut self, referenced: bool) {
        self.referenced = referenced;
    }
}

fn unexpected_value(syscall: &str) -> AsyncHostError {
    AsyncHostError::new(
        "ERR_KILN_HOST_VALUE",
        syscall,
        None,
        "host returned an incompatible value for the server operation",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use kiln_host_abi::{ResourceId, ResourceKind};

    fn completion(request: u64, value: AsyncHostValue) -> Completion {
        Completion {
            request: RequestId::new(request),
            sequence: request,
            result: Ok(value),
        }
    }

    #[test]
    fn listen_enters_a_single_perpetual_accept_loop() {
        let mut server = TcpServerDriver::default();
        let action = server.listen("127.0.0.1".into(), 0);
        let ServerAction::Start(listen) = action else {
            unreachable!()
        };
        assert!(server.started(&listen, RequestId::new(1)));
        let listener = ResourceInfo {
            id: ResourceId::new(7),
            kind: ResourceKind::TcpListener,
            address: Some("127.0.0.1".into()),
            port: Some(1234),
        };
        let actions = server
            .completed(completion(1, AsyncHostValue::Resource(listener.clone())))
            .unwrap();
        assert_eq!(actions[0], ServerAction::Listening(listener.clone()));
        let ServerAction::Start(accept) = &actions[1] else {
            unreachable!()
        };
        assert!(server.started(accept, RequestId::new(2)));
        assert!(!server.started(accept, RequestId::new(2)));

        let accepted = AcceptedTcpStream {
            resource: ResourceInfo {
                id: ResourceId::new(8),
                kind: ResourceKind::TcpStream,
                address: Some("127.0.0.1".into()),
                port: Some(1234),
            },
            peer_address: "127.0.0.1".into(),
            peer_port: 5000,
        };
        let actions = server
            .completed(completion(
                2,
                AsyncHostValue::AcceptedTcpStream(accepted.clone()),
            ))
            .unwrap();
        assert_eq!(actions[0], ServerAction::Connection(accepted));
        assert_eq!(
            actions[1],
            ServerAction::Start(AsyncHostOp::TcpAccept {
                listener: listener.id
            })
        );
    }

    #[test]
    fn close_stops_rearming_accept_and_emits_once() {
        let listener = ResourceInfo {
            id: ResourceId::new(3),
            kind: ResourceKind::TcpListener,
            address: None,
            port: Some(80),
        };
        let mut server = TcpServerDriver::default();
        let listen = AsyncHostOp::TcpListen {
            host: "0.0.0.0".into(),
            port: 80,
        };
        server.started(&listen, RequestId::new(1));
        let listening = server
            .completed(completion(1, AsyncHostValue::Resource(listener.clone())))
            .expect("listen completes");
        let ServerAction::Start(accept) = &listening[1] else {
            unreachable!()
        };
        assert!(server.started(accept, RequestId::new(2)));
        let actions = server.close();
        assert!(server.close().is_empty());
        assert_eq!(actions[0], ServerAction::Cancel(RequestId::new(2)));
        let ServerAction::Start(close_op) = &actions[1] else {
            unreachable!()
        };
        assert!(server.started(close_op, RequestId::new(3)));
        assert_eq!(
            server.completed(completion(3, AsyncHostValue::Undefined)),
            Some(vec![ServerAction::Close])
        );
        assert!(server.closed());
    }
}
