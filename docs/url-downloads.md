# URL downloads

Open **Downloads** in the desktop or **From URL** on Convert. Choose a direct file or media link, output folder, optional filename and (for media links) format. Start the same cancellable background queue used by the other studios. Links must use HTTP/HTTPS without embedded credentials.

```sh
video-toolkit workflow download --output-dir downloads --options '{"url":"https://example.com/file.zip","mode":"direct","filename":"file.zip","max_mb":2048}' --json
video-toolkit workflow download --output-dir downloads --options '{"url":"https://example.com/video","mode":"media","format":"mp3","filename":"soundtrack"}' --json
```

`url` is required. `mode` defaults to `direct`; `format` defaults to `original`. `filename` is a plain basename, never a folder. `max_mb` defaults to 2048 MiB and accepts 1–102400. Direct mode supports `original` only, saves the response bytes without transformation, follows at most five HTTP/HTTPS redirects and checks size/nonempty content. It uses curl without the user's curl configuration. No Content-Disposition filename is trusted.

Media mode uses a project-local yt-dlp from `scripts/setup-file-workflows.sh`, with FFmpeg/ffprobe for extraction, transcoding and full output decode. Targets are `original`, `mp4`, `webm`, `mp3`, `m4a`; output names use the resulting extension. Playlist expansion, user configuration, external plugins, cookies, netrc and executable hooks are not enabled. Media support depends on the site's current extractor and access rules. The app does not bypass login, DRM or site restrictions. Failure is reported in the queue without publishing a partial file.

Progress reports received bytes for direct downloads and staged bytes while media is downloading/transcoding; unknown remote sizes use stage progress. Cancellation terminates the owned dependency process group on Unix, waits for cleanup and removes staged files. The configured byte limit is checked during work and before publication. Existing destinations always win; they are never replaced. Downloaded media passes full FFmpeg decode; direct arbitrary files are validated for transfer completion and size, without claiming their format or contents have been decoded.

The MCP `run_workflow` request uses `kind:"download"`, `inputs:[]`, an existing `output_dir` within a server `--root`, and the same `options` object. Downloading is an explicit network operation; no links are fetched by tool discovery or local conversion.

Real integration tests use only an ephemeral loopback HTTP server and synthetic bytes/audio, including redirects, no-overwrite, cancellation, size rejection, byte preservation and MP3 extraction/decoding:

```sh
cargo test --locked -p file_workflows --test downloads -- --ignored --test-threads=1
```
