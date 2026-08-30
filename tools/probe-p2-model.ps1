param(
    [string]$ModelPath = (Join-Path (Split-Path -Parent $PSScriptRoot) 'model\artifacts\Qwen3-1.7B-Q4_K_M.gguf')
)

$ErrorActionPreference = 'Stop'
$root = [System.IO.Path]::GetFullPath((Split-Path -Parent $PSScriptRoot))
$artifactDir = Join-Path $root 'model\artifacts'
$toolDir = Join-Path $artifactDir 'llama-tools'
$completion = Join-Path $toolDir 'llama-completion.exe'
$grammar = Join-Path $root 'data\grammar\triage.gbnf'
$systemPrompt = (Get-Content -LiteralPath (Join-Path $root 'data\prompts\triage-system.txt') -Raw).Trim()
$model = Get-Item -LiteralPath $ModelPath
$modelEvidencePath = Join-Path $artifactDir 'model-evidence.json'
$modelEvidence = Get-Content -LiteralPath $modelEvidencePath -Raw | ConvertFrom-Json
function Get-Sha256([string]$Path) {
    $stream = [System.IO.File]::OpenRead($Path)
    $sha = [System.Security.Cryptography.SHA256]::Create()
    try {
        return ([System.BitConverter]::ToString($sha.ComputeHash($stream))).Replace('-', '').ToLowerInvariant()
    } finally {
        $sha.Dispose()
        $stream.Dispose()
    }
}
$modelHash = Get-Sha256 $model.FullName
if ($modelHash -ne $modelEvidence.output_sha256) {
    throw 'The Q4 model does not match model/artifacts/model-evidence.json'
}
if (-not (Test-Path -LiteralPath $completion)) {
    throw 'Run tools/prepare-p2-model.ps1 before probing the model'
}

$probeDir = Join-Path $artifactDir 'probe'
New-Item -ItemType Directory -Path $probeDir -Force | Out-Null
$utf8NoBom = [System.Text.UTF8Encoding]::new($false)
$cases = @(
    [ordered]@{ message = 'he is not breathing'; expected_protocol = 'cpr.adult' },
    [ordered]@{ message = 'my father has chest pain and feels sweaty'; expected_protocol = 'chest.pain' },
    # P0 deliberately over-triages conscious breathlessness to the CPR card.
    [ordered]@{ message = 'cant breath properly'; expected_protocol = 'cpr.adult' },
    [ordered]@{ message = 'burn from hot water on my arm'; expected_protocol = 'burn.thermal' },
    [ordered]@{ message = 'she is awake after a seizure'; expected_protocol = 'seizure.active' }
)
$records = @()

Push-Location $root
try {
    for ($index = 0; $index -lt $cases.Count; $index++) {
        $message = $cases[$index].message
        $expectedProtocol = $cases[$index].expected_protocol
        $prompt = "<|im_start|>system`n$systemPrompt<|im_end|>`n<|im_start|>user`n$($message.Trim()) /no_think<|im_end|>`n<|im_start|>assistant`n"
        $promptPath = Join-Path $probeDir "prompt-$index.txt"
        $outputPath = Join-Path $probeDir "output-$index.json"
        $logPath = Join-Path $probeDir "llama-$index.log"
        [System.IO.File]::WriteAllText($promptPath, $prompt, $utf8NoBom)

        $started = [System.Diagnostics.Stopwatch]::StartNew()
        # Windows PowerShell wraps every native stderr line as a non-terminating error.
        # llama.cpp writes both progress and generated text there, so capture it without
        # allowing the script-wide Stop preference to terminate on the first log line.
        $previousErrorPreference = $ErrorActionPreference
        $ErrorActionPreference = 'Continue'
        $nativeLines = & $completion `
            -m $model.FullName -f $promptPath `
            --grammar-file $grammar -n 384 -c 4096 -b 512 `
            -t 4 -tb 4 --top-k 20 --top-p 0.8 --temp 0.7 --seed 42 `
            --no-conversation --single-turn --no-display-prompt --simple-io 2>&1
        $exitCode = $LASTEXITCODE
        $ErrorActionPreference = $previousErrorPreference
        $started.Stop()
        if ($exitCode -ne 0) { throw "llama.cpp probe $index failed; see $logPath" }
        $nativeLog = (($nativeLines | ForEach-Object { $_.ToString() }) -join "`n")
        [System.IO.File]::WriteAllText($logPath, $nativeLog, $utf8NoBom)
        $jsonMatch = [regex]::Match(
            $nativeLog,
            '(?s)\{\s*"schema_version"\s*:\s*"1".*?"needs_emergency_services"\s*:\s*(?:true|false)\s*\}'
        )
        if (-not $jsonMatch.Success) { throw "No constrained JSON found for probe $index; see $logPath" }
        $json = $jsonMatch.Value
        [System.IO.File]::WriteAllText($outputPath, $json, $utf8NoBom)

        & cargo run --quiet --locked -p prohori-core --example validate_slots -- $message $outputPath $expectedProtocol
        if ($LASTEXITCODE -ne 0) { throw "Rust verifier refused probe $index" }
        $slots = $json | ConvertFrom-Json
        $records += [ordered]@{
            case = $index
            message = $message
            elapsed_ms = $started.ElapsedMilliseconds
            model_severity = $slots.severity
            model_protocol_id = $slots.protocol_id
            expected_final_protocol = $expectedProtocol
            verifier_passed = $true
        }
    }
} finally {
    Pop-Location
}

$evidence = [ordered]@{
    schema_version = 1
    model_sha256 = $modelHash
    constrained_case_count = $records.Count
    all_verifier_passed = $true
    measured_at_utc = [DateTime]::UtcNow.ToString('o')
    cases = $records
}
$evidencePath = Join-Path $probeDir 'host-probe-evidence.json'
$evidence | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $evidencePath -Encoding utf8
$evidence | ConvertTo-Json -Depth 8
