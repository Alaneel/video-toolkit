use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

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

    /// Get the path to the profile file
    fn get_profile_path(&self, name: &str, profile_type: ProfileType) -> PathBuf {
        let type_dir = match profile_type {
            ProfileType::Custom(ref custom) => self.profiles_dir.join("custom").join(custom),
            _ => self
                .profiles_dir
                .join(format!("{:?}", profile_type).to_lowercase()),
        };

        // Create the directory if it doesn't exist
        let _ = fs::create_dir_all(&type_dir);

        type_dir.join(format!("{}.json", name))
    }

    /// Save a profile
    pub fn save_profile(&self, profile: &Profile) -> Result<()> {
        let profile_path = self.get_profile_path(&profile.name, profile.profile_type.clone());

        // Create parent directories if they don't exist
        if let Some(parent) = profile_path.parent() {
            fs::create_dir_all(parent).map_err(ProfileError::ReadError)?;
        }

        // Check if the profile already exists
        if profile_path.exists() {
            return Err(ProfileError::AlreadyExists(profile.name.clone()));
        }

        // Serialize and save the profile
        let json = serde_json::to_string_pretty(profile)
            .map_err(|e| ProfileError::ParseError(e.to_string()))?;

        let mut file = File::create(profile_path).map_err(ProfileError::ReadError)?;

        file.write_all(json.as_bytes())
            .map_err(ProfileError::ReadError)?;

        Ok(())
    }

    /// Update an existing profile
    pub fn update_profile(&self, profile: &Profile) -> Result<()> {
        let profile_path = self.get_profile_path(&profile.name, profile.profile_type.clone());

        // Check if the profile exists
        if !profile_path.exists() {
            return Err(ProfileError::NotFound(profile.name.clone()));
        }

        // Serialize and save the profile
        let json = serde_json::to_string_pretty(profile)
            .map_err(|e| ProfileError::ParseError(e.to_string()))?;

        let mut file = File::create(profile_path).map_err(ProfileError::ReadError)?;

        file.write_all(json.as_bytes())
            .map_err(ProfileError::ReadError)?;

        Ok(())
    }

    /// Load a profile by name and type
    pub fn load_profile(&self, name: &str, profile_type: ProfileType) -> Result<Profile> {
        let profile_path = self.get_profile_path(name, profile_type);

        // Check if the profile exists
        if !profile_path.exists() {
            return Err(ProfileError::NotFound(name.to_string()));
        }

        // Read the profile file
        let mut file = File::open(profile_path).map_err(ProfileError::ReadError)?;

        let mut contents = String::new();
        file.read_to_string(&mut contents)
            .map_err(ProfileError::ReadError)?;

        // Parse the profile
        let profile: Profile =
            serde_json::from_str(&contents).map_err(|e| ProfileError::ParseError(e.to_string()))?;

        Ok(profile)
    }

    /// Delete a profile
    pub fn delete_profile(&self, name: &str, profile_type: ProfileType) -> Result<()> {
        let profile_path = self.get_profile_path(name, profile_type);

        // Check if the profile exists
        if !profile_path.exists() {
            return Err(ProfileError::NotFound(name.to_string()));
        }

        // Delete the profile file
        fs::remove_file(profile_path).map_err(ProfileError::ReadError)?;

        Ok(())
    }

    /// List all profiles of a specific type
    pub fn list_profiles(&self, profile_type: ProfileType) -> Result<Vec<String>> {
        let type_dir = match profile_type {
            ProfileType::Custom(ref custom) => self.profiles_dir.join("custom").join(custom),
            _ => self
                .profiles_dir
                .join(format!("{:?}", profile_type).to_lowercase()),
        };

        // Check if the directory exists
        if !type_dir.exists() {
            return Ok(Vec::new());
        }

        // Read the directory entries
        let entries = fs::read_dir(type_dir).map_err(ProfileError::ReadError)?;

        // Filter for JSON files and extract the profile names
        let mut profiles = Vec::new();
        for entry in entries {
            let entry = entry.map_err(ProfileError::ReadError)?;
            let path = entry.path();

            if let Some(extension) = path.extension() {
                if extension == "json" {
                    if let Some(stem) = path.file_stem() {
                        if let Some(name) = stem.to_str() {
                            profiles.push(name.to_string());
                        }
                    }
                }
            }
        }

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

                if path.is_dir() {
                    if let Some(custom_type) = path.file_name().and_then(|n| n.to_str()) {
                        let custom_profile_type = ProfileType::Custom(custom_type.to_string());
                        let profiles = self.list_profiles(custom_profile_type.clone())?;
                        if !profiles.is_empty() {
                            result.insert(custom_profile_type, profiles);
                        }
                    }
                }
            }
        }

        Ok(result)
    }

    /// Import a profile from a file
    pub fn import_profile<P: AsRef<Path>>(&self, path: P) -> Result<Profile> {
        // Read the profile file
        let mut file = File::open(path).map_err(ProfileError::ReadError)?;

        let mut contents = String::new();
        file.read_to_string(&mut contents)
            .map_err(ProfileError::ReadError)?;

        // Parse the profile
        let profile: Profile =
            serde_json::from_str(&contents).map_err(|e| ProfileError::ParseError(e.to_string()))?;

        // Save the profile
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
}
