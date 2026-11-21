use clap::Args;
use file_conversion::{output_requests, run_queue, CancelToken, ConversionRequest, TargetFormat};
use std::path::PathBuf;

#[derive(Args)]
pub struct ConvertArgs {
    /// Local input files; directories and URLs are rejected
    #[clap(required = true)]
    inputs: Vec<PathBuf>,
    /// mp4, mkv, mov, webm, mp3, m4a, wav, flac, ogg, png, jpg, webp, tiff
    #[clap(long)]
    format: TargetFormat,
    /// Exact output filename (requires a single input; never overwrites)
    #[clap(short, long, conflicts_with = "output_dir")]
    output: Option<PathBuf>,
    /// Destination for <stem>.converted.<format> (default: ./converted)
    #[clap(long)]
    output_dir: Option<PathBuf>,
    /// One JSON summary on stdout, progress events as JSON lines on stderr
    #[clap(long)]
    json: bool,
}

pub fn doctor(json: bool) {
    let tools = file_conversion::dependencies();
    let formats: Vec<_> = TargetFormat::all()
        .iter()
        .map(ToString::to_string)
        .collect();
    if json {
        println!(
            "{}",
            serde_json::json!({"schema":"file-conversion/v1", "dependencies":tools,"formats":formats,
                "workflow_dependencies":file_workflows::dependencies(),"workflows":file_workflows::tools()})
        );
    } else {
        println!("Local file conversion dependencies:");
        for tool in tools {
            println!(
                "  {}: {}",
                tool.name,
                tool.path
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| "missing".into())
            );
        }
        println!("Output formats: {}", formats.join(", "));
        println!("Additional workflow dependencies:");
        for tool in file_workflows::dependencies() {
            println!(
                "  {}: {}",
                tool.name,
                tool.path
                    .map(|path| path.display().to_string())
                    .unwrap_or_else(|| "missing".into())
            );
        }
        println!("Use `workflows --json` to inspect PDF, subtitle, document and download tools.");
    }
}

pub fn run(options: ConvertArgs) -> i32 {
    let requests = if let Some(output) = options.output {
        if options.inputs.len() != 1 {
            return fail(
                options.json,
                "--output requires exactly one input; use --output-dir for a batch",
            );
        }
        Ok(vec![ConversionRequest {
            input: options.inputs[0].clone(),
            output,
            format: options.format,
        }])
    } else {
        output_requests(
            &options.inputs,
            &options.output_dir.unwrap_or_else(|| "converted".into()),
            options.format,
        )
    };
    let requests = match requests {
        Ok(requests) => requests,
        Err(error) => return fail(options.json, &format!("{error:#}")),
    };
    let report = run_queue(&requests, &CancelToken::default(), |event| {
        if options.json {
            eprintln!(
                "{}",
                serde_json::to_string(&event).expect("serializable progress")
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
    let ok = report.success();
    if options.json {
        println!(
            "{}",
            serde_json::json!({"schema":"file-conversion/v1","ok":ok,"result":report,"error":null})
        );
    } else {
        for job in &report.jobs {
            match &job.error {
                Some(error) => eprintln!("[{}] {}: {error}", job.index + 1, job.output.display()),
                None => println!(
                    "[{}] {:?}: {}",
                    job.index + 1,
                    job.state,
                    job.output.display()
                ),
            }
        }
    }
    if ok {
        0
    } else {
        1
    }
}

fn fail(json: bool, message: &str) -> i32 {
    if json {
        println!(
            "{}",
            serde_json::json!({"schema":"file-conversion/v1","ok":false,"result":null,"error":message})
        );
    } else {
        eprintln!("Conversion: {message}");
    }
    1
}

pub fn parse_error(message: &str) {
    fail(true, message);
}
