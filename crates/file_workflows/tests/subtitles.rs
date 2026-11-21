use encoding_rs::GB18030;
use file_workflows::{execute, CancelToken, WorkflowKind, WorkflowRequest};
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};

const SRT: &str = "1\n00:00:01,000 --> 00:00:02,000\nHello, \"字幕\" & <literal>\nSecond line\n\n2\n00:00:04,000 --> 00:00:06,000\nGoodbye\n\n";

fn input(directory: &Path, name: &str, content: impl AsRef<[u8]>) -> PathBuf {
    let path = directory.join(name);
    fs::write(&path, content).unwrap();
    path
}
fn convert(
    source: &Path,
    destination: &Path,
    kind: WorkflowKind,
    options: Value,
) -> anyhow::Result<Vec<PathBuf>> {
    let before = fs::read(source)?;
    let result = execute(
        &WorkflowRequest {
            kind,
            inputs: vec![source.to_path_buf()],
            output_dir: destination.to_path_buf(),
            options,
        },
        &CancelToken::default(),
        |_| {},
    );
    assert_eq!(
        fs::read(source).unwrap(),
        before,
        "source subtitle must never change"
    );
    result.map(|result| result.outputs)
}
fn json_cues(source: &Path, destination: &Path, mut options: Value) -> Value {
    options["format"] = json!("json");
    let outputs = convert(source, destination, WorkflowKind::SubtitleConvert, options).unwrap();
    serde_json::from_slice::<Value>(&fs::read(&outputs[0]).unwrap()).unwrap()["cues"].clone()
}

#[test]
fn every_full_timeline_format_roundtrips_multiline_and_unicode() {
    let directory = tempfile::tempdir().unwrap();
    let source = input(directory.path(), "captions.srt", SRT);
    let expected = json_cues(&source, &directory.path().join("baseline"), json!({}));
    for format in ["srt", "vtt", "ass", "ssa", "sbv", "sub", "smi"] {
        let outputs = convert(
            &source,
            &directory.path().join(format),
            WorkflowKind::SubtitleConvert,
            json!({"format":format,"target_fps":25.0}),
        )
        .unwrap_or_else(|error| panic!("{format}: {error:#}"));
        let actual = json_cues(
            &outputs[0],
            &directory.path().join(format!("back-{format}")),
            json!({}),
        );
        assert_eq!(actual, expected, "{format}");
    }
}

#[test]
fn lrc_multiple_timestamps_infer_ends_and_explicitly_reject_unsupported_variants() {
    let directory = tempfile::tempdir().unwrap();
    let source = input(
        directory.path(),
        "lyrics.lrc",
        "[ar:Artist]\n[offset:0]\n[00:01.000][00:04.000]Hello\n",
    );
    let cues = json_cues(&source, &directory.path().join("json"), json!({}));
    assert_eq!(cues[0]["start_ms"], 1000);
    assert_eq!(cues[0]["end_ms"], 4000);
    assert_eq!(cues[1]["end_ms"], 6000);
    for (name, text) in [
        ("offset", "[offset:100]\n[00:01]Hello"),
        ("enhanced", "[00:01]<00:01.5>Hello"),
    ] {
        let source = input(directory.path(), &format!("{name}.lrc"), text);
        assert!(convert(
            &source,
            &directory.path().join(name),
            WorkflowKind::SubtitleConvert,
            json!({"format":"srt"})
        )
        .is_err());
    }
    let simple = input(
        directory.path(),
        "simple.srt",
        "1\n00:00:01,000 --> 00:00:02,000\nOne line\n",
    );
    let outputs = convert(
        &simple,
        &directory.path().join("lrc-out"),
        WorkflowKind::SubtitleConvert,
        json!({"format":"lrc"}),
    )
    .unwrap();
    assert_eq!(
        fs::read_to_string(&outputs[0]).unwrap(),
        "[00:01.000]One line\n"
    );
    let multiline = input(directory.path(), "multi.srt", SRT);
    assert!(convert(
        &multiline,
        &directory.path().join("lrc-multiline"),
        WorkflowKind::SubtitleConvert,
        json!({"format":"lrc"})
    )
    .is_err());
}

#[test]
fn webvtt_identifiers_settings_and_notes_are_handled() {
    let directory = tempfile::tempdir().unwrap();
    let source = input(directory.path(), "captions.vtt", "WEBVTT\n\nNOTE ignored note\nnotes\n\nSTYLE\n::cue {color:red;}\n\ncue-one\n00:01.000 --> 00:02.000 align:start position:10%\nFirst\nSecond\n\n");
    let cues = json_cues(&source, &directory.path().join("out"), json!({}));
    assert_eq!(
        cues[0],
        json!({"start_ms":1000,"end_ms":2000,"text":"First\nSecond"})
    );
}

