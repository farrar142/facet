#!/usr/bin/env bash
# Tests of .github/factory/verify.sh: `mech` against throwaway git repositories, `sendback` and
# `pass` against the fake gh (tests/fake/gh).
set -u
here=$(cd "$(dirname "$0")" && pwd)
verify="$here/../verify.sh"
export PATH="$here/fake:$PATH"
export REPO=owner/repo

fails=0
passes=0
current=
root=$(mktemp -d)
trap 'rm -rf "$root"' EXIT

ok() { passes=$((passes + 1)); }
bad() { echo "  FAIL [$current] $*"; fails=$((fails + 1)); }
eq() { # eq <what> <expected> <actual>
  if [ "$2" = "$3" ]; then ok; else bad "$1: expected [$2], got [$3]"; fi
}
has() { # has <what> <needle> <haystack>
  case "$3" in *"$2"*) ok ;; *) bad "$1: [$2] not in [$3]" ;; esac
}
lacks() { # lacks <what> <needle> <haystack>
  case "$3" in *"$2"*) bad "$1: [$2] unexpectedly in [$3]" ;; *) ok ;; esac
}

# --- mech: a repository whose main has the acceptance tests of tickets #7 and #8 -------------

SPEC=crates/core/tests/spec_demo_a.rs

spec_file() { # the acceptance test file as the spec PR merged it
  cat <<'EOF'
#[test]
#[ignore = "#7"]
fn seven_does_it() {
    assert_eq!(core::seven(), 7);
}

#[test]
#[ignore = "#8"]
fn eight_does_it() {
    assert_eq!(core::eight(), 8);
}

#[test]
#[ignore = "#70"]
fn seventy_does_it() {
    assert_eq!(core::seventy(), 70);
}
EOF
}

# repo <name>: a new repository with main = the spec file and a source file, on branch "ticket"
repo() {
  current=$1
  r="$root/$1"
  mkdir -p "$r/crates/core/tests" "$r/crates/core/src"
  cd "$r" || exit 1
  git init -q -b main .
  git config core.autocrlf false
  git config user.name test
  git config user.email test@example.invalid
  spec_file > "$SPEC"
  echo 'pub fn seven() -> u32 { 0 }' > crates/core/src/lib.rs
  git add -A && git commit -q -m base
  git switch -q -c ticket
}

commit() { git add -A && git commit -q -m change; }

# mech_run: runs `verify.sh mech 7 main`; sets $status and $report
mech_run() {
  report=$(bash "$verify" mech 7 main 2>&1)
  status=$?
}

repo only-ignore-lines
sed -i '/#\[ignore = "#7"\]/d' "$SPEC"
echo 'pub fn seven() -> u32 { 7 }' > crates/core/src/lib.rs
commit
mech_run
eq "exit" 0 "$status"
has "report" "기계 검사 통과" "$report"

repo ignore-left
echo 'pub fn seven() -> u32 { 7 }' > crates/core/src/lib.rs
commit
mech_run
eq "exit" 1 "$status"
has "report" '#[ignore = "#7"]`이 남아 있다' "$report"
has "report names the file" "$SPEC" "$report"

repo ignore-left-elsewhere
sed -i '/#\[ignore = "#7"\]/d' "$SPEC"
printf '#[test]\n#[ignore = "#7"]\nfn extra() {}\n' > crates/core/tests/other.rs
commit
mech_run
eq "exit" 1 "$status"
has "report" "crates/core/tests/other.rs" "$report"

# Odin's stub of the contract (todo!("#7")) must be gone; another ticket's stub may stay.
repo stub-left
sed -i '/#\[ignore = "#7"\]/d' "$SPEC"
printf 'pub fn seven() -> u32 {\n    todo!("#7")\n}\npub fn seventy() -> u32 {\n    todo!("#70")\n}\n' > crates/core/src/lib.rs
commit
mech_run
eq "exit" 1 "$status"
has "report" 'todo!("#7")` 스텁이 남아 있다' "$report"
has "report names the file" "crates/core/src/lib.rs:2" "$report"
lacks "another ticket's stub" '#70' "$report"

