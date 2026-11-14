"""Verify released inventories before reusing them for vulnerability matching."""

import hashlib
import json
import os
from pathlib import Path
import re
import sys


def verify(directory, tag):
    if not re.fullmatch(r"v\d+\.\d+\.\d+", tag):
        raise ValueError("Expected a release tag such as v0.2.0")
    archives = {
        f"silverhamu-{tag}-windows-x64.zip",
        f"silverhamu-owner-{tag}-windows-x64.zip",
        f"silverhamu-{tag}-linux-x64.tar.gz",
        f"silverhamu-{tag[1:]}-linux-amd64-docker.tar.gz",
    }
    expected = {name + ".syft.json" for name in archives}
    actual = {p.name for p in directory.glob("*.syft.json")}
    if actual != expected:
        raise ValueError(f"Release SBOM set mismatch: missing={expected - actual}, extra={actual - expected}. "
                         "Releases published before SBOM support need a new verified release.")
    checksums = {}
    for line in (directory / "SHA256SUMS").read_text(encoding="ascii").splitlines():
        digest, name = line.split("  ", 1)
        if not re.fullmatch(r"[0-9a-f]{64}", digest) or name in checksums:
            raise ValueError("Invalid or duplicate release checksum")
        checksums[name] = digest
    for name in sorted(expected):
        path = directory / name
        with path.open("rb") as source:
            digest = hashlib.file_digest(source, "sha256").hexdigest()
        if checksums.get(name) != digest:
            raise ValueError(f"Release SBOM checksum mismatch: {name}")
        document = json.loads(path.read_text(encoding="utf-8"))
        if not document.get("artifacts") or not document.get("schema", {}).get("version"):
            raise ValueError(f"Missing package inventory or Syft schema: {name}")
    return {"name": sorted(archives)}


if __name__ == "__main__":
    matrix = verify(Path(sys.argv[1]), sys.argv[2])
    if os.environ.get("GITHUB_OUTPUT"):
        with Path(os.environ["GITHUB_OUTPUT"]).open("a", encoding="utf-8") as output:
            output.write("matrix=" + json.dumps(matrix) + "\n")
    print(json.dumps(matrix))
