//! Score a model's eval-set output against every `PLAN.md` §8 gate and exit non-zero
//! unless the adapter may ship.
//!
//! ```text
//! cargo run --locked -p prohori-core --example evaluate_p5_gates -- \
//!     predictions.jsonl [--manifest model/datasets/p5/manifest.json] [--attest <id>]...
//! ```
//!
//! `predictions.jsonl` is one line per eval case, **in eval-set order**: either the raw
//! constrained model output, or a JSON object with an `"output"` string carrying it. A line
//! that is not valid slot JSON is scored as a failed case rather than skipped —
//! `tools/probe-p5-adapter.ps1` writes an empty object for a decode that produced nothing,
//! so a crashed generation costs the gate instead of shrinking it.
//!
//! # This runner folds the shipped pipeline, not a copy of it
//!
//! For each case it calls `redflag::assess`, then `inference::validate_slots` with that
//! floor, then `retain_grounded_symptoms`, and ranks the rule card ahead of the model's
//! pick — the same order `core/examples/validate_slots.rs` and the FFI produce. The gates
//! therefore score the program that ships, not the probe script's opinion of it.
//!
//! # Why it will refuse here
//!
//! `--attest clinician_reviewed_data` and `--attest p2_device_budget` are facts about the
//! world, not about this repository. Nobody working in this tree can supply them honestly,
//! so a run started here fails by design. That refusal is the deliverable.

use prohori_core::eval::{Attestation, Prediction};
use prohori_core::{bundled, dataset, eval, inference, redflag, severity::Severity};
use std::{env, fs, process::ExitCode};

fn main() -> ExitCode {
    let mut predictions_path: Option<String> = None;
    let mut manifest_path: Option<String> = None;
    let mut attestations: Vec<Attestation> = Vec::new();
    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--attest" => match args.next() {
                Some(id) => attestations.push(eval::attest(&id, true)),
                None => {
                    eprintln!("--attest needs an id");
                    return ExitCode::from(2);
                }
            },
            "--manifest" => match args.next() {
                Some(path) => manifest_path = Some(path),
                None => {
                    eprintln!("--manifest needs a path");
                    return ExitCode::from(2);
                }
            },
            other if predictions_path.is_none() => predictions_path = Some(other.to_owned()),
            other => {
                eprintln!("unexpected argument {other:?}");
                return ExitCode::from(2);
            }
        }
    }
    let Some(predictions_path) = predictions_path else {
        eprintln!(
            "usage: evaluate_p5_gates <predictions.jsonl> [--manifest <path>] [--attest <id>]..."
        );
        eprintln!();
        eprintln!("required attestations:");
        for (id, label) in eval::REQUIRED_ATTESTATIONS {
            eprintln!("  --attest {id:<26} {label}");
        }
        return ExitCode::from(2);
    };

    let (corpus, corpus_errors) = bundled::corpus();
    if !corpus_errors.is_empty() {
        eprintln!("bundled corpus is invalid; the gates would be scoring nothing");
        return ExitCode::FAILURE;
    }
    let built = dataset::build(&corpus);

    // The dataset is deterministic, so a manifest that disagrees means these predictions
    // were produced against different data. Scoring them anyway would be a lie with a
    // number attached.
    if let Some(path) = &manifest_path {
        let Ok(text) = fs::read_to_string(path) else {
            eprintln!("could not read manifest {path}");
            return ExitCode::FAILURE;
        };
        let recorded = serde_json::from_str::<serde_json::Value>(&text)
            .ok()
            .and_then(|value| {
                value
                    .get("eval_sha256")
                    .and_then(|v| v.as_str())
                    .map(ToOwned::to_owned)
            });
        if recorded.as_deref() != Some(built.manifest.eval_sha256.as_str()) {
            eprintln!("manifest eval_sha256 {recorded:?} does not match the eval set this");
            eprintln!(
                "build produces ({}). Rebuild the dataset and re-run the probe.",
                built.manifest.eval_sha256
            );
            return ExitCode::FAILURE;
        }
    }

    let Ok(raw) = fs::read_to_string(&predictions_path) else {
        eprintln!("could not read {predictions_path}");
        return ExitCode::FAILURE;
    };
    let outputs: Vec<String> = raw
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(model_output)
        .collect();

    let cases = dataset::eval_cases(&built.eval);
    let predictions: Vec<Prediction> = built
        .eval
        .iter()
        .zip(&outputs)
        .map(|(example, output)| predict(&example.input, output, &corpus))
        .collect();

    // Rendered card text for the readability gate: the steps a caller reads, measured the
    // same way `core/tests/corpus_integrity.rs` measures them.
    let rendered: Vec<(String, String)> = corpus
        .protocols()
        .map(|protocol| {
            let text = protocol
                .steps
                .iter()
                .map(|step| step.text.as_str())
                .collect::<Vec<_>>()
                .join(" ");
            (protocol.id.clone(), text)
        })
        .collect();
    let readability: Vec<(&str, &str)> = rendered
        .iter()
        .map(|(id, text)| (id.as_str(), text.as_str()))
        .collect();

    let report = eval::evaluate(&cases, &predictions, &readability, &attestations);
    print_report(
        &report,
        cases.len(),
        predictions.len(),
        &built.manifest.rule_coverage_lost,
    );

    if report.may_ship() {
        println!(
            "PASS — every PLAN.md §8 gate cleared on this data, with every attestation supplied."
        );
        ExitCode::SUCCESS
    } else {
        println!(
            "BLOCKED — {} gate(s) failed. The adapter does not ship.",
            report.failures.len()
        );
        ExitCode::FAILURE
    }
}

