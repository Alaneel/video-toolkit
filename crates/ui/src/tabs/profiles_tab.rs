use eframe::egui::{self, ComboBox, Ui};
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

use profile_system::{Profile, ProfileManager, ProfileType};

pub struct ProfilesTab {
    profile_manager: ProfileManager,

    // UI state
    selected_profile_type: ProfileTypeSelection,
    selected_profile_name: String,
    available_profiles: HashMap<ProfileType, Vec<String>>,

    // Profile creation/editing
    edit_mode: EditMode,
    profile_name: String,
    profile_description: String,
    profile_parameters: Vec<(String, String)>,

    // Import/Export
    import_path: String,
    export_path: String,

    // Status
    status: Arc<Mutex<String>>,
}

#[derive(PartialEq, Clone)]
pub enum ProfileTypeSelection {
    Clipper,
    GifConverter,
    GifTransparency,
    Splitter,
    Merger,
    Custom(String),
}

impl ProfileTypeSelection {
    fn to_profile_type(&self) -> ProfileType {
        match self {
            ProfileTypeSelection::Clipper => ProfileType::Clipper,
            ProfileTypeSelection::GifConverter => ProfileType::GifConverter,
            ProfileTypeSelection::GifTransparency => ProfileType::GifTransparency,
            ProfileTypeSelection::Splitter => ProfileType::Splitter,
            ProfileTypeSelection::Merger => ProfileType::Merger,
            ProfileTypeSelection::Custom(name) => ProfileType::Custom(name.clone()),
        }
    }

    fn display_name(&self) -> String {
        match self {
            ProfileTypeSelection::Clipper => "Clipper".to_string(),
            ProfileTypeSelection::GifConverter => "GIF Converter".to_string(),
            ProfileTypeSelection::GifTransparency => "GIF Transparency".to_string(),
            ProfileTypeSelection::Splitter => "Splitter".to_string(),
            ProfileTypeSelection::Merger => "Merger".to_string(),
            ProfileTypeSelection::Custom(name) => format!("Custom: {}", name),
        }
    }
}

#[derive(PartialEq)]
enum EditMode {
    None,
    Create,
    Edit,
    Delete,
    Import,
    Export,
}

impl ProfilesTab {
    pub fn new(status: Arc<Mutex<String>>, _processing: Arc<Mutex<bool>>) -> Self {
        let profile_manager = match ProfileManager::new() {
            Ok(pm) => pm,
            Err(e) => {
                *status.lock().unwrap() = format!("Error initializing profile manager: {}", e);
                // Create a dummy manager with a temporary directory
                ProfileManager::with_directory(std::env::temp_dir()).unwrap()
            }
        };

        // Get available profiles
        let available_profiles = profile_manager.list_all_profiles().unwrap_or_default();

        Self {
            profile_manager,
            selected_profile_type: ProfileTypeSelection::Clipper,
            selected_profile_name: String::new(),
            available_profiles,
            edit_mode: EditMode::None,
            profile_name: String::new(),
            profile_description: String::new(),
            profile_parameters: Vec::new(),
            import_path: String::new(),
            export_path: String::new(),
            status,
        }
    }

    pub fn ui(&mut self, ui: &mut Ui) {
        ui.heading("Profile Management");

        // Profile type selection
        ui.horizontal(|ui| {
            ui.label("Profile Type:");
            ComboBox::from_id_source("profile_type")
                .selected_text(self.selected_profile_type.display_name())
                .show_ui(ui, |ui| {
                    ui.selectable_value(
                        &mut self.selected_profile_type,
                        ProfileTypeSelection::Clipper,
                        "Clipper",
                    );
                    ui.selectable_value(
                        &mut self.selected_profile_type,
                        ProfileTypeSelection::GifConverter,
                        "GIF Converter",
                    );
                    ui.selectable_value(
                        &mut self.selected_profile_type,
                        ProfileTypeSelection::GifTransparency,
                        "GIF Transparency",
                    );
                    ui.selectable_value(
                        &mut self.selected_profile_type,
                        ProfileTypeSelection::Splitter,
                        "Splitter",
                    );
                    ui.selectable_value(
                        &mut self.selected_profile_type,
                        ProfileTypeSelection::Merger,
                        "Merger",
                    );
                    // TODO: Add custom profile types if needed
                });
        });

        ui.separator();

        // Main profile actions
        match self.edit_mode {
            EditMode::None => self.show_profile_list(ui),
            EditMode::Create => self.show_create_profile(ui),
            EditMode::Edit => self.show_edit_profile(ui),
            EditMode::Delete => self.show_delete_profile(ui),
            EditMode::Import => self.show_import_profile(ui),
            EditMode::Export => self.show_export_profile(ui),
        }
    }

