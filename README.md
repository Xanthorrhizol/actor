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

2. declare an actor_system in your lib.rs or main.rs

```rust
xan_actor::actor_system!();
```

3. declare Actor to register

```rust
actor!(
    TestActor,
    struct Message {
        pub message: String,
    },
    struct TestActorResource {
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
fn main() {
    let (whois_response_rx, mut actor_system) = ActorSystem::new();
    let actor = TestActor::new(
        "test-actor".to_string(),
        TestActorResource {
            data: "test".to_string(),
        },
    );
    ...
}
```

5. run actor

```rust
fn main() {
   ...
    let (_handle, ready_rx) = actor.run(whois_response_rx);
    ready_rx.await.unwrap();
    ...
}

6. send and receive messages

```rust
fn main() {
    ...
    let response_rx = send_msg!(
        &mut actor_system,
        "test-actor".to_string(), // address
        &"test".to_string() // message
    );
    let response = recv_res!(String /* return type */, response_rx);
    println!("{}", response);
    ...
}
```
