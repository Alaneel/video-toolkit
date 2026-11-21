use crate::theme::{self, Icon, Tone};
use eframe::egui::{self, Grid, ScrollArea, Ui};
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::thread;

use plugin_system::{PluginManager, PluginMetadata};

pub struct PluginsTab {
    plugin_manager: PluginManager,

    // UI state
    plugin_list: Vec<PluginMetadata>,
    selected_plugin_index: Option<usize>,
    plugin_directory: String,

    // Plugin execution
    execution_parameters: Vec<(String, String)>,

    // Plugin loading
    plugin_path: String,

    // Status
    status: Arc<Mutex<String>>,
    processing: Arc<Mutex<bool>>,
}

impl PluginsTab {
    pub fn new(status: Arc<Mutex<String>>, processing: Arc<Mutex<bool>>) -> Self {
        // Create plugin manager
        let plugin_manager = match PluginManager::new() {
            Ok(pm) => pm,
            Err(e) => {
                *status.lock().unwrap() = format!("Error initializing plugin manager: {}", e);
                // Create empty plugin manager as fallback
                PluginManager::new().expect("Failed to create plugin manager as fallback")
            }
        };

        // Get plugin list
        let plugin_list = plugin_manager.get_all_plugin_metadata();

        Self {
            plugin_manager,
            plugin_list,
            selected_plugin_index: None,
            plugin_directory: "plugins".to_string(),
            execution_parameters: Vec::new(),
            plugin_path: String::new(),
            status,
            processing,
        }
    }

