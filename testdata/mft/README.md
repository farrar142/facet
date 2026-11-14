# MFT 조각

MFT 파서 테스트에 쓰는, 시험 NTFS 볼륨에서 떠 낸 MFT 레코드 묶음과 그 기대값 목록이다
(spec #22의 Testing Decisions, 티켓 #38). 파서는 관리자 권한 없이 이 파일로 테스트한다.

| 파일 | 내용 |
| --- | --- |
| `ntfs-basic.mft` | 시험 VHD의 MFT 레코드 0..256. 레코드당 1024바이트를 디스크 그대로 이어 붙였다(고정값(fixup)을 되돌리지 않았다). 레코드 `n`은 `n * 1024`바이트 위치에 있다. |
| `ntfs-basic.expected.tsv` | 그 조각에 들어 있어야 하는 항목의 기대값 목록. |

## 담긴 경우

spec이 요구한 경우를 모두 담는다. 각 경우가 어느 레코드인지는 기대값 목록에 있다.

- 한글 이름(`\한글 폴더`, `\한글 폴더\문서.txt`), 이모지 이름(`\이모지 😀🎉 파일.txt`, UTF-16 대리 쌍), 긴 이름(UTF-16 250자)
- 여러 단계 폴더(`\깊은 폴더\1단계\2단계\3단계\4단계\끝.txt`)
- 하드 링크: 레코드 하나에 서로 다른 폴더의 이름 셋(`하드 링크 원본.txt` 등)
- 대체 데이터 스트림: `\대체 스트림.txt`에 `:숨은 스트림`(3000바이트, 비상주)과 `:Zone.Identifier`. 스트림은 항목이 아니다. 기대값의 크기는 이름 없는 `$DATA`의 크기다.
- 지운 레코드: 지운 파일 둘과 지운 폴더 하나. 이름은 레코드에 남아 있지만 레코드 머리의 사용 중 표시(플래그 0x0001)가 꺼져 있다.
- `$ATTRIBUTE_LIST`와 확장 레코드: `\링크 많음\속성 많은 파일.txt`에 하드 링크 40개를 걸어 `$FILE_NAME` 속성이 기본 레코드에 다 들어가지 않게 했다. 기본 레코드에 `$ATTRIBUTE_LIST`가 있고 이름 대부분은 확장 레코드에 있다.
- 크기 0 파일, 상주 `$DATA`, 비상주 `$DATA`(100000바이트)
- 8.3 짧은 이름: 볼륨의 8.3 이름 만들기를 켰으므로 한글·이모지·긴 이름 파일에는 DOS 이름공간의 `$FILE_NAME`이 따로 있다. 기대값 목록에는 Win32 이름만 적는다(짧은 이름은 항목이 아니다).
- NTFS 메타데이터 파일(레코드 0..23, `$Extend` 아래)은 포맷이 만든 그대로다. 기대값 목록에는 볼륨 루트(레코드 5)만 적는다.

일부러 깨뜨린 레코드(서명 틀림, 길이 넘침, 고정값 틀림)는 넣지 않았다. 파서 테스트
(`crates/facet-index/tests/mft.rs`)가 이 조각을 복사해 고쳐 만든다(티켓 #43).

포맷 뒤 Windows가 스스로 만드는 `\System Volume Information`(숨김·시스템 폴더)과 그 안의 파일도
조각에 들어 있다. 사용자 항목이므로 파서는 이것도 항목으로 내지만, 조각을 만드는 도구가 만든 것이
아니어서 기대값 목록에는 적지 않는다.

## 기대값 목록 형식

UTF-8 텍스트, 탭으로 나눈 열, `#`으로 시작하는 줄은 주석이다. 레코드 번호 순서다.

| 열 | 뜻 |
| --- | --- |
| 레코드 | MFT 레코드 번호(파일 ID의 아래 48비트) |
| 순번 | 그 레코드의 순번(sequence number, 파일 ID의 위 16비트). 지운 레코드는 지우기 **전** 값이다(지우면 디스크의 순번이 오른다). 확장 레코드는 그 레코드 머리의 값이다. |
| 종류 | `폴더`, `파일`, `지운 파일`, `지운 폴더`, `확장 레코드` |
| 경로 | 볼륨 루트에서의 경로. `\`로 시작한다. 하드 링크는 이름마다 한 줄이다. 확장 레코드는 기본 레코드의 경로다. |
| 크기 | 파일의 이름 없는 `$DATA` 크기(바이트). 그 밖에는 `-`. |
| 수정 날짜 | Windows가 알려 준 수정 날짜(`FILETIME`, 1601-01-01 UTC부터 100ns 단위, `$STANDARD_INFORMATION`의 값). 모든 항목을 만들고 지운 뒤에 읽는다. 지운 레코드·확장 레코드는 `-`. |
| 속성 | Windows가 알려 준 파일 속성(`0x` 16진수 8자리, 폴더는 `0x10` 포함). 지운 레코드·확장 레코드는 `-`. |
| 비고 | 이 줄이 담은 경우. 확장 레코드는 기본 레코드 번호. |

## 다시 만들기

레코드 번호는 Windows 버전에 따라 달라질 수 있으므로 조각과 기대값 목록은 언제나 함께 다시 만든다.

### 명령 하나로

관리자 터미널(Windows)에서:

```
cargo xtask mft-fixture
```

`xtask/src/mft_fixture.rs`가 아래 단계를 차례로 하고, 떠 낸 조각에서 지운 레코드의 사용 중 표시가
꺼졌는지와 `$ATTRIBUTE_LIST`가 실제로 생겼는지 확인한 뒤 두 파일을 이 폴더에 쓴다. 시험 VHD는
끝나면 떼고 지운다.

### 손으로

1. **시험 VHD 만들기** — 임시 폴더에 아래 내용으로 `vhd.txt`를 만들고 `diskpart /s vhd.txt`.
   `T`는 비어 있는 드라이브 문자로 바꾼다. 경로는 8.3 짧은 이름(`RUNNER~1` 같은) 없이 전체 경로로
   적는다. diskpart는 붙일 때와 다른 표기로 고르면 떼지 못한다.

   ```
   create vdisk file="C:\Users\me\AppData\Local\Temp\facet-test.vhdx" maximum=64 type=expandable
   select vdisk file="C:\Users\me\AppData\Local\Temp\facet-test.vhdx"
   attach vdisk
   create partition primary
   format fs=ntfs quick label=facet-test
   assign letter=T
   ```

2. **8.3 짧은 이름 켜기** — `fsutil 8dot3name set T: 0`
3. **항목 만들기** — `xtask/src/mft_fixture.rs`의 `populate`와 같은 순서로 만든다: 한글 폴더와 그 안의
   파일, 이모지 이름 파일, 긴 이름 파일, 여러 단계 폴더, 빈 파일, 큰 파일, 하드 링크 셋
   (`mklink /H`), 대체 데이터 스트림 둘(`echo x > "T:\대체 스트림.txt:숨은 스트림"`), 하드 링크 40개를
   건 파일, 그리고 마지막으로 지울 파일·폴더를 만들어 레코드 번호를 적은 뒤 지운다. 레코드 번호는
   `fsutil file queryfileid <경로>`의 아래 48비트다.
4. **떠 내기** — `cargo run -p facet-indexer -- dump-mft T: testdata\mft\ntfs-basic.mft`.
   볼륨의 쓰기 버퍼를 비운 뒤 MFT의 유효 데이터 끝까지 떠 낸다(`--start`, `--count`로 범위를 줄인다).
5. **기대값 목록 쓰기** — 위 형식으로 `ntfs-basic.expected.tsv`를 쓴다. 수정 날짜와 속성은
   PowerShell의 `[IO.File]::GetLastWriteTimeUtc(경로).ToFileTimeUtc()`와 `(Get-Item 경로).Attributes.value__`로 읽는다.
6. **VHD 떼기** — `select vdisk file="..."`, `detach vdisk`를 diskpart로 돌리고 `.vhdx` 파일을 지운다.
