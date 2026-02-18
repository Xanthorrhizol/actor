# xan-actor Wiki

`xan-actor` is an Akka-style Actor model library implemented in Rust.

This wiki focuses on:

- running multiple Actor types in a single `ActorSystem`
- compile-time message type checking via `send::<T>()`
- address-based routing and lifecycle control
- bounded/unbounded channel modes

## Documentation Map

- `Core Concepts`: core concepts and type-safety model
- `Architecture`: internal components and message flow
- `Quickstart`: minimal example for register/send/recv