#[test]
fn ass_field_order_tags_commas_and_unsorted_events_are_normalized() {
    let directory = tempfile::tempdir().unwrap();
    let source = input(directory.path(), "captions.ass", "[Script Info]\nScriptType: v4.00+\n[Events]\nFormat: End, Start, Text\nDialogue: 0:00:06.00,0:00:04.00,Later\nDialogue: 0:00:02.00,0:00:01.00,{\\i1}Hello, world{\\i0}\\NSecond\\hline\n");
    let cues = json_cues(&source, &directory.path().join("out"), json!({}));
    assert_eq!(cues[0]["text"], "Hello, world\nSecond\u{a0}line");
    assert_eq!(cues[0]["start_ms"], 1000);
}

#[test]
fn shift_clamps_endpoints_and_rejects_collapsed_intervals() {
    let directory = tempfile::tempdir().unwrap();
    let source = input(directory.path(), "captions.srt", SRT);
    let outputs = convert(
        &source,
        &directory.path().join("shifted"),
        WorkflowKind::SubtitleShift,
        json!({"format":"json","offset_ms":-1500}),
    )
    .unwrap();
    let cues: Value = serde_json::from_slice(&fs::read(&outputs[0]).unwrap()).unwrap();
    assert_eq!(cues["cues"][0]["start_ms"], 0);
    assert_eq!(cues["cues"][0]["end_ms"], 500);
    let rejected = directory.path().join("rejected");
    assert!(convert(
        &source,
        &rejected,
        WorkflowKind::SubtitleShift,
        json!({"offset_ms":-3000})
    )
    .is_err());
    assert_eq!(fs::read_dir(rejected).unwrap().count(), 0);
    let positive = convert(
        &source,
        &directory.path().join("positive"),
        WorkflowKind::SubtitleShift,
        json!({"format":"json","offset_ms":250}),
    )
    .unwrap();
    let value: Value = serde_json::from_slice(&fs::read(&positive[0]).unwrap()).unwrap();
    assert_eq!(value["cues"][0]["start_ms"], 1250);
}

#[test]
fn fps_retimes_by_source_over_target_and_microdvd_headers_are_checked() {
    let directory = tempfile::tempdir().unwrap();
    let source = input(directory.path(), "captions.srt", SRT);
    let outputs = convert(
        &source,
        &directory.path().join("fps"),
        WorkflowKind::SubtitleFps,
        json!({"format":"json","source_fps":25,"target_fps":50}),
    )
    .unwrap();
    let value: Value = serde_json::from_slice(&fs::read(&outputs[0]).unwrap()).unwrap();
    assert_eq!(value["cues"][0]["start_ms"], 500);
    assert_eq!(value["cues"][0]["end_ms"], 1000);
    let source = input(
        directory.path(),
        "frames.sub",
        "{1}{1}25\n{25}{50}Hello|World\n",
    );
    assert!(convert(
        &source,
        &directory.path().join("conflict"),
        WorkflowKind::SubtitleConvert,
        json!({"format":"srt","source_fps":30})
    )
    .is_err());
    let missing = input(directory.path(), "missing.sub", "{25}{50}Hello");
    assert!(convert(
        &missing,
        &directory.path().join("missingfps"),
        WorkflowKind::SubtitleConvert,
        json!({"format":"srt"})
    )
    .is_err());
}

#[test]
fn cleaning_removes_empty_and_duplicate_cues_and_optional_markup() {
    let directory = tempfile::tempdir().unwrap();
    let source = input(directory.path(), "dirty.srt", "1\n00:00:00,000 --> 00:00:00,500\n\n2\n00:00:01,000 --> 00:00:02,000\n<i>Hello</i>   &amp; world\n\n3\n00:00:01,000 --> 00:00:02,000\n<i>Hello</i>   &amp; world\n\n");
    let outputs = convert(
        &source,
        &directory.path().join("cleaned"),
        WorkflowKind::SubtitleClean,
        json!({"format":"json","clean":{"strip_markup":true}}),
    )
    .unwrap();
    let value: Value = serde_json::from_slice(&fs::read(&outputs[0]).unwrap()).unwrap();
    assert_eq!(value["cues"].as_array().unwrap().len(), 1);
    assert_eq!(value["cues"][0]["text"], "Hello & world");
}

