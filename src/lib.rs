//! Akka-style actor library for Rust, built on tokio.
//!
//! Multiple actor types share a single [`ActorSystem`]; addresses route
//! messages with compile-time message-type checks (`send::<T>` only accepts
//! `T::Message`). Each actor runs on its own task with a bounded or
//! unbounded mailbox, an [`ErrorHandling`] policy, and standard lifecycle
//! hooks ([`pre_start`], [`pre_restart`], [`post_stop`], [`post_restart`]).
//!
//! The optional `multi-node` feature extends the same API across processes
//! via a [xanq](https://crates.io/crates/xanq) broker — see the
//! `inter_node` module and the project wiki for setup details.
//!
//! See the [project wiki](https://xanthorrhizol.github.io/actor/) for the
//! full guide.
//!
//! [`pre_start`]: actor::Actor::pre_start
//! [`pre_restart`]: actor::Actor::pre_restart
//! [`post_stop`]: actor::Actor::post_stop
//! [`post_restart`]: actor::Actor::post_restart

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

/// Default per-actor mailbox capacity for the `bounded-channel` feature.
/// Used whenever `Actor::register`'s `channel_size` argument is `None`.
#[cfg(feature = "bounded-channel")]
pub(crate) const CHANNEL_SIZE: usize = 4096;

#[macro_use]
extern crate log;

/// Lifecycle state of a registered actor as tracked by `actor_system_loop`.
///
/// Drives which lifecycle hook fires next and whether `FindActor` reports
/// the actor as ready for delivery:
///
/// - `Starting`/`Restarting` → mailbox exists but `FindActor` returns
///   `ready=false`; the system loop reports the actor isn't ready yet.
/// - `Receiving` → normal operation; sends go through immediately.
/// - `Stopping`/`Terminated` → terminal phase; the actor task is winding
///   down or finished. Subsequent sends fail.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LifeCycle {
    /// Initial state right after `Register` is accepted but before
    /// `pre_start` finishes. Sends are queued (`ready=false`).
    Starting,
    /// Active state. `pre_start` (or `post_restart`) has returned and the
    /// actor is consuming messages from its mailbox.
    Receiving,
    /// Transitional state entered when the actor receives a kill or
    /// restart signal, or its handler returns `Err` under `ErrorHandling::Stop`.
    /// `post_stop` is about to run.
    Stopping,
    /// Final state after `post_stop`; the actor's task has exited.
    Terminated,
    /// Transitional state between `Stopping` and the next `Starting`, set
    /// when the actor is restarting (handler error under `ErrorHandling::Restart`
    /// or an explicit `restart` call). `pre_restart` runs here.
    Restarting,
}

/// Policy applied when an actor's `handle` returns `Err`.
///
/// Passed to `Actor::register` and consulted on every handler error.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorHandling {
    /// Log the error and keep the actor running on the same mailbox.
    /// The failed message is dropped; the next message is processed normally.
    Resume,
    /// Tear down the actor's mailbox, run `pre_restart` / `post_restart`,
    /// and re-enter the receive loop with a fresh mailbox at the same
    /// address.
    Restart,
    /// Run `post_stop` and terminate the actor. Subsequent sends fail.
    Stop,
}

/// Choice of tokio task primitive used to host the actor's receive loop.
///
/// Picked per actor at `register` time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Blocking {
    /// Use `tokio::task::spawn_blocking` + `block_on`. Gives the actor a
    /// dedicated OS thread, so a synchronous-looking handler body that
    /// blocks for a while won't starve the async runtime.
    Blocking,
    /// Use `tokio::spawn`. Cheapest option; the actor cooperates with
    /// the async runtime and should not block.
    NonBlocking,
}

/// Snapshot status for a running job (returned only via `JobController`'s
/// internal channels — currently exposed for completeness; the public API
/// uses `abort_job` / `stop_job` / `resume_job` directly).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JobStatus {
    /// The job loop is actively iterating (between sleeps and dispatches).
    Running,
    /// The job loop is paused after a `stop_job` and waiting for `resume_job`.
    Stopped,
}

/// Bound on `Actor::Message` / `Actor::Result`.
///
/// - Without `multi-node`: vacuous (`impl<T: ?Sized> MaybeCodec for T`),
///   every type satisfies it — single-node users see no extra constraint.
/// - With `multi-node`: a supertrait alias for `xancode::Codec`, so any
///   message/result that travels through `send` / `send_and_recv` /
///   `run_job` must be serializable. Local routing still skips encoding,
///   but the bound is enforced at compile time regardless.
///
/// Implemented automatically via blanket impl; you should never need to
/// implement it manually.
#[cfg(not(feature = "multi-node"))]
pub trait MaybeCodec {}
#[cfg(not(feature = "multi-node"))]
impl<T: ?Sized> MaybeCodec for T {}

#[cfg(feature = "multi-node")]
pub trait MaybeCodec: xancode::Codec {}
#[cfg(feature = "multi-node")]
impl<T: xancode::Codec> MaybeCodec for T {}
