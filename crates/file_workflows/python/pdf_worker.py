"""Local PDF operations. stdout is one JSON result; diagnostics use stderr."""
import io
import hashlib
import json
import math
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile


def number(options, key, default, minimum, maximum):
    value = options.get(key, default)
    if isinstance(value, bool) or not isinstance(value, (int, float)) or not math.isfinite(value) or not minimum <= value <= maximum:
        raise ValueError(f"{key} must be between {minimum} and {maximum}")
    return value


def pages(spec, count):
    """Return ordered zero-based page indices. Empty/all means all pages."""
    if spec is None or spec == "" or spec == "all":
        return list(range(count))
    if isinstance(spec, list):
        spec = ",".join(str(value) for value in spec)
    if not isinstance(spec, str):
        raise ValueError("pages must be a 1-based range such as 1,3-5")
    selected = []
    for part in spec.split(","):
        bounds = part.strip().split("-")
        if len(bounds) not in (1, 2) or not all(value.strip().isdigit() for value in bounds):
            raise ValueError(f"Invalid page range: {part}")
        first = int(bounds[0]); last = int(bounds[-1])
        if not 1 <= first <= last <= count:
            raise ValueError(f"Page range {part} is outside 1-{count}")
        for index in range(first - 1, last):
            if index not in selected:
                selected.append(index)
    if not selected:
        raise ValueError("Select at least one page")
    return selected


def reader(path):
    from pypdf import PdfReader
    result = PdfReader(str(path))
    if result.is_encrypted:
        raise ValueError("Encrypted PDFs are not supported; supply an unencrypted copy")
    if not 0 < len(result.pages) <= 5000:
        raise ValueError("PDF must contain 1 to 5000 pages")
    return result


def save(writer, output, expected):
    with output.open("wb") as stream:
        writer.write(stream)
    check = reader(output)
    if len(check.pages) != expected:
        raise ValueError("PDF output page count does not match the requested operation")
    return output


def tool(name):
    configured = os.environ.get("VIDEO_TOOLKIT_" + name.upper())
    if configured:
        if not Path(configured).is_file() or not os.access(configured, os.X_OK):
            raise ValueError(f"Configured {name} executable is not available")
        return configured
    path = shutil.which(name)
    if not path:
        for directory in ["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin"]:
            candidate = Path(directory) / name
            if candidate.is_file() and os.access(candidate, os.X_OK):
                path = str(candidate); break
    if not path:
        raise ValueError(f"Required dependency '{name}' is missing")
    return path


def output_prefix(source):
    """Keep batch outputs identifiable, portable and below filename limits."""
    original = source.stem
    normalized = re.sub(r"[^\w.-]+", "-", original).strip(" .-_") or "document"
    prefix = normalized
    while len(prefix.encode("utf-8")) > 160:
        prefix = prefix[:-1]
    if prefix != original:
        prefix += "-" + hashlib.sha256(original.encode("utf-8")).hexdigest()[:8]
    return prefix


def open_raster(source, staging):
    from PIL import Image
    if source.suffix.lower() not in [".heic", ".heif"]:
        return Image.open(source)
    # Decode HEIC/HEIF under an explicit coder and a safe staged filename.
    # External delegates and non-raster pseudo coders are not permitted.
    with tempfile.TemporaryDirectory(prefix="heic-decode-", dir=staging) as directory:
        temporary = Path(directory)
        staged = temporary / "source.heic"
        shutil.copyfile(source, staged)
        (temporary / "policy.xml").write_text('<policymap><policy domain="delegate" rights="none" pattern="*"/><policy domain="coder" rights="none" pattern="*"/><policy domain="coder" rights="read|write" pattern="{HEIC,PNG}"/><policy domain="resource" name="memory" value="256MiB"/><policy domain="resource" name="map" value="512MiB"/><policy domain="resource" name="disk" value="1GiB"/></policymap>')
        env = os.environ.copy(); env["MAGICK_CONFIGURE_PATH"] = str(temporary); env["MAGICK_TEMPORARY_PATH"] = str(temporary)
        result = subprocess.run([tool("magick"), "HEIC:" + str(staged) + "[0]", "PNG:-"], capture_output=True, timeout=120, env=env)
        if result.returncode:
            raise ValueError("HEIC decoding failed: " + result.stderr.decode("utf-8", "replace")[-2000:])
        return Image.open(io.BytesIO(result.stdout))


