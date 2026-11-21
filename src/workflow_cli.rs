use clap::Args;
use file_conversion::CancelToken;
use file_workflows::{WorkflowKind, WorkflowRequest};
use serde_json::{json, Value};
use std::path::PathBuf;

#[derive(Args)]
pub struct WorkflowArgs {
    /// A tool kind from `workflows --json`, such as subtitle_convert or pdf_merge
    kind: WorkflowKind,
    /// Local inputs (omit only for an explicitly requested URL download)
    inputs: Vec<PathBuf>,
    /// Destination for new output files; existing files are never replaced
    #[clap(long)]
    output_dir: PathBuf,
    /// Tool options as one JSON object; inspect `workflows --json` for fields
    #[clap(long, default_value = "{}")]
    options: String,
    /// JSON summary on stdout, JSON queue progress on stderr
    #[clap(long)]
    json: bool,
}

pub fn list(as_json: bool) {
    let tools = file_workflows::tools();
    if as_json {
        println!("{}", json!({"schema":"file-workflows/v1","tools":tools}));
    } else {
        for tool in tools {
            println!("{} · {}: {}", tool.kind, tool.category, tool.description);
        }
    }
}

pub fn run(arguments: WorkflowArgs) -> i32 {
    let options: Value = match serde_json::from_str(&arguments.options) {
        Ok(Value::Object(options)) => Value::Object(options),
        Ok(_) => return fail(arguments.json, "--options must be a JSON object"),
        Err(error) => return fail(arguments.json, &format!("Invalid --options JSON: {error}")),
    };
    let requests = plan_requests(
        arguments.kind,
        arguments.inputs,
        arguments.output_dir,
        options,
    );
    let report = file_workflows::run_queue(&requests, &CancelToken::default(), |event| {
        if arguments.json {
            eprintln!(
                "{}",
                serde_json::to_string(&event).expect("serializable workflow event")
            );
        } else {
            eprintln!(
                "[{}] {:?} {:>3.0}% {}",
                event.index + 1,
                event.state,
                event.progress * 100.0,
                event.message
            );
        }
    });
    let success = report.success();
    if arguments.json {
        println!(
            "{}",
            json!({"schema":"file-workflows/v1","ok":success,"result":report,"error":null})
        );
    } else {
        for job in &report.jobs {
            if let Some(error) = &job.error {
                eprintln!("{}: {error}", arguments.kind);
            } else {
                for output in &job.outputs {
                    println!("{}", output.display());
                }
            }
        }
    }
    if success {
        0
    } else {
        1
    }
}

/// Single-document PDF tools need one job per input. Compositional tools keep
/// their input order together, and subtitle/document backends own their batch.
fn plan_requests(
    kind: WorkflowKind,
    inputs: Vec<PathBuf>,
    output_dir: PathBuf,
    options: Value,
) -> Vec<WorkflowRequest> {
    let single_pdf = matches!(
        kind,
        WorkflowKind::PdfSplit
            | WorkflowKind::PdfRotate
            | WorkflowKind::PdfCompress
            | WorkflowKind::PdfWatermark
            | WorkflowKind::PdfMetadata
            | WorkflowKind::PdfToImages
            | WorkflowKind::PdfToText
    );
    if single_pdf && inputs.len() > 1 {
        inputs
            .into_iter()
            .map(|input| WorkflowRequest {
                kind,
                inputs: vec![input],
                output_dir: output_dir.clone(),
                options: options.clone(),
            })
            .collect()
    } else {
        // Preserve an empty request so the backend reports its normal missing
        // input error rather than returning an uninformative empty queue.
        vec![WorkflowRequest {
            kind,
            inputs,
            output_dir,
            options,
        }]
    }
}

fn fail(as_json: bool, message: &str) -> i32 {
    if as_json {
        println!(
            "{}",
            json!({"schema":"file-workflows/v1","ok":false,"result":null,"error":message})
        );
    } else {
        eprintln!("Workflow: {message}");
    }
    1
}

pub fn parse_error(message: &str) {
    fail(true, message);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_document_pdf_tools_plan_ordered_independent_jobs() {
        for kind in [
            WorkflowKind::PdfSplit,
            WorkflowKind::PdfRotate,
            WorkflowKind::PdfCompress,
            WorkflowKind::PdfWatermark,
            WorkflowKind::PdfMetadata,
            WorkflowKind::PdfToImages,
            WorkflowKind::PdfToText,
        ] {
            let inputs = vec![PathBuf::from("first.pdf"), PathBuf::from("second.pdf")];
            let options = json!({"pages":"1-2","test_value":"preserve options"});
            let jobs = plan_requests(kind, inputs.clone(), "outputs".into(), options.clone());
            assert_eq!(jobs.len(), 2);
            for (index, job) in jobs.iter().enumerate() {
                assert_eq!(job.kind, kind);
                assert_eq!(job.inputs, vec![inputs[index].clone()]);
                assert_eq!(job.output_dir, PathBuf::from("outputs"));
                assert_eq!(job.options, options);
            }
        }
    }

    #[test]
    fn merge_image_subtitle_and_document_tools_keep_their_group() {
        for kind in [
            WorkflowKind::PdfMerge,
            WorkflowKind::ImagesToPdf,
            WorkflowKind::SubtitleConvert,
            WorkflowKind::SubtitleShift,
            WorkflowKind::SubtitleFps,
            WorkflowKind::SubtitleClean,
            WorkflowKind::DocumentConvert,
        ] {
            let inputs = vec![PathBuf::from("first"), PathBuf::from("second")];
            let jobs = plan_requests(kind, inputs.clone(), "outputs".into(), json!({}));
            assert_eq!(jobs.len(), 1);
            assert_eq!(jobs[0].inputs, inputs);
        }
    }

    #[test]
    fn missing_inputs_and_explicit_downloads_reach_backend_validation() {
        for kind in [WorkflowKind::PdfRotate, WorkflowKind::Download] {
            let jobs = plan_requests(
                kind,
                vec![],
                "outputs".into(),
                json!({"url":"https://example.com/file"}),
            );
            assert_eq!(jobs.len(), 1);
            assert!(jobs[0].inputs.is_empty());
        }
    }
}
