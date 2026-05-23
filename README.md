## Actor

### Usage

1. add actor and dependency in Cargo.toml

```bash
$ cargo add xan-actor
$ cargo add serde --features=derive
$ cargo add async-trait
```

> :bulb: If you want to use unbounded-channel on actor system, use `cargo add xan-actor --features=unbounded-channel` instead of `cargo add xan-actor`.

2. create a actor as mutable

**bounded-channel feature**
```rust
use xan_actor::ActorSystem;
...

let mut actor_system = ActorSystem::new(None /* channel size */);
```

**unbounded-channel feature**
```rust
use xan_actor::ActorSystem;
...
let mut actor_system = ActorSystem::new();
```

3. declare Actor to register

> :bulb: The actor doesn't have to use same message type. Single ActorSystem supports it.

```rust
use xan_actor::prelude::*;

#[derive(Debug)]
pub enum MyMessage1 {
    A(String),
    C(String),
}
#[derive(Debug)]
pub enum MyMessage2 {
    B(String),
}

#[derive(thiserror::Error, Debug)]
enum MyError {
    #[error("bye")]
    Exit,
    #[error(transparent)]
    ActorError(#[from] ActorError),
}

struct MyActor1 {
  pub address: String,
}

struct MyActor2 {
    pub address: String,
}

#[async_trait::async_trait]
impl Actor for MyActor1 {
    type Message = MyMessage1;
    type Result = MyMessage1;
    type Error = MyError;

    fn address(&self) -> &str {
        &self.address
    }

    async fn handle(&mut self, msg: Arc<Self::Message>) -> Result<Self::Result, Self::Error> {
        println!("[{}] got MyMessage1: {:?}", self.address(), msg);
        Ok(msg)
    }
}

#[async_trait::async_trait]
impl Actor for MyActor2 {
    type Message = MyMessage2;
    type Result = MyMessage2;
    type Error = MyError;

    fn address(&self) -> &str {
        &self.address
    }

    async fn handle(&mut self, msg: Arc<Self::Message>) -> Result<Self::Result, Self::Error> {
        println!("[{}] got MyMessage2: {:?}", self.address(), msg);
        Ok(msg)
    }
}
```

4. register actor into actor system

**bounded-channel feature**
```rust
use xan_actor::prelude::*;

let actor1 = MyActor1 {
    address: "/some/address/1/1".to_string(),
};
actor1.register(&mut actor_system, ErrorHandling::Stop, Blocking::Blocking, None /* channel size */).await;

let actor2 = MyActor2 {
    address: "/some/address/2".to_string(),
};
actor2.register(&mut actor_system, ErrorHandling::Restart, Blocking::NonBlocking, None /* channel size */).await;

let actor3 = MyActor1 {
    address: "/some/address/1/2".to_string(),
};
actor3.register(&mut actor_system, ErrorHandling::Resume, Blocking::Blocking, None /* channel size */).await;
```

**unbounded-channel feature**
```rust
use xan_actor::prelude::*;

let actor1 = MyActor1 {
    address: "/some/address/1/1".to_string(),
};
actor1.register(&mut actor_system, ErrorHandling::Stop, Blocking::Blocking).await;

let actor2 = MyActor2 {
    address: "/some/address/2".to_string(),
};
actor2.register(&mut actor_system, ErrorHandling::Restart, Blocking::NonBlocking).await;

let actor3 = MyActor1 {
    address: "/some/address/1/2".to_string(),
};
actor3.register(&mut actor_system, ErrorHandling::Resume, Blocking::Blocking).await;
```

5. use it

```rust
// you can send message to multiple actor at once using address with regex
let _ = actor_system.send_broadcast::<MyActor1>(
  "/some/address/1/*".to_string(), /* address as regex */
  MyMessage1::A("a1".to_string()), /* message */
).await;
let result = actor_system.send_and_recv::<MyActor2>(
  "/some/address/2".to_string(), /* address */
  MyMessage2::B("b1".to_string()), /* message */
).await;

// restart actors
actor_system.restart(
  "/some/address/1/*".to_string(), /* address as regex */
);
// it needs some time. TODO: handle it inside of restart function
tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;

let result = actor_system.send_and_recv::<MyActor2>(
  "/some/address/2".to_string(), /* address */
  MyMessage2::B("b2".to_string()), /* message */
).await;

// kill and unregister actor
actor_system.unregister(
  "*".to_string(), /* address */
);
```

### Job

- If you send message at some time or with some iteration, you can use job

```rust
use xan_actor::JobSpec;
...

let job = JobSpec::new(
  None, /* max_iter */
  Some(std::time::Duration::from_secs(3)), /* interval */
  std::time::SystemTime::now(), /* start_at */
);
if let Ok(RunJobResult {
    job_id,
    result_subscriber_rx,
}) = actor_system.run_job::<MyActor1>(
  "/some/address/1/1".to_string(), /* address */
  true, /* whether subscribe the handler result or not(true => Some(rx)) */
  job, /* job as JobSpec */
  MyMessage1::C("c".to_string()), /* message */
  None, /* job_id(if you want to specify)*/
).await {
    let mut i = 0;
    if let Some(mut recv_rx) = result_subscriber_rx {
        while let Some(result) = recv_rx.recv().await {
            i += 1;
            println!("result returned");
            if i == 3 {
                // you can stop or resume the job
                actor_system.stop_job(job_id).await;
                tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;
                actor_system.resume_job(job_id).await;
            }
            if i == 5 {
                // you can abort the job
                actor_system.abort_job(job_id).await;
            }
        }
    }
}
```

