param(
    [string]$PackRoot = (Join-Path (Split-Path -Parent $PSScriptRoot) 'data\packs\built'),
    [long]$RouteEpoch = [DateTimeOffset]::UtcNow.ToUnixTimeSeconds()
)

$ErrorActionPreference = 'Stop'
$root = [System.IO.Path]::GetFullPath((Split-Path -Parent $PSScriptRoot))
$evidence = @()
foreach ($scenario in @('open', 'blocked', 'flooded', 'stale')) {
    $directory = Join-Path $PackRoot "ruet-$scenario"
    $output = & cargo run --quiet --locked -p prohori-core --example probe_p3_pack -- `
        $directory $RouteEpoch $scenario
    if ($LASTEXITCODE -ne 0) { throw "P3 $scenario probe failed" }
    $evidence += ($output -join "`n") | ConvertFrom-Json
}
$report = [ordered]@{
    schema_version = 1
    all_scenarios_passed = ($evidence.Count -eq 4)
    measured_at_utc = [DateTime]::UtcNow.ToString('o')
    scenarios = $evidence
}
$path = Join-Path $PackRoot 'p3-host-evidence.json'
$report | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $path -Encoding utf8
$report | ConvertTo-Json -Depth 8
