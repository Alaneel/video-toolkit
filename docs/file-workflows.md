# PDF document subtitle and download workflows

`file_workflows` adds typed jobs beside the existing media conversion core. The desktop tools, CLI and MCP adapters use the same request, cancellation, temporary staging and verified publication path. It does not replace the existing Video Agent or editing commands.

A workflow request consists of `kind`, ordered local `inputs`, `output_dir` and an `options` object. A successful result lists the actual output files. Existing output files are refused. All outputs are checked before publication; a failure or cancellation removes unpublished staging files. Conversion does not modify source files.

```json
{
  "kind": "pdf_split",
  "inputs": ["/absolute/path/report.pdf"],
  "output_dir": "/absolute/path/parts",
  "options": {"ranges": ["1-3", "4-6"]}
}
```

The command-line adapter accepts the same fields:

```sh
video-toolkit workflows --json
video-toolkit workflow pdf-merge first.pdf second.pdf --output-dir merged --json
video-toolkit workflow pdf-split report.pdf --output-dir parts --options '{"ranges":["1-3","4-6"]}' --json
video-toolkit workflow document-convert notes.md --output-dir documents --options '{"format":"pdf"}' --json
```

PDF and document workflows require a Python environment containing `pypdf`, `reportlab`, `Pillow`, `pypdfium2`, `python-docx`, `markdown` and `beautifulsoup4`. Document conversion also needs Pandoc. PDF compression requires Ghostscript (`gs`). HEIC/HEIF images need ImageMagick with its HEIC decoder. PDF output from documents uses LibreOffice against an intermediate generated DOCX. Dependency discovery is not proof that every backend or codec works.

Use `VIDEO_TOOLKIT_PYTHON` to select a Python environment and `VIDEO_TOOLKIT_PANDOC` to select Pandoc if needed. The document worker also recognizes the executable shipped by `pypandoc_binary` in that Python environment. `VIDEO_TOOLKIT_SOFFICE` or `VIDEO_TOOLKIT_BUNDLED_SOFFICE` explicitly selects LibreOffice; the default probes the Codex bundled runtime under the current user's home. No hard-coded developer home directory is required. The application does not install these tools during a conversion. Pandoc runs with `--sandbox`, so it must embed its data files: the `pypandoc_binary` and upstream release builds do, while some distribution packages (for example Ubuntu's `pandoc`) read templates from disk and fail in sandbox mode.

Backend commands share the converter's verified Unix process ownership and interruptible output readers. A valid-looking manifest prefix is not enough: read errors, missing EOF or stdout beyond 4 MiB fail the job. Pipe draining and helper joins have bounded grace periods, including cancellation and monitoring failures. Unix stdin delivery is interruptible under backpressure; on Windows a blocked native stdin writer may outlive a failed job until the inherited pipe closes or the process exits. Windows stops the direct child, not an entire descendant tree; Unix descendants that leave the private process group are also outside group cleanup. These limits are not an OS sandbox or a promise of zero residual helper threads on every platform.

## PDF operations

Page selection uses **1-based** inclusive ranges such as `"1,3-5"`. Omitted `pages`, `""` and `"all"` select all pages. Ordering is retained and duplicate pages within a selection are deduplicated. Invalid, descending and out-of-range selections fail. PDFs must have 1–5,000 pages; encrypted PDFs require an unencrypted source copy.

| Kind | Inputs | Options | Outputs |
| --- | --- | --- | --- |
| `pdf_merge` | One or more PDFs, in merge order | None | `merged.pdf`; checked against the combined page count |
| `pdf_split` | One PDF | `ranges`: nonempty array of selections, e.g. `["1-2","3"]`; otherwise `chunk_size`: integer 1–5,000, default 1; optional `pages` applies to chunk mode | `report-part-0001.pdf`, `report-part-0002.pdf`, …; each part checked against the selected count |
| `pdf_rotate` | One PDF | `pages`; `angle`: 90, 180 or 270, default 90 | `report-rotate.pdf`, retaining all pages; only selected pages rotate |
| `pdf_compress` | One PDF | `preset`: `screen`, `ebook` (default), `printer` or `prepress` | `report-compressed.pdf`; Ghostscript output must retain the page count; if it is larger, the source bytes are used instead |
| `pdf_watermark` | One PDF | `pages`; required `text` (1–500 characters); `opacity`: 0–1, default 0.2; `font_size`: 6–144, default 36; `angle`: -360–360, default 45 | `report-watermark.pdf`, centered text overlay on selected pages |
| `pdf_metadata` | One PDF | `title`, `author`, `subject`, `keywords`: strings up to 10,000 characters; `strip`: boolean, default false | `report-metadata.pdf`; `strip` removes document information and XMP before applying supplied fields |
| `pdf_to_images` | One PDF | `pages`; `format`: `png` (default) or `jpg`; `dpi`: 36–300, default 150 | One `report-page-0001.png`/`.jpg` per selected original page number |
| `pdf_to_text` | One PDF | `pages` | UTF-8 `report-extracted.txt`, with page separators; a document without extractable text fails |
| `images_to_pdf` | Ordered PNG/JPEG/WebP/BMP/TIFF/HEIC/HEIF images | None | `images.pdf`, one image per page in input order; transparency is composited onto white |

