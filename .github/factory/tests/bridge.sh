#!/usr/bin/env bash
# Tests of .github/factory/bridge.sh (marker comments → Agent Mail) against the fake gh and agent-mail.
set -u
here=$(cd "$(dirname "$0")" && pwd)
script="$here/../bridge.sh"
export PATH="$here/fake:$PATH"

fails=0
passes=0
current=
root=$(mktemp -d)
trap 'rm -rf "$root"' EXIT

setup() {
  current=$1
  tmp=$(mktemp -d "$root/t.XXXXXX")
  export FAKE_GH="$tmp/gh" FAKE_MAIL="$tmp/mail" XDG_STATE_HOME="$tmp/state"
  export FACTORY_REPO=owner/repo BRIDGE_SINCE=2026-11-01T00:00:00Z
  unset BRIDGE_TO AGENT_MAIL_PROFILE
  mkdir -p "$FAKE_GH" "$FAKE_MAIL"
  echo '[]' > "$FAKE_GH/repo-comments.json"
}

ok() { passes=$((passes + 1)); }
bad() { echo "  FAIL [$current] $*"; fails=$((fails + 1)); }
eq() { # eq <what> <expected> <actual>
  if [ "$2" = "$3" ]; then ok; else bad "$1: expected [$2], got [$3]"; fi
}
has() { # has <what> <needle> <haystack>
  case "$3" in *"$2"*) ok ;; *) bad "$1: [$2] not in [$3]" ;; esac
}

# issue <n> <state> <labels comma-separated> <title>
issue() {
  jq -n --argjson n "$1" --arg s "$2" --arg l "$3" --arg t "$4" \
    '{number: $n, state: $s, title: $t, html_url: "https://github.com/owner/repo/issues/\($n)",
      labels: ($l | split(",") | map(select(. != "") | {name: .}))}' \
    | tr -d '\r' > "$FAKE_GH/issue-$1.json"
}

# comment <id> <issue> <body>
comment() {
  local f="$FAKE_GH/repo-comments.json"
  jq --argjson id "$1" --arg n "$2" --arg b "$3" \
    '. + [{id: $id, body: $b, created_at: "2026-11-17T03:00:00Z",
           issue_url: "https://api.github.com/repos/owner/repo/issues/\($n)",
           html_url: "https://github.com/owner/repo/issues/\($n)#issuecomment-\($id)"}]' "$f" \
    | tr -d '\r' > "$f.new" && mv "$f.new" "$f"
}

run() { bash "$script" "$@" > "$tmp/log" 2>&1; echo $? > "$tmp/code"; }
code() { cat "$tmp/code"; }
sent_count() { [ -f "$FAKE_MAIL/sent.jsonl" ] && wc -l < "$FAKE_MAIL/sent.jsonl" | tr -d ' ' || echo 0; }
# arg <send index from 0> <option>: the value after the option in that send
arg() {
  jq -s -r --argjson i "$1" --arg o "$2" '.[$i].args as $a | ($a | index($o)) as $k | $a[$k + 1]' \
    "$FAKE_MAIL/sent.jsonl" | tr -d '\r'
}
remembered() { tr '\n' ' ' < "$XDG_STATE_HOME/facet-factory-bridge/sent" | sed 's/ $//'; }

# A repository with one of each marker, plus noise.
markers() {
  issue 249 open spec,needs-decision "공장 개선"
  issue 250 open spec,factory-paused "검색"
  issue 900 open factory-ledger "장부"
  issue 301 open ready-for-human "티켓 하나"
  comment 11 249 "<!-- factory-question -->
무엇이 갈리는가: A냐 B냐. 권장안: A"
  comment 12 250 "<!-- factory-budget-spec -->
spec 상한: \$61 / \$60"
  comment 13 900 "<!-- factory-budget-daily 2026-11-17 -->
하루 상한"
  comment 14 301 "<!-- factory-human -->
되돌림 두 번"
  comment 15 249 "그냥 댓글"
}

# --- sending -------------------------------------------------------------------------------

