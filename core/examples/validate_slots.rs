use prohori_core::{bundled, inference, redflag};
use std::{env, fs, process::ExitCode};

fn main() -> ExitCode {
    let mut args = env::args().skip(1);
    let Some(message) = args.next() else {
        eprintln!("usage: validate_slots <user-message> <model-output-file>");
        return ExitCode::from(2);
    };
    let Some(output_path) = args.next() else {
        eprintln!("missing model output file");
        return ExitCode::from(2);
    };
    let expected_protocol = args.next();
    let Ok(output) = fs::read_to_string(output_path) else {
        eprintln!("could not read model output file");
        return ExitCode::from(2);
    };
    let (corpus, errors) = bundled::corpus();
    if !errors.is_empty() {
        eprintln!("bundled corpus is invalid");
        return ExitCode::FAILURE;
    }
    let rules = redflag::assess(&message);
    let floor = rules.severity();
    match inference::validate_slots(output.trim(), &corpus, floor) {
        Ok(mut slots) => {
            slots.retain_grounded_symptoms(&message);
            let selected_protocol = rules
                .card()
                .and_then(|hit| hit.protocol_id)
                .or(slots.protocol_id.as_deref());
            if expected_protocol.as_deref().is_some_and(|expected| {
                selected_protocol.is_none_or(|selected| selected != expected)
            }) {
                eprintln!(
                    "rejected: final protocol {:?}, expected {:?}",
                    selected_protocol, expected_protocol
                );
                return ExitCode::FAILURE;
            }
            println!(
                "accepted severity={:?} final_protocol={selected_protocol:?} model_protocol={:?} grounded_symptoms={:?}",
                slots.severity, slots.protocol_id, slots.symptoms
            );
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("rejected: {error}");
            ExitCode::FAILURE
        }
    }
}
