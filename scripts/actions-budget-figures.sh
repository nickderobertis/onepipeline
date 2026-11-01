#!/usr/bin/env bash
# Copied from dero-skills' create-repo skill and edited only there, never in a copy.
# source: github.com/nickderobertis/dero-skills/skills/bootstrap/create-repo/assets/budgets/actions-budget-figures.sh
# version: 10
# sha256: 13ac7d79aaa3902dc7cfe87449283a05ba397fbc0612701e9b788ed59daf438d
#
# The GitHub Actions budget figures, measured from onevcs's read-only host answers
# and reported to `onebudgetspec check` (a `measure: reported` budget's command).
# onevcs selects the runs, jobs and pull requests; this script only applies the
# counting rules and the arithmetic. The contract is the "GitHub Actions budget
# figures" section of create-repo's references/tools/onebudgetspec.md, whose usage
# block the skill's tests hold to the lines below.
#
#   scripts/actions-budget-figures.sh longest-job --stage pr|cd [--release TAG] [--repo OWNER/NAME] [--now TIMESTAMP] [--exclude-workflow NAME]...
#   scripts/actions-budget-figures.sh job-count   --stage pr|cd [--release TAG] [--repo OWNER/NAME] [--now TIMESTAMP] [--exclude-workflow NAME]...
#   scripts/actions-budget-figures.sh release-cycle-time        [--release TAG] [--repo OWNER/NAME] [--now TIMESTAMP]
#   scripts/actions-budget-figures.sh release-read-requests     [--release TAG] [--repo OWNER/NAME] [--exclude-workflow NAME]...
#   scripts/actions-budget-figures.sh pr-read-requests          [--release TAG] [--repo OWNER/NAME]
#
# This script names no workflow of its own: which push workflows deliver nothing is
# each repository's to say, with --exclude-workflow in its CD budgets' commands. A
# run that concluded skipped ran nothing, whatever its workflow, so no CD figure
# counts it or reads its jobs.
#
# Needs bash, jq and `onevcs` (or the binary `ONEVCS` names). Exit 0 wrote the
# figure to ONEBUDGETSPEC_RESULT; 1 is missing data or a failed read, with the
# reason on stderr and nothing written; 2 is an invalid invocation.
set -euo pipefail

me=actions-budget-figures

usage() {
  [ -z "${1:-}" ] || printf '%s: %s\n' "$me" "$1" >&2
  sed -n 's/^#   \(scripts\/actions-budget-figures.sh \)/usage: \1/p' "$0" >&2
  exit 2
}
die() {
  printf '%s: %s\n' "$me" "$1" >&2
  exit 1
}
# What is missing ($1), then what to do about it ($2): a figure is never taken over
# what remained.
missing() {
  printf 'missing: %s\n  next: %s\n' "$1" "$2" >&2
  exit 1
}

case "${1:-}" in
  -h | --help) sed -n '7,/^set -euo/{/^#/s/^# \{0,1\}//p;}' "$0"; exit 0 ;;
esac
if [ -z "${ONEBUDGETSPEC_RESULT:-}" ]; then
  printf "%s: ONEBUDGETSPEC_RESULT is unset; run this as a budget's command under 'onebudgetspec check', which names the file the figure is written to\n" "$me" >&2
  exit 2
fi

figure="${1:-}"
case "$figure" in
  longest-job | job-count) takes="stage release repo now exclude-workflow" ;;
  release-cycle-time) takes="release repo now" ;;
  release-read-requests) takes="release repo exclude-workflow" ;;
  pr-read-requests) takes="release repo" ;;
  "") usage "name a figure" ;;
  *) usage "unknown figure: $figure" ;;
esac
shift
stage="" release="" repo="" now="" excludes=()
while [ $# -gt 0 ]; do
  case "$1" in
    --stage | --release | --repo | --now | --exclude-workflow) ;;
    *) usage "unknown argument: $1" ;;
  esac
  case " $takes " in
    *" ${1#--} "*) ;;
    *) usage "$figure takes no $1" ;;
  esac
  [ $# -ge 2 ] && [ -n "$2" ] || usage "$1 needs a value"
  case "$1" in
    --stage) stage=$2 ;;
    --release) release=$2 ;;
    --repo) repo=$2 ;;
    --now) now=$2 ;;
    --exclude-workflow) excludes+=("$2") ;;
  esac
  shift 2
done
case "$takes" in
  stage*) case "$stage" in pr | cd) ;; *) usage "$figure needs --stage pr or --stage cd" ;; esac ;;
