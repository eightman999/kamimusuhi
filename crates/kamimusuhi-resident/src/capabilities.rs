//! Capability advertisement and discovery for node tasks.
//!
//! A node's *capabilities* are the task types it can execute locally — not
//! the providers it can reach through its own router. A Pi that routes
//! chat through llm_master's `/v1/chat/completions` does not advertise
//! `llm.generate`; llm_master, whose tier is `node_local`, does. That
//! distinction is what makes a task delegated to a node run on that node
//! instead of bouncing back through the sender's own fallback.
//!
//! Derived from the config plus `node.capabilities` extras. A capability is
//! advertised only when the node has a real `/task` executor for it:
//!
//! - `node.ping`, `state.read` — every node
//! - `llm.generate` — a `node_local` tier exists (routed locally via the
//!   resident router)
//!
//! `node.capabilities` is the operator's way to declare executors the
//! derived set cannot see (a vLLM sidecar, a vision adapter). Declaring a
//! capability no executor serves makes tasks fail with
//! `unsupported_capability` — declare only what runs.
//!
//! Peers advertise theirs through `/health` and `/capabilities`; the live
//! probe result overrides the static `peers[].capabilities` declaration,
//! which exists so a task can *queue* for a peer that is currently down
//! instead of being rejected as unservable.

use std::collections::BTreeSet;

use kamimusuhi_node::caps;

use crate::config::Config;
use crate::state::Shared;

/// Task-type prefixes that are always executed on the node that received
/// them — heartbeat, status reads and queue management never delegate.
pub const LOCAL_PREFIXES: &[&str] = &["node.", "state.", "queue.", "heartbeat"];

/// Capability ids this node can execute locally.
pub fn local(config: &Config) -> BTreeSet<String> {
    let mut set: BTreeSet<String> = config
        .node
        .capabilities
        .iter()
        .filter(|c| kamimusuhi_node::valid_id(c))
        .cloned()
        .collect();
    set.insert(caps::NODE_PING.to_owned());
    set.insert(caps::STATE_READ.to_owned());
    if config.tiers.iter().any(|t| t.node_local) {
        set.insert(caps::LLM_GENERATE.to_owned());
    }
    set
}

/// Whether the task type is inherently node-local (heartbeat, state and
/// queue work per the light-node contract).
pub fn is_local_type(task_type: &str) -> bool {
    LOCAL_PREFIXES.iter().any(|p| task_type.starts_with(p))
}

/// The node's public capability document (`GET /capabilities`).
pub fn doc(shared: &Shared) -> kamimusuhi_node::CapabilityDoc {
    kamimusuhi_node::CapabilityDoc {
        node_id: shared.config.node.id.clone(),
        role: shared.config.node.role.as_str().to_owned(),
        mode: mode(&shared.config),
        capabilities: local(&shared.config).into_iter().collect(),
    }
}

/// `continuity` nodes are the light end of the split; cognition/executor
/// nodes are the heavy end. The same daemon runs both — mode is a role,
/// not a build.
pub fn mode(config: &Config) -> kamimusuhi_node::NodeMode {
    match config.node.role {
        crate::config::NodeRole::Continuity => kamimusuhi_node::NodeMode::Light,
        _ => kamimusuhi_node::NodeMode::Heavy,
    }
}

/// Capabilities currently attributed to `peer`: the probed `/health`
/// document's list when one has been seen, else the static declaration.
/// Returns `None` when neither exists (peer does not participate in the
/// task protocol or is older than it).
pub fn of_peer(shared: &Shared, peer: &crate::config::PeerConfig) -> Option<Vec<String>> {
    if let Some(probe) = shared
        .peers
        .read()
        .unwrap_or_else(|p| p.into_inner())
        .get(&peer.id)
        && let Some(list) = probe.detail.get("capabilities").and_then(|v| v.as_array())
    {
        return Some(
            list.iter()
                .filter_map(|c| c.as_str().map(str::to_owned))
                .collect(),
        );
    }
    (!peer.capabilities.is_empty()).then(|| peer.capabilities.clone())
}

/// Whether `caps` satisfies every entry of `requires` (an empty `requires`
/// is satisfied by any node — the task type itself is the requirement).
pub fn satisfy(caps: &[String], requires: &[String]) -> bool {
    let set: BTreeSet<&str> = caps.iter().map(String::as_str).collect();
    requires.iter().all(|r| set.contains(r.as_str()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    fn config(v: Value) -> Config {
        let c: Config = serde_json::from_value(v).expect("config");
        c.validate().expect("valid");
        c
    }

    #[test]
    fn a_bare_node_pings_and_reads_state() {
        let c = config(json!({
            "node": {"id": "pi", "role": "continuity", "listen": "127.0.0.1:0"},
            "paths": {"root": "/tmp/x"},
        }));
        let caps = local(&c);
        assert!(caps.contains(caps::NODE_PING));
        assert!(caps.contains(caps::STATE_READ));
        assert!(!caps.contains(caps::LLM_GENERATE));
    }

    #[test]
    fn a_node_local_tier_advertises_llm_generate() {
        let c = config(json!({
            "node": {"id": "gpu", "role": "cognition", "listen": "127.0.0.1:0"},
            "paths": {"root": "/tmp/x"},
            "tiers": [{"name": "local", "base_url": "http://127.0.0.1:1/v1",
                       "model": "m", "billing": "local", "node_local": true}],
        }));
        assert!(local(&c).contains(caps::LLM_GENERATE));
        // A remote tier alone does not: routing through it is not local
        // execution capacity.
        let c = config(json!({
            "node": {"id": "pi", "role": "continuity", "listen": "127.0.0.1:0"},
            "paths": {"root": "/tmp/x"},
            "tiers": [{"name": "hai", "base_url": "https://x/v1",
                       "model": "m", "billing": "subscription"}],
        }));
        assert!(!local(&c).contains(caps::LLM_GENERATE));
    }

    #[test]
    fn configured_extras_merge_in() {
        let c = config(json!({
            "node": {"id": "gpu", "role": "cognition", "listen": "127.0.0.1:0",
                     "capabilities": ["vision", "embedding"]},
            "paths": {"root": "/tmp/x"},
        }));
        let caps = local(&c);
        assert!(caps.contains("vision"));
        assert!(caps.contains("embedding"));
    }

    #[test]
    fn local_types_never_delegate() {
        assert!(is_local_type("node.ping"));
        assert!(is_local_type("state.read"));
        assert!(is_local_type("heartbeat"));
        assert!(!is_local_type("llm.generate"));
    }

    #[test]
    fn requirements_must_all_be_met() {
        let caps = vec!["llm.generate".to_owned(), "vision".to_owned()];
        assert!(satisfy(&caps, &[]));
        assert!(satisfy(&caps, &["llm.generate".to_owned()]));
        assert!(!satisfy(&caps, &["embedding".to_owned()]));
    }
}
