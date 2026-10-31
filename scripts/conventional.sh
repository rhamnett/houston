#!/usr/bin/env bash
# Conventional Commits -> semantic versions. Commit messages come in on stdin,
# NUL-separated (`git log -z --format=%B`); nothing here touches git or the
# network, so scripts/conventional_test.sh can check it.
#
#   conventional.sh title "<title>" [body]  exit 1 unless it's a conventional
#                                         header, with `!` if the body has a
#                                         BREAKING CHANGE footer
#   conventional.sh bump                 major | minor | patch | none
#   conventional.sh next <version>       the next version, or nothing
#   conventional.sh notes                release notes in Markdown
#
# fix/perf/revert bump the patch, feat the minor, and `!` or a BREAKING CHANGE
# footer the major (the minor while still 0.x). Other types release nothing.
set -euo pipefail

TYPES='feat|fix|perf|refactor|docs|test|build|ci|chore|style|revert'
HEADER="^(${TYPES})(\([a-z0-9][a-z0-9._/-]*\))?(!)?: [^ ].*"

# Each message as one line, its newlines escaped, so loops stay simple.
messages() {
  while IFS= read -r -d '' msg || [[ -n "$msg" ]]; do
    [[ -n "${msg//[[:space:]]/}" ]] || continue
    printf '%s\n' "${msg//$'\n'/\\n}"
  done
}

kind() { # one escaped message -> breaking | feat | fix | other | none
  local subject=${1%%\\n*}
  [[ "$subject" =~ $HEADER ]] || { echo none; return; }
  local type=${BASH_REMATCH[1]}
  if [[ -n "${BASH_REMATCH[3]}" || "$1" =~ (^|\\n)BREAKING[\ -]CHANGE: ]]; then
    echo breaking
  else
    case $type in
      feat) echo feat ;;
      fix | perf | revert) echo fix ;;
      *) echo other ;;
    esac
  fi
}

bump() {
  local best=none k
  while IFS= read -r line; do
    k=$(kind "$line")
    case $k in
      breaking) best=major ;;
      feat) [[ $best == major ]] || best=minor ;;
      fix) [[ $best == none ]] && best=patch ;;
    esac
  done < <(messages)
  echo "$best"
}

next() {
  local current=${1#v} kind major minor patch
  [[ "$current" =~ ^([0-9]+)\.([0-9]+)\.([0-9]+)$ ]] || {
    echo "not a version: $1" >&2
    exit 2
  }
  major=${BASH_REMATCH[1]} minor=${BASH_REMATCH[2]} patch=${BASH_REMATCH[3]}
  kind=$(bump)
  # Before 1.0 a breaking change bumps the minor: 0.x makes no promises.
  [[ $kind == major && $major == 0 ]] && kind=minor
  case $kind in
    major) echo "$((major + 1)).0.0" ;;
    minor) echo "$major.$((minor + 1)).0" ;;
    patch) echo "$major.$minor.$((patch + 1))" ;;
    none) ;;
  esac
}

notes() {
  local breaking=() feats=() fixes=() line subject k
  while IFS= read -r line; do
    subject=${line%%\\n*}
    k=$(kind "$line")
    # "feat(api)!: x" -> "**api:** x"
    [[ "$subject" =~ $HEADER ]] || continue
    local scope=${BASH_REMATCH[2]} text=${subject#*: }
    scope=${scope#(}
    scope=${scope%)}
    [[ -n "$scope" ]] && text="**$scope:** $text"
    case $k in
      breaking) breaking+=("$text") ;;
      feat) feats+=("$text") ;;
      fix) fixes+=("$text") ;;
    esac
  done < <(messages)
  section() {
    local title=$1
    shift
    (($#)) || return 0
    printf '### %s\n\n' "$title"
    printf -- '- %s\n' "$@"
    printf '\n'
  }
  section "⚠️ Breaking changes" ${breaking[@]+"${breaking[@]}"}
  section "Features" ${feats[@]+"${feats[@]}"}
  section "Fixes" ${fixes[@]+"${fixes[@]}"}
}

case ${1:-} in
  title)
    if ! [[ "${2:-}" =~ $HEADER ]]; then
      echo "Not a Conventional Commit header: ${2:-}" >&2
      echo "Use <type>[(scope)][!]: <description>, type one of ${TYPES//|/, }." >&2
      exit 1
    fi
    # The title alone must say it's breaking: a footer in the description only
    # reaches main if the squash message keeps it.
    if [[ -z "${BASH_REMATCH[3]}" && $'\n'"${3:-}" =~ $'\n'BREAKING[\ -]CHANGE: ]]; then
      echo "The description has a BREAKING CHANGE footer, so the title needs a !: ${2}" >&2
      echo "e.g. ${2/: /!: }" >&2
      exit 1
    fi
    ;;
  bump) bump ;;
  next) next "${2:?version}" ;;
  notes) notes ;;
  *)
    sed -n '2,12p' "$0"
    exit 2
    ;;
esac