esac
[ "$stage" != pr ] || [ "${#excludes[@]}" -eq 0 ] ||
  usage "--exclude-workflow applies to the CD figures only, not --stage pr"

command -v jq >/dev/null 2>&1 || die "jq is not on PATH: install jq (https://jqlang.org) where this figure is measured"
# The workflow names --exclude-workflow gave, as a JSON array. An empty array is
# never expanded: bash before 4.4 calls that an unbound variable under `set -u`.
excluded='[]'
[ "${#excludes[@]}" -eq 0 ] || excluded=$(jq -cn '$ARGS.positional' --args "${excludes[@]}")
onevcs=${ONEVCS:-onevcs}
command -v "$onevcs" >/dev/null 2>&1 ||
  die "onevcs is not on PATH (install a release carrying \`onevcs host\`, or name the binary in ONEVCS)"

[ -n "$now" ] || now=$(date -u +%Y-%m-%dT%H:%M:%SZ)
[[ "$now" =~ ^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}(\.[0-9]+)?Z$ ]] ||
  usage "--now must be an RFC 3339 UTC timestamp such as 2026-10-15T07:20:03Z, got: $now"

repo=${repo:-${GITHUB_REPOSITORY:-}}
[ -n "$repo" ] || die "no repository: pass --repo OWNER/NAME (GITHUB_REPOSITORY is unset)"
[[ "$repo" =~ ^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$ ]] || usage "--repo must be OWNER/NAME, got: $repo"

if [ -z "$release" ] && [ -n "${GITHUB_EVENT_PATH:-}" ] && [ -f "$GITHUB_EVENT_PATH" ]; then
  release=$(jq -r '.release.tag_name // empty' "$GITHUB_EVENT_PATH") ||
    die "the event payload $GITHUB_EVENT_PATH is not JSON"
fi
if [ -z "$release" ] && [ "${GITHUB_REF_TYPE:-}" = tag ]; then
  release=${GITHUB_REF_NAME:-}
fi
[ -n "$release" ] ||
  die "no release to measure: pass --release TAG (no release event payload and no tag ref in the environment)"

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

# Saved answers: one file per query and argument under RUNNER_TEMP, so every figure
# a job takes for one release asks onevcs each question once. Nothing is saved off
# a runner, and nothing is shared across jobs. The tag is percent-encoded, so two
# tags never share a directory (`release/v1` is `release%2Fv1`, `release_v1` stays).
# Encoding leaves `.` and `..` as they are, so those are refused rather than saved
# outside the release's own directory.
cache=""
if [ -n "${RUNNER_TEMP:-}" ]; then
  tag_dir=$(jq -rn --arg tag "$release" '$tag | @uri') || die "cannot encode the tag $release"
  [[ "$tag_dir" =~ ^[A-Za-z0-9._~%-]+$ ]] && [ "$tag_dir" != . ] && [ "$tag_dir" != .. ] ||
    die "the tag $release cannot name a saved-answers directory: pass the release's own tag with --release"
  cache="$RUNNER_TEMP/actions-budget-figures/${repo%%/*}__${repo#*/}/$tag_dir"
  mkdir -p "$cache"
fi

# fetch KEY QUERY ARG... prints the path of onevcs's answer to `host QUERY ARG...`,
# reading the saved answer when there is one and saving a fresh one whole before it
# is used. A failed read saves nothing and ends the script with onevcs's reason.
fetch() {
  local key=$1 query=$2 answer
  shift 2
  if [ -n "$cache" ] && [ -f "$cache/$key.json" ]; then
    printf '%s\n' "$cache/$key.json"
    return
  fi
  answer="$work/$key.json"
  if ! "$onevcs" host "$query" --repo "$repo" "$@" >"$work/answer" 2>"$work/stderr"; then
    die "onevcs host $query $* failed: $(tr -s '\n' ' ' <"$work/stderr")"
  fi
  jq -e '.schema_version == 1 and (.cost.requests | type) == "array"' "$work/answer" >/dev/null 2>&1 ||
    die "onevcs host $query $* answered no schema_version 1 document: install an onevcs release whose \`onevcs host\` answers schema_version 1, or name one in ONEVCS"
  mv "$work/answer" "$answer"
  if [ -n "$cache" ]; then
    cp "$answer" "$cache/.$key.$$" && mv "$cache/.$key.$$" "$cache/$key.json"
  fi
  printf '%s\n' "$answer"
}

