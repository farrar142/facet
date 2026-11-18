#!/usr/bin/env bash
# Odin's acceptance probe (#414): are the spec PR's acceptance tests red for the right reason?
#
#   acceptance-probe.sh [<base ref>]        (default origin/main)
#
# Picks the test files under `crates/<crate>/tests/` that the spec PR adds or changes (against the
# merge base with <base ref>, working tree and untracked files included: Odin's fixes are not
# committed yet), and in them every test switched off with `#[ignore = "#<ticket>"]`. Runs those
# tests once with the ignore lifted (`cargo test -p <crate> --test <file> -- --ignored <names>`) and
# sorts each one by why it is red:
#
#   assert   an assertion failed (`assert!`, `assert_eq!`, `assert_ne!`, `panic!` in the test file)  ok
#   stub     a ticket stub panicked (`todo!()`, `unimplemented!()`)                                ok
#   compile  the test file did not compile                                                          fails
#   missing  cargo ran but reported no result for the test                                           fails
#   green    the test already passes: it asserts nothing the ticket has to build                     fails
#   other    any other panic (`unwrap`/`expect` failed, fixture or I/O error, index out of range)   fails
#
# An `assert`/`stub` test can pass once the ticket is implemented; the others cannot, or say nothing.
# Prints one line per test (`ok` or `FAILED`, so the workflow's log digest picks the failures up) and,
# for a compile error, the compiler's error lines. Cargo's full output goes to $PROBE_LOG (default
# .factory/probe-cargo.log). Exits 0 when every probed test is `assert` or `stub` (or there are none),
# 1 otherwise, 2 on a usage error. The panic location decides `assert` against `other`: the source
# line the panic points at (cargo prints it relative to the workspace root) is read from the checkout.
set -u
[ $# -le 1 ] || { echo 'usage: acceptance-probe.sh [<base ref>]' >&2; exit 2; }
base=${1:-origin/main}
log=${PROBE_LOG:-.factory/probe-cargo.log}
mkdir -p "$(dirname "$log")"
: > "$log"

mb=$(git merge-base "$base" HEAD 2>/dev/null) || { echo "acceptance-probe: no merge base with $base" >&2; exit 2; }
files=$( { git diff --name-only --diff-filter=AM "$mb" -- 'crates/*/tests/*.rs'
           git ls-files --others --exclude-standard -- 'crates/*/tests/*.rs'; } \
         | tr -d '\r' | grep -E '^crates/[^/]+/tests/[^/]+\.rs$' | sort -u)

# probed <file>: "<name> <ticket>" for each test switched off with #[ignore = "#<ticket>"].
probed() {
  tr -d '\r' < "$1" | awk '
    match($0, /#\[ignore *= *"#[0-9]+/) { t = substr($0, RSTART, RLENGTH); sub(/.*#/, "", t); pending = t; next }
    pending != "" && match($0, /fn +[A-Za-z0-9_]+/) {
      n = substr($0, RSTART, RLENGTH); sub(/fn +/, "", n); print n, pending; pending = ""
    }'
}

# results <cargo output>: "<name>\t<status>\t<location>\t<message>" per test cargo reported, where
# status is ok/FAILED/ignored and location/message are the panic's (FAILED only).
results() {
  tr -d '\r' < "$1" | awk '
    function short(n) { sub(/.*::/, "", n); return n }
    /^test .* \.\.\. (ok|FAILED|ignored)/ {
      n = $2; s = $NF; status[short(n)] = s; order[++k] = short(n); next
    }
    /^---- .* stdout ----$/ { cur = short($2); want = 0; next }
    cur != "" && /panicked at / {
      line = $0
      if (match(line, /panicked at .*:[0-9]+:[0-9]+:$/)) {          # Rust >= 1.73: message on the next line
        l = substr(line, RSTART + 12); sub(/:$/, "", l); loc[cur] = l; want = 1
      } else if (match(line, /panicked at .*, [^ ]+:[0-9]+:[0-9]+$/)) { # older: at '\''msg'\'', file:line:col
        l = line; sub(/.*, /, "", l); loc[cur] = l
        m = line; sub(/.*panicked at '\''/, "", m); sub(/'\'', [^ ]+$/, "", m); msg[cur] = m
      }
      next
    }
    want && cur != "" { msg[cur] = $0; want = 0; next }
    END { for (i = 1; i <= k; i++) { n = order[i]; printf "%s\t%s\t%s\t%s\n", n, status[n], loc[n], msg[n] } }'
}

# kind <location> <message>: assert, stub or other for a failed test.
kind() {
  case "$2" in
    'not yet implemented'* | 'not implemented'*) echo stub; return ;;
    'assertion failed'* | 'assertion `'*) echo assert; return ;;
  esac
  local file=${1%:*}; file=${file%:*}; file=${file//\\//}
  local line=${1%:*}; line=${line##*:}
  case "$file" in crates/*/tests/*) ;; *) echo other; return ;; esac
  case "$line" in '' | *[!0-9]*) echo other; return ;; esac
  local src
  src=$(sed -n "${line}p" "$file" 2>/dev/null | tr -d '\r')
  case "$src" in
    *'.unwrap('* | *'.expect('* | *'.unwrap_err('* | *'.expect_err('*) echo other ;;
    *'assert!('* | *'assert_eq!('* | *'assert_ne!('* | *'assert_matches!('* | *'panic!('* | *'unreachable!('*) echo assert ;;
    *) echo other ;;
  esac
}

total=0 bad=0
report() { # report <ok|FAILED> <file> <name> <ticket> <why>
  total=$((total + 1))
  [ "$1" = ok ] || bad=$((bad + 1))
  printf 'acceptance-probe %s %s::%s (#%s): %s\n' "$1" "$2" "$3" "$4" "$5"
}

for f in $files; do
  list=$(probed "$f")
  [ -n "$list" ] || continue
  dir=${f#crates/}; dir=${dir%%/*}
  pkg=$(tr -d '\r' < "crates/$dir/Cargo.toml" | awk -F'"' '/^\[package\]/ { p = 1; next } /^\[/ { p = 0 } p && /^name *=/ { print $2; exit }')
  stem=$(basename "$f" .rs)
  names=$(printf '%s\n' "$list" | awk '{ print $1 }')
  out=$(mktemp)
  # shellcheck disable=SC2086 # one filter per test name
  cargo test -p "$pkg" --test "$stem" -- --ignored $names > "$out" 2>&1
  { echo "=== cargo test -p $pkg --test $stem -- --ignored $(echo $names)"; cat "$out"; } >> "$log"
  if ! grep -qE '^test .* \.\.\. (ok|FAILED|ignored)|^running [0-9]+ tests?' "$out" \
     && grep -qE '^error(\[E[0-9]+\])?:' "$out"; then
    while read -r name ticket; do report FAILED "$f" "$name" "$ticket" "컴파일 오류"; done <<< "$list"
    tr -d '\r' < "$out" | grep -E -A4 '^error(\[E[0-9]+\])?:' | head -n 40
    rm -f "$out"
    continue
  fi
  res=$(mktemp)
  results "$out" > "$res"
  while read -r name ticket; do
    row=$(awk -F'\t' -v n="$name" '$1 == n { print; exit }' "$res")
    status=$(printf '%s' "$row" | cut -f2)
    case "$status" in
      ok) report FAILED "$f" "$name" "$ticket" "이미 초록(구현 없이 통과한다)" ;;
      ignored) report FAILED "$f" "$name" "$ticket" "--ignored로도 돌지 않았다(다른 ignore가 붙었나)" ;;
      FAILED)
        loc=$(printf '%s' "$row" | cut -f3) msg=$(printf '%s' "$row" | cut -f4)
        case "$(kind "$loc" "$msg")" in
          assert) report ok "$f" "$name" "$ticket" "단언 실패($loc)" ;;
          stub) report ok "$f" "$name" "$ticket" "스텁($msg)" ;;
          *) report FAILED "$f" "$name" "$ticket" "단언·스텁이 아닌 panic($loc: $msg)" ;;
        esac ;;
      *) report FAILED "$f" "$name" "$ticket" "시험을 찾지 못했다(cargo가 결과를 내지 않음)" ;;
    esac
  done <<< "$list"
  rm -f "$out" "$res"
done

if [ "$total" -eq 0 ]; then
  echo "acceptance-probe: 바뀐 인수 시험 없음"
  exit 0
fi
echo "acceptance-probe: $total개 중 $bad개 실패"
[ "$bad" -eq 0 ]
