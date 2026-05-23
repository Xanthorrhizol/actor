use xancode::Codec;
use xanq::address::{Address as XanqAddress, DeliveryMode};

/// Distinguishes the two per-node xanq topics each `ActorSystem`
/// subscribes to.
#[derive(Codec, Debug, Clone, Copy, PartialEq, Eq)]
pub enum TopicKind {
    /// Incoming work for the owning node: `Fire`, `Call`, `BroadcastFire`.
    Request,
    /// Incoming replies addressed to the owning node — produced by peers
    /// fulfilling a `Call`'s `reply_to`.
    Response,
}

/// xanq topic used by inter-node delivery.
///
/// One pair of `Anycast` topics per node: one for incoming requests, one for
/// incoming responses. There is no shared discovery topic — node membership
/// is left to the caller (via `NodeFilter::Peers`).
///
/// `Anycast` means each message is delivered to exactly one subscriber.
/// In typical deployments only the owning `ActorSystem` subscribes to its
/// own `Topic::request(self)` / `Topic::response(self)`, so it behaves
/// like unicast. Multiple subscribers on the same `(node, kind)` topic
/// would load-balance the traffic.
#[derive(Codec, Debug, Clone)]
pub struct Topic {
    /// The node name this topic belongs to.
    pub node: String,
    /// Whether this is the request-side or response-side topic.
    pub kind: TopicKind,
}

impl Topic {
    /// The incoming-request topic for `node`. Published-to by peers
    /// sending `Fire` / `Call` / `BroadcastFire` envelopes; subscribed-to
    /// by `node`'s own `InterNodeRuntime`.
    pub fn request(node: impl Into<String>) -> Self {
        Self {
            node: node.into(),
            kind: TopicKind::Request,
        }
    }

    /// The incoming-response topic for `node`. Published-to by peers
    /// fulfilling a `Call`'s `reply_to`; subscribed-to by `node`'s own
    /// `InterNodeRuntime`.
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

/// Wire envelope for a single inter-node request.
///
/// Published to `Topic::request(target_node)`; received and dispatched by
/// that node's `InterNodeRuntime::start_consumers`'s request consumer.
#[derive(Codec, Debug, Clone)]
pub enum InterNodeMessage {
    /// Fire-and-forget single-actor send. Receiver decodes `payload` via
    /// the registry, then calls `dispatch_local_any(actor_type, target_name, _)`.
    Fire {
        /// Fully-qualified Rust type name (`std::any::type_name::<T>()`)
        /// used to look up the decoder in the inventory registry.
        actor_type: String,
        /// Name part of the target address (the node part is implicit from
        /// the request topic this envelope was published on).
        target_name: String,
        /// `<T as Actor>::Message` encoded via `xancode::Codec`.
        payload: Vec<u8>,
    },
    /// Request/response single-actor send. Receiver decodes, dispatches,
    /// encodes the result, and publishes an [`InterNodeResponse`] to
    /// `reply_to` carrying `req_id`.
    Call {
        actor_type: String,
        target_name: String,
        /// Where to publish the response. Always `Topic::response(<caller_node>)`.
        reply_to: Topic,
        /// Sender-local counter used by the caller's pending-requests map
        /// to match the response back to the awaiting `send_and_recv`.
        req_id: u64,
        payload: Vec<u8>,
    },
    /// Ask the receiving node to fan out a fire-and-forget broadcast across
    /// every locally registered actor whose name matches `name_regex`.
    /// No response is sent back; the caller only learns whether the
    /// envelope itself was accepted by the broker (via `BroadcastResult::remote`).
    BroadcastFire {
        actor_type: String,
        /// `*`-wildcard pattern; resolved by `filter_address` on the
        /// receiver.
        name_regex: String,
        payload: Vec<u8>,
    },
}

/// Wire envelope for a single inter-node response (reply to a `Call`).
/// Published to the `reply_to` topic carried by the original `Call`.
#[derive(Codec, Debug, Clone)]
pub struct InterNodeResponse {
    /// Matches the `req_id` from the originating `Call`; used by the
    /// caller's pending-requests map to wake the awaiting `send_and_recv`.
    pub req_id: u64,
    /// Either the encoded `<T as Actor>::Result` bytes (`Ok`) or a
    /// stringified error from the receiving side (`Err`).
    pub outcome: ResponseOutcome,
}

/// Two-variant outcome payload inside `InterNodeResponse`.
///
/// Modeled as an enum instead of `Result<Vec<u8>, String>` because
/// xancode's `Codec` derive doesn't currently support `Result`.
#[derive(Codec, Debug, Clone)]
pub enum ResponseOutcome {
    /// Receiver's handler returned `Ok(result)`; the inner bytes are
    /// `<T as Actor>::Result` encoded via `xancode::Codec`.
    Ok(Vec<u8>),
    /// Receiver could not produce a result. The string is the receiver's
    /// `ActorError`/handler-error `Display` for diagnostics; resurfaced
    /// to the caller as `ActorError::InterNodeRemote`.
    Err(String),
}
