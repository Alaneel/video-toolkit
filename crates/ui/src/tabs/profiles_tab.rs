use anyhow::{bail, Context, Result};
use batch_processing::{default_profile_parameters, ProfileBatchConfig};
use eframe::egui::{self, ComboBox, Ui};
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

use profile_system::{Profile, ProfileManager, ProfileType};

pub struct ProfilesTab {
    profile_manager: Option<ProfileManager>,
    initialization_error: Option<String>,

    // UI state
    selected_profile_type: ProfileTypeSelection,
    selected_profile_name: String,
    available_profiles: HashMap<ProfileType, Vec<String>>,

    // Profile creation/editing
    edit_mode: EditMode,
    profile_name: String,
    profile_description: String,
    profile_parameters: Vec<(String, String)>,
    editing_profile: Option<Profile>,
    pending_batch_profile: Option<Profile>,

    // Import/Export
    import_path: String,
    export_path: String,

    // Status
    status: Arc<Mutex<String>>,
    processing: Arc<Mutex<bool>>,
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
    fn from_profile_type(profile_type: &ProfileType) -> Self {
        match profile_type {
            ProfileType::Clipper => Self::Clipper,
            ProfileType::GifConverter => Self::GifConverter,
            ProfileType::GifTransparency => Self::GifTransparency,
            ProfileType::Splitter => Self::Splitter,
            ProfileType::Merger => Self::Merger,
            ProfileType::Custom(name) => Self::Custom(name.clone()),
        }
    }

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
    pub fn new(status: Arc<Mutex<String>>, processing: Arc<Mutex<bool>>) -> Self {
        Self::with_directory(status, processing, None)
    }

    pub fn with_directory(
        status: Arc<Mutex<String>>,
        processing: Arc<Mutex<bool>>,
        directory: Option<&Path>,
    ) -> Self {
        let manager_result = match directory {
            Some(directory) => ProfileManager::with_directory(directory),
            None => ProfileManager::new(),
        };
        let (profile_manager, initialization_error) = match manager_result {
            Ok(manager) => (Some(manager), None),
            Err(error) => {
                let message = format!("Error initializing profile storage: {}", error);
                *status.lock().unwrap() = message.clone();
                (None, Some(message))
            }
        };
        let available_profiles = match profile_manager.as_ref().map(|pm| pm.list_all_profiles()) {
            Some(Ok(profiles)) => profiles,
            Some(Err(error)) => {
                *status.lock().unwrap() = format!("Error listing profiles: {}", error);
                HashMap::new()
            }
            None => HashMap::new(),
        };

        Self {
            profile_manager,
            initialization_error,
            selected_profile_type: ProfileTypeSelection::Clipper,
            selected_profile_name: String::new(),
            available_profiles,
            edit_mode: EditMode::None,
            profile_name: String::new(),
            profile_description: String::new(),
            profile_parameters: Vec::new(),
            editing_profile: None,
            pending_batch_profile: None,
            import_path: String::new(),
            export_path: String::new(),
            status,
            processing,
        }
    }

    pub fn take_batch_profile(&mut self) -> Option<Profile> {
        self.pending_batch_profile.take()
    }

    fn manager(&self) -> Result<&ProfileManager> {
        self.profile_manager
            .as_ref()
            .context("Profile storage is unavailable")
    }

