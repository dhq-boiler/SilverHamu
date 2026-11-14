# SBOM and vulnerability management

SilverHamu uses Syft to inventory dependencies and Grype to match the inventory
against current vulnerability data. CI preserves Syft JSON for rescanning and
CycloneDX JSON for other SBOM consumers. The scanner versions and Anchore Actions
are pinned in `.github/actions/scan-sbom/action.yml`; update them together after
validation. Vulnerability data is refreshed on each scan rather than pinned.

## Find the result

| When | Workflow | Inventory and evidence |
|---|---|---|
| Pull request, main push, manual run, daily at 05:17 JST | Dependency security | Rust service/client lockfiles, resolved Java runtime JARs, resolved Owner/NuGet lockfiles; `security-source-*` artifacts |
| Verified release build | Release SilverHamu | Service and SQL host on Linux/Windows, Windows Owner, Docker runtime image; `security-*` artifacts |
| Daily at 05:47 JST or manual run | Rescan released SBOMs | Latest release's four Syft inventories, checked against its SHA256SUMS; `security-rescan-*` artifacts |

Scheduled workflows run on the default branch and may start later than their
scheduled time. `Rescan released SBOMs` accepts a `tag` input to investigate an
older release without rebuilding it. It fails if any inventory is missing,
empty or inconsistent with the release checksums. Releases published before this
workflow was introduced have no SBOMs; publish a newly verified release to start
the release rescan cycle. Do not relabel a current source inventory as an old
release's inventory.

The Actions job summary orders findings by severity and shows the package,
installed version, fixed versions and advisory ID. The artifact contains the
complete `.grype.json` report, including matching evidence and database
information. Scan failures retain available inventories and reports for 90 days
(subject to repository retention limits). Database/download/scanner errors also
fail the job; a missing report is never described as a clean scan.

High and Critical findings fail CI and block release publication, **including
findings without a fix**. Lower severity findings remain in the report. No
findings are silently allowlisted. Configure `Dependency security`'s three
`Dependencies (...)` checks as required in branch protection if merging should
also be blocked; adding a workflow alone does not change repository rules.
Watch failed scheduled runs in GitHub Actions notifications.

## Release assets and coverage

Each of the four archives has sidecars named `<archive>.syft.json` and
`<archive>.cdx.json`. `SHA256SUMS` covers all four archives and all eight SBOMs.
Release publication rejects missing or extra files. Security diagnostic artifacts
are kept separate from the `release-*` artifacts used for publication.

| Inventory | Includes | Interpretation and limits |
|---|---|---|
| Rust source | Both committed Cargo.lock files, including transitive dependencies | Conservative inventory; includes optional features, build dependencies and other platforms, not only linked code |
| Java source | Maven-resolved runtime dependency JARs including transitives | Excludes test-only JUnit and build plugins |
| .NET source | Owner's restored NuGet lockfiles, including referenced SQL project | Windows x64; generated after MAUI installation; SDK/workload tooling is not a runtime inventory |
| Native service/SQL distribution | Packaged binaries, self-contained SQL host DLLs and deps.json, root Cargo.lock | Lockfile covers the native Rust dependency candidates; .NET inventory is taken from published files |
| Windows Owner | Published DLLs/deps.json, bundled runtime, root Cargo.lock | Includes native dependency candidates, NuGet and discoverable bundled runtime components |
| Docker runtime | Final image's Wolfi APK packages and root Cargo.lock at `/usr/share/silverhamu/Cargo.lock` | Rust lockfile cataloger is explicitly enabled for image scans; build-stage tooling is excluded |

Rust binaries currently do not embed cargo-auditable dependency metadata. A
lockfile finding is evidence of a dependency candidate, not proof that the
vulnerable code was linked or is reachable. Scanners can also miss runtime
components or advisories; an empty finding list is not a security guarantee.
Node, Python, Go and Ruby clients currently declare no third-party runtime
dependencies. Revisit coverage when adding dependencies to them. Test tools,
benchmarks, SDKs and CI build dependencies are outside this runtime-focused
inventory. Source input is isolated to avoid including historical benchmark
lockfiles, local artifacts or unrelated files.

## Respond to a finding

1. Open the failed job summary and download its `security-*` artifact. Record the
   release tag or source commit, advisory ID, package/version and affected
   platform in Kudaki. Group findings by the component or upgrade decision so
   repeat scans do not create duplicate tasks.
2. Read the advisory and `.grype.json` matching evidence. For Rust lockfile
   findings, confirm the release feature/target dependency path. For .NET and
   Wolfi, confirm the bundled runtime or OS package version. Distinguish
   presence from exploitability; retain the evidence for either conclusion.
3. Upgrade the responsible direct dependency or runtime/base image and refresh
   its resolved dependencies. Rebuild, regenerate SBOMs and rerun the scan.
   Rescanning an existing SBOM updates findings but does not fix the software.
   The Docker runtime image is pinned by digest. To update it, resolve a new
   official `cgr.dev/chainguard/wolfi-base` digest, inspect its inventory, then
   verify both vulnerability checks and the container behavior tests before
   changing the pin. Preserve the APK package database for accurate matching.
4. If a finding is inapplicable or a fix must be deferred, record the rationale,
   owner, compensating measures and review deadline. This pipeline has no
   automatic exception mechanism; any future VEX/exception policy needs an
   explicit reviewed change rather than disabling the release gate.
5. Publish corrected binaries and new SBOMs together. Retain old release
   inventories so affected versions can still be identified quickly.

## Run locally

Install the pinned Syft and Grype versions from their official releases and
verify their checksums. For a release, scan its prepared core/Owner directory or
the built Docker image; for Rust source, use a clean directory containing the
current service/client manifests and lockfiles.

```sh
syft dir:artifacts/security/input --select-catalogers +rust-cargo-lock-cataloger \
  -o syft-json=artifacts/security/local.syft.json \
  -o cyclonedx-json=artifacts/security/local.cdx.json
grype sbom:artifacts/security/local.syft.json --fail-on high \
  -o json --file artifacts/security/local.grype.json
python scripts/security-summary.py artifacts/security/local.grype.json
```

To rescan a downloaded release SBOM, verify it against that release's
`SHA256SUMS`, then run the Grype command with its `.syft.json` path. No compiler,
source checkout of the old release or application startup is needed. Syft JSON
is the primary rescan format because it preserves the generator's matching
metadata; CycloneDX is also shipped for interoperability.

References: [Syft](https://github.com/anchore/syft),
[Grype](https://github.com/anchore/grype),
[Rust catalogers](https://oss.anchore.com/docs/capabilities/rust/),
[.NET catalogers](https://oss.anchore.com/docs/capabilities/dotnet/).
