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
rustls with the pure-Rust `ring` provider, chosen so that no target needs
CMake or NASM.

## How a build is made deterministic

`scripts/release/build.sh` (and `build.ps1` on Windows) run:

```text
cargo build --release --locked --bin gritt --target <triple>
```

with `SOURCE_DATE_EPOCH` set to the commit time, `CARGO_INCREMENTAL=0`,
and `RUSTFLAGS` remapping the source tree to `/build` and the cargo home
to `/cargo`. The `release` job in `.github/workflows/product.yml` runs the
script twice from clean target directories and fails unless the two
checksums match, then uploads the binary and a `SHA256SUMS` file.

## Verifying a download

```bash
shasum -a 256 -c SHA256SUMS      # macOS
sha256sum -c SHA256SUMS           # Linux
Get-FileHash gritt.exe -Algorithm SHA256   # Windows, compare by eye
```

## Rebuilding to verify

Check out the release commit with the same stable toolchain and run the
script for your target:

```bash
scripts/release/build.sh aarch64-apple-darwin dist/mine
cat dist/mine/SHA256SUMS
```

The checksum must equal the published one for that target. Different
toolchain versions produce different bytes, so match the toolchain the
release notes name.

## Evidence

On the development machine (macOS aarch64, Rust 1.93.1) two consecutive
clean builds of the same commit produced identical checksums. Cross-target
`cargo check` from macOS to Linux and Windows fails on the C build scripts
of `ring` and `zstd-sys` because no cross C compiler is installed there;
those targets are built and tested by the CI matrix on their own runners.
