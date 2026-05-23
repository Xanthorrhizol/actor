use xancode::Codec;
use xanq::address;

/// Fully-qualified actor address: a logical `name` paired with the `node`
/// that owns it.
///
/// Cross-node uniqueness is structural — two nodes physically cannot hold
/// the same `Address` because their `node` fields differ. Per-node uniqueness
/// (two registrations on the same node with the same name) is enforced by
/// `actor_system_loop`'s local check.
#[derive(Debug, Clone, Eq, PartialEq, Hash, Codec)]
pub struct Address {
    pub name: String,
    pub node: String,
}

impl Address {
    pub fn new(node: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            node: node.into(),
        }
    }
}

impl std::fmt::Display for Address {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}", self.node, self.name)
    }
}

impl xanq::address::Address for Address {
    fn delivery_mode(&self) -> address::DeliveryMode {
        address::DeliveryMode::Anycast
    }
}

/// Outcome of a multi-node `send_broadcast`.
///
/// The two fields count different things — keep that in mind when you
/// inspect lengths:
///
/// - `local` — one entry **per matched local actor**. Each `Result` is the
///   outcome of the in-process mailbox send. `local.len()` is the exact
///   number of local actors that received the message.
///
/// - `remote` — one entry **per remote peer that we sent a `BroadcastFire`
///   envelope to**. `Ok(())` means the broker accepted the produce — the
///   receiver then runs its own regex match against its local actors and
///   dispatches to 0..N of them, but **fire-and-forget**, so no per-actor
///   confirmation comes back. `remote.len()` is the number of remote peer
///   nodes we shipped envelopes to, **not** the number of remote actors
///   that received the message.
///
/// In other words, the total number of actors that received the broadcast
/// is `local.len() + (unknown remote actor count)`. If you need an
/// accurate cluster-wide actor count, this fire-style broadcast can't give
/// it to you — that would require a request/response protocol where each
/// peer replies with its own match count.
#[derive(Debug, Default)]
pub struct BroadcastResult {
    pub local: Vec<Result<(), crate::ActorError>>,
    pub remote: Vec<Result<(), crate::ActorError>>,
}

impl BroadcastResult {
    /// True iff every local mailbox send AND every remote produce succeeded.
    pub fn all_ok(&self) -> bool {
        self.local.iter().all(|r| r.is_ok()) && self.remote.iter().all(|r| r.is_ok())
    }

    /// Iterate over `local` followed by `remote`. Convenient for
    /// `results.iter().all(|r| r.is_ok())`-style checks when you don't
    /// care about the source.
    pub fn iter(&self) -> impl Iterator<Item = &Result<(), crate::ActorError>> {
        self.local.iter().chain(self.remote.iter())
    }
}

/// Selects which set of nodes a broadcast should fan out to.
#[derive(Debug, Clone)]
pub enum NodeFilter {
    /// Match `name_regex` against this node's local actors only.
    SelfOnly,
    /// Match against a single named node (which may be self).
    Node(String),
    /// Match against the union of the listed nodes.
    Peers(Vec<String>),
}
