use crate::ActorError;
use std::any::Any;
use std::sync::Arc;
use xancode::{Bytes, Codec};

pub type DecodeMessageFn =
    fn(&[u8]) -> Result<Arc<dyn Any + Send + Sync>, ActorError>;
pub type EncodeResultFn =
    fn(Box<dyn Any + Send>) -> Result<Vec<u8>, ActorError>;
pub type ActorTypeFn = fn() -> &'static str;

pub struct InterNodeDecoder {
    /// `std::any::type_name` isn't a `const fn`, so the registry holds a
    /// function pointer instead of a `&'static str`. The macro
    /// `register_for_inter_node!` fills this with `type_name_of::<T>`.
    pub actor_type: ActorTypeFn,
    pub decode_message: DecodeMessageFn,
    pub encode_result: EncodeResultFn,
}

inventory::collect!(InterNodeDecoder);

/// Generic function used by the registration macro to obtain a `fn` pointer
/// that, when called, returns the actor's type name.
pub fn type_name_of<T: ?Sized>() -> &'static str {
    std::any::type_name::<T>()
}

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
pub fn encode_codec<T>(any: Box<dyn Any + Send>) -> Result<Vec<u8>, ActorError>
where
    T: Codec + Send + 'static,
{
    let typed = any
        .downcast::<T>()
        .map_err(|_| ActorError::MessageTypeMismatch)?;
    Ok(typed.encode().to_vec())
}
