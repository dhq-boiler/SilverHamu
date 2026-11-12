"""Guarantees for Rust release input, artifact and registry decisions."""

import hashlib
import importlib.util
import io
import json
from pathlib import Path
import tarfile
import tempfile
import unittest
from unittest.mock import patch
from urllib.error import HTTPError


spec = importlib.util.spec_from_file_location("rust_release", Path(__file__).resolve().parents[1] / "scripts/rust-client-release.py")
release = importlib.util.module_from_spec(spec)
spec.loader.exec_module(release)


class RustReleaseTest(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)

    def package(self, *, version="0.2.0", extra=(), omit=(), wrong_manifest=False):
        path = self.root / "client.crate"
        manifest = f'[package]\nname = "silverhamu-client"\nversion = "{version}"\n'.encode()
        files = {name: b"fixture" for name in ("Cargo.lock", "LICENSE", "README.md", "src/lib.rs")}
        files.update({"Cargo.toml": manifest, "Cargo.toml.orig": manifest})
        if wrong_manifest:
            files["Cargo.toml"] = manifest.replace(b"silverhamu-client", b"wrong-client")
        with tarfile.open(path, "w:gz") as archive:
            for name, content in files.items():
                if name not in omit:
                    member = tarfile.TarInfo(f"silverhamu-client-{version}/{name}")
                    member.size = len(content)
                    archive.addfile(member, io.BytesIO(content))
            for member in extra:
                archive.addfile(member, io.BytesIO(b""))
        return path

    def test_input_versions_allow_release_and_prerelease_but_reject_paths_and_shell_text(self):
        for version in ("0.2.0", "1.2.3-rc.1", "1.2.3+build.4"):
            self.assertEqual(release.validate_version(version), version)
        for version in ("", "v0.2.0", "01.2.3", "../../x", "0.2.0\n", "0.2.0;echo x", "$(id)"):
            with self.subTest(version=version), self.assertRaises(ValueError):
                release.validate_version(version)

    def test_requested_manifest_and_lockfile_versions_must_all_match(self):
        client = self.root / "clients/rust"
        client.mkdir(parents=True)
        (client / "Cargo.toml").write_text('[package]\nname="silverhamu-client"\nversion="0.2.0"')
        (client / "Cargo.lock").write_text('[[package]]\nname="silverhamu-client"\nversion="0.2.0"')
        release.check_metadata("0.2.0", self.root)
        with self.assertRaises(ValueError):
            release.check_metadata("0.2.1", self.root)
        (client / "Cargo.lock").write_text('[[package]]\nname="silverhamu-client"\nversion="0.2.1"')
        with self.assertRaises(ValueError):
            release.check_metadata("0.2.0", self.root)

    def test_tested_archive_checksum_must_match_exactly(self):
        package = self.package()
        checksum = hashlib.sha256(package.read_bytes()).hexdigest()
        self.assertEqual(release.check_package(package, "0.2.0", checksum), checksum)
        for wrong in ("0" * 64, "", "a" * 63, "a" * 65, "A" * 64):
            with self.subTest(expected=wrong), self.assertRaises(ValueError):
                release.check_package(package, "0.2.0", wrong)

    def test_wrong_archive_identity_and_missing_required_files_are_rejected(self):
        with self.assertRaises(ValueError):
            release.check_package(self.package(wrong_manifest=True), "0.2.0")
        for missing in ("Cargo.lock", "Cargo.toml", "Cargo.toml.orig", "LICENSE", "README.md", "src/lib.rs"):
            with self.subTest(missing=missing), self.assertRaises(ValueError):
                release.check_package(self.package(omit=(missing,)), "0.2.0")
        with self.assertRaises(ValueError):
            release.check_package(self.package(version="0.2.1"), "0.2.0")

    def test_unsafe_duplicate_and_linked_archive_members_are_rejected(self):
        prefix = "silverhamu-client-0.2.0/"
        for name in ("/absolute", "outside", prefix + "../escape", prefix + "a\\b", prefix + "README.md"):
            with self.subTest(name=name), self.assertRaises(ValueError):
                release.check_package(self.package(extra=(tarfile.TarInfo(name),)), "0.2.0")
        for kind in (tarfile.SYMTYPE, tarfile.LNKTYPE):
            member = tarfile.TarInfo(prefix + "link")
            member.type, member.linkname = kind, "README.md"
            with self.subTest(kind=kind), self.assertRaises(ValueError):
                release.check_package(self.package(extra=(member,)), "0.2.0")

    def test_only_absent_registry_version_allows_publishing(self):
        self.assertEqual(release.registry_action(None, "0.2.0", "a" * 64), "publish")
        with self.assertRaises(ValueError):
            release.registry_action(None, "0.2.0", "a" * 64, require_published=True)

    def test_identical_non_yanked_version_is_an_idempotent_success(self):
        published = {"crate": "silverhamu-client", "num": "0.2.0", "checksum": "a" * 64, "yanked": False}
        for required in (False, True):
            self.assertEqual(release.registry_action(published, "0.2.0", "a" * 64, required), "skip")

    def test_existing_different_or_yanked_version_never_succeeds(self):
        published = {"crate": "silverhamu-client", "num": "0.2.0", "checksum": "a" * 64, "yanked": False}
        for key, value in (("crate", "other"), ("num", "0.2.1"), ("checksum", "b" * 64),
                           ("yanked", True), ("yanked", None), ("yanked", 0)):
            with self.subTest(key=key, value=value), self.assertRaises(ValueError):
                release.registry_action({**published, key: value}, "0.2.0", "a" * 64)

    def test_registry_404_is_distinct_from_authentication_rate_limits_and_server_failures(self):
        for status in (404, 403, 429, 500):
            error = HTTPError("https://crates.io", status, "fixture", {}, None)
            with self.subTest(status=status), patch.object(release, "urlopen", side_effect=error):
                if status == 404:
                    self.assertIsNone(release.registry_version("0.2.0"))
                else:
                    with self.assertRaises(HTTPError):
                        release.registry_version("0.2.0")
        with patch.object(release, "urlopen", side_effect=TimeoutError), self.assertRaises(TimeoutError):
            release.registry_version("0.2.0")

    def test_registry_response_preserves_metadata_for_checksum_verification(self):
        document = {"version": {"crate": "silverhamu-client", "num": "0.2.0", "checksum": "a" * 64, "yanked": False}}
        with patch.object(release, "urlopen", return_value=io.BytesIO(json.dumps(document).encode())):
            self.assertEqual(release.registry_version("0.2.0"), document["version"])


if __name__ == "__main__":
    unittest.main()
