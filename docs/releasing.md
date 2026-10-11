# Releasing

Releases are automated from Conventional Commits. Nobody bumps versions by hand.

1. **Commit with Conventional Commits.** PR titles are linted by `pr-title.yml` and
   squash-merged titles become the commit messages on `main`: `feat` (minor bump while
   the project is 0.x), `fix` and `perf` (patch), `feat!` or a `BREAKING CHANGE:` footer
   (also minor while pre-1.0), other types appear in the changelog only where configured.
2. **release-please opens a release PR.** On every push to `main`, `release-please.yml`
   (config: `release-please-config.json`, state: `.release-please-manifest.json`) opens or
   updates one PR that bumps `[workspace.package] version` in `Cargo.toml`, refreshes
   `Cargo.lock` and writes `CHANGELOG.md`. The first `feat` release becomes `0.1.0`.
3. **Merge the release PR.** release-please creates the tag `v<version>` and the GitHub
   release with the changelog as notes.
4. **`release.yml` attaches binaries.** The tag triggers it (for tags created with
   `GITHUB_TOKEN`, `release-please.yml` starts it with `workflow_dispatch`). Each platform
   runs `cargo xtask dist --target <triple>` and uploads to the release, plus `SHA256SUMS`:

   | Platform | Artifacts |
   |---|---|
   | Linux x86_64 | `nexus-<v>-linux-x86_64.tar.gz`, `nexus_<v>_amd64.deb`, `nexus-<v>-linux-x86_64.AppImage` |
   | macOS aarch64 / x86_64 | `nexus-<v>-macos-<arch>.tar.gz`, `nexus-<v>-macos-<arch>.dmg` (contains `Nexus.app`) |
   | Windows x86_64 | `nexus-<v>-windows-x86_64.zip`, `nexus-<v>-windows-x86_64.msi` |

   Binaries are **unsigned**: macOS builds are ad-hoc signed only (Gatekeeper asks for
   confirmation) and the MSI is not Authenticode-signed (SmartScreen warns).

To rebuild a release's assets, run **Release** manually with the tag (`workflow_dispatch`);
assets are uploaded with `--clobber`.

## Setup notes

- Repository settings must allow GitHub Actions to create pull requests (Settings ->
  Actions -> General), otherwise release-please cannot open the release PR.
- Optional secret `RELEASE_PLEASE_TOKEN` (PAT or GitHub App token with contents and
  pull-requests write): with it CI runs on the release PR and the tag push triggers
  `release.yml` directly. Without it the workflows fall back to `GITHUB_TOKEN`.

## Packaging locally

`cargo xtask dist [--target <triple>] [--version <v>]` builds `nexus` in release mode and
writes archives, installers for the host OS, and a `.sha256` per artifact into `dist/`.
Needs `dpkg-deb` and `appimagetool` (`APPIMAGETOOL`) on Linux, `codesign` and `hdiutil` on
macOS, and WiX v5 (`wix`) on Windows. It fails clearly while package `nexus` does not exist.
Optional icons are read from `assets/icon/nexus.{png,icns}` when present.
