param(
    [Parameter(Mandatory=$true)][string]$Cxx,
    [Parameter(Mandatory=$true)][string]$OrtInclude
)
$ErrorActionPreference = 'Stop'
$Cxx = (Resolve-Path -LiteralPath $Cxx).Path
$OrtInclude = (Resolve-Path -LiteralPath $OrtInclude).Path
$stage = Join-Path $env:TEMP 'voice-isolation-v2-build'
New-Item -ItemType Directory -Force -Path $stage | Out-Null
foreach ($name in @('onnxruntime_c_api.h','onnxruntime_ep_c_api.h')) {
    $source = Join-Path $OrtInclude $name
    if (!(Test-Path -LiteralPath $source -PathType Leaf)) { throw "Missing ONNX Runtime header: $source" }
    Copy-Item -LiteralPath $source -Destination $stage -Force
}
Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'vox_v2.cpp') -Destination $stage -Force
$env:PATH = "$(Split-Path -Parent $Cxx);$env:PATH"
& $Cxx -std=c++17 -O3 -finput-charset=UTF-8 -static-libgcc -static-libstdc++ -shared "-I$stage" (Join-Path $stage 'vox_v2.cpp') -o (Join-Path $stage 'vox_v2.dll')
if ($LASTEXITCODE -ne 0) { throw 'V2 experimental DLL build failed' }
$out = Join-Path $PSScriptRoot 'build'
New-Item -ItemType Directory -Force -Path $out | Out-Null
Copy-Item -LiteralPath (Join-Path $stage 'vox_v2.dll') -Destination $out -Force
Write-Host "Experimental V2 DLL: $out\vox_v2.dll"
