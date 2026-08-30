# Model workflows

P2 uses a pinned official Qwen3 1.7B source, a pinned llama.cpp release, constrained
decoding, and a three-phone performance gate. See [`P2.md`](P2.md) for the exact model
preparation, real-inference probe, and device-evidence commands.

## Optional P5 QLoRA adapter pipeline

The release app does not fetch or trust an adapter automatically. See [`P5.md`](../docs/P5.md)
for the full reasoning; the short version is three commands.

Build the dataset — `input` is stressed English, `output` is the exact slot JSON
`data/grammar/triage.gbnf` accepts, in grammar order. It trains format and protocol
selection only, never new medical instructions:

```bash
cargo run --locked -p prohori-core --example build_p5_dataset
```

Writes `model/datasets/p5/{train,eval}.jsonl` and `manifest.json`. Gitignored: the
generator is deterministic, so the recipe is the artefact. `manifest.json` carries the
per-file digests, the counts, `rule_coverage_lost`, and `clinical_review: null`.

Train (QLoRA, 4-bit NF4, rank 16, alpha 32, dropout 0.05, two epochs, lr 1e-4, seed
20260821, loss masked to the completion). It refuses before touching the GPU if the JSONL
files do not hash to the manifest, if the counts disagree, or if any eval input appears in
train:

```bash
python3 tools/train-p5-adapter.py --dataset model/datasets/p5 --out model/artifacts/p5-lora
```

Then merge, convert with the pinned llama.cpp checkout, quantize to Q4_K_M, and probe.
Run it once without `-LoraPath` first: the base model's score is the number the adapter has
to beat.

```powershell
pwsh tools/probe-p5-adapter.ps1 -LoraPath model\artifacts\p5-lora\p5-lora.gguf
```

The probe hands its predictions to `core/examples/evaluate_p5_gates.rs`, which scores every
`eval::evaluate` gate and exits non-zero unless all three `--attest` claims are supplied
too. Empty, short, or mismatched prediction sets deliberately fail, and a failed decode is
scored as a failed case rather than dropped. Do not copy an adapter or GGUF into a release
unless that report has no failures and the `docs/FIELD_TEST.md` device checklist passed on
three named phones.

Record the base revision, dataset SHA-256, package lock, GPU type, seed, and adapter digest
in the run manifest — `tools/train-p5-adapter.py` writes
`model/artifacts/p5-run-manifest.json` with all of it, and inherits `clinical_review`
rather than asserting it.

Neither tool has been executed in this repository's development environment. Python is
present and `tools/train-p5-adapter.py --dry-run` passes every refusal, but the GPU here has
4 GB of VRAM and the installed torch is a `+cpu` build, so `torch.cuda.is_available()` is
False. A free 16 GB Colab or Kaggle T4 is enough for a 1.7B QLoRA; `--precision auto` exists
because a T4 cannot do bf16. They are recipes until a real run produces a manifest.

Clinical vignettes and any PhysioNet-derived data are intentionally absent from this
repository. They require credentialing, licence review, de-identification, and clinician
sign-off; synthetic placeholders are not evidence that a medical model is safe. The
generated dataset above is exactly such a placeholder, and says so in a field.
