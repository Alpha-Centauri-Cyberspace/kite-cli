#!/usr/bin/env python3
"""Create a reproducible single-binary Kite release archive and SHA256 sidecar."""

from __future__ import annotations

import argparse
import gzip
import hashlib
import tarfile
from pathlib import Path


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--source-date-epoch", type=int, required=True)
    return parser.parse_args()


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as file:
        for chunk in iter(lambda: file.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def main() -> int:
    args = parse_args()
    binary = args.binary.resolve()
    output = args.output.resolve()
    if not binary.is_file():
        raise SystemExit(f"binary does not exist: {binary}")
    if binary.stat().st_size == 0:
        raise SystemExit(f"binary is empty: {binary}")
    if args.source_date_epoch < 0:
        raise SystemExit("--source-date-epoch must be non-negative")

    output.parent.mkdir(parents=True, exist_ok=True)
    with output.open("wb") as raw_archive:
        with gzip.GzipFile(
            filename="",
            mode="wb",
            compresslevel=9,
            fileobj=raw_archive,
            mtime=args.source_date_epoch,
        ) as compressed:
            with tarfile.open(fileobj=compressed, mode="w", format=tarfile.USTAR_FORMAT) as archive:
                info = tarfile.TarInfo(name="kite")
                info.size = binary.stat().st_size
                info.mode = 0o755
                info.uid = 0
                info.gid = 0
                info.uname = "root"
                info.gname = "root"
                info.mtime = args.source_date_epoch
                with binary.open("rb") as executable:
                    archive.addfile(info, executable)

    checksum = sha256_file(output)
    sidecar = output.with_name(f"{output.name}.sha256")
    sidecar.write_text(f"{checksum}  {output.name}\n", encoding="utf-8")
    print(f"created {output.name} ({output.stat().st_size} bytes, sha256 {checksum})")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
