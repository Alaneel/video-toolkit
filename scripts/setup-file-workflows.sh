#!/usr/bin/env bash
# Optional dependencies stay private to this checkout; no system Python mutation.
set -euo pipefail
repo_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
python_bin="${VIDEO_TOOLKIT_SETUP_PYTHON:-python3}"
"$python_bin" -c 'import sys; assert sys.version_info >= (3, 10), "Python 3.10+ is required"'
"$python_bin" -m venv "$repo_dir/.toolkit-venv"
"$repo_dir/.toolkit-venv/bin/python" -m pip install --disable-pip-version-check -r "$repo_dir/scripts/requirements-workflows.txt"
echo 'File workflow runtime is ready. PDF compression also requires Ghostscript.'
echo 'Document PDF export requires LibreOffice; set VIDEO_TOOLKIT_SOFFICE if it is outside PATH.'
echo 'Run video-toolkit doctor --json to inspect executable availability.'
