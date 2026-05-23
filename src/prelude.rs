//! Convenience re-exports for typical actor application code.
//!
//! Drop a `use xan_actor::prelude::*;` into your binary/crate root and
//! everything you need to define actors, run them, and address them is in
//! scope. Advanced types (`Mailbox`, `Message`, the inter-node decoder
//! helpers) remain accessible at their canonical paths.

pub use crate::actor::Actor;
pub use crate::actor_system::ActorSystem;
pub use crate::error::ActorError;
pub use crate::types::{JobSpec, RunJobResult};
pub use crate::{Blocking, ErrorHandling};

// `Message<T>` is the internal channel envelope wrapping `<T as Actor>::Message`
// (the user's own associated type). Users never construct it; pulling it into
// the prelude only invites confusion with `Self::Message`. It stays exported
// at `xan_actor::Message` for the rare advanced use.

#[cfg(feature = "multi-node")]
pub use crate::inter_node::{Address, NodeFilter, Topic};
