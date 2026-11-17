#!/usr/bin/env bash
# Tests of .github/factory/dispatch-next.sh: --candidates and the second look at an empty label search
# (#267), the spec queue and its bridge markers (#280). Which ticket goes first after Odin is pinned in
# odin.sh's tests.
set -u
here=$(cd "$(dirname "$0")" && pwd)
script="$here/../dispatch-next.sh"
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

# issue <n> <labels csv> [blocked_by]: a ticket of spec #50
issue() {
  jq -n --argjson n "$1" --arg l "$2" --argjson b "${3:-0}" \
    '{number: $n, state: "open", body: "## Parent\n\n#50\n",
      labels: ($l | split(",") | map(select(. != "")) | map({name: .})),
      issue_dependencies_summary: {blocked_by: $b}}' | tr -d '\r' > "$FAKE_GH/issue-$1.json"
}

setup() {
  current=$1
  tmp=$(mktemp -d "$root/t.XXXXXX")
  export FAKE_GH="$tmp/gh" FACTORY_LIST_RETRY_WAIT=0
  unset FACTORY_SPEC_QUEUE
  mkdir -p "$FAKE_GH"
  : > "$FAKE_GH/calls.log"
  jq -n '{number: 50, state: "open", body: "", labels: [{name: "spec"}]}' | tr -d '\r' \
    > "$FAKE_GH/issue-50.json"
}

run() { bash "$script" "$@" > "$tmp/log" 2>&1; echo $? > "$tmp/code"; }
code() { cat "$tmp/code"; }
log() { cat "$tmp/log"; }
dispatched() { grep '^workflow run factory-implementer.yml' "$FAKE_GH/calls.log" | sed 's/.*issue=//' | tr '\n' ' ' | sed 's/ $//'; }
searches() { grep -c '^issue list .*--label ready-for-agent' "$FAKE_GH/calls.log" || true; }

# --- candidates ----------------------------------------------------------------------------

setup "a candidate starts although the label search is empty"
issue 61 ready-for-agent 1
issue 62 ready-for-agent
touch "$FAKE_GH/list-empty"
run --tickets-only --candidates "61 62"
eq code 0 "$(code)"
eq "the unblocked candidate" 62 "$(dispatched)"
eq "no second search with candidates" 1 "$(searches)"

setup "candidates and the search together: the lowest number goes first"
issue 62 ready-for-agent
issue 65 ready-for-agent
run --candidates "65" --tickets-only
eq "lowest of both" 62 "$(dispatched)"

setup "a candidate without ready-for-agent or closed does not start"
issue 61 model:sonnet
issue 62 ready-for-agent
jq '.state = "closed"' "$FAKE_GH/issue-62.json" | tr -d '\r' > "$FAKE_GH/x" && mv "$FAKE_GH/x" "$FAKE_GH/issue-62.json"
touch "$FAKE_GH/list-empty"
run --tickets-only --candidates "61 62"
eq code 0 "$(code)"
eq "nothing dispatched" "" "$(dispatched)"
has "says so" "no unblocked approved ticket may start" "$(log)"

setup "a candidate of a paused spec does not start"
jq -n '{number: 50, state: "open", body: "", labels: [{name: "spec"}, {name: "factory-paused"}]}' \
  | tr -d '\r' > "$FAKE_GH/issue-50.json"
issue 62 ready-for-agent
touch "$FAKE_GH/list-empty"
run --tickets-only --candidates "62"
eq "guard still applies" "" "$(dispatched)"

# --- an empty search without candidates ----------------------------------------------------

setup "an empty search is asked once more"
issue 62 ready-for-agent
touch "$FAKE_GH/list-empty-once"
run --tickets-only
eq code 0 "$(code)"
eq "found on the second search" 62 "$(dispatched)"
eq "two searches" 2 "$(searches)"
has "notice" "asking again in 0s" "$(log)"

setup "a search that finds tickets is not repeated"
issue 62 ready-for-agent
run --tickets-only
eq "dispatched" 62 "$(dispatched)"
eq "one search" 1 "$(searches)"

setup "still empty after the second search: nothing starts"
touch "$FAKE_GH/list-empty"
run --tickets-only
eq code 0 "$(code)"
eq "nothing dispatched" "" "$(dispatched)"
eq "two searches" 2 "$(searches)"

# --- arguments -----------------------------------------------------------------------------

setup "bad arguments"
run --candidates
eq "missing value" 2 "$(code)"
run --candidates "61 x"
eq "not a number" 2 "$(code)"
run --other
eq "unknown option" 2 "$(code)"
lacks "nothing dispatched" "workflow run" "$(cat "$FAKE_GH/calls.log")"

# --- the spec queue and the bridge markers (#280) ------------------------------------------

