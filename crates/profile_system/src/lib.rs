use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Errors that can occur in the profile system
#[derive(Error, Debug)]
pub enum ProfileError {
    #[error("Failed to read profile: {0}")]
    ReadError(#[from] std::io::Error),

    #[error("Failed to parse profile: {0}")]
    ParseError(String),

    #[error("Profile '{0}' not found")]
    NotFound(String),

    #[error("Profile '{0}' already exists")]
    AlreadyExists(String),

    #[error("Invalid profile data: {0}")]
    InvalidData(String),

    #[error("Failed to create profile directory")]
    DirectoryCreationFailed,
}

/// Result type for profile operations
pub type Result<T> = std::result::Result<T, ProfileError>;

static NEXT_TEMPORARY_PROFILE: AtomicU64 = AtomicU64::new(0);

/// Profile names are used unchanged on every supported operating system.
fn validate_filename_component(label: &str, value: &str) -> Result<()> {
    let invalid = |reason: &str| ProfileError::InvalidData(format!("{label} '{value}' {reason}"));

    if value.is_empty() || value.trim() != value {
        return Err(invalid(
            "must be non-empty without leading or trailing whitespace",
        ));
    }
    if value == "." || value == ".." || value.ends_with('.') {
        return Err(invalid("cannot be '.', '..', or end with a dot"));
    }
    if value
        .chars()
        .any(|character| character.is_control() || "<>:\"/\\|?*".contains(character))
    {
        return Err(invalid(
            "contains a path separator, control character, or forbidden filename character",
        ));
    }
    let mut components = Path::new(value).components();
    if !matches!(components.next(), Some(Component::Normal(_))) || components.next().is_some() {
        return Err(invalid("must be a single filename component"));
    }

    // Windows reserves device names even when followed by an extension.
    let stem = value
        .split('.')
        .next()
        .unwrap_or_default()
        .trim_end_matches(' ')
        .to_ascii_uppercase();
    let numbered_device = stem.len() == 4
        && (stem.starts_with("COM") || stem.starts_with("LPT"))
        && matches!(stem.as_bytes()[3], b'1'..=b'9');
    if matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL") || numbered_device {
        return Err(invalid("is a reserved Windows device name"));
    }

    Ok(())
}

fn validate_profile_identity(profile: &Profile) -> Result<()> {
    validate_filename_component("Profile name", &profile.name)?;
    if let ProfileType::Custom(custom) = &profile.profile_type {
        validate_filename_component("Custom profile type", custom)?;
    }
    Ok(())
}

/// Removes an incomplete temporary file on every error path.
struct TemporaryProfileFile(PathBuf);

impl Drop for TemporaryProfileFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

/// Profile types supported by the system
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ProfileType {
    Clipper,
    GifConverter,
    GifTransparency,
    Splitter,
    Merger,
    Custom(String),
}

impl std::fmt::Display for ProfileType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProfileType::Clipper => write!(f, "Clipper"),
            ProfileType::GifConverter => write!(f, "GIF Converter"),
            ProfileType::GifTransparency => write!(f, "GIF Transparency"),
            ProfileType::Splitter => write!(f, "Splitter"),
            ProfileType::Merger => write!(f, "Merger"),
            ProfileType::Custom(name) => write!(f, "Custom: {}", name),
        }
    }
}

/// A profile containing parameters for a specific operation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Profile {
    pub name: String,
    pub description: Option<String>,
    pub profile_type: ProfileType,
    pub parameters: HashMap<String, String>,
    pub created: chrono::DateTime<chrono::Utc>,
    pub last_modified: chrono::DateTime<chrono::Utc>,
}

impl Profile {
    /// Create a new profile
    pub fn new(name: &str, profile_type: ProfileType, parameters: HashMap<String, String>) -> Self {
        let now = chrono::Utc::now();
        Self {
            name: name.to_string(),
            description: None,
            profile_type,
            parameters,
            created: now,
            last_modified: now,
        }
    }