/// Accept a bare model output line or an object wrapping it in `"output"`.
fn model_output(line: &str) -> String {
    serde_json::from_str::<serde_json::Value>(line)
        .ok()
        .and_then(|value| {
            value
                .get("output")
                .and_then(|v| v.as_str())
                .map(ToOwned::to_owned)
        })
        .unwrap_or_else(|| line.to_owned())
}

/// Fold one model output through the deterministic layers, exactly as the app does.
fn predict(message: &str, output: &str, corpus: &prohori_core::protocol::Corpus) -> Prediction {
    let assessment = redflag::assess(message);
    let floor = assessment.severity();
    let rule_card = assessment.card().and_then(|hit| hit.protocol_id);

    match inference::validate_slots(output, corpus, floor) {
        Ok(mut slots) => {
            let claimed = slots.symptoms.len();
            slots.retain_grounded_symptoms(message);
            // Faithful means the pipeline showed nothing the model invented. A dropped
            // symptom is a paraphrase the app refused, and it counts against the gate.
            let faithful = slots.symptoms.len() == claimed;
            let mut ranked: Vec<String> = Vec::new();
            if let Some(id) = rule_card {
                ranked.push(id.to_owned());
            }
            if let Some(id) = slots.protocol_id.as_deref()
                && Some(id) != rule_card
            {
                ranked.push(id.to_owned());
            }
            Prediction {
                severity: slots.severity,
                ranked_protocols: ranked,
                red_flag_caught: slots.severity == Severity::Critical,
                faithful,
                symptoms: slots.symptoms,
                handed_off: slots.needs_emergency_services,
            }
        }
        // A rejected output is not a missing case. The rule layer still holds, and that is
        // the honest floor to score: whatever the deterministic layer alone would show.
        Err(_) => {
            let severity = floor.unwrap_or(Severity::SelfCare);
            Prediction {
                severity,
                ranked_protocols: rule_card.map(ToOwned::to_owned).into_iter().collect(),
                red_flag_caught: severity == Severity::Critical,
                faithful: false,
                symptoms: Vec::new(),
                handed_off: severity.bypasses_model(),
            }
        }
    }
}

fn print_report(
    report: &eval::GateReport,
    case_count: usize,
    prediction_count: usize,
    rule_coverage_lost: &usize,
) {
    let m = &report.metrics;
    println!("cases {case_count}, predictions {prediction_count}");
    println!();
    println!(
        "undertriage on critical    {:>7.3}%  gate < 2%",
        m.undertriage_rate * 100.0
    );
    println!(
        "red-flag recall            {:>7.3}%  gate 100%",
        m.red_flag_recall * 100.0
    );
    println!(
        "protocol top-1             {:>7.3}%  gate >= 90%",
        m.protocol_top1 * 100.0
    );
    println!(
        "protocol top-3             {:>7.3}%  gate >= 98%",
        m.protocol_top3 * 100.0
    );
    println!(
        "output faithfulness        {:>7.3}%  gate 100%",
        m.faithfulness * 100.0
    );
    println!(
        "degraded slot accuracy     {:>7.3}%  gate >= 95%",
        m.degraded_slot_accuracy * 100.0
    );
    println!(
        "correct handoff            {:>7.3}%  gate >= 95%",
        m.handoff_accuracy * 100.0
    );
    println!(
        "hardest reading grade      {:>7.1}   gate <= 6",
        m.readability_max_grade
    );
    println!();
    println!("{rule_coverage_lost} degraded cases have no red-flag floor underneath them.");
    println!();
    if report.failures.is_empty() {
        println!("no failures");
    } else {
        println!("failures:");
        for failure in &report.failures {
            println!("  - {failure}");
        }
    }
    println!();
}
