# Multi-node

`multi-node` feature는 하나의 `ActorSystem`이 [xanq](https://crates.io/crates/xanq) 브로커를 거쳐 다른 프로세스나 머신의 actor로 `send::<T>` / `send_and_recv::<T>` 호출을 라우팅하도록 합니다. 메서드 이름은 로컬과 원격 모두 동일하며, 라우팅은 주소의 `node` 필드가 결정합니다.

노드 간 주소 유일성은 구조적으로 보장됩니다 — 주소가 `Address { name, node }` 전체 형태라서 두 노드가 물리적으로 같은 주소를 가질 수 없습니다(`node` 필드가 다르므로).

## Feature 활성화

```bash
cargo add xan-actor --features multi-node
cargo add async-trait     # `impl Actor`에 `#[async_trait::async_trait]` 붙이기 위해
cargo add thiserror       # `Actor::Error` 작성에 관용적 (필수는 아님)
cargo add xancode         # 메시지/결과 타입에 `#[derive(Codec)]` 적용을 위해 필요
cargo add xanq            # 직접 broker를 띄울 때만 (테스트/데모 등)
cargo add xan-log         # 선택: 손쉬운 `log` 백엔드
```

- `xan-actor`는 `Codec` trait을 re-export하지 않습니다 — derive 매크로가 emit하는 코드가 `xancode::Codec`을 참조하기 때문에, 사용자 crate에 `xancode`를 직접 의존으로 추가해야 derive와 trait이 동일 경로로 resolve돼 타입이 맞습니다.
- `xanq`도 `xanq::server::Server`로 직접 broker를 띄우려는 경우(테스트, 데모, 단일 바이너리)에만 사용자 측에서 추가하면 됩니다. 외부 broker에 client로 붙기만 한다면 `xan-actor`가 내부에서 처리하므로 직접 의존 불필요.
- `xan-log`는 선택. `xan-actor`는 `log` facade로 출력하므로 어떤 backend(`env_logger`, `tracing-log` 등)든 됩니다. `xan-log`를 쓴다면 실행 전에 `LOG_LEVEL=debug`(또는 `info`/`warn`/...) 환경변수를 설정하세요 — 기본값이 `Off`라 아무것도 안 찍힙니다.

## Step 1 — 메시지/결과 타입 직렬화 가능하게

원격 경로는 `<T as Actor>::Message`와 `<T as Actor>::Result`에 `xancode::Codec`이 필요합니다.

```rust
use xancode::Codec;

#[derive(Debug, Clone, Codec)]
pub enum MyMessage {
    Ping(String),
    Echo(String),
}
```

## Step 2 — Actor 타입당 한 번 등록

`register_for_inter_node!`는 수신 노드가 envelope 바이트를 `<T as Actor>::Message`로 디코드하고, 응답을 다시 바이트로 인코드하는 데 필요한 디코더/인코더 쌍을 등록합니다. **모듈 스코프**에서 actor 타입당 한 번씩 호출하세요(함수 내부 ❌).

```rust
xan_actor::register_for_inter_node!(MyActor);
```

## Step 3 — 어디에서나 `Address` 사용

`multi-node`가 켜지면 `Actor::address(&self)`는 `&str`이 아니라 `&inter_node::Address`를 반환합니다. Actor가 전체 주소를 보유합니다:

```rust
use xan_actor::prelude::*;   // Address, NodeFilter, Topic 등 (Codec은 xancode에서 직접)

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

## Step 4 — 노드명으로 `ActorSystem` 생성

`new`는 `node_name`이 필수. `broker_addr`은 옵션 — `None`이면 로컬 전용으로 동작합니다.

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

### 인프로세스 broker 직접 띄우기

`xan_actor::Address`가 `xanq::address::Address`를 구현(delivery mode `Anycast`)하므로, 우리 `Address`를 xanq Server의 타입 인자로 그대로 넘길 수 있습니다 — 별도 newtype 불필요:

```rust
use xan_actor::prelude::*;        // Address, ...
use xanq::server::Server;

let (_server, addr) = Server::<Address>::spawn("127.0.0.1:0").await.expect("broker");
let broker = addr.to_string();    // ActorSystem::new에 전달
// `_server`(Arc<Server>)는 binding 유지 — drop되면 accept loop 종료.
```

Server는 API 레벨에서만 제네릭이고 와이어는 바이트 단위 type-erased라 `xan_actor` 내부의 `Topic { node, kind }` 트래픽도 문제없이 라우팅됩니다. `Address`를 고르는 건 사용자 코드의 가독성 차원의 선택일 뿐입니다.

## Step 5 — 단일 노드와 동일한 API 사용

```rust
use xan_actor::prelude::*;   // Address, NodeFilter, ...

// 같은 노드 — 로컬 빠른 경로, 브로커 라운드트립 없음.
system
    .send::<MyActor>(Address::new("node-a", "/echo/1"), MyMessage::Ping("hi".into()))
    .await?;

// 다른 노드 — 인코드 후 브로커 경유.
let result = system
    .send_and_recv::<MyActor>(Address::new("node-b", "/echo/1"), MyMessage::Ping("hi".into()))
    .await?;

// 명시 피어 집합으로 브로드캐스트.
let results = system
    .send_broadcast::<MyActor>(
        "/echo/*".into(),
        NodeFilter::Peers(vec!["node-a".into(), "node-b".into()]),
        MyMessage::Ping("bcast".into()),
    )
    .await;
// results.local.len()  — 메시지를 받은 로컬 actor의 정확한 수
// results.remote.len() — envelope을 보낸 원격 peer 노드의 수
//                        (원격에서 실제로 메시지를 받은 actor 수가 아님)
// results.all_ok()     — 양쪽 모두 Ok면 true
```

`NodeFilter` variant:

- `SelfOnly` — 이 노드의 로컬 actor들과 regex 매칭.
- `Node(name)` — 하나의 명명된 대상(self일 수도, 원격일 수도).
- `Peers(Vec<name>)` — 나열된 노드들의 합집합.

`send_broadcast`(multi-node)는 `BroadcastResult { local, remote }`를 반환합니다. 두 필드는 **세는 대상이 다릅니다**:

- `local.len()` — 메시지를 받은 로컬 actor의 정확한 수.
- `remote.len()` — `BroadcastFire` envelope을 보낸 원격 peer 노드의 수. 각 peer는 자기 로컬 regex 매칭 후 0~N개 actor로 dispatch하지만, **fire-and-forget**이라 per-actor 확인이 호출자에게 안 돌아옵니다. `results`만으로는 원격에서 실제로 메시지를 받은 actor가 몇 개인지 알 수 없습니다.

이건 설계상의 trade-off입니다: broadcast를 round-trip 없는 가벼운 envelope 한 방으로 유지하기 위해 정확도를 포기. 클러스터 전역의 actor 단위 카운트가 필요하다면 이 API로는 불가능하고, 각 peer가 자기 매치 수를 응답으로 돌려주는 request/response 변형이 필요합니다.

## 외부 노드 주소 등록 거부

```rust
RemoteActor { addr: Address::new("node-b", "/foreign") }
    .register(&mut node_a, ...).await
// → Err(ActorError::AddressNotOwned("node-b:/foreign"))
```

Register 핸들러가 `address.node == self.node_name`을 검증해 불일치하면 거부합니다. 자동 디스커버리 모델에서 발생하던 중복 등록 race가 여기선 일어날 수 없습니다 — 두 노드가 물리적으로 같은 `Address`를 가질 수 없으니까요.

## Wire Protocol

```text
호출 노드(Caller)                              소유 노드(Owner)
------------------                            ------------------
send_and_recv::<T>(addr, msg)
  -> if addr.node == self_node:
       로컬 mailbox 경로 (브로커 없음)
  -> else:
       encode(msg)
       InterNodeMessage::Call {
         actor_type, target_name,
         reply_to, req_id, payload
       }
  -> produce(Topic::request(addr.node))      -> consumer task가 수신
                                               -> 레지스트리가 payload 디코드
                                               -> dispatch_local_any_and_recv
                                               -> 레지스트리가 result 인코드
                                               -> InterNodeResponse { req_id, outcome }
                                               -> produce(Topic::response(caller_node))
  consumer task가 수신
  -> pending map에서 req_id 매칭
  -> oneshot 해제 -> 호출자가 bytes를 T::Result로 디코드
```

각 노드는 `Topic::request(self)`와 `Topic::response(self)` 두 `Anycast` 토픽을 구독합니다. 송신 envelope에는 `actor_type`, `target_name`(name 부분만 — node는 request 토픽에서 암묵), 인코딩된 payload, `Call`이면 추가로 `reply_to`와 `req_id`가 담깁니다. `BroadcastFire`는 비슷하지만 수신 측이 자기 로컬 actor들에 regex 매칭 후 각 매치로 디스패치합니다.

디스커버리 채널이나 공유 디렉터리는 **없습니다**.

## 주의사항과 현재 한계

- `register_for_inter_node!` 호출 누락은 해당 actor 타입의 envelope이 처음 도착하는 시점에 `ActorError::InterNodeDecoderMissing`으로 표면화됩니다. 매크로는 `inventory::submit!`로 전개되므로 반드시 모듈 스코프에서 호출해야 합니다(함수 내부 ❌).
- `address.node != self_node`인 송신을 `broker_addr = None`으로 만든 시스템에서 호출하면 `ActorError::InterNodeNotConfigured`.
- 초기 broker 연결은 `inter_node::DEFAULT_BROKER_CONNECT_TIMEOUT`(5초)로 제한됩니다. broker가 없거나 도달 불가면 OS 기본 TCP connect 타임아웃(분 단위) 대신 `ActorError::InterNodeIo("broker connect to ... timed out after 5s")`로 즉시 `ActorSystem::new`가 실패합니다. 다른 값이 필요하면 `InterNodeRuntime::connect_with_timeout`을 직접 사용하세요.
- `NodeFilter::Peers`의 노드 멤버십은 호출자가 제공합니다. 라이브러리는 어느 피어가 살아있는지 추적하지 않습니다; 구독자가 없는 `Topic::request(node)`로 보내면 envelope이 브로커 큐에 누군가 구독할 때까지 쌓입니다.
- 주소가 완전 자격을 갖춤 → 단일 `ActorSystem` 안의 location transparency보다 *명시적*. 호출자는 어느 노드가 actor를 소유하는지 알아야 합니다. 대신 race window나 eventual-consistency window가 없습니다 — 라우팅은 주소 자체로 결정.
