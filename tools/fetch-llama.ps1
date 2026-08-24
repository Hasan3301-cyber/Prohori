param(
    [Parameter(Mandatory = $true)]
    [string]$Destination,

    [Parameter(Mandatory = $true)]
    [ValidatePattern('^[0-9a-f]{40}$')]
    [string]$Commit
)

$ErrorActionPreference = 'Stop'
$expectedRoot = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..\third_party'))
$destinationRoot = [System.IO.Path]::GetFullPath($Destination)

if (-not $destinationRoot.StartsWith($expectedRoot + [System.IO.Path]::DirectorySeparatorChar, [System.StringComparison]::OrdinalIgnoreCase)) {
    throw "Refusing to manage llama.cpp outside $expectedRoot (received $destinationRoot)"
}

$head = $null
if (Test-Path -LiteralPath (Join-Path $destinationRoot '.git')) {
    $head = (& git -C $destinationRoot rev-parse HEAD 2>$null)
}
if ($head -eq $Commit) {
    Write-Host "llama.cpp already pinned at $Commit"
    exit 0
}

if (Test-Path -LiteralPath $destinationRoot) {
    # Destination was validated above and is build output, so replacing a stale/incomplete
    # fetch cannot touch user source or an arbitrary directory.
    Remove-Item -LiteralPath $destinationRoot -Recurse -Force
}

New-Item -ItemType Directory -Path $destinationRoot | Out-Null
& git -C $destinationRoot init --quiet
& git -C $destinationRoot remote add origin https://github.com/ggml-org/llama.cpp.git
& git -C $destinationRoot fetch --quiet --depth 1 origin $Commit
& git -C $destinationRoot checkout --quiet --detach FETCH_HEAD

$actual = (& git -C $destinationRoot rev-parse HEAD)
if ($LASTEXITCODE -ne 0 -or $actual -ne $Commit) {
    throw "llama.cpp checkout verification failed: expected $Commit, got $actual"
}
Write-Host "Fetched llama.cpp $actual"
