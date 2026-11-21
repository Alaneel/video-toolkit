"""Independent semantic document conversion. Never promises original pagination.

Pandoc runs sandboxed with no user filters/custom readers. PDF uses an explicitly
selected LibreOffice (bundled runtime by default) against a generated DOCX.
"""
import base64
import html
import io
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile
import zipfile
from urllib.parse import unquote, urlsplit

BUNDLED_SOFFICE = Path.home() / ".cache/codex-runtimes/codex-primary-runtime/dependencies/bin/override/soffice"


def tool(name):
    configured = os.environ.get("VIDEO_TOOLKIT_" + name.upper())
    if configured and Path(configured).is_file():
        return configured
    if name == "pandoc":
        try:
            import pypandoc
            candidate = pypandoc.get_pandoc_path()
            if Path(candidate).is_file():
                return candidate
        except (ImportError, OSError):
            pass
    found = shutil.which(name)
    if found:
        return found
    # Standalone app and source-checkout installations can expose project tools.
    for directory in [Path.cwd() / ".toolkit-tools", Path.cwd() / "tools", Path("/opt/homebrew/bin"), Path("/usr/local/bin")]:
        candidate = directory / name
        if candidate.is_file() and os.access(candidate, os.X_OK):
            return str(candidate)
    raise ValueError(f"Required dependency '{name}' is missing")


def command(arguments, *, timeout=180):
    result = subprocess.run(arguments, capture_output=True, timeout=timeout)
    if result.returncode:
        raise ValueError(f"{Path(arguments[0]).name} failed: " + result.stderr.decode("utf-8", "replace")[-4000:])
    return result.stdout


def image_data_url(reference, source):
    from PIL import Image
    if len(reference) > 32 * 1024 * 1024:
        raise ValueError("Embedded image exceeds the document resource limit")
    if reference.startswith("data:"):
        if not re.match(r"^data:image/(?:png|jpeg|gif|webp);base64,", reference, re.I):
            raise ValueError("Only embedded PNG/JPEG/GIF/WebP raster images are supported")
        data = base64.b64decode(reference.split(",", 1)[1], validate=True)
    else:
        parsed = urlsplit(reference)
        if parsed.scheme or parsed.netloc or not source:
            raise ValueError("Remote and absolute image resource loading is disabled; embed the image in the source document")
        resource = unquote(parsed.path).replace("\\", "/")
        if source.suffix.lower() in [".docx", ".odt", ".epub"]:
            with zipfile.ZipFile(source) as package:
                matches = [name for name in package.namelist() if name == resource or name.endswith("/" + resource)]
                if not matches:
                    matches = [name for name in package.namelist() if Path(name).name == Path(resource).name]
                if len(matches) != 1:
                    raise ValueError("Cannot resolve a unique embedded document image")
                member = package.getinfo(matches[0])
                if member.file_size > 20 * 1024 * 1024:
                    raise ValueError("Document image exceeds 20 MiB")
                data = package.read(member)
        else:
            candidate = (source.parent / resource).resolve(strict=True)
            if not candidate.is_relative_to(source.parent.resolve()) or not candidate.is_file():
                raise ValueError("Document images must be local files beside or beneath the source document")
            if candidate.stat().st_size > 20 * 1024 * 1024:
                raise ValueError("Document image exceeds 20 MiB")
            data = candidate.read_bytes()
    with Image.open(io.BytesIO(data)) as image:
        if image.width * image.height > 40_000_000:
            raise ValueError("Document image exceeds 40 megapixels")
        if image.format not in ["PNG", "JPEG", "GIF", "WEBP", "BMP", "TIFF"]:
            raise ValueError("Unsupported document image coder")
        buffer = io.BytesIO(); image.convert("RGBA").save(buffer, format="PNG")
    return "data:image/png;base64," + base64.b64encode(buffer.getvalue()).decode("ascii")


