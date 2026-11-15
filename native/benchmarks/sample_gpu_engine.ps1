param(
    [Parameter(Mandatory=$true)][int]$TargetPid,
    [Parameter(Mandatory=$true)][string]$OutputPath,
    [int]$MaxSeconds = 120
)
$ErrorActionPreference = 'Stop'
$started = Get-Date
while ((Get-Date) -lt $started.AddSeconds($MaxSeconds)) {
    $p = Get-Process -Id $TargetPid -ErrorAction SilentlyContinue
    if (!$p) { break }
    try {
        $counter = Get-Counter '\GPU Engine(*)\Utilization Percentage' -ErrorAction Stop
        $now = (Get-Date).ToUniversalTime().ToString('o')
        foreach ($s in $counter.CounterSamples) {
            if ($s.InstanceName -match "^pid_${TargetPid}_") {
                [ordered]@{ utc = $now; instance = $s.InstanceName; percent = $s.CookedValue } |
                    ConvertTo-Json -Compress | Add-Content -Encoding utf8 $OutputPath
            }
        }
    } catch {
        [ordered]@{ utc = (Get-Date).ToUniversalTime().ToString('o'); error = $_.Exception.Message } |
            ConvertTo-Json -Compress | Add-Content -Encoding utf8 $OutputPath
    }
    Start-Sleep -Milliseconds 500
}
