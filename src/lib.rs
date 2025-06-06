pub mod actor;
mod error;
#[cfg(test)]
mod test;
pub mod types;

pub use crate::error::ActorError;
pub use crate::types::{JobSpec, Message};
pub use actor::*;

#[macro_use]
extern crate log;

#[derive(Clone, Debug)]
pub enum LifeCycle {
    Starting,
    Receiving,
    Stopping,
    Terminated,
    Restarting,
}
