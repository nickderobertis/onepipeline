#!/usr/bin/env python3
"""Prefix each line of stdin with its arrival time: wall-clock epoch seconds and the uptime clock."""
import sys
import time

for line in sys.stdin.buffer:
    with open("/proc/uptime") as fh:
        up = fh.read().split()[0]
    sys.stdout.buffer.write(f"{time.time():.3f} {up} ".encode() + line)
    sys.stdout.buffer.flush()