    pub fn ui(&mut self, ui: &mut Ui) {
        ui.heading("Profile Management");

        if let Some(error) = &self.initialization_error {
            ui.colored_label(egui::Color32::LIGHT_RED, error);
            ui.label(
                "Profile actions are unavailable. Check the profile directory and restart the app.",
            );
            return;
        }

        // Profile type selection
        let previous_type = self.selected_profile_type.clone();
        ui.add_enabled_ui(self.edit_mode == EditMode::None, |ui| {
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
                        let mut custom_types: Vec<_> = self
                            .available_profiles
                            .keys()
                            .filter_map(|profile_type| match profile_type {
                                ProfileType::Custom(name) => Some(name.clone()),
                                _ => None,
                            })
                            .collect();
                        custom_types.sort();
                        for name in custom_types {
                            ui.selectable_value(
                                &mut self.selected_profile_type,
                                ProfileTypeSelection::Custom(name.clone()),
                                format!("Custom: {}", name),
                            );
                        }
                    });
            })
        });
        if self.selected_profile_type != previous_type {
            self.selected_profile_name.clear();
        }

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
            match self
                .manager()
                .and_then(|manager| Ok(manager.list_all_profiles()?))
            {
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

                match self.manager().and_then(|manager| {
                    Ok(manager.load_profile(&self.selected_profile_name, profile_type)?)
                }) {
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
                        let mut parameters: Vec<_> = profile.parameters.iter().collect();
                        parameters.sort_by(|a, b| a.0.cmp(b.0));
                        for (key, value) in parameters {
                            ui.horizontal(|ui| {
                                ui.label(format!("{}:", key));
                                ui.label(value);
                            });
                        }
                        ui.separator();
                        let built_in = !matches!(profile.profile_type, ProfileType::Custom(_));
                        let processing = *self.processing.lock().unwrap();
                        if ui
                            .add_enabled(!processing && built_in, egui::Button::new("Use in Batch"))
                            .clicked()
                        {
                            self.pending_batch_profile = Some(profile);
                        }
                        if !built_in {
                            ui.label("Custom profiles can be stored and exported, but cannot run in Batch.");
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
                self.profile_parameters =
                    default_profile_parameters(&self.selected_profile_type.to_profile_type());
                self.editing_profile = None;
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

        match self.manager().and_then(|manager| {
            Ok(manager.load_profile(&self.selected_profile_name, profile_type)?)
        }) {
            Ok(profile) => {
                self.profile_name = profile.name.clone();
                self.profile_description = profile.description.clone().unwrap_or_default();

                // Convert parameters to vector of key-value pairs
                self.profile_parameters = profile.parameters.clone().into_iter().collect();
                self.profile_parameters.sort_by(|a, b| a.0.cmp(&b.0));
                self.editing_profile = Some(profile);

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
        let result = self.profile_from_form().and_then(|profile| {
            self.manager()?.save_profile(&profile)?;
            Ok(profile)
        });
        match result {
            Ok(profile) => {
                *self.status.lock().unwrap() =
                    format!("Profile '{}' created successfully.", profile.name);
                self.edit_mode = EditMode::None;
                self.refresh_profiles();
                self.selected_profile_name = profile.name;
            }
            Err(e) => {
                *self.status.lock().unwrap() = format!("Error creating profile: {}", e);
            }
        }
    }

    fn update_profile(&mut self) {
        let result = self.profile_from_form().and_then(|profile| {
            if profile.name == self.selected_profile_name {
                self.manager()?.update_profile(&profile)?;
            } else {
                self.manager()?
                    .rename_profile(&self.selected_profile_name, &profile)?;
            }
            Ok(profile)
        });
        match result {
            Ok(profile) => {
                *self.status.lock().unwrap() =
                    format!("Profile '{}' updated successfully.", profile.name);
                self.edit_mode = EditMode::None;
                self.editing_profile = None;
                self.refresh_profiles();
                self.selected_profile_name = profile.name;
            }
            Err(e) => {
                *self.status.lock().unwrap() = format!("Error updating profile: {}", e);
            }
        }
    }

    fn profile_from_form(&self) -> Result<Profile> {
        let name = self.profile_name.trim();
        if name.is_empty() {
            bail!("Profile name cannot be empty");
        }
        let parameters = self.build_parameters_map()?;
        let mut profile = match self.editing_profile.as_ref() {
            Some(profile) => profile.clone(),
            None => Profile::new(
                name,
                self.selected_profile_type.to_profile_type(),
                HashMap::new(),
            ),
        };
        profile.name = name.to_owned();
        profile.description = if self.profile_description.trim().is_empty() {
            None
        } else {
            Some(self.profile_description.clone())
        };
        profile.update_parameters(parameters);
        Self::validate_profile(&profile)?;
        Ok(profile)
    }

    fn validate_profile(profile: &Profile) -> Result<()> {
        if !matches!(profile.profile_type, ProfileType::Custom(_)) {
            ProfileBatchConfig::from_profile(profile)?;
        }
        Ok(())
    }

    fn refresh_profiles(&mut self) {
        match self
            .manager()
            .and_then(|manager| Ok(manager.list_all_profiles()?))
        {
            Ok(profiles) => self.available_profiles = profiles,
            Err(error) => {
                *self.status.lock().unwrap() = format!("Error refreshing profiles: {}", error);
            }
        }
    }

    fn delete_profile(&mut self) {
        let profile_type = self.selected_profile_type.to_profile_type();

        match self.manager().and_then(|manager| {
            Ok(manager.delete_profile(&self.selected_profile_name, profile_type)?)
        }) {
            Ok(()) => {
                *self.status.lock().unwrap() = format!(
                    "Profile '{}' deleted successfully.",
                    self.selected_profile_name
                );
                self.edit_mode = EditMode::None;

                // Update available profiles
                self.refresh_profiles();

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

        let result = (|| -> Result<Profile> {
            let profile = ProfileManager::read_profile(Path::new(&self.import_path))?;
            Self::validate_profile(&profile)?;
            self.manager()?.save_profile(&profile)?;
            Ok(profile)
        })();
        match result {
            Ok(profile) => {
                *self.status.lock().unwrap() =
                    format!("Profile '{}' imported successfully.", profile.name);
                self.edit_mode = EditMode::None;

                // Update available profiles
                self.refresh_profiles();

                // Select the imported profile
                self.selected_profile_type =
                    ProfileTypeSelection::from_profile_type(&profile.profile_type);

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

        match self.manager().and_then(|manager| {
            Ok(manager.export_profile(
                &self.selected_profile_name,
                profile_type,
                Path::new(&self.export_path),
            )?)
        }) {
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

    fn build_parameters_map(&self) -> Result<HashMap<String, String>> {
        let mut parameters = HashMap::new();

        for (key, value) in &self.profile_parameters {
            let key = key.trim();
            if key.is_empty() {
                if !value.trim().is_empty() {
                    bail!("A parameter value must have a key");
                }
                continue;
            }
            if parameters.insert(key.to_owned(), value.clone()).is_some() {
                bail!("Duplicate parameter key '{}'", key);
            }
        }

        Ok(parameters)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            static NEXT_ID: AtomicUsize = AtomicUsize::new(0);
            let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
            Self(std::env::temp_dir().join(format!(
                "video-toolkit-ui-profiles-{}-{}-{}",
                std::process::id(),
                chrono::Utc::now().timestamp_micros(),
                id
            )))
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn tab() -> ProfilesTab {
        ProfilesTab {
            profile_manager: None,
            initialization_error: None,
            selected_profile_type: ProfileTypeSelection::Clipper,
            selected_profile_name: String::new(),
            available_profiles: HashMap::new(),
            edit_mode: EditMode::None,
            profile_name: "new profile".into(),
            profile_description: String::new(),
            profile_parameters: default_profile_parameters(&ProfileType::Clipper),
            editing_profile: None,
            pending_batch_profile: None,
            import_path: String::new(),
            export_path: String::new(),
            status: Arc::new(Mutex::new("Ready".into())),
            processing: Arc::new(Mutex::new(false)),
        }
    }

    #[test]
    fn parameter_form_rejects_duplicates_and_values_without_keys() {
        let mut tab = tab();
        tab.profile_parameters = vec![("fps".into(), "10".into()), (" fps ".into(), "20".into())];
        assert!(tab
            .build_parameters_map()
            .unwrap_err()
            .to_string()
            .contains("Duplicate"));
        tab.profile_parameters = vec![(" ".into(), "20".into())];
        assert!(tab.build_parameters_map().is_err());
        tab.profile_parameters = vec![(" ".into(), " ".into()), ("width".into(), String::new())];
        let parameters = tab.build_parameters_map().unwrap();
        assert_eq!(parameters.len(), 1);
        assert_eq!(parameters["width"], "");
    }

    #[test]
    fn editing_preserves_creation_and_type_and_updates_metadata() {
        let mut tab = tab();
        let mut original = Profile::new("original", ProfileType::Clipper, HashMap::new());
        original.created = chrono::Utc::now() - chrono::Duration::days(10);
        original.last_modified = original.created;
        tab.editing_profile = Some(original.clone());
        tab.profile_name = "renamed".into();
        tab.profile_description = "updated description".into();
        // The form's type selector is disabled while editing; preserving the
        // stored type here also prevents accidental changes by other callers.
        tab.selected_profile_type = ProfileTypeSelection::GifConverter;
        let updated = tab.profile_from_form().unwrap();
        assert_eq!(updated.name, "renamed");
        assert_eq!(updated.profile_type, ProfileType::Clipper);
        assert_eq!(updated.created, original.created);
        assert!(updated.last_modified > original.last_modified);
        assert_eq!(updated.description.as_deref(), Some("updated description"));
        assert!(updated.parameters.contains_key("ranges"));
    }

    #[test]
    fn built_in_profiles_are_validated_before_storage_but_custom_profiles_are_allowed() {
        let mut tab = tab();
        tab.profile_parameters = vec![("ranges".into(), "invalid range".into())];
        assert!(tab.profile_from_form().is_err());
        tab.selected_profile_type = ProfileTypeSelection::Custom("extension".into());
        assert!(tab.profile_from_form().is_ok());
    }

    #[test]
    fn storage_initialization_failure_is_visible_without_fallback() {
        let status = Arc::new(Mutex::new("Ready".to_string()));
        let file_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
        let tab = ProfilesTab::with_directory(
            Arc::clone(&status),
            Arc::new(Mutex::new(false)),
            Some(&file_path),
        );
        assert!(tab.profile_manager.is_none());
        let error = tab.initialization_error.as_ref().unwrap();
        assert!(error.starts_with("Error initializing profile storage:"));
        assert_eq!(*status.lock().unwrap(), *error);
        assert!(tab.available_profiles.is_empty());
    }

    #[test]
    fn custom_directory_loads_its_existing_profiles() {
        let directory = TestDirectory::new();
        let profile = Profile::new(
            "custom-directory-preset",
            ProfileType::Clipper,
            default_profile_parameters(&ProfileType::Clipper)
                .into_iter()
                .collect(),
        );
        ProfileManager::with_directory(&directory.0)
            .unwrap()
            .save_profile(&profile)
            .unwrap();
        let tab = ProfilesTab::with_directory(
            Arc::new(Mutex::new("Ready".into())),
            Arc::new(Mutex::new(false)),
            Some(&directory.0),
        );
        assert!(tab.initialization_error.is_none());
        assert_eq!(
            tab.available_profiles.get(&ProfileType::Clipper).unwrap(),
            &["custom-directory-preset".to_string()]
        );
        let loaded = tab
            .manager()
            .unwrap()
            .load_profile(&profile.name, profile.profile_type.clone())
            .unwrap();
        assert_eq!(loaded.name, profile.name);
        assert_eq!(loaded.parameters, profile.parameters);
        assert_eq!(loaded.created, profile.created);
        assert_eq!(loaded.last_modified, profile.last_modified);
    }

    #[test]
    fn importing_invalid_builtin_profile_does_not_save_it() {
        let directory = TestDirectory::new();
        let source_manager = ProfileManager::with_directory(directory.0.join("source")).unwrap();
        let invalid = Profile::new(
            "invalid",
            ProfileType::GifConverter,
            HashMap::from([("fps".to_string(), "0".to_string())]),
        );
        source_manager.save_profile(&invalid).unwrap();
        let import_file = directory.0.join("invalid.json");
        source_manager
            .export_profile("invalid", ProfileType::GifConverter, &import_file)
            .unwrap();
        let mut tab = tab();
        tab.profile_manager =
            Some(ProfileManager::with_directory(directory.0.join("destination")).unwrap());
        tab.import_path = import_file.to_string_lossy().into_owned();
        tab.edit_mode = EditMode::Import;
        tab.import_profile();
        assert!(tab.status.lock().unwrap().contains("fps"));
        assert!(matches!(tab.edit_mode, EditMode::Import));
        assert!(tab
            .manager()
            .unwrap()
            .list_all_profiles()
            .unwrap()
            .values()
            .all(Vec::is_empty));
    }

    #[test]
    fn rename_conflict_from_form_preserves_original_profile() {
        let directory = TestDirectory::new();
        let mut tab = tab();
        tab.profile_manager = Some(ProfileManager::with_directory(&directory.0).unwrap());
        let mut original = tab.profile_from_form().unwrap();
        original.name = "original".into();
        let mut destination = original.clone();
        destination.name = "destination".into();
        tab.manager().unwrap().save_profile(&original).unwrap();
        tab.manager().unwrap().save_profile(&destination).unwrap();
        tab.selected_profile_name = "original".into();
        tab.load_profile_for_editing();
        tab.profile_name = "destination".into();
        tab.edit_mode = EditMode::Edit;
        tab.update_profile();
        assert!(tab.status.lock().unwrap().contains("already exists"));
        assert!(matches!(tab.edit_mode, EditMode::Edit));
        assert_eq!(tab.selected_profile_name, "original");
        let stored = tab
            .manager()
            .unwrap()
            .load_profile("original", ProfileType::Clipper)
            .unwrap();
        assert_eq!(stored.created, original.created);
        assert_eq!(stored.parameters, original.parameters);
        assert_eq!(
            tab.manager()
                .unwrap()
                .list_profiles(ProfileType::Clipper)
                .unwrap(),
            vec!["destination", "original"]
        );
    }
}
