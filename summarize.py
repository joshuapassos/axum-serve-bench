#!/usr/bin/env python3
"""Median per (scenario, build, mode, metric) from out/results.tsv, printed main vs fork."""
import collections, statistics, sys
path = sys.argv[1] if len(sys.argv) > 1 else "out/results.tsv"
rows = collections.defaultdict(list)
for line in open(path):
    sc, build, mode, rep, metric, value = line.rstrip("\n").split("\t")
    try:
        rows[(sc, metric, mode, build)].append(float(value))
    except ValueError:
        pass
seen = []
for k in rows:
    key = (k[0], k[1], k[2])
    if key not in seen:
        seen.append(key)
print(f"{'scenario':10} {'metric':26} {'mode':13} {'main(med)':>14} {'fork(med)':>14} {'fork/main':>9}  n")
for sc, metric, mode in seen:
    b = rows.get((sc, metric, mode, "main"), [])
    f = rows.get((sc, metric, mode, "fork"), [])
    mb = statistics.median(b) if b else float("nan")
    mf = statistics.median(f) if f else float("nan")
    ratio = f"{mf/mb:.3f}" if b and f and mb else "-"
    print(f"{sc:10} {metric:26} {mode:13} {mb:14.1f} {mf:14.1f} {ratio:>9}  {len(b)}/{len(f)}")
