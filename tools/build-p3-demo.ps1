param(
    [string]$OutputRoot = (Join-Path (Split-Path -Parent $PSScriptRoot) 'data\packs\built'),
    [string]$SigningKey = (Join-Path (Split-Path -Parent $PSScriptRoot) 'data\packs\.demo-signing-key.hex'),
    [long]$SnapshotEpoch = [DateTimeOffset]::UtcNow.ToUnixTimeSeconds()
)

$ErrorActionPreference = 'Stop'
$root = [System.IO.Path]::GetFullPath((Split-Path -Parent $PSScriptRoot))
$output = [System.IO.Path]::GetFullPath($OutputRoot)
$allowed = [System.IO.Path]::GetFullPath((Join-Path $root 'data\packs'))
if (-not $output.StartsWith($allowed + [System.IO.Path]::DirectorySeparatorChar, [System.StringComparison]::OrdinalIgnoreCase)) {
    throw "Refusing to write a demo pack outside $allowed"
}

if (-not (Test-Path -LiteralPath $SigningKey)) {
    $bytes = New-Object byte[] 32
    $random = [System.Security.Cryptography.RandomNumberGenerator]::Create()
    try { $random.GetBytes($bytes) } finally { $random.Dispose() }
    $hex = ([System.BitConverter]::ToString($bytes)).Replace('-', '').ToLowerInvariant()
    [System.IO.File]::WriteAllText($SigningKey, $hex, [System.Text.UTF8Encoding]::new($false))
    Write-Warning "Created a local DEMO signing key at $SigningKey. It is ignored by Git and must never sign a release pack."
}

New-Item -ItemType Directory -Path $output -Force | Out-Null
$scenarios = @('open', 'blocked', 'flooded', 'stale')
foreach ($scenario in $scenarios) {
    $destination = Join-Path $output "ruet-$scenario"
    New-Item -ItemType Directory -Path $destination -Force | Out-Null
    & cargo run --quiet --locked -p prohori-core --example build_p3_demo -- `
        $destination $SigningKey $SnapshotEpoch $scenario
    if ($LASTEXITCODE -ne 0) { throw "P3 $scenario pack build failed" }
    $archive = Join-Path $output "ruet-$scenario.prohori-pack"
    $payloads = @(
        'manifest.json', 'conditions.snap', 'emergency.json', 'hospitals.json',
        'roads.graph', 'shelters.json', 'zones.geojson'
    ) | ForEach-Object { Join-Path $destination $_ }
    $zip = "$archive.zip"
    Compress-Archive -LiteralPath $payloads -DestinationPath $zip -CompressionLevel Optimal -Force
    if (Test-Path -LiteralPath $archive) { [System.IO.File]::Delete($archive) }
    Move-Item -LiteralPath $zip -Destination $archive
}
Write-Host "Built signed P3 demo scenarios under $output"