    /// Set the description of the profile
    pub fn with_description(mut self, description: &str) -> Self {
        self.description = Some(description.to_string());
        self
    }

    /// Update the parameters of the profile
    pub fn update_parameters(&mut self, parameters: HashMap<String, String>) {
        self.parameters = parameters;
        self.last_modified = chrono::Utc::now();
    }

    /// Add or update a single parameter
    pub fn set_parameter(&mut self, key: &str, value: &str) {
        self.parameters.insert(key.to_string(), value.to_string());
        self.last_modified = chrono::Utc::now();
    }

    /// Get a parameter value by key
    pub fn get_parameter(&self, key: &str) -> Option<&String> {
        self.parameters.get(key)
    }
}

/// Manages profile storage and retrieval
pub struct ProfileManager {
    profiles_dir: PathBuf,
}

impl ProfileManager {
    /// Create a new profile manager
    pub fn new() -> Result<Self> {
        // Get the project directories
        let project_dirs = ProjectDirs::from("com", "video-toolkit", "VideoToolKit")
            .ok_or(ProfileError::DirectoryCreationFailed)?;

        // Create the config directory if it doesn't exist
        let profiles_dir = project_dirs.config_dir().join("profiles");
        fs::create_dir_all(&profiles_dir).map_err(|_| ProfileError::DirectoryCreationFailed)?;

        Ok(Self { profiles_dir })
    }

    /// Create a profile manager with a custom directory
    pub fn with_directory<P: AsRef<Path>>(dir: P) -> Result<Self> {
        let profiles_dir = dir.as_ref().to_path_buf();
        fs::create_dir_all(&profiles_dir).map_err(|_| ProfileError::DirectoryCreationFailed)?;

        Ok(Self { profiles_dir })
    }

    fn get_type_directory(&self, profile_type: &ProfileType) -> Result<PathBuf> {
        let directory = match profile_type {
            ProfileType::Clipper => self.profiles_dir.join("clipper"),
            ProfileType::GifConverter => self.profiles_dir.join("gifconverter"),
            ProfileType::GifTransparency => self.profiles_dir.join("giftransparency"),
            ProfileType::Splitter => self.profiles_dir.join("splitter"),
            ProfileType::Merger => self.profiles_dir.join("merger"),
            ProfileType::Custom(custom) => {
                validate_filename_component("Custom profile type", custom)?;
                self.profiles_dir.join("custom").join(custom)
            }
        };
        Ok(directory)
    }

    /// Calculate a validated path without creating directories or files.
    fn get_profile_path(&self, name: &str, profile_type: ProfileType) -> Result<PathBuf> {
        validate_filename_component("Profile name", name)?;
        Ok(self
            .get_type_directory(&profile_type)?
            .join(format!("{}.json", name)))
    }

    /// Save a profile
    pub fn save_profile(&self, profile: &Profile) -> Result<()> {
        let profile_path = self.get_profile_path(&profile.name, profile.profile_type.clone())?;

        // Serialize before touching storage so invalid data cannot truncate a file.
        let json = serde_json::to_string_pretty(profile)
            .map_err(|e| ProfileError::ParseError(e.to_string()))?;

        // Create parent directories if they don't exist
        if let Some(parent) = profile_path.parent() {
            fs::create_dir_all(parent).map_err(ProfileError::ReadError)?;
        }

        // create_new rejects duplicates atomically, including competing saves.
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&profile_path)
            .map_err(|error| {
                if error.kind() == std::io::ErrorKind::AlreadyExists {
                    ProfileError::AlreadyExists(profile.name.clone())
                } else {
                    ProfileError::ReadError(error)
                }
            })?;

        if let Err(error) = file
            .write_all(json.as_bytes())
            .and_then(|()| file.sync_all())
        {
            drop(file);
            let _ = fs::remove_file(profile_path);
            return Err(ProfileError::ReadError(error));
        }