    fn show_profile_list(&mut self, ui: &mut Ui) {
        // Refresh profile list button
        if ui.button("Refresh Profile List").clicked() {
            match self.profile_manager.list_all_profiles() {
                Ok(profiles) => {
                    self.available_profiles = profiles;
                    *self.status.lock().unwrap() = "Profile list refreshed.".to_string();
                }
                Err(e) => {
                    *self.status.lock().unwrap() = format!("Error refreshing profiles: {}", e);
                }
            }
        }

        // Get profiles for the selected type
        let profile_type = self.selected_profile_type.to_profile_type();
        let profiles = self
            .available_profiles
            .get(&profile_type)
            .cloned()
            .unwrap_or_default();

        // Profile selection
        if profiles.is_empty() {
            ui.label("No profiles available for this type.");
        } else {
            ui.label("Select a profile:");

            ComboBox::from_id_source("profile_selection")
                .selected_text(if self.selected_profile_name.is_empty() {
                    "Select a profile".to_string()
                } else {
                    self.selected_profile_name.clone()
                })
                .show_ui(ui, |ui| {
                    for name in &profiles {
                        if ui
                            .selectable_label(self.selected_profile_name == *name, name)
                            .clicked()
                        {
                            self.selected_profile_name = name.clone();
                        }
                    }
                });

            // Show profile details if selected
            if !self.selected_profile_name.is_empty() {
                ui.separator();

                match self
                    .profile_manager
                    .load_profile(&self.selected_profile_name, profile_type)
                {
                    Ok(profile) => {
                        ui.heading("Profile Details");
                        ui.label(format!("Name: {}", profile.name));
                        if let Some(desc) = &profile.description {
                            ui.label(format!("Description: {}", desc));
                        }
                        ui.label(format!("Created: {}", profile.created));
                        ui.label(format!("Last Modified: {}", profile.last_modified));

                        ui.separator();
                        ui.label("Parameters:");
                        for (key, value) in &profile.parameters {
                            ui.horizontal(|ui| {
                                ui.label(format!("{}:", key));
                                ui.label(value);
                            });
                        }
                    }
                    Err(e) => {
                        ui.label(format!("Error loading profile: {}", e));
                    }
                }
            }
        }

        ui.separator();

        // Action buttons
        ui.horizontal(|ui| {
            if ui.button("Create New").clicked() {
                self.edit_mode = EditMode::Create;
                self.profile_name = String::new();
                self.profile_description = String::new();
                self.profile_parameters = vec![(String::new(), String::new())];
            }

            if !self.selected_profile_name.is_empty() {
                if ui.button("Edit").clicked() {
                    // Load profile for editing
                    self.edit_mode = EditMode::Edit;
                    self.load_profile_for_editing();
                }

                if ui.button("Delete").clicked() {
                    self.edit_mode = EditMode::Delete;
                }
            }
        });

        ui.separator();

        // Import/Export buttons
        ui.horizontal(|ui| {
            if ui.button("Import Profile").clicked() {
                self.edit_mode = EditMode::Import;
                self.import_path = String::new();
            }

            if !self.selected_profile_name.is_empty() && ui.button("Export Profile").clicked() {
                self.edit_mode = EditMode::Export;
                self.export_path = String::new();
            }
        });
    }

    fn show_create_profile(&mut self, ui: &mut Ui) {
        ui.heading("Create New Profile");

        self.profile_edit_form(ui);

        ui.separator();

        ui.horizontal(|ui| {
            if ui.button("Create").clicked() {
                self.create_profile();
            }

            if ui.button("Cancel").clicked() {
                self.edit_mode = EditMode::None;
            }
        });
    }

    fn show_edit_profile(&mut self, ui: &mut Ui) {
        ui.heading(format!("Edit Profile: {}", self.selected_profile_name));

        self.profile_edit_form(ui);

        ui.separator();

        ui.horizontal(|ui| {
            if ui.button("Save Changes").clicked() {
                self.update_profile();
            }

            if ui.button("Cancel").clicked() {
                self.edit_mode = EditMode::None;
            }
        });
    }

    fn show_delete_profile(&mut self, ui: &mut Ui) {
        ui.heading("Delete Profile");

        ui.label(format!(
            "Are you sure you want to delete profile '{}'?",
            self.selected_profile_name
        ));
        ui.label("This action cannot be undone.");

        ui.separator();

        ui.horizontal(|ui| {
            if ui.button("Yes, Delete").clicked() {
                self.delete_profile();
            }

            if ui.button("Cancel").clicked() {
                self.edit_mode = EditMode::None;
            }
        });
    }

