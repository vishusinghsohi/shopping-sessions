#!/usr/bin/env python3
"""Compare baseline and parallel range-sort on >100k valid synthetic events.

The row count intentionally activates the parallel branch. This test compares
exact resulting Parquet data, not just a checksum; it is NOT a timing benchmark.
"""
import json
import os
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
                out.write(
                    f"e{i},u{(i * 43) % 2500:04d},{ts},{ts + 1000},"
                    f"{kind},{'p1' if i % 2 else 'p2'},{price}\n"
                )

        baseline = root / "baseline"
        parallel = root / "parallel"
        for mode, dest in ((False, baseline), (True, parallel)):
            dest.mkdir()
            env = dict(os.environ)
            for toggle in ("SESSION_PARALLEL_SORT", "SESSION_PARALLEL_ROUTING", "SESSION_CSV_SAFETY_HINT"):
                env.pop(toggle, None)
            if mode:
                if os.environ.get("SORT_TEST_MODE") == "route_export":
                    env["SESSION_PARALLEL_ROUTING"] = "1"
                    env["SESSION_CSV_SAFETY_HINT"] = "1"
                else:
                    env["SESSION_PARALLEL_SORT"] = "1"
            print("Running", "parallel" if mode else "baseline", flush=True)
            subprocess.run([sys.executable, str(RUNNER), str(data), str(dest)],
                           check=True, env=env)
        subprocess.run([sys.executable, str(COMPARE), str(baseline), str(parallel)],
                       check=True)
        print(f"PASS: {COUNT:,}-event baseline and {os.getenv('SORT_TEST_MODE', 'parallel-sort')} Parquet outputs match", flush=True)


if __name__ == "__main__":
    main()
