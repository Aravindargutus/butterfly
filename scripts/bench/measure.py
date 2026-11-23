#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
# Copyright (c) 2026 Dhakshnamoorthy Durgaraj
#
# Time one command, honestly.
#
#   measure.py <repeats> <command> [args...]   -> prints the best elapsed ms
#
# Three things this exists to get right, each of which the shell version got
# wrong (review of the bench harness):
#
# 1. The interpreter starts once, before the clock. Timing a command by
#    calling `python3 -c` for the start and end stamps put ~30-50ms of
#    interpreter startup inside every sample, which is most of the
#    measurement for anything that takes tens of milliseconds -- so the small
#    end of the scaling curve was measuring Python, not the program.
#
# 2. A command that fails is not a sample. Recording the time a process took
#    to reject its input as though it were the time to do the work produces a
#    number that looks like a fast run. This exits non-zero and says so.
#
# 3. Best of N, because timing noise only ever adds: a preemption or a page
#    fault can make a run slower, nothing can make it faster than the work
#    takes. The minimum is the honest estimate of the cost.

import subprocess
import sys
import time


def main(argv):
    if len(argv) < 3:
        sys.exit("usage: measure.py <repeats> <command> [args...]")
    try:
        repeats = int(argv[1])
    except ValueError:
        sys.exit("measure.py: <repeats> must be a number")
    if repeats < 1:
        sys.exit("measure.py: <repeats> must be at least 1")
    cmd = argv[2:]

    best = None
    for _ in range(repeats):
        start = time.perf_counter()
        try:
            p = subprocess.run(cmd, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
        except OSError as e:
            sys.stderr.write("measure.py: cannot run {}: {}\n".format(cmd[0], e))
            return 2
        elapsed = time.perf_counter() - start
        if p.returncode != 0:
            # Loudly, and with the program's own words: a failed command
            # timed as if it had succeeded is worse than no measurement.
            sys.stderr.write(
                "measure.py: {} exited {}\n{}".format(
                    " ".join(cmd), p.returncode, p.stderr.decode("utf-8", "replace")
                )
            )
            return 3
        if best is None or elapsed < best:
            best = elapsed
    print("{:.0f}".format(best * 1000))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