        Ok(())
    }

    /// Update an existing profile
    pub fn update_profile(&self, profile: &Profile) -> Result<()> {
        let profile_path = self.get_profile_path(&profile.name, profile.profile_type.clone())?;
        let original = self.load_profile(&profile.name, profile.profile_type.clone())?;
        let mut updated = profile.clone();
        updated.created = original.created;
        updated.last_modified = chrono::Utc::now();
        let json = serde_json::to_string_pretty(&updated)
            .map_err(|e| ProfileError::ParseError(e.to_string()))?;

        // Write beside the destination, then replace it in a single rename.
        // The old JSON stays readable if opening or writing the temporary file fails.
        let parent = profile_path.parent().ok_or_else(|| {
            ProfileError::InvalidData("Profile path has no parent directory".to_string())
        })?;
        let (temporary, mut file) = loop {
            let sequence = NEXT_TEMPORARY_PROFILE.fetch_add(1, Ordering::Relaxed);
            let path = parent.join(format!(".profile-{}-{sequence}.tmp", std::process::id()));
            match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(file) => break (TemporaryProfileFile(path), file),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(ProfileError::ReadError(error)),
            }
        };
        file.write_all(json.as_bytes())
            .map_err(ProfileError::ReadError)?;
        file.sync_all().map_err(ProfileError::ReadError)?;
        drop(file);
        fs::rename(&temporary.0, profile_path).map_err(ProfileError::ReadError)?;

        Ok(())
    }

    /// Load a profile by name and type
    pub fn load_profile(&self, name: &str, profile_type: ProfileType) -> Result<Profile> {
        let profile_path = self.get_profile_path(name, profile_type.clone())?;
        let profile = Self::read_profile(profile_path).map_err(|error| match error {
            ProfileError::ReadError(error) if error.kind() == std::io::ErrorKind::NotFound => {
                ProfileError::NotFound(name.to_string())
            }
            other => other,
        })?;
        if profile.name != name || profile.profile_type != profile_type {
            return Err(ProfileError::InvalidData(format!(
                "Profile file identity '{}' ({}) does not match requested '{}' ({})",
                profile.name, profile.profile_type, name, profile_type
            )));
        }
        Ok(profile)
    }

    /// Rename a profile without deleting the original until the new file is saved.
    /// The operation retains the original creation time and refuses to overwrite
    /// an existing profile with the destination name.
    pub fn rename_profile(&self, old_name: &str, updated: &Profile) -> Result<()> {
        // Validate both identities before reading or writing anything.
        self.get_profile_path(&updated.name, updated.profile_type.clone())?;
        let original = self.load_profile(old_name, updated.profile_type.clone())?;
        let mut renamed = updated.clone();
        renamed.created = original.created;
        renamed.last_modified = chrono::Utc::now();

        if old_name == renamed.name {
            return self.update_profile(&renamed);
        }

        self.save_profile(&renamed)?;
        if let Err(error) = self.delete_profile(old_name, renamed.profile_type.clone()) {
            // Only roll back when the original remains readable. If another
            // process removed it, retain the new file so the profile is not lost.
            let recovery = if self
                .load_profile(old_name, renamed.profile_type.clone())
                .is_ok()
            {
                match self.delete_profile(&renamed.name, renamed.profile_type.clone()) {
                    Ok(()) => "The new file was rolled back; the original is intact.".to_string(),
                    Err(rollback_error) => format!(
                        "Both files have been retained because rollback failed: {rollback_error}."
                    ),
                }
            } else {
                format!(
                    "The new profile '{}' has been retained for recovery.",
                    renamed.name
                )
            };
            return Err(ProfileError::ReadError(std::io::Error::other(format!(
                "Could not finish renaming profile '{old_name}': {error}. {recovery}"
            ))));
        }
        Ok(())
    }

    /// Delete a profile
    pub fn delete_profile(&self, name: &str, profile_type: ProfileType) -> Result<()> {
        let profile_path = self.get_profile_path(name, profile_type)?;

        fs::remove_file(profile_path).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                ProfileError::NotFound(name.to_string())
            } else {
                ProfileError::ReadError(error)
            }
        })?;

        Ok(())
    }

    /// List all profiles of a specific type
    pub fn list_profiles(&self, profile_type: ProfileType) -> Result<Vec<String>> {
        let type_dir = self.get_type_directory(&profile_type)?;
        let entries = match fs::read_dir(type_dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(ProfileError::ReadError(error)),
        };

        // Filter for JSON files and extract the profile names
        let mut profiles = Vec::new();
        for entry in entries {
            let entry = entry.map_err(ProfileError::ReadError)?;
            let path = entry.path();

            if !entry
                .file_type()
                .map_err(ProfileError::ReadError)?
                .is_file()
            {
                continue;
            }

            if let Some(extension) = path.extension() {
                if extension == "json" {
                    if let Some(stem) = path.file_stem() {
                        if let Some(name) = stem.to_str() {
                            validate_filename_component("Profile name", name)?;
                            self.load_profile(name, profile_type.clone())?;
                            profiles.push(name.to_string());
                        } else {
                            return Err(ProfileError::InvalidData(
                                "Profile filename is not valid UTF-8".to_string(),
                            ));
                        }
                    }
                }
            }
        }

        profiles.sort_unstable();
        Ok(profiles)
    }

    /// List all profiles of all types
    pub fn list_all_profiles(&self) -> Result<HashMap<ProfileType, Vec<String>>> {
        let mut result = HashMap::new();

        // List profiles for built-in types
        result.insert(
            ProfileType::Clipper,
            self.list_profiles(ProfileType::Clipper)?,
        );
        result.insert(
            ProfileType::GifConverter,
            self.list_profiles(ProfileType::GifConverter)?,
        );
        result.insert(
            ProfileType::GifTransparency,
            self.list_profiles(ProfileType::GifTransparency)?,
        );
        result.insert(
            ProfileType::Splitter,
            self.list_profiles(ProfileType::Splitter)?,
        );
        result.insert(
            ProfileType::Merger,
            self.list_profiles(ProfileType::Merger)?,
        );

        // List custom profiles
        let custom_dir = self.profiles_dir.join("custom");
        if custom_dir.exists() {
            let entries = fs::read_dir(custom_dir).map_err(ProfileError::ReadError)?;

            for entry in entries {
                let entry = entry.map_err(ProfileError::ReadError)?;
                let path = entry.path();

                if entry.file_type().map_err(ProfileError::ReadError)?.is_dir() {
                    let custom_type =
                        path.file_name()
                            .and_then(|name| name.to_str())
                            .ok_or_else(|| {
                                ProfileError::InvalidData(
                                    "Custom profile type directory is not valid UTF-8".to_string(),
                                )
                            })?;
                    let custom_profile_type = ProfileType::Custom(custom_type.to_string());
                    let profiles = self.list_profiles(custom_profile_type.clone())?;
                    if !profiles.is_empty() {
                        result.insert(custom_profile_type, profiles);
                    }
                }
            }
        }

        Ok(result)
    }

    /// Read and validate a portable profile without writing to profile storage.
    /// Callers can validate operation parameters before choosing to save it.
    pub fn read_profile<P: AsRef<Path>>(path: P) -> Result<Profile> {
        let mut file = File::open(path).map_err(ProfileError::ReadError)?;
        let mut contents = String::new();
        file.read_to_string(&mut contents)
            .map_err(ProfileError::ReadError)?;
        let profile: Profile =
            serde_json::from_str(&contents).map_err(|e| ProfileError::ParseError(e.to_string()))?;
        validate_profile_identity(&profile)?;
        Ok(profile)
    }

    /// Import a profile from a file
    pub fn import_profile<P: AsRef<Path>>(&self, path: P) -> Result<Profile> {
        let profile = Self::read_profile(path)?;
        self.save_profile(&profile)?;
        Ok(profile)
    }

    /// Export a profile to a file
    pub fn export_profile<P: AsRef<Path>>(
        &self,
        name: &str,
        profile_type: ProfileType,
        path: P,
    ) -> Result<()> {
        // Load the profile
        let profile = self.load_profile(name, profile_type)?;

        // Serialize the profile
        let json = serde_json::to_string_pretty(&profile)
            .map_err(|e| ProfileError::ParseError(e.to_string()))?;

        // Write to the file
        let mut file = File::create(path).map_err(ProfileError::ReadError)?;

        file.write_all(json.as_bytes())
            .map_err(ProfileError::ReadError)?;

        Ok(())
    }
}

