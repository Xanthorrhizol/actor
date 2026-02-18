# Quickstart

## 설치

```bash
cargo add xan-actor
cargo add async-trait
```

`bounded-channel`이 기본입니다.

unbounded를 쓰려면:

```bash
cargo add xan-actor --no-default-features --features unbounded-channel
```

## 최소 예제

```rust
use xan_actor::prelude::*;
use std::sync::Arc;

#[derive(Debug, Clone)]
enum MsgA {
    Ping(String),
}

#[derive(Debug, Clone)]
enum MsgB {
    Echo(String),
}

#[derive(thiserror::Error, Debug)]
enum MyError {
    #[error(transparent)]
    Actor(#[from] ActorError),
}

struct ActorA {
    address: String,
}

struct ActorB {
    address: String,
}

#[async_trait::async_trait]
impl Actor for ActorA {
    type Message = MsgA;
    type Result = MsgA;
    type Error = MyError;

    fn address(&self) -> &str { &self.address }

    async fn handle(&mut self, msg: Arc<Self::Message>) -> Result<Self::Result, Self::Error> {
        Ok((*msg).clone())
    }
}

#[async_trait::async_trait]
impl Actor for ActorB {
    type Message = MsgB;
    type Result = MsgB;
    type Error = MyError;

    fn address(&self) -> &str { &self.address }

    async fn handle(&mut self, msg: Arc<Self::Message>) -> Result<Self::Result, Self::Error> {
        Ok((*msg).clone())
    }
}

#[tokio::main]
async fn main() -> Result<(), ActorError> {
    let mut system = ActorSystem::new(None);

    ActorA { address: "/a/1".into() }
        .register(&mut system, ErrorHandling::Stop, Blocking::NonBlocking, None)
        .await?;

    ActorB { address: "/b/1".into() }
        .register(&mut system, ErrorHandling::Stop, Blocking::NonBlocking, None)
        .await?;

    // 컴파일 타임 검증: MsgA만 보낼 수 있음
    system.send::<ActorA>("/a/1".into(), MsgA::Ping("hello".into())).await?;

    let result = system
        .send_and_recv::<ActorB>("/b/1".into(), MsgB::Echo("world".into()))
        .await?;
    println!("result = {:?}", result);

    Ok(())
}
```

## 실행 포인트

- 하나의 `ActorSystem`에 `ActorA`, `ActorB`를 함께 등록
- `send::<ActorA>`는 `MsgA`만 허용
- `send_and_recv::<ActorB>`는 결과 타입도 `ActorB::Result`로 고정