#[test]
fn utf8_bom_utf16_both_endiannesses_and_gb18030_decode_without_replacement() {
    let directory = tempfile::tempdir().unwrap();
    let text = "1\r\n00:00:01,000 --> 00:00:02,000\r\n中文测试\r\n";
    let mut le = vec![0xff, 0xfe];
    let mut be = vec![0xfe, 0xff];
    for word in text.encode_utf16() {
        le.extend(word.to_le_bytes());
        be.extend(word.to_be_bytes());
    }
    let mut utf8 = vec![0xef, 0xbb, 0xbf];
    utf8.extend(text.as_bytes());
    let (gb, _, errors) = GB18030.encode(text);
    assert!(!errors);
    for (name, bytes, options) in [
        ("le", le, json!({})),
        ("be", be, json!({})),
        ("utf8", utf8, json!({})),
        ("gb", gb.into_owned(), json!({"encoding":"gb18030"})),
    ] {
        let source = input(directory.path(), &format!("{name}.srt"), bytes);
        let cues = json_cues(
            &source,
            &directory.path().join(format!("out-{name}")),
            options,
        );
        assert_eq!(cues[0]["text"], "中文测试");
    }
    let invalid = input(directory.path(), "invalid.srt", vec![0xff, 0xff, 0x20]);
    assert!(convert(
        &invalid,
        &directory.path().join("invalid"),
        WorkflowKind::SubtitleConvert,
        json!({"format":"srt"})
    )
    .is_err());
    let mismatch = input(directory.path(), "mismatch.srt", [0xff, 0xfe, 0x31, 0]);
    assert!(convert(
        &mismatch,
        &directory.path().join("mismatch"),
        WorkflowKind::SubtitleConvert,
        json!({"format":"srt","encoding":"utf-8"})
    )
    .is_err());
}

#[test]
fn malformed_and_ambiguous_input_is_rejected_without_publishing() {
    let directory = tempfile::tempdir().unwrap();
    for (name, text) in [
        ("reverse.srt", "1\n00:00:02,000 --> 00:00:01,000\nWrong\n"),
        ("minutes.srt", "1\n00:99:00,000 --> 01:40:00,000\nWrong\n"),
        ("overflow.srt", "1\n9999999999999999999:00:00,000 --> 9999999999999999999:00:01,000\nWrong\n"),
        ("unsorted.srt", "1\n00:00:04,000 --> 00:00:05,000\nLater\n\n2\n00:00:01,000 --> 00:00:02,000\nEarlier\n"),
        ("binary.sub", "\0\0binary VobSub"),
        ("header.vtt", "00:01.000 --> 00:02.000\nMissing header"),
        ("columns.ass", "[Events]\nDialogue: 0,0:00:01.00,0:00:02.00,Hello"),
        ("export.txt", "No timing"),
    ] {
        let source = input(directory.path(), name, text);
        let destination = directory.path().join(format!("out-{name}"));
        assert!(convert(&source, &destination, WorkflowKind::SubtitleConvert, json!({"format":"srt"})).is_err(), "{name}");
        assert_eq!(fs::read_dir(&destination).unwrap().count(), 0, "{name}");
    }
}

#[test]
fn unknown_options_and_invalid_fps_are_rejected() {
    let directory = tempfile::tempdir().unwrap();
    let source = input(directory.path(), "captions.srt", SRT);
    for (index, options) in [
        json!({"format":"srt","shell":"anything"}),
        json!({"format":"srt","clean":{"typo":true}}),
        json!({"format":"srt","source_fps":0}),
        json!({"format":"srt","encoding":"not-an-encoding"}),
    ]
    .into_iter()
    .enumerate()
    {
        assert!(convert(
            &source,
            &directory.path().join(format!("bad-{index}")),
            WorkflowKind::SubtitleConvert,
            options
        )
        .is_err());
    }
    assert!(convert(
        &source,
        &directory.path().join("missing-fps"),
        WorkflowKind::SubtitleFps,
        json!({"source_fps":25})
    )
    .is_err());
}

#[test]
fn json_csv_and_text_exports_preserve_escaping_and_multiline() {
    let directory = tempfile::tempdir().unwrap();
    let source = input(directory.path(), "captions.srt", SRT);
    for format in ["json", "csv", "txt"] {
        let outputs = convert(
            &source,
            &directory.path().join(format),
            WorkflowKind::SubtitleConvert,
            json!({"format":format}),
        )
        .unwrap();
        let body = fs::read_to_string(&outputs[0]).unwrap();
        if format == "json" {
            let value: Value = serde_json::from_str(&body).unwrap();
            assert_eq!(
                value["cues"][0]["text"],
                "Hello, \"字幕\" & <literal>\nSecond line"
            );
        } else if format == "csv" {
            assert!(body.contains("\"Hello, \"\"字幕\"\" & <literal>\nSecond line\""));
        } else {
            assert!(body.contains("Hello, \"字幕\" & <literal>\nSecond line"));
        }
    }
}

