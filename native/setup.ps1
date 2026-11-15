# Run from any directory. Downloads official pinned components and checks SHA-256.
# No user profile or voice sample is read or modified.
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
$native = $PSScriptRoot
$project = Split-Path -Parent $native
$models = Join-Path $project 'models'
$cache = Join-Path $native '.download-cache'
New-Item -ItemType Directory -Force -Path $models, $cache | Out-Null
function Download-Checked([string]$url, [string]$name, [string]$sha) {
    $file = Join-Path $cache $name
    if ((Test-Path -LiteralPath $file) -and (Get-FileHash -LiteralPath $file -Algorithm SHA256).Hash -eq $sha) { return $file }
    $partial = "$file.partial"
    if ((Test-Path -LiteralPath $partial) -and (Get-FileHash -LiteralPath $partial -Algorithm SHA256).Hash -eq $sha) {
        Move-Item -LiteralPath $partial -Destination $file -Force
        return $file
    }
    Write-Host "下载 $name ..."
    # curl retries interruptions. Only a hash-verified complete file is installed.
    & curl.exe --ssl-no-revoke --fail --location --retry 8 --retry-all-errors --connect-timeout 30 --speed-limit 1024 --speed-time 30 --continue-at - --output $partial $url
    if ($LASTEXITCODE -ne 0) { throw "下载失败：$url；可重新运行此脚本。" }
    $actual = (Get-FileHash -LiteralPath $partial -Algorithm SHA256).Hash
    if ($actual -ne $sha) { Remove-Item -LiteralPath $partial -Force; throw "SHA-256 校验失败：$name" }
    Move-Item -LiteralPath $partial -Destination $file -Force
    return $file
}
$base = 'https://github.com/k2-fsa/sherpa-onnx/releases/download'
$runtime = Download-Checked "$base/v2.0.1/sherpa-onnx-v2.0.1-win-x64-shared-MD-Release-no-tts.tar.bz2" 'sherpa-v2.0.1-win-x64.tar.bz2' '11fd6a50b97eed38b54fb16d8219f49b16aa7f6a8e43a042316469ef47652e82'
function Valid-File([string]$path, [string]$sha) {
    return (Test-Path -LiteralPath $path -PathType Leaf) -and ((Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash -eq $sha)
}
$haveVox = (Valid-File (Join-Path $models 'enroll.onnx') '5d06048c156bb8cd422d29848e6d38fb26a7b39f6f7fd92eccf2a6ed1ce003c6') -and (Valid-File (Join-Path $models 'chunk_int8.onnx') 'df61a9d0062b0adbcf1182302b1eb8b815f15ac47ea4923e8e164c1aff64121d')
if (!$haveVox) { $vox = Download-Checked "$base/speech-enhancement-models/sherpa-onnx-voxlock-int8.tar.bz2" 'sherpa-onnx-voxlock-int8.tar.bz2' 'bcb947f9e8676d3e963f6048b0088505c84f0843bfdc88cb26dde3264d81570b' }
$gtcrnPath = Join-Path $models 'gtcrn_simple.onnx'
$dpdfPath = Join-Path $models 'dpdfnet2.onnx'
if (!(Valid-File $gtcrnPath 'e77603ac0c23dac3227dd2d7135b3a585cbee2679048aecfa886657d3ae1b534')) { $gtcrn = Download-Checked "$base/speech-enhancement-models/gtcrn_simple.onnx" 'gtcrn_simple.onnx' 'e77603ac0c23dac3227dd2d7135b3a585cbee2679048aecfa886657d3ae1b534' }
if (!(Valid-File $dpdfPath 'ce35d6025fc71df0ef10d1540e1b7916837bbfe5f6896deb744508d2cad487a9')) { $dpdf = Download-Checked "$base/speech-enhancement-models/dpdfnet2.onnx" 'dpdfnet2.onnx' 'ce35d6025fc71df0ef10d1540e1b7916837bbfe5f6896deb744508d2cad487a9' }
$stage = Join-Path $cache 'staging'
New-Item -ItemType Directory -Force -Path $stage | Out-Null
& tar.exe -xf $runtime -C $stage
if ($LASTEXITCODE -ne 0) { throw '解压运行库失败' }
if (!$haveVox) {
    & tar.exe -xf $vox -C $stage
    if ($LASTEXITCODE -ne 0) { throw '解压 VoxLock 模型失败' }
}
$runtimeBin = Join-Path $stage 'sherpa-onnx-v2.0.1-win-x64-shared-MD-Release-no-tts/lib'
$voxDir = Join-Path $stage 'sherpa-onnx-voxlock-int8'
if (!$haveVox) { foreach ($name in @('enroll.onnx', 'chunk_int8.onnx')) {
    $source = Join-Path $voxDir $name
    if (!(Test-Path -LiteralPath $source -PathType Leaf)) { throw "模型包缺少 $name" }
    Copy-Item -LiteralPath $source -Destination (Join-Path $models $name) -Force
} }
if ($gtcrn) { Copy-Item -LiteralPath $gtcrn -Destination $gtcrnPath -Force }
if ($dpdf) { Copy-Item -LiteralPath $dpdf -Destination $dpdfPath -Force }
$dlls = @(Get-ChildItem -LiteralPath $runtimeBin -Filter '*.dll' -File)
if (!($dlls | Where-Object Name -eq 'sherpa-onnx-c-api.dll') -or !($dlls | Where-Object Name -eq 'onnxruntime.dll')) { throw '运行库包缺少必要 DLL' }
# Executables in either Cargo profile use their own adjacent runtime folder.
foreach ($buildKind in @('debug','release')) {
    $dest = Join-Path $native "target/$buildKind/runtime"
    New-Item -ItemType Directory -Force -Path $dest | Out-Null
    $dlls | Copy-Item -Destination $dest -Force
}
Write-Host "模型已安装到 $models；运行库已安装到 native/target/{debug,release}/runtime。"
