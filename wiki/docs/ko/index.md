# xan-actor Wiki

`xan-actor`는 Akka 스타일 Actor 모델을 Rust로 구현한 라이브러리입니다.

이 위키는 아래 내용을 중심으로 정리합니다.

- 하나의 `ActorSystem`에서 여러 Actor 타입을 함께 운용하는 방식
- `send::<T>()` 호출 시 컴파일 타임에 메시지 타입이 검증되는 구조
- 주소 기반 라우팅과 라이프사이클(재시작/중지/삭제) 처리
- bounded/unbounded 채널 모드 차이

## 문서 맵

- `Core Concepts`: 핵심 개념과 타입 안정성 모델
- `Architecture`: 내부 컴포넌트 구조와 메시지 흐름
- `Quickstart`: 최소 예제로 등록/송신/응답 받기

## 로컬 미리보기

```bash
cd wiki
python -m venv .venv
source .venv/bin/activate
pip install -r requirements.txt
mkdocs serve
```
