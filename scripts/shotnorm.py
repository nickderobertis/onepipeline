#!/usr/bin/env python3
"""Normalisation: everything in a view that changes from one run to the next.

screencomp gates on the **hash** of each image, so a capture has to be
byte-identical on every machine and every runner. `onepipeline` renders live
facts about a live run, and nearly every view carries something that is true of
one run and of no other: the wall-clock timestamp on each event, the launching
session, the host name, the pid inside an agent stream id, the random session
token `onevcs` mints per publication, the absolute temporary paths the world
lives under, a node's age computed from the clock, the free space on this
machine's disk, and the live journal byte offset `monitor`'s resume line carries.

**This engine has no clock override, no deterministic-id switch, no `--no-color`
and no demo flag, and this capture adds none.** The CLI surface is held to the
approved `docs/contract.md`, and a flag added for a screenshot's convenience
would be a contract change rather than a capture decision — so the whole answer
is here, in the capture, exactly as `llmlint`'s capture rewrites its three
per-run paths.

Every rule below replaces a **per-run or per-host** value with a fixed one. None
of them changes what a verb prints: no line is added, dropped or reordered, and
no word a verb chose is rewritten into a different word.
"""

from __future__ import annotations

import re

#: The fixed instant the first normalised timestamp is rewritten to. Each later
#: distinct timestamp is one second past the one before, in order of first
#: appearance, so a stream of N events reads as an N-second run.
BASE_INSTANT = "2026-05-04T11:20:00"

#: The fixed free-space reading. This machine's disk is a per-host fact exactly
#: as its host name is, and the shot states one rather than photographing
#: whichever runner captured it.
FREE_SPACE = "184.2 GiB of 460.0 GiB (40% free)"

#: The fixed byte offset in `monitor`'s resume line. The real one is the end of
#: the last finished record in the journal, which moves with every value above.
CURSOR_BYTE = "48219"

#: The fixed age every live view's clock-derived duration is rewritten to.
RUNNING_FOR = "8s"

#: And the fixed age of the last thing a live dispatch did.
SINCE = "2s"


class Normaliser:
    """Per-run values replaced by fixed ones, each mapped in order of appearance.

    An **ordinal** map rather than a single placeholder, so two different
    sessions in one view stay two different sessions: the first token of a kind
    to appear becomes the first fixed token of that kind, the second the second,
    and a view that shows one session shows one.

    **One of these per scene**, never one shared across the capture. The map is
    what fixes the order the synthetic instants are handed out in, so a shared
    one would hand a later scene the numbers an earlier scene had already taken
    and leave a stream of events reading as though it ran backwards.
    """

    def __init__(self, paths: dict[str, str]) -> None:
        # Longest first, so `<world>/runs` is rewritten before `<world>`.
        self.paths = sorted(paths.items(), key=lambda pair: -len(pair[0]))
        self.seen: dict[str, dict[str, str]] = {}

    # -- ordinal maps -----------------------------------------------------
    def _ordinal(self, family: str, key: str, make) -> str:
        table = self.seen.setdefault(family, {})
        if key not in table:
            table[key] = make(len(table))
        return table[key]

    def text(self, raw: str) -> str:
        """Every rule, in the one order they compose in."""
        out = raw
        for real, fixed in self.paths:
            out = out.replace(real, fixed)
        out = self._instants(out)
        out = self._identifiers(out)
        out = re.sub(
            r"\d+(?:\.\d+)? [KMGT]iB of \d+(?:\.\d+)? [KMGT]iB \(\d+% free\)",
            FREE_SPACE,
            out,
        )
        # Both places a cursor is printed: `monitor`'s resume line and the
        # `return` line every `watch` ends on.
        out = re.sub(r"(cursor \d+:[^\s:]+:)\d+", r"\g<1>" + CURSOR_BYTE, out)
        out = re.sub(r"(running for )\d+[a-z](?:\d+[a-z])*", r"\g<1>" + RUNNING_FOR, out)
        out = re.sub(r"\b\d+[a-z](?:\d+[a-z])* ago\b", SINCE + " ago", out)
        return out

    def _instants(self, text: str) -> str:
        """Rewrite each instant by **position**, one second apart, in reading order.

        Positional rather than value-keyed on purpose. Two records a real run
        wrote inside the same millisecond carry the same instant, and whether
        any two of them do is a property of the machine that ran it — so a map
        keyed on the value hands out a different number of instants from one
        capture to the next and the whole tail of the view shifts. Keyed on
        position, a view of N instants always reads as an N-second stretch,
        whatever the host's clock did.
        """
        counter = [0]

        def fixed(_match: re.Match[str]) -> str:
            minute, second = divmod(counter[0], 60)
            counter[0] += 1
            base = BASE_INSTANT[:-5]  # "2026-05-04T11:"
            return f"{base}{20 + minute:02d}:{second:02d}.000Z"

        return re.sub(
            r"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d+)?Z", fixed, text
        )

    def _identifiers(self, text: str) -> str:
        # The agent stream id carries the double's pid; the `vcs` streams carry
        # the session token `onevcs` mints per publication; a harness session id
        # carries the pid too. Each becomes the nth fixed token of its kind.
        text = re.sub(
            r"fake-oneagentgraph-\d+",
            lambda m: self._ordinal(
                "agent", m.group(0), lambda nth: f"oneagentgraph-{nth + 1}"
            ),
            text,
        )
        text = re.sub(
            r"fake-harness-([a-z0-9._-]+?)-\d+-(\d+)",
            lambda m: self._ordinal(
                "harness",
                m.group(0),
                lambda nth, m=m: f"harness-{m.group(1)}-{nth + 1}-{m.group(2)}",
            ),
            text,
        )
        text = re.sub(
            r"\bs-[0-9a-f]{12}\b",
            lambda m: self._ordinal(
                "session", m.group(0), lambda nth: SESSION_TOKENS[nth % len(SESSION_TOKENS)]
            ),
            text,
        )
        # The driver's own stream is `<host>-<pid>`; the host is already fixed by
        # the world, so only the pid moves.
        text = re.sub(
            r"(?<=shots-host-)\d+",
            lambda m: self._ordinal("driver", m.group(0), lambda nth: f"{41000 + nth}"),
            text,
        )
        return text


