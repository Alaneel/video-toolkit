pub mod agent_app;
pub mod app;
pub mod capture;
pub mod tabs;
pub mod theme;

pub use agent_app::VideoAgentApp;
pub use app::VideoToolKitApp;

/// Shared native host for the CLI subcommand and the double-clickable Agent app.
pub fn run_video_agent(workspace: Option<std::path::PathBuf>) -> eframe::Result<()> {
    eframe::run_native(
        "Video Agent",
        eframe::NativeOptions {
            viewport: egui::ViewportBuilder::default()
                .with_inner_size([1240.0, 840.0])
                .with_min_inner_size([980.0, 700.0]),
            ..Default::default()
        },
        Box::new(move |cc| {
            theme::configure(&cc.egui_ctx);
            Box::new(VideoAgentApp::new(workspace))
        }),
    )
}
