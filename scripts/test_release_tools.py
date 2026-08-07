#!/usr/bin/env python3
"""Tests for deterministic CLI release packaging and manifest validation."""

from __future__ import annotations

import hashlib
import json
import subprocess
import tempfile
import unittest
from pathlib import Path


REPO = Path(__file__).resolve().parent.parent
PACKAGE_SCRIPT = REPO / "scripts" / "package-release.py"
VALIDATE_SCRIPT = REPO / "scripts" / "validate-release.py"
ASSETS = ("kite-darwin-arm64.tar.gz", "kite-linux-x86_64.tar.gz")
SOURCE_SHA = "0123456789abcdef0123456789abcdef01234567"


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


class ReleaseToolsTest(unittest.TestCase):
    def run_package(self, binary: Path, output: Path) -> None:
        subprocess.run(
            [
                "python3",
                str(PACKAGE_SCRIPT),
                "--binary",
                str(binary),
                "--output",
                str(output),
                "--source-date-epoch",
                "1700000000",
            ],
            check=True,
            cwd=REPO,
            capture_output=True,
            text=True,
        )

    def make_release(self, directory: Path) -> Path:
        binary = directory / "kite-bin"
        binary.write_bytes(b"fake-kite-binary\n")
        binary.chmod(0o755)
        assets = []
        for name in ASSETS:
            archive = directory / name
            self.run_package(binary, archive)
            assets.append(
                {
                    "name": name,
                    "size": archive.stat().st_size,
                    "sha256": sha256(archive),
                    "download_url": f"https://downloads.getkite.sh/releases/v0.2.2/{name}",
                }
            )
        manifest = directory / "manifest.json"
        manifest.write_text(
            json.dumps(
                {
                    "tag_name": "v0.2.2",
                    "source_sha": SOURCE_SHA,
                    "published_at": "2026-08-07T00:00:00+00:00",
                    "assets": assets,
                },
                sort_keys=True,
            ),
            encoding="utf-8",
        )
        return manifest

    def test_archive_is_reproducible(self) -> None:
        with tempfile.TemporaryDirectory() as raw_directory:
            directory = Path(raw_directory)
            binary = directory / "kite-bin"
            binary.write_bytes(b"fake-kite-binary\n")
            first = directory / "first.tar.gz"
            second = directory / "second.tar.gz"
            self.run_package(binary, first)
            self.run_package(binary, second)
            self.assertEqual(first.read_bytes(), second.read_bytes())

    def test_exact_release_artifacts_validate(self) -> None:
        with tempfile.TemporaryDirectory() as raw_directory:
            directory = Path(raw_directory)
            manifest = self.make_release(directory)
            subprocess.run(
                [
                    "python3",
                    str(VALIDATE_SCRIPT),
                    "--tag",
                    "v0.2.2",
                    "--source-sha",
                    SOURCE_SHA,
                    "--manifest",
                    str(manifest),
                    "--artifacts-dir",
                    str(directory),
                ],
                check=True,
                cwd=REPO,
                capture_output=True,
                text=True,
            )

    def test_manifest_rejects_shell_metacharacters_in_url(self) -> None:
        with tempfile.TemporaryDirectory() as raw_directory:
            directory = Path(raw_directory)
            manifest = self.make_release(directory)
            data = json.loads(manifest.read_text(encoding="utf-8"))
            data["assets"][0]["download_url"] = (
                "https://downloads.getkite.sh/$(touch%20pwned)/releases/"
                "v0.2.2/kite-darwin-arm64.tar.gz"
            )
            manifest.write_text(json.dumps(data), encoding="utf-8")
            result = subprocess.run(
                [
                    "python3",
                    str(VALIDATE_SCRIPT),
                    "--tag",
                    "v0.2.2",
                    "--source-sha",
                    SOURCE_SHA,
                    "--manifest",
                    str(manifest),
                ],
                cwd=REPO,
                capture_output=True,
                text=True,
            )
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("unsafe characters", result.stderr)


if __name__ == "__main__":
    unittest.main()
