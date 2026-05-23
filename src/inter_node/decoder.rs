//! Cross-node decoder/encoder registry.
//!
//! Each actor type that participates in inter-node delivery contributes
//! one [`InterNodeDecoder`] entry via the `register_for_inter_node!`
//! macro. The receiving node uses the registry to turn incoming envelope
//! bytes back into a concrete `<T as Actor>::Message` (and to encode
//! `Result` bytes for the reply).
//!
//! Built on the [`inventory`](https://docs.rs/inventory) crate; entries
//! are linked into the binary at compile time, so registration must
//! happen at module scope (not inside a function).

use crate::ActorError;
use std::any::Any;
use std::sync::Arc;
use xancode::{Bytes, Codec};

/// Decoder: raw bytes → `Arc<dyn Any>` carrying a concrete `<T as Actor>::Message`.
/// Built from `decode_codec::<T::Message>` by the registration macro.
pub type DecodeMessageFn =
    fn(&[u8]) -> Result<Arc<dyn Any + Send + Sync>, ActorError>;
/// Encoder: `Box<dyn Any>` (must downcast to `<T as Actor>::Result`) →
/// raw bytes. Built from `encode_codec::<T::Result>` by the registration
/// macro.
pub type EncodeResultFn =
    fn(Box<dyn Any + Send>) -> Result<Vec<u8>, ActorError>;
/// Function returning the actor's fully-qualified Rust type name. Stored
/// as a `fn` pointer instead of a `&'static str` because
/// `std::any::type_name` is not a `const fn`.
pub type ActorTypeFn = fn() -> &'static str;

/// One entry in the inter-node decoder/encoder registry.
///
/// One entry per actor type that should be reachable across the cluster.
/// `actor_type` is matched against the `actor_type` field carried in
/// incoming envelopes ([`InterNodeMessage`]); `decode_message` turns the
/// envelope's payload bytes into a typed `Arc<dyn Any>` for
/// `Mailbox::send`/`send_and_recv`; `encode_result` turns the
/// `Box<dyn Any>` returned by `dispatch_local_any_and_recv` back into
/// bytes for the `InterNodeResponse`.
///
/// Use the `register_for_inter_node!` macro to construct + submit
/// entries; never build one by hand.
///
/// [`InterNodeMessage`]: crate::inter_node::InterNodeMessage
pub struct InterNodeDecoder {
    /// `std::any::type_name` isn't a `const fn`, so the registry holds a
    /// function pointer instead of a `&'static str`. The macro
    /// `register_for_inter_node!` fills this with `type_name_of::<T>`.
    pub actor_type: ActorTypeFn,
    /// `Vec<u8>` → `Arc<dyn Any>` carrying `<T as Actor>::Message`.
    pub decode_message: DecodeMessageFn,
    /// `Box<dyn Any>` (downcastable to `<T as Actor>::Result`) → `Vec<u8>`.
    pub encode_result: EncodeResultFn,
}

inventory::collect!(InterNodeDecoder);

/// Generic function used by the registration macro to obtain a `fn` pointer
/// that, when called, returns the actor's type name.
///
/// Equivalent to writing `|| std::any::type_name::<T>()`, but as a real
/// `fn` item (so it has a stable function-pointer address) rather than a
/// closure type.
pub fn type_name_of<T: ?Sized>() -> &'static str {
    std::any::type_name::<T>()
}

/// Look up the decoder for `actor_type` and use it to decode `bytes`
/// into an `Arc<dyn Any>` carrying the concrete message type.
///
/// Returns `InterNodeDecoderMissing` if no `register_for_inter_node!`
/// entry exists for this actor type on this binary; `InterNodeDecode`
/// if the bytes are invalid for the registered type.
pub fn decode_message_for(
    actor_type: &str,
    bytes: &[u8],
) -> Result<Arc<dyn Any + Send + Sync>, ActorError> {
    let entry = inventory::iter::<InterNodeDecoder>
        .into_iter()
        .find(|d| (d.actor_type)() == actor_type)
        .ok_or_else(|| ActorError::InterNodeDecoderMissing(actor_type.to_string()))?;
    (entry.decode_message)(bytes)
}

/// Look up the encoder for `actor_type` and use it to turn the result
/// `Box<dyn Any>` from `dispatch_local_any_and_recv` back into bytes
/// for the `InterNodeResponse`.
///
/// Returns `InterNodeDecoderMissing` if no entry exists,
/// `MessageTypeMismatch` if the boxed value isn't actually `T::Result`.
pub fn encode_result_for(
    actor_type: &str,
    any: Box<dyn Any + Send>,
) -> Result<Vec<u8>, ActorError> {
    let entry = inventory::iter::<InterNodeDecoder>
        .into_iter()
        .find(|d| (d.actor_type)() == actor_type)
        .ok_or_else(|| ActorError::InterNodeDecoderMissing(actor_type.to_string()))?;
    (entry.encode_result)(any)
}

/// Helper used by the `register_for_inter_node!` macro.
/// Decodes raw bytes into `Arc<dyn Any>` carrying a concrete `T`.
///
/// You should not call this directly — the macro instantiates it with
/// `T = <YourActor as Actor>::Message` and stores the resulting
/// `fn` pointer in the `InterNodeDecoder` entry.
pub fn decode_codec<T>(bytes: &[u8]) -> Result<Arc<dyn Any + Send + Sync>, ActorError>
where
    T: Codec + Send + Sync + 'static,
{
    let value = T::decode(&Bytes::copy_from_slice(bytes)).map_err(|_| {
        ActorError::InterNodeDecode(format!(
            "decode failed for {}",
            std::any::type_name::<T>()
        ))
    })?;
    Ok(Arc::new(value))
}

/// Helper used by the `register_for_inter_node!` macro.
/// Downcasts `Box<dyn Any>` to `T` and encodes via `Codec`.
///
/// You should not call this directly — the macro instantiates it with
/// `T = <YourActor as Actor>::Result` and stores the resulting `fn`
/// pointer in the `InterNodeDecoder` entry.
pub fn encode_codec<T>(any: Box<dyn Any + Send>) -> Result<Vec<u8>, ActorError>
where
    T: Codec + Send + 'static,
{
    let typed = any
        .downcast::<T>()
        .map_err(|_| ActorError::MessageTypeMismatch)?;
    Ok(typed.encode().to_vec())
}
