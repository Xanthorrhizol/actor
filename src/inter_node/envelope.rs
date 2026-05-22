use xancode::Codec;
use xanq::address::{Address as XanqAddress, DeliveryMode};

#[derive(Codec, Debug, Clone, Copy, PartialEq, Eq)]
pub enum TopicKind {
    Request,
    Response,
}

/// xanq topic used by inter-node delivery.
///
/// One pair of `Anycast` topics per node: one for incoming requests, one for
/// incoming responses. There is no shared discovery topic — node membership
/// is left to the caller (via `NodeFilter::Peers`).
#[derive(Codec, Debug, Clone)]
pub struct Topic {
    pub node: String,
    pub kind: TopicKind,
}

impl Topic {
    pub fn request(node: impl Into<String>) -> Self {
        Self {
            node: node.into(),
            kind: TopicKind::Request,
        }
    }

    pub fn response(node: impl Into<String>) -> Self {
        Self {
            node: node.into(),
            kind: TopicKind::Response,
        }
    }
}

impl XanqAddress for Topic {
    fn delivery_mode(&self) -> DeliveryMode {
        DeliveryMode::Anycast
    }
}

#[derive(Codec, Debug, Clone)]
pub enum InterNodeMessage {
    Fire {
        actor_type: String,
        /// Name part of the target address (the node part is implicit from
        /// the request topic this envelope was published on).
        target_name: String,
        payload: Vec<u8>,
    },
    Call {
        actor_type: String,
        target_name: String,
        reply_to: Topic,
        req_id: u64,
        payload: Vec<u8>,
    },
    /// Ask the receiving node to fan out a fire-and-forget broadcast across
    /// every locally registered actor whose name matches `name_regex`.
    BroadcastFire {
        actor_type: String,
        name_regex: String,
        payload: Vec<u8>,
    },
}

#[derive(Codec, Debug, Clone)]
pub struct InterNodeResponse {
    pub req_id: u64,
    pub outcome: ResponseOutcome,
}

#[derive(Codec, Debug, Clone)]
pub enum ResponseOutcome {
    Ok(Vec<u8>),
    Err(String),
}