// Test module
#[cfg(test)]
mod tests {
    use super::*;
    use std::env;
    use std::sync::atomic::AtomicBool;
    use std::sync::{Arc, Barrier};
    use std::thread;

    fn create_test_profile_manager(test_name: &str) -> ProfileManager {
        let temp_dir = env::temp_dir()
            .join("video_toolkit_test_profiles")
            .join(format!("{}_{test_name}", std::process::id()));
        let _ = fs::remove_dir_all(&temp_dir); // Clean up from previous tests
        ProfileManager::with_directory(temp_dir).unwrap()
    }

    #[test]
    fn test_save_and_load_profile() {
        let manager = create_test_profile_manager("save_and_load");

        let mut params = HashMap::new();
        params.insert("width".to_string(), "480".to_string());
        params.insert("fps".to_string(), "15".to_string());

        let profile = Profile::new("test_profile", ProfileType::GifConverter, params)
            .with_description("Test profile for GIF conversion");

        // Save the profile
        manager.save_profile(&profile).unwrap();

        // Load the profile
        let loaded = manager
            .load_profile("test_profile", ProfileType::GifConverter)
            .unwrap();

        assert_eq!(loaded.name, "test_profile");
        assert_eq!(loaded.profile_type, ProfileType::GifConverter);
        assert_eq!(loaded.get_parameter("width").unwrap(), "480");
        assert_eq!(loaded.get_parameter("fps").unwrap(), "15");
    }