Single-input PDF output names include the source filename stem (`report.pdf` in the table), so separate jobs for different sources can share one output directory. Unsafe or long stems are normalized with a short hash suffix; sources with the same stem still require separate destinations or a renamed source. Existing outputs are always refused. Merge and images-to-PDF retain group-level names.

PDF rendering is limited to 500 selected pages per request and 100 megapixels per rendered page. Images-to-PDF uses each image's first frame, applies EXIF orientation and sizes the page from the image dimensions. Large page dimensions are scaled to fit PDF's page-size limit. HEIC/HEIF decoding uses an explicit ImageMagick coder and staged safe filenames.

Compression can change image quality, fonts, forms and other document features. These operations do not preserve digital signatures. Text extraction follows the PDF's text structure; it is not OCR and does not promise the visual reading order of columns. A watermark is an overlay, not redaction. Metadata removal is not a promise to remove personal information from page content, attachments or every embedded object.

## Document conversion

`document_convert` accepts TXT, Markdown (`.md`, `.markdown`), HTML (`.html`, `.htm`), DOCX, RTF, ODT, EPUB and searchable PDF. Inputs are processed in order, one output per input. The input size limit is 64 MiB per document.

| Option | Values | Default |
| --- | --- | --- |
| `format` | `txt`, `md`, `html`, `docx`, `pdf`, `epub` | `pdf` |
| `encoding` | A Python text codec name, for TXT/MD/HTML inputs | `utf-8` |
| `title` | Output document title | Source filename stem |

Outputs use a stable numbered filename such as `001-notes.docx`. Text is UTF-8. DOCX and EPUB containers are reopened and checked; PDF outputs must reopen with at least one page; text/HTML outputs must contain readable content. Empty documents, malformed inputs and missing tools report errors.

Conversions preserve a semantic document structure where the readers and writers support it: headings, paragraphs, emphasis, lists, simple tables, links and supported raster images. They **rebuild layout**. Original margins, fonts, pagination, floating objects, annotations, tracked revisions, complex tables, equations and exact placement are not guaranteed. TXT drops formatting. PDF input is extracted text and reflowed into a new document; it does not preserve the original PDF's page layout. Image-only PDFs fail because OCR is not included.

HTML is sanitized before export: scripts, frames, embedded active objects, external styles and event handlers are removed. Inline vector graphics and MathML require prior rasterization. Supported raster images are decoded and re-encoded locally; DOCX/ODT/EPUB package images and local images beneath the source document's folder can be included. Remote images, absolute resource URLs, paths escaping the source folder and ambiguous package image paths fail instead of being fetched or silently omitted. Documents containing unsupported linked resources need those resources embedded first. Individual document images are limited to 20 MiB/40 megapixels, with a 64 MiB total inline resource budget. Only the first frame of an animated image is retained.

Pandoc runs with `--sandbox`, without user-provided filters, readers or PDF engines. PDF creation passes a generated DOCX to an explicitly selected LibreOffice executable with a private temporary profile. This is a bounded application workflow, not an OS sandbox or a malicious-file security guarantee.

## Subtitle and URL operations

The registry also provides `subtitle_convert`, `subtitle_shift`, `subtitle_fps`, `subtitle_clean` and `download`. Subtitle tools accept eight timed formats (SRT/VTT/ASS/SSA/SBV/MicroDVD SUB/SMI/LRC), with TXT/CSV/JSON exports. They preserve a text-and-timing model rather than full styling. See [Subtitle workflows](subtitle-workflows.md) for exact options, frame-rate semantics, LRC inference and explicitly rejected inputs.

The forms expose the same typed `options` used by CLI/MCP. URL retrieval has a separate adapter so local document conversion does not call a network service. See [URL downloads](url-downloads.md) for modes, options and validation behavior.

## Verification

The following real-engine tests are explicitly opt-in. Selecting them fails if a required dependency is absent; tests never silently report success without a tool.

```sh
cargo test --locked -p file_workflows --test pdf_documents -- --ignored --nocapture
```

The PDF test checks page counts and selected-page edits, watermark/metadata content, compression size behavior, text extraction, actual PNG/JPEG decoding, transparent image pages, HEIC decoding and unchanged sources. The document test checks all eight input families through all six output formats, content/table structure, embedded DOCX images, remote-resource rejection and unchanged sources. The public `execute` integration test additionally verifies the shared PDF/document publication path, existing-output refusal, pre-cancellation, two-source rotate/render batches sharing a destination and absence of staging residue. Visual QA of representative PDF/DOCX output complements these structural checks; it does not establish fidelity for every user document.
