## Actor

### Caution

This version has breaking changes from the previous version. The previous version is not compatible with this version.

### Usage

1. add dependencies

```bash
$ cargo add xan-actor bincode log
$ cargo add serde --features=serde_derive
$ cargo add tokio --features="rt-multi-thread macros sync"
```

2. declare an actor_system in your lib.rs or main.rs with declare the message types that will be sent to the actors.

```rust
xan_actor::actor_system!(
    struct Message {
        pub message: String,
    },
    ...
);
```

3. declare Actor to register

```rust
xan_actor::actor!(
    Actor, // actor name
    Message, // message type
    struct ActorResource {
        pub data: String,
    },
    fn handle_message(&self, message: Message) -> String {
        message.message
    },
    fn pre_start(&mut self) {},
    fn post_stop(&mut self) {},
    fn pre_restart(&mut self) {},
    fn post_restart(&mut self) {},
    true
);
```

4. create ActorSystem & declared actor

```rust
#[tokio::main]
async fn main() {
    let (mut actor_system, register_tx) = ActorSystem::new();
    let actor = Actor::new(
        address!("test".to_string(), "actor".to_string(), "*".to_string()),
        ActorResource {
            data: "test".to_string(),
        },
        register_tx,
    );
    ...
}
```

5. run actor

```rust
#[tokio::main]
async fn main() {
   ...
    let (_handle, ready_rx) = actor.run();
    ready_rx.await.unwrap();
    ...
}
```

6. send and receive messages

```rust
#[tokio::main]
async fn main() {
    ...
    let response_rxs = send_msg!(
        &mut actor_system,
        address!("test".to_string(), "actor".to_string(), "*".to_string()), // actor address
        &"test".to_string()                                                 // message
    );

    for response in recv_res!(String /* return type */, response_rxs) {
        println!("{}", response);
    }
    ...
}
```
