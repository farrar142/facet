# AGENTS.md

## Agent skills

### Issue tracker

Issues live in GitHub Issues for `farrar142/facet` (via the `gh` CLI). See `docs/agents/issue-tracker.md`.

### Triage labels

Default vocabulary: `needs-triage`, `needs-info`, `ready-for-agent`, `ready-for-human`, `wontfix`. See `docs/agents/triage-labels.md`.

### Domain docs

Single-context: root `GLOSSARY.md` + `docs/adr/`. See `docs/agents/domain.md`.

## 빌드와 검사

Rust는 [rustup](https://rustup.rs)으로 설치한다. 툴체인과 `rustfmt`·`clippy`는 `rust-toolchain.toml`이 고정하므로 처음 `cargo`를 부를 때 rustup이 알아서 받는다. 그 밖의 설정은 없다.

작업 공간 배치:

- `crates/facet-core` — 코어. 화면에 의존하지 않는 라이브러리. 동작과 테스트는 여기에 둔다.
- `crates/facet-app` — 앱 실행 파일(`facet`). 코어를 부르기만 하는 얇은 껍질.
- `crates/facet-index` — 인덱스: 항목 넣기와 질의 실행(이름 부분 일치 + `Filter` 조건 트리: 와일드카드·정규식·경로·크기·날짜·속성·확장자·그리고/또는/아님), 결과의 이름·경로·크기·날짜 정렬과 결과 수 제한, 전체 개수, 이어 받기 표지(`Index::search`·`Index::resume`, 인덱스가 바뀌면 표지는 `ResumeError::Stale`).  USN 저널 레코드 적용(`Index::apply_usn`: 만들기·지우기·이름바꾸기·이동, 보안 변경 폴더 알림)과 저널 상태 판정(`Index::check_journal`, 저널 ID 변경·넘침은 `Freshness::RebuildNeeded`). 화면도 Windows API도 쓰지 않는 순수 크레이트(spec #22의 크레이트 (1)).
- `crates/facet-win` — Windows API 안전 래퍼. 앱 프로세스 쪽에서 `unsafe_code`를 허용하는 유일한 크레이트이고 안전한 함수만 드러낸다(ADR-0002). 셸 호출은 COM을 초기화한 전용 일꾼 스레드(`ShellWorker`)에서 한다. Windows 전용 부분은 `cfg(windows)`로 둔다.
- `crates/facet-bench` — 측정 도구(`facet-bench`): 시험 폴더 생성, 코어 벤치마크, 인덱스 벤치마크.
- `crates/facet-indexer` — 인덱서 서비스 실행 파일(`facet-indexer`). 지금은 개발용 하위 명령 `dump-mft`만 있다. 서비스 쪽 `unsafe`와 Windows API는 이 크레이트에만 둔다(Windows 전용 코드는 `#[cfg(windows)]`). 작업 공간의 `unsafe_code` 금지를 끈 크레이트는 이것과 `facet-win` 둘이다(ADR-0002).
- `xtask` — 저장소 작업 명령(`cargo xtask`).
- 셸 확장 도우미 같은 다른 실행 파일은 `crates/` 아래 새 크레이트로 추가한다(작업 공간 멤버는 `crates/*`).

명령:

- 빌드: `cargo build`
- 테스트: `cargo test --workspace`
- 앱 실행: `cargo run -p facet-app -- [폴더 경로]` — 그 폴더(빼면 현재 폴더)의 항목 목록을 창에 띄운다. 「도움말 → facet 정보」가 정보 화면(`AboutSlint`)이다. 항목을 고르고 F2를 누르면 그 행에 이름 입력란이 뜬다(Enter 확정, Esc 취소). 지금은 입력값을 상태 줄에 보일 뿐 파일 이름은 바꾸지 않는다.
- 자동 검사 전체: `cargo xtask ci` — 형식 검사(`cargo fmt --check`) → lint(`cargo clippy --all-targets`, 경고를 오류로) → 빌드 → 테스트 → 실제 볼륨 테스트(관리자일 때만, 아래) → 코어 벤치마크 → 인덱스 벤치마크를 차례로 돌리고, 한 단계라도 실패하면 0이 아닌 코드로 끝난다. PR을 열기 전에 이것을 통과시킨다.
- 형식 고치기: `cargo fmt --all`
- 배포 산출물 만들기: `cargo xtask dist <label>` — `target/dist/`를 비우고 facet을 release로 빌드해 `facet-<label>-windows-x64.zip`(`facet.exe`, `LICENSE`, `NOTICE.txt`)과 그 `.sha256`을 만든다. `<label>`은 `nightly`나 태그 이름(예: `v0.1.0`). `.github/workflows/release.yml`이 부른다.

## 실제 볼륨 테스트와 MFT 조각

- 실제 볼륨 테스트: 관리자 권한과 NTFS 볼륨이 필요한 테스트. 이름을 `real_volume_`으로 시작하고 `#[ignore = "실제 볼륨 테스트 ..."]`를 붙여 기본 `cargo test --workspace`에서 뺀다. 시험 볼륨의 드라이브 문자는 환경 변수 `FACET_TEST_VOLUME`(예: `T:`)으로 받는다. `cargo xtask ci`는 관리자일 때 시험 VHD(64MB, NTFS)를 `diskpart`로 만들어 붙이고 `cargo test --workspace -- --ignored real_volume_`으로 이 테스트만 돌린 뒤 VHD를 뗀다. 관리자가 아니거나 Windows가 아니면 건너뛴다고 출력한다. GitHub 호스트 Windows 러너는 관리자로 돈다. 예: `crates/facet-indexer/tests/real_volume.rs`.
- MFT 떠 내기: `cargo run -p facet-indexer -- dump-mft <볼륨> <출력 파일> [--start N] [--count N]`(관리자) — 볼륨의 MFT 원시 레코드를 디스크 그대로 파일로 떠 낸다. 관리자가 아니면 그 이유를 알리고 끝난다.
- MFT 조각: `testdata/mft/` — MFT 파서 테스트용 조각과 기대값 목록. `cargo xtask mft-fixture`(관리자)로 다시 만든다. 담긴 경우, 형식, 손으로 따라 하는 방법은 `testdata/mft/README.md`.

## 시험 폴더와 벤치마크

- 시험 폴더 만들기: `cargo xtask test-folder [경로] [--count N]` — N개(기본 100000) 항목이 든 폴더를 만든다. 경로를 빼면 임시 폴더 아래 `facet-test-folders/items-N`(Windows에서는 `%TEMP%\facet-test-folders\items-N`). CI 캐시에 섞이지 않게 `target/` 밑에 두지 않는다. 이름에 한글·이모지·긴 이름이 섞이고 열 개 중 두 개가 폴더다. 같은 명령을 다시 돌리면 빠진 항목만 채워 결과(이름·종류·크기)가 같다. 시험 폴더의 항목이 아닌 것이 들어 있으면 아무것도 지우지 않고 멈춘다.
- 벤치마크: `cargo xtask bench` — 릴리스 빌드로 두 가지를 잰다. `cargo xtask ci`의 마지막 두 단계이기도 하다.
  - 코어 벤치마크: `facet-test-folders/items-100000`을 만들거나 확인한 뒤 「위치의 항목 목록 읽기」를 한 번 데우고 5회 재어 중앙값·최악값을 출력한다.
  - 인덱스 벤치마크: 시험 폴더와 같은 이름의 합성 인덱스 100만 개를 메모리에 만들어 메모리 사용을 출력하고, 부분 일치 질의(결과 100개 제한, 끝쪽 항목 하나만 맞아 인덱스 전체를 훑는 조각)와 정렬 질의(결과 10만 개를 경로 내림으로 정렬해 100개만 받기)를 각각 한 번 데우고 5회 재어 중앙값·최악값을 출력한다.
- 기준과 상한: 로컬 기준은 10만 항목 100ms 미만(`LIST_TARGET`), CI 상한은 그 10배인 1000ms(`LIST_LIMIT`, `crates/facet-bench/src/lib.rs`). 중앙값이 상한을 넘으면 넘은 수치를 출력하고 실패한다. 공유 러너의 편차를 견디면서 알고리즘이 한 단계 나빠지는 실수만 잡으려는 값이다(`docs/roadmap.md`의 「속도는 두 군데서 지킨다」). 미세한 회귀는 잡지 않는다.
- 인덱스 기준과 상한: 질의 로컬 기준 30ms(`QUERY_TARGET`), CI 상한 300ms(`QUERY_LIMIT`). 두 질의 모두 같은 상한을 지킨다. 메모리는 100만 개에 150MB(10진, `INDEX_MEMORY_LIMIT`) 미만이어야 하고 넘으면 실패한다. 메모리는 인덱스가 할당한 용량(`Index::memory_bytes`)으로 센다.
- 다른 수나 상한으로 직접 재기: `cargo run --release -p facet-bench -- list <경로> [--count N] [--runs N] [--limit-ms N]`, `cargo run --release -p facet-bench -- index [--count N] [--runs N] [--limit-ms N]`.

## 화면 수치 측정

콜드 스타트와 스크롤 프레임 시간은 CI가 지키지 않는다. 사람이 자기 기계에서 `cargo xtask measure`로 재어 spec이 끝날 때마다 기록한다(`docs/roadmap.md`의 「화면 수치는 사람이 잰다」).

- 명령: `cargo xtask measure [경로] [--count N] [--runs N] [--scroll-runs N] [--scroll-rows N] [--refresh-hz N] [--json 파일]` — facet과 `facet-bench`를 release로 빌드하고, 시험 폴더(경로를 빼면 10만 항목 기본 시험 폴더, 없으면 만든다)를 열도록 앱을 측정 모드로 여러 번 띄운다.
  - **콜드 스타트**: 측정 도구가 앱을 띄우기 직전부터 목록이 든 첫 프레임을 그리고 창을 화면에 띄운 뒤 이벤트 루프로 돌아올 때까지의 시간을 `--runs`회(기본 5) 재어 중앙값을 낸다. 매번 프로세스를 새로 띄우지만 운영체제 파일 캐시는 비우지 않는다. Slint는 첫 프레임을 창을 띄우기 전에 그리고(그리기 알림은 화면에 내보내기 전에 온다) 그 뒤 창을 띄우는 데 수십 ms가 걸리므로, 그리기 알림 시각이 아니라 그 뒤를 잰다. 앱은 목록 읽기를 창 만들기보다 먼저 시작해 둘을 겹친다.
  - **스크롤**: 첫 화면(위 콜드 스타트의 끝 시각)부터 프레임마다 `--scroll-rows`행(기본 50)씩 목록 끝까지 자동으로 내리며 프레임 사이 시간을 모아 중앙값과 최악값을 낸다. `--scroll-runs`회(기본 1)의 프레임을 합쳐 계산한다. 상한을 넘은 프레임 수도 함께 내어, 최악값 하나만 튄 것(기계가 잠깐 멈춤 등)인지 늘 끊기는 것인지 가른다. 다른 프로그램(녹화·빌드 등)이 CPU를 쓰는 중이면 수치가 흔들리니 닫고 잰다.
  - 대상 폴더와 항목 수는 경로와 `--count`로 정한다. 앱이 읽은 항목 수가 `--count`와 다르면 실패한다.
- 출력: 사람이 읽는 표(표준 출력)와 JSON(`--json`, 기본 `target/measure.json`) 둘 다. 측정 기계의 CPU·메모리·디스크(시험 폴더가 있는 드라이브의 디스크)·Windows 버전과 화면 주사율을 함께 낸다. 주사율을 못 읽으면 60Hz로 가정했다고 표시한다. `--refresh-hz`로 직접 줄 수 있다.
- 통과 기준(결과 옆에 통과 여부로 나온다): 콜드 스타트 중앙값 200ms 미만, 프레임 최악값이 한 프레임 시간(1000/주사율)의 두 배 이하. 못 맞추면 0이 아닌 코드로 끝난다. 상수는 `crates/facet-bench/src/screen.rs`.
- 릴리스 빌드에서 재야 의미가 있다(디버그 빌드면 표 첫 줄에 경고). 창이 실제로 그려지는 데스크톱 세션이 필요하므로 CI에서는 돌리지 않는다.
- 앱 쪽 계측은 환경 변수(`FACET_MEASURE_OUT` 등, 약속은 `crates/facet-core/src/measure.rs`)가 있을 때만 켜진다. 평소 실행은 그리기 알림도 걸지 않으며, 목록이 들어갔다고 원자 변수에 값 하나를 쓸 뿐이다.

CI(`.github/workflows/ci.yml`)는 PR과 `main` push에서 Windows 러너로 `cargo xtask ci`만 부른다. 검사를 바꾸려면 workflow가 아니라 `xtask/src/main.rs`를 고친다. 의존성을 바꾸면 `Cargo.lock`도 함께 커밋한다(검사는 `--locked`로 돈다).

## 한글 IME 체크리스트

화면은 자동 테스트하지 않는다. 한글 입력은 사람이 Windows 기계에서 [`docs/ime-checklist.md`](docs/ime-checklist.md)를 따라 F2 이름 입력란으로 확인한다. Slint나 입력 처리 코드를 바꾸면 이 목록을 다시 돈다.
