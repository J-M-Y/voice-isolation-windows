param(
    [Parameter(Mandatory=$true)][string]$Cxx,
    [string]$Package,
    [string]$Destination = (Join-Path $PSScriptRoot 'target\release\runtime-dml')
)
# Builds the checked worker and installs its isolated official
# DirectML runtime. Does not modify the existing CPU runtime or launch apps.
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
$cxxPath = (Get-Command $Cxx -ErrorAction Stop).Source
if (!$Package) {
    $cache = Join-Path $PSScriptRoot '.download-cache'
    New-Item -ItemType Directory -Force $cache | Out-Null
    $Package = Join-Path $cache 'microsoft.ml.onnxruntime.directml.1.24.4.nupkg'
    if (!(Test-Path -LiteralPath $Package -PathType Leaf)) {
        $url = 'https://api.nuget.org/v3-flatcontainer/microsoft.ml.onnxruntime.directml/1.24.4/microsoft.ml.onnxruntime.directml.1.24.4.nupkg'
        $partial = "$Package.partial"
        Invoke-WebRequest -Uri $url -OutFile $partial
        Move-Item -LiteralPath $partial -Destination $Package -Force
    }
}
$Package = (Resolve-Path -LiteralPath $Package).Path
if ((Get-FileHash -LiteralPath $Package -Algorithm SHA256).Hash -ne '57E9F11B73437BEF7A309496135D4C1F96B1A8E9DDBA60013FA27BFC1D788681') { throw 'DirectML NuGet package SHA-256 mismatch' }
$stage = Join-Path $env:TEMP ('voice-gpu-build-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Force $stage | Out-Null
& tar.exe -xf $Package -C $stage
if ($LASTEXITCODE -ne 0) { throw 'Failed to extract the DirectML package' }
$include = Join-Path $stage 'build\native\include'
$dll = Join-Path $stage 'runtimes\win-x64\native\onnxruntime.dll'
$shared = Join-Path $stage 'runtimes\win-x64\native\onnxruntime_providers_shared.dll'
if ((Get-FileHash -LiteralPath $dll -Algorithm SHA256).Hash -ne 'E7EEDEC6A6F26DC39DC948276A75EF6D2BEE3FFF944D874CEED0BBD3B97BFF40') { throw 'DirectML runtime DLL SHA-256 mismatch' }
if ((Get-FileHash -LiteralPath $shared -Algorithm SHA256).Hash -ne '265C8DAF29637CB259CAC8BE9F08F2CD45F3883F0F0E4949CBFDDD5B4CBEC3B6') { throw 'DirectML provider DLL SHA-256 mismatch' }
# ASCII staging avoids GNU linker errors on Windows paths containing Chinese.
Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'experimental_gpu\worker.cpp'),(Join-Path $PSScriptRoot 'experimental_gpu\vox_v1.cpp') -Destination $stage
$oldPath = $env:PATH
try {
    $env:PATH = "$(Split-Path -Parent $cxxPath);$oldPath"
    & $cxxPath -std=c++17 -O3 -w -static-libgcc -static-libstdc++ -municode "-I$include" (Join-Path $stage 'worker.cpp') (Join-Path $stage 'vox_v1.cpp') -ldxgi -luuid -o (Join-Path $stage 'vox_v1_worker_checked.exe')
    if ($LASTEXITCODE -ne 0) { throw 'GPU worker build failed' }
} finally { $env:PATH = $oldPath }
New-Item -ItemType Directory -Force $Destination | Out-Null
Copy-Item -LiteralPath $dll,$shared,(Join-Path $stage 'vox_v1_worker_checked.exe') -Destination $Destination -Force
Write-Host "GPU worker and DirectML runtime installed to $Destination"
Write-Host 'The CPU runtime is separate; do not enable GPU mode without completing audio, deadline and failure tests.'
