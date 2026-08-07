#!/usr/bin/env python3
"""Validate Kite CLI source metadata and generated release manifests."""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import sys
import tarfile
import tomllib
from datetime import datetime
from pathlib import Path
from typing import NoReturn
from urllib.parse import urlparse


PACKAGE_NAME = "kite-cli"
EXPECTED_ASSETS = {
    "kite-darwin-arm64.tar.gz",
    "kite-linux-x86_64.tar.gz",
}
SEMVER = re.compile(r"^[0-9]+\.[0-9]+\.[0-9]+$")
SHA256 = re.compile(r"^[0-9a-f]{64}$")
GIT_SHA = re.compile(r"^[0-9a-f]{40}$")
SAFE_DOWNLOAD_URL = re.compile(
    r"^https://[A-Za-z0-9.-]+(?::[0-9]+)?/[A-Za-z0-9._~:/?&=+,%@;-]+$"
)


def fail(message: str) -> NoReturn:
    raise ValueError(message)


def load_toml(path: Path) -> dict:
    with path.open("rb") as file:
        return tomllib.load(file)


def validate_source(repo: Path, expected_tag: str | None) -> str:
    manifest = load_toml(repo / "Cargo.toml")
    package = manifest.get("package", {})
    if package.get("name") != PACKAGE_NAME:
        fail(f"Cargo.toml package name must be {PACKAGE_NAME!r}")
    if package.get("publish") is not False:
        fail("Cargo.toml must set publish = false; the crates.io name is not owned by Kite")

    version = package.get("version")
    if not isinstance(version, str) or not SEMVER.fullmatch(version):
        fail(f"Cargo.toml package version must be clean X.Y.Z semver, got {version!r}")

    lock = load_toml(repo / "Cargo.lock")
    local_packages = [
        item
        for item in lock.get("package", [])
        if item.get("name") == PACKAGE_NAME and "source" not in item
    ]
    if len(local_packages) != 1:
        fail(f"Cargo.lock must contain exactly one local {PACKAGE_NAME!r} package")
    if local_packages[0].get("version") != version:
        fail(
            "Cargo.lock package version does not match Cargo.toml: "
            f"{local_packages[0].get('version')!r} != {version!r}"
        )

    tag = f"v{version}"
    if expected_tag is not None and expected_tag != tag:
        fail(f"release tag {expected_tag!r} does not match Cargo.toml version {tag!r}")

    forbidden_claims = (
        "cargo install " + PACKAGE_NAME,
        "ghcr.io/alpha-centauri-cyberspace/" + PACKAGE_NAME,
    )
    for document in repo.rglob("*.md"):
        if any(part in {".git", "target"} for part in document.parts):
            continue
        contents = document.read_text(encoding="utf-8").lower()
        for claim in forbidden_claims:
            if claim in contents:
                fail(
                    f"{document.relative_to(repo)} advertises unsupported distribution channel: "
                    f"{claim!r}"
                )

    readme = (repo / "README.md").read_text(encoding="utf-8").lower()
    unsupported_notice = (
        "does not publish this cli to crates.io or as a public container image"
    )
    if unsupported_notice not in readme:
        fail(
            "README must explicitly state that crates.io and public container images are unsupported"
        )

    dockerfile = repo / "Dockerfile"
    if dockerfile.exists():
        docker_contents = dockerfile.read_text(encoding="utf-8")
        toolchain = load_toml(repo / "rust-toolchain.toml").get("toolchain", {})
        channel = toolchain.get("channel")
        if not isinstance(channel, str) or f"FROM rust:{channel}-" not in docker_contents:
            fail("Dockerfile builder image must match the repository Rust toolchain exactly")
        if "COPY rust-toolchain.toml" not in docker_contents:
            fail("Dockerfile must copy rust-toolchain.toml into the source build")

    return tag


