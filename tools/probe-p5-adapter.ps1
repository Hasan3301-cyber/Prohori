# Run the P5 eval set through llama.cpp under the shipped grammar and score every
# PLAN.md §8 gate.
#
#   tools/probe-p5-adapter.ps1 -LoraPath model\artifacts\p5-lora\p5-lora.gguf
#
# Omit -LoraPath to measure the base model, which is the number the adapter has to beat.
# Nothing here decides whether the adapter ships; core/examples/evaluate_p5_gates.rs does,
# and it refuses unless -Attest carries all three claims it cannot compute.
#
# The prompt is assembled exactly as tools/probe-p2-model.ps1 assembles it, and the system
# prompt is read from the same file the app embeds. If those drift apart, the probe is
# measuring a program that does not ship.

param(
    [string]$ModelPath = (Join-Path (Split-Path -Parent $PSScriptRoot) 'model\artifacts\Qwen3-1.7B-Q4_K_M.gguf'),
    [string]$LoraPath = '',
    [string]$DatasetDir = (Join-Path (Split-Path -Parent $PSScriptRoot) 'model\datasets\p5'),
    # Smoke-test escape hatch. A limited run has fewer predictions than cases, so the gate
    # runner refuses it by construction. Use it to check the plumbing, never to claim a pass.
    [int]$Limit = 0,
    [string[]]$Attest = @()
)

$ErrorActionPreference = 'Stop'
$root = [System.IO.Path]::GetFullPath((Split-Path -Parent $PSScriptRoot))
$artifactDir = Join-Path $root 'model\artifacts'
$completion = Join-Path $artifactDir 'llama-tools\llama-completion.exe'
$grammar = Join-Path $root 'data\grammar\triage.gbnf'
$systemPrompt = (Get-Content -LiteralPath (Join-Path $root 'data\prompts\triage-system.txt') -Raw).Trim()
$evalPath = Join-Path $DatasetDir 'eval.jsonl'
$manifestPath = Join-Path $DatasetDir 'manifest.json'

if (-not (Test-Path -LiteralPath $completion)) {
    throw 'Run tools/prepare-p2-model.ps1 before probing an adapter'
}
if (-not (Test-Path -LiteralPath $evalPath)) {
    throw "Missing $evalPath. Run: cargo run --locked -p prohori-core --example build_p5_dataset"
}
$model = Get-Item -LiteralPath $ModelPath
if ($LoraPath -and -not (Test-Path -LiteralPath $LoraPath)) {
    throw "Adapter not found at $LoraPath"
}

$probeDir = Join-Path $artifactDir 'p5-probe'
New-Item -ItemType Directory -Path $probeDir -Force | Out-Null
$utf8NoBom = [System.Text.UTF8Encoding]::new($false)

$cases = @(Get-Content -LiteralPath $evalPath | Where-Object { $_.Trim() } | ForEach-Object { $_ | ConvertFrom-Json })
if ($Limit -gt 0 -and $Limit -lt $cases.Count) {
    Write-Warning "Probing only $Limit of $($cases.Count) cases. The gate runner will refuse this run: a prediction count that does not match the case count can never pass."
    $cases = $cases[0..($Limit - 1)]
}

$predictionsPath = Join-Path $probeDir 'predictions.jsonl'
$writer = [System.IO.StreamWriter]::new($predictionsPath, $false, $utf8NoBom)
$emptyDecodes = 0
$started = [System.Diagnostics.Stopwatch]::StartNew()

try {
    for ($index = 0; $index -lt $cases.Count; $index++) {
        $message = $cases[$index].input
        $prompt = "<|im_start|>system`n$systemPrompt<|im_end|>`n<|im_start|>user`n$($message.Trim()) /no_think<|im_end|>`n<|im_start|>assistant`n"
        $promptPath = Join-Path $probeDir 'prompt.txt'
        [System.IO.File]::WriteAllText($promptPath, $prompt, $utf8NoBom)

        $arguments = @(
            '-m', $model.FullName, '-f', $promptPath,
            '--grammar-file', $grammar, '-n', '384', '-c', '4096', '-b', '512',
            '-t', '4', '-tb', '4', '--top-k', '20', '--top-p', '0.8', '--temp', '0.7',
            '--seed', '42', '--no-conversation', '--single-turn', '--no-display-prompt',
            '--simple-io'
        )
        if ($LoraPath) { $arguments += @('--lora', $LoraPath) }

        # Windows PowerShell wraps every native stderr line as a non-terminating error, and
        # llama.cpp writes both progress and generated text there.
        $previousErrorPreference = $ErrorActionPreference
        $ErrorActionPreference = 'Continue'
        $nativeLines = & $completion @arguments 2>&1
        $exitCode = $LASTEXITCODE
        $ErrorActionPreference = $previousErrorPreference

        $nativeLog = (($nativeLines | ForEach-Object { $_.ToString() }) -join "`n")
        $json = ''
        if ($exitCode -eq 0) {
            $match = [regex]::Match(
                $nativeLog,
                '(?s)\{\s*"schema_version"\s*:\s*"1".*?"needs_emergency_services"\s*:\s*(?:true|false)\s*\}'
            )
            if ($match.Success) { $json = $match.Value -replace '\s*\r?\n\s*', '' }
        }
        if (-not $json) {
            # A failed decode is a failed case, not a missing one. Writing an empty object
            # keeps the line count honest and costs the faithfulness gate, which is what a
            # generation that produced nothing usable deserves.
            $json = '{}'
            $emptyDecodes++
            [System.IO.File]::WriteAllText((Join-Path $probeDir "failed-$index.log"), $nativeLog, $utf8NoBom)
        }
        $writer.WriteLine((@{ output = $json } | ConvertTo-Json -Compress))

        if (($index + 1) % 25 -eq 0) {
            $rate = [Math]::Round(($index + 1) / $started.Elapsed.TotalSeconds, 2)
            Write-Host "$($index + 1)/$($cases.Count) cases, $rate/s, $emptyDecodes empty decodes"
        }
    }
} finally {
    $writer.Dispose()
    $started.Stop()
}

Write-Host ''
Write-Host "wrote $predictionsPath ($($cases.Count) predictions, $emptyDecodes empty decodes) in $([Math]::Round($started.Elapsed.TotalMinutes, 1)) min"
if ($LoraPath) { Write-Host "adapter: $LoraPath" } else { Write-Host 'base model, no adapter — this is the number to beat' }
Write-Host ''

$gateArgs = @('run', '--quiet', '--locked', '-p', 'prohori-core', '--example', 'evaluate_p5_gates', '--',
    $predictionsPath, '--manifest', $manifestPath)
foreach ($claim in $Attest) { $gateArgs += @('--attest', $claim) }

Push-Location $root
try {
    & cargo @gateArgs
    $gateExit = $LASTEXITCODE
} finally {
    Pop-Location
}

if ($gateExit -ne 0) {
    Write-Host ''
    Write-Host 'The adapter does not ship. P0/P1 remains a useful no-model product until it does.'
}
exit $gateExit
