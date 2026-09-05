# Reproducible builds

Releases are one native binary per platform with a SHA-256 checksum. There
are no signed installers; verification is by checksum and by rebuilding.

## Targets

| Platform | Target | Built on |
| --- | --- | --- |
| Linux x86_64 | `x86_64-unknown-linux-gnu` | ubuntu-latest |
| Linux aarch64 | `aarch64-unknown-linux-gnu` | ubuntu-24.04-arm |
| Windows x86_64 | `x86_64-pc-windows-msvc` | windows-latest |
| macOS x86_64 | `x86_64-apple-darwin` | macos-13 |
| macOS aarch64 | `aarch64-apple-darwin` | macos-latest |

Only the Rust toolchain and the platform's C compiler are needed. TLS is
rustls with `ring` as the selected crypto provider. `ring` compiles a small
amount of C and assembly through the `cc` crate, which every target's
default C compiler handles, so no target needs CMake or NASM.

## Toolchain

`rust-toolchain.toml` at the repository root pins the exact compiler
(currently `nightly-2026-09-03`). `cargo` and `rustup` read it automatically, the
`product` workflow installs that version for every job, and both release
scripts refuse to run with any other `rustc`. Each build writes a
`BUILD-INFO` file beside `SHA256SUMS` with the `rustc` release, the
toolchain channel, the target, the commit, and `SOURCE_DATE_EPOCH`; the
release job uploads it with the binary. The dated nightly enables Cargo's
`build.artifact-dir` setting, which places the executable at the repository
root during an ordinary `cargo build --release --locked`.

## How a build is made deterministic

`scripts/release/build.sh` (and `build.ps1` on Windows) run:

```text
cargo build --release --locked --bin gritt --target <triple>
```

with `SOURCE_DATE_EPOCH` set to the commit time, `CARGO_INCREMENTAL=0`,
and `RUSTFLAGS` remapping the source tree to `/build` and the cargo home
to `/cargo`. The `release` job in `.github/workflows/product.yml` runs the
script twice from clean target directories and fails unless the two
checksums and the two `BUILD-INFO` files match, then uploads the binary,
`SHA256SUMS`, and `BUILD-INFO`.

## Verifying a download

```bash
shasum -a 256 -c SHA256SUMS      # macOS
sha256sum -c SHA256SUMS           # Linux
Get-FileHash gritt.exe -Algorithm SHA256   # Windows, compare by eye
```

## Rebuilding to verify

Check out the release commit. Its `rust-toolchain.toml` names the
compiler, and the published `BUILD-INFO` repeats it. Install that version
and run the script for your target:

```bash
rustup toolchain install "$(sed -n 's/^channel = "\(.*\)"/\1/p' rust-toolchain.toml)"
scripts/release/build.sh aarch64-apple-darwin dist/mine
cat dist/mine/SHA256SUMS dist/mine/BUILD-INFO
```

The checksum must equal the published one for that target. Different
toolchain versions produce different bytes, which is why the script stops
when `rustc` does not match the pin.

## Evidence

On the development machine (macOS aarch64, Rust 1.93.1) two consecutive
clean builds of the same commit produced identical checksums. Cross-target
`cargo check` from macOS to Linux and Windows fails on the C build scripts
of `ring` and `zstd-sys` because no cross C compiler is installed there;
those targets are built and tested by the CI matrix on their own runners.
