#!/usr/bin/env python3

import argparse
from contextlib import closing
import json
from pathlib import Path
import sqlite3
import statistics
import subprocess
import time


def process_sample(pid):
    fields = subprocess.check_output(
        ["/bin/ps", "-p", str(pid), "-o", "lstart=", "-o", "time=", "-o", "rss="],
        text=True,
    ).split()
    started = " ".join(fields[:-2])
    cpu_seconds = 0.0
    for field in fields[-2].split(":"):
        cpu_seconds = 60 * cpu_seconds + float(field)
    return started, cpu_seconds, int(fields[-1]) * 1024


def database_sample(path):
    if path is None:
        return None
    try:
        size = path.stat().st_size
    except FileNotFoundError:
        return {"bytes": 0, "samples": 0, "exists": False}
    with closing(sqlite3.connect(path.resolve().as_uri() + "?mode=ro", uri=True, timeout=1)) as database:
        count, = database.execute("SELECT count(*) FROM metric_samples").fetchone()
    return {"bytes": size, "samples": count, "exists": True}


def positive(value):
    number = int(value)
    if number <= 0:
        raise argparse.ArgumentTypeError("must be a positive integer")
    return number


def main():
    parser = argparse.ArgumentParser(description="Measure one macOS app process and optional history database.")
    parser.add_argument("pid", type=positive)
    parser.add_argument("--label", required=True)
    parser.add_argument("--seconds", type=positive, default=60)
    parser.add_argument("--database", type=Path)
    args = parser.parse_args()

    try:
        database_before = database_sample(args.database)
        started, cpu_start, rss = process_sample(args.pid)
        start = time.monotonic()
        memory = [rss]
        deadline = start + args.seconds
        while True:
            time.sleep(max(0, min(2, deadline - time.monotonic())))
            identity, cpu_end, rss = process_sample(args.pid)
            if identity != started or cpu_end < cpu_start:
                parser.exit(1, "The app process changed during measurement.\n")
            memory.append(rss)
            elapsed = time.monotonic() - start
            if elapsed >= args.seconds:
                break
        database_after = database_sample(args.database)
    except (OSError, ValueError, IndexError, subprocess.CalledProcessError, sqlite3.Error) as error:
        parser.exit(1, f"Measurement failed: {error}\n")

    print(json.dumps({
        "label": args.label,
        "pid": args.pid,
        "elapsed_seconds": round(elapsed, 2),
        "mean_cpu_percent_of_one_core": round(100 * (cpu_end - cpu_start) / elapsed, 2),
        "median_rss_mib": round(statistics.median(memory) / 2**20, 2),
        "peak_sampled_rss_mib": round(max(memory) / 2**20, 2),
        "database_before": database_before,
        "database_after": database_after,
    }, indent=2))


if __name__ == "__main__":
    main()
