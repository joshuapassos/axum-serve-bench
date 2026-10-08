#!/usr/bin/env python3
"""Median per (scenario, metric, mode) and build from out/results.tsv."""
import collections, statistics, sys
path = sys.argv[1] if len(sys.argv) > 1 else "out/results.tsv"
ORDER = ["main", "3915", "fork", "both"]
rows = collections.defaultdict(list)
builds = set()
for line in open(path):
    sc, build, mode, rep, metric, value = line.rstrip("\n").split("\t")
    try:
        rows[(sc, metric, mode, build)].append(float(value))
        builds.add(build)
    except ValueError:
        pass
builds = [b for b in ORDER if b in builds] + sorted(builds - set(ORDER))
seen = list(dict.fromkeys(k[:3] for k in rows))
print(f"{'scenario':20} {'metric':26} {'mode':13}" + "".join(f"{b:>12}" for b in builds))
for sc, metric, mode in seen:
    cells = []
    for b in builds:
        v = rows.get((sc, metric, mode, b))
        cells.append(f"{statistics.median(v):12.1f}" if v else f"{'-':>12}")
    print(f"{sc:20} {metric:26} {mode:13}" + "".join(cells))
