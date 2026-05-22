# Architecture

## Components

- `Actor` implementations: user business logic
- `TypedMailbox<A>`: mailbox bound to actor type `A`
- `ActorSystem`: routing, lifecycle, cache, and job control
- `actor_system_loop`: command-processing loop

## Message Flow

```text
Caller
  -> ActorSystem::send::<T>(address, msg: T::Message)
  -> ActorSystemCmd::FindActor { actor_type, address }
  -> Mailbox.send(payload)
  -> TypedMailbox<A> downcast to A::Message
  -> Actor::handle(...)
```

Key points:

- API requires `T::Message`, so wrong message types fail at compile time
- internals use `Any`, but `TypedMailbox<A>` validates via downcast

## Address and Routing

Addresses are strings filtered by regex (`*` wildcard conversion).

- `send_broadcast::<T>(regex, msg)` sends to every matched address
- cache (`cache`) optimizes repeated sends
- cache entry is dropped when type mismatch or send failure occurs

## Job Execution Model

`run_job::<T>()` executes periodic/iterative messages from `JobSpec`.

- `subscribe = true`: returns `result_subscriber_rx`
- control APIs: `stop_job`, `resume_job`, `abort_job`

## Channel Modes

Feature flags expose the same API over different channel backends.

- `bounded-channel` (default): `tokio::sync::mpsc::channel(size)`
- `unbounded-channel`: `tokio::sync::mpsc::unbounded_channel()`

## Inter-node Delivery (optional)

With the `multi-node` feature enabled, addresses become structs
`Address { name, node }` and `ActorSystem` carries a `node_name` plus an
optional `InterNodeRuntime` connected to a xanq broker. Every send/job entry
point routes based on the address itself:

- `address.node == self.node_name` → existing local mailbox flow, no encoding
- otherwise → encode payload, fire/call over the broker

Cross-node uniqueness is structural — two systems cannot physically register
the same `Address` because the `node` fields differ. There is no discovery
protocol, no shared directory, and no race window: routing is decided by the
address itself.

The same `JobController` (`abort_job` / `stop_job` / `resume_job`) and the
same method names work for both local and remote actors. `send_broadcast`
takes a `NodeFilter` (`SelfOnly` / `Node(name)` / `Peers(Vec<name>)`) so the
caller declares which nodes to fan out to. See the
[Multi-node](multi-node.md) page for the wire protocol and setup steps.
