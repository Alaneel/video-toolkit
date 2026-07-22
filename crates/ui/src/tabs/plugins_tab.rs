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
        ui.heading("Plugin Management");

        ui.horizontal(|ui| {
            // Left panel: Plugin list and actions
            let list_size = egui::vec2(300.0, 400.0);
            ui.allocate_ui(list_size, |ui| {
                egui::ScrollArea::both()
                    .id_source("plugins_list_scroll")
                    .show(ui, |ui| {
                        self.plugin_list_ui(ui);
                    });
            });

            ui.separator();

            // Right panel: Plugin details and execution
            let details_size = egui::vec2(400.0, 400.0);
            ui.allocate_ui(details_size, |ui| {
                egui::ScrollArea::both()
                    .id_source("plugins_details_scroll")
                    .show(ui, |ui| {
                        self.plugin_details_ui(ui);
                    });
            });
        });
    }

    fn plugin_list_ui(&mut self, ui: &mut Ui) {
        // Plugin directory
        ui.horizontal(|ui| {
            ui.label("Plugin Directory:");
            ui.text_edit_singleline(&mut self.plugin_directory);
            if ui.button("Browse").clicked() {
                if let Some(path) = rfd::FileDialog::new().pick_folder() {
                    self.plugin_directory = path.to_string_lossy().to_string();
                }
            }
        });

        // Plugin discovery and refresh actions
        ui.horizontal(|ui| {
            if ui.button("Discover Plugins").clicked() {
                self.discover_plugins();
            }

            if ui.button("Refresh List").clicked() {
                self.refresh_plugin_list();
            }
        });

        ui.separator();

        // Plugin list
        ScrollArea::vertical().show(ui, |ui| {
            if self.plugin_list.is_empty() {
                ui.label(
                    "No plugins loaded. Use 'Discover Plugins' or 'Load Plugin' to find plugins.",
                );
            } else {
                ui.heading("Installed Plugins");

                // Store clicked index for processing after the loop
                let mut clicked_idx = None;
                let current_selected = self.selected_plugin_index;

                for (i, plugin) in self.plugin_list.iter().enumerate() {
                    let is_selected = current_selected == Some(i);
                    let selection_ui = ui.selectable_label(
                        is_selected,
                        format!("{} v{}", plugin.name, plugin.version),
                    );

                    if selection_ui.clicked() {
                        clicked_idx = Some(i);
                    }
                }

                // Handle selection changes after the loop
                if let Some(idx) = clicked_idx {
                    if current_selected == Some(idx) {
                        self.selected_plugin_index = None;
                    } else {
                        self.selected_plugin_index = Some(idx);
                        self.update_execution_parameters();
                    }
                }
            }
        });

        ui.separator();

        // Load individual plugin
        ui.horizontal(|ui| {
            ui.label("Plugin File:");
            ui.text_edit_singleline(&mut self.plugin_path);
            if ui.button("Browse").clicked() {
                if let Some(path) = rfd::FileDialog::new()
                    .add_filter("Plugin Files", self.get_platform_plugin_extension())
                    .pick_file()
                {
                    self.plugin_path = path.to_string_lossy().to_string();
                }
            }
        });

        if ui.button("Load Plugin").clicked() {
            self.load_plugin();
        }
    }

    fn plugin_details_ui(&mut self, ui: &mut Ui) {
        // Show plugin details if a plugin is selected
        if let Some(index) = self.selected_plugin_index {
            if index < self.plugin_list.len() {
                let plugin = &self.plugin_list[index];

                ui.heading("Plugin Details");
                ui.label(format!("Name: {} v{}", plugin.name, plugin.version));
                ui.label(format!("Author: {}", plugin.author));
                ui.label(format!("Description: {}", plugin.description));
                ui.label(format!("API Version: {}", plugin.api_version));

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
                                ui.text_edit_singleline(value);

                                // Parameter description
                                ui.label(&info.description);
                                ui.end_row();
                            }
                        });

                        ui.separator();

                        // Execute plugin button
                        if ui.button("Execute Plugin").clicked() {
                            self.execute_plugin();
                        }
                    } else {
                        ui.label("This plugin does not declare any parameters.");

                        ui.separator();

                        // Execute plugin button (no parameters)
                        if ui.button("Execute Plugin").clicked() {
                            self.execute_plugin();
                        }
                    }
                } else {
                    ui.label("Error: Unable to access plugin.");
                }
            }
        } else {
            ui.heading("Plugin Details");
            ui.label("Select a plugin from the list to view details.");
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
