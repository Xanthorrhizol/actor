use thiserror::Error;

/// All errors surfaced by the actor system.
///
/// Variants gated on `multi-node` only exist when that feature is enabled.
/// Most user-facing call sites return `Result<_, ActorError>`; match on the
/// variant when you need to distinguish (e.g. retry on `ActorNotReady` vs.
/// give up on `AddressNotFound`).
#[derive(Error, Debug)]
pub enum ActorError {
    /// `oneshot::Receiver::await` failed (sender dropped before sending).
    /// Typically means the actor task died between accepting the message
    /// and producing a reply.
    #[error(transparent)]
    OneshotRecv(#[from] tokio::sync::oneshot::error::RecvError),
    /// Channel send failed — the receiver was dropped. Indicates the
    /// target mailbox/loop has shut down. Same variant for both the
    /// bounded and unbounded mpsc backends.
    #[error("Failed to send on channel: {0}")]
    ChannelSend(String),
    /// Channel `recv` returned `None` — all senders are gone. Same
    /// variant for both backends.
    #[error("Failed to recv from channel")]
    ChannelRecv,
    /// A timed `send_and_recv` (via
    /// [`ActorSystem::send_and_recv_with_timeout`] or
    /// [`ActorSystem::send_and_recv_without_tx_cache_with_timeout`]) did
    /// not receive its reply within the configured duration. Common
    /// cause in multi-node mode: the remote peer died or the broker
    /// dropped the in-flight request.
    ///
    /// [`ActorSystem::send_and_recv_with_timeout`]: crate::ActorSystem::send_and_recv_with_timeout
    /// [`ActorSystem::send_and_recv_without_tx_cache_with_timeout`]: crate::ActorSystem::send_and_recv_without_tx_cache_with_timeout
    #[error("Operation timed out after {0:?}")]
    Timeout(std::time::Duration),
    /// `address_regex` failed to compile (broadcast / restart / unregister).
    #[error(transparent)]
    AddressRegexError(#[from] regex::Error),

    /// Tried to register an actor at an address that's already taken on
    /// this node (and not a restart). Re-registration with the same
    /// address fails until the existing entry is unregistered.
    #[error("Address {0} already exists")]
    AddressAlreadyExist(String),
    /// `send` / `send_and_recv` / `run_job` could not find the address in
    /// the local actor map. With `multi-node` this is the local-side
    /// failure mode after the routing check decides the call is local.
    #[error("Address {0} not found")]
    AddressNotFound(String),
    /// The address exists but the actor's lifecycle is not yet `Receiving`
    /// after the retry budget (10 × 100 ms). Usually means the actor is
    /// still in `Starting` / `Restarting`.
    #[error("Actor that's address is {0} not ready")]
    ActorNotReady(String),
    /// Reserved for cases where cloning an actor state fails. Currently
    /// not produced by the runtime.
    #[error("Actor clone failed: {0}")]
    CloneFailed(String),
    /// `Actor::register` could not reach `actor_system_loop` after 10
    /// retries — the system is wedged or shutting down.
    #[error("Unhealthy ActorSystem")]
    UnhealthyActorSystem,
    /// Downcast from `Arc<dyn Any>` / `Box<dyn Any>` to the expected
    /// concrete message or result type failed. Usually means a `send::<T>`
    /// (or `send_and_recv::<T>`) was routed to an address whose actor type
    /// doesn't match `T`.
    #[error("Message type mismatch")]
    MessageTypeMismatch,

    /// Underlying xanq client IO failed (e.g. broker disconnected, TCP
    /// reset, broker connect timed out).
    #[cfg(feature = "multi-node")]
    #[error("Inter-node IO error: {0}")]
    InterNodeIo(String),
    /// `xancode::Codec::decode` failed when turning bytes back into
    /// `<T as Actor>::Message` (receiver side) or `<T as Actor>::Result`
    /// (caller side).
    #[cfg(feature = "multi-node")]
    #[error("Inter-node decode error: {0}")]
    InterNodeDecode(String),
    /// The receiving node got an envelope for an actor type with no
    /// matching `register_for_inter_node!` entry in its inventory
    /// registry. Fix by adding the macro call at module scope on both
    /// nodes.
    #[cfg(feature = "multi-node")]
    #[error("Inter-node decoder not registered for actor type {0}")]
    InterNodeDecoderMissing(String),
    /// The peer node responded with `ResponseOutcome::Err`, or the
    /// pending-response channel was dropped (peer crashed mid-call). The
    /// payload is the original error string from the peer.
    #[cfg(feature = "multi-node")]
    #[error("Inter-node remote error: {0}")]
    InterNodeRemote(String),
    /// Attempted a cross-node send on an `ActorSystem` created with
    /// `broker_addr = None`. Provide a broker address (or keep all
    /// addresses on the local node).
    #[cfg(feature = "multi-node")]
    #[error("Inter-node not configured")]
    InterNodeNotConfigured,
    /// `Actor::register` was called with `address.node` not equal to the
    /// system's own `node_name`. Each node can only host actors whose
    /// addresses claim that node.
    #[cfg(feature = "multi-node")]
    #[error("Address {0} is not owned by this node")]
    AddressNotOwned(String),
}
