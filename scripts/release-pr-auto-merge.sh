#!/usr/bin/env bash
# Ask GitHub to merge release-plz's open release PR automatically, once the
# checks branch protection requires have passed.
#
# release-plz.yml's last step. Auto-merge is GitHub's own conditional merge: the
# request is accepted now and the merge happens only when every required check
# is green, so nothing here waits or merges. A refused request fails this step
# rather than warning, because a release PR left without auto-merge is a release
# nobody cuts.
#
# Only a PR whose head branch release-plz made (`release-plz-…`) on this
# repository is eligible; any other open PR is left alone, and two release PRs
# at once are refused rather than guessed between.
#
# Reads GITHUB_TOKEN, GITHUB_REPOSITORY, GITHUB_API_URL and GITHUB_GRAPHQL_URL,
# which every Actions job sets (the two URLs default to github.com's).
#
# Exits 0 with auto-merge requested or no release PR open; 1 when GitHub refused
# or could not be read, named on stderr.
set -euo pipefail

fail() {
  echo "::error::$1" >&2
  echo "ACTION: $2" >&2
  exit 1
}

for name in GITHUB_TOKEN GITHUB_REPOSITORY; do
  [ -n "${!name:-}" ] || fail "$name is not set" "run this as release-plz.yml does, with $name in the environment"
done
api="${GITHUB_API_URL:-https://api.github.com}"
graphql="${GITHUB_GRAPHQL_URL:-https://api.github.com/graphql}"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

# One request; prints the HTTP status, leaves the body in $work/body.
request() {
  curl -sS -o "$work/body" -w '%{http_code}' \
    -H "Authorization: Bearer $GITHUB_TOKEN" \
    -H "Accept: application/vnd.github+json" \
    -H "User-Agent: onepipeline-release-plz" "$@" \
    || fail "GitHub did not answer at $api" "re-run the job once GitHub answers"
}

status="$(request "$api/repos/$GITHUB_REPOSITORY/pulls?state=open&per_page=100")"
[ "$status" = 200 ] || fail "GitHub answered $status listing open pull requests: $(head -c 300 "$work/body")" \
  "check that RELEASE_PLZ_TOKEN can read pull requests, then re-run"

# Every pull request is checked for the fields read here, so a malformed answer
# is refused rather than mistaken for "no release PR".
jq -ce --arg repo "$GITHUB_REPOSITORY" '
  if type == "array" and all(.[]; (.number | type) == "number" and (.node_id | type) == "string"
                              and (.head.ref | type) == "string"
                              and ((.head.repo | type) == "null" or (.head.repo.full_name | type) == "string"))
  then [.[] | select((.head.ref | startswith("release-plz-")) and .head.repo.full_name? == $repo)
              | select(.node_id != "") | {number, node_id}]
  else error("not a pull request list") end' "$work/body" >"$work/release-prs" 2>/dev/null \
  || fail "GitHub's pull request list is not the JSON array of pull requests it documents" "re-run the job once GitHub answers normally"
count="$(jq length "$work/release-prs")"
case "$count" in
  0)
    echo "No open release PR to auto-merge."
    exit 0
    ;;
  1) ;;
  *)
    fail "$count open release PRs ($(jq -r 'map("#\(.number)") | join(", ")' "$work/release-prs")); release-plz keeps one" \
      "close every release PR but the newest, then re-run"
    ;;
esac
number="$(jq -r '.[0].number' "$work/release-prs")"
node="$(jq -r '.[0].node_id' "$work/release-prs")"

jq -n --arg id "$node" '{
  query: "mutation($id: ID!) { enablePullRequestAutoMerge(input: {pullRequestId: $id, mergeMethod: SQUASH}) { pullRequest { number autoMergeRequest { mergeMethod } } } }",
  variables: {id: $id}
}' >"$work/mutation"
status="$(request -X POST -H "Content-Type: application/json" --data-binary "@$work/mutation" "$graphql")"
if [ "$status" != 200 ] || ! jq -e --argjson number "$number" '(.errors | length // 0) == 0
      and .data.enablePullRequestAutoMerge.pullRequest.number == $number
      and .data.enablePullRequestAutoMerge.pullRequest.autoMergeRequest.mergeMethod == "SQUASH"' "$work/body" >/dev/null 2>&1; then
  fail "GitHub refused auto-merge on release PR #$number (HTTP $status): $(jq -r '[.errors[]?.message] | join("; ")' "$work/body" 2>/dev/null || head -c 300 "$work/body")" \
    "allow auto-merge in the repository settings and check RELEASE_PLZ_TOKEN can write pull requests, then re-run the job"
fi
echo "Auto-merge requested on release PR #$number: it squash-merges once its required checks pass."