def run(request):
    from pypdf import PdfWriter
    kind = request["kind"]
    inputs = [Path(path) for path in request["inputs"]]
    staging = Path(request["staging"]).resolve()
    options = request.get("options") or {}
    if not isinstance(options, dict):
        raise ValueError("options must be an object")
    if not inputs:
        raise ValueError("Choose at least one input file")
    if not staging.is_dir():
        raise ValueError("The staging directory must exist")
    if kind == "images_to_pdf":
        from PIL import Image, ImageOps
        from reportlab.pdfgen import canvas
        from reportlab.lib.utils import ImageReader
        output = staging / "images.pdf"
        pdf = canvas.Canvas(str(output))
        for source in inputs:
            with open_raster(source, staging) as image:
                image = ImageOps.exif_transpose(image)
                if image.width * image.height > 100_000_000:
                    raise ValueError("Image exceeds the 100 megapixel page limit")
                rgba = image.convert("RGBA")
                background = Image.new("RGB", rgba.size, "white")
                background.paste(rgba, mask=rgba.getchannel("A"))
                scale = min(1, 14400 / max(background.size))
                width, height = (max(1, int(value * scale)) for value in background.size)
                pdf.setPageSize((width, height))
                pdf.drawImage(ImageReader(background), 0, 0, width, height)
                pdf.showPage()
        pdf.save()
        if len(reader(output).pages) != len(inputs):
            raise ValueError("Image-to-PDF page validation failed")
        return [output]
    if kind == "pdf_merge":
        writer = PdfWriter(); expected = 0
        for source in inputs:
            document = reader(source)
            selection = pages(options.get("pages"), len(document.pages))
            writer.append(document, pages=selection)
            expected += len(selection)
        return [save(writer, staging / "merged.pdf", expected)]
    if len(inputs) != 1:
        raise ValueError("This PDF operation accepts exactly one input")
    source = inputs[0]; document = reader(source)
    prefix = output_prefix(source)
    selection = pages(options.get("pages"), len(document.pages))
    if kind == "pdf_split":
        ranges = options.get("ranges")
        if ranges is not None:
            if not isinstance(ranges, list) or not ranges or len(ranges) > 5000:
                raise ValueError("ranges must be a nonempty list of page ranges")
            groups = [pages(spec, len(document.pages)) for spec in ranges]
        else:
            size = number(options, "chunk_size", 1, 1, 5000)
            if int(size) != size:
                raise ValueError("chunk_size must be an integer")
            size = int(size)
            groups = [selection[index:index + size] for index in range(0, len(selection), size)]
        outputs = []
        for index, group in enumerate(groups):
            writer = PdfWriter()
            for page in group:
                writer.add_page(document.pages[page])
            outputs.append(save(writer, staging / f"{prefix}-part-{index + 1:04}.pdf", len(group)))
        return outputs
    if kind == "pdf_to_text":
        text = "\n\f\n".join(document.pages[index].extract_text() or "" for index in selection)
        if not text.strip("\n\f "):
            raise ValueError("PDF has no extractable text; OCR is not included")
        output = staging / f"{prefix}-extracted.txt"; output.write_text(text, encoding="utf-8")
        return [output]
    if kind == "pdf_to_images":
        import pypdfium2 as pdfium
        from PIL import Image
        format_name = options.get("format", "png").lower()
        if format_name not in ["png", "jpg", "jpeg"]:
            raise ValueError("Image format must be png or jpg")
        format_name = "jpg" if format_name == "jpeg" else format_name
        dpi = number(options, "dpi", 150, 36, 300)
        if len(selection) > 500:
            raise ValueError("Render at most 500 pages per request")
        outputs = []
        with pdfium.PdfDocument(str(source)) as pdf:
            for index in selection:
                page = pdf[index]
                try:
                    width, height = page.get_size()
                    if width * height * (dpi / 72) ** 2 > 100_000_000:
                        raise ValueError("Rendered page exceeds 100 megapixels")
                    bitmap = page.render(scale=dpi / 72)
                    try:
                        image = bitmap.to_pil().convert("RGB")
                        output = staging / f"{prefix}-page-{index + 1:04}.{format_name}"
                        image.save(output, quality=90)
                    finally:
                        bitmap.close()
                    with Image.open(output) as check:
                        check.load()
                    outputs.append(output)
                finally:
                    page.close()
        return outputs
    if kind == "pdf_compress":
        preset = options.get("preset", "ebook")
        if preset not in ["screen", "ebook", "printer", "prepress"]:
            raise ValueError("Compression preset must be screen, ebook, printer or prepress")
        output = staging / f"{prefix}-compressed.pdf"
        command = [tool("gs"), "-dSAFER", "-dBATCH", "-dNOPAUSE", "-sDEVICE=pdfwrite", "-dCompatibilityLevel=1.7", f"-dPDFSETTINGS=/{preset}", f"-sOutputFile={output}", "--", str(source)]
        result = subprocess.run(command, capture_output=True, timeout=300)
        if result.returncode:
            raise ValueError("Ghostscript compression failed: " + result.stderr.decode("utf-8", "replace")[-4000:])
        if len(reader(output).pages) != len(document.pages):
            raise ValueError("Compression changed the page count")
        # Never label a larger re-encoding as a successful size reduction.
        if output.stat().st_size >= source.stat().st_size:
            shutil.copyfile(source, output)
        return [output]
    writer = PdfWriter(); writer.clone_document_from_reader(document)
    if kind == "pdf_rotate":
        angle = number(options, "angle", 90, -3600, 3600)
        if angle % 90:
            raise ValueError("Rotation angle must be a multiple of 90")
        for index in selection:
            writer.pages[index].rotate(int(angle))
    elif kind == "pdf_metadata":
        if not isinstance(options.get("strip", False), bool):
            raise ValueError("strip must be a boolean")
        if options.get("strip", False):
            writer.metadata = None
            writer.xmp_metadata = None
        metadata = {}
        for name in ["title", "author", "subject", "keywords", "creator"]:
            if name in options:
                if not isinstance(options[name], str) or len(options[name]) > 10000:
                    raise ValueError(f"{name} must be a string of at most 10000 characters")
                metadata["/" + name.title()] = options[name]
        if metadata:
            writer.add_metadata(metadata)
    elif kind == "pdf_watermark":
        from reportlab.pdfgen import canvas
        text = options.get("text", "")
        if not isinstance(text, str) or not text.strip() or len(text) > 500:
            raise ValueError("Watermark text must contain 1 to 500 characters")
        opacity = number(options, "opacity", 0.2, 0, 1)
        size = number(options, "font_size", 36, 6, 144)
        angle = number(options, "angle", 45, -360, 360)
        font = "Helvetica"
        if not text.isascii():
            from reportlab.pdfbase import pdfmetrics
            from reportlab.pdfbase.cidfonts import UnicodeCIDFont
            pdfmetrics.registerFont(UnicodeCIDFont("STSong-Light")); font = "STSong-Light"
        for index in selection:
            page = writer.pages[index]
            page.transfer_rotation_to_content()
            width, height = float(page.mediabox.width), float(page.mediabox.height)
            overlay = io.BytesIO(); layer = canvas.Canvas(overlay, pagesize=(width, height))
            layer.setFillAlpha(opacity); layer.setFont(font, size)
            layer.translate(width / 2, height / 2); layer.rotate(angle)
            layer.drawCentredString(0, 0, text); layer.save()
            from pypdf import PdfReader
            page.merge_page(PdfReader(io.BytesIO(overlay.getvalue())).pages[0])
    else:
        raise ValueError(f"Unsupported PDF operation: {kind}")
    return [save(writer, staging / (prefix + "-" + kind.removeprefix("pdf_") + ".pdf"), len(document.pages))]


if __name__ == "__main__":
    try:
        with open(sys.argv[1], encoding="utf-8") if len(sys.argv) > 1 else sys.stdin as stream:
            request = json.load(stream)
        outputs = run(request)
        print(json.dumps({"outputs": [str(path.resolve()) for path in outputs]}))
    except Exception as error:
        print(f"PDF workflow: {error}", file=sys.stderr)
        sys.exit(1)
