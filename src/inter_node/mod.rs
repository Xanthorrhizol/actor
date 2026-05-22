pub mod address;
pub mod decoder;
pub mod dispatcher;
pub mod envelope;

pub use address::{Address, NodeFilter};
pub use decoder::{
    ActorTypeFn, DecodeMessageFn, EncodeResultFn, InterNodeDecoder, decode_codec,
    decode_message_for, encode_codec, encode_result_for, type_name_of,
};
pub use dispatcher::InterNodeRuntime;
pub use envelope::{InterNodeMessage, InterNodeResponse, ResponseOutcome, Topic, TopicKind};

/// Register an `Actor`'s message and result types so that this node can
/// participate in inter-node delivery for `$actor`.
///
/// Call once per actor type at module level.
///
/// ```ignore
/// xan_actor::register_for_inter_node!(MyActor);
/// ```
#[macro_export]
macro_rules! register_for_inter_node {
    ($actor:ty) => {
        $crate::inter_node::__private_inventory::submit! {
            $crate::inter_node::InterNodeDecoder {
                actor_type: $crate::inter_node::type_name_of::<$actor>,
                decode_message: $crate::inter_node::decode_codec::<
                    <$actor as $crate::actor::Actor>::Message,
                >,
                encode_result: $crate::inter_node::encode_codec::<
                    <$actor as $crate::actor::Actor>::Result,
                >,
            }
        }
    };
}

#[doc(hidden)]
pub mod __private_inventory {
    pub use inventory::submit;
}
