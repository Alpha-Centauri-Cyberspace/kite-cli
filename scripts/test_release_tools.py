#!/usr/bin/env python3
"""Tests for deterministic CLI release packaging and manifest validation."""

from __future__ import annotations

import hashlib
import json
import re
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path


REPO = Path(__file__).resolve().parent.parent
PACKAGE_SCRIPT = REPO / "scripts" / "package-release.py"
VALIDATE_SCRIPT = REPO / "scripts" / "validate-release.py"
RELEASE_WORKFLOW = REPO / ".github" / "workflows" / "auto-release.yml"
HOMEBREW_WORKFLOW = REPO / ".github" / "workflows" / "publish-homebrew.yml"
CI_WORKFLOW = REPO / ".github" / "workflows" / "ci.yml"
ASSETS = ("kite-darwin-arm64.tar.gz", "kite-linux-x86_64.tar.gz")
SOURCE_SHA = "0123456789abcdef0123456789abcdef01234567"


def package_version() -> str:
    """Read the crate version from Cargo.toml so tests track the release bump."""

    import tomllib

    with (REPO / "Cargo.toml").open("rb") as file:
        return tomllib.load(file)["package"]["version"]


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def release_python() -> str:
    """Return a Python interpreter that can run the release scripts.

    The release scripts need ``tomllib`` (Python 3.11+). Prefer the running
    interpreter, then common interpreter names, so the tests pass on hosts
    whose default ``python3`` is older than 3.11.
    """

    candidates = [sys.executable]
    for version in ("3.14", "3.13", "3.12", "3.11"):
        candidates.append(f"python{version}")
    candidates.append("python3")
    for candidate in candidates:
        if not candidate:
            continue
        result = subprocess.run(
            [candidate, "-c", "import tomllib"],
            capture_output=True,
            text=True,
        )
        if result.returncode == 0:
            return candidate
    raise RuntimeError("no Python interpreter with tomllib (3.11+) found")


class ReleaseToolsTest(unittest.TestCase):
    def run_package(self, binary: Path, output: Path) -> None:
        subprocess.run(
            [
                release_python(),
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
        version = package_version()
        assets = []
        for name in ASSETS:
            archive = directory / name
            self.run_package(binary, archive)
            assets.append(
                {
                    "name": name,
                    "size": archive.stat().st_size,
                    "sha256": sha256(archive),
                    "download_url": f"https://downloads.getkite.sh/releases/v{version}/{name}",
                }
            )
        manifest = directory / "manifest.json"
        manifest.write_text(
            json.dumps(
                {
                    "tag_name": f"v{version}",
                    "source_sha": SOURCE_SHA,
                    "published_at": "2026-08-07T00:00:00+00:00",
                    "assets": assets,
                },
                sort_keys=True,
            ),
            encoding="utf-8",
        )
        return manifest

    def assertWorkflowRunsReleasePython(self, workflow: str) -> None:
        """Every step that runs validate-release.py must have Python 3.11+ available.

        GitHub Actions jobs are independent environments, so the check is
        scoped per job (delimited by each top-level ``runs-on:`` line): a
        setup step earlier in the same job satisfies the contract; a setup in
        another job does not.
        """

        job_pattern = re.compile(r"^\s{2}([a-z0-9_-]+):\s*$")
        lines = workflow.splitlines()
        jobs: dict[str, list[str]] = {}
        current: str | None = None
        for line in lines:
            match = job_pattern.match(line)
            if match:
                current = match.group(1)
                jobs[current] = []
            elif current is not None:
                jobs[current].append(line)

        job_names: list[str] = [name for name in jobs if name]
        for name in job_names:
            body = jobs[name]
            joined = "\n".join(body)
            # validate-release.py imports tomllib (Python 3.11+); package-release.py
            # is stdlib-only and runs on any interpreter.
            if "scripts/validate-release.py" not in joined:
                continue
            script_pos = joined.find("scripts/validate-release.py")
            if script_pos == -1:
                script_pos = joined.find("scripts\\/")
            setup_pos = joined.find("actions/setup-python")
            self.assertNotEqual(
                setup_pos,
                -1,
                f"job {name!r} runs release scripts without provisioning Python 3.11+",
            )
            self.assertLess(
                setup_pos,
                script_pos,
                f"job {name!r} must provision Python 3.11+ before running release scripts",
            )

    def test_release_workflows_provision_python_for_release_scripts(self) -> None:
        for label, workflow_path in (
            ("auto-release", RELEASE_WORKFLOW),
            ("publish-homebrew", HOMEBREW_WORKFLOW),
            ("ci", CI_WORKFLOW),
        ):
            with self.subTest(workflow=label):
                self.assertWorkflowRunsReleasePython(
                    workflow_path.read_text(encoding="utf-8")
                )

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
                    release_python(),
                    str(VALIDATE_SCRIPT),
                    "--tag",
                    f"v{package_version()}",
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
                    release_python(),
                    str(VALIDATE_SCRIPT),
                    "--tag",
                    f"v{package_version()}",
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

    def test_release_workflow_serializes_latest_pointer_updates(self) -> None:
        workflow = RELEASE_WORKFLOW.read_text(encoding="utf-8")

        self.assertIn("group: cli-release\n", workflow)
        self.assertNotIn("group: cli-release-${{ inputs.tag }}", workflow)

    def test_release_workflow_conditionally_creates_immutable_objects(self) -> None:
        workflow = RELEASE_WORKFLOW.read_text(encoding="utf-8")
        immutable_step = workflow.split(
            "- name: Upload or verify immutable R2 objects", maxsplit=1
        )[1].split("- name: Verify public immutable release artifacts", maxsplit=1)[0]

        self.assertIn("--if-none-match '*'", immutable_step)
        self.assertNotIn('aws s3 cp "$source" "$destination"', immutable_step)


if __name__ == "__main__":
    unittest.main()
