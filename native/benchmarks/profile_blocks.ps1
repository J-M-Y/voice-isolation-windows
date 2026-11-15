param(
    [string]$Reference = (Join-Path $env:TEMP 'voice-isolation-cpu-benchmark\reference.wav'),
    [string]$InputWav = (Join-Path $env:TEMP 'voice-isolation-cpu-benchmark\input.wav'),
    [string]$Models = (Join-Path (Split-Path -Parent (Split-Path -Parent $PSScriptRoot)) 'models'),
    [string]$OutputDir = (Join-Path $env:TEMP ("voice-block-benchmark-" + [DateTime]::Now.ToString('yyyyMMdd-HHmmss'))),
    [string]$Cargo = 'cargo',
    [int]$Trials = 3
)
$ErrorActionPreference = 'Stop'
if ($Trials -lt 1 -or $Trials -gt 20) { throw 'Trials must be between 1 and 20.' }
$native = Split-Path -Parent $PSScriptRoot
$Reference = (Resolve-Path -LiteralPath $Reference).Path
$InputWav = (Resolve-Path -LiteralPath $InputWav).Path
$Models = (Resolve-Path -LiteralPath $Models).Path
New-Item -ItemType Directory -Force -Path $OutputDir | Out-Null
$OutputDir = (Resolve-Path -LiteralPath $OutputDir).Path
$project = Join-Path $OutputDir 'project'
$bin = Join-Path $OutputDir 'bin'
New-Item -ItemType Directory -Force -Path "$project\src", "$bin\runtime" | Out-Null
Copy-Item -LiteralPath (Join-Path $native 'Cargo.toml'), (Join-Path $native 'Cargo.lock') -Destination $project
Copy-Item -LiteralPath (Join-Path $native 'src\processor.rs') -Destination "$project\src\processor.rs"
Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'block_profile.rs') -Destination "$project\src\main.rs"
$runtime = Join-Path $native 'target\release\runtime'
Copy-Item -Path (Join-Path $runtime '*.dll') -Destination "$bin\runtime"
# An ASCII target path avoids GNU ld issues with non-ASCII workspace paths.
$oldTarget = $env:CARGO_TARGET_DIR
if (!$env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR = Join-Path $env:TEMP 'voice-isolation-block-profile-target' }
try {
    & $Cargo build --release --locked --manifest-path "$project\Cargo.toml" --bin voice-isolation-native
    if ($LASTEXITCODE -ne 0) { throw 'Benchmark build failed.' }
    $exe = Join-Path $bin 'block_profile.exe'
    Copy-Item -LiteralPath (Join-Path $env:CARGO_TARGET_DIR 'release\voice-isolation-native.exe') -Destination $exe -Force
    $results = Join-Path $OutputDir 'results.jsonl'
    $logs = Join-Path $OutputDir 'sherpa.log'
    if (Test-Path -LiteralPath $results) { throw "Output already exists: $results; choose a new OutputDir." }
    foreach ($mode in @('normal', 'strong')) {
        foreach ($block in @(3840, 7680, 11520)) {
            foreach ($trial in 1..$Trials) {
                $wav = Join-Path $OutputDir "${mode}_${block}_trial${trial}.wav"
                $result = & $exe $Reference $InputWav $Models $block $mode $wav $trial 2>> $logs
                if ($LASTEXITCODE -ne 0) { throw "Run failed; see $logs." }
                $result | Add-Content -Encoding utf8 $results
                Write-Host $result
            }
        }
    }
    Write-Host "Results and output WAVs: $OutputDir"
} finally { $env:CARGO_TARGET_DIR = $oldTarget }