safe() { printf '%s' "$1" | tr -c 'A-Za-z0-9._+-' '-'; }

# The selections every figure reads, as one JSON document on stdout:
#   {"blocks": [{"label", "number", "merged_at", "runs": [run + jobs]}],
#    "answers": [{"query", "cost"}]}
# with one unlabelled block for --stage cd and one per pull request for --stage pr.
release_changes() {
  fetch release-changes release-changes --tag "$release"
}

cd_selection() {
  local rc cr commit id payload_run seen ids=() answers=()
  rc=$(release_changes) || exit 1
  commit=$(jq -r .commit "$rc")
  [[ "$commit" =~ ^[0-9a-f]{40}$ ]] || die "onevcs host release-changes answered no commit SHA: $commit"
  cr=$(fetch "commit-runs-$commit-push-release" commit-runs --commit "$commit" --event push --event release) || exit 1
  [ "$(jq '.runs | length' "$cr")" -gt 0 ] ||
    missing "no push or release run at $release's commit $commit" \
      "check that --release names the tag this repository's CD built, and that its CD workflows run on push or release"
  # A run is excluded by its workflow name, so with --exclude-workflow given every run
  # must carry one: a run without it would be counted however it was named.
  [ "$excluded" = '[]' ] || jq -e '.runs | all(.workflow_name | type == "string" and . != "")' "$cr" >/dev/null ||
    die "onevcs host commit-runs answered a run with no workflow name, so --exclude-workflow cannot be applied: install an onevcs release whose \`onevcs host commit-runs\` names each run's workflow, or name one in ONEVCS"
  # A run is left out as skipped by its conclusion, so every run must carry one: a
  # string once it completed, null while it runs. A run without it would be read and
  # counted however it concluded.
  jq -e '.runs | all(has("conclusion") and (.conclusion | type == "string" or type == "null"))' "$cr" >/dev/null ||
    die "onevcs host commit-runs answered a run with no conclusion, so a run concluded skipped cannot be left out: install an onevcs release whose \`onevcs host commit-runs\` reports each run's conclusion, or name one in ONEVCS"
  # Delivery: every push or release run at the commit, except the runs of a workflow
  # --exclude-workflow names and the runs the listing reports concluded `skipped`
  # (every job skipped by its condition, so the run ran nothing). Neither counts
  # toward any CD figure and neither's jobs are read, so a skipped run costs no
  # release read (`seen` holds both, so neither GITHUB_RUN_ID nor a payload adds one
  # back). A run that concluded any other way, cancelled or failed among them, is
  # read and counted. The runs counted, then the run GITHUB_RUN_ID names, then the
  # one a workflow_run payload names, one per line.
  jq -r --argjson out "$excluded" '.runs[] | select((.workflow_name | IN($out[])) or .conclusion == "skipped" | not) | .id' "$cr" >"$work/runs" ||
    die "onevcs host commit-runs answered no run ids"
  seen=" $(jq -r --argjson out "$excluded" '[.runs[] | select((.workflow_name | IN($out[])) or .conclusion == "skipped") | .id] | join(" ")' "$cr") "
  [ -z "${GITHUB_RUN_ID:-}" ] || printf '%s\n' "$GITHUB_RUN_ID" >>"$work/runs"
  if [ -n "${GITHUB_EVENT_PATH:-}" ] && [ -f "$GITHUB_EVENT_PATH" ]; then
    payload_run=$(jq -r '.workflow_run.id // empty' "$GITHUB_EVENT_PATH") ||
      die "the event payload $GITHUB_EVENT_PATH is not JSON"
    [ -z "$payload_run" ] || printf '%s\n' "$payload_run" >>"$work/runs"
  fi
  # Ids to read: each once, never an excluded run's, however the environment names it.
  while read -r id; do
    [[ "$id" =~ ^[0-9]+$ ]] || die "a run id must be a number, got: $id"
    case "$seen" in *" $id "*) continue ;; esac
    seen="$seen$id "
    ids+=("$id")
  done <"$work/runs"
  # Judged after exclusion and deduplication, before any job is read.
  if [ "${#ids[@]}" -eq 0 ]; then
    jq -e '[.runs[] | select(.conclusion == "skipped")] | length > 0' "$cr" >/dev/null &&
      missing "every push or release run at $release's commit $commit concluded skipped or is a workflow --exclude-workflow names" \
        "check on GitHub that the workflows making and publishing $release ran at that commit rather than skipping every job, and name with --exclude-workflow only the push workflows that deliver nothing"
    missing "every push or release run at $release's commit $commit is a workflow --exclude-workflow names" \
      "name with --exclude-workflow only the push workflows that deliver nothing, never the ones that make or publish the release"
  fi
  for id in "${ids[@]}"; do
    answers+=("$(fetch "run-jobs-$id" run-jobs --run "$id")") || exit 1
  done
  jq -n --slurpfile rc "$rc" --slurpfile cr "$cr" --argjson out "$excluded" '
    [inputs] as $jobs
    | ($cr[0].runs | map({key: (.id | tostring), value: .}) | from_entries) as $at
    | {blocks: [{label: null, runs: [$jobs[] | ($at[.run_id | tostring] // {id: .run_id, workflow_name: null, event: null}) + {jobs}],
                 excluded: [$cr[0].runs[] | select(.workflow_name | IN($out[])) | {id, workflow_name, event}],
                 skipped: [$cr[0].runs[] | select((.workflow_name | IN($out[]) | not) and .conclusion == "skipped")
                           | {id, workflow_name, event}]}],
       answers: ([{query: "release-changes --tag \($rc[0].tag)", cost: $rc[0].cost},
                  {query: "commit-runs --commit \($cr[0].commit) --event push --event release", cost: $cr[0].cost}]
                 + [$jobs[] | {query: "run-jobs --run \(.run_id)", cost}])}' "${answers[@]}"
}

pr_selection() {
  local rc unmapped number head merged answers=()
  rc=$(release_changes) || exit 1
  unmapped=$(jq -r '[.commits[] | select(.change == null) | .sha] | join(", ")' "$rc")
  [ -z "$unmapped" ] ||
    missing "commit $unmapped of $release maps to no merged pull request (a rebase merge, or a push with no pull request)" \
      "land every change by squash or merge commit through a pull request, or give this repository no pull-request budget"
  [ "$(jq '.changes | length' "$rc")" -gt 0 ] ||
    missing "$release carries no pull request" "measure a release that merged at least one pull request"
  jq -r '.changes[] | "\(.number) \(.head_sha) \(.merged_at)"' "$rc" >"$work/changes" ||
    die "onevcs host release-changes answered no changes"
  while read -r number head merged; do
    [[ "$number" =~ ^[1-9][0-9]*$ && "$head" =~ ^[0-9a-f]{40}$ ]] ||
      die "onevcs host release-changes answered a change with no number or head SHA: $number $head"
    [ "$merged" != null ] ||
      missing "#$number has no merged_at" "check on GitHub that #$number was merged rather than closed"
    answers+=("$(fetch "change-checks-$number-$head-$(safe "$merged")" change-checks \
      --change "$number" --head "$head" --merged-at "$merged")") || exit 1
  done <"$work/changes"
  jq -n '[inputs] | {blocks: map({label: "#\(.change.number)", number: .change.number,
                                   head: .change.head_sha, merged_at: .change.merged_at, runs, cost}),
                     answers: map({query: "#\(.change.number)", cost})}' "${answers[@]}"
}

