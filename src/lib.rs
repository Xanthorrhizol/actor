extern crate proc_macro;
#[macro_use]
extern crate quote;

mod actor;
mod actor_system;
mod messenger;

#[proc_macro]
/// Generate actor system struct and inner methods.  
/// It contains LifeCycle, WhoisResponse struct too
pub fn actor_system(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    actor_system::actor_system(input)
}

#[proc_macro]
/// Genereate actor struct and inner methods
/// # Arguments
/// * `actor_struct_name` - Name of the actor struct
/// * `message_struct` - Message struct that the actor will receive
/// * `resource_struct` - Resource struct that the actor will use
/// * `handle_message` - Method that will handle the message
/// * `pre_start` - Method that will be called before the actor starts
/// * `post_stop` - Method that will be called after the actor stops
/// * `pre_restart` - Method that will be called before the actor restarts
/// * `post_restart` - Method that will be called after the actor restarts
/// * `kill_in_error` - kill the actor if an error occurs - TODO: not supported yet
///
/// # Example
/// ```rust
/// use xan_actor::{actor, actor_system, recv_res, send_msg};
///
/// actor_system!(); // It always needs to be called once in lib.rs or main.rs
///
/// actor!(
///     TestActor,
///     struct Message {
///         pub message: String,
///     },
///     struct TestActorResource {
///         pub name: String,
///     },
///     fn handle_message(&self, message: Message) -> String {
///         message.message
///     },
///     fn pre_start(&mut self) {},
///     fn post_stop(&mut self) {},
///     fn pre_restart(&mut self) {},
///     fn post_restart(&mut self) {},
///     true
/// );
/// ```
pub fn actor(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    actor::actor(input)
}

#[proc_macro]
/// Send a message to an actor
/// # Arguments
/// * `actor_system` - Actor system that the actor belongs to
/// * `address` - Address of the actor
/// * `message` - Message to send
///
/// # Example
/// ```rust
/// use xan_actor::send_msg;
/// send_msg!(
///     &mut actor_system,
///     "test-actor".to_string(),
///     &"test".to_string()
/// );
/// ```
pub fn send_msg(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    messenger::send_msg(input)
}

#[proc_macro]
/// Receive a response from an actor
/// # Arguments
/// * `res_type` - Type of the response
/// * `response_rx` - Response receiver
///
/// # Example
/// ```rust
/// use xan_actor::{recv_res, send_msg};
/// let response_rx = send_msg!(
///     &mut actor_system,
///     "test-actor".to_string(),
///     &"test".to_string()
/// );
/// let response = recv_res!(
///    String,
///    response_rx
/// );
/// ```
pub fn recv_res(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    messenger::recv_res(input)
}
