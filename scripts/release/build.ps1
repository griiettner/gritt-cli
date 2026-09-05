# Deterministic release build of the `gritt` binary plus a SHA-256 checksum.
#
# Usage: scripts/release/build.ps1 [-Target <triple>] [-Out <dir>]
#
# Mirrors build.sh: locked dependencies, SOURCE_DATE_EPOCH from the last
# commit, and remapped source and cargo paths, so two builds of one commit on
# one toolchain match byte for byte. The toolchain is pinned by
# rust-toolchain.toml and recorded in BUILD-INFO beside the checksum.
param(
  [string]$Target = (rustc -vV | Select-String '^host: ' | ForEach-Object { $_.Line.Substring(6) }),
  [string]$Out = ''
)
$ErrorActionPreference = 'Stop'

$Root = Resolve-Path (Join-Path $PSScriptRoot '..\..')
if ($Out -eq '') { $Out = Join-Path $Root "dist\$Target" }
$CargoHome = if ($env:CARGO_HOME) { $env:CARGO_HOME } else { Join-Path $HOME '.cargo' }

if (-not $env:SOURCE_DATE_EPOCH) {
  try { $env:SOURCE_DATE_EPOCH = (git -C $Root log -1 --pretty=%ct) } catch { $env:SOURCE_DATE_EPOCH = '0' }
}
$env:CARGO_INCREMENTAL = '0'
$env:RUSTFLAGS = "$($env:RUSTFLAGS) --remap-path-prefix=$Root=/build --remap-path-prefix=$CargoHome=/cargo".Trim()

Push-Location $Root
try {
  $Pinned = (Get-Content (Join-Path $Root 'rust-toolchain.toml') | Select-String '^channel = "(.*)"').Matches[0].Groups[1].Value
  $RustcVersion = (rustc -vV | Select-String '^release: ' | ForEach-Object { $_.Line.Substring(9) })
  $ExpectedCompiler = (rustup run $Pinned rustc -vV) -join "`n"
  if ($LASTEXITCODE -ne 0) { throw "cannot read the pinned toolchain $Pinned" }
  if (((rustc -vV) -join "`n") -ne $ExpectedCompiler) { throw "rust-toolchain.toml pins $Pinned but rustc is $RustcVersion; run: rustup toolchain install $Pinned" }
  $Commit = try { (git -C $Root rev-parse HEAD) } catch { 'unknown' }
  cargo build --release --locked --bin gritt --target $Target
  if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }
  New-Item -ItemType Directory -Force -Path $Out | Out-Null
  $Bin = if ($Target -like '*windows*') { 'gritt.exe' } else { 'gritt' }
  Copy-Item (Join-Path $Root "target\$Target\release\$Bin") (Join-Path $Out $Bin) -Force
  $Hash = (Get-FileHash -Algorithm SHA256 (Join-Path $Out $Bin)).Hash.ToLower()
  "$Hash  $Bin`n" | Set-Content -NoNewline -Path (Join-Path $Out 'SHA256SUMS')
  @("rustc $RustcVersion", "toolchain $Pinned", "target $Target", "commit $Commit", "source_date_epoch $($env:SOURCE_DATE_EPOCH)") -join "`n" | Set-Content -Path (Join-Path $Out 'BUILD-INFO')
  Get-Content (Join-Path $Out 'SHA256SUMS')
  Get-Content (Join-Path $Out 'BUILD-INFO')
} finally {
  Pop-Location
}