setup "each marker is mailed once with its link, body and what to do"
markers
run
eq code 0 "$(code)"
eq sends 4 "$(sent_count)"
eq to "sandcorp/boss" "$(arg 0 --to)"
eq task facet-factory "$(arg 0 --task)"
eq key facet-factory-11 "$(arg 0 --idempotency-key)"
eq key4 facet-factory-14 "$(arg 3 --idempotency-key)"
eq profile automation "$(jq -s -r '.[0].profile' "$FAKE_MAIL/sent.jsonl" | tr -d '\r')"
has subject "질문: #249 공장 개선" "$(arg 0 --subject)"
body=$(arg 0 --body)
has "issue link" "https://github.com/owner/repo/issues/249" "$body"
has "comment link" "#issuecomment-11" "$body"
has "comment body" "A냐 B냐. 권장안: A" "$body"
has "question todo" "needs-decision 라벨을 떼면" "$body"
has "spec budget todo" "factory-paused" "$(arg 1 --body)"
has "daily todo" "KST 0시" "$(arg 2 --body)"
has "human reason" "되돌림 두 번" "$(arg 3 --body)"
has "human kind" "사람 넘김" "$(arg 3 --subject)"
eq remembered "11 12 13 14" "$(remembered)"

setup "a second run sends nothing again"
markers
run
run
eq code 0 "$(code)"
eq sends 4 "$(sent_count)"

setup "only the new marker is sent on the next run"
markers
run
comment 16 301 "<!-- factory-human -->
다시"
run
eq sends 5 "$(sent_count)"
eq key facet-factory-16 "$(arg 4 --idempotency-key)"

setup "BRIDGE_TO and AGENT_MAIL_PROFILE are passed through"
markers
export BRIDGE_TO=someone/else AGENT_MAIL_PROFILE=other
run
eq to someone/else "$(arg 0 --to)"
eq profile other "$(jq -s -r '.[0].profile' "$FAKE_MAIL/sent.jsonl" | tr -d '\r')"

# --- what is not sent ----------------------------------------------------------------------

setup "a question whose needs-decision is gone, and closed issues, are not sent"
issue 249 open spec "공장 개선"
issue 260 closed spec,needs-decision "닫힌 spec"
comment 11 249 "<!-- factory-question -->
A냐 B냐"
comment 12 260 "<!-- factory-question -->
닫힘"
run
eq code 0 "$(code)"
eq sends 0 "$(sent_count)"
eq remembered "" "$(remembered)"
issue 249 open spec,needs-decision "공장 개선"
run
eq "sent after the label is back" 1 "$(sent_count)"

# --- failures ------------------------------------------------------------------------------

setup "a failed send is not remembered and is retried next time"
markers
touch "$FAKE_MAIL/fail"
run
eq code 1 "$(code)"
eq sends 0 "$(sent_count)"
eq remembered "" "$(remembered)"
rm "$FAKE_MAIL/fail"
run
eq code 0 "$(code)"
eq sends 4 "$(sent_count)"

# --- seed ----------------------------------------------------------------------------------

setup "--seed remembers the markers without sending"
markers
run --seed
eq code 0 "$(code)"
eq sends 0 "$(sent_count)"
eq calls 0 "$([ -f "$FAKE_MAIL/calls.log" ] && wc -l < "$FAKE_MAIL/calls.log" | tr -d ' ' || echo 0)"
eq remembered "11 12 13 14" "$(remembered)"
comment 16 301 "<!-- factory-human -->
다시"
run
eq "only the new one after seeding" 1 "$(sent_count)"
eq key facet-factory-16 "$(arg 0 --idempotency-key)"

# --- the script itself ---------------------------------------------------------------------

current="no address, token, host or home path in the script"
eq "http" 0 "$(grep -c 'https\?://' "$script" || true)"
eq "home" 0 "$(grep -ci '/home/\|/Users/\|C:\\' "$script" || true)"
eq "token" 0 "$(grep -ci 'token' "$script" || true)"

echo "bridge.sh: $passes passed, $fails failed"
[ "$fails" -eq 0 ]