    pub fn ui(&mut self, ui: &mut Ui) {
        let total = ui.available_width().min(1100.0);
        let left = (total * 0.42).clamp(320.0, 440.0);
        let right = (total - left - 16.0).max(320.0);
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = 16.0;
            ui.allocate_ui_with_layout(
                egui::vec2(left, 0.0),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    ui.set_width(left);
                    self.plugin_list_ui(ui);
                },
            );
            ui.allocate_ui_with_layout(
                egui::vec2(right, 0.0),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    ui.set_width(right);
                    theme::card().show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.set_min_height(320.0);
                        self.plugin_details_ui(ui);
                    });
                },
            );
        });
    }

    fn plugin_list_ui(&mut self, ui: &mut Ui) {
        theme::section(
            ui,
            "Library",
            Some("Plugins found in your plugin folder."),
            |ui| {
                ui.horizontal(|ui| {
                    theme::icon(ui, Icon::Folder, 16.0, theme::MUTED);
                    let width = (ui.available_width() - 90.0).max(80.0);
                    ui.add(
                        egui::TextEdit::singleline(&mut self.plugin_directory)
                            .margin(egui::vec2(10.0, 7.0))
                            .desired_width(width),
                    );
                    if ui.add(theme::secondary_button("Browse")).clicked() {
                        if let Some(path) = rfd::FileDialog::new().pick_folder() {
                            self.plugin_directory = path.to_string_lossy().to_string();
                        }
                    }
                });
                ui.horizontal(|ui| {
                    if ui.add(theme::primary_button("Discover")).clicked() {
                        self.discover_plugins();
                    }
                    if ui.add(theme::secondary_button("Refresh")).clicked() {
                        self.refresh_plugin_list();
                    }
                });
                ui.separator();
                if self.plugin_list.is_empty() {
                    ui.add_space(12.0);
                    theme::empty_state(
                        ui,
                        Icon::Grid,
                        "No plugins loaded",
                        "Discover a folder or load a single plugin file.",
                    );
                    ui.add_space(12.0);
                } else {
                    let mut clicked_idx = None;
                    let current_selected = self.selected_plugin_index;
                    ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
                        for (i, plugin) in self.plugin_list.iter().enumerate() {
                            let is_selected = current_selected == Some(i);
                            let response = egui::Frame::none()
                                .fill(if is_selected {
                                    theme::ACCENT_SOFT
                                } else {
                                    theme::SUNKEN
                                })
                                .stroke(egui::Stroke::new(
                                    1.0_f32,
                                    if is_selected {
                                        theme::ACCENT_LINE
                                    } else {
                                        theme::LINE
                                    },
                                ))
                                .rounding(theme::RADIUS)
                                .inner_margin(egui::Margin::symmetric(10.0, 8.0))
                                .show(ui, |ui| {
                                    ui.set_width(ui.available_width());
                                    ui.horizontal(|ui| {
                                        theme::icon_tile(ui, Icon::Grid, theme::HUE_DOWNLOAD, 26.0);
                                        ui.label(
                                            egui::RichText::new(&plugin.name)
                                                .font(theme::semibold(13.0))
                                                .color(theme::TEXT),
                                        );
                                        theme::badge(
                                            ui,
                                            &format!("v{}", plugin.version),
                                            Tone::Neutral,
                                        );
                                    });
                                })
                                .response
                                .interact(egui::Sense::click())
                                .on_hover_cursor(egui::CursorIcon::PointingHand);
                            if response.clicked() {
                                clicked_idx = Some(i);
                            }
                        }
                    });
                    if let Some(idx) = clicked_idx {
                        if current_selected == Some(idx) {
                            self.selected_plugin_index = None;
                        } else {
                            self.selected_plugin_index = Some(idx);
                            self.update_execution_parameters();
                        }
                    }
                }
            },
        );

        theme::section(ui, "Load a single plugin", None, |ui| {
            ui.horizontal(|ui| {
                let width = (ui.available_width() - 90.0).max(80.0);
                ui.add(
                    egui::TextEdit::singleline(&mut self.plugin_path)
                        .margin(egui::vec2(10.0, 7.0))
                        .hint_text(egui::RichText::new("Plugin file").color(theme::FAINT))
                        .desired_width(width),
                );
                if ui.add(theme::secondary_button("Browse")).clicked() {
                    if let Some(path) = rfd::FileDialog::new()
                        .add_filter("Plugin Files", self.get_platform_plugin_extension())
                        .pick_file()
                    {
                        self.plugin_path = path.to_string_lossy().to_string();
                    }
                }
            });
            if ui.add(theme::secondary_button("Load plugin")).clicked() {
                self.load_plugin();
            }
        });
    }

    fn plugin_details_ui(&mut self, ui: &mut Ui) {
        // Show plugin details if a plugin is selected
        if let Some(index) = self.selected_plugin_index {
            if index < self.plugin_list.len() {
                let plugin = &self.plugin_list[index];

                ui.horizontal(|ui| {
                    theme::icon_tile(ui, Icon::Grid, theme::HUE_DOWNLOAD, 40.0);
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 0.0;
                        ui.label(
                            egui::RichText::new(&plugin.name)
                                .font(theme::bold(18.0))
                                .color(theme::TEXT),
                        );
                        ui.label(
                            egui::RichText::new(format!(
                                "v{} · by {} · API {}",
                                plugin.version, plugin.author, plugin.api_version
                            ))
                            .size(12.5)
                            .color(theme::MUTED),
                        );
                    });
                });
                theme::hint(ui, &plugin.description);

                ui.separator();

                // Get the plugin to show parameter info
                if let Some(param_info) = self.plugin_manager.get_plugin_parameters(&plugin.name) {
                    if !param_info.is_empty() {
                        ui.heading("Parameters");

                        // Use a grid for parameter editing
                        Grid::new("parameters_grid").show(ui, |ui| {
                            for (i, info) in param_info.iter().enumerate() {
                                // Ensure we have a parameter entry for this info
                                if i >= self.execution_parameters.len() {
                                    let default_value =
                                        info.default_value.clone().unwrap_or_default();
                                    self.execution_parameters
                                        .push((info.name.clone(), default_value));
                                }

                                let required_text = if info.required { " (*)" } else { "" };
                                ui.label(format!("{}{}:", info.name, required_text));

                                // Update parameter value
                                let (_, value) = &mut self.execution_parameters[i];
                                crate::theme::text_input(ui, value);

                                // Parameter description
                                ui.label(&info.description);
                                ui.end_row();
                            }
                        });

                        ui.separator();

                        // Execute plugin button
                        if ui.add(theme::primary_button("Run plugin")).clicked() {
                            self.execute_plugin();
                        }
                    } else {
                        ui.label("This plugin does not declare any parameters.");

                        ui.separator();

                        // Execute plugin button (no parameters)
                        if ui.add(theme::primary_button("Run plugin")).clicked() {
                            self.execute_plugin();
                        }
                    }
                } else {
                    ui.label("Error: Unable to access plugin.");
                }
            }
        } else {
            ui.add_space(90.0);
            theme::empty_state(
                ui,
                Icon::Grid,
                "No plugin selected",
                "Pick a plugin from the library to see its parameters and run it.",
            );
        }
    }

    fn discover_plugins(&mut self) {
        *self.status.lock().unwrap() = "Discovering plugins...".to_string();
        *self.processing.lock().unwrap() = true;

        // Update plugin directory
        let plugin_dir = self.plugin_directory.clone();

        // Add plugin directory
        self.plugin_manager.add_plugin_directory(&plugin_dir);

        // Discover plugins in a separate thread
        let plugin_manager = self.plugin_manager.clone();
        let status_clone = Arc::clone(&self.status);
        let processing_clone = Arc::clone(&self.processing);
        let plugin_list = Arc::new(Mutex::new(Vec::new()));
        let plugin_list_clone = Arc::clone(&plugin_list);

        thread::spawn(move || {
            let results = plugin_manager.discover_plugins();

            // Count successes and failures
            let success_count = results.iter().filter(|r| r.is_ok()).count();
            let failure_count = results.len() - success_count;

            // Update status
            if failure_count > 0 {
                *status_clone.lock().unwrap() = format!(
                    "Discovered {} plugin(s), {} failed to load.",
                    success_count, failure_count
                );
            } else if success_count > 0 {
                *status_clone.lock().unwrap() =
                    format!("Successfully discovered {} plugin(s).", success_count);
            } else {
                *status_clone.lock().unwrap() = "No plugins found.".to_string();
            }

            // Update plugin list
            *plugin_list_clone.lock().unwrap() =
                results.into_iter().filter_map(|r| r.ok()).collect();

            *processing_clone.lock().unwrap() = false;
        });

        // Wait for the thread to update the plugin list
        // In a real app, you might want to use a more sophisticated approach with UI state
        // Here we take a simple approach and just refresh the list after a short delay
        thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(500));
            // The caller should call refresh_plugin_list after this function returns
        });

        // Refresh the list
        self.refresh_plugin_list();
    }

    fn load_plugin(&mut self) {
        if self.plugin_path.is_empty() {
            *self.status.lock().unwrap() = "Error: Please select a plugin file.".to_string();
            return;
        }

        *self.status.lock().unwrap() = "Loading plugin...".to_string();
        *self.processing.lock().unwrap() = true;

        let path = self.plugin_path.clone();
        let plugin_manager = self.plugin_manager.clone();
        let status_clone = Arc::clone(&self.status);
        let processing_clone = Arc::clone(&self.processing);

        thread::spawn(move || {
            match plugin_manager.load_plugin(Path::new(&path)) {
                Ok(()) => {
                    *status_clone.lock().unwrap() = "Plugin loaded successfully.".to_string();
                }
                Err(e) => {
                    *status_clone.lock().unwrap() = format!("Error loading plugin: {}", e);
                }
            }

            *processing_clone.lock().unwrap() = false;
        });

        // Refresh the list after a short delay
        thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(500));
            // The caller should call refresh_plugin_list after this function returns
        });

        // Refresh the list
        self.refresh_plugin_list();
    }

    fn refresh_plugin_list(&mut self) {
        self.plugin_list = self.plugin_manager.get_all_plugin_metadata();

        // Clear selection if the selected plugin no longer exists
        if let Some(index) = self.selected_plugin_index {
            if index >= self.plugin_list.len() {
                self.selected_plugin_index = None;
                self.execution_parameters.clear();
            }
        }
    }

    fn update_execution_parameters(&mut self) {
        // Clear current parameters
        self.execution_parameters.clear();

        // Get parameters from the plugin
        if let Some(index) = self.selected_plugin_index {
            if index < self.plugin_list.len() {
                let plugin_name = &self.plugin_list[index].name;

                if let Some(param_info) = self.plugin_manager.get_plugin_parameters(plugin_name) {
                    for info in param_info {
                        let default_value = info.default_value.unwrap_or_default();
                        self.execution_parameters.push((info.name, default_value));
                    }
                }
            }
        }
    }

    fn execute_plugin(&mut self) {
        if let Some(index) = self.selected_plugin_index {
            if index < self.plugin_list.len() {
                let plugin_name = self.plugin_list[index].name.clone();

                // Convert parameters to HashMap
                let mut params = HashMap::new();
                for (key, value) in &self.execution_parameters {
                    params.insert(key.clone(), value.clone());
                }

                *self.status.lock().unwrap() = format!("Executing plugin '{}'...", plugin_name);
                *self.processing.lock().unwrap() = true;

                let plugin_manager = self.plugin_manager.clone();
                let status_clone = Arc::clone(&self.status);
                let processing_clone = Arc::clone(&self.processing);

                thread::spawn(move || {
                    // Execute the plugin
                    match plugin_manager.execute_plugin(&plugin_name, params) {
                        Ok(()) => {
                            *status_clone.lock().unwrap() =
                                format!("Plugin '{}' executed successfully.", plugin_name);
                        }
                        Err(e) => {
                            *status_clone.lock().unwrap() =
                                format!("Error executing plugin: {}", e);
                        }
                    }

                    *processing_clone.lock().unwrap() = false;
                });
            }
        } else {
            *self.status.lock().unwrap() = "Error: No plugin selected.".to_string();
        }
    }

    fn get_platform_plugin_extension(&self) -> &'static [&'static str] {
        #[cfg(target_os = "windows")]
        return &["dll"];

        #[cfg(target_os = "linux")]
        return &["so"];

        #[cfg(target_os = "macos")]
        return &["dylib"];

        #[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
        return &[""];
    }
}
