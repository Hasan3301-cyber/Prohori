#!/usr/bin/env python3
"""QLoRA fine-tune of the shipping base model on the P5 dataset.

    python3 tools/train-p5-adapter.py --dataset model/datasets/p5 --out model/artifacts/p5-lora

`PLAN.md` §8: QLoRA, 4-bit, single 24 GB GPU (or a rented A100). The adapter is kept
separate from the base weights so an update is cheap; merging and quantising to GGUF
Q4_K_M is a later step, gated on `tools/probe-p5-adapter.ps1` passing.

What this script trains on is *format and selection*: the slot JSON in
`data/grammar/triage.gbnf` order, and which card an input names. It never trains on
protocol text. The advice a caller reads is rendered verbatim from `data/firstaid/` by
code that has no model in it, and that is what `PLAN.md` §1 is protecting.

# It refuses more readily than it trains

Four refusals, all before a single GPU second is spent:

* **Digest mismatch.** The dataset is deterministic. If `train.jsonl` or `eval.jsonl`
  does not hash to what `manifest.json` recorded, the files were edited by hand and the
  run manifest would be recording a fiction. Re-run `build_p5_dataset`.
* **Split leak.** Any eval input that also appears in train makes the eval numbers a
  measurement of memory. Checked exactly, on the full input string.
* **Prompt drift.** The chat template here must match `tools/probe-p2-model.ps1`
  character for character, and the system prompt is read from the same file the app
  embeds. An adapter trained against a prompt the app does not send is an adapter tuned
  for a program nobody runs.
* **Unreviewed data.** `manifest.json` carries `clinical_review: null` and this script
  will not pretend otherwise. It trains anyway — training on unreviewed data is fine,
  *shipping* it is not — but the run manifest inherits the null, so
  `core/examples/evaluate_p5_gates.rs` still has nothing to attest with.

# Loss is on the completion only

The prompt is masked out with -100. Computing loss over the system prompt would spend
most of the gradient teaching the model to recite instructions it is already given at
inference time.

# Not run in this repository's development environment

This file is a recipe, not evidence. `model/artifacts/p5-run-manifest.json` is what turns it
into evidence, and only a real run produces one.

The development machine has a Python interpreter and an RTX 3050 Laptop, but the GPU has
4 GB of VRAM and the installed torch is a `+cpu` build, so `torch.cuda.is_available()` is
False here and nothing below the refusals has ever executed. 4 GB is genuinely tight for
this even with 4-bit weights and 8-bit optimiser states: expect to drop to
`--batch-size 1 --grad-accum 16 --max-length 512 --optim adamw_8bit` and still risk an OOM.
`--max-length 512` costs no data — the longest record in this dataset is under 500 tokens —
and `adamw_8bit` is needed because the paged default pages through CUDA unified memory,
which Windows does not support. A free Colab or Kaggle T4 (16 GB) is the path of least
resistance, which is exactly why `--precision` defaults to `auto` — a T4 is Turing and
cannot do bf16.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import random
import sys
from dataclasses import dataclass
from pathlib import Path

SEED = 20260821

# Must match tools/probe-p2-model.ps1 exactly. Qwen3 chat template, thinking disabled.
PROMPT_TEMPLATE = (
    "<|im_start|>system\n{system}<|im_end|>\n"
    "<|im_start|>user\n{message} /no_think<|im_end|>\n"
    "<|im_start|>assistant\n"
)

REPO = Path(__file__).resolve().parent.parent
SYSTEM_PROMPT_PATH = REPO / "data" / "prompts" / "triage-system.txt"


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1 << 20), b""):
            digest.update(chunk)
    return digest.hexdigest()


def load_jsonl(path: Path) -> list[dict]:
    rows = []
    with path.open("r", encoding="utf-8") as handle:
        for line_number, line in enumerate(handle, start=1):
            line = line.strip()
            if not line:
                continue
            try:
                rows.append(json.loads(line))
            except json.JSONDecodeError as error:
                raise SystemExit(f"{path}:{line_number}: {error}") from error
    return rows


@dataclass
class Refusals:
    """Every check that must pass before the GPU is touched."""

    dataset_dir: Path

    def run(self) -> tuple[dict, list[dict], list[dict]]:
        manifest_path = self.dataset_dir / "manifest.json"
        train_path = self.dataset_dir / "train.jsonl"
        eval_path = self.dataset_dir / "eval.jsonl"
        for path in (manifest_path, train_path, eval_path):
            if not path.is_file():
                raise SystemExit(
                    f"missing {path}. Run:\n"
                    "  cargo run --locked -p prohori-core --example build_p5_dataset"
                )
        manifest = json.loads(manifest_path.read_text(encoding="utf-8"))

        for name, path, key in (
            ("train.jsonl", train_path, "train_sha256"),
            ("eval.jsonl", eval_path, "eval_sha256"),
        ):
            actual = sha256(path)
            if actual != manifest.get(key):
                raise SystemExit(
                    f"{name} hashes to {actual} but manifest.json records "
                    f"{manifest.get(key)}. The dataset generator is deterministic, so "
                    "these files were edited by hand. Rebuild rather than train on them."
                )

        train = load_jsonl(train_path)
        evaluation = load_jsonl(eval_path)
        if len(train) != manifest.get("train_count") or len(evaluation) != manifest.get("eval_count"):
            raise SystemExit("row counts disagree with the manifest")

        train_inputs = {row["input"] for row in train}
        leaked = sorted(row["input"] for row in evaluation if row["input"] in train_inputs)
        if leaked:
            raise SystemExit(
                f"{len(leaked)} eval inputs also appear in train, starting with "
                f"{leaked[0]!r}. Eval numbers would be measuring memorisation."
            )

        if not SYSTEM_PROMPT_PATH.is_file():
            raise SystemExit(f"missing {SYSTEM_PROMPT_PATH}")

        if manifest.get("clinical_review") is not None:
            print(
                "note: manifest.json claims a clinical review. This script does not "
                "verify that claim; core/examples/evaluate_p5_gates.rs is where it has "
                "to be made explicitly.",
                file=sys.stderr,
            )
        else:
            print(
                "This dataset is NOT clinically reviewed. Training on it is fine. "
                "Shipping the result is not, and the P5 gate will say so.",
                file=sys.stderr,
            )
        return manifest, train, evaluation


def build_records(rows: list[dict], system_prompt: str) -> list[dict]:
    """Prompt/completion pairs. The completion is the slot JSON, verbatim."""
    records = []
    for row in rows:
        prompt = PROMPT_TEMPLATE.format(system=system_prompt, message=row["input"].strip())
        records.append({"prompt": prompt, "completion": row["output"] + "<|im_end|>"})
    return records


def tokenize(records, tokenizer, max_length: int):
    """Mask the prompt so loss lands only on the slot JSON."""

    def encode(record):
        prompt_ids = tokenizer(record["prompt"], add_special_tokens=False)["input_ids"]
        completion_ids = tokenizer(record["completion"], add_special_tokens=False)["input_ids"]
        input_ids = (prompt_ids + completion_ids)[:max_length]
        labels = ([-100] * len(prompt_ids) + completion_ids)[:max_length]
        return {
            "input_ids": input_ids,
            "attention_mask": [1] * len(input_ids),
            "labels": labels,
        }

    return [encode(record) for record in records]


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--dataset", type=Path, default=REPO / "model" / "datasets" / "p5")
    parser.add_argument("--out", type=Path, default=REPO / "model" / "artifacts" / "p5-lora")
    parser.add_argument("--base", default="Qwen/Qwen3-1.7B")
    parser.add_argument("--epochs", type=float, default=2.0)
    parser.add_argument("--lr", type=float, default=1e-4)
    parser.add_argument("--batch-size", type=int, default=8)
    parser.add_argument("--grad-accum", type=int, default=2)
    parser.add_argument("--max-length", type=int, default=768)
    parser.add_argument("--lora-r", type=int, default=16)
    parser.add_argument("--lora-alpha", type=int, default=32)
    parser.add_argument("--lora-dropout", type=float, default=0.05)
    parser.add_argument(
        "--precision",
        choices=("auto", "bf16", "fp16"),
        default="auto",
        help=(
            "compute dtype. auto picks bf16 on Ampere or newer and fp16 otherwise. A T4 -- "
            "the free Colab and Kaggle GPU -- is Turing and has no bf16, so this is what "
            "makes the only GPU most people can reach able to run this at all"
        ),
    )
    parser.add_argument(
        "--optim",
        choices=("paged_adamw_8bit", "adamw_8bit", "adamw_torch"),
        default="paged_adamw_8bit",
        help=(
            "optimiser. The paged default spills optimiser state to host RAM under memory "
            "pressure, which is what makes a small card viable -- but it does that through "
            "CUDA unified memory, which Windows does not support. Use adamw_8bit there: "
            "same 8-bit state, no paging, so an OOM is an OOM instead of a hang"
        ),
    )
    parser.add_argument(
        "--dry-run",
        action="store_true",
        help="run every refusal and print the plan, without importing torch",
    )
    args = parser.parse_args()

    manifest, train_rows, eval_rows = Refusals(args.dataset).run()
    system_prompt = SYSTEM_PROMPT_PATH.read_text(encoding="utf-8").strip()

    train_records = build_records(train_rows, system_prompt)
    eval_records = build_records(eval_rows, system_prompt)
    print(f"train {len(train_records)} / eval {len(eval_records)}", file=sys.stderr)

    if args.dry_run:
        print(json.dumps({"plan": vars(args) | {"seed": SEED}}, default=str, indent=2))
        return 0

    # Imported late so --dry-run works on a machine with no CUDA stack at all.
    import numpy
    import torch
    import transformers
    from datasets import Dataset
    from peft import LoraConfig, get_peft_model, prepare_model_for_kbit_training
    from transformers import (
        AutoModelForCausalLM,
        AutoTokenizer,
        BitsAndBytesConfig,
        DataCollatorForSeq2Seq,
        Trainer,
        TrainingArguments,
    )

    random.seed(SEED)
    numpy.random.seed(SEED)
    torch.manual_seed(SEED)
    transformers.set_seed(SEED)

    # transformers 5 removed part of the TrainingArguments surface this script uses --
    # `warmup_ratio` is the first one it hits. Left unchecked that surfaces as a TypeError
    # from inside a constructor, which reads like a bug in this file rather than a version
    # mismatch, and it happens *after* the base weights have already been downloaded and
    # quantised. Fail here instead, and name the fix.
    if int(transformers.__version__.split(".")[0]) >= 5:
        raise SystemExit(
            f"transformers {transformers.__version__} is installed. This script targets the "
            "4.x TrainingArguments surface. Install \"transformers<5\", or port the "
            "TrainingArguments block below deliberately -- do not let a kwarg be dropped "
            "silently, because warmup and schedule changes do not announce themselves in "
            "the loss curve."
        )

    if not torch.cuda.is_available():
        raise SystemExit("no CUDA device; PLAN.md §8 asks for a 24 GB GPU or a rented A100")

    # bf16 needs Ampere (SM 8.0) or newer. A T4 is Turing (SM 7.5), and a hardcoded bf16
    # made this script refuse to start on the free Colab and Kaggle GPUs -- the only ones
    # most people can actually get to for a 1.7B QLoRA. Resolved here, after the import,
    # because answering the question needs torch.
    #
    # Deliberately NOT `torch.cuda.is_bf16_supported()`. That returns True on a T4: below
    # SM 8.0 it falls through to probing whether a bf16 tensor can be created at all, and
    # Turing can -- by emulation, off the tensor cores. Trusting it picked bf16 on the exact
    # device this flag exists to protect, which is a silent slow path rather than an error.
    # Compute capability is the fact; tensor-creation is a proxy for a different question.
    capability = torch.cuda.get_device_capability()
    native_bf16 = capability[0] >= 8
    requested_precision = args.precision
    if args.precision == "auto":
        precision = "bf16" if native_bf16 else "fp16"
    else:
        precision = args.precision
    if precision == "bf16" and not native_bf16:
        raise SystemExit(
            f"--precision bf16 was asked for, but {torch.cuda.get_device_name(0)} is compute "
            f"capability {capability[0]}.{capability[1]} and bf16 needs 8.0. Emulated bf16 "
            "would run and be slow. Use --precision fp16, or auto to pick per device."
        )
    compute_dtype = torch.bfloat16 if precision == "bf16" else torch.float16
    # The run manifest must record what ran, not what was asked for: `auto` is a request and
    # `fp16` is a fact, and an adapter's loss curve is not reproducible without knowing which.
    args.precision = precision
    print(
        f"precision {precision} on {torch.cuda.get_device_name(0)} "
        f"(SM {capability[0]}.{capability[1]}, native bf16 {native_bf16})"
        + (f", requested {requested_precision}" if requested_precision == "auto" else ""),
        file=sys.stderr,
    )

    tokenizer = AutoTokenizer.from_pretrained(args.base)
    tokenizer.pad_token = tokenizer.pad_token or tokenizer.eos_token
    model = AutoModelForCausalLM.from_pretrained(
        args.base,
        quantization_config=BitsAndBytesConfig(
            load_in_4bit=True,
            bnb_4bit_quant_type="nf4",
            bnb_4bit_use_double_quant=True,
            bnb_4bit_compute_dtype=compute_dtype,
        ),
        device_map={"": 0},
    )
    model = prepare_model_for_kbit_training(model)
    model = get_peft_model(
        model,
        LoraConfig(
            r=args.lora_r,
            lora_alpha=args.lora_alpha,
            lora_dropout=args.lora_dropout,
            bias="none",
            task_type="CAUSAL_LM",
            target_modules=[
                "q_proj",
                "k_proj",
                "v_proj",
                "o_proj",
                "gate_proj",
                "up_proj",
                "down_proj",
            ],
        ),
    )
    model.print_trainable_parameters()

    train_dataset = Dataset.from_list(tokenize(train_records, tokenizer, args.max_length))
    eval_dataset = Dataset.from_list(tokenize(eval_records, tokenizer, args.max_length))

    args.out.mkdir(parents=True, exist_ok=True)
    trainer = Trainer(
        model=model,
        args=TrainingArguments(
            output_dir=str(args.out / "checkpoints"),
            num_train_epochs=args.epochs,
            per_device_train_batch_size=args.batch_size,
            gradient_accumulation_steps=args.grad_accum,
            learning_rate=args.lr,
            lr_scheduler_type="cosine",
            warmup_ratio=0.03,
            logging_steps=25,
            eval_strategy="epoch",
            save_strategy="epoch",
            bf16=precision == "bf16",
            fp16=precision == "fp16",
            optim=args.optim,
            gradient_checkpointing=True,
            report_to=[],
            seed=SEED,
            data_seed=SEED,
        ),
        train_dataset=train_dataset,
        eval_dataset=eval_dataset,
        data_collator=DataCollatorForSeq2Seq(tokenizer, padding=True, label_pad_token_id=-100),
    )
    result = trainer.train()
    model.save_pretrained(args.out)
    tokenizer.save_pretrained(args.out)

    run_manifest = {
        "schema_version": 1,
        "seed": SEED,
        "base_model": args.base,
        "prompt_template_sha256": hashlib.sha256(PROMPT_TEMPLATE.encode()).hexdigest(),
        "system_prompt_sha256": sha256(SYSTEM_PROMPT_PATH),
        "dataset": {
            "label_source_sha256": manifest.get("label_source_sha256"),
            "train_sha256": manifest.get("train_sha256"),
            "eval_sha256": manifest.get("eval_sha256"),
            "train_count": manifest.get("train_count"),
            "eval_count": manifest.get("eval_count"),
            "rule_coverage_lost": manifest.get("rule_coverage_lost"),
            # Inherited, never asserted. Only a person can change this.
            "clinical_review": manifest.get("clinical_review"),
        },
        "hyperparameters": {
            k: v
            for k, v in vars(args).items()
            if k not in {"dataset", "out", "dry_run"}
        },
        "versions": {
            "torch": torch.__version__,
            "transformers": transformers.__version__,
            "python": sys.version.split()[0],
        },
        "gpu": torch.cuda.get_device_name(0),
        # `auto` resolves per device, so a reader can tell an fp16 run on a T4 from an fp16
        # run someone asked for on a card that could have done bf16.
        "precision_requested": requested_precision,
        "train_runtime_seconds": result.metrics.get("train_runtime"),
        "train_loss": result.metrics.get("train_loss"),
    }
    manifest_path = REPO / "model" / "artifacts" / "p5-run-manifest.json"
    manifest_path.parent.mkdir(parents=True, exist_ok=True)
    manifest_path.write_text(json.dumps(run_manifest, indent=2) + "\n", encoding="utf-8")
    print(f"wrote {manifest_path}", file=sys.stderr)
    print(
        "Next: tools/probe-p5-adapter.ps1. The adapter does not ship until every "
        "PLAN.md §8 gate passes and someone attests to the three claims the gate "
        "cannot compute.",
        file=sys.stderr,
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
