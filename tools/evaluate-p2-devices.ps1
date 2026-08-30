param(
    [string]$EvidenceDirectory = (Join-Path (Split-Path -Parent $PSScriptRoot) 'model\benchmarks\private')
)

$ErrorActionPreference = 'Stop'
$root = [System.IO.Path]::GetFullPath((Split-Path -Parent $PSScriptRoot))
$files = @(Get-ChildItem -LiteralPath $EvidenceDirectory -Filter '*.json' -File -ErrorAction SilentlyContinue)
$failures = [System.Collections.Generic.List[string]]::new()
if ($files.Count -lt 3) { $failures.Add("three physical-phone reports required; found $($files.Count)") }
$reports = @($files | ForEach-Object { Get-Content -LiteralPath $_.FullName -Raw | ConvertFrom-Json })

$devices = @()
foreach ($report in $reports) {
    $runs = @($report.benchmark.runs)
    if ($runs.Count -eq 0) {
        $failures.Add("$($report.manufacturer) $($report.model): no inference runs")
        continue
    }
    $maxPss = ($runs | Measure-Object -Property total_pss_bytes -Maximum).Maximum
    $maxTtft = ($runs | Measure-Object -Property ttft_ms -Maximum).Maximum
    $minSpeed = ($runs | Measure-Object -Property tokens_per_second -Minimum).Minimum
    if ([long]$report.benchmark.model_bytes -gt 1500000000) { $failures.Add("$($report.model): model exceeds 1.5 GB") }
    if ([long]$maxPss -gt 2000000000) { $failures.Add("$($report.model): peak PSS exceeds 2.0 GB") }
    if ([long]$maxTtft -gt 1500) { $failures.Add("$($report.model): TTFT exceeds 1.5 s") }
    if ([double]$minSpeed -lt 8.0) { $failures.Add("$($report.model): generation falls below 8 tokens/s") }
    $devices += [ordered]@{
        manufacturer = $report.manufacturer
        model = $report.model
        android = $report.android
        total_ram_bytes = $report.total_ram_bytes
        max_pss_bytes = $maxPss
        max_ttft_ms = $maxTtft
        min_tokens_per_second = [Math]::Round([double]$minSpeed, 2)
    }
}

$result = [ordered]@{
    schema_version = 1
    passed = ($failures.Count -eq 0)
    evaluated_at_utc = [DateTime]::UtcNow.ToString('o')
    device_count = $reports.Count
    devices = $devices
    failures = @($failures)
}
$output = Join-Path $root 'model\benchmarks\p2-device-gate.json'
New-Item -ItemType Directory -Path (Split-Path -Parent $output) -Force | Out-Null
$result | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $output -Encoding utf8
$result | ConvertTo-Json -Depth 8
if (-not $result.passed) { exit 1 }
