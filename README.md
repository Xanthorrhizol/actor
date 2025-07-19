## Actor

### Usage

1. add actor and dependency in Cargo.toml

```bash
$ cargo add xan-actor
$ cargo add serde --features=derive
```
2. create a actor as mutable

```rust
use xan_actor::ActorSystem;
...

let mut actor_system = ActorSystem::new();
```

3. declare Actor to register

> :bulb: The actor doesn't have to use same message type. Single ActorSystem supports it.

```rust
use crate::xan_actor::{Actor, Handler, Message, ActorError};

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub enum MyMessage1 {
    A(String),
    C(String),
}
#[derive(Debug, serde::Serialize, serde::Deserialize)]
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
    type ActorMessage = MyMessage1;
    type ActorResult = MyMessage1;
    type ActorError = MyError;

    fn address(&self) -> &str {
        &self.address
    }

    async fn actor(&mut self, msg: MyMessage1) -> Result<MyMessage1, MyError> {
        println!("[{}] got MyMessage1: {:?}", self.address(), msg);
        Ok(msg)
    }
}

#[async_trait::async_trait]
impl Actor for MyActor2 {
    type ActorMessage = MyMessage2;
    type ActorResult = MyMessage2;
    type ActorError = MyError;

    fn address(&self) -> &str {
        &self.address
    }

    async fn actor(&mut self, msg: MyMessage2) -> Result<MyMessage2, MyError> {
        println!("[{}] got MyMessage2: {:?}", self.address(), msg);
        Ok(msg)
    }
}
```

4. register actor into actor system

```rust
let actor1 = MyActor1 {
    address: "/some/address/1/1".to_string(),
};
actor1.register(&mut actor_system).await;

let actor2 = MyActor2 {
    address: "/some/address/2".to_string(),
};
actor2.register(&mut actor_system).await;

let actor3 = MyActor1 {
    address: "/some/address/1/2".to_string(),
};
actor3.register(&mut actor_system).await;
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
  Some(2), /* max_iter */
  Some(std::time::Duration::from_secs(3)), /* interval */
  std::time::SystemTime::now(), /* start_at */
);
if let Ok(Some(recv_rx)) = actor_system.run_job::<MyActor1>(
  "/some/address/1/1".to_string(), /* address */
  true, /* whether subscribe the handler result or not(true => Some(rx)) */
  job, /* job as JobSpec */
  MyMessage1::C("c".to_string()), /* message */
).await {
    while let Some(result) = recv_rx.recv().await {
        println!("result returned");
    }
}
```
