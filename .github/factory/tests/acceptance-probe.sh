#!/usr/bin/env bash
# Tests of .github/factory/acceptance-probe.sh against a fake cargo (tests/fake/cargo), #414.
# Each case is a throwaway git repository whose spec branch adds an ignored acceptance test, and
# canned `cargo test` output for one reason the test is red.
set -u
here=$(cd "$(dirname "$0")" && pwd)
script="$here/../acceptance-probe.sh"
export PATH="$here/fake:$PATH"
root=$(mktemp -d)
trap 'rm -r "$root"' EXIT
passes=0 fails=0 current=
ok() { passes=$((passes + 1)); }
bad() { echo "  FAIL [$current] $*"; fails=$((fails + 1)); }
eq() { if [ "$2" = "$3" ]; then ok; else bad "$1: expected [$2], got [$3]"; fi; }
has() { case "$2" in *"$1"*) ok ;; *) bad "expected [$1] in: $2" ;; esac; }

g() { git -c user.name=t -c user.email=t@example.invalid -c core.autocrlf=false "$@" > /dev/null 2>&1; }

# setup <name>: a repository with main (a crate without spec tests) and the spec branch checked out
# with crates/foo/tests/spec_x.rs: test `adds` (#42) asserts on line 7 and unwraps on line 6.
setup() {
  current=$1
  repo="$root/$RANDOM$RANDOM"
  mkdir -p "$repo/crates/foo/tests" "$repo/crates/foo/src"
  cd "$repo" || exit 1
  printf '[package]\nname = "foo"\nversion = "0.1.0"\n\n[dependencies]\n' > crates/foo/Cargo.toml
  printf 'pub fn thing() -> Result<u32, String> {\n    Ok(1)\n}\n' > crates/foo/src/lib.rs
  g init -q -b main . && g add -A && g commit -q -m base
  g checkout -q -b spec
  write_spec
  g add -A && g commit -q -m spec
  export FAKE_CARGO_LOG="$repo/.cargo-calls" FAKE_CARGO_OUT="$repo/.cargo-out" FAKE_CARGO_EXIT=101
  export PROBE_LOG="$repo/.factory/probe.log"
  : > "$FAKE_CARGO_LOG"
}
write_spec() {
  mkdir -p crates/foo/tests
  cat > crates/foo/tests/spec_x.rs <<'RS'
use foo::thing;

#[test]
#[ignore = "#42"]
fn adds() {
    let v = thing().unwrap();
    assert_eq!(v, 3);
}

#[test]
fn already_merged_spec_test_is_not_probed() {}
RS
}
cargo_says() { printf '%s\n' "$@" > "$FAKE_CARGO_OUT"; }
probe() { out=$(bash "$script" main 2>&1); code=$?; }
failed_at() { # failed_at <location> <message>: cargo output of `adds` panicking there
  cargo_says '' 'running 1 test' 'test adds ... FAILED' '' 'failures:' '' '---- adds stdout ----' \
    "thread 'adds' panicked at $1:" "$2" 'note: run with `RUST_BACKTRACE=1`' '' 'failures:' '    adds' '' \
    'test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 1 filtered out'
}

setup "an assertion failure passes"
failed_at crates/foo/tests/spec_x.rs:7:5 'assertion `left == right` failed'
probe
eq exit 0 "$code"
has 'acceptance-probe ok crates/foo/tests/spec_x.rs::adds (#42): 단언 실패' "$out"
eq "cargo call" "test -p foo --test spec_x -- --ignored adds" "$(cat "$FAKE_CARGO_LOG")"

setup "a custom assertion message is still an assertion (by its line)"
failed_at crates/foo/tests/spec_x.rs:7:5 'the sum must be three'
probe
eq exit 0 "$code"
has '단언 실패' "$out"

setup "a todo! stub passes"
failed_at crates/foo/src/lib.rs:2:5 'not yet implemented: #42'
probe
eq exit 0 "$code"
has 'acceptance-probe ok crates/foo/tests/spec_x.rs::adds (#42): 스텁' "$out"

setup "an unwrap panic fails"
failed_at crates/foo/tests/spec_x.rs:6:21 'called `Result::unwrap()` on an `Err` value: "no fixture"'
probe
eq exit 1 "$code"
has 'acceptance-probe FAILED crates/foo/tests/spec_x.rs::adds (#42): 단언·스텁이 아닌 panic' "$out"

setup "a panic in a library outside the tests fails"
failed_at crates/foo/src/lib.rs:2:5 'index out of bounds: the len is 0 but the index is 0'
probe
eq exit 1 "$code"
has 'FAILED' "$out"

setup "a compile error fails and shows the error"
cargo_says '   Compiling foo v0.1.0' 'error[E0425]: cannot find function `thing2` in crate `foo`' \
  ' --> crates/foo/tests/spec_x.rs:6:13' '' 'error: could not compile `foo` (test "spec_x") due to 1 previous error'
probe
eq exit 1 "$code"
has 'acceptance-probe FAILED crates/foo/tests/spec_x.rs::adds (#42): 컴파일 오류' "$out"
has 'error[E0425]: cannot find function' "$out"

setup "an already green test fails"
cargo_says '' 'running 1 test' 'test adds ... ok' '' 'test result: ok. 1 passed; 0 failed; 0 ignored'
FAKE_CARGO_EXIT=0
probe
eq exit 1 "$code"
has '이미 초록' "$out"

setup "a test cargo did not run fails"
cargo_says '' 'running 0 tests' '' 'test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 1 filtered out'
FAKE_CARGO_EXIT=0
probe
eq exit 1 "$code"
has '시험을 찾지 못했다' "$out"

setup "no changed acceptance test runs nothing"
g checkout -q main
g checkout -q -b other
printf '// docs\n' >> crates/foo/src/lib.rs && g add -A && g commit -q -m other
probe
eq exit 0 "$code"
has '바뀐 인수 시험 없음' "$out"
eq "cargo calls" "" "$(cat "$FAKE_CARGO_LOG")"

setup "an uncommitted fix (Odin's) is probed too"
g checkout -q main
g checkout -q -b uncommitted
write_spec
failed_at crates/foo/tests/spec_x.rs:7:5 'assertion `left == right` failed'
probe
eq exit 0 "$code"
has 'acceptance-probe ok crates/foo/tests/spec_x.rs::adds' "$out"

cd "$here" || exit 1
echo "acceptance-probe: $passes passed, $fails failed"
[ "$fails" -eq 0 ]
