param(
    [Parameter(Mandatory=$true)][string]$Python,
    [string]$Destination = (Join-Path $env:TEMP 'voice-isolation-two-speaker')
)
$ErrorActionPreference = 'Stop'
New-Item -ItemType Directory -Force $Destination | Out-Null
$Destination = (Resolve-Path $Destination).Path
$voice = New-Object -ComObject SAPI.SpVoice
$tokens = $voice.GetVoices()
$z = $tokens | Where-Object { $_.GetDescription() -like '*Zira*' } | Select-Object -First 1
$h = $tokens | Where-Object { $_.GetDescription() -like '*Huihui*' } | Select-Object -First 1
if (!$z -or !$h) { throw 'This fixture requires Windows SAPI Zira and Huihui voices.' }
function Save-Speech($token, $file, $words) {
    $voice.Voice = $token
    $stream = New-Object -ComObject SAPI.SpFileStream
    $stream.Open((Join-Path $Destination $file), 3, $false)
    try { $voice.AudioOutputStream = $stream; $voice.Speak($words) | Out-Null }
    finally { $stream.Close() }
}
Save-Speech $z 'ref-source.wav' 'The sound of the human voice is different for every person. This is a voice sample used only for a local computer test. Today the weather is clear and pleasant.'
Save-Speech $z 'target-source.wav' 'A small train travels through the mountains, passing a river and several villages on its way to the city. The passengers are reading books, drinking coffee, and enjoying the view outside the window. Later, the train arrives at the station, where friends and families are waiting to greet them.'
Save-Speech $h 'interference-source.wav' '今天我们一起讨论声音处理的效果。窗外有树木和街道，远处的车辆正在缓慢行驶。明天早上大家会在车站见面，然后一起乘坐火车去城市参观。请注意保持音量清晰，继续正常地说话。现在我们继续测试系统是否能够区分不同的说话声音。'
& $Python (Join-Path $PSScriptRoot 'two_speaker_test.py') prepare $Destination
if ($LASTEXITCODE -ne 0) { throw 'Failed to prepare synthetic audio.' }
Write-Host "Synthetic reference and mixture written to $Destination"
