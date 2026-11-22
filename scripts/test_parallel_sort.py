#!/usr/bin/env python3
"""Compare baseline and parallel range-sort on >100k valid synthetic events.

The row count intentionally activates the parallel branch. This test compares
exact resulting Parquet data, not just a checksum; it is NOT a timing benchmark.
"""
import json
import os
import re
import statistics
import time
from pathlib import Path
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[1]
RUNNER = ROOT / "scripts" / "docker_run_windows.py"
COMPARE = ROOT / "scripts" / "compare.py"
COUNT = int(os.environ.get("SORT_TEST_EVENTS", "200000"))


def main():
    with tempfile.TemporaryDirectory(prefix="sort-architecture-") as dirname:
        root = Path(dirname)
        data = root / "data"
        for folder in ("events", "clicks", "links"):
            (data / folder).mkdir(parents=True)
        (data / "manifest.json").write_text(json.dumps({
            "window_start": "2026-10-01T00:00:00Z",
            "window_end": "2026-10-15T00:00:00Z",
        }) + "\n")
        (data / "products.csv").write_text("product_id,category\np1,books\np2,shoes\n")
        (data / "campaigns.csv").write_text("campaign_id,channel\nc1,search\nc2,social\n")
        (data / "links" / "part-0.csv").write_text("user_id_a,user_id_b\nu0001,u0002\n")
        (data / "clicks" / "part-0.csv").write_text(
            "click_id,user_id,campaign_id,ts\n" +
            "".join(
                f"k{i},u{i % 2500:04d},{'c1' if i % 2 else 'c2'},2026-10-03T09:00:00Z\n"
                for i in range(5000)
            )
        )

        # Writing in scrambled time/user order exposes improper concatenation.
        base = 1790985600000  # inside manifest; date convention only matters for comparison
        with (data / "events" / "part-0.csv").open("w", encoding="utf-8", newline="\n") as out:
            out.write("event_id,user_id,ts,ingest_ts,event_type,product_id,price_cents\n")
            for i in range(COUNT):
                ts = base + ((i * 1237) % 1200) * 60_000
                kind = "purchase" if i % 7 == 0 else "view"
                price = "500" if kind == "purchase" else ""
                duplicate_test = os.environ.get("SORT_TEST_DUPLICATES") == "1"
                distinct = COUNT - max(1, COUNT // 10)
                duplicate = duplicate_test and i >= distinct
                event_id = i - distinct if duplicate else i
                ingest = ts + 1000
                if duplicate:
                    # These far-apart duplicates stress global winner
                    # reconciliation. Many late rows win by earlier ingest.
                    ingest = base + (event_id % 2) * 1000
                    if event_id % 11 == 0:
                        kind = "invalid_event_kind"
                out.write(
                    f"e{event_id},u{(i * 43) % 2500:04d},{ts},{ingest},"
                    f"{kind},{'p1' if i % 2 else 'p2'},{price}\n"
                )

        trials = max(1, int(os.environ.get("SORT_TEST_TRIALS", "1")))
        measurements = {"baseline": [], "candidate": []}
        for trial in range(trials):
            outputs = {}
            order = (False, True) if trial % 2 == 0 else (True, False)
            for optimized in order:
                label = "candidate" if optimized else "baseline"
                dest = root / f"{label}-{trial}"
                dest.mkdir()
                env = dict(os.environ)
                for toggle in ("SESSION_PARALLEL_SORT", "SESSION_PARALLEL_ROUTING", "SESSION_CSV_SAFETY_HINT", "SESSION_CANONICAL_PARTITIONS", "SESSION_DIRECT_ROUTING", "SESSION_FINGERPRINTED_WINNERS", "SESSION_SELECTIVE_DEDUP_FIELDS"):
                    env.pop(toggle, None)
                mode = os.environ.get("SORT_TEST_MODE", "")
                if mode == "selective_dedup":
                    env["SESSION_DIRECT_ROUTING"] = "1"
                    env["SESSION_CANONICAL_PARTITIONS"] = "1"
                    env["SESSION_FINGERPRINTED_WINNERS"] = "1"
                if mode in ("direct_canonical", "buffered_canonical", "fingerprint_direct"):
                    env["SESSION_CANONICAL_PARTITIONS"] = "1"
                if mode == "fingerprint_direct":
                    # Both competitors use the verified direct router; only
                    # the shard-local deduplication algorithm differs.
                    env["SESSION_DIRECT_ROUTING"] = "1"
                if optimized:
                    if mode == "selective_dedup":
                        env["SESSION_SELECTIVE_DEDUP_FIELDS"] = "1"
                    elif mode == "fingerprint_direct":
                        env["SESSION_FINGERPRINTED_WINNERS"] = "1"
                    elif mode == "direct_canonical":
                        env["SESSION_DIRECT_ROUTING"] = "1"
                    elif mode == "buffered_canonical":
                        env["SESSION_PARALLEL_ROUTING"] = "1"
                    elif mode == "route_export":
                        env["SESSION_PARALLEL_ROUTING"] = "1"
                        env["SESSION_CSV_SAFETY_HINT"] = "1"
                    elif os.environ.get("SORT_TEST_MODE") == "canonical_users":
                        env["SESSION_CANONICAL_PARTITIONS"] = "1"
                    else:
                        env["SESSION_PARALLEL_SORT"] = "1"
                started = time.perf_counter()
                result = subprocess.run([sys.executable, str(RUNNER), str(data), str(dest)],
                                        check=True, env=env, capture_output=True, text=True)
                wall = time.perf_counter() - started
                match = re.search(r"Python pipeline total:\s+([0-9.]+) s", result.stdout)
                if match is None:
                    raise RuntimeError("Missing engine timing:\\n" + result.stdout[-2500:])
                internal = float(match[1])
                if os.environ.get("SORT_TEST_PROFILE") == "1":
                    for line in result.stdout.splitlines():
                        if (line.startswith("[INNER] ") or
                            line.startswith("[PROFILE] C++ peak RSS:") or
                            line.startswith("[PROFILE] C++ time:")):
                            print(f"PROFILE {label} trial={trial} {line}", flush=True)
                measurements[label].append(internal)
                outputs[label] = dest
                print(f"{label} trial={trial} pipeline={internal:.4f}s host={wall:.4f}s", flush=True)
            subprocess.run([sys.executable, str(COMPARE),
                            str(outputs["baseline"]), str(outputs["candidate"])],
                           check=True)
        print(f"PASS: {COUNT:,}-event baseline and {os.getenv('SORT_TEST_MODE', 'parallel-sort')} Parquet outputs match", flush=True)
        for name in ("baseline", "candidate"):
            values = measurements[name]
            print(f"PERF {name} median={statistics.median(values):.4f}s "
                  f"range={min(values):.4f}-{max(values):.4f}s", flush=True)


if __name__ == "__main__":
    main()
