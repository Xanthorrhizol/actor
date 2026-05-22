pub mod actor;
pub mod actor_system;
mod error;
#[cfg(feature = "multi-node")]
pub mod inter_node;
pub mod prelude;
#[cfg(test)]
mod test;
pub mod types;

pub use actor::*;
pub use actor_system::*;
pub use error::ActorError;
pub use types::{JobController, JobSpec, Message, RunJobResult};
pub(crate) use types::{Mailbox, TypedMailbox};

#[cfg(feature = "bounded-channel")]
pub(crate) const CHANNEL_SIZE: usize = 4096;

#[macro_use]
extern crate log;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// Represents the lifecycle of an actor
pub enum LifeCycle {
    Starting,
    Receiving,
    Stopping,
    Terminated,
    Restarting,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// Behavior of an actor on error
pub enum ErrorHandling {
    Resume,
    Restart,
    Stop,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// Blocking or Non-blocking
pub enum Blocking {
    Blocking,
    NonBlocking,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// Job status
pub enum JobStatus {
    Running,
    Stopped,
}

/// Bound on `Actor::Message` / `Actor::Result`.
///
/// - Without `multi-node`: vacuous, every type satisfies it.
/// - With `multi-node`: requires `xancode::Codec` so the type can be
///   serialized across nodes.
#[cfg(not(feature = "multi-node"))]
pub trait MaybeCodec {}
#[cfg(not(feature = "multi-node"))]
impl<T: ?Sized> MaybeCodec for T {}

#[cfg(feature = "multi-node")]
pub trait MaybeCodec: xancode::Codec {}
#[cfg(feature = "multi-node")]
impl<T: xancode::Codec> MaybeCodec for T {}