# What is missing from a selection, if anything: a pull request with no run, or a
# selected run whose job listing came back empty.
check_selection() {
  local gap
  gap=$(jq -r 'first(.blocks[] | . as $b
      | if .runs == [] then "\(.label) has no pull_request run at its head \(.head) created by its merge"
        else (.runs[] | select(.jobs == [])
              | "run \(.id)\(if .workflow_name then " (\(.workflow_name), \(.event))" else "" end)"
                + "\(if $b.label then " at \($b.label)\u0027s head" else "" end) lists no job")
        end) // empty' "$1")
  [ -z "$gap" ] || missing "$gap" \
    "check on GitHub that the pull_request workflows ran at that head and that the run was not deleted, then re-run this figure"
}

# shellcheck disable=SC2016 # a jq program: every $name in it is jq's, never the shell's
figures='
def ts: sub("\\.[0-9]+Z$"; "Z") | fromdate;
# A counted job: completed, not skipped, and taken by a runner; or the job running
# this script, still in progress, timed to --now. Its seconds are never below 0.
def counted:
  if .status == "completed" and .conclusion != "skipped" and .runner_name != null then
    . + {secs: ([(.completed_at | ts) - (.started_at | ts), 0] | max), end: (.completed_at | ts)}
  elif $self != "" and $runner != "" and (.run_id | tostring) == $self
       and .status == "in_progress" and .runner_name == $runner then
    . + {secs: ([$now - (.started_at | ts), 0] | max), end: $now}
  else empty end;
