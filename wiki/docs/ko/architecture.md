# Architecture

## 구성 요소

- `Actor` 구현체: 사용자 비즈니스 로직
- `TypedMailbox<A>`: Actor 타입 `A`에 바인딩된 메일박스
- `ActorSystem`: 주소/타입/라이프사이클 관리, 라우팅, 잡 제어
- `actor_system_loop`: 시스템 명령 처리 루프

## 메시지 흐름

```text
Caller
  -> ActorSystem::send::<T>(address, msg: T::Message)
  -> ActorSystemCmd::FindActor { actor_type, address }
  -> Mailbox.send(payload)
  -> TypedMailbox<A> downcast to A::Message
  -> Actor::handle(...)
```

핵심 포인트:

- API 레벨에서 `T::Message`를 요구하므로 잘못된 메시지 타입은 컴파일 에러
- 내부 메일박스는 `Any` 기반이지만, `TypedMailbox<A>`에서 다운캐스트 검증

## 주소와 라우팅

주소는 문자열이며 정규식(`*` 와일드카드 변환)으로 필터링됩니다.

- `send_broadcast::<T>(regex, msg)`는 매칭된 주소 전체에 전송
- 캐시(`cache`)를 사용해 반복 송신 경로를 최적화
- 캐시 타입 불일치/송신 실패 시 캐시 제거 후 재조회

## 잡(Job) 실행 모델

`run_job::<T>()`는 주기/횟수/시작시각을 담은 `JobSpec`을 받아 비동기 작업을 실행합니다.

- `subscribe = true`: 결과 채널(`result_subscriber_rx`) 제공
- 제어 채널: `stop_job`, `resume_job`, `abort_job`

## 채널 모드

기능 플래그로 동일한 인터페이스를 두 방식으로 제공합니다.

- `bounded-channel` (기본): `tokio::sync::mpsc::channel(size)`
- `unbounded-channel`: `tokio::sync::mpsc::unbounded_channel()`

API 형태는 동일하고 내부 채널 타입만 달라집니다.
