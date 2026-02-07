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
