//! Developer-only window capture used to refresh documentation screenshots.
//!
//! `VTK_SCREENSHOT=out.png VTK_PAGE=editors video-toolkit` renders the page,
//! saves one frame and exits. Nothing happens when the variable is unset.
use eframe::egui;
use std::path::PathBuf;

pub struct Capture {
    path: PathBuf,
    frames: u32,
    requested: bool,
}

impl Capture {
    pub fn from_env() -> Option<Self> {
        let path = std::env::var_os("VTK_SCREENSHOT")?;
        Some(Self {
            path: path.into(),
            frames: 0,
            requested: false,
        })
    }

    /// The page named by `VTK_PAGE`, lower-case.
    pub fn page() -> Option<String> {
        std::env::var("VTK_PAGE").ok().map(|p| p.to_lowercase())
    }

    pub fn update(&mut self, ctx: &egui::Context) {
        self.update_when(ctx, true);
    }

    /// Capture once the page reports it has finished loading.
    pub fn update_when(&mut self, ctx: &egui::Context, ready: bool) {
        let image = ctx.input(|input| {
            input.events.iter().find_map(|event| match event {
                egui::Event::Screenshot { image, .. } => Some(image.clone()),
                _ => None,
            })
        });
        if let Some(image) = image {
            let [w, h] = image.size;
            let bytes: Vec<u8> = image.pixels.iter().flat_map(|c| c.to_array()).collect();
            match image::RgbaImage::from_raw(w as u32, h as u32, bytes)
                .map(|img| img.save(&self.path))
            {
                Some(Ok(())) => eprintln!("saved {}", self.path.display()),
                other => eprintln!("screenshot failed: {other:?}"),
            }
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }
        if ready {
            self.frames += 1;
        }
        // Let fonts, layout and first-frame animations settle.
        if self.frames > 20 && !self.requested {
            self.requested = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot);
        }
        ctx.request_repaint();
    }
}
