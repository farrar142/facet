#!/usr/bin/env bash
# The factory's writes that must wake other workflows go through the factory GitHub App's token
# instead of the default GITHUB_TOKEN: a push or a PR made or merged with GITHUB_TOKEN starts no
# workflow (no pull_request CI, no push CI on main). Only these writes use it: branch pushes,
# opening a PR and merging one. Labels, comments, `gh workflow run` and the ledger stay on GH_TOKEN.
#
#   app-push.sh check                  the App token is there (exit 0), else says so (exit 3)
#   app-push.sh push <git push args>   `git push <args>` authenticated as the App
#   app-push.sh git-env <command...>   run <command> with every git it starts authenticated as the
#                                      App (GH_TOKEN untouched), e.g. workflow-guard.sh push-retry,
#                                      which pushes again itself
#   app-push.sh gh <gh args>           `gh <args>` with GH_TOKEN set to the App token
#
# Without the App token nothing is run and the exit code is 3: never fall back to GITHUB_TOKEN, which
# would push or merge without starting CI and look like it worked. Otherwise the exit code is the
# command's.
#
# Git authentication: actions/checkout keeps GITHUB_TOKEN as an `http.<server>/.extraheader` in the
# local git config. The App's header is given through git's environment config (GIT_CONFIG_COUNT,
# like `git -c`, read after the local config): an empty value first resets the header list, so only
# the App's header is sent. Nothing is written to the git config, so later git commands and the agent
# steps keep GITHUB_TOKEN.
#
# Environment: FACTORY_APP_TOKEN (the App installation token, from actions/create-github-app-token);
# GITHUB_SERVER_URL (default https://github.com). Needs bash, git, base64 and gh.
set -u

usage() {
  echo 'usage: app-push.sh check | push <git push args...> | git-env <command...> | gh <gh args...>' >&2
  exit 2
}

check() {
  if [ -z "${FACTORY_APP_TOKEN:-}" ]; then
    echo "::error::no factory App token (FACTORY_APP_TOKEN): not pushing or merging with GITHUB_TOKEN instead, which would start no CI. Check the App installation and the FACTORY_APP_ID variable and FACTORY_APP_PRIVATE_KEY secret." >&2
    return 3
  fi
}

# Exports the git environment config that authenticates as the App.
git_env() {
  local server header i
  server=${GITHUB_SERVER_URL:-https://github.com}
  header="AUTHORIZATION: basic $(printf 'x-access-token:%s' "$FACTORY_APP_TOKEN" | base64 | tr -d '\r\n')"
  if [ -n "${GITHUB_ACTIONS:-}" ]; then echo "::add-mask::${header#AUTHORIZATION: basic }"; fi
  i=${GIT_CONFIG_COUNT:-0}
  export "GIT_CONFIG_KEY_$i=http.$server/.extraheader" "GIT_CONFIG_VALUE_$i="
  i=$((i + 1))
  export "GIT_CONFIG_KEY_$i=http.$server/.extraheader" "GIT_CONFIG_VALUE_$i=$header"
  export GIT_CONFIG_COUNT=$((i + 1))
}

cmd=${1:-}
[ $# -gt 0 ] && shift
case "$cmd" in
  check) [ $# -eq 0 ] || usage; check ;;
  push) [ $# -gt 0 ] || usage; check || exit; git_env; git push "$@" ;;
  git-env) [ $# -gt 0 ] || usage; check || exit; git_env; "$@" ;;
  gh) [ $# -gt 0 ] || usage; check || exit; GH_TOKEN=$FACTORY_APP_TOKEN gh "$@" ;;
  *) usage ;;
esac
