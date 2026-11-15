param(
    [Parameter(Mandatory=$true)][string]$Cxx,
    [Parameter(Mandatory=$true)][string]$Iscc,
    [string]$DirectMLPackage
)
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
$native = Split-Path -Parent $PSScriptRoot
$root = Split-Path -Parent $native
$manifest = Join-Path $native 'Cargo.toml'
$out = Join-Path $native 'target\release-assets\v2.1.0'
$stage = Join-Path $native ('target\package-stage-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Force -Path $out,$stage | Out-Null
foreach ($name in @('runtime','runtime-dml','models','licenses','licenses\rust-crates')) {
    New-Item -ItemType Directory -Force -Path (Join-Path $stage $name) | Out-Null
}

# setup.ps1 installs the verified CPU runtime and model files. Re-run it first
# on a new machine. Cargo may use an ASCII CARGO_TARGET_DIR on MinGW systems.
& cargo build --release --locked --offline --manifest-path $manifest --bin voice-isolation-native -j 1
if ($LASTEXITCODE -ne 0) { throw 'Rust release build failed' }
$target = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $native 'target' }
if (![IO.Path]::IsPathRooted($target)) { $target = Join-Path (Get-Location) $target }
$exe = Join-Path $target 'release\voice-isolation-native.exe'
Copy-Item -LiteralPath $exe -Destination (Join-Path $stage 'VoiceIsolation.exe')

$expected = @{
    'enroll.onnx'='5D06048C156BB8CD422D29848E6D38FB26A7B39F6F7FD92ECCF2A6ED1CE003C6'
    'chunk_int8.onnx'='DF61A9D0062B0ADBCF1182302B1EB8B815F15AC47EA4923E8E164C1AFF64121D'
    'gtcrn_simple.onnx'='E77603AC0C23DAC3227DD2D7135B3A585CBEE2679048AECFA886657D3AE1B534'
    'dpdfnet2.onnx'='CE35D6025FC71DF0EF10D1540E1B7916837BBFE5F6896DEB744508D2CAD487A9'
}
foreach ($entry in $expected.GetEnumerator()) {
    $file = Join-Path $root "models\$($entry.Key)"
    if (!(Test-Path -LiteralPath $file -PathType Leaf) -or (Get-FileHash -LiteralPath $file -Algorithm SHA256).Hash -ne $entry.Value) { throw "Missing or mismatched model: $file" }
    Copy-Item -LiteralPath $file -Destination (Join-Path $stage 'models')
}
$expectedRuntime = @{
    'onnxruntime.dll'='422D776AB0E3218260F7F628FCB84606AAA5C21116F720C8619A6DA5E0B2E0F9'
    'onnxruntime_providers_shared.dll'='0190137DEE4933261C065C5D030C3568A68C7AA43CAD942B4726A07011738BC2'
    'sherpa-onnx-c-api.dll'='71728FB12B3A15A9AB7C4961FCEAFC6DD802F7FC038F461F5EA59E07D7104BEC'
    'sherpa-onnx-cxx-api.dll'='471D9AF349F1CB7E057FE64A95C3F8CF59FA1AD842CDCE053A0D3656298D679E'
}
foreach ($entry in $expectedRuntime.GetEnumerator()) {
    $file = Join-Path $native "target\release\runtime\$($entry.Key)"
    if (!(Test-Path -LiteralPath $file -PathType Leaf) -or (Get-FileHash -LiteralPath $file -Algorithm SHA256).Hash -ne $entry.Value) { throw "Missing or mismatched CPU runtime: $file" }
    Copy-Item -LiteralPath $file -Destination (Join-Path $stage 'runtime')
}
$gpuArgs = @{ Cxx = $Cxx; Destination = (Join-Path $stage 'runtime-dml') }
if ($DirectMLPackage) { $gpuArgs.Package = $DirectMLPackage }
& (Join-Path $native 'setup-gpu.ps1') @gpuArgs
Copy-Item -LiteralPath (Join-Path $root 'README.md'),(Join-Path $root 'README.en.md'),(Join-Path $root 'README.zh-CN.md'),(Join-Path $root 'CHANGELOG.md') -Destination $stage
Copy-Item -Path (Join-Path $PSScriptRoot 'licenses\*') -Destination (Join-Path $stage 'licenses') -Recurse

# Include the pinned Rust dependency metadata and the license/notice files in
# each downloaded crate. Include nested font notices bundled by egui as well.
$hostLine = (& rustc -vV | Where-Object { $_ -like 'host: *' })
if (!$hostLine) { throw 'Could not determine Rust host target' }
$targetTriple = $hostLine.Substring(6).Trim()
$json = & cargo metadata --locked --offline --format-version 1 --filter-platform $targetTriple --manifest-path $manifest
if ($LASTEXITCODE -ne 0) { throw 'Could not collect Rust dependency metadata' }
$meta = $json | ConvertFrom-Json
$index = New-Object 'System.Collections.Generic.List[string]'
$index.Add('Name | Version | License | Upstream')
foreach ($pkg in ($meta.packages | Where-Object { $_.source -like 'registry+*' } | Sort-Object name,version)) {
    $index.Add("$($pkg.name) | $($pkg.version) | $($pkg.license) | $($pkg.repository)")
    $crateRoot = Split-Path -Parent $pkg.manifest_path
    $licenseDest = Join-Path $stage "licenses\rust-crates\$($pkg.name)-$($pkg.version)"
    $files = @(Get-ChildItem -LiteralPath $crateRoot -File | Where-Object { $_.Name -match '^(LICENSE|LICENCE|COPYING|NOTICE|COPYRIGHT)' })
    if ($pkg.license_file) {
        $declared = Join-Path $crateRoot $pkg.license_file
        if (Test-Path -LiteralPath $declared -PathType Leaf) { $files += Get-Item -LiteralPath $declared }
    }
    if ($pkg.name -eq 'epaint_default_fonts') {
        $files += @(Get-ChildItem -LiteralPath (Join-Path $crateRoot 'fonts') -Filter '*.txt' -File)
    }
    if ($files.Count) {
        New-Item -ItemType Directory -Force -Path $licenseDest | Out-Null
        $files | Copy-Item -Destination $licenseDest -Force
    }
}
[IO.File]::WriteAllLines((Join-Path $stage 'licenses\rust-crates\INDEX.txt'),$index,[Text.UTF8Encoding]::new($false))

$zip = Join-Path $out 'VoiceIsolation-v2.1.0-win-x64.zip'
$installer = Join-Path $out 'VoiceIsolation-v2.1.0-win-x64-setup.exe'
foreach ($asset in @($zip,$installer,(Join-Path $out 'SHA256SUMS.txt'))) {
    if (Test-Path -LiteralPath $asset) { throw "Asset already exists: $asset; move the old asset before rebuilding" }
}
Compress-Archive -Path (Join-Path $stage '*') -DestinationPath $zip -CompressionLevel Optimal
$compiler = (Get-Command $Iscc -ErrorAction Stop).Source
& $compiler "/DSourceDir=$stage" "/DOutputDir=$out" (Join-Path $PSScriptRoot 'setup.iss')
if ($LASTEXITCODE -ne 0) { throw 'Inno Setup build failed' }
$checksums = @($zip,$installer) | ForEach-Object { $h=Get-FileHash -LiteralPath $_ -Algorithm SHA256; "$($h.Hash.ToLower())  $([IO.Path]::GetFileName($_))" }
[IO.File]::WriteAllLines((Join-Path $out 'SHA256SUMS.txt'),$checksums,[Text.UTF8Encoding]::new($false))
Write-Host "Release assets: $out"
Write-Host "Staged files: $stage"
