#!/usr/bin/env bash
# Tests of .github/factory/app-push.sh: the factory's pushes and PR writes as the factory App.
set -u
here=$(cd "$(dirname "$0")" && pwd)
script="$here/../app-push.sh"

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
  case "$3" in *"$2"*) bad "$1: [$2] in [$3]" ;; *) ok ;; esac
}

g() { git -c user.name=t -c user.email=t@example.invalid -c core.autocrlf=false "$@"; }

key=http.https://github.com/.extraheader
default_header='AUTHORIZATION: basic ZGVmYXVsdA=='
app_header="AUTHORIZATION: basic $(printf 'x-access-token:%s' app-secret | base64 | tr -d '\r\n')"

# setup <name>: a checkout (as actions/checkout leaves it: the default token's header in the local
# config) with a bare origin, and a fake gh that prints the token it got.
setup() {
  current=$1
  unset FACTORY_APP_TOKEN GIT_CONFIG_COUNT GIT_CONFIG_KEY_0 GIT_CONFIG_VALUE_0 GITHUB_ACTIONS GITHUB_SERVER_URL
  tmp=$(mktemp -d "$root/t.XXXXXX")
  g init -q --bare "$tmp/origin.git"
  g init -q -b main "$tmp/repo"
  (cd "$tmp/repo" && echo a > a && g add a && g commit -q -m init && g remote add origin "$tmp/origin.git" \
    && git config "$key" "$default_header")
  mkdir -p "$tmp/bin"
  printf '#!/usr/bin/env bash\necho "gh token=$GH_TOKEN args=$*"\n' > "$tmp/bin/gh"
  chmod +x "$tmp/bin/gh"
  export GH_TOKEN=default-token
}
run() { (cd "$tmp/repo" && PATH="$tmp/bin:$PATH" bash "$script" "$@" 2>&1); }
# The headers git sends for <key>: an empty value resets the list (git-config http.extraHeader).
sent() {
  run git-env git config --get-all "$1" \
    | awk '$0 == "" { n = 0; next } { l[n++] = $0 } END { for (i = 0; i < n; i++) print l[i] }'
}

# --- no App token: nothing runs, no fallback to the default token --------------------------------

setup no-token-push
out=$(run push origin main)
eq "exit" 3 "$?"
has "says why" "::error::no factory App token" "$out"
eq "nothing pushed" "" "$(g -C "$tmp/origin.git" branch --list main)"

setup no-token-gh
out=$(run gh pr merge 7)
eq "exit" 3 "$?"
lacks "gh not run" "gh token=" "$out"

setup no-token-git-env
out=$(run git-env echo ran)
eq "exit" 3 "$?"
lacks "command not run" "ran" "$out"

setup empty-token
export FACTORY_APP_TOKEN=
out=$(run check)
eq "exit" 3 "$?"

setup usage
out=$(run frobnicate)
eq "exit" 2 "$?"
out=$(run push)
eq "push without args" 2 "$?"

# --- with the App token ---------------------------------------------------------------------------

setup check
export FACTORY_APP_TOKEN=app-secret
out=$(run check)
eq "exit" 0 "$?"

setup git-env-replaces-the-header
export FACTORY_APP_TOKEN=app-secret
out=$(run git-env git config --get-all "$key")
eq "exit" 0 "$?"
eq "only the App header" "$app_header" "$(sent "$key")"
eq "local config untouched" "$default_header" "$(git -C "$tmp/repo" config --get-all "$key")"

setup git-env-keeps-gh-token
export FACTORY_APP_TOKEN=app-secret
out=$(run git-env bash -c 'echo "token=$GH_TOKEN"')
eq "GH_TOKEN untouched" "token=default-token" "$out"

setup git-env-adds-to-existing-env-config
export FACTORY_APP_TOKEN=app-secret GIT_CONFIG_COUNT=1 GIT_CONFIG_KEY_0=core.abbrev GIT_CONFIG_VALUE_0=9
out=$(run git-env git config core.abbrev)
eq "earlier env config kept" 9 "$out"
eq "only the App header" "$app_header" "$(sent "$key")"

setup git-env-other-server
export FACTORY_APP_TOKEN=app-secret GITHUB_SERVER_URL=https://ghe.example.invalid
eq "header for that server" "$app_header" "$(sent http.https://ghe.example.invalid/.extraheader)"

setup masks-in-actions
export FACTORY_APP_TOKEN=app-secret GITHUB_ACTIONS=true
out=$(run git-env true)
has "mask" "::add-mask::${app_header#AUTHORIZATION: basic }" "$out"

setup push
export FACTORY_APP_TOKEN=app-secret
out=$(run push -q origin main)
eq "exit" 0 "$?"
eq "pushed" "$(git -C "$tmp/repo" rev-parse main)" "$(g -C "$tmp/origin.git" rev-parse main)"

setup push-failure-passes-through
export FACTORY_APP_TOKEN=app-secret
out=$(run push -q origin no-such-branch)
code=$?
if [ "$code" -ne 0 ] && [ "$code" -ne 3 ]; then ok; else bad "exit: expected git's failure, got $code"; fi

setup gh-gets-the-app-token
export FACTORY_APP_TOKEN=app-secret
out=$(run gh pr merge 7 --squash)
eq "exit" 0 "$?"
eq "App token" "gh token=app-secret args=pr merge 7 --squash" "$out"

echo "app-push.sh: $passes passed, $fails failed"
[ "$fails" -eq 0 ]
