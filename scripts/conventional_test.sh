#!/usr/bin/env bash
# Tests for scripts/conventional.sh. Run: scripts/conventional_test.sh
set -uo pipefail
cd "$(dirname "$0")"
failed=0

check() { # name expected actual
  if [[ "$2" == "$3" ]]; then
    printf 'ok   %s\n' "$1"
  else
    printf 'FAIL %s\n  want: %q\n  got:  %q\n' "$1" "$2" "$3"
    failed=1
  fi
}

# NUL-separated messages, as `git log -z --format=%B` writes them.
msgs() { printf '%s\0' "$@"; }

title_ok() { ./conventional.sh title "$1" "${2:-}" 2>/dev/null && echo yes || echo no; }

check "feat title" yes "$(title_ok 'feat: watch list')"
check "scoped title" yes "$(title_ok 'fix(mcp): answer pings')"
check "breaking title" yes "$(title_ok 'feat(api)!: drop v0 routes')"
check "deps title" yes "$(title_ok 'ci(deps): bump the actions group')"
check "no type" no "$(title_ok 'Update island')"
check "unknown type" no "$(title_ok 'feature: x')"
check "capital type" no "$(title_ok 'Feat: x')"
check "no space" no "$(title_ok 'fix:x')"
check "empty description" no "$(title_ok 'fix: ')"
check "breaking body, no bang" no "$(title_ok 'feat: new ports' $'Moves things.\n\nBREAKING CHANGE: port 7878 is gone')"
check "breaking body, bang" yes "$(title_ok 'feat!: new ports' $'BREAKING CHANGE: port 7878 is gone')"
check "breaking body, scoped bang" yes "$(title_ok 'feat(api)!: new ports' $'x\nBREAKING-CHANGE: gone')"
check "breaking only in prose" yes "$(title_ok 'fix: y' 'this is not a BREAKING CHANGE: really')"

check "only chores" none "$(msgs 'chore: tidy' 'docs: readme' 'ci: cache' | ./conventional.sh bump)"
check "fix" patch "$(msgs 'docs: x' 'fix: y' | ./conventional.sh bump)"
check "perf" patch "$(msgs 'perf(cw): one call' | ./conventional.sh bump)"
check "feat beats fix" minor "$(msgs 'fix: y' 'feat: z' 'fix: w' | ./conventional.sh bump)"
check "bang" major "$(msgs 'feat: z' 'refactor!: rename config keys' | ./conventional.sh bump)"
check "footer" major "$(msgs $'fix: y\n\nBREAKING CHANGE: the port moved' | ./conventional.sh bump)"
check "footer, hyphen" major "$(msgs $'fix: y\n\nBREAKING-CHANGE: moved' | ./conventional.sh bump)"
check "breaking only in prose" patch "$(msgs $'fix: y\n\nnot a BREAKING CHANGE: really' | ./conventional.sh bump)"
check "non-conventional ignored" none "$(msgs 'Update island' 'Merge branch x' | ./conventional.sh bump)"
check "no messages" none "$(printf '' | ./conventional.sh bump)"

check "next patch" 0.2.1 "$(msgs 'fix: y' | ./conventional.sh next v0.2.0)"
check "next minor" 0.3.0 "$(msgs 'feat: y' | ./conventional.sh next 0.2.0)"
check "0.x breaking is minor" 0.3.0 "$(msgs 'feat!: y' | ./conventional.sh next 0.2.0)"
check "1.x breaking is major" 2.0.0 "$(msgs 'feat!: y' | ./conventional.sh next 1.4.2)"
check "minor resets patch" 1.5.0 "$(msgs 'feat: y' | ./conventional.sh next 1.4.2)"
check "nothing to release" "" "$(msgs 'docs: y' | ./conventional.sh next 0.2.0)"
check "bad version" 2 "$(msgs 'fix: y' | ./conventional.sh next banana 2>/dev/null; echo $?)"

notes=$(msgs 'feat(overlay): watch list' 'fix: tick count' $'refactor(api)!: rename routes\n\nBREAKING CHANGE: /v1/x is now /v2/x' 'chore: tidy' | ./conventional.sh notes)
expected='### ⚠️ Breaking changes

- **api:** rename routes

### Features

- **overlay:** watch list

### Fixes

- tick count'
check "notes" "$expected" "$notes"
check "notes, nothing" "" "$(msgs 'chore: x' | ./conventional.sh notes)"

exit $failed
