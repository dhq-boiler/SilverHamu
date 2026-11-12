"""Check Rust client release metadata, package contents and registry checksums."""

import argparse
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import re
import tarfile
import tomllib
from urllib.error import HTTPError
from urllib.request import Request, urlopen


ROOT = Path(__file__).resolve().parents[1]
CRATE = "silverhamu-client"
VERSION = re.compile(r"(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)(?:-[0-9A-Za-z]+(?:[.-][0-9A-Za-z]+)*)?(?:\+[0-9A-Za-z]+(?:[.-][0-9A-Za-z]+)*)?")
USER_AGENT = "SilverHamu-client-release (https://github.com/dhq-boiler/SilverHamu)"


def validate_version(version):
    if not VERSION.fullmatch(version):
        raise ValueError("version must be a Cargo semantic version")
    return version


def check_metadata(version, root=ROOT):
    validate_version(version)
    client = root / "clients/rust"
    package = tomllib.loads((client / "Cargo.toml").read_text(encoding="utf-8"))["package"]
    if package["name"] != CRATE or package["version"] != version:
        raise ValueError("requested version must match the Rust client manifest")
    lock = tomllib.loads((client / "Cargo.lock").read_text(encoding="utf-8"))
    own = [p for p in lock["package"] if p["name"] == CRATE and "source" not in p]
    if len(own) != 1 or own[0]["version"] != version:
        raise ValueError("Rust client lockfile must match the manifest version")


def check_package(path, version, expected=None):
    validate_version(version)
    if expected is not None and not re.fullmatch(r"[0-9a-f]{64}", expected):
        raise ValueError("expected SHA-256 must contain 64 lowercase hex digits")
    checksum = hashlib.sha256(path.read_bytes()).hexdigest()
    if expected is not None and checksum != expected:
        raise ValueError("package differs from the tested artifact")
    prefix = f"{CRATE}-{version}/"
    with tarfile.open(path, "r:gz") as archive:
        files = {}
        for member in archive.getmembers():
            name = PurePosixPath(member.name)
            if not member.name.startswith(prefix) or ".." in name.parts or "\\" in member.name:
                raise ValueError("unexpected archive path")
            if member.isdir():
                continue
            if not member.isfile() or member.name in files:
                raise ValueError("package must contain unique regular files")
            files[member.name] = member
        for name in ("Cargo.lock", "Cargo.toml", "Cargo.toml.orig", "LICENSE", "README.md", "src/lib.rs"):
            if prefix + name not in files:
                raise ValueError(f"package is missing {name}")
        for name in ("Cargo.toml", "Cargo.toml.orig"):
            package = tomllib.loads(archive.extractfile(files[prefix + name]).read().decode())["package"]
            if package["name"] != CRATE or package["version"] != version:
                raise ValueError("unexpected package name or version in archive")
    return checksum


def registry_version(version):
    validate_version(version)
    request = Request(f"https://crates.io/api/v1/crates/{CRATE}/{version}", headers={"User-Agent": USER_AGENT})
    try:
        with urlopen(request, timeout=60) as response:
            return json.load(response)["version"]
    except HTTPError as error:
        if error.code == 404:
            return None
        raise


def registry_action(published, version, checksum, require_published=False):
    if published is None:
        if require_published:
            raise ValueError("published version is not available in the registry")
        return "publish"
    if (published.get("crate") != CRATE or published.get("num") != version
            or published.get("checksum") != checksum or published.get("yanked") is not False):
        raise ValueError("existing registry version differs from the tested package or is yanked")
    return "skip"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=("metadata", "package", "registry"))
    parser.add_argument("--version", required=True)
    parser.add_argument("--package", type=Path)
    parser.add_argument("--expected-sha")
    parser.add_argument("--require-published", action="store_true")
    args = parser.parse_args()
    check_metadata(args.version)
    result = {"crate": CRATE, "version": args.version}
    if args.command != "metadata":
        if args.package is None:
            parser.error("--package is required")
        checksum = check_package(args.package, args.version, args.expected_sha)
        result["sha256"] = checksum
        if args.command == "registry":
            action = registry_action(registry_version(args.version), args.version, checksum, args.require_published)
            result["action"] = action
            if output := os.environ.get("GITHUB_OUTPUT"):
                with open(output, "a", encoding="utf-8") as stream:
                    stream.write(f"publish={'true' if action == 'publish' else 'false'}\n")
    print(json.dumps(result))


if __name__ == "__main__":
    main()
