# Actor

## Usage

1. add actor in Cargo.toml

```toml
actor = { git = "ssh://git@github.com/Xanthorrhizol/actor", branch = "main" }
```

2. create a actor as mutable

```rust
use actor::Actor;
...

let mut actor = Actor::new();
```

3. declare Handler to register

```rust
use crate::actor::{Actor, Handler, Message, error::ActorError};

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

struct MyHandler {
  address: String,
}

impl MyHandler {
  pub fn new(address: String) -> Self {
    Self { address }
  }
}

#[async_trait::async_trait]
impl Handler<MyMessage, (), MyError<MyMessage, ()>> for MyHandler {
  fn address(&self) -> String {
    self.address.clone()
  }

  async fn handler(
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
}
```

4. register handler into actor

```rust
let handler = MyHandler::new("some-address".to_string());
handler.register(&mut actor);
```

5. use it

```rust
let _ = actor.send(
  "some-address".to_string(), /* address */
  MyMessage::A("a".to_string()), /* message */
);
let result = actor.send_and_recv(
  "some-address".to_string(), /* address */
  MyMessage::B("b".to_string()), /* message */
).await;
```

## Job

- If you send message at some time or with some iteration, you can use job

```rust
use actor::JobSpec;
...

let job = JobSpec::new(
  Some(2), /* max_iter */
  Some(std::time::Duration::from_secs(3)), /* interval */
  std::time::SystemTime::now(), /* start_at */
);
let recv_rx: Option<tokio::sync::mpsc::UnboundedReceiver<()>> = actor.run_job(
  "some-address".to_string(), /* address */
  true, /* whether subscribe the handler result or not(if true, it returns Some(rx)) */
  job, /* job as JobSpec */
  MyMessage::C("c".to_string()), /* message */
);

if let Some(recv_rx) = recv_rx {
    while let Some(result) = recv_rx.recv().await {
        println!("result returned");
    }
}
```