### Address Usage

- You should use address as unique identifier for each actor.
- If you register duplicated address, it will return `ActorError::AddressAlreadyExists`.

```rust
let actor = MyActor1 {
    address: "/some/address/1/2".to_string(),
};
actor.register(&mut actor_system, false).await;

let actor_duplicated = MyActor2 {
    address: "/some/address/1/2".to_string(),
};
info!(
    "[{}] test duplicated actor registration",
    actor_duplicated.address(),
);

assert!(
    actor_duplicated
        .register(&mut actor_system, false)
        .await
        .err()
        .is_some()
);
```

## Features

| Feature             | Default | Description                                                                                |
| ------------------- | :-----: | ------------------------------------------------------------------------------------------ |
| `bounded-channel`   |   yes   | `tokio::sync::mpsc::channel(CHANNEL_SIZE)` for actor mailboxes. `CHANNEL_SIZE = 4096`. `ActorSystem::new` takes `channel_size: Option<usize>`; `Actor::register` takes a trailing `channel_size: Option<usize>`. |
| `unbounded-channel` |    -    | `tokio::sync::mpsc::unbounded_channel()` for actor mailboxes. Mutually exclusive with `bounded-channel`. `ActorSystem::new` and `Actor::register` drop the `channel_size` argument. |
| `xan-log`           |    -    | Wires the optional `xan-log` integration.                                                  |
| `multi-node`        |    -    | Enables inter-node delivery via the [`xanq`](https://crates.io/crates/xanq) broker. Pulls in `xancode`, `xanq`, and `inventory`. See the section below. |

Top-of-file examples already show the bounded/unbounded shapes side by side; only `multi-node` needs additional explanation.

### `multi-node`

Adds inter-node delivery so that the same `send::<T>` / `send_and_recv::<T>` API can route to an actor living on another process or machine, on top of a [xanq](https://crates.io/crates/xanq) broker. Works with either `bounded-channel` or `unbounded-channel`.

```bash
cargo add xan-actor --features multi-node
cargo add async-trait     # `#[async_trait::async_trait]` on your `impl Actor`
cargo add thiserror       # idiomatic for `Actor::Error` types; not strictly required
cargo add xancode         # needed for `#[derive(Codec)]` on your message/result types
cargo add xanq            # only if you spawn an in-process broker yourself (tests / demos);
                          # skip if you connect to an externally-running broker
cargo add xan-log         # optional: a ready-made `log` backend; any other backend works too
```

- `xancode` is a direct dependency on the user side — `xan-actor` doesn't re-export the `Codec` trait/derive because the proc-macro paths between the two crates have to resolve through the user's own `xancode` import to type-check correctly.
- `xanq` is only required when you bring up your own broker (`xanq::server::Server`); the `xan-actor` runtime itself only needs `xanq::client::Client` and handles that internally.
- `xan-log` is optional. `xan-actor` emits via the `log` facade; any backend (`env_logger`, `tracing-log`, etc.) works. If you use `xan-log`, remember it reads the `LOG_LEVEL` env var on init and defaults to `Off` — running tests/binaries without `LOG_LEVEL=debug` (or similar) yields no output.

The address shape changes from `String` to a struct `Address { name, node }`. The `node` field decides routing structurally — a local call (`address.node == self_node`) goes through the existing mailbox path with zero serialization; a remote call goes over the broker. Cross-node uniqueness is automatic (two nodes can't physically hold the same `Address` because the `node` fields differ).

#### 1. Make your message and result types serializable

The multi-node path needs `xancode::Codec` for `<T as Actor>::Message` and `<T as Actor>::Result` (so they can cross node boundaries).

```rust
use xancode::Codec;

#[derive(Debug, Clone, Codec)]
pub enum MyMessage {
    Ping(String),
    Echo(String),
}
```

#### 2. Register each actor type once at module level

`register_for_inter_node!` installs the decoder/encoder entries the receiving node needs to turn raw envelope bytes back into `<T as Actor>::Message` and the response back into bytes. Call it once per actor type at module scope, **not inside a function**.

```rust
xan_actor::register_for_inter_node!(MyActor);
```

#### 3. Use `Address` everywhere, return `&Address` from `Actor::address`

With `multi-node` enabled, `Actor::address(&self)` returns `&inter_node::Address` instead of `&str`. Your actor struct holds an `Address`:

```rust
use xan_actor::prelude::*;   // brings Address, NodeFilter, Topic, ...

struct MyActor { addr: Address }

#[async_trait::async_trait]
impl Actor for MyActor {
    type Message = MyMessage;
    type Result = MyMessage;
    type Error = MyError;

    fn address(&self) -> &Address { &self.addr }

    async fn handle(&mut self, msg: Arc<Self::Message>) -> Result<Self::Result, Self::Error> { ... }
}
```

#### 4. Construct `ActorSystem` with a node name (and optionally a broker)

`new` requires `node_name`; `broker_addr` is optional (passing `None` keeps the system local-only — cross-node calls will fail with `InterNodeNotConfigured`).

```rust
// bounded-channel + multi-node
let mut system = ActorSystem::new(
    None,                              // channel_size
    "node-a".into(),                   // node_name
    Some("127.0.0.1:7777".into()),     // broker_addr
).await?;

// unbounded-channel + multi-node (no channel_size)
let mut system = ActorSystem::new("node-a".into(), Some("127.0.0.1:7777".into())).await?;
```

#### 4a. Spawning your own broker (in-process)

`xan_actor::Address` implements `xanq::address::Address` (delivery mode `Anycast`), so you can pass our `Address` straight to xanq as the Server type parameter — no separate newtype required:

```rust
use xan_actor::prelude::*;        // Address, ...
use xanq::server::Server;

let (_server, addr) = Server::<Address>::spawn("127.0.0.1:0").await.expect("broker");
let broker = addr.to_string();    // hand this to ActorSystem::new
// Keep `_server` (Arc<Server>) bound so the accept loop stays alive.
```

The Server is generic over the topic type at the API level, but the wire is type-erased (bytes). It still happily routes `xan_actor`'s internal `Topic { node, kind }` traffic — picking `Address` here is just an ergonomic choice for the user-facing call site.

#### 5. Use the same API as single-node

```rust
use xan_actor::prelude::*;   // Address, NodeFilter, ...

// Local actor: address.node == "node-a", routed through the local mailbox path (no broker).
system
    .send::<MyActor>(Address::new("node-a", "/echo/1"), MyMessage::Ping("hi".into()))
    .await?;

// Remote actor on node-b: encoded and shipped over the broker.
let result = system
    .send_and_recv::<MyActor>(Address::new("node-b", "/echo/2"), MyMessage::Ping("hi".into()))
    .await?;

// Broadcast across a known peer set.
let results = system
    .send_broadcast::<MyActor>(
        "/echo/.*".into(),
        NodeFilter::Peers(vec!["node-a".into(), "node-b".into()]),
        MyMessage::Ping("bcast".into()),
    )
    .await;
```

`NodeFilter` is `SelfOnly`, `Node(name)`, or `Peers(Vec<name>)`. Local addresses go through the existing local fan-out (per-actor results); each remote peer is sent one `BroadcastFire` envelope (one result per peer). The receiver matches the regex against its own local addresses and dispatches.

#### Trying to register an address you don't own

```rust
RemoteActor { addr: Address::new("node-b", "/foreign") }
    .register(&mut node_a, ...).await
// → Err(ActorError::AddressNotOwned("node-b:/foreign"))
```

The Register handler validates `address.node == self_node` and rejects mismatches. This is the structural cross-node uniqueness — there's no race window because two nodes can't legitimately try the same `Address`.

#### Wire protocol overview

Each node connects to a shared xanq broker and subscribes to two `Anycast` topics: one for incoming requests, one for incoming responses. There is no discovery channel — node membership is left to the caller (via `NodeFilter::Peers`).

- Outgoing envelopes (`InterNodeMessage::Fire { ... }` / `Call { ... }` / `BroadcastFire { ... }`) carry `actor_type`, the target's `name` (the node part is implicit from the request topic), and the encoded payload. `Call` also carries `reply_to` + sender-local `req_id`.
- The receiving node decodes the payload via the inventory registry, dispatches through the existing local mailbox flow, encodes the result (if any), and for `Call` publishes an `InterNodeResponse { req_id, outcome }` back to `reply_to`.
- The sender's response consumer matches `req_id` against a pending-requests map and resolves the awaiting `send_and_recv`.
- `BroadcastFire` runs `filter_address(name_regex)` on the receiver and dispatches to each match (fire-and-forget).

#### Notes and current limitations

- `send`, `send_and_recv`, `send_without_tx_cache`, `send_and_recv_without_tx_cache`, `send_broadcast`, `send_broadcast_without_tx_cache`, `run_job`, and `run_job_without_tx_cache` all route based on `address.node` (`==` self → local, otherwise remote).
- `run_job` against a remote address spawns the job loop on the *calling* node; each iteration goes over the broker via the same `JobController` (`abort_job` / `stop_job` / `resume_job`) that local jobs use.
- A missing `register_for_inter_node!` call surfaces as `ActorError::InterNodeDecoderMissing` the first time an envelope arrives for that actor type.
- Calling `send` / `send_and_recv` with `address.node != self_node` while the system was created with `broker_addr = None` returns `ActorError::InterNodeNotConfigured`.
- The initial broker connection is capped by `inter_node::DEFAULT_BROKER_CONNECT_TIMEOUT` (5 s). If the broker is missing or unreachable, `ActorSystem::new` fails with `ActorError::InterNodeIo("broker connect to ... timed out after 5s")` instead of hanging on the OS-default TCP connect timeout. Use `InterNodeRuntime::connect_with_timeout` directly if you need a different cap.
