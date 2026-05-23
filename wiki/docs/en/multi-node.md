# Multi-node

The `multi-node` feature lets one `ActorSystem` route `send::<T>` / `send_and_recv::<T>` to actors living on another process or machine via a [xanq](https://crates.io/crates/xanq) broker. The same method names work for both local and remote targets — routing is decided by the address's `node` field.

Cross-node uniqueness is structural: addresses are full `Address { name, node }` values, and two nodes physically cannot hold the same address because their `node` fields differ.

## Enable the feature

```bash
cargo add xan-actor --features multi-node
cargo add async-trait     # `#[async_trait::async_trait]` on your `impl Actor`
cargo add thiserror       # idiomatic for `Actor::Error`; not strictly required
cargo add xancode         # needed for `#[derive(Codec)]` on your message/result types
cargo add xanq            # only if you spawn an in-process broker yourself
cargo add xan-log         # optional: a ready-made `log` backend
```

- `xan-actor` does not re-export the `Codec` trait — add `xancode` as a direct dependency in your crate so the derive macro and the trait resolve through the same path (re-exporting through `xan-actor` causes type-resolution mismatches when the proc-macro emits code referencing `xancode::Codec`).
- `xanq` is also a direct dependency only if you intend to bring up your own broker via `xanq::server::Server` (tests, demos, single-binary deployments). Connecting to an externally-running broker only needs the client side, which `xan-actor` handles internally.
- `xan-log` is optional. `xan-actor` logs via the `log` facade, so any backend (`env_logger`, `tracing-log`, etc.) works. If you use `xan-log`, set `LOG_LEVEL=debug` (or `info`/`warn`/...) before running — it defaults to `Off`.

## Step 1 — Make message and result types serializable

The remote path needs `xancode::Codec` on `<T as Actor>::Message` and `<T as Actor>::Result`.

```rust
use xancode::Codec;

#[derive(Debug, Clone, Codec)]
pub enum MyMessage {
    Ping(String),
    Echo(String),
}
```

## Step 2 — Register each actor type once

`register_for_inter_node!` installs the decoder/encoder pair the receiving node needs to turn raw envelope bytes back into `<T as Actor>::Message` and the response back into bytes. Call it once per actor type **at module scope** (not inside a function).

```rust
xan_actor::register_for_inter_node!(MyActor);
```

## Step 3 — Use `Address` everywhere

With `multi-node` enabled, `Actor::address(&self)` returns `&inter_node::Address` instead of `&str`. Your actor stores the full qualified address:

```rust
use xan_actor::prelude::*;   // brings Address, NodeFilter, Topic, ... (Codec comes from xancode)

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

## Step 4 — Construct `ActorSystem` with a node name

`new` requires `node_name`. `broker_addr` is optional — `None` keeps the system local-only.

```rust
// bounded-channel + multi-node
let mut system = ActorSystem::new(
    None,                              // channel_size
    "node-a".into(),                   // node_name
    Some("127.0.0.1:7777".into()),     // broker_addr
).await?;

// unbounded-channel + multi-node
let mut system = ActorSystem::new("node-a".into(), Some("127.0.0.1:7777".into())).await?;
```

### Spawning your own broker (in-process)

`xan_actor::Address` implements `xanq::address::Address` (delivery mode `Anycast`), so you can hand our `Address` directly to xanq as the Server's type parameter — no separate newtype:

```rust
use xan_actor::prelude::*;        // Address, ...
use xanq::server::Server;

let (_server, addr) = Server::<Address>::spawn("127.0.0.1:0").await.expect("broker");
let broker = addr.to_string();    // hand this to ActorSystem::new
// Keep `_server` (Arc<Server>) bound so the accept loop stays alive.
```

The Server is generic at the API level but type-erased on the wire (bytes), so it routes `xan_actor`'s internal `Topic { node, kind }` traffic just fine. Picking `Address` here is purely ergonomic for the user-facing call site.

## Step 5 — Use the same API as single-node

```rust
use xan_actor::prelude::*;   // Address, NodeFilter, ...
use std::time::Duration;

// Same node — local fast path, no broker round trip.
system
    .send::<MyActor>(Address::new("node-a", "/echo/1"), MyMessage::Ping("hi".into()))
    .await?;

// Different node — encoded and shipped over the broker.
let result = system
    .send_and_recv::<MyActor>(Address::new("node-b", "/echo/1"), MyMessage::Ping("hi".into()))
    .await?;

// Same call with a deadline — recommended for cross-node so a dead peer
// can't park the call indefinitely.
match system
    .send_and_recv_with_timeout::<MyActor>(
        Address::new("node-b", "/echo/1"),
        MyMessage::Ping("hi".into()),
        Duration::from_secs(3),
    )
    .await
{
    Ok(reply) => println!("got {reply:?}"),
    Err(ActorError::Timeout(d)) => eprintln!("peer didn't reply within {d:?}"),
    Err(e) => return Err(e),
}

// Broadcast across an explicit peer set.
let results = system
    .send_broadcast::<MyActor>(
        "/echo/*".into(),
        NodeFilter::Peers(vec!["node-a".into(), "node-b".into()]),
        MyMessage::Ping("bcast".into()),
    )
    .await;
// results.local.len()  — exact number of local actors that received it
// results.remote.len() — number of remote peer nodes we sent envelopes to
//                        (NOT the number of remote actors that received it)
// results.all_ok()     — every entry succeeded; for `remote`, this only
//                        confirms envelope acceptance, not actor reach
```

`NodeFilter` variants:

- `SelfOnly` — regex match against this node's local actors. No broker traffic.
- `Node(name)` — single named target. If `name` equals this node, behaves like `SelfOnly`.
- `Peers(Vec<name>)` — union of listed nodes. Duplicates are deduped; entries equal to this node turn into local fan-out (no extra envelope to ourselves).

`send_broadcast` (multi-node) returns `BroadcastResult { local, remote }`, and the two fields **count different things**:

- `local.len()` — exact number of local actors that received the message.
- `remote.len()` — number of remote peer nodes you shipped a `BroadcastFire` envelope to. Each peer then runs its own regex match locally and dispatches to 0..N of its actors, but it's **fire-and-forget**, so no per-actor confirmation comes back. There's no way to tell from `results` how many remote actors actually got it.

`BroadcastResult::iter()` chains `local` then `remote` for terse "any failure?" checks, but the chained entries have different meanings — prefer `all_ok()` for a global verdict, or read `local` / `remote` separately when the distinction matters.

That's the intended trade-off: broadcasts stay a single one-shot envelope per peer, with no round trip. If you need an accurate cluster-wide actor count, this API can't give it to you — that would require a request/response variant where each peer replies with its own match count.

## Register-time rejection of foreign addresses

```rust
RemoteActor { addr: Address::new("node-b", "/foreign") }
    .register(&mut node_a, ...).await
// → Err(ActorError::AddressNotOwned("node-b:/foreign"))
```

The Register handler validates `address.node == self.node_name`. The duplicate-address race that would exist in an auto-discovery model can't happen here — two nodes physically can't register the same `Address`.

## Wire Protocol

```text
Caller node                                  Owner node
-----------                                  -----------
send_and_recv::<T>(addr, msg)
  -> if addr.node == self_node:
       local mailbox path (no broker)
  -> else:
       encode(msg)
       InterNodeMessage::Call {
         actor_type, target_name,
         reply_to, req_id, payload
       }
  -> produce(Topic::request(addr.node))     -> consumer task picks it up
                                              -> registry decodes payload
                                              -> dispatch_local_any_and_recv
                                              -> registry encodes result
                                              -> InterNodeResponse { req_id, outcome }
                                              -> produce(Topic::response(caller_node))
  consumer task picks it up
  -> match req_id in pending map
  -> resolve oneshot -> caller decodes bytes -> T::Result
```

Each node subscribes to `Topic::request(self)` and `Topic::response(self)` (both `Anycast`). Outgoing envelopes carry `actor_type`, `target_name` (just the name part; node is implicit from the request topic), encoded payload, and for `Call` also `reply_to` + `req_id`. `BroadcastFire` is similar but the receiver runs a regex match against its local actors and dispatches each match.

There is no discovery channel and no shared directory.

## Notes and current limitations

- A missing `register_for_inter_node!` call surfaces as `ActorError::InterNodeDecoderMissing` the first time an envelope arrives for that actor type. All `register_for_inter_node!` calls must be at module scope (not inside fns) because they expand to `inventory::submit!`.
- Calling `send` / `send_and_recv` with `address.node != self_node` while the system was created with `broker_addr = None` returns `ActorError::InterNodeNotConfigured`.
- Cross-node `send_and_recv` has no implicit failure detection — the underlying `oneshot` waiter can only fire when the peer publishes a response back, so a dead peer parks the call indefinitely. Use `send_and_recv_with_timeout(addr, msg, duration)` (or `send_and_recv_without_tx_cache_with_timeout`) to bound the wait (see the example in Step 5 above); on expiry it returns `ActorError::Timeout(duration)` and the dispatcher's pending-requests entry is reclaimed via a `Drop` guard.
- The initial broker connection is bounded by `inter_node::DEFAULT_BROKER_CONNECT_TIMEOUT` (5 s). If the broker is missing or unreachable, `ActorSystem::new` fails with `ActorError::InterNodeIo("broker connect to ... timed out after 5s")` instead of hanging on the OS-default TCP connect timeout. `InterNodeRuntime::connect_with_timeout` is available if you need a different cap.
- Node membership for `NodeFilter::Peers` is supplied by the caller. The library doesn't track which peers are alive; sending to a non-subscribed `Topic::request(node)` will queue the envelope in the broker until someone subscribes.
- The address is fully qualified, so location transparency is by design *less* than in a single `ActorSystem`. Callers must know which node owns the actor they're calling. In return, there's no race window or eventual-consistency window — routing is decided by the address itself.
