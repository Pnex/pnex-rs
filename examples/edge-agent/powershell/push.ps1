# Push values to the local PNeX edge agent (Windows PowerShell 5.1 or 7).
$Agent = if ($env:PNEX_AGENT_URL) { $env:PNEX_AGENT_URL } else { "http://127.0.0.1:7070" }

function Push-Point($Points) {
    Invoke-RestMethod -Method Post -Uri "$Agent/v1/points" -ContentType "application/json" `
        -Body ($Points | ConvertTo-Json -Depth 5 -Compress)
}

$cpu = (Get-CimInstance Win32_Processor | Measure-Object -Property LoadPercentage -Average).Average
$os = Get-CimInstance Win32_OperatingSystem
$memUsed = [math]::Round(100 * (1 - $os.FreePhysicalMemory / $os.TotalVisibleMemorySize), 1)

Push-Point @{ key = "cpu_load"; value = $cpu; unit = "%" }
Push-Point @(
    @{ key = "memory_used"; value = $memUsed; unit = "%" },
    @{ key = "computer"; value = $env:COMPUTERNAME; record = $true }
)
Write-Host "ok"
