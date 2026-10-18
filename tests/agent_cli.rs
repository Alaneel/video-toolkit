//! Real CLI process checks. No model, credential, or private user task is used.
//! Default tests are offline. The ignored whole-chain case uses synthetic media and FFmpeg.
use serde_json::Value;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);

struct TestDirectory {
    root: PathBuf,
    empty_path: PathBuf,
}

impl TestDirectory {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "video-toolkit-agent-cli-{}-{}",
            std::process::id(),
            NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        let empty_path = root.join("empty executable path");
        fs::create_dir(&empty_path).unwrap();
        Self { root, empty_path }
    }

    fn cli(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_video-toolkit"));
        command
            .current_dir(&self.root)
            .env("PATH", &self.empty_path)
            .stdin(Stdio::null());
        command
    }

    fn json(&self, arguments: &[&str]) -> Output {
        self.cli()
            .args(["agent-task", "--json"])
            .args(arguments)
            .output()
            .unwrap()
    }

    fn media_json(&self, arguments: &[&str]) -> Output {
        // This helper intentionally retains the real PATH for the ignored media test.
        Command::new(env!("CARGO_BIN_EXE_video-toolkit"))
            .current_dir(&self.root)
            .stdin(Stdio::null())
            .args(["agent-task", "--json"])
            .args(arguments)
            .output()
            .unwrap()
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn response(output: &Output, command: &str, ok: bool) -> Value {
    assert!(
        output.stderr.is_empty(),
        "JSON mode must not emit raw stderr"
    );
    let value: Value = serde_json::from_slice(&output.stdout)
        .expect("stdout must contain exactly one JSON value and no text prefix");
    assert!(value.is_object());
    assert_eq!(value["schema"], "video-agent-cli/v1");
    assert_eq!(value["command"], command);
    assert_eq!(value["ok"], ok);
    assert_eq!(output.status.success(), ok);
    if ok {
        assert!(value["error"].is_null());
    } else {
        assert!(value["result"].is_null());
        assert!(value["error"]["code"].is_string());
    }
    value
}

#[test]
fn offline_discovery_is_one_json_object_with_truthful_boundaries() {
    let directory = TestDirectory::new();
    let output = directory.json(&["discover"]);
    let result = response(&output, "discover", true)["result"].clone();
    assert_eq!(result["capability_schema"], "video-agent-capabilities/v1");
    assert_eq!(result["dependency_installation_checked"], false);
    assert_eq!(result["quality"]["semantic"], "not_evaluated");
    assert_eq!(result["iteration"]["cloud_calls"], false);
    assert_eq!(result["iteration"]["maximum_attempts"], 3);
    assert_eq!(result["smart_feedback"]["maximum_rounds"], 3);
    assert_eq!(
        result["smart_feedback"]["desktop_default_maximum_rounds"],
        2
    );
    assert_eq!(
        result["smart_feedback"]["desktop_resumes_saved_round_bound"],
        true
    );
    assert_eq!(
        result["smart_feedback"]["separate_preview_upload_consent_required"],
        true
    );
    assert_eq!(result["smart_feedback"]["automatic_plan_adoption"], false);
    assert_eq!(result["smart_feedback"]["automatic_approval"], false);
    assert_eq!(result["smart_feedback"]["automatic_export"], false);
    assert_eq!(
        result["cloud"]["commands"],
        serde_json::json!(["analyze", "smart-iterate"])
    );
    assert_eq!(result["cloud"]["source_video_uploaded"], false);
    assert_eq!(result["cloud"]["audio_uploaded"], false);
    assert_eq!(result["cloud"]["sparse_samples_only"], true);
    for command in ["smart-iterate", "smart-status", "smart-adopt"] {
        assert!(result["commands"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item == command));
    }
    assert_eq!(
        result["preview"]["maximum_duration_ms"],
        video_agent::feedback::MAX_PREVIEW_MS
    );
    assert_eq!(
        result["preview"]["maximum_clip_count"],
        video_agent::feedback::MAX_CLIPS
    );
    assert_eq!(
        result["preview"]["maximum_media_bytes"],
        video_agent::feedback::MAX_MEDIA_BYTES
    );
    assert_eq!(
        result["preview"]["maximum_plan_bytes"],
        video_agent::feedback::MAX_PLAN_BYTES
    );
    assert_eq!(
        result["preview"]["persistent_attempt_limit"],
        video_agent::feedback::MAX_ATTEMPTS
    );
    assert_eq!(
        result["structured_edit_plan"]["maximum_input_bytes"],
        video_agent::feedback::MAX_PLAN_BYTES
    );
    assert_eq!(
        result["preview"]["media_size_limit_scope"],
        "post_generation_acceptance_not_os_quota"
    );
    assert_eq!(result["safety"]["automatic_approval"], false);
    assert_eq!(result["safety"]["external_hub_installation"], false);
    assert_eq!(
        result["safety"]["final_export_requires_exact_human_approval"],
        true
    );
    assert!(result["dependencies"]
        .as_array()
        .unwrap()
        .iter()
        .any(|tool| {
            tool["tool"] == "ffmpeg" && tool["external"] == true && tool["bundled"] == false
        }));
    let output = directory
        .cli()
        .args(["agent-task", "discover", "--json"])
        .output()
        .unwrap();
    response(&output, "discover", true);
}

#[test]
fn parse_errors_are_json_without_echoing_arguments() {
    let directory = TestDirectory::new();
    for arguments in [
        vec![],
        vec!["discover", "--inert-private-marker"],
        vec!["inert-private-marker"],
        vec![
            "iterate",
            "unused",
            "--candidates",
            "unused.json",
            "--max-attempts",
            "4",
        ],
        vec![
            "iterate",
            "unused",
            "--candidates",
            "unused.json",
            "--max-attempts",
            "0",
        ],
        vec!["smart-iterate", "unused", "--max-rounds", "0"],
        vec!["smart-iterate", "unused", "--max-rounds", "4"],
        vec!["smart-adopt", "unused"],
    ] {
        let output = directory.json(&arguments);
        let value = response(&output, "parse", false);
        assert_eq!(output.status.code(), Some(2));
        assert_eq!(value["error"]["code"], "Invalid");
        assert!(!String::from_utf8_lossy(&output.stdout).contains("inert-private-marker"));
    }
}

#[test]
fn missing_local_task_has_a_sanitized_stable_error() {
    let directory = TestDirectory::new();
    let missing = directory.root.join("private-path-inert-marker");
    let output = directory.json(&["show", missing.to_str().unwrap()]);
    let value = response(&output, "show", false);
    assert_eq!(value["error"]["code"], "Io");
    assert!(!String::from_utf8_lossy(&output.stdout).contains("private-path-inert-marker"));
    assert_eq!(output.status.code(), Some(1));
}

#[test]
fn preview_cloud_consent_is_required_before_opening_a_missing_task_or_provider() {
    let directory = TestDirectory::new();
    let missing = directory.root.join("private-path-inert-marker");
    let output = directory.json(&[
        "smart-iterate",
        missing.to_str().unwrap(),
        "--max-rounds",
        "2",
    ]);
    let value = response(&output, "smart-iterate", false);
    assert_eq!(value["error"]["code"], "Invalid");
    assert_eq!(output.status.code(), Some(1));
    assert!(!missing.exists());
    assert!(!String::from_utf8_lossy(&output.stdout).contains("private-path-inert-marker"));
}

#[test]
fn smart_status_and_explicit_adoption_have_sanitized_missing_task_errors() {
    let directory = TestDirectory::new();
    let missing = directory.root.join("private-path-inert-marker");
    for arguments in [
        vec!["smart-status", missing.to_str().unwrap()],
        vec![
            "smart-adopt",
            missing.to_str().unwrap(),
            "--run-id",
            "inert-private-marker",
        ],
    ] {
        let output = directory.json(&arguments);
        response(&output, arguments[0], false);
        assert_eq!(output.status.code(), Some(1));
        assert!(!String::from_utf8_lossy(&output.stdout).contains("private-path-inert-marker"));
        assert!(!String::from_utf8_lossy(&output.stdout).contains("inert-private-marker"));
    }
}

#[test]
fn bounded_plan_and_candidate_inputs_fail_before_opening_a_task() {
    let directory = TestDirectory::new();
    let malformed = directory.root.join("malformed.json");
    fs::write(&malformed, b"{inert-private-marker").unwrap();
    let oversized = directory.root.join("oversized.json");
    fs::write(&oversized, vec![b' '; 65_537]).unwrap();
    for (file, expected) in [(&malformed, "Json"), (&oversized, "Invalid")] {
        for (command, input_flag) in [("validate", "--plan"), ("iterate", "--candidates")] {
            let output = directory.json(&[command, "no-task", input_flag, file.to_str().unwrap()]);
            let value = response(&output, command, false);
            assert_eq!(value["error"]["code"], expected);
            assert!(!String::from_utf8_lossy(&output.stdout).contains("inert-private-marker"));
        }
    }
}

#[cfg(unix)]
#[test]
fn plan_inputs_reject_symlinks_without_opening_a_task() {
    let directory = TestDirectory::new();
    let actual = directory.root.join("actual.json");
    let link = directory.root.join("link.json");
    fs::write(&actual, b"[]").unwrap();
    std::os::unix::fs::symlink(&actual, &link).unwrap();
    for (command, input_flag) in [
        ("edit", "--plan"),
        ("validate", "--plan"),
        ("iterate", "--candidates"),
    ] {
        let output = directory.json(&[command, "no-task", input_flag, link.to_str().unwrap()]);
        let value = response(&output, command, false);
        assert_eq!(value["error"]["code"], "Invalid");
    }
}

#[test]
fn help_and_version_keep_normal_success_output_and_json_stays_scoped() {
    let directory = TestDirectory::new();
    for arguments in [
        vec!["agent-task", "--json", "--help"],
        vec!["agent-task", "--json", "discover", "--help"],
        vec!["agent-task", "--json", "--version"],
        vec!["agent-task", "--json", "discover", "--version"],
        vec!["--version"],
        vec!["clipper", "--help"],
    ] {
        let output = directory.cli().args(&arguments).output().unwrap();
        assert!(output.status.success());
        assert!(output.stderr.is_empty());
        assert!(!output.stdout.is_empty());
        assert!(serde_json::from_slice::<Value>(&output.stdout).is_err());
        if arguments.first() == Some(&"clipper") {
            assert!(!String::from_utf8_lossy(&output.stdout).contains("--json"));
        }
    }
    let output = directory
        .cli()
        .args(["formats", "--json"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(!output.stderr.is_empty());
}

#[test]
#[ignore = "requires real FFmpeg/ffprobe; synthetic media through the binary JSON CLI, no model or approval"]
fn real_json_cli_preview_quality_iteration_keeps_review_and_export_boundaries() {
    let directory = TestDirectory::new();
    let source = directory.root.join("synthetic source with spaces.mp4");
    let generated = Command::new("ffmpeg")
        .args([
            "-v",
            "error",
            "-nostdin",
            "-n",
            "-f",
            "lavfi",
            "-i",
            "color=c=blue:s=160x90:r=30:d=3",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:sample_rate=48000:duration=3",
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-pix_fmt",
            "yuv420p",
            "-threads",
            "1",
            "-c:a",
            "aac",
        ])
        .arg(&source)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(
        generated.status.success(),
        "Synthetic media generation failed: {}",
        String::from_utf8_lossy(&generated.stderr)
    );
    let original_source = fs::read(&source).unwrap();
    let task_parent = directory.root.join("isolated synthetic tasks");
    let created = response(
        &directory.media_json(&[
            "create",
            source.to_str().unwrap(),
            "--workspace-root",
            task_parent.to_str().unwrap(),
            "--intent",
            "SYNTHETIC CLI feedback fixture; objective duration only, no semantic claim",
            "--target-seconds",
            "2",
        ]),
        "create",
        true,
    );
    let workspace = PathBuf::from(created["result"]["workspace"].as_str().unwrap());
    assert!(workspace.starts_with(fs::canonicalize(&task_parent).unwrap()));
    let workspace_argument = workspace.to_str().unwrap();
    let manual = response(
        &directory.media_json(&["manual-draft", workspace_argument]),
        "manual-draft",
        true,
    );
    let mut plan = manual["result"]["task"]["plan"].clone();
    assert_eq!(plan["clips"].as_array().unwrap().len(), 1);
    plan["clips"][0]["end_ms"] = serde_json::json!(1000);
    let edit_path = directory.root.join("one-second-plan.json");
    fs::write(&edit_path, serde_json::to_vec_pretty(&plan).unwrap()).unwrap();
    let edited = response(
        &directory.media_json(&[
            "edit",
            workspace_argument,
            "--plan",
            edit_path.to_str().unwrap(),
        ]),
        "edit",
        true,
    );
    let reviewed_plan = edited["result"]["task"]["plan"].clone();
    let original_approval = edited["result"]["task"]["approval"].clone();
    let original_export = edited["result"]["task"]["export"].clone();
    assert!(original_approval.is_null());
    assert!(original_export.is_null());
    assert_eq!(reviewed_plan["clips"][0]["end_ms"], 1000);
    assert!(reviewed_plan["revision"].as_u64().unwrap() > plan["revision"].as_u64().unwrap());

    let validated = response(
        &directory.media_json(&["validate", workspace_argument]),
        "validate",
        true,
    );
    assert_eq!(validated["result"]["plan_duration_ms"], 1000);
    assert_eq!(validated["result"]["sources_verified"], true);
    assert_eq!(validated["result"]["semantic_evaluation"], "not_evaluated");
    let preview = response(
        &directory.media_json(&["preview", workspace_argument]),
        "preview",
        true,
    );
    assert_eq!(preview["result"]["attempt"], 1);
    assert_eq!(preview["result"]["quality"]["objective_pass"], false);
    assert_eq!(preview["result"]["quality"]["needs_human_review"], true);
    let quality = response(
        &directory.media_json(&["quality", workspace_argument]),
        "quality",
        true,
    );
    assert_eq!(quality["result"]["objective_pass"], false);
    assert_eq!(quality["result"]["semantic_evaluation"], "not_evaluated");
    let latest = response(
        &directory.media_json(&["preview-latest", workspace_argument]),
        "preview-latest",
        true,
    );
    assert_eq!(
        latest["result"]["bundle_id"],
        preview["result"]["bundle_id"]
    );

    let mut candidate = reviewed_plan.clone();
    candidate["clips"][0]["end_ms"] = serde_json::json!(2000);
    let candidates_path = directory.root.join("explicit-candidates.json");
    fs::write(
        &candidates_path,
        serde_json::to_vec_pretty(&vec![candidate.clone()]).unwrap(),
    )
    .unwrap();
    let iteration = response(
        &directory.media_json(&[
            "iterate",
            workspace_argument,
            "--candidates",
            candidates_path.to_str().unwrap(),
            "--max-attempts",
            "2",
        ]),
        "iterate",
        true,
    );
    let attempts = iteration["result"]["attempts"].as_array().unwrap();
    assert_eq!(attempts.len(), 2);
    assert_eq!(attempts[0]["attempt"], 2);
    assert_eq!(attempts[0]["quality"]["objective_pass"], false);
    assert_eq!(attempts[1]["attempt"], 3);
    assert_eq!(attempts[1]["quality"]["objective_pass"], true);
    assert_eq!(
        attempts[1]["quality"]["semantic_evaluation"],
        "not_evaluated"
    );
    assert_eq!(attempts[1]["quality"]["matches_current_plan"], false);
    assert_eq!(iteration["result"]["task_plan_unchanged"], true);
    assert_eq!(iteration["result"]["needs_human_review"], true);
    let selected = Path::new(iteration["result"]["selected_plan"].as_str().unwrap());
    assert!(!selected.is_absolute());
    assert!(selected
        .components()
        .all(|part| matches!(part, Component::Normal(_))));
    let selected_path = fs::canonicalize(workspace.join(selected)).unwrap();
    assert!(selected_path.starts_with(fs::canonicalize(&workspace).unwrap()));
    let selected_plan: Value = serde_json::from_slice(&fs::read(selected_path).unwrap()).unwrap();
    assert_eq!(selected_plan, candidate);

    let trajectory = response(
        &directory.media_json(&["trajectory", workspace_argument]),
        "trajectory",
        true,
    );
    assert_eq!(trajectory["result"]["attempts_started"], 3);
    assert_eq!(
        trajectory["result"]["events"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|event| event["kind"] == "started")
            .count(),
        3
    );
    let shown = response(
        &directory.media_json(&["show", workspace_argument]),
        "show",
        true,
    );
    assert_eq!(shown["result"]["task"]["plan"], reviewed_plan);
    assert_eq!(shown["result"]["task"]["approval"], original_approval);
    assert_eq!(shown["result"]["task"]["export"], original_export);
    assert_eq!(shown["result"]["task"]["cloud_consent"], false);
    assert!(shown["result"]["task"]["model_calls"]
        .as_array()
        .unwrap()
        .is_empty());
    let refused_export = response(
        &directory.media_json(&["export", workspace_argument]),
        "export",
        false,
    );
    assert_eq!(refused_export["error"]["code"], "Invalid");
    assert!(fs::read_dir(workspace.join("exports"))
        .unwrap()
        .next()
        .is_none());
    assert_eq!(fs::read(&source).unwrap(), original_source);
}
