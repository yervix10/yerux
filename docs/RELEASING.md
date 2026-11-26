# Releasing Yerux

Every compiler binary embeds the version from `VERSION`. Both `yerux --version` and
`yerux version` print it. To prepare a release, set the same version in `VERSION`
and `Cargo.toml`, refresh the package entry in `Cargo.lock` with `cargo check`,
and move the relevant `Unreleased` entries in `CHANGELOG.md` under the new
`## X.Y.Z` or `## X.Y.Z-rcN` heading. Start candidate numbers at 1. Commit
these changes together.

## Verify the release

On Linux x86-64, run the full test suite and build both release archives.
The shell commands use Bash. Install QEMU user-mode emulation, including
`qemu-aarch64`, to verify the cross-built compiler and its output.

```sh
cargo build --release --locked
setsid nohup bash tests/run_all.sh > /tmp/yerux-release-suite.log 2>&1 < /dev/null &
rustup target add x86_64-unknown-linux-musl aarch64-unknown-linux-musl
scripts/release.sh dist x86_64-unknown-linux-musl
scripts/release.sh dist aarch64-unknown-linux-musl
(cd dist && sha256sum -c ./*.sha256)
```

Wait for `failures: 0` in the suite log. If `actionlint` is available, run it
from the checkout to check both workflows. Run `bash tests/dist.sh` to exercise
the offline and download installers, the version commands, and checksum rejection.

Each archive includes the compiler, its embedded standard library,
documentation, and an offline installer. The default invocation of
`scripts/release.sh` packages a host build. The tagged Linux builds use
Rust's musl targets and LLD. Compilation through the C backend requires a C
toolchain on the machine running Yerux. On AArch64 Linux, use `--native`
when building programs that use standard-library OS services. The opt-in
`--selfhost` driver can be bootstrapped separately from a source checkout.
See [Self-hosting](SELFHOST.md).

## Publish a release

Linux CI builds and runs the fast suite for pull requests and the complete
suite for pushes to `main`. After verification, create and push an annotated
`vX.Y.Z` or `vX.Y.Z-rcN` tag on the release commit when authorized to publish.
The release workflow checks the tag against `VERSION`, builds and validates
both Linux archives, and publishes a GitHub release. Its five assets are the
two archives, their two `.sha256` files and the installer’s `VERSION` asset.
A candidate tag publishes a prerelease with `--latest=false`.

## Checks on GitHub

- Confirm the annotated tag triggers the workflow, and that the hosted runner
  can install the Rust and QEMU dependencies and build both Linux archives.
- Confirm the job token can create the release, generate notes and upload all
  five assets under the repository’s rules. Check that GitHub labels the RC
  as a prerelease and keeps the previous stable release at `/releases/latest`.
  In a repository with no stable release, the latest-stable URL can return 404.
- Download the GitHub-hosted assets, verify both checksums, and run the pinned
  installer from the public release URL on Linux x86-64 and AArch64. The unpinned
  installer should still select the previous stable release if one exists.

## Install

```sh
curl -fsSL https://raw.githubusercontent.com/yervix10/yerux/main/install.sh | sh
curl -fsSL https://raw.githubusercontent.com/yervix10/yerux/main/install.sh | YERUX_VERSION=1.0.0 sh
```

The second command selects the candidate explicitly once GitHub publishes it.

The installer needs `curl`, `tar`, and `sha256sum` for downloads. It
selects the Linux archive for the current CPU, verifies its SHA-256, and
installs `yerux` in `~/.local/bin`. Use `sh -s -- --prefix DIR` to install
into `DIR/bin`. For offline installation, verify the downloaded archive
with its `.sha256` file, extract it, and run the included `./install.sh`.
To test against a mirror or local fixtures, set `YERUX_RELEASE_BASE_URL` to
a URL with the same GitHub Releases layout. A direct archive URL can be
supplied with `YERUX_URL`. Keep the release archive filename and its
`.sha256` sidecar, or supply the version with `YERUX_VERSION` for a
renamed archive and a trusted digest with `YERUX_SHA256`.
