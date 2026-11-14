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
- `xtask` — 저장소 작업 명령(`cargo xtask`).
- 인덱서 서비스·셸 확장 도우미 같은 다른 실행 파일은 `crates/` 아래 새 크레이트로 추가한다(작업 공간 멤버는 `crates/*`).

명령:

- 빌드: `cargo build`
- 테스트: `cargo test --workspace`
- 앱 실행: `cargo run -p facet-app`
- 자동 검사 전체: `cargo xtask ci` — 형식 검사(`cargo fmt --check`) → lint(`cargo clippy --all-targets`, 경고를 오류로) → 빌드 → 테스트를 차례로 돌리고, 한 단계라도 실패하면 0이 아닌 코드로 끝난다. PR을 열기 전에 이것을 통과시킨다.
- 형식 고치기: `cargo fmt --all`

CI(`.github/workflows/ci.yml`)는 PR과 `main` push에서 Windows 러너로 `cargo xtask ci`만 부른다. 검사를 바꾸려면 workflow가 아니라 `xtask/src/main.rs`를 고친다. 의존성을 바꾸면 `Cargo.lock`도 함께 커밋한다(검사는 `--locked`로 돈다).
