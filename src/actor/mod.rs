#[cfg(feature = "bounded-channel")]
pub mod actor_bounded;
#[cfg(feature = "unbounded-channel")]
pub mod actor_unbounded;
#[cfg(feature = "bounded-channel")]
pub use actor_bounded::Actor;
#[cfg(feature = "unbounded-channel")]
pub use actor_unbounded::Actor;