def sanitize_html(text, source=None):
    """Preserve semantic HTML and local raster images, without fetching URLs."""
    from bs4 import BeautifulSoup
    soup = BeautifulSoup(text, "html.parser")
    for header in soup.find_all("header", id="title-block-header"):
        header.decompose()
    for tag in soup.find_all(["script", "iframe", "object", "embed", "link", "style", "base", "form", "input", "video", "audio", "source"]):
        tag.decompose()
    for tag in soup.find_all("meta", attrs={"http-equiv": True}):
        tag.decompose()
    for tag in soup.find_all(["svg", "math"]):
        raise ValueError("Inline vector graphics and MathML are not supported in semantic conversion; rasterize them first")
    resource_bytes = 0
    seen_ids = set()
    for tag in soup.find_all(True):
        if tag.has_attr("id"):
            identifier = str(tag["id"])
            if identifier in seen_ids:
                del tag["id"]
            else:
                seen_ids.add(identifier)
        for attribute in list(tag.attrs):
            if attribute.lower().startswith("on") or attribute.lower() in ["style", "srcset", "action", "formaction", "background", "srcdoc", "poster", "data"]:
                del tag[attribute]
        if tag.name == "img":
            tag["src"] = image_data_url(str(tag.get("src", "")), source)
            resource_bytes += len(tag["src"])
            if resource_bytes > 64 * 1024 * 1024:
                raise ValueError("Document images exceed the 64 MiB resource limit")
        if tag.has_attr("href") and str(tag["href"]).strip().lower().startswith(("javascript:", "file:", "data:", "vbscript:")):
            del tag["href"]
    return str(soup)


def validate_zip(path, marker):
    with zipfile.ZipFile(path) as archive:
        if marker not in archive.namelist() or archive.testzip() is not None:
            raise ValueError("Document container validation failed")


def validate(path, format_name):
    if not path.is_file() or not path.stat().st_size:
        raise ValueError("Conversion produced an empty document")
    if format_name == "pdf":
        from pypdf import PdfReader
        document = PdfReader(str(path))
        if document.is_encrypted or not document.pages:
            raise ValueError("Output PDF is empty or encrypted")
    elif format_name == "docx":
        validate_zip(path, "word/document.xml")
        from docx import Document
        Document(str(path))
    elif format_name == "epub":
        validate_zip(path, "META-INF/container.xml")
        with zipfile.ZipFile(path) as archive:
            if archive.read("mimetype") != b"application/epub+zip":
                raise ValueError("Output EPUB has an invalid media type")
    else:
        text = path.read_text(encoding="utf-8")
        if not text.strip():
            raise ValueError("Conversion contains no readable content")
        if format_name == "html":
            from bs4 import BeautifulSoup
            parsed = BeautifulSoup(text, "html.parser")
            if not parsed.get_text(strip=True) and not parsed.find("img"):
                raise ValueError("Output HTML contains no readable content")


