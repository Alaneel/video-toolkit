# Subtitle workflows

The subtitle backend is a native Rust text workflow in the existing Video Toolkit
workspace. It requires no FFmpeg, Python, cloud service or account. The CLI,
desktop and MCP call the same validated cue engine. Inputs are read without
modification; the shared workflow executor stages and verifies new files before
publishing, refuses existing destinations, and cleans unpublished work on failure
or cooperative cancellation.

## Supported formats

| Format | Input | Output | Timing and text behavior |
| --- | --- | --- | --- |
| SRT | Yes | Yes | Millisecond timestamps and multiline text. |
| WebVTT | Yes | Yes | Cue identifiers/settings, NOTE, STYLE and REGION blocks are read, but layout/style metadata is omitted. |
| ASS / SSA | Yes | Yes | Reads Events Format and Dialogue fields, including reordered timing columns when Text is last. Text line breaks are retained; override styles, positioning and animation are omitted. Events are sorted by time. |
| SBV | Yes | Yes | Start/end timing and multiline text. |
| SUB / MicroDVD | Yes | Yes | Text frame-based SUB only. Needs an FPS header or source_fps to read; target_fps or source_fps to write. Binary VobSub and other SUB dialects are rejected. |
| SMI / SAMI | Yes | Yes | SYNC timing and text/HTML entities. Empty SYNC entries clear text. Output rejects overlaps rather than discarding them. |
| LRC | Yes | Yes | Repeated line timestamps are supported. End times use the next distinct timestamp; the final line gets two seconds. Output records starts only and rejects multiline cues. |
| TXT | No | Yes | Text only; timestamps are intentionally omitted. |
| CSV | No | Yes | start_ms, end_ms, text; quoted fields preserve commas, quotes and line breaks. |
| JSON | No | Yes | video-toolkit/subtitles/v1 schema with start_ms, end_ms and text. |

Conversion is for text captions, not preservation of a styled subtitle document.
ASS vector drawings, enhanced LRC word timestamps, and nonzero LRC offset metadata
are explicitly rejected. Normalize a nonzero LRC metadata offset before importing
it. ASS/SSA quantize to centiseconds and MicroDVD to frames; a cue that collapses
to zero duration after quantization is rejected. Literal text resembling reserved
ASS controls or MicroDVD pipe/style syntax is rejected for those destinations
when it cannot be represented faithfully. Use SRT, VTT or JSON for such text.

## Operations and options

Use workflow subtitle_convert, subtitle_shift, subtitle_fps or subtitle_clean.
Inspect their machine-readable descriptors with:

~~~sh
cargo run --locked -- workflows --json
~~~

Each operation accepts one JSON options object; unknown top-level and nested
clean options are rejected.

| Option | Meaning |
| --- | --- |
| format | srt, vtt, ass, ssa, sbv, sub, smi, lrc, txt, csv or json. Required for subtitle_convert; other operations default to the input format. |
| encoding | Input encoding. Default auto accepts UTF-8 and UTF-8/UTF-16 BOMs. Legacy text requires an explicit encoding_rs label, such as gb18030, gbk, big5, shift_jis or windows-1252. UTF-16 without a BOM requires an explicit endian label. All outputs are UTF-8. |
| offset_ms | Signed millisecond offset, required by subtitle_shift. Positive values delay captions. Each negative endpoint is clamped to zero; if this collapses a cue, the whole job fails without publishing outputs. |
| source_fps | Source frame rate, from 0.01 to 1000. Required for subtitle_fps and for MicroDVD input without an FPS header. A conflicting header is rejected. |
| target_fps | Destination frame rate, from 0.01 to 1000. Required for subtitle_fps; used to encode MicroDVD output, with source_fps as fallback. |
| clean | true, false, or a cleaning options object. subtitle_clean enables defaults unless explicitly disabled; other operations clean only when requested. |

The subtitle_fps operation multiplies cue times by source_fps / target_fps.
For example, 25 FPS to 50 FPS moves a cue starting at 1000 ms to 500 ms.
For ordinary format conversion, FPS values describe MicroDVD parsing/encoding
and do not retime other formats.

Cleaning options are remove_empty (default true), remove_duplicates (true),
normalize_whitespace (true), and strip_markup (false). Duplicate removal compares
start, end and resulting text. Whitespace normalization retains line breaks;
markup removal decodes common HTML/numeric entities. Overlapping cues remain
valid in formats that can represent them.

~~~sh
cargo run --locked -- workflow subtitle_convert captions.srt \
  --output-dir converted --options '{"format":"vtt"}' --json

cargo run --locked -- workflow subtitle_shift captions.srt \
  --output-dir shifted --options '{"offset_ms":-500,"format":"srt"}'

cargo run --locked -- workflow subtitle_fps captions.srt \
  --output-dir retimed --options '{"source_fps":25,"target_fps":23.976}'

cargo run --locked -- workflow subtitle_clean captions.srt \
  --output-dir cleaned --options '{"clean":{"strip_markup":true}}'
~~~

Multiple local inputs may be supplied to the CLI. Output names are
stem.converted.extension; colliding input stems are rejected before processing.
The JSON CLI response uses file-workflows/v1, with queue events on stderr.
A failed operation returns a nonzero status.

For MCP, call workflow_tools to discover schemas, then run_workflow:

~~~json
{
  "kind": "subtitle_convert",
  "inputs": ["/absolute/allowed/captions.srt"],
  "output_dir": "/absolute/allowed/converted",
  "options": {"format": "vtt", "encoding": "auto"}
}
~~~

The output folder must already exist inside a configured MCP root. The adapter
supports asynchronous progress and cancellation using the same conventions as
convert_files. Subtitle processing itself makes no network requests.

## Validation and verification

Input files are limited to 16 MiB, 100,000 cues and a 1000-hour timeline.
Timestamp components, chronological order and positive cue durations are checked.
ASS events and repeated LRC timestamps are normalized into chronological order;
other out-of-order timelines are rejected. Invalid byte sequences cause an error,
rather than replacement characters or a guessed legacy encoding.

~~~sh
cargo test --offline --locked -p file_workflows --test subtitles
cargo test --offline --locked --test file_conversion_cli workflow
# Optional independent packet/timestamp verification using installed ffprobe:
cargo test --offline --locked -p file_workflows --test subtitles -- --ignored
~~~

The subtitle fixture tests cover all eight timed formats, seven full-timeline
round trips, multiline Unicode/escaping, WebVTT settings, ASS field order, LRC
inference, MicroDVD headers, shift/FPS/clean, UTF-8 and both UTF-16 endiannesses,
GB18030, malformed input, strict options, cancellation and refusal to overwrite.
Binary-level CLI/MCP tests exercise actual subtitle conversion without external
media dependencies, including a cancelled 40,000-cue job.
The optional ffprobe test independently parses SRT, VTT, ASS, SSA, MicroDVD,
SAMI and LRC outputs and checks their cue start/clear packet timestamps.

