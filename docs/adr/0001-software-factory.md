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
| Implementer | `ready-for-agent` 라벨 | 티켓을 구현하고, 자동 검사를 통과시킨 뒤 draft PR을 연다 | 없음. 의존 티켓 연쇄는 PR **병합**이 트리거한다 | PR 리뷰·병합 | concurrency group `issue-<번호>`. 시작 시 `ready-for-agent`를 떼고 `in-progress`를 붙인다 |
| Decompose | `spec` 라벨 | spec을 티켓으로 나누고 blocked-by(native dependency)를 기록한다 | 시작 가능한 티켓에 `ready-for-agent`를 붙인다 | 연쇄 전에 사람이 티켓을 확인 | spec 번호로 concurrency group. 하위 이슈가 이미 있으면 종료한다 |
| Explore | 이슈 생성 (`needs-triage`) | 코드베이스와 대조해 분류한다 | 쉬운 건은 `ready-for-agent`, 나머지는 `ready-for-human` / `needs-info` / `wontfix` | 라벨 변경 | 이슈 번호로 concurrency group. `needs-triage`가 있을 때만 실행한다 |

같은 티켓에서 실패한 run이 2번 쌓이면 더 돌리지 않고 `ready-for-human`을 붙여 사람에게 넘긴다.

### 켜는 순서

브레이크(자동 검사·사람 리뷰)가 신뢰를 얻는 만큼만 트리거를 켠다.

0. **전제 조건**: 스택을 정하고, 빌드·lint·test CI workflow를 만든다. 자동 검사가 없으면 Implementer를 켜지 않는다.
1. **Implementer**: 사람이 직접 `ready-for-agent`를 붙일 때만 돈다. PR마다 사람이 리뷰한다.
2. **Decompose**: 로컬 `to-tickets` 스킬과 하는 일이 겹친다. 원격으로도 필요할 때 켠다.
3. **Explore**: 외부 사용자가 이슈를 올리기 시작하면 켠다. 처음에는 일부 이슈에만 적용한다.

PR이 쌓이면 Automated review task(그리고 그 짝인 Rework task)를 넣어 브레이크를 추가한다.

## 결과

- 슬롯 하나를 제품 하나(GitHub)가 대부분 채우므로, 관리할 서버가 없다.
- Claude Code cloud 같은 번들 플랫폼은 Linux라 Windows 빌드·테스트를 못 하므로 쓰지 않았다.
- 자체 호스트 Windows 러너는 공개 저장소에서 외부 PR이 내 기계에서 돌 위험이 있어 쓰지 않았다. 빌드가 너무 느려지면 다시 검토한다.
- 구독 OAuth 토큰을 쓰므로 공장 run이 구독 사용량을 함께 쓴다.