    fn show_import_profile(&mut self, ui: &mut Ui) {
        ui.heading("Import Profile");

        ui.horizontal(|ui| {
            ui.label("Profile File:");
            ui.text_edit_singleline(&mut self.import_path);
            if ui.button("Browse").clicked() {
                if let Some(path) = rfd::FileDialog::new()
                    .add_filter("JSON Files", &["json"])
                    .pick_file()
                {
                    self.import_path = path.to_string_lossy().to_string();
                }
            }
        });

        ui.separator();

        ui.horizontal(|ui| {
            if ui.button("Import").clicked() {
                self.import_profile();
            }

            if ui.button("Cancel").clicked() {
                self.edit_mode = EditMode::None;
            }
        });
    }

    fn show_export_profile(&mut self, ui: &mut Ui) {
        ui.heading("Export Profile");

        ui.label(format!("Exporting profile: {}", self.selected_profile_name));

        ui.horizontal(|ui| {
            ui.label("Save to:");
            ui.text_edit_singleline(&mut self.export_path);
            if ui.button("Browse").clicked() {
                if let Some(path) = rfd::FileDialog::new()
                    .add_filter("JSON Files", &["json"])
                    .save_file()
                {
                    self.export_path = path.to_string_lossy().to_string();

                    // Add .json extension if missing
                    if !self.export_path.ends_with(".json") {
                        self.export_path.push_str(".json");
                    }
                }
            }
        });

        ui.separator();

        ui.horizontal(|ui| {
            if ui.button("Export").clicked() {
                self.export_profile();
            }

            if ui.button("Cancel").clicked() {
                self.edit_mode = EditMode::None;
            }
        });
    }

