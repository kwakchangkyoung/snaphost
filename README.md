# ⚡ SnapHost (스냅호스트) - 프로젝트 기획 및 개발 명세서

> **"손가락을 딱(Snap) 튕기듯 0.001초 만에 전환되는 Apple Silicon 네이티브 초경량 Hosts & 로컬 도메인 매니저"**
>
> 구형 Gas Mask의 직관적인 사용성을 계승하고, Rust의 압도적인 성능과 로컬 포트 포워딩(Reverse Proxy) 기능을 결합한 macOS 전용 메뉴바 유틸리티입니다.

---

## 1. 프로젝트 목표 및 핵심 가치

* **Apple Silicon Native & Ultra-Light**: Electron을 배제하고 순수 네이티브/Rust 기반으로 제작하여 상주 메모리 **15MB 이하**, CPU 점유율 **0%** 유지.
* **Zero-Latency Switching**: 메뉴바에서 프로필을 클릭하는 즉시(1ms 이내) 비밀번호 재입력 없이 `/etc/hosts` 교체 및 DNS 캐시 초기화 완료.
* **Beyond `/etc/hosts` (킬러 기능)**: 기존 `/etc/hosts`의 한계였던 포트 번호 매핑(`my-api.local` → `127.0.0.1:3000`)을 Rust 내장 초경량 리버스 프록시로 해결.

---

## 2. 추천 기술 스택 (Tech Stack)

| 구분 | 기술 스택 | 선정 이유 |
| :--- | :--- | :--- |
| **Core Engine** | **Rust (2024 Edition)** | 초고속 텍스트 파싱, 파일 I/O, 안전한 메모리 관리 및 비동기 프록시 처리 |
| **UI / App Framework** | **Tauri v2** 또는 **SwiftUI + UniFFI** | **옵션 A (Tauri v2)**: Rust와 바로 연동되며 macOS 기본 WebKit 사용으로 용량 5MB 이하, 코드 에디터 구현 용이<br>**옵션 B (SwiftUI + UniFFI)**: 100% 순수 macOS 네이티브 UI 및 `MenuBarExtra` 완벽 활용 |
| **Async Runtime** | `tokio` | 비동기 파일 감시, 원격 Hosts 다운로드, 로컬 프록시 서버 구동 |
| **Reverse Proxy** | `hyper` / `axum` | 도메인별 로컬 포트 포워딩 처리 |
| **Serialization** | `serde`, `serde_json`, `toml` | 프로필 메타데이터 및 앱 환경설정 저장 |
| **File Watcher** | `notify` | 외부 프로그램이 `/etc/hosts`를 임의로 변경했는지 실시간 감지 |

---

## 3. 핵심 기능 명세 (Feature Specifications)

### Phase 1: MVP (핵심 기본 기능 - Gas Mask 완벽 대체)
1. **원본 백업 및 안전장치 (Safety First)**
   * 앱 최초 실행 시 기존 `/etc/hosts` 파일을 `original_hosts.bak`으로 영구 백업.
   * 언제든 원클릭으로 맥 초기 상태로 복구하는 **'안전 모드 복구(Panic Restore)'** 버튼 제공.
2. **프로필(스냅샷) 관리**
   * 로컬 호스트 파일 생성, 이름 변경, 복제, 삭제.
   * 저장 위치: `~/Library/Application Support/SnapHost/profiles/*.hosts`
   * **단일 전환 모드**: 선택한 1개의 프로필로 즉시 교체.
   * **다중 병합 모드 (Combined Profiles)**: 기본(Base) 프로필 + 회사 개발용 + 광고 차단용 등 체크박스로 여러 프로필을 동시에 켜면 Rust 엔진이 실시간으로 병합하여 `/etc/hosts`에 반영.
3. **고속 Hosts 에디터**
   * IP 주소, 도메인, 주석(`#`), 포트 매핑 문법 하이라이팅(Syntax Highlighting).
   * 줄 단위 주석 토글(`Cmd + /`) 단축키 지원으로 특정 도메인만 빠르게 On/Off.
   * 실시간 문법 검증(유효하지 않은 IP 형식이나 잘못된 도메인 문자열 즉시 경고).
4. **권한 위임 및 즉시 반영 엔진**
   * 최초 1회 macOS 관리자 인증(Touch ID / Password)을 통해 `/etc/hosts` 쓰기 권한 확보(또는 Privileged Helper 등록).
   * 파일 변경 즉시 백그라운드에서 DNS 캐시 초기화 자동 실행:
     * `dscacheutil -flushcache`
     * `killall -HUP mDNSResponder`