    #[test]
    fn test_list_profiles() {
        let manager = create_test_profile_manager("list");

        // Create test profiles
        let profile1 = Profile::new("profile1", ProfileType::Clipper, HashMap::new());
        let profile2 = Profile::new("profile2", ProfileType::Clipper, HashMap::new());

        manager.save_profile(&profile1).unwrap();
        manager.save_profile(&profile2).unwrap();

        // List profiles
        let profiles = manager.list_profiles(ProfileType::Clipper).unwrap();

        assert_eq!(profiles.len(), 2);
        assert!(profiles.contains(&"profile1".to_string()));
        assert!(profiles.contains(&"profile2".to_string()));
    }

    #[test]
    fn test_delete_profile() {
        let manager = create_test_profile_manager("delete");

        let profile = Profile::new("delete_me", ProfileType::Merger, HashMap::new());

        // Save the profile
        manager.save_profile(&profile).unwrap();

        // Verify it exists
        assert!(manager
            .load_profile("delete_me", ProfileType::Merger)
            .is_ok());

        // Delete the profile
        manager
            .delete_profile("delete_me", ProfileType::Merger)
            .unwrap();

        // Verify it's gone
        assert!(manager
            .load_profile("delete_me", ProfileType::Merger)
            .is_err());
    }

    fn assert_invalid_data<T: std::fmt::Debug>(result: Result<T>) {
        assert!(
            matches!(result, Err(ProfileError::InvalidData(_))),
            "{result:?}"
        );
    }

