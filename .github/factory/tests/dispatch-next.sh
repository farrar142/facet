#!/usr/bin/env bash
# Tests of .github/factory/dispatch-next.sh: --candidates and the second look at an empty label search
# (#267). Which ticket goes first after Odin is pinned in odin.sh's tests.
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

echo "dispatch-next.sh: $passes passed, $fails failed"
[ "$fails" -eq 0 ]
