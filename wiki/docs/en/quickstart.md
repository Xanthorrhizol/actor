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