#[test]
fn cancelled_workflows_and_existing_outputs_leave_inputs_and_outputs_intact() {
    let directory = tempfile::tempdir().unwrap();
    let source = input(directory.path(), "captions.srt", SRT);
    let destination = directory.path().join("out");
    fs::create_dir(&destination).unwrap();
    let request = WorkflowRequest {
        kind: WorkflowKind::SubtitleConvert,
        inputs: vec![source.clone()],
        output_dir: destination.clone(),
        options: json!({"format":"vtt"}),
    };
    let cancel = CancelToken::default();
    cancel.cancel();
    assert!(execute(&request, &cancel, |_| {}).is_err());
    assert_eq!(fs::read_dir(&destination).unwrap().count(), 0);
    let cancel = CancelToken::default();
    assert!(execute(&request, &cancel, |_| cancel.cancel()).is_err());
    assert_eq!(fs::read_dir(&destination).unwrap().count(), 0);
    let existing = destination.join("captions.converted.vtt");
    fs::write(&existing, b"user output").unwrap();
    assert!(execute(&request, &CancelToken::default(), |_| {}).is_err());
    assert_eq!(fs::read(existing).unwrap(), b"user output");
    assert_eq!(fs::read_to_string(source).unwrap(), SRT);
}

#[test]
fn unrepresentable_precision_overlap_and_control_syntax_are_explicit_errors() {
    let directory = tempfile::tempdir().unwrap();
    for (index, (text, options)) in [
        (
            "1\n00:00:01,001 --> 00:00:01,002\nTiny\n",
            json!({"format":"ass"}),
        ),
        (
            "1\n00:00:01,001 --> 00:00:01,002\nTiny\n",
            json!({"format":"sub","target_fps":25}),
        ),
        (
            "1\n00:00:01,000 --> 00:00:03,000\nA\n\n2\n00:00:02,000 --> 00:00:04,000\nB\n",
            json!({"format":"smi"}),
        ),
        (
            "1\n00:00:01,000 --> 00:00:02,000\nLiteral|pipe\n",
            json!({"format":"sub","target_fps":25}),
        ),
        (
            "1\n00:00:01,000 --> 00:00:02,000\nLiteral\\Nsequence\n",
            json!({"format":"ass"}),
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let source = input(directory.path(), &format!("case-{index}.srt"), text);
        assert!(convert(
            &source,
            &directory.path().join(format!("case-{index}")),
            WorkflowKind::SubtitleConvert,
            options
        )
        .is_err());
    }
}

#[test]
#[ignore = "requires installed ffprobe for independent subtitle packet verification"]
fn ffprobe_independently_accepts_generated_timed_subtitles() {
    let probe = file_conversion::dependencies()
        .into_iter()
        .find(|tool| tool.name == "ffprobe")
        .unwrap()
        .path
        .expect("Install ffprobe to run independent subtitle verification");
    let directory = tempfile::tempdir().unwrap();
    let source = input(
        directory.path(),
        "captions.srt",
        "1\n00:00:01,000 --> 00:00:02,000\nHello\n\n2\n00:00:04,000 --> 00:00:06,000\nGoodbye\n",
    );
    for (format, demuxer) in [
        ("srt", "srt"),
        ("vtt", "webvtt"),
        ("ass", "ass"),
        ("ssa", "ass"),
        ("sub", "microdvd"),
        ("smi", "sami"),
        ("lrc", "lrc"),
    ] {
        let outputs = convert(
            &source,
            &directory.path().join(format),
            WorkflowKind::SubtitleConvert,
            json!({"format":format,"target_fps":25}),
        )
        .unwrap();
        let result = std::process::Command::new(&probe)
            .args([
                "-v",
                "error",
                "-f",
                demuxer,
                "-show_packets",
                "-show_entries",
                "packet=pts_time,duration_time",
                "-of",
                "json",
            ])
            .arg(&outputs[0])
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{format}: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        let packets: Value = serde_json::from_slice(&result.stdout).unwrap();
        let packets = packets["packets"].as_array().unwrap();
        let expected = if format == "smi" {
            vec!["1.000000", "2.000000", "4.000000", "6.000000"]
        } else {
            vec!["1.000000", "4.000000"]
        };
        // SAMI intentionally emits explicit empty SYNC packets at cue ends.
        assert_eq!(
            packets
                .iter()
                .map(|packet| packet["pts_time"].as_str().unwrap())
                .collect::<Vec<_>>(),
            expected,
            "{format}"
        );
    }
}
