# Releasing Yerux

Every compiler binary embeds the version from `VERSION`. Both `yerux --version` and
`yerux version` print it. To prepare a release, set the same version in `VERSION`
and `Cargo.toml`, refresh the package entry in `Cargo.lock` with `cargo check`,
and move the relevant `Unreleased` entries in `CHANGELOG.md` under the new
`## X.Y.Z` heading. Commit these changes together.

## Verify the release

On Linux x86-64, run the full test suite and build both release archives.
The shell commands use Bash. Install QEMU user-mode emulation, including
`qemu-aarch64`, to verify the cross-built compiler and its output.

```sh
cargo build --release --locked
bash tests/run_all.sh
rustup target add x86_64-unknown-linux-musl aarch64-unknown-linux-musl
scripts/release.sh dist x86_64-unknown-linux-musl
scripts/release.sh dist aarch64-unknown-linux-musl
(cd dist && sha256sum -c ./*.sha256)
```

Each archive includes the compiler, its embedded standard library,
documentation, and an offline installer. The default invocation of
`scripts/release.sh` packages a host build. The tagged Linux builds use
Rust's musl targets and LLD. Compilation through the C backend requires a C
toolchain on the machine running Yerux. On AArch64 Linux, use `--native`
when building programs that use standard-library OS services. The opt-in
`--selfhost` driver can be bootstrapped separately from a source checkout.
See [Self-hosting](SELFHOST.md).

## Publish a stable release

Linux CI builds and runs the fast suite for pull requests and the complete
suite for pushes to `main`. After verification, create an annotated
`vX.Y.Z` tag on the release commit and push that tag. The release workflow
checks it against `VERSION`, builds and validates both Linux archives, and
publishes a GitHub release. Its assets include both archives, their matching
`.sha256` files, and a copy of `VERSION` used by the installer.

## Install

```sh
curl -fsSL https://raw.githubusercontent.com/yervix10/yerux/main/install.sh | sh
curl -fsSL https://raw.githubusercontent.com/yervix10/yerux/main/install.sh | YERUX_VERSION=X.Y.Z sh
```

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
