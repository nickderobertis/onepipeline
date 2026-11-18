# Sourced by scripts/nx.sh, from the repository root, only while `just gate-profile` runs (it
# sets ONEPIPELINE_GATE_PROFILE_DIR); it may rewrite nx.sh's arguments.
#
# At the run's own level, a task run gets its own Nx task profile (NX_PROFILE, one file per
# invocation, since each would overwrite one shared file), the remote cache is skipped, and the
# local cache and the workspace data that indexes it are this run's own, fresh: every task does
# its work once, and a later invocation in the same run replays what an earlier one produced,
# as the unprofiled gate would, rather than doing it twice. Inside one of the run's Nx tasks, an
# Nx run is that task's own work — a suite driving Nx over a scratch workspace — so every setting
# above is taken back off it.
. "$ROOT/gate-profile/lib.sh"
gate_profile_check_dir || exit
if ! gate_profile_at_top; then
  for gate_profile_setting in NX_PROFILE NX_CACHE_DIRECTORY NX_WORKSPACE_DATA_DIRECTORY; do
    case "${!gate_profile_setting-}" in
      "$ONEPIPELINE_GATE_PROFILE_DIR"/* | "${ONEPIPELINE_GATE_PROFILE_DIR#"$ROOT"/}"/*) unset "$gate_profile_setting" ;;
    esac
  done
  unset ONEPIPELINE_GATE_PROFILE_DIR ONEPIPELINE_GATE_PROFILE_INVOCATION gate_profile_setting
elif [ "${1-}" = run-many ] || [ "${1-}" = run ] || [ "${1-}" = affected ]; then
  case "$ONEPIPELINE_GATE_PROFILE_DIR" in
    "$ROOT"/*) ;;
    *)
      echo "gate-profile: the run directory $ONEPIPELINE_GATE_PROFILE_DIR is outside $ROOT, where Nx writes its task profile; run \`just gate-profile\` from this checkout" >&2
      exit 2
      ;;
  esac
  gate_profile_state="$ONEPIPELINE_GATE_PROFILE_DIR/nx-state"
  if [ ! -d "$gate_profile_state/data" ]; then
    # The project graph Nx already computed is kept, so the first invocation does not pay to
    # compute it again; the databases that say which task results are cached are not.
    mkdir -p "$gate_profile_state/data" "$gate_profile_state/cache" &&
      if [ -d .nx/workspace-data ]; then
        find .nx/workspace-data -maxdepth 1 -type f -name '*.json' -exec cp {} "$gate_profile_state/data/" \;
      fi ||
      gate_profile_unrecorded "could not set up this run's own Nx cache in $gate_profile_state (see above); make the run directory writable and profile again"
  fi
  ONEPIPELINE_GATE_PROFILE_INVOCATION="${EPOCHREALTIME/./}-$$"
  gate_profile_invocation="$ONEPIPELINE_GATE_PROFILE_DIR/nx/$ONEPIPELINE_GATE_PROFILE_INVOCATION"
  { mkdir -p "$ONEPIPELINE_GATE_PROFILE_DIR/nx" &&
    printf '{"id":"%s","phase":%s,"args":%s}\n' "$ONEPIPELINE_GATE_PROFILE_INVOCATION" \
      "$(gate_profile_json_string "${ONEPIPELINE_GATE_PROFILE_PHASE-}")" \
      "$(gate_profile_json_array "$@")" >"$gate_profile_invocation.invocation.json"; } ||
    gate_profile_unrecorded "could not record \`nx $*\` in $gate_profile_invocation.invocation.json (see above); make the run directory writable and profile again"
  # Nx joins NX_PROFILE onto its working directory, so the path is given relative to the root.
  export NX_PROFILE="${gate_profile_invocation#"$ROOT"/}.profile.json"
  export NX_CACHE_DIRECTORY="$gate_profile_state/cache"
  export NX_WORKSPACE_DATA_DIRECTORY="$gate_profile_state/data"
  export ONEPIPELINE_GATE_PROFILE_INVOCATION
  # nx-linger.cjs says why Nx outlives its last task by a few samples, and restores this.
  export ONEPIPELINE_GATE_PROFILE_NODE_OPTIONS="${NODE_OPTIONS-}"
  export NODE_OPTIONS="--require \"$ROOT/gate-profile/nx-linger.cjs\"${NODE_OPTIONS:+ $NODE_OPTIONS}"
  set -- "$@" --skip-remote-cache
  unset gate_profile_state gate_profile_invocation
fi