# spec <n> <labels csv> [state]: a spec of the queue with two tickets, one merged
spec() {
  jq -n --argjson n "$1" --arg l "$2" --arg s "${3:-open}" \
    '{number: $n, state: $s, body: "",
      labels: ($l | split(",") | map(select(. != "")) | map({name: .})),
      sub_issues_summary: {total: 2, completed: 1}}' | tr -d '\r' > "$FAKE_GH/issue-$1.json"
}
ledger() {
  jq -n '{number: 900, state: "open", body: "", labels: [{name: "factory-ledger"}]}' | tr -d '\r' \
    > "$FAKE_GH/issue-900.json"
}
# marked <prefix>: the ledger comments starting with that text
marked() {
  [ -f "$FAKE_GH/comments-900.json" ] || { echo; return; }
  jq -r '.[].body | split("\n")[0]' "$FAKE_GH/comments-900.json" | tr -d '\r' | grep -F -- "$1" | tr '\n' ' ' \
    | sed 's/ $//'
}
decomposed() { grep '^workflow run factory-decompose.yml' "$FAKE_GH/calls.log" | sed 's/.*issue=//' | tr '\n' ' ' | sed 's/ $//'; }

setup "a spec that is not the last of the queue starts without a marker"
spec 50 spec; ledger; spec 60 ""; spec 61 ""
FACTORY_SPEC_QUEUE="60 61" run
eq code 0 "$(code)"
eq "decompose" 60 "$(decomposed)"
eq "labelled" true "$(jq 'any(.labels[]; .name == "spec")' "$FAKE_GH/issue-60.json" | tr -d '\r')"
eq "no marker" "" "$(marked '<!-- factory-')"

setup "the last spec of the queue starts: queue-empty once"
spec 50 spec; ledger; spec 60 spec; spec 61 ""; spec 62 "" closed
FACTORY_SPEC_QUEUE="60 61 62" run
eq code 0 "$(code)"
eq "decompose" 61 "$(decomposed)"
eq "queue-empty" "<!-- factory-queue-empty 61 -->" "$(marked '<!-- factory-')"
has "what to do" "FACTORY_SPEC_QUEUE" "$(jq -r '.[0].body' "$FAKE_GH/comments-900.json")"
# idle on the next call, once, then a new queue is told again
FACTORY_SPEC_QUEUE="60 61 62" run
eq code 0 "$(code)"
has "notice" "the factory is idle" "$(log)"
eq "idle" "<!-- factory-idle 60-61-62 -->" "$(marked '<!-- factory-idle')"
eq "one decompose" 61 "$(decomposed)"
FACTORY_SPEC_QUEUE="60 61 62" run
eq "not again" "<!-- factory-idle 60-61-62 -->" "$(marked '<!-- factory-idle')"
FACTORY_SPEC_QUEUE=" 60  61 " run
eq "a new queue" "<!-- factory-idle 60-61-62 --> <!-- factory-idle 60-61 -->" "$(marked '<!-- factory-idle')"
eq "queue-empty not again" "<!-- factory-queue-empty 61 -->" "$(marked '<!-- factory-queue-empty')"

setup "an empty queue is idle as none"
spec 50 spec; ledger
run
eq code 0 "$(code)"
eq "idle none" "<!-- factory-idle none -->" "$(marked '<!-- factory-')"

setup "a queue-empty marker already there is not left again"
spec 50 spec; ledger; spec 61 ""
jq -n '[{id: 1, created_at: "2026-11-17T01:00:00Z", body: "<!-- factory-queue-empty 61 -->\n앞서 남김"}]' \
  | tr -d '\r' > "$FAKE_GH/comments-900.json"
FACTORY_SPEC_QUEUE="61" run
eq "decompose" 61 "$(decomposed)"
eq "one marker" "<!-- factory-queue-empty 61 -->" "$(marked '<!-- factory-')"

setup "a failed comment does not change the exit code"
spec 50 spec; ledger; spec 61 ""
touch "$FAKE_GH/fail-comments"
FACTORY_SPEC_QUEUE="61" run
eq code 0 "$(code)"
eq "decompose" 61 "$(decomposed)"
has "warns" "could not leave <!-- factory-queue-empty 61 -->" "$(log)"
FACTORY_SPEC_QUEUE="61" run
eq "idle code" 0 "$(code)"
has "idle notice" "the factory is idle" "$(log)"
has "warns" "could not leave <!-- factory-idle 61 -->" "$(log)"
eq "no marker" "" "$(marked '<!-- factory-')"

setup "no ledger issue only warns"
spec 50 spec
run
eq code 0 "$(code)"
has "warns" "no open factory-ledger issue" "$(log)"

echo "dispatch-next.sh: $passes passed, $fails failed"
[ "$fails" -eq 0 ]
