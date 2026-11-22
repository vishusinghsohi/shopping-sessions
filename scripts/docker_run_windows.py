
#!/usr/bin/env python3
"""Windows-friendly Docker runner for official fixtures."""

from pathlib import Path
import os
import subprocess
import sys
import tempfile

def main():
    if len(sys.argv) != 3:
        raise SystemExit(
            "Usage: docker_run_windows.py INPUT_DIR OUTPUT_DIR"
        )

    data = Path(sys.argv[1]).resolve()
    output = Path(sys.argv[2]).resolve()

    output.mkdir(
        parents=True,
        exist_ok=True,
    )
    # Fixture harness output directories may be mode 0700 on Linux.
    # Container runs as UID 1000, which must be able to write here.
    if os.name == "posix":
        output.chmod(0o777)

    if not data.is_dir():
        raise SystemExit(
            f"Missing data directory: {data}"
        )

    with tempfile.TemporaryDirectory(
        prefix="shopping-scratch-"
    ) as temp:
        scratch = Path(temp).resolve()
        if os.name == "posix":
            scratch.chmod(0o777)

        cpus = os.getenv("CPUS", "8")
        memory = os.getenv("MEM", "24g")
        image = os.getenv(
            "IMAGE",
            "shopping-sessions:latest",
        )

        command = [
            "docker",
            "run",
            "--rm",
            "--platform",
            "linux/amd64",
            "--network",
            "none",
            "--read-only",
            "--tmpfs",
            "/tmp:rw,size=64m,mode=1777",
            "--cap-drop",
            "ALL",
            "--security-opt",
            "no-new-privileges",
            "--cpus",
            cpus,
            "--memory",
            memory,
            "--memory-swap",
            memory,
            "--user",
            os.getenv("TEST_DOCKER_USER", "1000:1000"),
            "--mount",
            f"type=bind,source={data},target=/data,readonly",
            "--mount",
            f"type=bind,source={output},target=/output",
            "--mount",
            f"type=bind,source={scratch},target=/scratch",
            "-e",
            "DATA_DIR=/data",
            "-e",
            "OUTPUT_DIR=/output",
            "-e",
            "SCRATCH_DIR=/scratch",
            "-e",
            "HOME=/scratch",
            "-e",
            f"NCPU={cpus}",
            "-e",
            f"MEM_GB={memory.rstrip('gG')}",
            image,
        ]

        # Optional experimental sorting architecture; defaults to the baseline.
        for toggle in ("SESSION_PARALLEL_SORT", "SESSION_PARALLEL_ROUTING", "SESSION_CSV_SAFETY_HINT", "SESSION_COMPACT_WINNERS"):
            if os.getenv(toggle) == "1":
                command[-1:-1] = ["-e", f"{toggle}=1"]

        return subprocess.run(
            command,
            check=False,
        ).returncode

if __name__ == "__main__":
    sys.exit(main())