def os: .runner_os // "no runner OS";
def line: "\(.workflow_name) / \(.name): \(.secs)s";
def head: if .label then ["\(.label):"] else [] end;
def indent: if .label then "  " else "" end;
# A run --exclude-workflow left out, then a run left out as concluded skipped, one
# line each, after the counted jobs.
def left_out: indent as $i | [(.excluded // [])[] | "\($i)left out by --exclude-workflow: \(.workflow_name) (run \(.id), \(.event))"]
  + [(.skipped // [])[] | "\($i)left out as concluded skipped: \(.workflow_name) (run \(.id), \(.event))"];
def blocks: [.blocks[] | . + {all: [.runs[].jobs[]], jobs: ([.runs[].jobs[] | counted] | sort_by(-.secs))}];

def longest_job:
  blocks as $b
  | ([$b[].jobs[].secs] | max) as $value
  | if $value == null then {missing: "no counted job: every job the selection lists was skipped, or never taken by a runner"}
    else {value: $value, detail: ([$b[] | indent as $i | head + (if .jobs == [] then ["\($i)no counted job"]
          else [.jobs[] | $i + line] + ["\($i)longest per runner OS:"]
               + [.jobs | group_by(os)[] | max_by(.secs) | "\($i)  \(os): \(line)"] end) + left_out] | add | join("\n"))} end;

def job_count:
  blocks as $b
  | {value: ([$b[].jobs | length] | max),
     detail: ([$b[] | indent as $i | .jobs as $jobs
       | ["\(if .label then "\(.label): " else "" end)\($jobs | length) jobs", "\($i)per workflow:"]
         + [.all | group_by(.workflow_name)[] | .[0].workflow_name as $w
            | "\($i)  \($w): \([$jobs[] | select(.workflow_name == $w)] | length)"]
         + ["\($i)per runner OS:"] + [$jobs | group_by(os)[] | "\($i)  \(.[0] | os): \(length)"] + left_out] | add | join("\n"))};

def release_cycle_time:
  [blocks[] | . as $b
   | ([.runs[].created_at | ts] | min) as $first
   | ([.jobs[].end] | max) as $last
   | if $last == null then {missing: "\(.label): no counted job ends its checks"}
     else {label, checks: ($last - $first), tail: ($now - (.merged_at | ts))} end] as $rows
  | (first($rows[] | select(.missing)) // null) as $gap
  | if $gap then $gap
    else {value: ([$rows[] | .checks + .tail] | max),
          detail: ([$rows[] | "\(.label): checks \(.checks)s + merge to release \(.tail)s"] | join("\n"))} end;

def requests: [.cost.requests[] | "  \(.)"];
def release_read_requests:
  {value: ([.answers[].cost.rest_requests] | add),
   detail: ([.answers[] | ["\(.query): \(.cost.rest_requests) requests"] + requests] | add | join("\n"))};
def pr_read_requests:
  {value: ([.blocks[].cost.rest_requests] | max),
   detail: ([.blocks[] | ["\(.label): \(.cost.rest_requests) requests"] + requests] | add | join("\n"))};
'

case "$figure" in
  release-read-requests) cd_selection >"$work/selection" ;;
  pr-read-requests | release-cycle-time) pr_selection >"$work/selection" ;;
  *) "${stage}_selection" >"$work/selection" ;;
esac
check_selection "$work/selection"

now_epoch=$(jq -n --arg t "$now" '$t | sub("\\.[0-9]+Z$"; "Z") | fromdate')
jq --arg self "${GITHUB_RUN_ID:-}" --arg runner "${RUNNER_NAME:-}" --argjson now "$now_epoch" \
  "$figures ${figure//-/_}" "$work/selection" >"$work/figure"
gap=$(jq -r '.missing // empty' "$work/figure")
[ -z "$gap" ] || missing "$gap" \
  "check on GitHub why the selected runs' jobs were skipped or cancelled before a runner took them, then re-run this figure"

jq '{value, detail}' "$work/figure" >"$ONEBUDGETSPEC_RESULT"
# llmlint: ignore[tool_output_is_signal] the figures contract (references/tools/onebudgetspec.md) has a successful figure print its detail to stderr as well: `onebudgetspec check` prints only the figure, so this breakdown is what a CI log shows of which job or pull request a figure came from.
jq -r .detail "$work/figure" >&2
