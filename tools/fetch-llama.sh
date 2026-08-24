#!/usr/bin/env bash
set -euo pipefail

destination="${1:?destination required}"
commit="${2:?commit required}"
root="$(cd "$(dirname "$0")/../third_party" 2>/dev/null && pwd || true)"
mkdir -p "$(dirname "$destination")"
root="$(cd "$(dirname "$destination")" && pwd)"
resolved="$(cd "$(dirname "$destination")" && pwd)/$(basename "$destination")"

case "$resolved" in
  "$root"/*) ;;
  *) echo "Refusing to manage llama.cpp outside $root" >&2; exit 2 ;;
esac

if [[ -d "$destination/.git" ]] && [[ "$(git -C "$destination" rev-parse HEAD 2>/dev/null || true)" == "$commit" ]]; then
  echo "llama.cpp already pinned at $commit"
  exit 0
fi

if [[ -e "$destination" ]]; then
  # This exact target was constrained to the repository's third_party directory above.
  rm -rf -- "$destination"
fi
git init --quiet "$destination"
git -C "$destination" remote add origin https://github.com/ggml-org/llama.cpp.git
git -C "$destination" fetch --quiet --depth 1 origin "$commit"
git -C "$destination" checkout --quiet --detach FETCH_HEAD
actual="$(git -C "$destination" rev-parse HEAD)"
[[ "$actual" == "$commit" ]] || { echo "checkout mismatch: $actual" >&2; exit 3; }
echo "Fetched llama.cpp $actual"
