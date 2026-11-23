#!/usr/bin/env bash
# The human-items fixture's store on GitHub Actions, which `tests/human_items/fixture.rs`
# runs as `HUMAN_ITEMS_FIXTURE_STORE`: workflow artifacts named
# `human-items-fixture-<OS>-<arch>-<key>`, saved only by ci.yml's push-to-main `timed` job.
#
#   github-store.sh fetch NAME ARCHIVE
#     Write the newest unexpired artifact NAME that a run on HUMAN_ITEMS_FIXTURE_BRANCH
#     (main where unset) uploaded to ARCHIVE. Exit 0 when restored, 3 when there is no such
#     artifact, and any other status when GitHub could not answer, which the fixture reads as
#     a reason to generate rather than as an absence.
#   github-store.sh save NAME ARCHIVE
#     Stage ARCHIVE in HUMAN_ITEMS_FIXTURE_SAVE_DIR, and NAME in its `name` file, for the
#     job's upload step. Where that directory is unset — every pull-request job — say so and
#     save nothing: a pull request never saves.
#
# Reads `gh` (with GH_TOKEN, which needs `actions: read`), `jq`, `unzip` and
# GITHUB_REPOSITORY.
set -euo pipefail

usage() {
  echo "github-store: $1" >&2
  echo "ACTION: run 'github-store.sh fetch|save NAME ARCHIVE'" >&2
  exit 2
}

[ "$#" -eq 3 ] || usage "takes a verb, a name and an archive"
verb="$1" name="$2" archive="$3"
case "$name" in
  human-items-fixture-*) ;;
  *) usage "'$name' is not a human-items fixture's name" ;;
esac

case "$verb" in
  fetch)
    repo="${GITHUB_REPOSITORY:?github-store: GITHUB_REPOSITORY names no repository}"
    branch="${HUMAN_ITEMS_FIXTURE_BRANCH:-main}"
    listing="$(gh api "repos/$repo/actions/artifacts?name=$name&per_page=100")" || {
      echo "github-store: GitHub did not list the artifacts named $name" >&2
      exit 1
    }
    id="$(jq -r --arg branch "$branch" '[.artifacts[]
        | select((.expired | not) and .workflow_run.head_branch == $branch)]
        | sort_by(.created_at) | last | .id // empty' <<<"$listing")" || {
      echo "github-store: the artifact listing for $name is not what GitHub answers" >&2
      exit 1
    }
    if [ -z "$id" ]; then
      echo "github-store: no unexpired $name from a run on $branch"
      exit 3
    fi
    unpacked="$(mktemp -d)"
    trap 'rm -rf "$unpacked"' EXIT
    gh api "repos/$repo/actions/artifacts/$id/zip" >"$unpacked/artifact.zip" || {
      echo "github-store: GitHub did not serve artifact $id ($name)" >&2
      exit 1
    }
    unzip -q "$unpacked/artifact.zip" -d "$unpacked/files"
    [ -f "$unpacked/files/$name.tar.gz" ] || {
      echo "github-store: artifact $id holds no $name.tar.gz" >&2
      exit 1
    }
    mv "$unpacked/files/$name.tar.gz" "$archive"
    echo "github-store: restored $name from artifact $id"
    ;;
  save)
    dir="${HUMAN_ITEMS_FIXTURE_SAVE_DIR:-}"
    if [ -z "$dir" ]; then
      echo "github-store: $name not saved: only the push-to-main timed job saves a fixture"
      exit 0
    fi
    mkdir -p "$dir"
    cp "$archive" "$dir/$name.tar.gz"
    printf '%s\n' "$name" >"$dir/name"
    echo "github-store: staged $name for this job's upload"
    ;;
  *) usage "'$verb' is not fetch or save" ;;
esac