repo stub-replaced
sed -i '/#\[ignore = "#7"\]/d' "$SPEC"
printf 'pub fn seven() -> u32 {\n    7\n}\npub fn eight() -> u32 {\n    todo!("#8")\n}\n' > crates/core/src/lib.rs
commit
mech_run
eq "exit" 0 "$status"
has "report" '기계 검사 통과' "$report"

repo assertion-changed
sed -i '/#\[ignore = "#7"\]/d; s/assert_eq!(core::seven(), 7);/assert_eq!(core::seven(), 0);/' "$SPEC"
commit
mech_run
eq "exit" 1 "$status"
has "report" "인수 시험을 고쳤다(줄을 더함)" "$report"
has "report" "assert_eq!(core::seven(), 0);" "$report"

repo test-deleted
spec_file | sed '1,6d' > "$SPEC"
commit
mech_run
eq "exit" 1 "$status"
has "report" "인수 시험을 고쳤다(줄을 지움)" "$report"
has "report" "fn seven_does_it()" "$report"

repo other-ticket-ignore
sed -i '/#\[ignore = "#7"\]/d; /#\[ignore = "#8"\]/d' "$SPEC"
commit
mech_run
eq "exit" 1 "$status"
has "report" '다른 티켓의 인수 시험을 켰다' "$report"
has "report" '#[ignore = "#8"]' "$report"

repo line-added
sed -i '/#\[ignore = "#7"\]/d' "$SPEC"
printf '\n#[test]\nfn mine() {}\n' >> "$SPEC"
commit
mech_run
eq "exit" 1 "$status"
has "report" "fn mine() {}" "$report"

repo new-spec-file
sed -i '/#\[ignore = "#7"\]/d' "$SPEC"
printf '#[test]\nfn mine() {}\n' > crates/core/tests/spec_demo_b.rs
commit
mech_run
eq "exit" 1 "$status"
has "report" "spec_demo_b.rs" "$report"

# main moved on after the branch started (another spec PR changed the file): only the branch's
# own change counts.
repo main-moved
sed -i '/#\[ignore = "#7"\]/d' "$SPEC"
commit
git switch -q main
printf '\n#[test]\n#[ignore = "#9"]\nfn nine() {}\n' >> "$SPEC"
commit
git switch -q ticket
mech_run
eq "exit" 0 "$status"

repo crlf-checkout
sed -i '/#\[ignore = "#7"\]/d' "$SPEC"
commit
sed -i 's/$/\r/' "$SPEC" # a working tree with CRLF does not matter: mech reads HEAD
mech_run
eq "exit" 0 "$status"

cd "$root" || exit 1

# --- sendback and pass against the fake gh -----------------------------------------------------

setup() {
  current=$1
  tmp=$(mktemp -d "$root/g.XXXXXX")
  export FAKE_GH="$tmp/gh" FACTORY_MERGE_WAIT=0 RUN_URL=https://example.invalid/run/1
  mkdir -p "$FAKE_GH"
  : > "$FAKE_GH/calls.log"
  printf '#!/usr/bin/env bash
touch "%s/next-dispatched"; printf "%%s\n" "$*" >> "%s/next-args"
' "$tmp" "$tmp" > "$tmp/next.sh"
  export FACTORY_DISPATCH_NEXT="bash $tmp/next.sh"
  issue 7 in-progress
  jq -n '{number: 40, state: "open", merged: false, head: {ref: "agent/issue-7"}}' | tr -d '\r' > "$FAKE_GH/pull-40.json"
  printf 'AC-1: 미충족 — 시험이 없다\n' > "$tmp/reason.md"
}

