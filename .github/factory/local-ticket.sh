#!/usr/bin/env bash
# A ticket that changes `.github/workflows/` is not for the factory (ADR-0001, "workflow 파일은 사람 몫"):
# the factory's GITHUB_TOKEN cannot push workflow files. A local worker takes it.
#
#   local-ticket.sh <issue>
#
# A ticket is local when it carries the `local` label or its title or body mentions `.github/workflows`.
# A local ticket gets `local` (adding a label creates it when missing) and, unless it is already with a
# person, is handed over by hand-to-human.sh (`ready-for-human`, no `ready-for-agent`, one comment
# `<!-- factory-human -->` whose next line is `<!-- factory-local -->`).
# Writes local=true|false to $GITHUB_OUTPUT (stdout when unset); exits 0 unless it cannot read the issue.
# decompose.sh (right after Decompose), odin.sh (before approving) and factory-implementer.yml (the
# `approved` job and the claim) call it, so a ticket that slipped past Decompose is turned around there.
#
# Environment: REPO, GH_TOKEN.
set -u
: "${REPO:?REPO is not set}"
n=${1:?usage: local-ticket.sh <issue>}
here=$(cd "$(dirname "$0")" && pwd)

output() {
  if [ -n "${GITHUB_OUTPUT:-}" ]; then printf '%s\n' "$@" >> "$GITHUB_OUTPUT"; else printf '%s\n' "$@"; fi
}

read -r mentions labels < <(gh api "repos/$REPO/issues/$n" --jq '[
  ((.title // "") + "\n" + (.body // "") | contains(".github/workflows") | tostring),
  ([.labels[].name] | join(","))] | @tsv' | tr -d '\r') \
  || { echo "::error::local-ticket: could not read #$n"; exit 1; }
labels=",${labels:-},"
case "$labels" in
  *,local,*) is_local=true ;;
  *) is_local=$mentions ;;
esac
if [ "$is_local" != true ]; then output local=false; exit 0; fi

case "$labels" in *,local,*) ;; *)
  gh api "repos/$REPO/issues/$n/labels" -f "labels[]=local" > /dev/null     || echo "::warning::local-ticket: could not add local to #$n" ;;
esac
# hand-to-human.sh takes ready-for-agent off and leaves the marker the bridge mails; once.
case "$labels" in *,ready-for-human,*) ;; *)
  bash "$here/hand-to-human.sh" "$n" '<!-- factory-local -->
workflow 파일 — 로컬 작업자 몫. 이 티켓은 `.github/workflows/` 를 건드리므로 공장(GITHUB_TOKEN)이 push 할 수 없다. `local` 로 공장 밖에 두고 사람(로컬 작업자)에게 넘긴다.'     || echo "::warning::local-ticket: could not hand #$n to a human" ;;
esac
output local=true
