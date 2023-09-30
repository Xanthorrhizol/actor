## Actor - for sync feature

### Usage

1. add actor in Cargo.toml

```toml
xan-actor = { git = "ssh://git@github.com/Xanthorrhizol/actor", branch = "main", feature = ["sync"] }
```

2. create a actor as mutable

```rust
use xan_actor::ActorSystem;
...

let mut actor_system = ActorSystem::new();
```

3. declare Actor to register

```rust
use crate::xan_actor::{Actor, Handler, Message, error::ActorError};

#[derive(Clone, Debug)]
pub enum MyMessage {
  A(String),
  B(String),
  Exit,
}

#[derive(thiserror::Error, Debug)]
enum MyError<T, R>
where
  T: Sized + Send + Clone,
  R: Sized + Send,
{
  #[error("bye")]
  Exit,
  #[error(transparent)]
  ActorError(#[from] ActorError<T, R>),
}

struct MyActor {
  address: String,
}

#[async_trait::async_trait]
impl Actor<MyMessage, (), MyError<MyMessage, ()>, String> for MyActor
where
    Self: Send + Sized + Sync + 'static,
{
  fn address(&self) -> &str {
    &self.address
  }

  fn new(params: String) -> Self {
    Self { address: params }
  }

  fn actor(
    &mut self, msg: MyMessage,
  ) -> Result<(), MyError<MyMessage, ()>> {
    match msg {
      MyMessage::A(s) => {
        println!("got A: {}", s);
      }
      MyMessage::B(s) => {
        println!("got B: {}", s);
      }
      MyMessage::Exit => {
        println!("got Exit");
        return Err(MyError::Exit);
      }
    }
    Ok(())
  }

  fn pre_start(&mut self) {}
  fn pre_restart(&mut self) {}
  fn post_stop(&mut self) {}
  fn post_restart(&mut self) {}
}
```

4. register actor into actor system

```rust
let actor = MyActor::new("some-address".to_string());
actor.register(&mut actor_system);
```

5. use it

```rust
let _ = actor_system.send(
  "some-address".to_string(), /* address */
  MyMessage::A("a".to_string()), /* message */
);
let result = actor_system.send_and_recv(
  "some-address".to_string(), /* address */
  MyMessage::B("b".to_string()), /* message */
);
```

### Job

- If you send message at some time or with some iteration, you can use job

```rust
use xan_actor::JobSpec;
...

let job = JobSpec::new(
  Some(2), /* max_iter */
  Some(std::time::Duration::from_secs(3)), /* interval */
  std::time::SystemTime::now(), /* start_at */
);
if let Some(recv_rx) = actor_system.run_job(
  "some-address".to_string(), /* address */
  true, /* whether subscribe the handler result or not(true => Some(rx)) */
  job, /* job as JobSpec */
  MyMessage::C("c".to_string()), /* message */
) {
    while let Some(result) = recv_rx.recv() {
        println!("result returned");
    }
}
```

## Actor - for tokio feature

### Usage

1. add actor in Cargo.toml

```toml
xan-actor = { git = "ssh://git@github.com/Xanthorrhizol/actor", branch = "main", feature = ["tokio"] }
```

2. create a actor as mutable

```rust
use xan_actor::ActorSystem;
...

let mut actor_system = ActorSystem::new();
```

3. declare Actor to register

```rust
use crate::xan_actor::{Actor, Handler, Message, error::ActorError};

#[derive(Clone, Debug)]
pub enum MyMessage {
  A(String),
  B(String),
  Exit,
}

#[derive(thiserror::Error, Debug)]
enum MyError<T, R>
where
  T: Sized + Send + Clone,
  R: Sized + Send,
{
  #[error("bye")]
  Exit,
  #[error(transparent)]
  ActorError(#[from] ActorError<T, R>),
}

struct MyActor {
  address: String,
}

#[async_trait::async_trait]
impl Actor<MyMessage, (), MyError<MyMessage, ()>, String> for MyActor
where
    Self: Send + Sized + Sync + 'static,
{
  fn address(&self) -> &str {
    &self.address
  }

  async fn new(params: String) -> Self {
    Self { address: params }
  }

  async fn actor(
    &mut self, msg: MyMessage,
  ) -> Result<(), MyError<MyMessage, ()>> {
    match msg {
      MyMessage::A(s) => {
        println!("got A: {}", s);
      }
      MyMessage::B(s) => {
        println!("got B: {}", s);
      }
      MyMessage::Exit => {
        println!("got Exit");
        return Err(MyError::Exit);
      }
    }
    Ok(())
  }

  async fn pre_start(&mut self) {}
  async fn pre_restart(&mut self) {}
  async fn post_stop(&mut self) {}
  async fn post_restart(&mut self) {}
}
```

4. register actor into actor system

```rust
let actor = MyActor::new("some-address".to_string());
actor.register(&mut actor_system).await;
```

5. use it

```rust
let _ = actor_system.send(
  "some-address".to_string(), /* address */
  MyMessage::A("a".to_string()), /* message */
);
let result = actor_system.send_and_recv(
  "some-address".to_string(), /* address */
  MyMessage::B("b".to_string()), /* message */
).await;
```

### Job

- If you send message at some time or with some iteration, you can use job

```rust
use xan_actor::JobSpec;
...

let job = JobSpec::new(
  Some(2), /* max_iter */
  Some(std::time::Duration::from_secs(3)), /* interval */
  std::time::SystemTime::now(), /* start_at */
);
if let Some(recv_rx) = actor_system.run_job(
  "some-address".to_string(), /* address */
  true, /* whether subscribe the handler result or not(true => Some(rx)) */
  job, /* job as JobSpec */
  MyMessage::C("c".to_string()), /* message */
) {
    while let Some(result) = recv_rx.recv().await {
        println!("result returned");
    }
}
```
