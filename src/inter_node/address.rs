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
    /// Logical actor name within its owning node (e.g. `"/echo/1"`).
    /// Used as the key in the local actor map.
    pub name: String,
    /// Name of the node that owns this actor. Must match the registering
    /// `ActorSystem`'s `node_name`; sends compare against this to decide
    /// local vs. remote routing.
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
    /// One entry per local actor whose name matched the regex.
    /// `local.len()` is the exact number of local actors that received
    /// the message; each `Result` is the outcome of the in-process
    /// mailbox send.
    pub local: Vec<Result<(), crate::ActorError>>,
    /// One entry per remote peer node we sent a `BroadcastFire` envelope
    /// to. `Ok(())` means the broker accepted the produce.
    ///
    /// `remote.len()` is the number of peer nodes addressed, **not** the
    /// number of remote actors that received the message — `BroadcastFire`
    /// is fire-and-forget on the receiver side, so per-actor confirmation
    /// never comes back.
    pub remote: Vec<Result<(), crate::ActorError>>,
}

impl BroadcastResult {
    /// True iff every local mailbox send AND every remote produce succeeded.
    /// Note: this only checks envelope acceptance for remote entries, not
    /// whether any remote actors actually matched the regex.
    pub fn all_ok(&self) -> bool {
        self.local.iter().all(|r| r.is_ok()) && self.remote.iter().all(|r| r.is_ok())
    }

    /// Iterate over `local` followed by `remote`. Convenient for
    /// `results.iter().all(|r| r.is_ok())`-style checks when you don't
    /// care about the source. Beware: the entries you're iterating over
    /// have different meanings (per-actor vs. per-peer); use `all_ok()` or
    /// inspect the two `Vec`s separately when the distinction matters.
    pub fn iter(&self) -> impl Iterator<Item = &Result<(), crate::ActorError>> {
        self.local.iter().chain(self.remote.iter())
    }
}

/// Selects which set of nodes a broadcast should fan out to.
///
/// Passed to `send_broadcast` / `send_broadcast_without_tx_cache` (multi-node
/// only). The result shape is [`BroadcastResult`] — see its docs for what
/// `local.len()` vs. `remote.len()` actually mean.
///
/// Duplicates in `Peers` are deduped; self-node appearing in any variant
/// runs as a local fan-out (no broker round trip).
#[derive(Debug, Clone)]
pub enum NodeFilter {
    /// Match `name_regex` against this node's local actors only. No
    /// broker traffic. Equivalent to `Node(<self>)`.
    SelfOnly,
    /// Match against a single named node. If `name` equals the caller's
    /// own `node_name`, behaves like `SelfOnly`; otherwise sends one
    /// `BroadcastFire` envelope to that peer.
    Node(String),
    /// Match against the union of the listed nodes. Self-node entries
    /// produce local fan-out; every other distinct peer gets one
    /// `BroadcastFire` envelope. Duplicates are deduped.
    Peers(Vec<String>),
}
