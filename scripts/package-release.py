"""Validate release metadata and package only explicitly selected build outputs."""

import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import sys
import tarfile
import tomllib
import xml.etree.ElementTree as ET
import zipfile

ROOT = Path(__file__).resolve().parent.parent
OUT = ROOT / "artifacts/release"
DOWNLOADS = OUT / "downloads"


def metadata():
    cargo = tomllib.loads((ROOT / "Cargo.toml").read_text(encoding="utf-8"))
    version = cargo["package"]["version"]
    tag = os.environ["RELEASE_TAG"]
    if not re.fullmatch(r"v\d+\.\d+\.\d+", tag) or tag != "v" + version:
        raise ValueError("Release tag must match the Cargo package version")
    lock = tomllib.loads((ROOT / "Cargo.lock").read_text(encoding="utf-8"))
    assert next(p["version"] for p in lock["package"] if p["name"] == "silverhamu") == version
    owner = ET.parse(ROOT / "apps/SilverHamu.Owner/SilverHamu.Owner.csproj")
    assert owner.findtext(".//ApplicationDisplayVersion") == version
    assert owner.findtext(".//Version") == version
    for project in ("SilverHamu.Sql", "SilverHamu.Sql.Host"):
        assert ET.parse(ROOT / f"dotnet/{project}/{project}.csproj").findtext(".//Version") == version
    assert (ROOT / f"docs/releases/{tag}.md").is_file()
    return version, tag


def require(path):
    if not path.is_file() or path.stat().st_size == 0:
        raise FileNotFoundError(path)
    return path


def archive(directory, target):
    files = sorted(p for p in directory.rglob("*") if p.is_file())
    assert files
    if target.suffix == ".zip":
        with zipfile.ZipFile(target, "w", compression=zipfile.ZIP_DEFLATED) as output:
            for path in files:
                output.write(path, path.relative_to(directory).as_posix())
        with zipfile.ZipFile(target) as output:
            assert output.testzip() is None
    else:
        with tarfile.open(target, "w:gz") as output:
            for path in files:
                output.add(path, arcname=path.relative_to(directory).as_posix())
        with tarfile.open(target) as output:
            assert len(output.getmembers()) == len(files)


def package(version, tag):
    windows = sys.platform == "win32"
    platform = "windows-x64" if windows else "linux-x64"
    suffix = ".exe" if windows else ""
    library = "silverhamu.dll" if windows else "libsilverhamu.so"
    staging = OUT / f"core-{platform}"
    staging.mkdir(parents=True, exist_ok=False)
    DOWNLOADS.mkdir(parents=True, exist_ok=True)
    for name in ("silverhamu" + suffix, "silverhamu-server" + suffix, library):
        shutil.copy2(require(ROOT / "target/release" / name), staging / name)
    host = OUT / "host"
    require(host / ("SilverHamu.Sql.Host" + suffix))
    require(host / library)
    shutil.copytree(host, staging / "sql-host")
    for name in ("LICENSE", "README.md"):
        shutil.copy2(require(ROOT / name), staging / name)
    shutil.copytree(ROOT / "docs", staging / "docs", ignore=shutil.ignore_patterns("benchmarks", "*.wbs.yaml"))
    (staging / "scripts").mkdir()
    shutil.copy2(ROOT / "scripts/create-silverhamu-token.py", staging / "scripts")
    (staging / "README.txt").write_text(
        f"SilverHamu {version}\n\n"
        "Create a private token file with Python 3:\n"
        "  python scripts/create-silverhamu-token.py token.txt\n\n"
        "Start locally (Windows: use silverhamu-server.exe; Linux: ./silverhamu-server):\n"
        "  silverhamu-server --db state.ndb --init --bind 127.0.0.1:7400 --token-file token.txt\n\n"
        "For server-side SQL, configure CATDB_SQL_HOST with the absolute path to\n"
        "sql-host/SilverHamu.Sql.Host.exe (Windows) or sql-host/SilverHamu.Sql.Host (Linux).\n"
        "The SQL host includes its .NET runtime. See docs/service.md and docs/sql.md.\n"
        "The backup/restore utility only operates on closed databases.\n"
        "Native consumers must now load silverhamu.dll / libsilverhamu.so; C ABI is unchanged.\n",
        encoding="utf-8",
    )
    extension = ".zip" if windows else ".tar.gz"
    archive(staging, DOWNLOADS / f"silverhamu-{tag}-{platform}{extension}")
    if windows:
        owner = OUT / "owner"
        for name in ("SilverHamu.Owner.exe", "SilverHamu.Owner.dll", library, "coreclr.dll", "Microsoft.UI.Xaml.dll"):
            require(owner / name)
        shutil.copy2(ROOT / "LICENSE", owner / "LICENSE")
        shutil.copy2(ROOT / "apps/SilverHamu.Owner/README.md", owner / "README.md")
        shutil.copy2(ROOT / f"docs/releases/{tag}.md", owner / "RELEASE-NOTES.md")
        archive(owner, DOWNLOADS / f"silverhamu-owner-{tag}-windows-x64.zip")
    print(json.dumps({"version": version, "files": [p.name for p in sorted(DOWNLOADS.iterdir())]}))


def checksums(version, tag):
    expected = {
        f"silverhamu-{tag}-windows-x64.zip",
        f"silverhamu-owner-{tag}-windows-x64.zip",
        f"silverhamu-{tag}-linux-x64.tar.gz",
        f"silverhamu-{version}-linux-amd64-docker.tar.gz",
    }
    actual = {p.name for p in DOWNLOADS.iterdir()}
    if actual != expected:
        raise ValueError(f"Download set mismatch: missing={expected - actual}, extra={actual - expected}")
    lines = []
    for name in sorted(expected):
        with require(DOWNLOADS / name).open("rb") as source:
            digest = hashlib.file_digest(source, "sha256").hexdigest()
        lines.append(f"{digest}  {name}\n")
    (DOWNLOADS / "SHA256SUMS").write_text("".join(lines), encoding="ascii")
    print("".join(lines), end="")


if __name__ == "__main__":
    version, tag = metadata()
    {"check": lambda *_: print(tag), "package": package, "checksums": checksums}[sys.argv[1]](version, tag)
