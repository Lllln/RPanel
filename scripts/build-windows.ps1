#Requires -Version 5.1

$ErrorActionPreference = 'Stop'

if ($env:OS -ne 'Windows_NT') {
    throw 'Run this packaging script on Windows with Rust and the MSVC C++ Build Tools installed.'
}

$projectRoot = Split-Path -Parent $PSScriptRoot
$target = 'x86_64-pc-windows-msvc'
$targetDir = Join-Path $projectRoot 'target'
$exePath = Join-Path $targetDir "$target/release/rpanel.exe"
$distDir = Join-Path $projectRoot 'dist'
$archivePath = Join-Path $distDir 'rpanel-windows-x64.zip'

$previousFlags = $env:CARGO_ENCODED_RUSTFLAGS
Push-Location -LiteralPath $projectRoot
try {
    $env:CARGO_ENCODED_RUSTFLAGS = '-Ctarget-feature=+crt-static'
    & cargo build --locked --release -p rpanel --target $target --target-dir $targetDir --bin rpanel
    if ($LASTEXITCODE -ne 0) {
        throw "Cargo build failed (exit code $LASTEXITCODE)."
    }
} finally {
    $env:CARGO_ENCODED_RUSTFLAGS = $previousFlags
    Pop-Location
}

if (-not (Test-Path -LiteralPath $exePath -PathType Leaf)) {
    throw "Release executable not found: $exePath"
}

New-Item -ItemType Directory -Path $distDir -Force | Out-Null
$stagedArchive = Join-Path $distDir ('.rpanel-' + [Guid]::NewGuid().ToString('N') + '.zip')
try {
    Compress-Archive -LiteralPath $exePath -DestinationPath $stagedArchive
    if ([IO.File]::Exists($archivePath)) {
        [IO.File]::Replace($stagedArchive, $archivePath, [NullString]::Value)
    } else {
        [IO.File]::Move($stagedArchive, $archivePath)
    }
} finally {
    if (Test-Path -LiteralPath $stagedArchive) {
        Remove-Item -LiteralPath $stagedArchive -Force
    }
}

Write-Output "Executable: $exePath"
Write-Output "Archive:    $archivePath"
