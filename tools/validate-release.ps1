param(
    [Parameter(Mandatory = $true)]
    [string]$ApkPath,
    [ValidateSet('Evaluation', 'UnsignedRelease', 'Production')]
    [string]$Mode = 'Evaluation',
    [string]$RepositoryRoot = (Split-Path -Parent $PSScriptRoot)
)

$ErrorActionPreference = 'Stop'
$root = [System.IO.Path]::GetFullPath($RepositoryRoot)
$apk = [System.IO.Path]::GetFullPath((Join-Path $root $ApkPath))
if (-not (Test-Path -LiteralPath $apk -PathType Leaf)) { throw "APK not found: $apk" }

# apksigner.bat requires Java even when Gradle has already built the APK. CI supplies
# JAVA_HOME, while a normal Android Studio installation on Windows often does not expose
# its bundled runtime to standalone PowerShell sessions.
if ([string]::IsNullOrWhiteSpace($env:JAVA_HOME)) {
    $studioJbr = Join-Path $env:ProgramFiles 'Android\Android Studio\jbr'
    if (Test-Path -LiteralPath (Join-Path $studioJbr 'bin\java.exe') -PathType Leaf) {
        $env:JAVA_HOME = $studioJbr
    }
}

$sdk = if ($env:ANDROID_HOME) { $env:ANDROID_HOME } elseif ($env:ANDROID_SDK_ROOT) { $env:ANDROID_SDK_ROOT } else { Join-Path $env:LOCALAPPDATA 'Android\Sdk' }
$buildTools = Get-ChildItem -LiteralPath (Join-Path $sdk 'build-tools') -Directory |
    Sort-Object { [version]($_.Name -replace '[^0-9.]','') } -Descending |
    Select-Object -First 1
if (-not $buildTools) { throw 'Android build-tools are not installed.' }
$apksigner = Join-Path $buildTools.FullName 'apksigner.bat'
$zipalign = Join-Path $buildTools.FullName 'zipalign.exe'
$aapt2 = Join-Path $buildTools.FullName 'aapt2.exe'

& $zipalign -c -P 16 4 $apk | Out-Null
if ($LASTEXITCODE -ne 0) { throw 'APK is not 16-KiB page zip-aligned.' }

$signature = (& $apksigner verify --verbose --print-certs $apk 2>&1 | Out-String)
$signatureExit = $LASTEXITCODE
if ($Mode -eq 'UnsignedRelease') {
    if ($signatureExit -eq 0) { Write-Host 'UnsignedRelease mode: artifact happens to be signed; continuing with structural checks.' }
} else {
    if ($signatureExit -ne 0) { throw 'APK signature verification failed.' }
    if ($Mode -eq 'Production' -and $signature -match 'CN=Android Debug') {
        throw 'Production validation refused an Android debug certificate.'
    }
}

$badging = (& $aapt2 dump badging $apk 2>&1 | Out-String)
if ($LASTEXITCODE -ne 0) { throw 'aapt2 could not read the APK manifest.' }
$packageMatch = [regex]::Match($badging, "package: name='([^']+)'")
if (-not $packageMatch.Success) { throw 'APK package id was not found.' }
$expectedPackage = if ($Mode -eq 'Evaluation') { 'org.prohori.app.debug' } else { 'org.prohori.app' }
if ($packageMatch.Groups[1].Value -ne $expectedPackage) {
    throw "Wrong package id: expected $expectedPackage."
}

Add-Type -AssemblyName System.IO.Compression.FileSystem
$archive = [System.IO.Compression.ZipFile]::OpenRead($apk)
try {
    $requiredEntries = @(
        'lib/arm64-v8a/libprohori_ffi.so',
        'lib/arm64-v8a/libprohori_llama.so',
        'lib/armeabi-v7a/libprohori_ffi.so',
        'lib/armeabi-v7a/libprohori_llama.so'
    )
    foreach ($name in $requiredEntries) {
        if (-not ($archive.Entries | Where-Object FullName -eq $name)) { throw "Required native library is missing: $name" }
    }

    $model = $archive.Entries | Where-Object FullName -eq 'assets/models/qwen3-1.7b-q4_k_m.gguf'
    if (-not $model) { throw 'Bundled Qwen model asset is missing.' }
    if ($model.Length -ne 1107408544L) { throw 'Bundled model size is not the verified size.' }
    if ($model.CompressedLength -ne $model.Length) { throw 'Bundled model was compressed; first-launch extraction would be unnecessarily slow.' }
    $sha = [System.Security.Cryptography.SHA256]::Create()
    try {
        $stream = $model.Open()
        try { $modelHash = ([BitConverter]::ToString($sha.ComputeHash($stream))).Replace('-','').ToLowerInvariant() }
        finally { $stream.Dispose() }
    } finally { $sha.Dispose() }
    if ($modelHash -ne '54c0f1203a724e9f33e76916beab3bdfaffef56cf7b42a93b1bc21319fc0bf97') {
        throw 'Bundled model SHA-256 does not match the verified artifact.'
    }

    $secretNames = @('LOCATIONIQ_API_KEY','TELEGRAM_BOT_TOKEN','PROHORI_RELAY_DEVICE_TOKEN','PROHORI_DEBUG_BOT_TOKEN')
    $secrets = [System.Collections.Generic.List[string]]::new()
    foreach ($source in @((Join-Path $root '.env'), (Join-Path $root 'local.properties'))) {
        if (-not (Test-Path -LiteralPath $source)) { continue }
        foreach ($line in Get-Content -LiteralPath $source) {
            if ($line -match '^\s*([A-Za-z_][A-Za-z0-9_.]*)\s*=\s*(.*)$' -and $secretNames -contains $matches[1]) {
                $value = $matches[2].Trim().Trim('"').Trim("'")
                if ($value.Length -ge 8) { $secrets.Add($value) }
            }
        }
    }
    foreach ($name in $secretNames) {
        $value = [Environment]::GetEnvironmentVariable($name)
        if ($value -and $value.Length -ge 8) { $secrets.Add($value) }
    }
    $scanEntries = $archive.Entries | Where-Object { $_.FullName -match '^(classes\d*\.dex|resources\.arsc|AndroidManifest\.xml)$' }
    foreach ($entry in $scanEntries) {
        $stream = $entry.Open()
        try {
            $memory = [System.IO.MemoryStream]::new()
            try { $stream.CopyTo($memory); $text = [Text.Encoding]::Latin1.GetString($memory.ToArray()) }
            finally { $memory.Dispose() }
        } finally { $stream.Dispose() }
        foreach ($secret in $secrets) {
            if ($text.Contains($secret, [StringComparison]::Ordinal)) {
                throw "Configured secret material was found in $($entry.FullName)."
            }
        }
    }
} finally {
    $archive.Dispose()
}

$file = Get-Item -LiteralPath $apk
$apkHash = (Get-FileHash -LiteralPath $apk -Algorithm SHA256).Hash.ToLowerInvariant()
Write-Host "Validated $Mode APK"
Write-Host "Package: $expectedPackage"
Write-Host "Bytes:   $($file.Length)"
Write-Host "SHA-256: $apkHash"
Write-Host 'Model, ABIs, alignment, signature policy, and secret scan passed.'
