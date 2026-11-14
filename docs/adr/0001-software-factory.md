# 0001. 소프트웨어 공장: 모든 서비스를 GitHub에 둔다

- 상태: 채택 (일부 task는 아직 꺼져 있음)
- 날짜: 2026-11-15

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
| Runner | Actions 호스티드 러너. Implementer는 `windows-latest`, 나머지는 `ubuntu-latest` |
| Observability platform | 없음. Actions run 페이지로 충분하다 (의도한 선택) |

- Implementer run은 Windows에서 빌드·테스트해야 하므로 `windows-latest`를 쓴다. 공개 저장소라 Actions 분(minute)은 무료이고, 호스티드 job 하나는 최대 6시간이다.
- 에이전트는 `anthropics/claude-code-action`으로 돌린다. 인증은 구독 OAuth 토큰(`claude setup-token` → secret `CLAUDE_CODE_OAUTH_TOKEN`)이다.

### Task

| Task | Trigger | 하는 일 | 다음 run | 사람이 다시 들어오는 곳 | 중복 방지 |
| --- | --- | --- | --- | --- | --- |
| Implementer | `ready-for-agent` 라벨(사람이 붙임), 수동 실행, 또는 티켓이 닫힐 때 `frontier` job의 dispatch | 열린 blocker가 없을 때만 시작한다. 에이전트는 수정·커밋만 한다. workflow가 `cargo xtask ci`를 다시 돌리고, 통과하면 push·PR을 열고 **바로 squash 병합한다**. 지시는 `.github/factory/implementer.md` | 막힘이 풀린 승인 티켓을 dispatch한다. GITHUB_TOKEN으로 한 병합은 `issues: closed`를 트리거하지 않으므로 run이 직접 한다. 사람이 병합해 티켓이 닫힐 때는 `frontier` job이 같은 일을 한다 | `ready-for-agent`를 붙이는 것이 승인이다. 병합 전 리뷰는 없다. 손으로 확인할 것은 run이 spec 이슈에 댓글로 남긴다 | concurrency group `factory-implementer-<번호>`. 시작 시 `ready-for-agent`를 떼고 `in-progress`를 붙인다. 막힌 티켓은 라벨을 그대로 둔다 |
| Decompose | `spec` 라벨, 또는 수동 실행(`workflow_dispatch`, spec 번호) | spec을 하위 이슈(티켓)로 나누고 blocked-by(native dependency)를 기록한다. 지시는 `.github/factory/decompose.md` | 없음. 티켓에 `ready-for-agent`를 붙이지 않는다 | 사람이 분해 결과를 검토하고 티켓에 직접 `ready-for-agent`를 붙인다 | spec 번호로 concurrency group. 하위 이슈가 이미 있으면 에이전트를 부르기 전에 종료한다 |
| Explore | 이슈 생성 (`needs-triage`) | 코드베이스와 대조해 분류한다 | 쉬운 건은 `ready-for-agent`, 나머지는 `ready-for-human` / `needs-info` / `wontfix` | 라벨 변경 | 이슈 번호로 concurrency group. `needs-triage`가 있을 때만 실행한다 |

같은 티켓에서 실패한 run이 2번 쌓이면 더 돌리지 않고 `ready-for-human`을 붙여 사람에게 넘긴다.

### 켜는 순서

브레이크(자동 검사·사람 리뷰)가 신뢰를 얻는 만큼만 트리거를 켠다.

1. **Decompose (2026-11-15 켬)**: 코드를 빌드하지 않으므로 CI 없이 켠다. 티켓에 `ready-for-agent`를 붙이지 않으므로 다른 run을 트리거하지 않는다. 첫 대상은 S0(#1)이다. S0 티켓은 공장 밖(로컬 implement)에서 구현하므로 라벨 없이 둔다.
2. **Implementer**: 전제 조건은 자동 검사다. 빌드·lint·test CI workflow를 만든다(S0, #1). 자동 검사 전체는 명령 하나 `cargo xtask ci`로 돈다. `.github/workflows/ci.yml`도 Implementer run도 이 명령을 쓴다. 자동 검사가 없으면 Implementer를 켜지 않는다. 2026-11-15에 켰다. 대표 결정으로 S0의 #3 #5 #6 #7 #8을 맡기고, **병합 전 사람 리뷰를 뺐다**. 브레이크는 `cargo xtask ci` 하나뿐이다. 그래서 같은 실수가 반복되면 사람 리뷰를 다시 넣기보다 검사를 더 엄격하게 만들거나 Automated review task를 넣는다.
3. **Explore**: 외부 사용자가 이슈를 올리기 시작하면 켠다. 처음에는 일부 이슈에만 적용한다.

PR이 쌓이면 Automated review task(그리고 그 짝인 Rework task)를 넣어 브레이크를 추가한다.

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
