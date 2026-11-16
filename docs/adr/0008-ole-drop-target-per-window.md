# 0008. 다른 앱에서 끌어 온 파일은 창마다 facet이 등록한 OLE 드롭 대상이 받는다

- 상태: 채택
- 날짜: 2026-11-16

## 맥락

spec #20(S5)은 탐색기·다른 앱과 끌어 놓기를 양방향으로 하고, 「Slint의 창 이벤트와 OLE 드래그 앤 드롭을 붙이는 방법을
첫 티켓에서 확인한다」고 했다(#184). 확인할 것은 셋이었다.

1. Slint(winit) 창 핸들에 facet이 드롭 대상을 직접 등록할 수 있는가.
2. Slint·winit의 기본 파일 드롭과 부딪히지 않는가(winit 쪽을 끄는 방법).
3. 바깥에서 끄는 동안 facet 창이 다시 그려지는가.

알아낸 것(Slint 1.18.1, winit 0.30.13, Windows):

- **winit은 기본으로 창마다 자기 `IDropTarget`을 등록한다.** 창을 만들 때 `platform_specific.drag_and_drop`이 참이면
  UI 스레드에서 `OleInitialize`를 부르고 `RegisterDragDrop`으로 자기 처리기를 건다. 그 처리기는 `CF_HDROP`만 보고
  `DroppedFile`·`HoveredFile` 이벤트를 낼 뿐이라, 원본 앱이 허용한 효과·Ctrl·Shift·오른쪽 버튼·데이터 종류를 알 수 없고
  커서 효과도 고를 수 없다. 창 하나에는 드롭 대상이 하나만 등록되므로(`DRAGDROP_E_ALREADYREGISTERED`) 그대로 두면
  facet이 등록하지 못한다.
- **winit 쪽은 창 속성으로 끈다.** winit의 `WindowAttributesExtWindows::with_drag_and_drop(false)`를 Slint의
  `BackendSelector::with_winit_window_attributes_hook`(feature `unstable-winit-030`, 이미 측정 모드가 쓰는 것)으로 모든
  창에 건다. 끄면 winit은 `OleInitialize`도 부르지 않는다. `SLINT_BACKEND`로 고른 렌더러는 그대로 따른다.
- **winit 창은 `show` 때가 아니라 이벤트 루프가 돈 뒤에 생긴다.** `show` 직후에는 창 핸들이 없다. 그래서 창의 winit
  이벤트 훅에 첫 이벤트가 오면 이벤트 루프로 돌아간 뒤 그 창 핸들에 등록한다.
- 실제로 띄운 facet(디버그 빌드, 창 둘: 첫 실행과 단일 실행으로 넘어온 두 번째 창)에서 창마다
  `OleDropTargetInterface` 창 속성(OLE가 `RegisterDragDrop` 때 거는 것)이 생기고 등록 실패 출력이 없음을 확인했다.
  winit 처리기가 남아 있었다면 facet의 등록이 `AlreadyRegistered`로 실패했을 것이다.
- **다시 그리기**: 바깥 앱이 끄는 동안 OLE 드래그 루프(`DoDragDrop`)는 **원본 앱의** 스레드에서 돈다. facet의 UI
  스레드는 막히지 않고 자기 메시지 루프(winit)를 계속 돌며, OLE는 드롭 대상 호출(`DragEnter`·`DragOver`·`DragLeave`·
  `Drop`)을 그 메시지 루프 안에서 부른다. 처리기가 Slint 속성(놓일 폴더 행)을 바꾸면 Slint가 winit에 다시 그리기를
  청하고, 같은 메시지 루프가 `WM_PAINT`를 처리해 그린다. 그래서 끄는 동안에도 강조가 움직인다. 이것은 화면이라
  자동 테스트가 보지 못하고 사람 확인 목록(`docs/dragdrop-checklist.md`)에 두었다. facet에서 **바깥으로** 끄는 경우는
  facet의 UI 스레드가 `DoDragDrop`을 돌므로 사정이 다르다(다음 티켓이 확인한다: OLE 모달 루프도 메시지를 펌프하므로
  그리기 자체는 돌 것으로 보지만, winit 이벤트 처리 안에서 다시 들어오는 문제가 있다).

## 결정

- **facet이 창마다 OLE 드롭 대상을 직접 등록한다.** winit의 파일 드롭은 모든 창에서 끈다. 등록·해제와 데이터 개체
  읽기는 안전 래퍼 `facet_win::drop_target`에 두고(`unsafe`는 `windows_drop_target.rs`), 등록은 창을 만든 UI 스레드에서
  한다(OLE가 처리기를 그 스레드에서 부르므로 처리기는 `Send`가 아니어도 된다). 화면 상태(`View`)가 등록을 갖고, 버리면
  해제한다.
- **처리기는 판단만 하고 곧바로 돌아간다.** 끄는 동안은 놓을 자리(앱의 순수 함수 `file_drop::place_at`)와 코어의
  `dragdrop::decide`로 커서 효과를 정하고 놓일 폴더 행을 강조한다. 놓으면 파일 목록(이미 복사한 경로)을 작업 대기열에
  넣고(`App::start_drop`) 돌아간다. 파일 시스템 작업은 작업 대기열이 한다. 원본 앱은 그동안만 기다린다.
- **이동 드롭은 셸의 최적화된 이동 규약을 따른다.** facet이 이동을 직접 하므로 놓기에서 데이터 개체에
  `Performed DropEffect`를 `DROPEFFECT_NONE`, `Logical Performed DropEffect`를 `DROPEFFECT_MOVE`로 적고 원본 앱에는
  `DROPEFFECT_NONE`을 돌려준다. 원본 앱은 원본을 지우지 않는다(두 번 지우지 않는다). 복사는 `DROPEFFECT_COPY`를 그대로
  돌려준다.
- **파일이 아닌 데이터는 「파일 아님」이다.** `CF_HDROP`이 없거나(글자·그림·`FileGroupDescriptor` 가상 파일), 읽을 수
  없는 목록(ANSI·깨진 바이트)이나 빈 목록이면 코어 결정이 놓을 수 없음이 되어 금지 커서가 보인다.
- **오른쪽 버튼으로 끌어 놓기는 이 티켓에서는 놓을 수 없음이다.** 코어 결정은 고르기 메뉴(`Decision::Menu`)를 내지만
  메뉴는 다음 티켓이 만든다. 기본 동작(왼쪽 버튼처럼 복사·이동)으로 몰래 하지 않고 금지 커서로 둔다: 사용자가 메뉴를
  기대하고 놓았는데 바로 옮겨지면 되돌리기 전까지 무엇이 일어났는지 모른다.
- **놓을 자리**: 목록의 폴더 행 위면 그 폴더, 패널 안의 그 밖의 곳(목록 빈 곳·머리글·주소창·탭 줄)이면 그 패널 활성
  탭의 위치, 파일 행 위와 패널 밖(작업 영역·상태 표시줄·패널 사이)은 놓을 수 없음. 탭 위·주소창 조각 위로 보내는 것은
  뒤 티켓이다.
- 셸의 끌기 그림 도우미(`IDropTargetHelper`, `CLSID_DragDropHelper`)가 있으면 원본 앱이 만든 끌기 그림을 facet 창 위에도
  그린다. Windows에 든 구현이고 셸 확장을 싣지 않으므로 ADR-0006의 「셸 확장은 도우미 프로세스에서」에 걸리지 않는다.
  만들지 못하면 그림만 안 보인다.

## 결과

- 끌기의 모든 정보(허용 효과·키·데이터 종류)를 facet이 직접 보므로 결과 결정을 코어의 순수 함수 하나로 둔다.
- 등록은 Slint의 불안정 API(`unstable-winit-030`)에 기댄다. Slint 부 버전을 올릴 때 이 훅과 winit 창 속성을 다시 본다
  (`crates/facet-app/Cargo.toml`이 1.18.x로 묶는다).
- facet에서 바깥으로 끌기(다음 티켓)는 이 드롭 대상과 같은 OLE 경로를 쓰므로 facet 안의 끌기도 이 처리기로 들어온다.
