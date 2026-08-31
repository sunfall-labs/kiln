//! Provider-neutral HTTP Agent state.
//!
//! The Agent chooses which origin owns a connection and when that connection may accept another
//! request. It owns no socket, Tokio task, Hyper sender, or JavaScript value; bindings translate
//! these decisions into their existing resource and request identities.

use std::collections::{BTreeMap, VecDeque};

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

#[derive(Debug, Default)]
pub struct HttpAgent {
    origins: BTreeMap<HttpOrigin, OriginPool>,
    resources: BTreeMap<ResourceId, HttpOrigin>,
}

#[derive(Debug, Default)]
struct OriginPool {
    connecting: bool,
    idle: VecDeque<ResourceId>,
    busy: BTreeMap<ResourceId, u64>,
    waiting: VecDeque<u64>,
}

impl HttpAgent {
    pub fn acquire(&mut self, origin: HttpOrigin, request: u64) -> AgentDecision {
        let pool = self.origins.entry(origin).or_default();
        if let Some(resource) = pool.idle.pop_front() {
            pool.busy.insert(resource, request);
            return AgentDecision::Reuse(resource);
        }
        if pool.connecting || !pool.busy.is_empty() {
            pool.waiting.push_back(request);
            return AgentDecision::Queued;
        }
        pool.connecting = true;
        AgentDecision::Connect
    }

    pub fn connected(&mut self, origin: HttpOrigin, resource: ResourceId, request: u64) -> bool {
        let pool = self.origins.entry(origin.clone()).or_default();
        if !pool.connecting || self.resources.contains_key(&resource) {
            return false;
        }
        pool.connecting = false;
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
        if pool.busy.remove(&resource).is_none() || !reusable {
            self.resources.remove(&resource);
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

    pub fn connect_failed(&mut self, origin: &HttpOrigin) -> AgentFailure {
        let Some(pool) = self.origins.get_mut(origin) else {
            return AgentFailure::Empty;
        };
        pool.connecting = false;
        let Some(request) = pool.waiting.pop_front() else {
            return AgentFailure::Empty;
        };
        pool.connecting = true;
        AgentFailure::Connect { request }
    }

    pub fn closed(&mut self, resource: ResourceId) -> Option<u64> {
        let origin = self.resources.remove(&resource)?;
        let pool = self.origins.get_mut(&origin)?;
        pool.idle.retain(|candidate| *candidate != resource);
        pool.busy.remove(&resource);
        pool.waiting.pop_front()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_origin_reuses_idle_connections_and_preserves_waiter_order() {
        let mut agent = HttpAgent::default();
        let origin = HttpOrigin::http("EXAMPLE.com", 80);
        let resource = ResourceId::new(7);

        assert_eq!(agent.acquire(origin.clone(), 1), AgentDecision::Connect);
        assert_eq!(agent.acquire(origin.clone(), 2), AgentDecision::Queued);
        assert!(agent.connected(origin.clone(), resource, 1));
        assert_eq!(
            agent.release(resource, true),
            AgentRelease::Dispatch { request: 2 }
        );
        assert_eq!(agent.release(resource, true), AgentRelease::Idle);
        assert_eq!(agent.acquire(origin, 3), AgentDecision::Reuse(resource));
    }

    #[test]
    fn origins_never_share_connections() {
        let mut agent = HttpAgent::default();
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
        let mut agent = HttpAgent::default();
        let origin = HttpOrigin::http("example.com", 80);
        let resource = ResourceId::new(11);
        assert_eq!(agent.acquire(origin.clone(), 1), AgentDecision::Connect);
        assert!(agent.connected(origin.clone(), resource, 1));
        assert_eq!(agent.acquire(origin, 2), AgentDecision::Queued);
        assert_eq!(agent.release(resource, false), AgentRelease::Retire);
        assert_eq!(agent.closed(resource), None);
    }

    #[test]
    fn a_failed_connect_promotes_the_oldest_waiter_to_retry() {
        let mut agent = HttpAgent::default();
        let origin = HttpOrigin::http("example.com", 80);
        assert_eq!(agent.acquire(origin.clone(), 1), AgentDecision::Connect);
        assert_eq!(agent.acquire(origin.clone(), 2), AgentDecision::Queued);
        assert_eq!(agent.acquire(origin.clone(), 3), AgentDecision::Queued);
        assert_eq!(
            agent.connect_failed(&origin),
            AgentFailure::Connect { request: 2 }
        );
        assert_eq!(agent.acquire(origin, 4), AgentDecision::Queued);
    }
}
