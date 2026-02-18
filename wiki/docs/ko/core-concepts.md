# Core Concepts

## 1) Actor

각 Actor는 `Actor` 트레이트를 구현합니다.

- `type Message`: 해당 Actor가 받을 수 있는 메시지 타입
- `type Result`: 핸들러 결과 타입
- `type Error`: 핸들러 에러 타입
- `handle(&mut self, Arc<Self::Message>)`

핵심은 Actor마다 메시지 타입이 독립적으로 정의된다는 점입니다.

## 2) ActorSystem

`ActorSystem`은 주소(`String`)를 기준으로 Actor를 관리합니다.

- 등록: `register(...)`
- 단건 송신: `send::<T>(address, msg)`
- 요청/응답: `send_and_recv::<T>(address, msg)`
- 브로드캐스트: `send_broadcast::<T>(regex, msg)`
- 잡 실행: `run_job::<T>(...)`

## 3) 타입 안정성 모델

### 컴파일 타임 보장

`send::<T>()`의 메시지 인자는 `<T as Actor>::Message`입니다.
즉 `T`를 지정하면 메시지 타입이 컴파일 타임에 고정됩니다.

예를 들어 `send::<MyActor1>(..., MyMessage2::B(...))`는 컴파일되지 않습니다.

### 런타임 보장

주소는 문자열이므로, 주소가 실제로 `T` 타입 Actor를 가리키는지는 런타임에 확인합니다.

- 내부적으로 `actor_type`을 함께 저장
- `FindActor` 시 타입 일치 확인
- 불일치 시 `AddressNotFound` 또는 `MessageTypeMismatch` 등으로 실패 처리

정리하면:
- 메시지 타입 적합성: 컴파일 타임
- 주소-Actor 매핑 적합성: 런타임

## 4) 라이프사이클과 장애 처리

Actor 라이프사이클:

- `Starting`
- `Receiving`
- `Stopping`
- `Terminated`
- `Restarting`

핸들러 에러 발생 시 정책(`ErrorHandling`)에 따라 동작이 달라집니다.

- `Resume`: 현재 Actor 계속 실행
- `Restart`: Actor 재시작
- `Stop`: Actor 종료
