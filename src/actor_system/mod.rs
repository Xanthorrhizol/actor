#[cfg(feature = "bounded-channel")]
pub mod actor_system_bounded;
#[cfg(feature = "unbounded-channel")]
pub mod actor_system_unbounded;

#[cfg(feature = "unbounded-channel")]
pub use actor_system_unbounded::{ActorSystem, ActorSystemCmd};

#[cfg(feature = "bounded-channel")]
pub use actor_system_bounded::{ActorSystem, ActorSystemCmd};
