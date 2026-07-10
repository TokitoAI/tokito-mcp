#!/usr/bin/env python3
"""Fail when workspace, advertised, and release versions can drift."""

from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SEMVER = re.compile(r"^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$")


def fail(message: str) -> None:
    print(f"version check failed: {message}", file=sys.stderr)
    raise SystemExit(1)


def version_tuple(version: str) -> tuple[int, int, int]:
    match = SEMVER.fullmatch(version)
    if not match:
        fail(f"{version!r} is not a plain MAJOR.MINOR.PATCH version")
    return tuple(map(int, match.groups()))


def workspace_version() -> str:
    with (ROOT / "Cargo.toml").open("rb") as source:
        manifest = tomllib.load(source)
    return manifest["workspace"]["package"]["version"]


def package_versions() -> dict[str, str]:
    result = subprocess.run(
        ["cargo", "metadata", "--format-version", "1", "--no-deps"],
        cwd=ROOT,
        check=True,
        capture_output=True,
        text=True,
    )
    metadata = json.loads(result.stdout)
    members = set(metadata["workspace_members"])
    return {
        package["name"]: package["version"]
        for package in metadata["packages"]
        if package["id"] in members
    }


def lockfile_versions(workspace_packages: set[str]) -> dict[str, str]:
    with (ROOT / "Cargo.lock").open("rb") as source:
        lockfile = tomllib.load(source)
    return {
        package["name"]: package["version"]
        for package in lockfile["package"]
        if package["name"] in workspace_packages
    }


def latest_release_tag() -> str | None:
    result = subprocess.run(
        ["git", "tag", "--list", "v[0-9]*.[0-9]*.[0-9]*"],
        cwd=ROOT,
        check=True,
        capture_output=True,
        text=True,
    )
    tags = [tag for tag in result.stdout.splitlines() if SEMVER.fullmatch(tag.removeprefix("v"))]
    return max(tags, key=lambda tag: version_tuple(tag.removeprefix("v"))) if tags else None


def check_documented_versions(expected: str) -> None:
    readme = (ROOT / "README.md").read_text()
    image = f"ghcr.io/vtrontokito/tokito-mcp:v{expected}"
    if image not in readme:
        fail(f"README Docker example must advertise {image}")

    env_example = (ROOT / "deploy" / "production" / ".env.example").read_text()
    image_line = f"TOKITO_MCP_IMAGE={image}"
    if image_line not in env_example:
        fail(f"production .env.example must advertise {image_line}")

    runbook = (ROOT / "docs" / "deployment.md").read_text()
    if f"TOKITO_MCP_EXPECTED_VERSION={expected}" not in runbook:
        fail(f"deployment smoke example must expect version {expected}")


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--release-tag",
        help="tag being released; must exactly match the workspace version (for example v0.1.2)",
    )
    args = parser.parse_args()

    expected = workspace_version()
    expected_tuple = version_tuple(expected)
    packages = package_versions()
    mismatches = {
        name: version
        for name, version in packages.items()
        if version != expected
    }
    if mismatches:
        rendered = ", ".join(f"{name}={version}" for name, version in sorted(mismatches.items()))
        fail(f"workspace.package.version is {expected}, but {rendered}")
    lock_mismatches = {
        name: version
        for name, version in lockfile_versions(set(packages)).items()
        if version != expected
    }
    if lock_mismatches:
        rendered = ", ".join(
            f"{name}={version}" for name, version in sorted(lock_mismatches.items())
        )
        fail(f"Cargo.lock is stale for workspace version {expected}: {rendered}")
    check_documented_versions(expected)

    latest = latest_release_tag()
    if latest and expected_tuple < version_tuple(latest.removeprefix("v")):
        fail(f"workspace version {expected} is older than latest release tag {latest}")

    if args.release_tag and args.release_tag != f"v{expected}":
        fail(f"release tag {args.release_tag} does not match workspace version v{expected}")

    print(
        f"version check passed: all workspace packages advertise {expected}"
        + (f"; latest release is {latest}" if latest else "")
    )


if __name__ == "__main__":
    main()
