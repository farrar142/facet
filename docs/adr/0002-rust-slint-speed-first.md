# 0002. 속도 최우선, Rust 코어 + Slint UI

- 상태: 채택 (2026-11-15 검증 완료, 아래 「검증 결과」)
- 날짜: 2026-11-15

## 맥락

Files, Directory Opus, Total Commander, File Pilot 같은 대체 탐색기가 이미 있다. facet이 선택받을 이유와, 기능이 서로 충돌할 때 이기는 기준이 필요했다.

## 결정

- **속도가 최우선이다.** 콜드 스타트 200ms 미만, 10만 항목 폴더의 끊김 없는 스크롤, 즉시 검색을 목표로 한다. 기능이 속도를 해치면 기능을 미루거나 뺀다.
- **코어는 Rust**로 쓰고, 셸 COM 연동은 Microsoft `windows` crate로 한다.
- **UI는 Slint**로 그린다.

## 검토한 대안

- **C#/WinUI**: COM은 편하지만 콜드 스타트가 느리다.
- **Tauri(WebView2)**: WebView2 기동 비용과 IPC 때문에 속도 목표와 충돌한다.
- **gpui**: 대량 목록 성능은 증명됐지만 1.0 전이라 API가 자주 깨지고 문서가 빈약하다. 구현을 에이전트가 하므로(ADR-0001) 위험이 크다.
- **egui**: 단순하지만 텍스트 품질·IME·접근성이 약하다.

## 결과

- 첫 티켓은 스파이크다. 10만 행 스크롤, 한글 IME, 콜드 스타트를 재고, 기준에 못 미치면 이 ADR을 다시 연다.
- 가상화된 썸네일 그리드는 직접 만들어야 한다.
- 셸 호출은 STA 스레드와 메시지 루프가 필요하고 전부 `unsafe`다. 안전 래퍼 모듈 하나에 가둔다.
  - 고침(2026-11-15): `unsafe_code`를 허용하는 크레이트는 **두 개**다. 앱 프로세스가 부르는 Windows API 안전 래퍼 크레이트(S1)와, 따로 도는 인덱서 서비스 실행 파일 크레이트(S7, ADR-0003)다. 둘은 다른 프로세스에서 돌고, 하나로 합치면 앱이 서비스의 MFT·권한 코드를 끌어오게 된다. 그 밖의 크레이트는 계속 `unsafe_code`를 금지한다. 셋째가 필요하면 이 ADR을 다시 고친다.
- Slint는 로열티 프리 라이선스로 쓰고, facet 코드는 MIT로 둔다. 조건과 표시 의무는 아래 「라이선스 확인」 절에 적는다.

## 라이선스 확인

- 확인 날짜: 2026-11-15
- 확인한 Slint 버전: 1.18.1 (2026-09-21 배포, 확인 시점의 최신 안정 버전, 태그 `v1.18.1` = 커밋 `372cf0e`). 같은 날 `master`의 `LICENSE.md`와 로열티 프리 본문도 이 태그와 글자 하나 다르지 않았다.
- 근거는 Slint 저장소의 라이선스 파일과 slint.dev 가격 페이지뿐이다. 아래 링크는 모두 커밋 `372cf0e`에 고정했다.
- 법률 자문이 아니다. 개발자가 본문을 읽고 정리한 것이다.

### 라이선스 이름과 위치

