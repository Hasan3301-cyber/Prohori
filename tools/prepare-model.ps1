param(
    [string]$RepositoryRoot = (Split-Path -Parent $PSScriptRoot)
)

$ErrorActionPreference = 'Stop'
$root = [System.IO.Path]::GetFullPath($RepositoryRoot)
$lock = Get-Content -LiteralPath (Join-Path $root 'model\model.lock.json') -Raw | ConvertFrom-Json
$artifactDir = Join-Path $root 'model\artifacts'
$toolDir = Join-Path $artifactDir 'llama-tools'
$q8Path = Join-Path $artifactDir $lock.base.file
$q4Path = Join-Path $artifactDir $lock.quantization.output
$zipPath = Join-Path $artifactDir $lock.llama_cpp.windows_cpu_asset
New-Item -ItemType Directory -Path $artifactDir -Force | Out-Null

function Test-Artifact([string]$Path, [long]$Bytes, [string]$Sha256) {
    if (-not (Test-Path -LiteralPath $Path)) { return $false }
    if ((Get-Item -LiteralPath $Path).Length -ne $Bytes) { return $false }
    $actual = (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($actual -ne $Sha256) {
        throw "Checksum mismatch for $Path`nexpected $Sha256`nactual   $actual"
    }
    return $true
}

if (-not (Test-Artifact $q8Path $lock.base.bytes $lock.base.sha256)) {
    Write-Host 'Downloading the pinned official Qwen Q8 GGUF (resume enabled)...'
    & curl.exe -L --fail --retry 5 --continue-at - --output $q8Path $lock.base.url
    if ($LASTEXITCODE -ne 0) { throw 'Qwen download failed' }
    if (-not (Test-Artifact $q8Path $lock.base.bytes $lock.base.sha256)) {
        throw 'Downloaded Qwen artifact failed size or SHA-256 verification'
    }
}

if (-not (Test-Artifact $zipPath $lock.llama_cpp.windows_cpu_bytes $lock.llama_cpp.windows_cpu_sha256)) {
    Write-Host 'Downloading the pinned llama.cpp host tools...'
    & curl.exe -L --fail --retry 5 --output $zipPath $lock.llama_cpp.windows_cpu_url
    if ($LASTEXITCODE -ne 0) { throw 'llama.cpp tool download failed' }
    if (-not (Test-Artifact $zipPath $lock.llama_cpp.windows_cpu_bytes $lock.llama_cpp.windows_cpu_sha256)) {
        throw 'Downloaded llama.cpp tools failed size or SHA-256 verification'
    }
}

$quantize = Join-Path $toolDir 'llama-quantize.exe'
if (-not (Test-Path -LiteralPath $quantize)) {
    if (Test-Path -LiteralPath $toolDir) {
        $resolved = [System.IO.Path]::GetFullPath($toolDir)
        $safe = [System.IO.Path]::GetFullPath($artifactDir)
        if (-not $resolved.StartsWith($safe + [System.IO.Path]::DirectorySeparatorChar, [System.StringComparison]::OrdinalIgnoreCase)) {
            throw 'Unsafe llama tool extraction target'
        }
        Remove-Item -LiteralPath $resolved -Recurse -Force
    }
    New-Item -ItemType Directory -Path $toolDir | Out-Null
    Expand-Archive -LiteralPath $zipPath -DestinationPath $toolDir
}

if ((Test-Path -LiteralPath $q4Path) -and (Get-Item -LiteralPath $q4Path).Length -lt 500000000) {
    # llama-quantize creates its destination before work begins. Remove only that exact,
    # known output if an interrupted/failed run left an unusable partial artifact.
    Remove-Item -LiteralPath $q4Path -Force
}
if (-not (Test-Path -LiteralPath $q4Path)) {
    Write-Host 'Requantizing the verified official Q8 model to Q4_K_M (output tensor retained)...'
    $quantizeFlags = @($lock.quantization.flags)
    & $quantize $quantizeFlags $q8Path $q4Path $lock.quantization.type
    if ($LASTEXITCODE -ne 0) { throw 'llama.cpp quantization failed' }
}

$q4 = Get-Item -LiteralPath $q4Path
if ($q4.Length -gt [long]$lock.quantization.maximum_bytes) {
    throw "Q4 model exceeds the on-disk gate: $($q4.Length) bytes"
}
$q4Hash = (Get-FileHash -LiteralPath $q4Path -Algorithm SHA256).Hash.ToLowerInvariant()
$evidence = [ordered]@{
    schema_version = 1
    base_revision = $lock.base.revision
    base_sha256 = $lock.base.sha256
    llama_cpp_release = $lock.llama_cpp.release
    llama_cpp_commit = $lock.llama_cpp.commit
    source_quantization = $lock.quantization.source_type
    quantization = $lock.quantization.type
    quantization_flags = @($lock.quantization.flags)
    output_file = $q4.Name
    output_bytes = $q4.Length
    output_sha256 = $q4Hash
    produced_at_utc = [DateTime]::UtcNow.ToString('o')
}
$evidence | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $artifactDir 'model-evidence.json') -Encoding utf8
Write-Host "Ready: $q4Path"
Write-Host "Size:  $($q4.Length) bytes"
Write-Host "SHA:   $q4Hash"