def validate_manifest(
    path: Path, expected_tag: str, expected_source_sha: str | None = None
) -> dict:
    data = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(data, dict):
        fail("release manifest root must be an object")
    if data.get("tag_name") != expected_tag:
        fail(
            f"release manifest tag {data.get('tag_name')!r} does not match {expected_tag!r}"
        )

    source_sha = data.get("source_sha")
    if source_sha is not None and (
        not isinstance(source_sha, str) or not GIT_SHA.fullmatch(source_sha)
    ):
        fail(f"release manifest source_sha is invalid: {source_sha!r}")
    if expected_source_sha is not None:
        if not GIT_SHA.fullmatch(expected_source_sha):
            fail(f"expected source SHA is invalid: {expected_source_sha!r}")
        if source_sha != expected_source_sha:
            fail(
                f"release manifest source_sha {source_sha!r} does not match "
                f"{expected_source_sha!r}"
            )

    published_at = data.get("published_at")
    if not isinstance(published_at, str):
        fail("release manifest published_at must be an ISO-8601 string")
    try:
        timestamp = datetime.fromisoformat(published_at.replace("Z", "+00:00"))
    except ValueError as error:
        fail(f"release manifest published_at is invalid: {error}")
    if timestamp.tzinfo is None:
        fail("release manifest published_at must include a timezone")

    assets_value = data.get("assets")
    if not isinstance(assets_value, list):
        fail("release manifest assets must be an array")
    assets: list[dict] = []
    names: list[str] = []
    for item in assets_value:
        if not isinstance(item, dict) or not isinstance(item.get("name"), str):
            fail("release manifest assets must be objects with string names")
        assets.append(item)
        names.append(item["name"])
    if len(set(names)) != len(names):
        fail("release manifest assets must have unique names")
    if set(names) != EXPECTED_ASSETS:
        fail(
            "release manifest asset set does not match the supported target matrix: "
            f"expected {sorted(EXPECTED_ASSETS)!r}, got {sorted(names)!r}"
        )

    for asset in assets:
        name = asset["name"]
        if Path(name).name != name:
            fail(f"release asset name must not contain a path: {name!r}")
        size = asset.get("size")
        if not isinstance(size, int) or isinstance(size, bool) or size <= 0:
            fail(f"release asset {name!r} has invalid size {size!r}")
        checksum = asset.get("sha256")
        if not isinstance(checksum, str) or not SHA256.fullmatch(checksum):
            fail(f"release asset {name!r} has invalid sha256 {checksum!r}")
        download_url = asset.get("download_url")
        if not isinstance(download_url, str):
            fail(f"release asset {name!r} is missing download_url")
        if not SAFE_DOWNLOAD_URL.fullmatch(download_url):
            fail(f"release asset {name!r} download_url contains unsafe characters")
        parsed = urlparse(download_url)
        if (
            parsed.scheme != "https"
            or not parsed.hostname
            or parsed.username is not None
            or parsed.password is not None
            or parsed.query
            or parsed.fragment
        ):
            fail(f"release asset {name!r} download_url must be HTTPS")
        if "/releases/" not in parsed.path or not parsed.path.endswith(
            f"/{expected_tag}/{name}"
        ):
            fail(
                f"release asset {name!r} URL does not contain the matching tag and filename"
            )

    return data


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as file:
        for chunk in iter(lambda: file.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def validate_artifacts(directory: Path, manifest: dict) -> None:
    expected_files = {
        filename
        for asset in manifest["assets"]
        for filename in (asset["name"], f"{asset['name']}.sha256")
    }
    actual_files = {
        path.name
        for path in directory.iterdir()
        if path.is_file()
        and (path.name.endswith(".tar.gz") or path.name.endswith(".tar.gz.sha256"))
    }
    if actual_files != expected_files:
        fail(
            "release directory archive/checksum set is not exact: "
            f"expected {sorted(expected_files)!r}, got {sorted(actual_files)!r}"
        )

    for asset in manifest["assets"]:
        name = asset["name"]
        archive = directory / name
        sidecar = directory / f"{name}.sha256"
        if not archive.is_file() or not sidecar.is_file():
            fail(f"release directory is missing {name!r} or its checksum sidecar")
        actual = sha256_file(archive)
        if actual != asset["sha256"]:
            fail(f"archive checksum for {name!r} does not match the release manifest")
        sidecar_fields = sidecar.read_text(encoding="utf-8").split()
        if (
            len(sidecar_fields) != 2
            or sidecar_fields[0].lower() != actual
            or sidecar_fields[1] != name
        ):
            fail(f"checksum sidecar for {name!r} does not match the archive")
        try:
            with tarfile.open(archive, mode="r:gz") as release_archive:
                members = release_archive.getmembers()
        except tarfile.TarError as error:
            fail(f"release archive {name!r} is invalid: {error}")
        if len(members) != 1:
            fail(f"release archive {name!r} must contain exactly one entry")
        binary = members[0]
        if binary.name != "kite" or not binary.isfile() or binary.size <= 0:
            fail(f"release archive {name!r} must contain one regular non-empty file named 'kite'")
        if binary.mode & 0o777 != 0o755:
            fail(f"release archive {name!r} kite mode must be 0755")


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--repo", type=Path, default=Path(__file__).resolve().parent.parent)
    parser.add_argument("--tag", help="Expected vX.Y.Z release tag")
    parser.add_argument("--source-sha", help="Expected immutable 40-character source commit")
    parser.add_argument("--manifest", type=Path, help="Generated release manifest")
    parser.add_argument("--artifacts-dir", type=Path, help="Directory containing release archives")
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    try:
        source_tag = validate_source(args.repo.resolve(), args.tag)
        expected_tag = args.tag or source_tag
        if args.artifacts_dir and not args.manifest:
            fail("--artifacts-dir requires --manifest")
        if args.manifest:
            release_manifest = validate_manifest(
                args.manifest, expected_tag, args.source_sha
            )
            if args.artifacts_dir:
                validate_artifacts(args.artifacts_dir, release_manifest)
    except (OSError, ValueError, json.JSONDecodeError, tomllib.TOMLDecodeError) as error:
        print(f"release validation failed: {error}", file=sys.stderr)
        return 1

    print(f"release validation passed for {expected_tag}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())