- Slint 프레임워크는 세 라이선스 중 하나를 고르는 방식이다. GPLv3, 로열티 프리, 상용. ([LICENSE.md L17-L22](https://github.com/slint-ui/slint/blob/372cf0ee5577c3dfec309a45e7b778ba4e81b734/LICENSE.md#L17-L22))
- facet이 쓰는 것은 **Slint Royalty-free Desktop, Mobile, and Web Applications License, Version 2.0**이다. SPDX 식별자는 `LicenseRef-Slint-Royalty-free-2.0`. 본문: [LICENSES/LicenseRef-Slint-Royalty-free-2.0.md](https://github.com/slint-ui/slint/blob/372cf0ee5577c3dfec309a45e7b778ba4e81b734/LICENSES/LicenseRef-Slint-Royalty-free-2.0.md)
- 저작권자는 SixtyFPS GmbH다. ([본문 L7](https://github.com/slint-ui/slint/blob/372cf0ee5577c3dfec309a45e7b778ba4e81b734/LICENSES/LicenseRef-Slint-Royalty-free-2.0.md#L7))
- 무료다. 허락 조항이 "a world-wide, royalty-free, non-exclusive license to use, reproduce, make available, modify, display, perform, distribute the Software as part of a Desktop, Mobile, or Web Application"이다. 데스크톱·모바일·웹 앱의 일부로 쓰고 고치고 배포할 권리를 돈 없이 준다. ([본문 L11](https://github.com/slint-ui/slint/blob/372cf0ee5577c3dfec309a45e7b778ba4e81b734/LICENSES/LicenseRef-Slint-Royalty-free-2.0.md#L11))

### 적용 대상

- 데스크톱 앱은 허용된다. 정의는 "a computer program that is designed to run on a general-purpose computer (PC or notebook), typically installed and executed locally on the computer's operating system"이다. 범용 PC에 설치해 돌리는 프로그램이다. facet(Windows PC에 설치하는 탐색기)은 여기에 든다. ([본문 L13](https://github.com/slint-ui/slint/blob/372cf0ee5577c3dfec309a45e7b778ba4e81b734/LICENSES/LicenseRef-Slint-Royalty-free-2.0.md#L13))
- 임베디드는 제외다. "The License does not permit the use of the Software within Embedded Systems." 임베디드는 "a computer system designed to perform a specific task within a larger mechanical or electrical system"이다. 기계·전기 장치 안에서 한 가지 일을 하는 시스템을 말한다. facet은 해당하지 않는다. ([본문 L33](https://github.com/slint-ui/slint/blob/372cf0ee5577c3dfec309a45e7b778ba4e81b734/LICENSES/LicenseRef-Slint-Royalty-free-2.0.md#L33))
- 저장소 설명도 같은 선을 긋는다. "meant for applications that run on a user's own general-purpose computer or phone, installed as one application among many." 사용자 컴퓨터에 여러 앱 중 하나로 설치되는 앱용이다. ([LICENSE.md L34-L35](https://github.com/slint-ui/slint/blob/372cf0ee5577c3dfec309a45e7b778ba4e81b734/LICENSE.md#L34-L35))
- 그 밖의 제한 세 가지. ([본문 L31-L37](https://github.com/slint-ui/slint/blob/372cf0ee5577c3dfec309a45e7b778ba4e81b734/LICENSES/LicenseRef-Slint-Royalty-free-2.0.md#L31-L37))
  - "does not permit to distribute or make the Software publicly available alone and without integration into an Application." Slint만 따로 배포하면 안 된다. facet은 Slint를 저장소에 넣지 않고 Cargo 의존성으로 받아 앱에 넣어 배포하므로 해당하지 않는다.
  - "does not permit the distribution of Application that exposes the APIs, in part or in total, of the Software." Slint API를 밖에 내놓는 앱은 안 된다. 나중에 플러그인·스크립트 기능을 만들 때 Slint 타입이나 `.slint` 로딩을 플러그인 쪽에 열면 이 조항에 걸린다.
  - "You may not remove or alter any license notices ... contained within the source code form of the Software." Slint 소스의 라이선스 표기를 지우거나 고치면 안 된다.

### MIT와의 관계

- 충돌하지 않는다. 저장소 FAQ가 "Yes — whichever Slint license you use, your own source files can stay under a permissive license such as MIT or Apache-2.0."라고 답한다. 어떤 Slint 라이선스를 쓰든 facet 자체 파일은 MIT로 둘 수 있다. ([FAQ.md L61-L63](https://github.com/slint-ui/slint/blob/372cf0ee5577c3dfec309a45e7b778ba4e81b734/FAQ.md#L61-L63))
- MIT는 facet 코드에만 붙는다. Slint는 계속 Slint의 라이선스를 따른다. ([FAQ.md L65](https://github.com/slint-ui/slint/blob/372cf0ee5577c3dfec309a45e7b778ba4e81b734/FAQ.md#L65))
- facet을 받아 고쳐 다시 배포하는 제3자에게 facet의 Slint 라이선스 선택은 따라가지 않는다. 그 사람이 직접 셋 중 하나를 고른다. "If the forked application also uses Slint, then the person forking the application can choose to use Slint under any one of the licenses - Royalty-free, GPLv3, or Commercial license." ([FAQ.md L106-L108](https://github.com/slint-ui/slint/blob/372cf0ee5577c3dfec309a45e7b778ba4e81b734/FAQ.md#L106-L108))
- 그래서 로열티 프리를 고른 제3자는 아래 표시 의무와 제한을 스스로 지켜야 한다. 정보 화면을 지우고 배포하려면 웹 페이지 배지를 달거나, GPLv3나 상용 라이선스로 옮겨야 한다.
- Slint 자체를 고친 경우의 배포 방식은 제한하지 않는다. "The license does not restrict 'if' and 'how' the modifications to Slint should be distributed." ([FAQ.md L110-L112](https://github.com/slint-ui/slint/blob/372cf0ee5577c3dfec309a45e7b778ba4e81b734/FAQ.md#L110-L112))
- facet의 README나 LICENSE 근처에 「facet 코드는 MIT, Slint는 별도 라이선스(GPLv3·로열티 프리·상용 중 선택)」라고 한 줄 적어 두면 제3자가 헷갈리지 않는다. 라이선스가 요구하는 것은 아니다.

### 표시 의무

- 배포할 때 아래 둘 중 **하나**를 해야 한다. "You may distribute the Software as part of an Application, modified or unmodified, provided that You do either of the following" ([본문 L21-L27](https://github.com/slint-ui/slint/blob/372cf0ee5577c3dfec309a45e7b778ba4e81b734/LICENSES/LicenseRef-Slint-Royalty-free-2.0.md#L21-L27))
  - (a) 앱 안 정보 화면: "Display the `AboutSlint` widget in an "About" screen or dialog that is accessible from the top level menu of the Application. In the absence of such a screen or dialog, display the widget in the "Splash Screen" of the Application."
    - 무엇을: Slint 표준 위젯 `AboutSlint`. 직접 그린 문구나 로고가 아니라 이 위젯을 띄워야 한다.
    - 어디에: 앱의 최상위 메뉴에서 열 수 있는 「정보(About)」 화면이나 대화상자. 그런 화면이 없으면 시작 화면(splash screen).
    - 위젯 내용: v1.18.1 기준으로 「Made with Slint」 로고(밝은·어두운 테마별), "Version 1.18.1\nhttps://slint.dev/" 문구, 누르면 slint.dev를 여는 동작이다. ([about-slint.slint](https://github.com/slint-ui/slint/blob/372cf0ee5577c3dfec309a45e7b778ba4e81b734/internal/compiler/widgets/common/about-slint.slint)) 쓰는 법은 `import { AboutSlint } from "std-widgets.slint";` 뒤 `AboutSlint {}`이다. ([aboutslint.mdx](https://github.com/slint-ui/slint/blob/372cf0ee5577c3dfec309a45e7b778ba4e81b734/docs/astro/src/content/docs/reference/std-widgets/misc/aboutslint.mdx))
  - (b) 공개 웹 페이지: "Display the Slint attribution badge on a public webpage, preferably where the binaries of your Application can be downloaded from, in such a way that it can be easily found by any visitor to that page."
    - 무엇을: Slint 배지 이미지. 본문이 가리키는 파일은 [`logo/MadeWithSlint-logo-whitebg.png`](https://github.com/slint-ui/slint/blob/372cf0ee5577c3dfec309a45e7b778ba4e81b734/logo/MadeWithSlint-logo-whitebg.png)다.
    - 어디에: 누구나 볼 수 있는 웹 페이지. 되도록 바이너리를 내려받는 페이지(facet이면 GitHub Releases나 README).
    - 어떻게: 방문자가 쉽게 찾을 수 있게.
- facet의 선택: (a)를 기본으로 한다. 최상위 메뉴(또는 그에 해당하는 설정·도움말 메뉴)에서 여는 정보 화면에 `AboutSlint`를 넣는다. 저장소 README에 (b) 배지도 함께 달면 정보 화면 구성이 바뀌어도 의무가 끊기지 않는다.
- 저장소 설명도 같은 조건을 요약한다. "The Royalty-free License is free, as long as you disclose that you use Slint (for example with the `AboutSlint` widget or the Slint badge); without that disclosure, use the Commercial license." ([LICENSE.md L31](https://github.com/slint-ui/slint/blob/372cf0ee5577c3dfec309a45e7b778ba4e81b734/LICENSE.md#L31))
- slint.dev [가격 페이지](https://slint.dev/pricing)의 비교 표도 맞는다. 「Attribution to Slint is optional」 줄은 유료 세 요금제에만 표시되고 Royalty-Free 열에는 표시되지 않는다. 「No obligation to provide source code upon distribution」 줄은 Royalty-Free 열에 표시된다. 표시는 필수, 소스 공개 의무는 없다는 뜻이다. (2026-11-15에 페이지 HTML로 확인)

### Cargo에서의 표시

- `slint` crate의 라이선스 필드는 셋 중 하나를 고르는 SPDX 식이다. `license = "GPL-3.0-only OR LicenseRef-Slint-Royalty-free-2.0 OR LicenseRef-Slint-Software-3.0"` ([Cargo.toml L82](https://github.com/slint-ui/slint/blob/372cf0ee5577c3dfec309a45e7b778ba4e81b734/Cargo.toml#L82))
- Slint 문서에는 Cargo 의존성에서 라이선스를 고르는 방법이 나오지 않는다. 고르는 행위는 라이선스 조건(표시 의무)을 지키는 것으로 이뤄지고, Cargo 설정으로 하는 것이 아니다.
- facet은 고른 라이선스를 이 ADR에 적는 것으로 기록한다. 나중에 `cargo-deny` 같은 라이선스 검사를 붙이면 `LicenseRef-Slint-Royalty-free-2.0`을 허용 목록에 넣는다.

### 결론

- **쓸 수 있다.** facet은 Windows PC에 설치하는 데스크톱 앱이고 임베디드가 아니므로 로열티 프리 2.0의 적용 대상이다. facet 코드는 MIT로 둔다.
- 조건은 셋이다. 정보 화면에 `AboutSlint`(또는 웹 페이지 배지)를 보인다. Slint API를 플러그인 등으로 밖에 내놓지 않는다. Slint 소스의 라이선스 표기를 건드리지 않는다.
- Slint를 올릴 때마다 라이선스 본문이 바뀌었는지 다시 본다. 본문 버전(2.0)이 바뀌면 이 절을 고친다.

## 검증 결과

2026-11-15, #9. 수치는 #9 댓글(PR #73 진단, #75 렌더러 비교)에 있다.

### 측정 기계

i5-14600KF, 메모리 64GB, NVMe(Seagate FireCuda 520), RTX 4060 Ti, 1920×1080 60Hz, Windows 11 Pro 10.0.26300. 릴리스 빌드, 10만 항목 시험 폴더(`cargo xtask test-folder`), 창 900×600. 측정하는 동안 다른 사용자 앱(OBS·Chrome·Discord)이 켜져 있어 CPU를 20~40% 썼다.

### 수치와 판정

| 항목 | 결과 | 기준 | 판정 |
| --- | --- | --- | --- |
| 콜드 스타트 중앙값 (software 렌더러) | 142.6 / 184.8ms, 20회 묶음 155.0ms | 200ms 미만 | 통과 |
| 코어 목록 읽기 10만 항목 | 중앙값 56.1ms, 최악값 60.3ms | 100ms 미만 | 통과 |
| 스크롤 프레임 중앙값 | 5.3ms | (참고) | — |
| 스크롤 프레임 최악값 | 18.1 / 11.8 / 12.0ms, 상한을 넘은 프레임 0 | 33.3ms 이하 | 통과 |

- 처음 Slint 기본 렌더러 femtovg(OpenGL)에서는 콜드 스타트가 315~523ms로 기준을 넘었다. `AppWindow::new()`에서 OpenGL 렌더러를 준비하는 데 140~160ms가 들었다. skia(D3D)는 420~446ms였다. 그래서 **렌더러를 software로 바꿨다**(#75, PR #78). 실행 파일도 13.7MB에서 13.0MB로 줄었다.
- 스크롤은 Slint 기본 가상화 목록이 세 렌더러 모두에서 기준을 넘었다. 직접 만든 가상 스크롤은 필요 없다.
- 첫 측정에서 나온 프레임 최악값 2835.7ms는 다시 재어도 나오지 않았다. 측정 중 기계가 잠깐 멈춘 것으로 본다.

### 한글 IME

Microsoft 입력기(두벌식), software 렌더러 빌드(8e0f7b4), `docs/ime-checklist.md`. 키 입력은 실제 키 코드(`SendInput`)로 보내 IME를 거쳤다.

| # | 항목 | 판정 |
| - | --- | --- |
| 1 | 「새 폴더 가나다」 입력과 조합 표시 | 통과 |
| 2 | 조합 중 Enter에서 한 번만 확정 | 통과 |
| 3 | 조합 중 Esc 취소 | 통과 |
| 4 | 조합 중 ←·클릭으로 확정과 커서 이동 | 통과 |
| 5 | 한자 키 후보 창 | 통과 |
| 6 | 한/영 전환 뒤 영문 입력 | 통과 |
| — | 「함께 볼 것」 세 항목 | 통과 (항목이 적은 폴더라 목록이 움직이지 않는지는 제한적으로 봤다) |

표시 버그 하나가 남았다. 이름 전체가 선택된 채로 조합을 시작하면, 조합하는 동안 선택된 글자가 지워지지 않고 남아 보인다. 확정하면 값은 맞다(#80). 렌더러와는 상관없다.

### 라이선스

위 「라이선스 확인」 절대로 로열티 프리 2.0으로 쓸 수 있다. 렌더러를 바꿔도 조건은 같다.

### 남은 위험

- software 렌더러의 콜드 스타트는 기준 200ms와의 차이가 크지 않다.
- software 렌더러는 화면 크기에 비례해 그리는 비용이 늘어난다. 창을 1080p 화면 크기로 키웠을 때 프레임 최악값은 28.6ms로, 상한 33.3ms에 가까웠다. 4K·고주사율 화면을 가진 기계에서 spec이 끝날 때마다 다시 잰다. 그때 기준을 못 넘으면 렌더러 선택(software·skia·femtovg)을 다시 본다.