    #[test]
    fn invalid_names_are_rejected_by_every_storage_operation() {
        let manager = create_test_profile_manager("invalid_names");
        let invalid_names = [
            "",
            " ",
            " leading",
            "trailing ",
            "\u{a0}name",
            "name\u{a0}",
            ".",
            "..",
            "../escape",
            "/escape",
            "C:\\escape",
            "\\\\server\\share",
            "nested/name",
            "name\\child",
            "a<b",
            "a>b",
            "a:b",
            "a\"b",
            "a|b",
            "a?b",
            "a*b",
            "a\nb",
            "a\0b",
            "name.",
            "CON",
            "con.txt",
            "PRN",
            "aux",
            "NUL.json",
            "COM1",
            "com9.backup",
            "LPT1",
            "lpt9.log",
            "con .txt",
        ];
        for name in invalid_names {
            let profile = Profile::new(name, ProfileType::Clipper, HashMap::new());
            assert_invalid_data(manager.save_profile(&profile));
            assert_invalid_data(manager.update_profile(&profile));
            assert_invalid_data(manager.load_profile(name, ProfileType::Clipper));
            assert_invalid_data(manager.delete_profile(name, ProfileType::Clipper));
        }
        assert!(!manager.profiles_dir.join("clipper").exists());
    }

    #[test]
    fn invalid_custom_types_cannot_escape_storage_or_create_directories() {
        let fixture = create_test_profile_manager("invalid_custom_types");
        let manager = ProfileManager::with_directory(fixture.profiles_dir.join("storage")).unwrap();
        let outside = fixture.profiles_dir.join("outside.json");
        fs::write(&outside, b"keep this file").unwrap();
        let absolute = fixture.profiles_dir.join("absolute");
        let types = [
            "".to_string(),
            "..".to_string(),
            "../..".to_string(),
            "../../outside".to_string(),
            "child/type".to_string(),
            "child\\type".to_string(),
            "CON".to_string(),
            "LPT2.json".to_string(),
            "type.".to_string(),
            " type".to_string(),
            absolute.to_string_lossy().into_owned(),
        ];
        for custom in types {
            let profile_type = ProfileType::Custom(custom);
            let profile = Profile::new("safe", profile_type.clone(), HashMap::new());
            assert_invalid_data(manager.save_profile(&profile));
            assert_invalid_data(manager.update_profile(&profile));
            assert_invalid_data(manager.load_profile("safe", profile_type.clone()));
            assert_invalid_data(manager.delete_profile("safe", profile_type.clone()));
            assert_invalid_data(manager.list_profiles(profile_type));
        }
        assert_eq!(fs::read(&outside).unwrap(), b"keep this file");
        assert!(!manager.profiles_dir.join("custom").exists());
        assert!(!absolute.exists());
    }

    #[test]
    fn imported_traversal_and_absolute_names_do_not_write_outside_storage() {
        let fixture = create_test_profile_manager("invalid_import");
        let manager = ProfileManager::with_directory(fixture.profiles_dir.join("storage")).unwrap();
        let outside = fixture.profiles_dir.join("outside.json");
        fs::write(&outside, b"keep this file").unwrap();
        let input = fixture.profiles_dir.join("import.json");
        let absolute = fixture.profiles_dir.join("absolute");
        for name in [
            "../../outside".to_string(),
            absolute.to_string_lossy().into_owned(),
        ] {
            let profile = Profile::new(&name, ProfileType::Clipper, HashMap::new());
            fs::write(&input, serde_json::to_vec(&profile).unwrap()).unwrap();
            assert_invalid_data(ProfileManager::read_profile(&input));
            assert_invalid_data(manager.import_profile(&input));
            assert_eq!(fs::read(&outside).unwrap(), b"keep this file");
        }
        assert!(!manager.profiles_dir.join("clipper").exists());
        assert!(!absolute.with_extension("json").exists());
    }

