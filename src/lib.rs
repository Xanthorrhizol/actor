mod error;
#[cfg(feature = "std")]
pub mod std_actor;
#[cfg(feature = "tokio")]
pub mod tokio_actor;
pub mod types;

pub use crate::error::ActorError;
pub use crate::types::{JobSpec, Message};
#[cfg(feature = "std")]
pub use std_actor::*;
#[cfg(feature = "tokio")]
pub use tokio_actor::*;

#[macro_use]
extern crate log;

#[derive(Clone)]
pub enum LifeCycle {
    Starting,
    Receiving,
    Stopping,
    Terminated,
    Restarting,
}