#: Fixed session tokens, in the shape `onevcs` mints them. A capture that shows
#: more than these is one whose journey grew, and the modulo keeps it readable
#: rather than crashing on a shot nobody is looking at yet.
SESSION_TOKENS = [
    "s-4f21ac09d38b",
    "s-9c07be5412da",
    "s-27e6b0fa5c41",
    "s-b35d81c7e920",
    "s-6a1f42d0c8b7",
    "s-0e93cf17ab64",
    "s-58d2e6b09f3c",
    "s-c41a7fe28d05",
]


#: An event line of the two-column views: a normalised instant, the stream
#: column, and the rest. Anchored on the instant so the summary lines the
#: attached stream interleaves (`-- <run>  2/6 done  ACTIVE  waiting`) are left
#: exactly as the tool wrote them.
EVENT_LINE = re.compile(r"^(\d{4}-\d{2}-\d{2}T[\d:.]+Z)( {2,})(\S+)( +)(\S.*)$")


def realign(text: str) -> str:
    """Re-lay the two-column event views after their stream ids were normalised.

    `monitor`, `watch` and the attached `start` stream print
    `<instant>  <stream><padding> <rest>`, padding the stream column to a fixed
    width and letting an id wider than that column overflow with a single space.
    The real agent stream ids overflow (they carry a pid); the normalised ones do
    not, so a plain textual substitution would leave those lines short of the
    column every other line keeps.

    This restores the renderer's own rule — pad to the column, one space past it
    — reading the column's position off the lines of the same view that already
    sit on it, so nothing about the layout is invented here.
    """
    lines = text.splitlines(keepends=True)
    fields = [re.match(EVENT_LINE, line) for line in lines]
    # The column is read off the lines that are genuinely **padded** to it — the
    # ones whose gap is more than the single space an overflowing id leaves. An
    # overflowing line's own column is past the real one, so taking the maximum
    # over every line would move the whole view four characters right of where
    # the renderer puts it.
    padded = [
        len(m.group(1)) + len(m.group(2)) + len(m.group(3)) + len(m.group(4))
        for m in fields
        if m and len(m.group(4)) > 1
    ]
    if not padded:
        return text
    column = max(padded)
    out = []
    for line, match in zip(lines, fields):
        if not match:
            out.append(line)
            continue
        head = match.group(1) + match.group(2) + match.group(3)
        gap = max(1, column - len(head))
        ending = "\n" if line.endswith("\n") else ""
        out.append(head + " " * gap + match.group(5).rstrip("\n") + ending)
    return "".join(out)
