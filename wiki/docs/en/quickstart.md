# Quickstart

## Install

```bash
cargo add xan-actor
cargo add async-trait
```

`bounded-channel` is the default.

To use unbounded mode:

```bash
cargo add xan-actor --no-default-features --features unbounded-channel
```

## Minimal Example

```rust
use xan_actor::prelude::*;
use std::sync::Arc;

#[derive(Debug, Clone)]
enum MsgA {
    Ping(String),
}

#[derive(Debug, Clone)]
enum MsgB {
    Echo(String),
}

#[derive(thiserror::Error, Debug)]
enum MyError {
    #[error(transparent)]
    Actor(#[from] ActorError),
}

struct ActorA {
    address: String,
}

struct ActorB {
    address: String,
}

#[async_trait::async_trait]
impl Actor for ActorA {
    type Message = MsgA;
    type Result = MsgA;
    type Error = MyError;

    fn address(&self) -> &str { &self.address }

    async fn handle(&mut self, msg: Arc<Self::Message>) -> Result<Self::Result, Self::Error> {
        Ok((*msg).clone())
    }
}

#[async_trait::async_trait]
impl Actor for ActorB {
    type Message = MsgB;
    type Result = MsgB;
    type Error = MyError;

    fn address(&self) -> &str { &self.address }

    async fn handle(&mut self, msg: Arc<Self::Message>) -> Result<Self::Result, Self::Error> {
        Ok((*msg).clone())
    }
}

#[tokio::main]
async fn main() -> Result<(), ActorError> {
    let mut system = ActorSystem::new(None);

    ActorA { address: "/a/1".into() }
        .register(&mut system, ErrorHandling::Stop, Blocking::NonBlocking, None)
        .await?;

    ActorB { address: "/b/1".into() }
        .register(&mut system, ErrorHandling::Stop, Blocking::NonBlocking, None)
        .await?;

    // compile-time check: only MsgA is accepted here
    system.send::<ActorA>("/a/1".into(), MsgA::Ping("hello".into())).await?;

    let result = system
        .send_and_recv::<ActorB>("/b/1".into(), MsgB::Echo("world".into()))
        .await?;
    println!("result = {:?}", result);

    Ok(())
}
```

## What This Shows

- one `ActorSystem` hosting `ActorA` and `ActorB`
- `send::<ActorA>` accepts only `MsgA`
- `send_and_recv::<ActorB>` returns `ActorB::Result`

## Multi-node Minimal Example

```bash
cargo add xan-actor --features multi-node
cargo add async-trait
cargo add thiserror
cargo add xancode    # needed for `#[derive(Codec)]`; xan-actor doesn't re-export it
cargo add xanq       # this example spawns its own in-process broker; skip if connecting to an external one
```

This example runs an in-process xanq broker so it works as a single binary.
Two `ActorSystem` instances (`node-b` and `node-a`) speak to it; the actor
lives on `node-b` and `node-a` calls it via a fully qualified `Address`.
See [Multi-node](multi-node.md) for the full setup.

```rust
use std::sync::Arc;
use xan_actor::prelude::*;       // Address, NodeFilter, Topic, ...
use xancode::Codec;              // direct dependency on the user side
use xanq::server::Server;

// `Codec` is required so messages and results can cross node boundaries.
#[derive(Debug, Clone, Codec)]
enum Echo {
    Ping(String),
    Pong(String),
}

#[derive(thiserror::Error, Debug)]
enum E {
    #[error(transparent)]
    Actor(#[from] ActorError),
}

struct EchoActor {
    addr: Address,
}

#[async_trait::async_trait]
impl Actor for EchoActor {
    type Message = Echo;
    type Result = Echo;
    type Error = E;

    fn address(&self) -> &Address { &self.addr }

    async fn handle(&mut self, msg: Arc<Self::Message>) -> Result<Self::Result, Self::Error> {
        match &*msg {
            Echo::Ping(s) => Ok(Echo::Pong(format!("pong:{s}"))),
            other => Ok(other.clone()),
        }
    }
}

// Module-scope registration. Wires `EchoActor` into the inter-node decoder/encoder
// registry so a peer node can deserialize messages addressed to it.
xan_actor::register_for_inter_node!(EchoActor);

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<(), ActorError> {
    // 1. Start an in-process xanq broker. Our `Address` impls `xanq::address::Address`,
    //    so it doubles as the Server's type parameter — no extra newtype needed.
    let (_server, broker_addr) = Server::<Address>::spawn("127.0.0.1:0")
        .await
        .expect("spawn broker");
    let broker = broker_addr.to_string();

    // 2. Owner node hosts the actor. The actor's Address.node must match the system's node_name.
    let mut node_b = ActorSystem::new(
        None,                       // channel_size
        "node-b".into(),            // node_name
        Some(broker.clone()),       // broker_addr
    ).await?;
    EchoActor { addr: Address::new("node-b", "/echo") }
        .register(&mut node_b, ErrorHandling::Stop, Blocking::NonBlocking, None)
        .await?;

    // 3. Caller node.
    let mut node_a = ActorSystem::new(None, "node-a".into(), Some(broker)).await?;

    // 4. Same API as single-node — the Address.node field decides routing.
    let resp = node_a
        .send_and_recv::<EchoActor>(
            Address::new("node-b", "/echo"),
            Echo::Ping("hi".into()),
        )
        .await?;
    println!("got = {resp:?}");
    Ok(())
}
```

### What's Different

- `Echo` derives `xancode::Codec` so the payload can be serialized.
- `register_for_inter_node!(EchoActor)` is called once at module scope.
- The actor holds an `Address` (struct), not a `String`. `Actor::address` returns `&Address`.
- `ActorSystem::new` is `async` and requires `node_name`; `broker_addr` is optional.
- `send_and_recv::<EchoActor>(Address::new("node-b", "/echo"), ...)` — the `Address.node` field structurally decides whether the call goes local (no broker) or remote (over the broker).
