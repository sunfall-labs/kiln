//! Provider-neutral HTTP Agent state.
//!
//! The Agent chooses which origin owns a connection and when that connection may accept another
//! request. It owns no socket, Tokio task, Hyper sender, or JavaScript value; bindings translate
//! these decisions into their existing resource and request identities.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use kiln_host_abi::ResourceId;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct HttpOrigin {
    scheme: String,
    host: String,
    port: u16,
}

impl HttpOrigin {
    pub fn http(host: impl Into<String>, port: u16) -> Self {
        Self {
            scheme: "http".into(),
            host: host.into().to_ascii_lowercase(),
            port,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentDecision {
    Connect,
    Reuse(ResourceId),
    Queued,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentRelease {
    Idle,
    Dispatch { request: u64 },
    Retire,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentFailure {
    Empty,
    Connect { request: u64 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentCancellation {
    Missing,
    Waiting,
    Connecting { next: Option<u64> },
}

#[derive(Debug)]
pub struct HttpAgent {
    max_sockets: usize,
    origins: BTreeMap<HttpOrigin, OriginPool>,
    resources: BTreeMap<ResourceId, HttpOrigin>,
}

#[derive(Debug, Default)]
struct OriginPool {
    connecting: BTreeSet<u64>,
    idle: VecDeque<ResourceId>,
    busy: BTreeMap<ResourceId, u64>,
    /// Connections released as non-reusable remain accounted for until the transport's close
    /// event. A waiter can only start a new connection after that event retires the old resource.
    retiring: BTreeSet<ResourceId>,
    waiting: VecDeque<u64>,
}

impl HttpAgent {
    pub fn new(max_sockets: usize) -> Self {
        Self {
            max_sockets: max_sockets.max(1),
            origins: BTreeMap::new(),
            resources: BTreeMap::new(),
        }
    }

    pub fn acquire(&mut self, origin: HttpOrigin, request: u64) -> AgentDecision {
        let pool = self.origins.entry(origin).or_default();
        if let Some(resource) = pool.idle.pop_front() {
            pool.busy.insert(resource, request);
            return AgentDecision::Reuse(resource);
        }
        if pool
            .connecting
            .len()
            .saturating_add(pool.busy.len())
            .saturating_add(pool.retiring.len())
            >= self.max_sockets
        {
            pool.waiting.push_back(request);
            return AgentDecision::Queued;
        }
        pool.connecting.insert(request);
        AgentDecision::Connect
    }

    pub fn connected(&mut self, origin: HttpOrigin, resource: ResourceId, request: u64) -> bool {
        let pool = self.origins.entry(origin.clone()).or_default();
        if self.resources.contains_key(&resource) || !pool.connecting.remove(&request) {
            return false;
        }
        pool.busy.insert(resource, request);
        self.resources.insert(resource, origin);
        true
    }

    pub fn release(&mut self, resource: ResourceId, reusable: bool) -> AgentRelease {
        let Some(origin) = self.resources.get(&resource).cloned() else {
            return AgentRelease::Retire;
        };
        let Some(pool) = self.origins.get_mut(&origin) else {
            return AgentRelease::Retire;
        };
        if pool.busy.remove(&resource).is_none() {
            return AgentRelease::Retire;
        }
        if !reusable {
            // Keep the resource-to-origin mapping until `closed()`. In particular, a request
            // queued while this socket was busy must be promoted when a response listener
            // destroys the socket before the response-end release reaches the transport.
            pool.retiring.insert(resource);
            return AgentRelease::Retire;
        }
        if let Some(request) = pool.waiting.pop_front() {
            pool.busy.insert(resource, request);
            AgentRelease::Dispatch { request }
        } else {
            pool.idle.push_back(resource);
            AgentRelease::Idle
        }
    }

    /// Whether this connection currently owns a request whose result is still observable.
    ///
    /// An idle keep-alive socket remains in the pool for reuse, but a peer closing that socket is
    /// not a request failure. Transport adapters use this distinction to suppress idle-close
    /// errors without hiding failures from a request that is still in flight.
    pub fn has_active_request(&self, resource: ResourceId) -> bool {
        self.resources
            .get(&resource)
            .and_then(|origin| self.origins.get(origin))
            .is_some_and(|pool| pool.busy.contains_key(&resource))
    }

    pub fn connect_failed(&mut self, origin: &HttpOrigin, failed_request: u64) -> AgentFailure {
        let Some(pool) = self.origins.get_mut(origin) else {
            return AgentFailure::Empty;
        };
        if !pool.connecting.remove(&failed_request) {
            return AgentFailure::Empty;
        }
        let Some(request) = pool.waiting.pop_front() else {
            return AgentFailure::Empty;
        };
        pool.connecting.insert(request);
        AgentFailure::Connect { request }
    }

    pub fn cancel_connect(&mut self, origin: &HttpOrigin, request: u64) -> AgentCancellation {
        let Some(pool) = self.origins.get_mut(origin) else {
            return AgentCancellation::Missing;
        };
        if let Some(position) = pool.waiting.iter().position(|queued| *queued == request) {
            pool.waiting.remove(position);
            return AgentCancellation::Waiting;
        }
        if !pool.connecting.remove(&request) {
            return AgentCancellation::Missing;
        }
        let next = pool.waiting.pop_front();
        if let Some(next) = next {
            pool.connecting.insert(next);
        }
        AgentCancellation::Connecting { next }
    }

    pub fn closed(&mut self, resource: ResourceId) -> Option<u64> {
        let origin = self.resources.remove(&resource)?;
        let pool = self.origins.get_mut(&origin)?;
        pool.idle.retain(|candidate| *candidate != resource);
        pool.busy.remove(&resource);
        pool.retiring.remove(&resource);
        let request = pool.waiting.pop_front();
        if let Some(request) = request {
            pool.connecting.insert(request);
        }
        request
    }
}

impl Default for HttpAgent {
    fn default() -> Self {
        Self::new(usize::MAX)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_origin_reuses_idle_connections_and_preserves_waiter_order() {
        let mut agent = HttpAgent::new(1);
        let origin = HttpOrigin::http("EXAMPLE.com", 80);
        let resource = ResourceId::new(7);

        assert_eq!(agent.acquire(origin.clone(), 1), AgentDecision::Connect);
        assert_eq!(agent.acquire(origin.clone(), 2), AgentDecision::Queued);
        assert!(agent.connected(origin.clone(), resource, 1));
        assert_eq!(
            agent.release(resource, true),
            AgentRelease::Dispatch { request: 2 }
        );
        assert!(agent.has_active_request(resource));
        assert_eq!(agent.release(resource, true), AgentRelease::Idle);
        assert!(!agent.has_active_request(resource));
        assert_eq!(agent.acquire(origin, 3), AgentDecision::Reuse(resource));
        assert!(agent.has_active_request(resource));
    }

    #[test]
    fn origins_never_share_connections() {
        let mut agent = HttpAgent::new(1);
        let first = HttpOrigin::http("a.example", 80);
        let second = HttpOrigin::http("b.example", 80);
        let resource = ResourceId::new(9);
        assert_eq!(agent.acquire(first.clone(), 1), AgentDecision::Connect);
        assert!(agent.connected(first, resource, 1));
        assert_eq!(agent.release(resource, true), AgentRelease::Idle);
        assert_eq!(agent.acquire(second, 2), AgentDecision::Connect);
    }

    #[test]
    fn non_reusable_and_closed_connections_are_retired() {
        let mut agent = HttpAgent::new(1);
        let origin = HttpOrigin::http("example.com", 80);
        let resource = ResourceId::new(11);
        assert_eq!(agent.acquire(origin.clone(), 1), AgentDecision::Connect);
        assert!(agent.connected(origin.clone(), resource, 1));
        assert_eq!(agent.acquire(origin, 2), AgentDecision::Queued);
        assert_eq!(agent.release(resource, false), AgentRelease::Retire);
        assert_eq!(agent.closed(resource), Some(2));
    }

    #[test]
    fn non_reusable_connection_promotes_waiter_only_after_close() {
        let mut agent = HttpAgent::new(1);
        let origin = HttpOrigin::http("example.com", 80);
        let resource = ResourceId::new(12);
        assert_eq!(agent.acquire(origin.clone(), 1), AgentDecision::Connect);
        assert!(agent.connected(origin.clone(), resource, 1));
        assert_eq!(agent.acquire(origin, 2), AgentDecision::Queued);
        assert_eq!(agent.release(resource, false), AgentRelease::Retire);
        assert_eq!(
            agent.acquire(HttpOrigin::http("example.com", 80), 3),
            AgentDecision::Queued
        );
        assert_eq!(agent.closed(resource), Some(2));
        assert_eq!(
            agent.connect_failed(&HttpOrigin::http("example.com", 80), 2),
            AgentFailure::Connect { request: 3 }
        );
        assert_eq!(
            agent.acquire(HttpOrigin::http("example.com", 80), 4),
            AgentDecision::Queued
        );
    }

    #[test]
    fn a_failed_connect_promotes_the_oldest_waiter_to_retry() {
        let mut agent = HttpAgent::new(1);
        let origin = HttpOrigin::http("example.com", 80);
        assert_eq!(agent.acquire(origin.clone(), 1), AgentDecision::Connect);
        assert_eq!(agent.acquire(origin.clone(), 2), AgentDecision::Queued);
        assert_eq!(agent.acquire(origin.clone(), 3), AgentDecision::Queued);
        assert_eq!(
            agent.connect_failed(&origin, 1),
            AgentFailure::Connect { request: 2 }
        );
        assert_eq!(agent.acquire(origin, 4), AgentDecision::Queued);
    }

    #[test]
    fn canceling_a_connect_retires_its_slot_and_promotes_the_oldest_waiter() {
        let mut agent = HttpAgent::new(1);
        let origin = HttpOrigin::http("example.com", 80);
        assert_eq!(agent.acquire(origin.clone(), 1), AgentDecision::Connect);
        assert_eq!(agent.acquire(origin.clone(), 2), AgentDecision::Queued);
        assert_eq!(agent.acquire(origin.clone(), 3), AgentDecision::Queued);
        assert_eq!(
            agent.cancel_connect(&origin, 1),
            AgentCancellation::Connecting { next: Some(2) }
        );
        assert_eq!(agent.cancel_connect(&origin, 3), AgentCancellation::Waiting);
        assert_eq!(agent.connect_failed(&origin, 2), AgentFailure::Empty);
        assert_eq!(agent.acquire(origin, 4), AgentDecision::Connect);
    }

    #[test]
    fn a_closed_connection_promotes_the_oldest_waiter_to_retry() {
        let mut agent = HttpAgent::new(1);
        let origin = HttpOrigin::http("example.com", 80);
        let resource = ResourceId::new(12);
        assert_eq!(agent.acquire(origin.clone(), 1), AgentDecision::Connect);
        assert!(agent.connected(origin.clone(), resource, 1));
        assert_eq!(agent.acquire(origin.clone(), 2), AgentDecision::Queued);
        assert_eq!(agent.closed(resource), Some(2));
        assert_eq!(agent.acquire(origin, 3), AgentDecision::Queued);
    }

    #[test]
    fn capacity_allows_parallel_connects_before_queueing() {
        let mut agent = HttpAgent::new(2);
        let origin = HttpOrigin::http("example.com", 80);
        assert_eq!(agent.acquire(origin.clone(), 1), AgentDecision::Connect);
        assert_eq!(agent.acquire(origin.clone(), 2), AgentDecision::Connect);
        assert_eq!(agent.acquire(origin, 3), AgentDecision::Queued);
    }
}