def convert_one(source, output, format_name, options, work):
    extension = source.suffix.lower().lstrip(".")
    if extension not in ["txt", "md", "markdown", "html", "htm", "docx", "rtf", "odt", "epub", "pdf"]:
        raise ValueError(f"Unsupported document input: .{extension}")
    if source.stat().st_size > 64 * 1024 * 1024:
        raise ValueError("Document input exceeds 64 MiB")
    pandoc = tool("pandoc")
    source_format = {"txt": "markdown", "md": "gfm", "markdown": "gfm", "htm": "html"}.get(extension, extension)
    safe_source = source
    if extension in ["txt", "md", "markdown", "html", "htm"]:
        encoding = options.get("encoding", "utf-8")
        if not isinstance(encoding, str):
            raise ValueError("encoding must be a codec name")
        text = source.read_text(encoding=encoding)
        if extension == "txt":
            text = "<!doctype html><html><body><pre>" + html.escape(text) + "</pre></body></html>"
            source_format = "html"
        elif extension in ["md", "markdown"]:
            # Python Markdown is local and does not dereference image/link URLs.
            import markdown
            text = markdown.markdown(text, extensions=["tables", "fenced_code", "sane_lists"])
            source_format = "html"
        text = sanitize_html(text, source)
        safe_source = work / "source.html"; safe_source.write_text(text, encoding="utf-8")
    elif extension == "pdf":
        from pypdf import PdfReader
        pdf = PdfReader(str(source))
        if pdf.is_encrypted:
            raise ValueError("Encrypted PDF input is not supported")
        if len(pdf.pages) > 5000:
            raise ValueError("PDF input exceeds 5000 pages")
        text = "\n\n".join(page.extract_text() or "" for page in pdf.pages)
        if not text.strip():
            raise ValueError("PDF has no extractable text; OCR is not included")
        safe_source = work / "source.html"
        safe_source.write_text("<html><body><pre>" + html.escape(text) + "</pre></body></html>", encoding="utf-8")
        source_format = "html"
    # For binary document inputs the sandboxed reader returns semantic HTML.
    # Sanitization prevents active/raw HTML reaching an HTML viewer or LO.
    canonical = work / "canonical.html"
    raw = command([pandoc, "--sandbox", "--from=" + source_format, "--to=html5", "--standalone", "--metadata=title:" + str(options.get("title", source.stem)), str(safe_source)])
    canonical.write_text(sanitize_html(raw.decode("utf-8"), source), encoding="utf-8")
    target = {"txt": "plain", "md": "gfm", "html": "html5", "epub": "epub3", "docx": "docx", "pdf": "docx"}[format_name]
    intermediate = work / "print.docx" if format_name == "pdf" else output
    args = [pandoc, "--sandbox", "--fail-if-warnings", "--from=html", "--to=" + target, "--standalone", "--metadata=title:" + str(options.get("title", source.stem)), str(canonical), "--output=" + str(intermediate)]
    command(args)
    if format_name == "pdf":
        configured = os.environ.get("VIDEO_TOOLKIT_SOFFICE") or os.environ.get("VIDEO_TOOLKIT_BUNDLED_SOFFICE")
        soffice = Path(configured) if configured else BUNDLED_SOFFICE
        if not soffice.is_file():
            raise ValueError("LibreOffice is unavailable. Configure VIDEO_TOOLKIT_SOFFICE to an explicit LibreOffice executable")
        profile = (work / "lo-profile").resolve().as_uri()
        command([str(soffice), "-env:UserInstallation=" + profile, "--headless", "--convert-to", "pdf", "--outdir", str(work), str(intermediate)], timeout=180)
        rendered = work / "print.pdf"
        if not rendered.is_file():
            raise ValueError("LibreOffice did not produce a PDF")
        shutil.move(str(rendered), output)
    validate(output, format_name)


def run(request):
    if request.get("kind") != "document_convert":
        raise ValueError("Unsupported document workflow")
    options = request.get("options") or {}
    if not isinstance(options, dict):
        raise ValueError("options must be an object")
    format_name = options.get("format", "pdf")
    if not isinstance(format_name, str) or format_name.lower() not in ["txt", "md", "html", "docx", "pdf", "epub"]:
        raise ValueError("Document target must be txt, md, html, docx, pdf or epub")
    format_name = format_name.lower()
    staging = Path(request["staging"]).resolve()
    if not staging.is_dir():
        raise ValueError("The staging directory must exist")
    inputs = [Path(path) for path in request.get("inputs", [])]
    if not inputs:
        raise ValueError("Choose at least one document")
    outputs = []
    for index, source in enumerate(inputs):
        safe_name = re.sub(r"[^\w .-]", "_", source.stem, flags=re.UNICODE).strip(". ")[:100] or "document"
        output = staging / f"{index + 1:03}-{safe_name}.{format_name}"
        with tempfile.TemporaryDirectory(prefix="document-work-", dir=staging) as directory:
            convert_one(source, output, format_name, options, Path(directory))
        outputs.append(output)
    return outputs


if __name__ == "__main__":
    try:
        with open(sys.argv[1], encoding="utf-8") if len(sys.argv) > 1 else sys.stdin as stream:
            request = json.load(stream)
        print(json.dumps({"outputs": [str(path.resolve()) for path in run(request)]}))
    except Exception as error:
        print(f"Document workflow: {error}", file=sys.stderr)
        sys.exit(1)
