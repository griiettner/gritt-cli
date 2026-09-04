# Deterministic release build of the `gritt` binary plus a SHA-256 checksum.
#
# Usage: scripts/release/build.ps1 [-Target <triple>] [-Out <dir>]
#
# Mirrors build.sh: locked dependencies, SOURCE_DATE_EPOCH from the last
# commit, and remapped source and cargo paths, so two builds of one commit on
# one toolchain match byte for byte.
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
  cargo build --release --locked --bin gritt --target $Target
  if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }
  New-Item -ItemType Directory -Force -Path $Out | Out-Null
  $Bin = if ($Target -like '*windows*') { 'gritt.exe' } else { 'gritt' }
  Copy-Item (Join-Path $Root "target\$Target\release\$Bin") (Join-Path $Out $Bin) -Force
  $Hash = (Get-FileHash -Algorithm SHA256 (Join-Path $Out $Bin)).Hash.ToLower()
  "$Hash  $Bin" | Set-Content -NoNewline -Path (Join-Path $Out 'SHA256SUMS')
  Get-Content (Join-Path $Out 'SHA256SUMS')
} finally {
  Pop-Location
}
