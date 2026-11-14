#!/usr/bin/env bash
# Demonstration only (spike-launch-inventory, never merged): run the e2e tier with the
# operating system recording every process start, into target/test-profile/os/ so the
# job's test-profile artifact carries it. Linux: `strace -f` of each e2e test process,
# through scripts/launch-os-trace.sh as cargo's target runner. Windows: an ETW session
# of Microsoft-Windows-Kernel-Process's process events around the whole tier, written
# out as one {pid, ppid, image} JSON line per start. Elsewhere the tier runs as it is.
#   bash scripts/launch-os-observe.sh -- <the tier's command>
set -uo pipefail
[ "${1:-}" = "--" ] && shift
root="$(pwd)"
out="$root/target/test-profile/os"
mkdir -p "$out"
case "$(uname -s)" in
  Linux)
    command -v strace >/dev/null || sudo apt-get install -y strace >/dev/null 2>&1 || true
    # The ci profile runs every test whatever another did: the journeys that run the
    # binary under strace themselves fail under this one (ptrace does not nest), and
    # the default profile's fail-fast would end the tier at the first of them.
    NEXTEST_PROFILE=ci CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUNNER="$root/scripts/launch-os-trace.sh" \
      ONEPIPELINE_OS_TRACE_DIR="$out" "$@"
    rc=$?
    exit "$rc"
    ;;
  MINGW* | MSYS* | CYGWIN*)
    etl="$(cygpath -w "$out/procs.etl")"
    logman start onepipeline-procs -p Microsoft-Windows-Kernel-Process 0x10 -o "$etl" -ets -bs 1024 -nb 64 512 >"$out/logman-start.txt" 2>&1
    echo "logman start: $?" >>"$out/logman-start.txt"
    "$@"
    rc=$?
    logman query onepipeline-procs -ets >"$out/logman-query.txt" 2>&1
    logman stop onepipeline-procs -ets >"$out/logman-stop.txt" 2>&1
    pwsh -NoProfile -Command "
      \$ErrorActionPreference = 'Stop'
      \$w = [System.IO.StreamWriter]::new('$(cygpath -w "$out/etw-starts.jsonl")')
      \$n = 0
      Get-WinEvent -Path '$etl' -Oldest -FilterXPath '*[System[EventID=1]]' | ForEach-Object {
        \$d = @{}; foreach (\$x in ([xml]\$_.ToXml()).Event.EventData.Data) { \$d[\$x.Name] = \$x.'#text' }
        \$w.WriteLine((@{ pid = [int]\$d.ProcessID; ppid = [int]\$d.ParentProcessID; image = \$d.ImageName; t = \$_.TimeCreated.ToUniversalTime().ToString('o') } | ConvertTo-Json -Compress))
        \$n++
      }
      \$w.Close()
      Write-Output \"etw starts: \$n\"
    " >"$out/etw-convert.txt" 2>&1
    rm -f "$out/procs.etl"
    exit "$rc"
    ;;
  *)
    exec "$@"
    ;;
esac