    fn profile_edit_form(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.label("Name:");
            ui.text_edit_singleline(&mut self.profile_name);
        });

        ui.horizontal(|ui| {
            ui.label("Description:");
            ui.text_edit_singleline(&mut self.profile_description);
        });

        ui.separator();
        ui.label("Parameters:");

        let mut remove_idx = None;
        for (i, (key, value)) in self.profile_parameters.iter_mut().enumerate() {
            ui.horizontal(|ui| {
                let mut key_clone = key.clone();
                ui.add_sized(
                    egui::vec2(150.0, 0.0),
                    egui::TextEdit::singleline(&mut key_clone),
                );
                *key = key_clone;
                ui.label("=");
                let mut value_clone = value.clone();
                ui.add_sized(
                    egui::vec2(250.0, 0.0),
                    egui::TextEdit::singleline(&mut value_clone),
                );
                *value = value_clone;

                if ui.button("Remove").clicked() {
                    remove_idx = Some(i);
                }
            });
        }

        // Remove parameter if requested
        if let Some(idx) = remove_idx {
            self.profile_parameters.remove(idx);
        }

        // Add parameter button
        if ui.button("Add Parameter").clicked() {
            self.profile_parameters.push((String::new(), String::new()));
        }
    }

    fn load_profile_for_editing(&mut self) {
        let profile_type = self.selected_profile_type.to_profile_type();

        match self
            .profile_manager
            .load_profile(&self.selected_profile_name, profile_type)
        {
            Ok(profile) => {
                self.profile_name = profile.name.clone();
                self.profile_description = profile.description.unwrap_or_default();

                // Convert parameters to vector of key-value pairs
                self.profile_parameters = profile.parameters.into_iter().collect();

                // Add an empty parameter row if none exist
                if self.profile_parameters.is_empty() {
                    self.profile_parameters.push((String::new(), String::new()));
                }
            }
            Err(e) => {
                *self.status.lock().unwrap() = format!("Error loading profile for editing: {}", e);
                self.edit_mode = EditMode::None;
            }
        }
    }

    fn create_profile(&mut self) {
        // Validate inputs
        if self.profile_name.is_empty() {
            *self.status.lock().unwrap() = "Error: Profile name cannot be empty.".to_string();
            return;
        }

        // Convert parameters to HashMap
        let parameters = self.build_parameters_map();

        // Create profile
        let profile_type = self.selected_profile_type.to_profile_type();
        let mut profile = Profile::new(&self.profile_name, profile_type, parameters);

        if !self.profile_description.is_empty() {
            profile = profile.with_description(&self.profile_description);
        }

        // Save profile
        match self.profile_manager.save_profile(&profile) {
            Ok(()) => {
                *self.status.lock().unwrap() =
                    format!("Profile '{}' created successfully.", self.profile_name);
                self.edit_mode = EditMode::None;

                // Update available profiles
                if let Ok(profiles) = self.profile_manager.list_all_profiles() {
                    self.available_profiles = profiles;
                }

                self.selected_profile_name = self.profile_name.clone();
            }
            Err(e) => {
                *self.status.lock().unwrap() = format!("Error creating profile: {}", e);
            }
        }
    }

    fn update_profile(&mut self) {
        // Validate inputs
        if self.profile_name.is_empty() {
            *self.status.lock().unwrap() = "Error: Profile name cannot be empty.".to_string();
            return;
        }

        // Convert parameters to HashMap
        let parameters = self.build_parameters_map();

        // Create updated profile
        let profile_type = self.selected_profile_type.to_profile_type();
        let profile_type_for_delete = profile_type.clone();
        let mut profile = Profile::new(&self.profile_name, profile_type, parameters);

        if !self.profile_description.is_empty() {
            profile = profile.with_description(&self.profile_description);
        }

        // If name changed, delete old profile first
        if self.profile_name != self.selected_profile_name {
            let _ = self
                .profile_manager
                .delete_profile(&self.selected_profile_name, profile_type_for_delete);

            // Save new profile
            match self.profile_manager.save_profile(&profile) {
                Ok(()) => {
                    *self.status.lock().unwrap() =
                        format!("Profile '{}' updated successfully.", self.profile_name);
                    self.edit_mode = EditMode::None;

                    // Update available profiles
                    if let Ok(profiles) = self.profile_manager.list_all_profiles() {
                        self.available_profiles = profiles;
                    }

                    self.selected_profile_name = self.profile_name.clone();
                }
                Err(e) => {
                    *self.status.lock().unwrap() = format!("Error updating profile: {}", e);
                }
            }
        } else {
            // Update existing profile
            match self.profile_manager.update_profile(&profile) {
                Ok(()) => {
                    *self.status.lock().unwrap() =
                        format!("Profile '{}' updated successfully.", self.profile_name);
                    self.edit_mode = EditMode::None;
                }
                Err(e) => {
                    *self.status.lock().unwrap() = format!("Error updating profile: {}", e);
                }
            }
        }
    }

    fn delete_profile(&mut self) {
        let profile_type = self.selected_profile_type.to_profile_type();

        match self
            .profile_manager
            .delete_profile(&self.selected_profile_name, profile_type)
        {
            Ok(()) => {
                *self.status.lock().unwrap() = format!(
                    "Profile '{}' deleted successfully.",
                    self.selected_profile_name
                );
                self.edit_mode = EditMode::None;

                // Update available profiles
                if let Ok(profiles) = self.profile_manager.list_all_profiles() {
                    self.available_profiles = profiles;
                }

                self.selected_profile_name = String::new();
            }
            Err(e) => {
                *self.status.lock().unwrap() = format!("Error deleting profile: {}", e);
            }
        }
    }

    fn import_profile(&mut self) {
        if self.import_path.is_empty() {
            *self.status.lock().unwrap() =
                "Error: Please select a profile file to import.".to_string();
            return;
        }

        match self
            .profile_manager
            .import_profile(Path::new(&self.import_path))
        {
            Ok(profile) => {
                *self.status.lock().unwrap() =
                    format!("Profile '{}' imported successfully.", profile.name);
                self.edit_mode = EditMode::None;

                // Update available profiles
                if let Ok(profiles) = self.profile_manager.list_all_profiles() {
                    self.available_profiles = profiles;
                }

                // Select the imported profile
                self.selected_profile_type = match profile.profile_type {
                    ProfileType::Clipper => ProfileTypeSelection::Clipper,
                    ProfileType::GifConverter => ProfileTypeSelection::GifConverter,
                    ProfileType::GifTransparency => ProfileTypeSelection::GifTransparency,
                    ProfileType::Splitter => ProfileTypeSelection::Splitter,
                    ProfileType::Merger => ProfileTypeSelection::Merger,
                    ProfileType::Custom(name) => ProfileTypeSelection::Custom(name),
                };

                self.selected_profile_name = profile.name;
            }
            Err(e) => {
                *self.status.lock().unwrap() = format!("Error importing profile: {}", e);
            }
        }
    }

    fn export_profile(&mut self) {
        if self.export_path.is_empty() {
            *self.status.lock().unwrap() =
                "Error: Please select a location to save the profile.".to_string();
            return;
        }

        let profile_type = self.selected_profile_type.to_profile_type();

        match self.profile_manager.export_profile(
            &self.selected_profile_name,
            profile_type,
            Path::new(&self.export_path),
        ) {
            Ok(()) => {
                *self.status.lock().unwrap() = format!(
                    "Profile '{}' exported successfully to {}.",
                    self.selected_profile_name, self.export_path
                );
                self.edit_mode = EditMode::None;
            }
            Err(e) => {
                *self.status.lock().unwrap() = format!("Error exporting profile: {}", e);
            }
        }
    }

    fn build_parameters_map(&self) -> HashMap<String, String> {
        let mut parameters = HashMap::new();

        for (key, value) in &self.profile_parameters {
            if !key.is_empty() {
                parameters.insert(key.clone(), value.clone());
            }
        }

        parameters
    }
}
