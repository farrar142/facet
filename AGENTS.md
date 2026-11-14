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
- `crates/facet-bench` — 측정 도구(`facet-bench`): 시험 폴더 생성과 코어 벤치마크.
- `xtask` — 저장소 작업 명령(`cargo xtask`).
- 인덱서 서비스·셸 확장 도우미 같은 다른 실행 파일은 `crates/` 아래 새 크레이트로 추가한다(작업 공간 멤버는 `crates/*`).

명령:

- 빌드: `cargo build`
- 테스트: `cargo test --workspace`
- 앱 실행: `cargo run -p facet-app`
- 자동 검사 전체: `cargo xtask ci` — 형식 검사(`cargo fmt --check`) → lint(`cargo clippy --all-targets`, 경고를 오류로) → 빌드 → 테스트 → 코어 벤치마크를 차례로 돌리고, 한 단계라도 실패하면 0이 아닌 코드로 끝난다. PR을 열기 전에 이것을 통과시킨다.
- 형식 고치기: `cargo fmt --all`

## 시험 폴더와 벤치마크

- 시험 폴더 만들기: `cargo xtask test-folder [경로] [--count N]` — N개(기본 100000) 항목이 든 폴더를 만든다. 경로를 빼면 임시 폴더 아래 `facet-test-folders/items-N`(Windows에서는 `%TEMP%\facet-test-folders\items-N`). CI 캐시에 섞이지 않게 `target/` 밑에 두지 않는다. 이름에 한글·이모지·긴 이름이 섞이고 열 개 중 두 개가 폴더다. 같은 명령을 다시 돌리면 빠진 항목만 채워 결과(이름·종류·크기)가 같다. 시험 폴더의 항목이 아닌 것이 들어 있으면 아무것도 지우지 않고 멈춘다.
- 코어 벤치마크: `cargo xtask bench` — 릴리스 빌드로 `facet-test-folders/items-100000`을 만들거나 확인한 뒤 「위치의 항목 목록 읽기」를 한 번 데우고 5회 재어 중앙값·최악값을 출력한다. `cargo xtask ci`의 마지막 단계이기도 하다.
- 기준과 상한: 로컬 기준은 10만 항목 100ms 미만(`LIST_TARGET`), CI 상한은 그 10배인 1000ms(`LIST_LIMIT`, `crates/facet-bench/src/lib.rs`). 중앙값이 상한을 넘으면 넘은 수치를 출력하고 실패한다. 공유 러너의 편차를 견디면서 알고리즘이 한 단계 나빠지는 실수만 잡으려는 값이다(`docs/roadmap.md`의 「속도는 두 군데서 지킨다」). 미세한 회귀는 잡지 않는다.
- 다른 수나 상한으로 직접 재기: `cargo run --release -p facet-bench -- list <경로> [--count N] [--runs N] [--limit-ms N]`.

CI(`.github/workflows/ci.yml`)는 PR과 `main` push에서 Windows 러너로 `cargo xtask ci`만 부른다. 검사를 바꾸려면 workflow가 아니라 `xtask/src/main.rs`를 고친다. 의존성을 바꾸면 `Cargo.lock`도 함께 커밋한다(검사는 `--locked`로 돈다).
