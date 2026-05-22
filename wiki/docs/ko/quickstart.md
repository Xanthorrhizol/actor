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

## 멀티 노드 최소 예제

```bash
cargo add xan-actor --features multi-node
cargo add async-trait
cargo add thiserror
cargo add xancode    # `#[derive(Codec)]`에 필요. xan-actor는 re-export 안 함
cargo add xanq       # 이 예제는 broker를 인프로세스로 띄우므로 필요. 외부 broker 사용 시 생략
```

이 예제는 xanq 브로커를 같은 프로세스 안에서 띄워 단일 바이너리로 동작합니다.
두 `ActorSystem` 인스턴스(`node-b`, `node-a`)가 브로커에 붙고, actor는 `node-b`에
살며 `node-a`는 완전 자격을 갖춘 `Address`로 호출합니다. 전체 셋업은
[Multi-node](multi-node.md) 페이지를 참고하세요.

```rust
use std::sync::Arc;
use xan_actor::prelude::*;       // Address, NodeFilter, Topic, ...
use xancode::Codec;              // 사용자 측에서 직접 의존
use xanq::server::Server;

// `Codec`은 메시지/결과가 노드 경계를 넘는 데 필요합니다.
#[derive(Debug, Clone, Codec)]
enum Echo {
    Ping(String),
    Pong(String),
}

#[derive(thiserror::Error, Debug)]
enum E {
    #[error(transparent)]
    Actor(#[from] ActorError),
}

struct EchoActor {
    addr: Address,
}

#[async_trait::async_trait]
impl Actor for EchoActor {
    type Message = Echo;
    type Result = Echo;
    type Error = E;

    fn address(&self) -> &Address { &self.addr }

    async fn handle(&mut self, msg: Arc<Self::Message>) -> Result<Self::Result, Self::Error> {
        match &*msg {
            Echo::Ping(s) => Ok(Echo::Pong(format!("pong:{s}"))),
            other => Ok(other.clone()),
        }
    }
}

// 모듈 스코프 등록. `EchoActor`를 inter-node 디코더/인코더 레지스트리에 연결해서
// 피어 노드가 이 actor 앞으로 온 메시지를 역직렬화할 수 있게 합니다.
xan_actor::register_for_inter_node!(EchoActor);

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<(), ActorError> {
    // 1. xanq 브로커를 인프로세스로 띄움 (포트 0 → OS가 할당).
    //    우리 `Address`가 `xanq::address::Address`를 구현해서 Server 타입 인자로 그대로 사용 가능.
    let (_server, broker_addr) = Server::<Address>::spawn("127.0.0.1:0")
        .await
        .expect("spawn broker");
    let broker = broker_addr.to_string();

    // 2. 소유 노드가 actor를 호스팅. Address.node는 system의 node_name과 일치해야 함.
    let mut node_b = ActorSystem::new(
        None,                       // channel_size
        "node-b".into(),            // node_name
        Some(broker.clone()),       // broker_addr
    ).await?;
    EchoActor { addr: Address::new("node-b", "/echo") }
        .register(&mut node_b, ErrorHandling::Stop, Blocking::NonBlocking, None)
        .await?;

    // 3. 호출 노드.
    let mut node_a = ActorSystem::new(None, "node-a".into(), Some(broker)).await?;

    // 4. API는 단일 노드와 동일 — Address.node 필드가 라우팅을 결정.
    let resp = node_a
        .send_and_recv::<EchoActor>(
            Address::new("node-b", "/echo"),
            Echo::Ping("hi".into()),
        )
        .await?;
    println!("got = {resp:?}");
    Ok(())
}
```

### 단일 노드와 무엇이 다른가

- `Echo`에 `xancode::Codec` derive 추가 — payload 직렬화 가능하게.
- `register_for_inter_node!(EchoActor)`를 모듈 스코프에서 한 번 호출.
- Actor가 `String` 대신 `Address`(구조체)를 보유. `Actor::address`는 `&Address` 반환.
- `ActorSystem::new`가 `async`이고 `node_name` 필수, `broker_addr`은 옵션.
- `send_and_recv::<EchoActor>(Address::new("node-b", "/echo"), ...)` — `Address.node` 필드가 로컬(브로커 없음) / 원격(브로커 경유) 라우팅을 구조적으로 결정.
