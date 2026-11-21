# Video Toolkit: shared file toolkit architecture

The existing Rust/egui desktop, clap CLI, editing crates, profile batches and Video Agent remain the foundation. Two shared engines now serve the desktop, CLI and MCP: `file_conversion` handles 13 video/audio/image output targets, while `file_workflows` provides a registry of 15 PDF, subtitle, document and download operations. These are different measures: an operation can produce several files or support several output formats.

```text
macOS egui desktop       clap convert/workflow CLI       MCP stdio tools
        \                            |                          /
         +-------------- typed requests and discovery --------+
         |                                                     |
         | file_conversion              file_workflows         |
         | 13 media/image targets       15 registered operations|
         | FFmpeg + ImageMagick 7       PDF / subtitles / docs  |
         |                              direct/media downloads |
         +------- queues, progress, errors, cancellation -------+
         +------- staged validation, no-clobber publication ----+
```

## Requests and backends

`ConversionRequest` carries input/output paths and a `TargetFormat`. `output_requests` plans batch names before work; `run_queue` reports each item and continues after processing failures. `convert` owns the codec invocation, stream inspection, full decode and publication. Existing `common::VideoFormat` defaults are reused for supported video targets. The adapters do not construct codec commands.

`WorkflowRequest` carries a `WorkflowKind`, ordered local `inputs`, an `output_dir` and typed registry options. `tools()` exposes titles, categories, input extensions, output formats and option schemas to the adapters. `execute` dispatches one operation and returns the actual output paths; `run_queue` reports per-job states and errors.

| Backend | Implemented operations | Execution and validation |
| --- | --- | --- |
| Media/image conversion | Four video, five audio and four image targets; video audio extraction; HEIC/HEIF image input | FFmpeg/ffprobe or ImageMagick 7; streams, dimensions, duration where known, and complete decode |
| PDF | Merge, split, rotate, compress, text watermark, metadata, render pages, extract text, images to PDF | Python PDF/image libraries, PDFium and Ghostscript; page counts, readable structure, selected-page behavior and decoded image outputs |
| Subtitles | Convert, shift timing, retime FPS, clean | Rust parsers/writers; eight timed formats plus TXT/CSV/JSON export; validated text-and-timing model |
| Documents | TXT/MD/HTML/DOCX/RTF/ODT/EPUB/searchable PDF to TXT/MD/HTML/DOCX/PDF/EPUB | Sanitized semantic HTML, Pandoc and generated DOCX to LibreOffice for PDF; readable content and reopened output containers |
| Downloads | Direct HTTP(S) files or media-link extraction/conversion | curl or yt-dlp plus FFmpeg; byte limits, completed transfer and nonempty output; downloaded media also receives full decode |

PDF text extraction is not OCR. Document conversion rebuilds semantic layout rather than preserving exact pagination. Subtitle conversion preserves text and timing rather than all styling. Direct arbitrary downloads are checked as transfers; their contents are not claimed to have been decoded. Exact options and limitations are documented in [file workflows](file-workflows.md), [subtitles](subtitle-workflows.md) and [downloads](url-downloads.md).

## Desktop, CLI and MCP

The desktop uses a light icon rail, searchable tools, conversion drop area, operation forms and separate queue panel. PDF, subtitle, document and download forms submit the same workflow requests as CLI/MCP. Local forms own an editable request until starting a background worker; the task gate prevents overlapping desktop operations. Queues remain sequential and in memory. Progress can represent backend stages when an engine cannot provide a meaningful exact percentage.

The CLI offers `convert`, `workflow`, `workflows --json` and dependency discovery, with human-readable status or structured JSON results. MCP performs initialization, discovery, allowed-folder checks, bounded message handling, progress and cancellation. It has one active worker; its lifetime guard cancels and joins on disconnect or read error. Discovery and local conversion do not retrieve URLs; only the download operation starts a network transfer.

## Dependency and output lifecycle

Media engines are discovered through executable overrides, PATH and platform installation locations. Optional PDF/document/download dependencies are installed into the project-private `.toolkit-venv` by `scripts/setup-file-workflows.sh`; conversions do not install dependencies. `VIDEO_TOOLKIT_PYTHON`, `VIDEO_TOOLKIT_PANDOC`, `VIDEO_TOOLKIT_SOFFICE` and other engine overrides can select an explicit runtime. The private Python environment's executable path is preserved so its installed modules remain available.

Workers use argument arrays rather than a shell. Media inputs use format/protocol whitelists; raster conversion stages safe filenames and restricts ImageMagick coders/delegates. Document resources are decoded locally and sanitized; remote images and resources escaping the source folder fail. Both engines share process ownership and interruptible output handling; incomplete captures fail, and pipe draining/helper joins are bounded. Cancellation/deadlines stop the direct child and, on Unix, verified members of its private process group before reaping the leader. Windows descendant trees and Unix descendants that leave that group are outside this cleanup guarantee. A native blocked workflow stdin writer on Windows may persist until its inherited pipe closes; this is not an OS-enforced process/thread quota.

Workflow backends write inside a private staging directory. Before publication, the shared layer rejects empty files, symlinks, duplicate names and outputs outside staging. Backend-specific structure checks run before this step. Single-source PDF operations include a safe source stem in output names, allowing different-source rotate/render/split jobs to share a destination. Final files are published without replacing existing destinations; failed multi-output publication removes the files published by that job. Cancellation and ordinary errors clean up staging. Source inputs are not modified. This is application-level validation, not an OS sandbox or a guarantee about every malformed input.

## Verification and remaining work

Real-tool tests cover the 13 media/image targets, PDF operations, all eight document input families through all six targets, subtitle workflows, loopback URL transfers and output publication. PDF/document tests also exercise source preservation, remote-resource rejection, no-overwrite and cancellation. Representative generated PDF and DOCX pages have been rendered and visually inspected; structural checks do not establish exact fidelity for arbitrary documents.

CI runs dependency-free workspace tests on Linux, macOS and Windows. Linux's real-engine run installs the optional Python runtime, Ghostscript, Pandoc and LibreOffice, and exercises document and loopback download tests. The PDF case containing ImageMagick HEIC decoding runs explicitly on macOS alongside the existing media/image `real_tools` checks; it is not silently omitted on Linux.

The next work is data-file conversion (JSON/YAML/XML/CSV/LOG), persistent queue/history and resume/retry, bounded general conversion concurrency, additional image formats and editing operations, richer previews and a natural-language planner that produces these typed requests. The existing Video Agent's persisted, reviewed editing workflow remains separate from a general file-conversion planner.

The [9xConvert UI study](9xconvert-ui-study.md) and [feature comparison](feature-comparison.md) distinguish static reference observations from this repository's implemented and tested behavior. The reference's 183 static tool registrations are not a claim of equivalent coverage here.
