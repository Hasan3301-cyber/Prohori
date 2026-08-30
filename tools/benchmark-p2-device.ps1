param(
    [string]$ModelPath = (Join-Path (Split-Path -Parent $PSScriptRoot) 'model\artifacts\Qwen3-1.7B-Q4_K_M.gguf'),
    [switch]$AllowPartial
)

$ErrorActionPreference = 'Stop'
$root = [System.IO.Path]::GetFullPath((Split-Path -Parent $PSScriptRoot))
$model = Get-Item -LiteralPath $ModelPath
$lock = Get-Content -LiteralPath (Join-Path $root 'model\model.lock.json') -Raw | ConvertFrom-Json
if ($model.Length -gt [long]$lock.quantization.maximum_bytes) { throw 'Model exceeds the 1.5 GB disk gate' }
$modelHash = (Get-FileHash -LiteralPath $model.FullName -Algorithm SHA256).Hash.ToLowerInvariant()

$adb = Join-Path $env:ANDROID_HOME 'platform-tools\adb.exe'
if (-not (Test-Path -LiteralPath $adb)) { throw 'Set ANDROID_HOME to an SDK containing platform-tools/adb' }
$devices = @(& $adb devices | Select-Object -Skip 1 | ForEach-Object {
    if ($_ -match '^([^\s]+)\s+device$') { $Matches[1] }
})
if ($devices.Count -eq 0) { throw 'No authorized Android device is connected' }
if ($devices.Count -lt 3 -and -not $AllowPartial) {
    throw "P2 requires three physical phones; found $($devices.Count). Use -AllowPartial only for a non-gating dry run."
}

$env:JAVA_HOME = if ($env:JAVA_HOME) { $env:JAVA_HOME } else { 'C:\Program Files\Android\Android Studio\jbr' }
Push-Location $root
try {
    & .\gradlew.bat :app:assembleDebug :app:assembleDebugAndroidTest -PprohoriAbis=arm64-v8a --no-daemon
    if ($LASTEXITCODE -ne 0) { throw 'Android benchmark APK build failed' }
} finally {
    Pop-Location
}

$appApk = Join-Path $root 'app\build\outputs\apk\debug\app-debug.apk'
$testApk = Join-Path $root 'app\build\outputs\apk\androidTest\debug\app-debug-androidTest.apk'
$evidenceDir = Join-Path $root 'model\benchmarks\private'
New-Item -ItemType Directory -Path $evidenceDir -Force | Out-Null

foreach ($serial in $devices) {
    Write-Host "Benchmarking $serial"
    & $adb -s $serial install -r $appApk | Out-Host
    if ($LASTEXITCODE -ne 0) { throw "App install failed on $serial" }
    & $adb -s $serial install -r $testApk | Out-Host
    if ($LASTEXITCODE -ne 0) { throw "Test install failed on $serial" }
    & $adb -s $serial shell monkey -p org.prohori.app.debug 1 | Out-Null
    & $adb -s $serial shell am force-stop org.prohori.app.debug | Out-Null
    $remoteDir = '/sdcard/Android/data/org.prohori.app.debug/files/models'
    & $adb -s $serial shell mkdir -p $remoteDir
    & $adb -s $serial push $model.FullName "$remoteDir/qwen3-1.7b.gguf" | Out-Host
    if ($LASTEXITCODE -ne 0) { throw "Model push failed on $serial" }

    $output = & $adb -s $serial shell am instrument -w -r `
        -e class org.prohori.app.P2DeviceBenchmarkTest `
        org.prohori.app.debug.test/androidx.test.runner.AndroidJUnitRunner 2>&1
    $output | Out-Host
    if ($LASTEXITCODE -ne 0 -or ($output -join "`n") -notmatch 'OK \(1 test\)') {
        throw "P2 instrumentation failed on $serial"
    }
    $joined = $output -join "`n"
    $match = [regex]::Match($joined, 'PROHORI_P2_EVIDENCE=(\{.+\})')
    if (-not $match.Success) { throw "No benchmark evidence returned by $serial" }
    $runs = $match.Groups[1].Value | ConvertFrom-Json
    $record = [ordered]@{
        schema_version = 1
        gating = ($devices.Count -ge 3)
        serial_sha256 = [Convert]::ToHexString([Security.Cryptography.SHA256]::HashData([Text.Encoding]::UTF8.GetBytes($serial))).ToLowerInvariant()
        manufacturer = (& $adb -s $serial shell getprop ro.product.manufacturer).Trim()
        model = (& $adb -s $serial shell getprop ro.product.model).Trim()
        android = (& $adb -s $serial shell getprop ro.build.version.release).Trim()
        sdk = [int](& $adb -s $serial shell getprop ro.build.version.sdk).Trim()
        total_ram_bytes = [long](([regex]::Match((& $adb -s $serial shell cat /proc/meminfo | Select-Object -First 1), '\d+').Value)) * 1024
        model_sha256 = $modelHash
        measured_at_utc = [DateTime]::UtcNow.ToString('o')
        benchmark = $runs
    }
    $safeName = ($record.manufacturer + '-' + $record.model) -replace '[^A-Za-z0-9._-]', '_'
    $record | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath (Join-Path $evidenceDir "$safeName.json") -Encoding utf8
}

Write-Host "Private device evidence written under $evidenceDir"
