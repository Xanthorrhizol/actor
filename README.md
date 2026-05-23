# xan-actor

Akka-style actor library for Rust, built on tokio. Multiple actor types share
one `ActorSystem`, addresses route messages with compile-time message-type
checks, and an optional `multi-node` feature extends the same API across
processes via a [xanq](https://crates.io/crates/xanq) broker.

📖 **Full documentation: <https://xanthorrhizol.github.io/actor/>**

## Install

```bash
cargo add xan-actor
cargo add async-trait
```

Default is `bounded-channel`. Other feature flags:

| Feature             | What it does |
|---------------------|--------------|
| `bounded-channel`   | (default) `tokio::sync::mpsc::channel` mailboxes |
| `unbounded-channel` | `tokio::sync::mpsc::unbounded_channel` mailboxes (mutually exclusive with `bounded-channel`) |
| `xan-log`           | optional `log` backend |
| `multi-node`        | cross-process/-machine delivery via a xanq broker — see the wiki's [Multi-node](https://xanthorrhizol.github.io/actor/multi-node/) page |

## Minimal example

```rust
use std::sync::Arc;
use xan_actor::prelude::*;

#[derive(Debug, Clone)]
enum Msg { Ping(String) }

#[derive(thiserror::Error, Debug)]
enum Err { #[error(transparent)] Actor(#[from] ActorError) }

struct Echo { address: String }

#[async_trait::async_trait]
impl Actor for Echo {
    type Message = Msg;
    type Result = Msg;
    type Error = Err;
    fn address(&self) -> &str { &self.address }
    async fn handle(&mut self, msg: Arc<Self::Message>) -> Result<Self::Result, Self::Error> {
        Ok((*msg).clone())
    }
}

#[tokio::main]
async fn main() -> Result<(), ActorError> {
    let mut system = ActorSystem::new(None);
    Echo { address: "/echo/1".into() }
        .register(&mut system, ErrorHandling::Stop, Blocking::NonBlocking, None)
        .await?;
    let resp = system
        .send_and_recv::<Echo>("/echo/1".into(), Msg::Ping("hi".into()))
        .await?;
    println!("got = {resp:?}");
    Ok(())
}
```

For the unbounded variant, multi-node setup, job scheduling, broadcast,
lifecycle hooks, and the rest, see the [wiki](https://xanthorrhizol.github.io/actor/).

## License

MIT