issue() { # issue <n> <labels comma-separated>
  jq -n --argjson n "$1" --arg l "$2" \
    '{number: $n, state: "open", body: "", labels: ($l | split(",") | map(select(. != "") | {name: .}))}' \
    | tr -d '\r' > "$FAKE_GH/issue-$1.json"
}
labels_of() { jq -r '[.labels[].name] | join(",")' "$FAKE_GH/issue-$1.json" | tr -d '\r'; }
state_of() { jq -r .state "$FAKE_GH/$1.json" | tr -d '\r'; }
count_marker() { # count_marker <n> <marker>
  local f="$FAKE_GH/comments-$1.json"
  [ -f "$f" ] || { echo 0; return; }
  jq --arg m "$2" '[.[] | select(.body | contains($m))] | length' "$f" | tr -d '\r'
}
calls() { cat "$FAKE_GH/calls.log"; }
next_dispatched() { [ -e "$tmp/next-dispatched" ] && echo yes || echo no; }

runs_left() { jq -r '[.[].id] | join(",")' "$FAKE_GH/runs.json" | tr -d ''; }

setup first-sendback
jq -n '[{id: 1, event: "pull_request", head_branch: "agent/issue-7", triggering_actor: {login: "github-actions[bot]"}, jobs: 0}]' | tr -d '' > "$FAKE_GH/runs.json"
bash "$verify" sendback 7 40 "$tmp/reason.md" > /dev/null
eq "exit" 0 "$?"
eq "sendback comments" 1 "$(count_marker 7 '<!-- factory-sendback -->')"
has "the reason is in the comment" "AC-1: 미충족" "$(jq -r '.[0].body' "$FAKE_GH/comments-7.json")"
eq "PR state" closed "$(state_of pull-40)"
has "branch deleted" "git/refs/heads/agent/issue-7" "$(calls)"
eq "bot pull_request run deleted (#270)" "" "$(runs_left)"
eq "labels" ready-for-agent "$(labels_of 7)"
eq "the ticket goes first into a track (#286)" "--first 7" "$(cat "$tmp/next-args")"
lacks "no direct dispatch" "factory-implementer.yml" "$(calls)"
eq "no human marker" 0 "$(count_marker 7 '<!-- factory-human -->')"

setup second-sendback
jq -n '[{id: 1, created_at: "2026-11-17T01:00:00Z", body: "<!-- factory-sendback -->\nVerify가 PR #39을 되돌렸다.\n\n앞 사유"}]' \
  | tr -d '\r' > "$FAKE_GH/comments-7.json"
bash "$verify" sendback 7 40 "$tmp/reason.md" > /dev/null
eq "exit" 0 "$?"
eq "sendback comments" 2 "$(count_marker 7 '<!-- factory-sendback -->')"
eq "human marker" 1 "$(count_marker 7 '<!-- factory-human -->')"
eq "labels" ready-for-human "$(labels_of 7)"
lacks "implementer not dispatched" "factory-implementer.yml" "$(calls)"
eq "next ticket" yes "$(next_dispatched)"
eq "PR state" closed "$(state_of pull-40)"

# Other markers (implementer failures) do not count as send-backs.
setup failures-are-not-sendbacks
jq -n '[{id: 1, created_at: "2026-11-17T01:00:00Z", body: "<!-- factory-implementer-failed -->\nfailed"}]' \
  | tr -d '\r' > "$FAKE_GH/comments-7.json"
bash "$verify" sendback 7 40 "$tmp/reason.md" > /dev/null
eq "labels" ready-for-agent "$(labels_of 7)"
eq "the ticket goes first" "--first 7" "$(cat "$tmp/next-args")"

setup pass
jq -n '[{id: 1, event: "pull_request", head_branch: "agent/issue-7", triggering_actor: {login: "github-actions[bot]"}, jobs: 0}]' | tr -d '' > "$FAKE_GH/runs.json"
printf 'AC-1: 충족 — spec_demo_a::seven_does_it\n' > "$tmp/report.md"
issue 40 ""
bash "$verify" pass 7 40 "$tmp/report.md" > /dev/null
eq "exit" 0 "$?"
eq "merged" true "$(jq -r .merged "$FAKE_GH/pull-40.json" | tr -d '\r')"
has "squash" "pulls/40/merge -f merge_method=squash" "$(calls)"
has "branch deleted" "git/refs/heads/agent/issue-7" "$(calls)"
eq "bot pull_request run deleted (#270)" "" "$(runs_left)"
eq "ticket closed" closed "$(state_of issue-7)"
eq "labels" "" "$(labels_of 7)"
has "ci dispatched" "actions/workflows/ci.yml/dispatches -f ref=main" "$(calls)"
has "release dispatched" "actions/workflows/release.yml/dispatches -f ref=main" "$(calls)"
eq "next ticket" yes "$(next_dispatched)"
has "report on the PR" "AC-1: 충족" "$(jq -r '.[0].body' "$FAKE_GH/comments-40.json")"

