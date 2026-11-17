#!/usr/bin/env bash
# Tests of the repository .gitattributes (#304): a text file written with CRLF line ends goes
# into the index with LF, whatever core.autocrlf is, and binary files keep their bytes.
set -u
here=$(cd "$(dirname "$0")" && pwd)
attributes="$here/../../../.gitattributes"

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

# committed_crs <repo> <path>: the number of CR bytes in the committed blob
committed_crs() {
  git -C "$1" cat-file blob "HEAD:$2" | tr -cd '\r' | wc -c | tr -d ' '
}

# committed_hash <repo> <path>, file_hash <file>: blob ids, to compare bytes
committed_hash() { git -C "$1" rev-parse "HEAD:$2"; }
file_hash() { git hash-object --no-filters "$1"; }

# setup <name> <core.autocrlf>: a fresh repository with this repository's .gitattributes
setup() {
  current=$1
  repo=$(mktemp -d "$root/r.XXXXXX")
  git -C "$repo" init -q
  git -C "$repo" config user.name test
  git -C "$repo" config user.email test@example.invalid
  git -C "$repo" config core.autocrlf "$2"
  git -C "$repo" config core.safecrlf false
  cp "$attributes" "$repo/.gitattributes"
}

commit_all() {
  git -C "$repo" add -A
  git -C "$repo" commit -q -m test
}

for autocrlf in false true input; do
  setup "crlf text files become lf (autocrlf=$autocrlf)" "$autocrlf"
  mkdir -p "$repo/crates/x/src" "$repo/docs"
  printf '# 제목\r\n\r\n본문\r\n' > "$repo/AGENTS.md"
  printf 'fn main() {\r\n}\r\n' > "$repo/crates/x/src/main.rs"
  printf '[package]\r\nname = "x"\r\n' > "$repo/crates/x/Cargo.toml"
  printf 'a: 1\r\n' > "$repo/docs/ci.yml"
  printf 'no extension\r\n' > "$repo/LICENSE"
  commit_all
  for f in AGENTS.md crates/x/src/main.rs crates/x/Cargo.toml docs/ci.yml LICENSE; do
    eq "$f CRs in the index" 0 "$(committed_crs "$repo" "$f")"
  done
  eq "AGENTS.md committed bytes" "$(printf '# 제목\n\n본문\n' | git hash-object --stdin)" \
    "$(committed_hash "$repo" AGENTS.md)"

  setup "binary files keep their bytes (autocrlf=$autocrlf)" "$autocrlf"
  mkdir -p "$repo/testdata/mft"
  printf 'FILE0\r\n\000\001\r\n\377' > "$repo/testdata/mft/x.mft"
  printf '\211PNG\r\n\032\n\000\000\000\rIHDR' > "$repo/icon.png"
  printf 'MZ\220\000\r\n\000\000' > "$repo/tool.exe"
  commit_all
  for f in testdata/mft/x.mft icon.png tool.exe; do
    eq "$f committed bytes" "$(file_hash "$repo/$f")" "$(committed_hash "$repo" "$f")"
  done
done

echo "gitattributes.sh: $passes passed, $fails failed"
[ "$fails" -eq 0 ]
