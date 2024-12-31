extern crate proc_macro;
#[macro_use]
extern crate quote;

mod actor;
mod actor_system;
mod address;
mod messenger;

#[proc_macro]
/// Generate actor system struct and inner methods.  
/// # Arguments
/// * `message_types` - Message structs or enums that the actor system will use
///
/// # Example
/// ```rust
/// xan_actor::actor_system!(
///     pub struct StructMessage {
///         pub message: String,
///     },
///     enum EnumMessage {
///         A,
///         B,
///     }
/// );
/// ```
pub fn actor_system(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    actor_system::actor_system(input)
}

#[proc_macro]
/// Genereate actor struct and inner methods
/// # Arguments
/// * `actor_struct_name` - Name of the actor struct
/// * `message_type` - Message struct or enum that the actor will receive
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
/// xan_actor::actor_system!(); // It always needs to be called once in lib.rs or main.rs
///
/// xan_actor::actor!(
///     TestActor,
///     Message,
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
/// Send messages to actors
/// # Arguments
/// * `actor_system` - Actor system that the actor belongs to
/// * `address` - Address of the actor(can use "*")
/// * `message` - Message to send
///
/// # Example
/// ```rust
/// xan_actor::send_msg!(
///     &mut actor_system,
///     address!("root".to_string(), "parent".to_string(), "*".to_string()),
///     &"test".to_string()
/// );
/// ```
pub fn send_msg(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    messenger::send_msg(input)
}

#[proc_macro]
/// Receive responses from actors
/// # Arguments
/// * `res_type` - Type of the response
/// * `response_rxs` - Response receiver vector
///
/// # Example
/// ```rust
/// let response_rxs = xan_actor::send_msg!(
///     &mut actor_system,
///     address!("root".to_string(), "parent".to_string(), "*".to_string()),
///     &"test".to_string()
/// );
/// for response in xan_actor::recv_res!(
///    String,
///    response_rxs,
/// ) {
///    println!("{}", response);
/// }
/// ```
pub fn recv_res(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    messenger::recv_res(input)
}

#[proc_macro]
/// Generate an actor address
/// # Arguments
/// * `parts` - Address parts. It takes multiple strings
///
/// # Example
/// ```rust
/// let address = xan_actor::address!("root".to_string(), "parent".to_string(), "child".to_string());
///
/// // you can use it like regex
/// let address = xan_actor::address!("root".to_string(), "parent".to_string(), "*".to_string());
/// let address = xan_actor::address!("root".to_string(), "*".to_string(), "child".to_string());
/// ```
pub fn address(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    address::address(input)
}
