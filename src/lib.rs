mod error;
#[cfg(test)]
mod test;
pub mod tokio_actor;
pub mod types;

pub use crate::error::ActorError;
pub use crate::types::{JobSpec, Message};
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