    #[test]
    fn reading_an_import_does_not_create_profile_storage() {
        let fixture = create_test_profile_manager("read_import");
        let input = fixture.profiles_dir.join("import.json");
        let profile = Profile::new("My clip 你好", ProfileType::Clipper, HashMap::new());
        fs::write(&input, serde_json::to_vec(&profile).unwrap()).unwrap();
        let read = ProfileManager::read_profile(&input).unwrap();
        assert_eq!(read.name, profile.name);
        assert!(!fixture.profiles_dir.join("clipper").exists());
        fixture.save_profile(&read).unwrap();
        assert_eq!(
            fixture
                .load_profile(&read.name, ProfileType::Clipper)
                .unwrap()
                .name,
            read.name
        );
    }

    #[test]
    fn missing_reads_and_deletes_do_not_create_type_directories() {
        let manager = create_test_profile_manager("missing_read");
        assert!(matches!(
            manager.load_profile("missing", ProfileType::GifConverter),
            Err(ProfileError::NotFound(_))
        ));
        assert!(matches!(
            manager.delete_profile("missing", ProfileType::GifConverter),
            Err(ProfileError::NotFound(_))
        ));
        assert!(manager
            .list_profiles(ProfileType::GifConverter)
            .unwrap()
            .is_empty());
        assert!(!manager.profiles_dir.join("gifconverter").exists());
        assert!(matches!(
            manager.load_profile("missing", ProfileType::Custom("custom-type".to_string())),
            Err(ProfileError::NotFound(_))
        ));
        assert!(!manager.profiles_dir.join("custom").exists());
    }

    #[test]
    fn load_rejects_mismatched_metadata_and_update_preserves_the_file() {
        let manager = create_test_profile_manager("metadata_mismatch");
        let path = manager
            .get_profile_path("requested", ProfileType::Clipper)
            .unwrap();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        for profile in [
            Profile::new("different", ProfileType::Clipper, HashMap::new()),
            Profile::new("requested", ProfileType::GifConverter, HashMap::new()),
        ] {
            let contents = serde_json::to_vec(&profile).unwrap();
            fs::write(&path, &contents).unwrap();
            assert_invalid_data(manager.load_profile("requested", ProfileType::Clipper));
            assert_invalid_data(manager.list_profiles(ProfileType::Clipper));
            assert_invalid_data(manager.update_profile(&Profile::new(
                "requested",
                ProfileType::Clipper,
                HashMap::new(),
            )));
            assert_eq!(fs::read(&path).unwrap(), contents);
        }
    }

    #[test]
    fn concurrent_saves_never_replace_an_existing_profile() {
        let manager = Arc::new(create_test_profile_manager("save_race"));
        let barrier = Arc::new(Barrier::new(8));
        let threads: Vec<_> = (0..8)
            .map(|index| {
                let manager = Arc::clone(&manager);
                let barrier = Arc::clone(&barrier);
                thread::spawn(move || {
                    let profile = Profile::new("same", ProfileType::Clipper, HashMap::new())
                        .with_description(&format!("writer {index}"));
                    barrier.wait();
                    manager.save_profile(&profile)
                })
            })
            .collect();
        let mut saved = 0;
        for thread in threads {
            match thread.join().unwrap() {
                Ok(()) => saved += 1,
                Err(ProfileError::AlreadyExists(_)) => {}
                error => panic!("unexpected concurrent save result: {error:?}"),
            }
        }
        assert_eq!(saved, 1);
        manager.load_profile("same", ProfileType::Clipper).unwrap();
    }

