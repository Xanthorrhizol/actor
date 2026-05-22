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
