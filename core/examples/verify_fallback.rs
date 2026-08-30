//! Score one model-written answer with the shipped validator, from the host.
//!
//! `tools/probe-fallback.ps1` drives this. The reason it exists at all: every claim about
//! the unmatched-query path so far is a claim about a grammar file and a Rust function, and
//! neither of those has met the actual GGUF. This runs the real generation through the same
//! `fallback::permission` and `fallback::validate` the phone runs, so the evidence is scored
//! by the shipped code rather than by eye.
//!
//! ```text
//! cargo run --locked -p prohori-core --example verify_fallback -- permission "the tv remote is broken"
//! cargo run --locked -p prohori-core --example verify_fallback -- validate "<message>" answer.json
//! ```
//!
//! Exit codes say whether the *tool* worked, never whether the answer was good — a refusal
//! is the design working, and a script that treats it as a crash cannot tell the two apart.
//! The verdict is the first word on stdout: `allowed`, `suppressed`, `accepted`, `refused`.
//!
//! - `0` — a verdict was reached and printed
//! - `1` — this build is broken (the corpus or the safety-net card failed to load)
//! - `2` — wrong arguments

use prohori_core::fallback::{self, ModelWrittenGuidance};
use prohori_core::retrieval::Index;
use prohori_core::{bundled, redflag};
use std::{env, fs, process::ExitCode};

const USAGE: &str = "usage:\n  \
     verify_fallback permission <message>\n  \
     verify_fallback validate <message> <model-output-file>";

fn main() -> ExitCode {
    let mut args = env::args().skip(1);
    let Some(mode) = args.next() else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };
    let Some(message) = args.next() else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };

    let (corpus, errors) = bundled::corpus();
    if !errors.is_empty() {
        for error in &errors {
            eprintln!("bundled corpus is invalid: {error}");
        }
        return ExitCode::FAILURE;
    }
    let index = Index::build(&corpus);

    match mode.as_str() {
        "permission" => {
            report_permission(&index, &message);
            ExitCode::SUCCESS
        }
        "validate" => {
            let Some(path) = args.next() else {
                eprintln!("{USAGE}");
                return ExitCode::from(2);
            };
            let Ok(json) = fs::read_to_string(&path) else {
                eprintln!("could not read model output file {path}");
                return ExitCode::from(2);
            };
            // Permission first, and in that order, because that is the order the phone
            // uses: `Prohori::accept_fallback_output` asks again after generation, so an
            // answer to a message that has become an emergency is thrown away unread.
            if report_permission(&index, &message) {
                report_validation(&json);
            }
            ExitCode::SUCCESS
        }
        other => {
            eprintln!("unknown mode {other}\n{USAGE}");
            ExitCode::from(2)
        }
    }
}

/// Print whether the model may write for this message, and return that answer.
fn report_permission(index: &Index, message: &str) -> bool {
    let rules = redflag::assess(message);
    let permission = fallback::permission(message, &rules, &index.template_search(message, 3));
    match permission.reason() {
        Some(reason) => {
            // Which layer answered instead is the useful half of a suppression: it is how
            // you find out that a query you expected to reach the model is being absorbed
            // by an over-broad `also_called` phrase somewhere in the corpus. Protocol ids
            // rather than titles, because an id is what you go and edit.
            let instead = rules
                .card()
                .and_then(|hit| hit.protocol_id)
                .map(str::to_owned)
                .or_else(|| {
                    index
                        .template_search(message, 1)
                        .first()
                        .map(|hit| format!("{} via {:?}", hit.protocol_id, hit.matched))
                });
            match instead {
                Some(answered_by) => println!("suppressed: {reason} (answered by: {answered_by})"),
                None => println!("suppressed: {reason}"),
            }
            false
        }
        None => {
            println!("allowed");
            true
        }
    }
}

/// Print the validator's verdict, and the answer itself when it survives.
///
/// The text is printed because a red-team pass is a person reading what the model actually
/// said. Every automated check here can pass on an answer that is fluent, number-free and
/// useless, and no test in this repository can tell you that.
fn report_validation(json: &str) {
    match fallback::validate(json) {
        Ok(written) => {
            println!("accepted");
            print_answer(&written);
        }
        Err(error) => println!("refused: {error}"),
    }
}

fn print_answer(written: &ModelWrittenGuidance) {
    println!("  reassurance: {}", written.reassurance);
    for step in &written.steps {
        println!("  step: {step}");
    }
    for warning in &written.do_not {
        println!("  do not: {warning}");
    }
}