    #[test]
    fn rename_collision_preserves_both_profiles() {
        let manager = create_test_profile_manager("rename_collision");
        let original = Profile::new("original", ProfileType::Clipper, HashMap::new())
            .with_description("original data");
        let target = Profile::new("target", ProfileType::Clipper, HashMap::new())
            .with_description("existing target data");
        manager.save_profile(&original).unwrap();
        manager.save_profile(&target).unwrap();
        let updated = Profile::new("target", ProfileType::Clipper, HashMap::new())
            .with_description("new data");
        assert!(matches!(
            manager.rename_profile("original", &updated),
            Err(ProfileError::AlreadyExists(_))
        ));
        assert_eq!(
            manager
                .load_profile("original", ProfileType::Clipper)
                .unwrap()
                .description,
            original.description
        );
        assert_eq!(
            manager
                .load_profile("target", ProfileType::Clipper)
                .unwrap()
                .description,
            target.description
        );
    }

    #[test]
    fn rename_retains_creation_time_and_applies_updated_settings() {
        let manager = create_test_profile_manager("rename_success");
        let mut original = Profile::new("original", ProfileType::Clipper, HashMap::new());
        original.created = chrono::Utc::now() - chrono::Duration::days(1);
        original.last_modified = original.created;
        manager.save_profile(&original).unwrap();
        let mut updated = Profile::new("renamed", ProfileType::Clipper, HashMap::new())
            .with_description("updated description");
        updated.set_parameter("ranges", "00:00-00:01");
        manager.rename_profile("original", &updated).unwrap();
        assert!(matches!(
            manager.load_profile("original", ProfileType::Clipper),
            Err(ProfileError::NotFound(_))
        ));
        let renamed = manager
            .load_profile("renamed", ProfileType::Clipper)
            .unwrap();
        assert_eq!(renamed.created, original.created);
        assert!(renamed.last_modified > original.last_modified);
        assert_eq!(renamed.parameters, updated.parameters);
        assert_eq!(renamed.description, updated.description);
    }

    #[test]
    fn failed_rename_validation_keeps_the_original() {
        let manager = create_test_profile_manager("invalid_rename");
        let original = Profile::new("original", ProfileType::Clipper, HashMap::new());
        manager.save_profile(&original).unwrap();
        let updated = Profile::new("../escaped", ProfileType::Clipper, HashMap::new());
        assert_invalid_data(manager.rename_profile("original", &updated));
        assert_eq!(
            manager
                .load_profile("original", ProfileType::Clipper)
                .unwrap()
                .created,
            original.created
        );
    }

    #[test]
    fn atomic_updates_preserve_identity_and_never_publish_partial_json() {
        let manager = Arc::new(create_test_profile_manager("atomic_update"));
        let mut original = Profile::new("updatable", ProfileType::GifConverter, HashMap::new());
        original.created = chrono::Utc::now() - chrono::Duration::days(1);
        original.last_modified = original.created;
        manager.save_profile(&original).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let barrier = Arc::new(Barrier::new(2));
        let reader = {
            let manager = Arc::clone(&manager);
            let stop = Arc::clone(&stop);
            let barrier = Arc::clone(&barrier);
            thread::spawn(move || {
                barrier.wait();
                let mut reads = 0;
                while !stop.load(Ordering::Acquire) {
                    manager
                        .load_profile("updatable", ProfileType::GifConverter)
                        .unwrap();
                    reads += 1;
                }
                reads
            })
        };
        barrier.wait();
        for index in 0..12 {
            let mut updated = Profile::new("updatable", ProfileType::GifConverter, HashMap::new());
            updated.set_parameter("payload", &format!("{index}:{}", "x".repeat(64 * 1024)));
            manager.update_profile(&updated).unwrap();
        }
        stop.store(true, Ordering::Release);
        assert!(reader.join().unwrap() > 0);
        let loaded = manager
            .load_profile("updatable", ProfileType::GifConverter)
            .unwrap();
        assert_eq!(loaded.created, original.created);
        assert!(loaded.last_modified > original.last_modified);
        assert!(loaded.get_parameter("payload").unwrap().starts_with("11:"));
        let directory = manager.profiles_dir.join("gifconverter");
        assert_eq!(
            fs::read_dir(directory).unwrap().count(),
            1,
            "temporary files must be removed"
        );
    }
}