setup merge-fails
touch "$FAKE_GH/fail-merge"
bash "$verify" pass 7 40 > /dev/null 2>&1
eq "exit" 1 "$?"
eq "ticket open" open "$(state_of issue-7)"
eq "next ticket" no "$(next_dispatched)"
lacks "no build" "ci.yml" "$(calls)"

# --- next-verify (#286) ------------------------------------------------------------------------

# waiting <pr> <ticket> [checked]: an open PR of the Implementer for the ticket
waiting() {
  local f="$FAKE_GH/pulls.json" b="Closes #$2"
  [ "${3:-yes}" = yes ] && b="$b

<!-- factory-checked abc123 -->"
  [ -f "$f" ] || echo '[]' > "$f"
  jq --argjson p "$1" --arg b "$b" '. + [{number: $p, state: "open", body: $b}]' "$f" | tr -d '\r' > "$f.new" \
    && mv "$f.new" "$f"
}
verifying() { # verifying <ticket> <pr> [status]: a Verify run, not completed
  local f="$FAKE_GH/runs-list.json"
  [ -f "$f" ] || echo '[]' > "$f"
  jq --arg t "verify #$1 (PR #$2)" --arg s "${3:-in_progress}" \
    '[{databaseId: (([.[].databaseId] | max // 1000) + 1), workflow: "factory-verify.yml", status: $s,
       event: "workflow_dispatch", displayTitle: $t}] + .' "$f" | tr -d '\r' > "$f.new" && mv "$f.new" "$f"
}
verify_dispatched() { grep '^workflow run factory-verify.yml' "$FAKE_GH/calls.log" | sed 's/.*issue=//' | tr '\n' ' ' | sed 's/ $//'; }

setup next-verify-oldest-waiting
issue 8 in-progress; issue 9 in-progress
waiting 45 9; waiting 41 8
bash "$verify" next-verify 40 > /dev/null
eq "exit" 0 "$?"
eq "the oldest PR" "8 -f pr=41" "$(verify_dispatched)"

setup next-verify-skips
issue 8 in-progress; issue 9 ready-for-agent; issue 10 in-progress
waiting 40 7; waiting 41 8 no; waiting 42 9; waiting 43 10
jq '.state = "closed"' "$FAKE_GH/issue-10.json" | tr -d '\r' > "$FAKE_GH/x" && mv "$FAKE_GH/x" "$FAKE_GH/issue-10.json"
bash "$verify" next-verify 40 > /dev/null
eq "this run's PR, unchecked, not in progress, closed ticket: none" "" "$(verify_dispatched)"

setup next-verify-another-run-going
waiting 41 7
verifying 9 45
bash "$verify" next-verify 40 > "$tmp/out"
eq "nothing while another Verify runs" "" "$(verify_dispatched)"
has "says so" "other Verify run" "$(cat "$tmp/out")"

setup next-verify-own-run-and-completed-runs-do-not-count
waiting 41 7
verifying 9 45 completed; verifying 7 40
GITHUB_RUN_ID=$(jq '.[0].databaseId' "$FAKE_GH/runs-list.json" | tr -d '\r') bash "$verify" next-verify 40 > /dev/null
eq "dispatched" "7 -f pr=41" "$(verify_dispatched)"

setup usage
bash "$verify" sendback 7 > /dev/null 2>&1
eq "usage exit" 2 "$?"

echo "verify.sh: $passes passed, $fails failed"
[ "$fails" -eq 0 ]
