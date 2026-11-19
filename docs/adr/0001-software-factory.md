# 0001. 소프트웨어 공장: 모든 서비스를 GitHub에 둔다

- 상태: 채택 (일부 task는 아직 꺼져 있음)
- 날짜: 2026-11-15, 갱신 2026-11-17(#249: 인수 시험·Odin·Verify·비용 장부·알림 다리), 2026-11-18(#359: Odin의 충돌 풀기, #389: 티켓 후보·장부·부모·workflow 티켓)

## 맥락

facet은 Windows 파일 탐색기를 대체할 앱이다. 스택은 아직 정하지 않았다(따로 grill한다). 풀려는 문제는 세 가지다.

1. ready 티켓을 구현할 사람이 없다.
2. spec이 한 run에 담기에 너무 크다.
3. 새 티켓이 트리아지되지 않고 쌓인다.

## 결정

### 서비스

| 서비스 | 제품 |
| --- | --- |
| Ticket tracker | GitHub Issues (`farrar142/facet`, 라벨은 `docs/agents/triage-labels.md`) |
| Ticket source | 없음. 사람이 직접 등록한다 |
| Git host | GitHub (`farrar142/facet`) |
| Orchestrator | GitHub Actions (workflow 파일이 곧 orchestrator) |
| Runner | Actions 호스티드 러너. Implementer·Odin·Verify는 `windows-latest`, 나머지는 `ubuntu-latest` |
| Observability platform | 없음. Actions run 페이지로 충분하다 (의도한 선택) |

- Implementer run은 Windows에서 빌드·테스트해야 하므로 `windows-latest`를 쓴다. 공개 저장소라 Actions 분(minute)은 무료이고, 호스티드 job 하나는 최대 6시간이다.
- 에이전트는 `anthropics/claude-code-action`으로 돌린다. 인증은 구독 OAuth 토큰(`claude setup-token` → secret `CLAUDE_CODE_OAUTH_TOKEN`)이다.

### Task

| Task | Trigger | 하는 일 | 다음 run | 사람이 다시 들어오는 곳 | 중복 방지 |
| --- | --- | --- | --- | --- | --- |
| Decompose | `spec` 라벨, 수동 실행(`workflow_dispatch`, spec 번호), 또는 재개 dispatch | 상태를 정한다(`decompose.sh state`: 새 spec, 이미 분해됨, 스펙 PR 있음, 질문 대기). 새 spec이면 하위 이슈(티켓)와 blocked-by를 만들고, 티켓별 인수 시험을 `crates/<crate>/tests/spec_*.rs`에 `#[ignore = "#<티켓>"]`으로 쓴다. 해법이 갈리면(사용자에게 보이는 선택, 앞 spec의 시험·고정값과 충돌) 고르지 않고 spec에 `<!-- factory-question -->` 댓글과 `needs-decision`을 남기고 끝낸다. 지시는 `.github/factory/decompose.md` | workflow가 `factory/spec-<S>`에 시험을 커밋해 스펙 PR(라벨 `spec-pr`)을 열고 Odin을 dispatch한다. `ready-for-agent`는 붙이지 않는다 | `needs-decision`: 답 댓글 뒤 라벨을 떼면 재개 | spec 번호로 concurrency group. 열린 스펙 PR이 있으면 에이전트 없이 Odin만 dispatch한다 |
| Odin | Decompose·재개의 dispatch(입력 spec) | 스펙 PR 브랜치에 main을 merge하고(충돌은 에이전트가 푼다, #359), 티켓·인수 시험을 기존 코드·앞 spec의 시험·AGENTS.md·ADR·GLOSSARY와 대조해 고칠 수 있는 결함은 직접 고친다(시험, 티켓 본문, 계약 이름의 `todo!("#<티켓>")` 스텁). 범위·사용자 선택을 바꿔야 하면 Decompose처럼 묻는다. workflow가 `cargo fmt --check`·`cargo clippy -D warnings`(인수 시험 컴파일 확인)를 돌리고 실패하면 한 번 더 고칠 기회를 준다. 지시는 `.github/factory/odin.md` | 스펙 PR을 squash 병합하고 열린 티켓에 `ready-for-agent`를 붙여 사슬 첫 티켓을 dispatch한다. 시험이 없는 빈 스펙 PR은 닫고 곧바로 사슬을 시작한다 | 검사가 두 번 실패하거나 main과의 충돌 풀기가 한도(2회)를 넘으면 spec에 `ready-for-human`과 `<!-- factory-human -->`(충돌이면 충돌 파일을 적는다) | spec 번호로 concurrency group |
| Implementer | `ready-for-agent` 라벨, 수동 실행, Verify·`frontier`·예약 실행(KST 00:05, 하루 상한 뒤 재개)의 dispatch | 열린 blocker가 없을 때만 시작한다. 에이전트는 자기 티켓의 `#[ignore = "#N"]`을 떼고 `todo!("#N")` 스텁을 구현해 인수 시험을 통과시킨다. 인수 시험의 다른 줄은 고치지 않는다. workflow가 `cargo xtask ci`를 다시 돌리고 통과하면 push·PR(`Closes #N`, `<!-- factory-checked <sha> -->`)까지 하고 **병합하지 않는다**. 지시는 `.github/factory/implementer.md`. Verify가 충돌로 되돌린 티켓은 **충돌 모드**(아래 「되돌림 종류와 한도」)로 다시 구현하지 않고 열린 PR 브랜치에 main을 merge해 풀기만 한다 | Verify를 dispatch한다. 실패면 다음 티켓을 dispatch한다(사슬). 트랙이 둘 이상이면 넘긴 뒤에도 빈 트랙을 채운다 | 실패한 run 2번이면 `ready-for-human` | 트랙마다 concurrency group `factory-impl-<트랙>`(아래 「트랙」, `FACTORY_TRACKS=1`이면 예전처럼 `factory-implementer` 하나를 Verify와 공유)과 티켓마다 `factory-ticket-<N>`. 시작 시 `ready-for-agent`를 떼고 `in-progress`를 붙인다 |
| Verify | Implementer·재개의 dispatch(티켓, PR) | 기계 검사(`verify.sh mech`: 그 티켓의 ignore·스텁이 남지 않음, `spec_*.rs`의 변경이 그 티켓 ignore 줄 삭제뿐), 필요하면 main을 merge한 뒤 `cargo xtask ci`(2026-11-18 #307부터 rebase가 아니라 merge: 충돌 모드가 푼 merge 커밋을 rebase하면 같은 충돌이 다시 난다), Sonnet의 인수 판단(기준마다 충족, 범위 밖 변경 없음). 지시는 `.github/factory/verify.md` | 통과면 squash 병합, 티켓 닫기, 다음 티켓 dispatch. 문제면 티켓에 `<!-- factory-sendback kind=<acceptance|conflict> pr=<PR> -->`과 사유, `ready-for-agent`로 되돌려 `dispatch-next.sh --first N`이 빈 트랙에 먼저 넣음(Opus). 인수 실패는 PR을 닫고, 충돌은 PR을 열어 둔다. 트랙이 둘 이상이면 끝날 때 기다리는 PR의 Verify를 띄움(`verify.sh next-verify`) | 인수 실패 2번이나 충돌 3번이면 `ready-for-human`과 `<!-- factory-human -->`(어느 한도인지 적음) | 직렬: concurrency group `factory-verify`(`FACTORY_TRACKS=1`이면 `factory-implementer`) |
| 재개 | spec에서 `needs-decision`·`factory-paused`가 떨어질 때, 수동 실행 | `resume.sh`가 멈춘 단계를 정해 Decompose·Odin·Verify·다음 티켓 중 하나를 dispatch한다 | 그 단계 | 없음 | 가드가 아직 막으면 아무것도 하지 않는다 |
| Explore | 이슈 생성 (`needs-triage`) | 코드베이스와 대조해 분류한다 | 쉬운 건은 `ready-for-agent`, 나머지는 `ready-for-human` / `needs-info` / `wontfix` | 라벨 변경 | 이슈 번호로 concurrency group. `needs-triage`가 있을 때만 실행한다 |

**모델 배정**: Decompose가 티켓마다 `model:sonnet` 또는 `model:opus` 라벨을 하나 붙인다. 기준은 `.github/factory/decompose.md`에 있다. Implementer는 그 라벨대로 모델을 쓰고, 라벨이 없으면 Sonnet을 쓴다. 기본은 Sonnet이고 Opus는 동시성·unsafe·Windows API·OLE 안전성처럼 어려운 티켓에만 붙인다(2026-11-16). 코드 실패(자동 검사·병합 실패)로 다시 돌릴 때는 Opus로 올리고, 코드와 무관한 실패(러너·네트워크·설치)는 같은 모델로 다시 돌린다(세 번이면 관리 세션으로). Fable은 쓰지 않는다. Decompose·Odin은 Opus, Verify는 Sonnet으로 돈다.

같은 티켓에서 실패한 run이 2번 쌓이거나, Verify가 인수 실패로 2번(`ACCEPTANCE_LIMIT`) 또는 main과의 충돌로 3번(`CONFLICT_LIMIT`, `verify.sh`) 되돌리면 더 돌리지 않고 `ready-for-human`을 붙인다. 두 되돌림은 따로 센다(아래 「되돌림 종류와 한도」). 이 라벨은 「공장 밖에서 관리 세션이 처리」라는 뜻이다. 대표에게 넘기지 않는다(대표 지시 2026-11-15). 관리 세션이 원인을 찾아 공장을 고치거나 직접(Opus 서브에이전트로) 구현한다. 「사람이 손으로 확인할 것」 목록과 측정 티켓도 관리 세션이 이 기계에서 앱을 띄워 처리한다. 대표에게는 제품 방향처럼 결정이 필요한 것만 묻는다.

### 켜는 순서

브레이크(자동 검사·사람 리뷰)가 신뢰를 얻는 만큼만 트리거를 켠다.

1. **Decompose (2026-11-15 켬)**: 코드를 빌드하지 않으므로 CI 없이 켠다. 티켓에 `ready-for-agent`를 붙이지 않으므로 다른 run을 트리거하지 않는다. 첫 대상은 S0(#1)이다. S0 티켓은 공장 밖(로컬 implement)에서 구현하므로 라벨 없이 둔다.
2. **Implementer**: 전제 조건은 자동 검사다. 빌드·lint·test CI workflow를 만든다(S0, #1). 자동 검사 전체는 명령 하나 `cargo xtask ci`로 돈다. `.github/workflows/ci.yml`도 Implementer run도 이 명령을 쓴다. 자동 검사가 없으면 Implementer를 켜지 않는다. 2026-11-15에 켰다. 대표 결정으로 S0의 #3 #5 #6 #7 #8을 맡기고, **병합 전 사람 리뷰를 뺐다**. 브레이크는 `cargo xtask ci` 하나뿐이다. 그래서 같은 실수가 반복되면 사람 리뷰를 다시 넣기보다 검사를 더 엄격하게 만들거나 Automated review task를 넣는다.
3. **Explore**: 외부 사용자가 이슈를 올리기 시작하면 켠다. 처음에는 일부 이슈에만 적용한다.

2026-11-17(#249)에 브레이크를 더했다: 인수 시험을 구현 전에 고정하고(스펙 PR), Odin이 분해 결과를 검토하고, Verify가 병합한다. 2026-11-15의 「분해 결과 검토 없음」과 「병합 전 리뷰 없음」은 이것으로 바뀌었다(아래 「인수 시험·검토·비용」). Verify의 되돌림이 Rework task 몫을 한다.

### 빌드 산출물 (2026-11-15, facet-e3 세션과 합의, 대표 승인)

- `.github/workflows/release.yml`(공장 쪽)이 `cargo xtask dist <label>`(제품 쪽)을 부른다. 이 명령은 `target/dist/facet-<label>-windows-x64.zip`과 `.zip.sha256`을 만든다. zip에는 `facet.exe`, `LICENSE`, `NOTICE.txt`(Slint 표시)가 들어간다. x64만 만든다.
- **nightly**: 움직이는 `nightly` 태그의 prerelease다. 자산 이름은 `facet-nightly-windows-x64.zip`으로 고정해 내려받는 주소가 바뀌지 않는다. 본문에는 「에이전트가 사람 리뷰 없이 병합한 빌드」 경고, SHA, 빌드 시각, 미서명 안내, Slint 배지(ADR-0002 표시 의무 (b))를 적는다.
  - 트리거: Implementer가 병합한 직후의 dispatch(GITHUB_TOKEN 병합은 push 이벤트를 내지 않으므로), 사람의 main push, 하루 한 번(03:00 KST). 예약 실행은 nightly가 이미 main SHA를 가리키면 건너뛴다.
- **정식 릴리스**: 사람이 `v*` 태그를 push하면 `facet-v0.1.0-windows-x64.zip` 같은 이름으로 Release를 만들고, 릴리스 노트는 자동 생성한다.
- PR에는 산출물을 올리지 않는다. `contents: write`는 `release.yml`에만 있다.
- 미룬 것: 코드 서명(그동안 SmartScreen 경고가 뜬다), 설치 프로그램(MSIX/MSI), 자동 업데이트. 인덱서 서비스는 설치와 권한 상승이 필요해 S18(설치 프로그램) 전까지 zip에 넣지 않는다. 그동안 facet은 서비스 없이 강등 동작으로 돈다.

## 결과

- 슬롯 하나를 제품 하나(GitHub)가 대부분 채우므로, 관리할 서버가 없다.
- Claude Code cloud 같은 번들 플랫폼은 Linux라 Windows 빌드·테스트를 못 하므로 쓰지 않았다.
- 자체 호스트 Windows 러너는 공개 저장소에서 외부 PR이 내 기계에서 돌 위험이 있어 쓰지 않았다. 빌드가 너무 느려지면 다시 검토한다.
- 구독 OAuth 토큰을 쓰므로 공장 run이 구독 사용량을 함께 쓴다.

### spec 사슬 (2026-11-15, 대표 결정)

대표가 자는 동안에도 공장이 멈추지 않게, 다음 spec으로 넘어가는 일도 공장이 한다.

- Implementer run이 끝날 때 다음 티켓이 없고, `in-progress`도 막힌 `ready-for-agent`도 남지 않았으면 다음 spec으로 넘어간다.
- 먼저 하위 이슈가 모두 닫힌 spec 이슈를 닫는다. 그다음 저장소 변수 `FACTORY_SPEC_QUEUE`(로드맵 순서, 비어 있으면 다음 spec으로 넘어가지 않음)에서 아직 `spec` 라벨이 없는 첫 spec에 라벨을 붙이고 Decompose를 dispatch한다. 분해 중인 spec(하위 이슈 0개)이 있으면 기다린다. 라벨을 붙인 spec이 큐의 마지막이면 장부 이슈에 `<!-- factory-queue-empty S -->`, 큐의 spec이 모두 라벨을 받아 띄울 것이 없으면 `<!-- factory-idle Q -->`(Q는 큐 번호를 `-`로 이음, 비면 `none`)를 같은 표식마다 한 번 남겨 알림 다리가 보낸다(댓글 실패는 경고만).
- Decompose는 스펙 PR을 열고, Odin이 그것을 병합한 뒤 티켓에 `ready-for-agent`를 붙이고 사슬을 시작한다(라벨 검색 색인이 늦을 수 있어 방금 승인한 티켓 번호를 `dispatch-next.sh --candidates`로 넘긴다).
- 두 번 실패한 티켓(`ready-for-human`)은 사슬을 멈추지 않는다. 관리 세션이 따로 처리한다.

### 트랙 (2026-11-18, #286)

Implementer를 여러 줄(트랙)로 나란히 돌린다. Verify는 직렬로 둔다.

- **트랙 수**는 저장소 변수 `FACTORY_TRACKS`다. 없으면 2, 숫자가 아니거나 0이면 경고하고 2. `1`이면 예전과 같다: Implementer와 Verify가 group `factory-implementer` 하나를 나누고, Implementer는 Verify에 넘긴 뒤 다음 티켓을 띄우지 않으며, spec은 하나씩이다.
- **바쁜 트랙 세기**: Implementer run의 제목(`run-name`)이 `implement #N (track T)`이다. `dispatch-next.sh`가 끝나지 않은 run 목록(`gh run list`, 자기 run 제외)에서 바쁜 트랙과 티켓을 읽고, 빈 트랙에만 `-f track=T`로 dispatch한다. 트랙마다 group `factory-impl-<T>`라 빈 트랙에 넣은 run은 곧바로 돈다.
- **채우는 차례**: 빈 트랙을 먼저 티켓으로 채운다(`--first N`으로 넘긴 되돌림 티켓, 그다음 번호 순). 남은 빈 트랙이 있고 도는 spec 수가 트랙 수보다 적으면 큐의 다음 spec을 연다. 그래서 열린 spec 수는 트랙 수를 넘지 않는다.
- **같은 티켓 두 번 dispatch 막기**: 이미 run이 있는 티켓은 건너뛰고, dispatch한 뒤 그 run이 목록에 보일 때까지(`FACTORY_DISPATCH_SEEN_WAIT`, 기본 60초) 기다려 다음 호출이 그 트랙·티켓을 바쁘게 본다. 그래도 겹치면 티켓마다 group `factory-ticket-<N>`과 시작 시 라벨 가드(`ready-for-agent` → `in-progress`)가 두 번째 run을 멈춘다.
- **Verify는 직렬**이다(group `factory-verify`). group에는 대기 run이 하나만 남고 새 대기가 앞 대기를 취소하므로, 트랙이 둘 이상이면 Verify가 끝날 때마다 `verify.sh next-verify`가 `factory-checked` PR 가운데 번호가 가장 낮고 티켓이 `in-progress`인 것의 Verify를 띄운다(다른 Verify가 돌거나 기다리면 띄우지 않는다). 취소된 대기 run의 PR은 이것으로 다시 잡힌다.
- **예산**: 가드는 run을 시작할 때 장부 합계를 본다. 트랙이 나란히 돌면 이미 시작한 run의 비용은 끝날 때 기록되므로 상한을 run 하나쯤 넘을 수 있다.
- **남은 위험**: 두 `dispatch-next.sh`가 같은 순간에 같은 빈 트랙을 고르면 한 run이 대기로 들어가 다른 dispatch에 취소될 수 있다(티켓은 `ready-for-agent`로 남아 다음 호출이 다시 넣는다). 이 변경 전에 띄운 제목 없는 run은 바쁜 트랙으로 세지 않는다.

### 인수 시험·검토·비용 (2026-11-17, 대표 결정, spec #249)

자율 그래프 시범 두 번(facet-graph)의 교훈을 들였다. 뼈대(`spec` 라벨 → Decompose → Implementer 직렬 사슬)는 그대로다. 원장은 이슈이고 `specs/` 폴더는 두지 않는다.

- **인수 시험을 먼저 고정한다.** Decompose가 티켓별 인수 시험을 실제 시험 자리에 `#[ignore = "#<티켓>"]`으로 쓰고 스펙 PR로 낸다. Odin이 검토해 병합한다(계약의 아직 없는 이름은 `todo!("#<티켓>")` 스텁으로 두어 main이 늘 컴파일된다). Implementer는 자기 티켓의 ignore를 떼고 스텁을 구현할 뿐 시험을 고치지 않고, Verify의 기계 검사가 이것을 지킨다. 구현자가 시험을 약하게 만들어 통과하는 길이 막힌다.
- **인수 시험은 옳은 까닭으로 빨강이어야 한다**(2026-11-19, #414): Odin의 검사 끝에 인수 탐침(`.github/factory/acceptance-probe.sh`)이 스펙 PR이 더하거나 바꾼 `#[ignore = "#N"]` 시험을 `--ignored`로 한 번 돌린다. 단언 실패와 `todo!`·`unimplemented!` 스텁 panic만 통과이고, 컴파일 오류·시험을 못 찾음·그 밖의 panic(`unwrap`, 픽스처 없음)·이미 초록은 실패다(구현해도 초록이 될 수 없거나 아무것도 단언하지 않는다). 실패하면 fmt·clippy와 같이 Odin이 한 번 고치고, 두 번째도 실패면 사람에게 넘긴다. 바뀐 시험만 보므로 새 시험이 이미 병합된 spec 시험과 맞서는 경우(#321)는 잡지 못한다.
- **커밋 없이 끝난 Implementer의 까닭 댓글**(#414): run이 커밋 없이 끝났는데 run 시작 뒤 티켓에 표지가 아닌 댓글이 있으면 그것을 까닭으로 보고 실패로 세지 않고 곧바로 사람에게 넘긴다(`record-failure.sh`의 넷째 인자). 댓글이 없으면 지금처럼 센다.
- **main이 들어가지 않은 브랜치는 넘기지 않는다**(#434): 충돌 모드 run의 에이전트가 충돌을 풀고도 merge 커밋 없이 끝나 PR 머리가 그대로인 채 Verify로 넘어가 같은 충돌로 한도까지 되돌려졌다(#342). 그래서 merge 커밋은 장치가 한다: 해결 단계 뒤 「Finish merge」(`merge-check.sh finish`)가 풀리지 않은 경로와 merge가 더한 줄의 충돌 표지가 없을 때만 `git add -A`·`git commit --no-edit`을 하고 아니면 실패이며, 해결 에이전트의 마지막 글과 Bash 명령(비밀 꼴을 지움)은 run 요약에 남는다(`agent-summary.sh`, 전체 기록은 공개 저장소라 올리지 않음). 이어 Implementer는 「Require commits」 뒤 `merge-check.sh check`로 브랜치가 rebase 단계가 가져온 main을 포함하고 미완 merge·rebase·풀리지 않은 파일이 없는지 보고(일반 모드도 같다), 아니면 push·Verify 없이 코드 실패로 세며 까닭(「충돌을 풀지 않고 끝남: …」)을 실패 댓글에 적는다(`record-failure.sh`의 `FAILURE_REASON`).
- **해법이 갈리면 묻는다.** Decompose·Odin은 사용자에게 보이는 선택이 여럿이거나 앞 spec과 부딪히면 고르지 않고 spec에 `<!-- factory-question -->` 댓글(선택지와 권장안)과 `needs-decision`을 남긴다. 그 spec만 멈춘다. 사람이 답 댓글을 달고 라벨을 떼면 재개 workflow(`factory-resume.yml`)가 멈춘 단계부터 잇는다. 답 댓글은 다음 run에 구속력이 있다.
- **병합은 구현한 run이 아닌 Verify가 한다.** Verify(Sonnet)가 기계 검사와 인수 판단을 하고 병합한다. 문제면 사유를 남기고 되돌리고, 같은 종류의 되돌림이 한도에 닿으면 사람에게 넘긴다.
- **되돌림 종류와 한도**(2026-11-18, #307): 트랙이 여럿이 되자(#286) 옆 트랙이 먼저 병합해 생긴 main과의 충돌이 되돌림의 큰 몫이 됐고, 충돌 때마다 PR을 닫고 다시 구현해 비용이 들고, 충돌 두 번이면 사람에게 넘어갔다. 그래서 되돌림 댓글에 종류를 적는다: `kind=acceptance`(기계 검사·인수 판단 실패, 병합 실패)와 `kind=conflict`(Verify가 main을 merge하면 충돌 `conflict`, merge 뒤 `cargo xtask ci` 실패 `rebase-ci`, merge한 브랜치 push 실패 `push`). 종류가 없는 #307 전 댓글은 인수 실패로 센다. 한도는 따로 세어 인수 실패 2번, 충돌 3번(`verify.sh`의 `ACCEPTANCE_LIMIT`·`CONFLICT_LIMIT`)이고, 사람 넘김 댓글에 어느 한도에 닿았는지 적는다. 인수 실패는 지금처럼 PR을 닫고 브랜치를 지워 main부터 다시 구현한다. 충돌은 PR과 브랜치를 남기고 PR 본문의 `factory-checked` 표지를 `factory-stale-checked`로 바꿔(`next-verify`가 풀기 전의 PR을 집지 않게) 티켓을 `ready-for-agent`로 돌린다. 다음 Implementer run은 `verify.sh conflict-pr N`(마지막 되돌림이 충돌이고 그 PR이 열려 있으면 PR 번호)을 보고 **충돌 모드**(`mode` 입력, 기본 `auto`)로 돈다: 그 PR 브랜치를 받아 main을 merge하고, 충돌이 있으면 구현 단계의 「Resolve conflicts」와 같은 프롬프트·도구로 에이전트가 풀고, 같은 자동 검사(`cargo xtask ci`) 뒤 push하고 같은 PR 본문에 새 `factory-checked`를 붙여 Verify에 넘긴다. 구현 단계와 그 비용 기록은 건너뛰고, `impl-start`에 `mode=conflict`를 적는다.
  - **어디서 푸는가**: Verify 안에서 풀지 않고 Implementer의 충돌 모드로 푼다. Verify는 직렬(`factory-verify` 한 group)이라, 그 안에서 에이전트가 충돌을 풀고 `cargo xtask ci`(수 분)를 돌리면 다른 트랙의 PR이 모두 그만큼 기다린다. Implementer run은 트랙 배정·`dispatch-next.sh`의 `(track T)` 제목 세기·`factory-ticket-<N>` group을 그대로 쓰므로 트랙 규칙을 바꾸지 않는다. Verify는 rebase 대신 merge로 main을 넣어, 충돌 모드가 만든 merge 커밋이 다음 Verify에서 다시 충돌하지 않게 한다(PR은 squash 병합이라 merge 커밋이 main에 남지 않는다). 공장이 `GITHUB_TOKEN`으로 연 PR의 `ci.yml` pull_request run은 승인 대기(job 0개)로 생겨 병합·닫힘 뒤 실패로 남으므로, Odin·Verify가 PR을 병합하거나 닫은 뒤 `prune-runs.sh`로 지우고 main에서 `ci.yml`을 dispatch한다(#270).
  - **Odin의 충돌**(2026-11-18, #359): 스펙 PR도 검토·검사 사이에 main이 움직여 충돌한다(#25가 S9 병합과 `command.rs`·`window.rs`·`text.rs`·`tests/command.rs`에서 충돌해 검사를 통과하고도 사람에게 넘어갔다). 스펙 PR은 무시한 인수 시험과 `todo!` 스텁뿐이라 충돌은 대개 같은 enum·match·목록에 양쪽이 더한 것이고 둘 다 넣으면 풀린다. 그래서 Odin은 main 위로 rebase하지 않고 main을 merge하며(Verify와 같은 까닭: 푼 merge 커밋이 다음에 다시 충돌하지 않고 push가 force 없는 일반 push다), 충돌이 나면 같은 run의 「Resolve conflicts」 에이전트(Implementer와 같은 「양쪽을 다 남긴다」 지시)가 풀고, 검사를 fmt·clippy 대신 `cargo xtask ci` 전체로 돌린 뒤 일반 push로 올린다. 검사를 통과했는데 squash 병합이 실패하면(그 사이 main이 또 움직임) 지금 충돌하는 파일을 찾아 `factory-odin.yml`을 `mode=conflict`로 다시 dispatch한다: 그 run은 검토를 건너뛰고(판정 `pass`) main을 merge해 풀고 검사·push·병합만 한다. **어디서 푸는가**: Implementer의 충돌 모드를 쓰지 않고 Odin 안에서 푼다. Implementer 충돌 모드는 티켓·트랙(`(track T)` 제목, `verify.sh conflict-pr`)에 묶여 있는데 스펙 PR은 티켓이 아니고, Odin의 concurrency group은 spec마다라 Odin run이 길어져도(작업 한도 180분) 다른 spec·트랙을 막지 않는다. **한도**: 충돌 풀기 차례를 스펙 PR의 `<!-- factory-odin-conflict -->` 댓글(공장 계정이 단 것만, 충돌 파일과 「풀기 n/2」)로 세고(`odin.sh conflict`, `ODIN_CONFLICT_LIMIT` 기본 2), 마지막 사람 넘김(`<!-- factory-odin-human -->`) 뒤의 것만 센다. 한도를 넘거나 에이전트가 충돌을 다 풀지 못하거나 풀고도 검사가 두 번 실패하면 충돌 파일을 적어 사람에게 넘긴다.
- **비용 장부와 상한**(`.github/factory/ledger.sh`): Claude 단계마다 끝날 때 `factory-ledger` 라벨의 장부 이슈에 `<!-- ledger -->` 댓글 하나(spec·단계·티켓·run·API 정가 비용·턴·토큰). 단계를 시작하기 전 가드가 spec 합계(상한은 spec 본문의 `예산: $N` 줄, 없으면 저장소 변수 `FACTORY_SPEC_BUDGET_USD`, 없으면 $60)와 KST 하루 합계(`FACTORY_DAILY_BUDGET_USD`, 없으면 $150)를 본다. spec 상한이면 그 spec에 `factory-paused`와 `<!-- factory-budget-spec -->`, 하루 상한이면 장부에 `<!-- factory-budget-daily <날짜> -->`를 한 번 남기고 돌지 않는다. 하루 상한 뒤에는 Implementer의 예약 실행(KST 00:05)이 사슬을 다시 시작한다. 단계 하나의 중간에는 막지 않는다.
- **트랙 측정**(#286, `ledger.sh event`·`stats`): 장부에 비용과 따로 `<!-- factory-event -->` 댓글(`event=<종류> issue=#N … [wait=초] run=<id>`)을 남긴다. 합계는 이 댓글을 세지 않는다. `impl-start`(트랙, `ready-for-agent`가 붙은 때부터 Implementer 시작까지 기다림), `impl-end`(트랙), `verify-start`(그 티켓의 마지막 `impl-end`부터 Verify 시작까지 기다림), `verify-end`(결과 `merged`·`mech`·`conflict`·`rebase-ci`·`push`·`review`·`merge-failed`·`cancelled`·`stopped`). `ledger.sh stats`가 충돌 되돌림 비율(`conflict`·`rebase-ci`·`push` / Verify 수), 트랙별 Implementer 기다림, Verify 기다림(수·평균·최대), 파일별 충돌 수(충돌 되돌림의 `verify-end`에 적힌 `files=a,b`를 세어 많은 것부터)를 낸다. 기록이 실패해도 run은 실패하지 않는다.
- **알림 다리**(`.github/factory/bridge.sh`, 2026-11-19 #410에서 라벨 기준으로 바꿈): 공장은 switchboard 메일함을 부르지 않는다. 공장 밖 서버의 크론(2분)이 `gh`로 세 가지를 보고 새 것만 보낸다. (1) **라벨**: 열린 이슈의 `needs-decision`(질문)·`ready-for-human`(사람 넘김, `hand-to-human.sh`만 붙인다)·`factory-paused`(spec 상한 멈춤)·`human-check`(본문에 체크하지 않은 `- [ ]`가 남아 있을 때만)를 (이슈, 라벨, 타임라인의 마지막 `labeled` 시각, 받는 곳)마다 한 번 보낸다. 라벨을 뗐다 다시 붙이면 새 알림이다. 메일의 까닭은 그 이슈에서 믿을 수 있는 작성자의 가장 최근 `<!-- factory-… -->` 표식 댓글을 인용하고, 없으면 「까닭 댓글 없음」으로 그래도 보낸다. `needs-triage`·`needs-info`·`local`은 보내지 않는다. (2) **라벨 없는 표식 댓글**: 하루 상한(`factory-budget-daily`)·spec 큐 바닥남(`factory-queue-empty`)·idle(`factory-idle`)·공장 막힘(`factory-stalled`, 기다리는 티켓이 사람 몫 이슈에 막힘)·벤치 한도 초과(`factory-bench-regression <sha>`, `bench.yml`이 `bench-alert` 이슈에 단다, #366)를 댓글마다 한 번 보낸다. 공개 저장소라 `github-actions[bot]`과 쓰기 권한자(admin·maintain·write)가 단 표식만 보내고(까닭 인용도 같은 규칙), 그 밖의 작성자 댓글은 보내지 않고 기억해 다시 보지 않는다. (3) **idle 직접 판정**: `FACTORY_SPEC_QUEUE`의 spec이 모두 닫혔고 `ready-for-agent`·`in-progress` 열린 이슈가 없으면 큐 값마다 한 번 보낸다(같은 큐의 `factory-idle` 표식과 키를 나눠 한 통만 간다). 받는 곳: `needs-decision`은 대장(`BRIDGE_TO_BOSS`)에게만, 나머지는 조율자(`BRIDGE_TO_COORD`)와 참조(`BRIDGE_CC`, 쉼표로 여럿)에게. 받는 곳은 기본값이 없어 둘 중 하나라도 없으면 까닭을 출력하고 보내지 않고 끝난다(코드에 받는 곳 이름을 두지 않는다). 옛 `BRIDGE_TO`는 경고만 하고 무시한다. 상태 파일은 받는 곳마다 한 줄이고(조율자에게 보낸 표식 댓글은 그 전처럼 댓글 ID만, 참조는 `<ID> <받는 곳>`, 라벨은 `label <n> <라벨> <시각> <받는 곳>`, idle은 `idle <큐> <받는 곳>`) 보내기에 성공한 뒤에만 적으며 멱등 키도 받는 곳마다다. 스크립트에는 주소·토큰·호스트가 없고 메일 서버는 switchboard 프로필이 안다. 설치·업그레이드 때 `--seed`로 지금 라벨 상태·표식·idle을 보내지 않고 기억만 한다. 같은 스크립트를 다른 저장소의 공장에도 쓴다(#380): 저장소마다 크론 한 줄을 두고 `FACTORY_REPO`(owner/name)와 받는 곳을 준다. 이름 부분 `<name>`으로 메일의 task `<name>-factory`, 멱등 키 접두어 `<name>-factory-`, 제목 접두어 `[<name> 공장]`, 상태 폴더 `$XDG_STATE_HOME/<name>-factory-bridge`(`BRIDGE_STATE_DIR`로 바꿈)를 정한다. 사람이 이슈를 공장에 되돌릴 때는 `ready-for-human`을 떼고 `ready-for-agent`를 붙이며 `<!-- factory-reset -->`으로 시작하는 댓글을 남긴다: 그 앞의 실패·되돌림은 더 세지 않는다.
- **티켓 후보에서 빼는 것**(2026-11-18, #389): `dispatch-next.sh`는 `spec` 라벨이 있거나 하위 이슈(sub-issue)가 있는 이슈(부모는 티켓이 아니다)와 `local` 라벨 이슈(아래)를 집지 않는다.
- **장부는 스스로 만든다**(#389): `ledger.sh record`는 열린 `factory-ledger` 이슈가 없으면 「공장 비용 장부」 이슈를 만들어 기록한다(비용을 버리지 않는다).
- **spec 귀속**(#389): 티켓의 부모는 GitHub sub-issue의 parent(`GET /issues/{n}/parent`)를 먼저 보고, 없을 때만 본문 `## Parent` 절을 본다(`ledger.sh parent-of`·`spec-of`). 손으로 만든 티켓도 sub-issue로 묶으면 그 spec에 비용·상한이 걸린다.
- **`ready-for-agent`는 부모가 있어야 한다**(#389): 이슈 템플릿 `.github/ISSUE_TEMPLATE/ticket.md`가 `## Parent` 절을 깔고, `parent-check.sh`(Implementer의 `approved` job과 claim 단계)가 부모 없는 티켓의 라벨을 떼고 까닭 댓글(`<!-- factory-no-parent -->`)을 단다. 쓰기 권한(admin·maintain·write)이 없는 사람이 붙인 라벨은 보지도 답하지도 않는다(공개 저장소). `spec` 라벨 이슈는 티켓이 아니라 검사하지 않는다.
- **workflow 티켓은 쪼갤 때부터 공장 밖으로**(#389): 공장에는 workflow 쓰기 토큰을 주지 않는다(공개 저장소). Decompose는 `.github/workflows/`를 바꿔야 하는 일을 따로 티켓으로 떼어 `ready-for-human` + `local` 라벨과 「workflow 파일 — 로컬 작업자 몫」 한 줄을 단다(`decompose.md`). `local-ticket.sh`가 Decompose 직후(`decompose.sh finish`)·Odin 승인 전(`odin.sh`의 티켓 승인)·Implementer의 `approved` job과 claim에서 다시 보아, `local` 라벨이 있거나 제목·본문이 `.github/workflows`를 언급하는 티켓에서 `ready-for-agent`를 떼고 `ready-for-human`·`local`을 붙이고 `<!-- factory-local -->` 댓글을 한 번 단다(라벨이 없으면 붙일 때 생긴다). 로컬 작업자(관리 세션)가 처리한다.
- **workflow 파일은 사람 몫**(#389): `GITHUB_TOKEN`은 `.github/workflows/*`를 push할 수 없는데, 그 거절을 코드 실패로 세어 다시 돌리면 비용만 버린다(이식한 공장에서 실제로 일어났다). `implementer.md`는 workflow 파일을 고치지 말고 티켓에 댓글로 남기고 멈추라 하고, 커밋 뒤 diff(`origin/main...HEAD`)에 `.github/workflows/`가 있으면 자동 검사 전에, push가 그 거절을 내면 그 자리에서 `workflow-guard.sh`가 실패 횟수를 늘리지 않고 `in-progress`를 `ready-for-human`으로 바꾸고 `<!-- factory-human -->`(사유: workflow 파일) 댓글을 단다. 다시 dispatch하지 않는다(쪼갤 때 빠져나온 경우의 안전망). 단, push 거절이 run 도중 main에 들어온 workflow 변경 탓이면(#417) 넘기기 전에 `workflow-guard.sh push-retry`가 `git fetch origin main` 뒤 브랜치 자기 커밋(`origin/main...HEAD`)이 `.github/workflows/`를 건드리지 않을 때만 main을 따라잡아(구현은 rebase, 충돌 모드는 merge — 강제 push가 아니라서) 한 번 더 push하고, 충돌하거나 다시 거절되면 「main을 따라잡은 뒤에도 거절」로 넘긴다. PR의 `factory-checked`는 검사한 커밋 그대로라 Verify가 따라잡은 head에서 `cargo xtask ci`를 다시 돈다.
- **사람 넘김 사유 코드**(2026-11-19, #437): `hand-to-human.sh <이슈> <코드> <까닭>`의 코드는 `conflict`·`acceptance-unpassable`·`push-rejected`·`attempts-exhausted`·`agent-blocked`·`workflow-change`·`local-only`·`spec-merge-failed`·`other` 가운데 하나(모르는 코드는 종료 2)이고 표지 `<!-- factory-human <코드> -->`로 남아 장부(`ledger.sh stats`의 코드별 넘김 수)와 알림 다리 메일 제목·첫 줄에 나온다(코드 없는 옛 표지는 `other`). 충돌·인수·push 거절은 공장이 먼저 고쳐 보고(merge-check·acceptance-probe·workflow-guard push-retry) 안 될 때만 넘긴다. 넘김 메일은 조율자에게 원인 확인 → 고침 → 재발 방지 장치 → 결과 보고의 처리 지시이고 참조(CC)는 한 줄 요약이다.
- **공장 장치 한 벌**(2026-11-19, #437): `.github/factory/kit.txt`에 적은 파일은 이 장치를 복사해 쓰는 저장소에서도 정본과 바이트까지 같아야 하고(`kit-check.sh <정본 디렉터리>`로 확인), 저장소마다 다른 값은 파일을 고치지 않고 환경 변수(`FACTORY_REPO`(없으면 git origin), `REPO`, `BRIDGE_TO_*` 등)로 준다.
- 공장 스크립트는 가짜 `gh`·`switchboard`로 시험하고(`.github/factory/test.sh`), `cargo xtask ci`의 「공장 스크립트 시험」 단계가 돌린다. workflow YAML의 연결은 자동 시험하지 않고 시범으로 확인한다.
