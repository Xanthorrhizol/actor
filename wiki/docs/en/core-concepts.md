# Core Concepts

## 1) Actor

Each actor implements the `Actor` trait.

- `type Message`: message type this actor accepts
- `type Result`: handler result type
- `type Error`: handler error type
- `handle(&mut self, Arc<Self::Message>)`

Each actor defines its own independent message type.

## 2) ActorSystem

`ActorSystem` manages actors by address (`String`).

- register: `register(...)`
- send: `send::<T>(address, msg)`
- request/response: `send_and_recv::<T>(address, msg)`
- broadcast: `send_broadcast::<T>(regex, msg)`
- job execution: `run_job::<T>(...)`

## 3) Type-Safety Model

### Compile-Time Guarantees

`send::<T>()` accepts `<T as Actor>::Message`.
Once `T` is chosen, the message type is fixed at compile time.

For example, `send::<MyActor1>(..., MyMessage2::B(...))` does not compile.

### Runtime Guarantees

Addresses are plain strings, so address-to-actor-type matching is validated at runtime.

- system stores `actor_type` with each address
- `FindActor` checks type equality
- mismatch returns runtime errors such as `AddressNotFound` or `MessageTypeMismatch`

In short:

- message type compatibility: compile time
- address mapping compatibility: runtime

## 4) Lifecycle and Failure Handling

Actor lifecycle states:

- `Starting`
- `Receiving`
- `Stopping`
- `Terminated`
- `Restarting`

On handler error, behavior depends on `ErrorHandling`:

- `Resume`: continue actor loop
- `Restart`: restart actor
- `Stop`: terminate actor
