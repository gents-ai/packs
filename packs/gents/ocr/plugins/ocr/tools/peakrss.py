#!/usr/bin/env python3
"""Runs a command and writes its peak resident memory, in MiB, to a file.

Usage: peakrss.py <out file> <command> [args...]

The command's own stdin, stdout and stderr and its exit status pass through.
The peak is the kernel's maximum resident set size of that process, which is
bytes on macOS and KiB on Linux, so it needs no time(1) flag that differs
between the two.
"""
import os
import subprocess
import sys


def main() -> int:
    out, command = sys.argv[1], sys.argv[2:]
    if not command:
        print(__doc__, file=sys.stderr)
        return 2
    child = subprocess.Popen(command)
    _, status, usage = os.wait4(child.pid, 0)
    unit = 1 if sys.platform == "darwin" else 1024
    with open(out, "w") as file:
        file.write(f"{usage.ru_maxrss * unit / 1048576:.0f}")
    return os.waitstatus_to_exitcode(status)


sys.exit(main())
