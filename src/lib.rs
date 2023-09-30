mod error;
#[cfg(feature = "sync")]
pub mod sync_actor;
#[cfg(feature = "tokio")]
pub mod tokio_actor;
pub mod types;

pub use crate::error::ActorError;
pub use crate::types::{JobSpec, Message};
#[cfg(feature = "sync")]
pub use sync_actor::*;
#[cfg(feature = "tokio")]
pub use tokio_actor::*;

#[macro_use]
pub extern crate log;

#[derive(Clone, Debug)]
pub enum LifeCycle {
    Starting,
    Receiving,
    Stopping,
    Terminated,
    Restarting,
}
