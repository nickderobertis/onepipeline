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
  echo "ACTION: run 'github-store.sh fetch|save NAME ARCHIVE', NAME a human-items fixture's name" >&2
  exit 2
}

# Say what failed and what to do about it, and exit with `status` (1 where unstated).
fail() {
  echo "github-store: $1" >&2
  echo "ACTION: $2" >&2
  exit "${3:-1}"
}

[ "$#" -eq 3 ] || usage "takes a verb, a name and an archive"
verb="$1" name="$2" archive="$3"
# The names `tests/human_items/fixture.rs` gives: the family, the OS and arch, a 24-digit key.
[[ "$name" =~ ^human-items-fixture-Linux-[A-Za-z0-9_]+-[0-9a-f]{24}$ ]] \
  || usage "'$name' is not a human-items fixture's name"

case "$verb" in
  fetch)
    [ -n "${GITHUB_REPOSITORY:-}" ] \
      || fail "GITHUB_REPOSITORY names no repository" "set GITHUB_REPOSITORY to owner/repository"
    repo="$GITHUB_REPOSITORY"
    branch="${HUMAN_ITEMS_FIXTURE_BRANCH:-main}"
    listing="$(gh api "repos/$repo/actions/artifacts?name=$name&per_page=100")" \
      || fail "GitHub did not list the artifacts named $name" \
        "check that GH_TOKEN can read $repo's actions (actions: read), then re-run"
    # Every entry is checked before one is chosen: a field of another type is a listing this
    # cannot read, not an artifact to skip or take.
    id="$(jq -r --arg branch "$branch" '
        if (.artifacts | type) != "array"
          or any(.artifacts[]; (.id | type) != "number" or (.expired | type) != "boolean"
                 or (.created_at | type) != "string"
                 or (.workflow_run.head_branch | type) != "string")
        then error("an artifact entry is not the shape GitHub documents")
        else [.artifacts[] | select((.expired | not) and .workflow_run.head_branch == $branch)]
             | sort_by(.created_at) | last | .id // empty
        end' <<<"$listing")" \
      || fail "the artifact listing for $name is not the shape GitHub documents" \
        "read 'gh api repos/$repo/actions/artifacts?name=$name' and report what it answers; the fixture is generated meanwhile"
    if [ -z "$id" ]; then
      echo "github-store: no unexpired $name from a run on $branch"
      exit 3
    fi
    unpacked="$(mktemp -d)"
    trap 'rm -rf "$unpacked"' EXIT
    gh api "repos/$repo/actions/artifacts/$id/zip" >"$unpacked/artifact.zip" \
      || fail "GitHub did not serve artifact $id ($name)" \
        "check that artifact $id has not expired and GH_TOKEN can read $repo's actions, then re-run"
    unzip -q "$unpacked/artifact.zip" -d "$unpacked/files" \
      || fail "artifact $id ($name) is not a zip this can open" \
        "delete artifact $id with 'gh api -X DELETE repos/$repo/actions/artifacts/$id'; the next push to main saves it again"
    [ -f "$unpacked/files/$name.tar.gz" ] \
      || fail "artifact $id holds no $name.tar.gz" \
        "delete artifact $id with 'gh api -X DELETE repos/$repo/actions/artifacts/$id'; the next push to main saves it again"
    mv "$unpacked/files/$name.tar.gz" "$archive" \
      || fail "cannot write the restored fixture to $archive" "make $(dirname "$archive") writable, then re-run"
    echo "github-store: restored $name from artifact $id"
    ;;
  save)
    dir="${HUMAN_ITEMS_FIXTURE_SAVE_DIR:-}"
    if [ -z "$dir" ]; then
      echo "github-store: $name not saved: only the push-to-main timed job saves a fixture"
      exit 0
    fi
    { mkdir -p "$dir" && cp "$archive" "$dir/$name.tar.gz" && printf '%s\n' "$name" >"$dir/name"; } \
      || fail "cannot stage $archive in $dir" "make HUMAN_ITEMS_FIXTURE_SAVE_DIR ($dir) a writable directory and $archive readable, then re-run"
    echo "github-store: staged $name for this job's upload"
    ;;
  *) usage "'$verb' is not fetch or save" ;;
esac
