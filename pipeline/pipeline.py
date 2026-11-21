
#!/usr/bin/env python3
"""Run the verified Rust logic and convert its reports to typed Parquet."""

import json
import os
from pathlib import Path
import subprocess

import duckdb

SCHEMAS = {
    "user_daily": [
        ("user_id", "VARCHAR"),
        ("day", "DATE"),
        ("sessions", "BIGINT"),
        ("events", "BIGINT"),
        ("revenue_cents", "BIGINT"),
        ("top_category", "VARCHAR"),
    ],
    "campaign_daily": [
        ("campaign_id", "VARCHAR"),
        ("channel", "VARCHAR"),
        ("day", "DATE"),
        ("sessions", "BIGINT"),
        ("purchasing_sessions", "BIGINT"),
        ("revenue_cents", "BIGINT"),
        ("assisted_sessions", "BIGINT"),
    ],
}

def sql_string(value):
    return "'" + str(value).replace("'", "''") + "'"

def diagnostic_csv_to_jsonl(csv_path, json_path, schema):
    """
    Convert Rust's restricted diagnostic CSV to NDJSON.

    Read LF-separated records at the byte level.
    A CR inside the final field is data, not a
    record terminator.

    The Rust diagnostic CSV contains no embedded
    commas or escaped quotes.
    """
    names = [name for name, _ in schema]

    numeric_names = {
        name
        for name, kind in schema
        if kind == "BIGINT"
    }

    count = 0

    with csv_path.open("rb") as source, json_path.open(
        "w",
        encoding="utf-8",
        newline="\n",
    ) as destination:

        header = source.readline()

        if header.endswith(b"\n"):
            header = header[:-1]

        expected_header = ",".join(names).encode("utf-8")

        if header != expected_header:
            raise ValueError(
                f"Unexpected columns in {csv_path}: {header!r}"
            )

        for raw in source:
            if raw.endswith(b"\n"):
                raw = raw[:-1]

            cells = raw.split(b",")

            if len(cells) != len(names):
                raise ValueError(
                    f"Malformed diagnostic CSV row in {csv_path}"
                )

            record = {}

            for (name, _), raw_field in zip(schema, cells):
                value = raw_field.decode("utf-8")

                if name in numeric_names:
                    record[name] = int(value)

                elif name == "top_category" and value == "":
                    record[name] = None

                else:
                    record[name] = value

            destination.write(
                json.dumps(
                    record,
                    ensure_ascii=True,
                    separators=(",", ":"),
                )
                + "\n"
            )

            count += 1

    return count

def parquet_copy(
    con,
    name,
    schema,
    json_path,
    row_count,
    output_dir,
):
    destination = output_dir / name
    destination.mkdir(parents=True, exist_ok=True)

    parquet_file = destination / "part-0.parquet"

    typed_columns = ", ".join(
        f'CAST("{field}" AS {kind}) AS "{field}"'
        for field, kind in schema
    )

    if row_count:
        json_types = ", ".join(
            f"'{field}': "
            f"'{'BIGINT' if kind == 'BIGINT' else 'VARCHAR'}'"
            for field, kind in schema
        )

        query = (
            f"SELECT {typed_columns} "
            f"FROM read_json("
            f"{sql_string(json_path)}, "
            f"format='newline_delimited', "
            f"columns={{{json_types}}})"
        )

    else:
        # Empty fixtures still need valid, typed Parquet.
        empty_columns = ", ".join(
            f'CAST(NULL AS {kind}) AS "{field}"'
            for field, kind in schema
        )

        query = f"SELECT {empty_columns} WHERE FALSE"

    con.execute(
        f"COPY ({query}) "
        f"TO {sql_string(parquet_file)} "
        "(FORMAT PARQUET, COMPRESSION 'SNAPPY')"
    )

    print(
        f"{name}: wrote {row_count} rows to {parquet_file}",
        flush=True,
    )

def main():
    data_dir = Path(
        os.environ.get("DATA_DIR", "/data")
    ).resolve()

    output_dir = Path(
        os.environ.get("OUTPUT_DIR", "/output")
    ).resolve()

    scratch_dir = Path(
        os.environ.get("SCRATCH_DIR", "/scratch")
    ).resolve()

    ncpu = max(
        1,
        int(os.environ.get("NCPU", "8")),
    )

    mem_gb = max(
        1,
        int(os.environ.get("MEM_GB", "24")),
    )

    scratch_dir.mkdir(
        parents=True,
        exist_ok=True,
    )

    output_dir.mkdir(
        parents=True,
        exist_ok=True,
    )

    # ================================================
    # PHASE 1: RUN RUST
    # ================================================

    print("Running Rust engine...", flush=True)

    subprocess.run(
        [
            "/app/rust_engine",
            str(data_dir),
        ],
        cwd=scratch_dir,
        check=True,
    )

    # Rust writes its diagnostic CSVs into:
    # /scratch/out/
    reports_dir = scratch_dir / "out"

    # ================================================
    # PHASE 2: INITIALIZE DUCKDB
    # ================================================

    con = duckdb.connect(database=":memory:")

    con.execute(
        f"SET threads = {ncpu}"
    )

    con.execute(
        f"SET memory_limit = "
        f"'{max(1, int(mem_gb * 0.75))}GB'"
    )

    con.execute(
        "SET temp_directory = "
        + sql_string(scratch_dir / "duckdb_tmp")
    )

    con.execute(
        "SET preserve_insertion_order = false"
    )

    # ================================================
    # PHASE 3: WRITE BOTH PARQUET REPORTS
    # ================================================

    for name, schema in SCHEMAS.items():
        csv_path = reports_dir / f"{name}.csv"

        if not csv_path.is_file():
            raise FileNotFoundError(
                f"Rust did not create {csv_path}"
            )

        json_path = scratch_dir / f"{name}.jsonl"

        count = diagnostic_csv_to_jsonl(
            csv_path,
            json_path,
            schema,
        )

        parquet_copy(
            con,
            name,
            schema,
            json_path,
            count,
            output_dir,
        )

        # Free temporary disk space after conversion.
        json_path.unlink(missing_ok=True)
        csv_path.unlink(missing_ok=True)

    con.close()

    print(
        "Both Parquet reports created successfully",
        flush=True,
    )

if __name__ == "__main__":
    main()