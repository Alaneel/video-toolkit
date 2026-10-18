//! Native application entry point. The original no-argument toolkit stays unchanged.
use clap::Parser;

#[derive(Parser)]
#[command(
    name = "video-agent",
    version,
    about = "Human-reviewed visual rough cuts with local media export"
)]
struct Options {
    /// Reopen an existing private task directory.
    #[arg(long)]
    workspace: Option<std::path::PathBuf>,
}

fn main() -> eframe::Result<()> {
    // Finder does not inherit a shell's Homebrew PATH. Extend only this process,
    // before spawning threads, and retain the user's existing search order.
    #[cfg(target_os = "macos")]
    {
        let current = std::env::var_os("PATH").unwrap_or_else(|| "/usr/bin:/bin".into());
        let mut paths: Vec<_> = std::env::split_paths(&current).collect();
        for directory in ["/opt/homebrew/bin", "/usr/local/bin"] {
            let path = std::path::PathBuf::from(directory);
            if path.is_dir() && !paths.contains(&path) {
                paths.push(path);
            }
        }
        if let Ok(path) = std::env::join_paths(paths) {
            std::env::set_var("PATH", path);
        }
    }
    ui::run_video_agent(Options::parse().workspace)
}