5. **macOS 메뉴바(System Tray) 통합**
   * 현재 활성화된 프로필 이름을 메뉴바 아이콘 옆에 표시(옵션).
   * 메뉴바 클릭만으로 에디터 창을 열지 않고도 0.1초 만에 프로필 스위치.
   * 글로벌 단축키(예: `Cmd + Shift + H`)로 빠른 전환 팝업 호출.

---

### Phase 2: 킬러 기능 (Rust만의 특화 기능)
1. **로컬 포트 리버스 프록시 (Port-free Localhost)**
   * 에디터에서 확장 문법 지원:
     ```text
     # 기존 hosts 문법
     127.0.0.1       local. legacy-app.com

     # SnapHost 확장 문법 (포트 직접 매핑!)
     127.0.0.1:3000  web.myproject.local
     127.0.0.1:8080  api.myproject.local
     ```
   * 포트 번호가 포함된 라인이 감지되면:
     1. `/etc/hosts`에는 자동으로 `127.0.0.1 web.myproject.local`로 변환하여 기록.
     2. SnapHost 내부의 초경량 Rust 프록시가 `80`(및 `443`) 포트에서 대기하다가 `Host` 헤더를 보고 `:3000` 또는 `:8080`으로 제로 레이턴시 포워딩.
2. **원격(Remote) Hosts 구독 및 초고속 병합**
   * URL을 등록해 두면(예: 사내 공용 개발 도메인 리스트, StevenBlack 광고 차단 리스트) 주기적으로 다운로드 및 업데이트.
   * 수십만 줄의 텍스트도 Rust 멀티스레드로 수 밀리초(ms) 내에 중복 제거 및 병합.
3. **실시간 충돌 및 헬스 체크 (Ping / Port Check)**
   * 현재 활성화된 도메인 옆에 로컬 서버(포트)가 실제로 떠 있는지 초록불/회색불로 상태를 표시.

---

## 4. 권장 폴더 및 아키텍처 구조

```text
SnapHost/
├── README.md                  # 프로젝트 기획 및 명세서 (현재 문서)
├── Cargo.toml                 # Rust 워크스페이스 설정
├── crates/
│   ├── snaphost-core/         # 핵심 비즈니스 로직 (UI 독립적 라이브러리)
│   │   ├── src/
│   │   │   ├── lib.rs
│   │   │   ├── parser.rs      # 초고속 hosts 문법 파서 및 포트 매핑 추출기
│   │   │   ├── merger.rs      # 다중 프로필 병합 및 중복 제거 엔진
│   │   │   ├── system.rs      # /etc/hosts 파일 I/O, 백업, DNS 캐시 플러시
│   │   │   ├── privilege.rs   # macOS 권한 획득 처리
│   │   │   └── proxy.rs       # 로컬 포트 포워딩용 경량 리버스 프록시
│   └── snaphost-cli/          # (선택) 터미널에서 바로 쓰는 CLI 도구 (`snaphost use work`)
└── app/                       # macOS GUI 및 메뉴바 애플리케이션 (Tauri v2 또는 SwiftUI)
```

---

## 5. 단계별 개발 로드맵 (Milestones)

- [x] **Step 1: `snaphost-core` 파서 & 파일 교체 엔진 구현**
  - `/etc/hosts` 읽기, 백업 생성, 다중 텍스트 프로필 병합 로직 작성 및 단위 테스트 완료.
- [x] **Step 2: macOS 권한 처리 및 DNS Flush 연동**
  - 관리자 권한 1회 획득(`osascript` 연동) 후 비밀번호 없이 `/etc/hosts` 업데이트 및 `mDNSResponder` 초기화 구현 완료.
- [x] **Step 3: 메뉴바(Tray) UI 및 프로필 에디터 연결**
  - Tauri v2 기반 메뉴바 아이콘에서 프로필 목록 표시, 체크박스 토글, 메인 에디터 윈도우(`Cmd + /` 주석 토글, `Cmd + S` 즉시 반영) 구현 완료.
- [x] **Step 4: 로컬 포트 포워딩(`127.0.0.1:PORT`) 프록시 엔진 탑재**
  - `Host` 헤더 라우팅 리버스 프록시(`proxy.rs`) 및 로컬 서버 포트 생존 여부(🟢/⚪) 실시간 감지 연동 완료.
- [ ] **Step 5: 패키징 및 최적화**
  - Apple Silicon(`aarch64-apple-darwin`) 릴리즈 빌드 최적화 및 `.app` / `.dmg` 번들링.

---

## 6. 빌드 및 실행 방법 (Quick Start)

```bash
cd /Volumes/m2ssd/bak_workspace/SnapHost

# 1. 코어 엔진 단위 테스트 실행
cargo test -p snaphost-core

# 2. SnapHost 데스크톱 & 메뉴바 앱 실행
cargo run -p snaphost-app
```
