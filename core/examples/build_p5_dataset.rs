//! Write the P5 fine-tuning dataset to disk.
//!
//! ```text
//! cargo run --locked -p prohori-core --example build_p5_dataset [output-dir]
//! ```
//!
//! Output defaults to `model/datasets/p5/`. The generator is deterministic, so re-running
//! it overwrites the files with identical bytes; if the digests move, something in the
//! corpus or the label tables moved with them.
//!
//! Nothing written here is reviewed. `manifest.json` says so in a field, and
//! `core/src/eval.rs` will refuse to clear a release without a human attesting otherwise.

use prohori_core::{bundled, dataset};
use std::{env, fs, path::Path, process::ExitCode};

fn main() -> ExitCode {
    let out = env::args()
        .nth(1)
        .unwrap_or_else(|| "model/datasets/p5".to_owned());
    let out = Path::new(&out);

    let (corpus, errors) = bundled::corpus();
    if !errors.is_empty() {
        eprintln!("bundled corpus is invalid; refusing to build a dataset from it:");
        for error in &errors {
            eprintln!("  {error}");
        }
        return ExitCode::FAILURE;
    }

    let built = dataset::build(&corpus);
    let manifest = match serde_json::to_string_pretty(&built.manifest) {
        Ok(json) => format!("{json}\n"),
        Err(error) => {
            eprintln!("could not serialise the manifest: {error}");
            return ExitCode::FAILURE;
        }
    };

    if let Err(error) = fs::create_dir_all(out) {
        eprintln!("could not create {}: {error}", out.display());
        return ExitCode::FAILURE;
    }
    for (name, body) in [
        ("train.jsonl", dataset::jsonl(&built.train)),
        ("eval.jsonl", dataset::jsonl(&built.eval)),
        ("manifest.json", manifest),
    ] {
        let path = out.join(name);
        if let Err(error) = fs::write(&path, body) {
            eprintln!("could not write {}: {error}", path.display());
            return ExitCode::FAILURE;
        }
        println!("wrote {}", path.display());
    }

    let m = &built.manifest;
    println!();
    println!(
        "train        {:>6} examples ({} degraded)",
        m.train_count, m.train_degraded_count
    );
    println!(
        "eval         {:>6} examples ({} degraded)",
        m.eval_count, m.eval_degraded_count
    );
    println!("negatives    {:>6}", m.negative_count);
    println!(
        "held out     {} phrases per card, {} eval frames",
        m.phrases_held_out_per_protocol, m.eval_frame_count
    );
    println!("labels       {}", m.label_source_sha256);
    println!("train sha    {}", m.train_sha256);
    println!("eval sha     {}", m.eval_sha256);
    println!();
    println!(
        "{} degraded inputs no longer trip the red-flag rule that covers them.",
        m.rule_coverage_lost
    );
    println!("On those, nothing sits underneath the model. They are the cases to read first.");
    println!();
    println!("NOT REVIEWED. This is synthetic data generated from the corpus's own search");
    println!("vocabulary. PENDING_CLINICAL_SEVERITY assigns nine cards a placeholder severity");
    println!("that no clinician has seen. The P5 gate will not clear a release until someone");
    println!("attests that it has been reviewed, and this tool cannot make that attestation.");
    ExitCode::SUCCESS
}
