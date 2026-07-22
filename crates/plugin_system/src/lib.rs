use std::any::Any;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use libloading::{Library, Symbol};
use thiserror::Error;

/// Errors specific to the plugin system
#[derive(Error, Debug)]
pub enum PluginError {
    #[error("Failed to load plugin: {0}")]
    LoadError(String),

    #[error("Plugin initialization failed: {0}")]
    InitError(String),

    #[error("Invalid plugin: {0}")]
    InvalidPlugin(String),

    #[error("Plugin not found: {0}")]
    NotFound(String),

    #[error("Plugin {0} is not compatible with this version")]
    IncompatibleVersion(String),
}

/// Plugin API version to ensure compatibility
pub const PLUGIN_API_VERSION: u32 = 1;

/// Represents the metadata of a plugin
#[derive(Clone, Debug)]
pub struct PluginMetadata {
    pub name: String,
    pub version: String,
    pub author: String,
    pub description: String,
    pub api_version: u32,
}

/// Trait that must be implemented by all plugins
pub trait Plugin: Send + Sync {
    /// Get plugin metadata
    fn metadata(&self) -> PluginMetadata;

    /// Initialize the plugin
    fn initialize(&mut self) -> Result<(), Box<dyn std::error::Error>>;

    /// Execute the plugin's functionality with the given parameters
    fn execute(&self, params: HashMap<String, String>) -> Result<(), Box<dyn std::error::Error>>;

    /// Get information about the parameters the plugin accepts
    fn get_parameter_info(&self) -> Vec<ParameterInfo>;

    /// Clean up resources when the plugin is being unloaded
    fn shutdown(&self) -> Result<(), Box<dyn std::error::Error>>;

    /// Allows plugins to provide additional functionality that can be accessed via downcasting
    fn as_any(&self) -> &dyn Any;
}

/// Describes a parameter that the plugin accepts
#[derive(Clone, Debug)]
pub struct ParameterInfo {
    pub name: String,
    pub description: String,
    pub required: bool,
    pub default_value: Option<String>,
    pub parameter_type: ParameterType,
}

/// Types of parameters that plugins can declare
#[derive(Clone, Debug, PartialEq)]
pub enum ParameterType {
    String,
    Integer,
    Float,
    Boolean,
    FilePath,
    DirectoryPath,
}

/// Type definition for the plugin creation function that must be exported by plugin libraries
pub type CreatePluginFunc = unsafe fn() -> *mut dyn Plugin;

/// Manages loading and interaction with plugins
#[derive(Clone)]
pub struct PluginManager {
    plugins: Arc<Mutex<HashMap<String, Box<dyn Plugin>>>>,
    libraries: Arc<Mutex<HashMap<String, Library>>>,
    plugin_dirs: Vec<PathBuf>,
}

impl PluginManager {
    /// Create a new plugin manager with the default plugin directory
    pub fn new() -> Result<Self, PluginError> {
        Ok(Self {
            plugins: Arc::new(Mutex::new(HashMap::new())),
            libraries: Arc::new(Mutex::new(HashMap::new())),
            plugin_dirs: vec![PathBuf::from("plugins")],
        })
    }

    /// Add a directory to search for plugins
    pub fn add_plugin_directory<P: AsRef<Path>>(&mut self, dir: P) {
        self.plugin_dirs.push(dir.as_ref().to_path_buf());
    }

    /// Load a plugin from a dynamic library
    pub fn load_plugin<P: AsRef<Path>>(&self, path: P) -> Result<(), PluginError> {
        let path = path.as_ref();

        // Load the dynamic library
        let lib = unsafe { Library::new(path).map_err(|e| PluginError::LoadError(e.to_string()))? };

        // Get the plugin creation function
        let constructor: Symbol<CreatePluginFunc> = unsafe {
            lib.get(b"create_plugin").map_err(|e| {
                PluginError::InvalidPlugin(format!("Missing create_plugin symbol: {}", e))
            })?
        };

        // Create the plugin instance
        let plugin_ptr = unsafe { constructor() };
        if plugin_ptr.is_null() {
            return Err(PluginError::InitError(
                "Plugin creation returned null".to_string(),
            ));
        }

        let mut plugin = unsafe { Box::from_raw(plugin_ptr) };

        // Initialize the plugin
        plugin
            .initialize()
            .map_err(|e| PluginError::InitError(e.to_string()))?;

        // Check API version compatibility
        let metadata = plugin.metadata();
        if metadata.api_version != PLUGIN_API_VERSION {
            return Err(PluginError::IncompatibleVersion(metadata.name.clone()));
        }

        // Store the plugin and library
        let plugin_name = metadata.name.clone();
        self.plugins
            .lock()
            .unwrap()
            .insert(plugin_name.clone(), plugin);
        self.libraries.lock().unwrap().insert(plugin_name, lib);

        Ok(())
    }

    /// Discover and load all plugins from the configured plugin directories
    pub fn discover_plugins(&self) -> Vec<Result<PluginMetadata, PluginError>> {
        let mut results = Vec::new();

        for dir in &self.plugin_dirs {
            if !dir.exists() || !dir.is_dir() {
                continue;
            }

            if let Ok(entries) = std::fs::read_dir(dir) {
                for entry in entries.filter_map(Result::ok) {
                    let path = entry.path();
                    let extension = path.extension().and_then(|e| e.to_str());

                    // Check for platform-specific plugin extensions
                    #[cfg(target_os = "windows")]
                    let is_plugin = extension == Some("dll");

                    #[cfg(target_os = "linux")]
                    let is_plugin = extension == Some("so");

                    #[cfg(target_os = "macos")]
                    let is_plugin = extension == Some("dylib");

                    if is_plugin {
                        match self.load_plugin(&path) {
                            Ok(()) => {
                                let plugin_name =
                                    path.file_stem().unwrap().to_string_lossy().to_string();
                                if let Some(metadata) =
                                    self.with_plugin(&plugin_name, |plugin| plugin.metadata())
                                {
                                    results.push(Ok(metadata));
                                }
                            }
                            Err(e) => results.push(Err(e)),
                        }
                    }
                }
            }
        }

        results
    }

    /// Get a plugin by name and execute a function on it
    pub fn with_plugin<F, R>(&self, name: &str, f: F) -> Option<R>
    where
        F: FnOnce(&dyn Plugin) -> R,
    {
        let plugins = self.plugins.lock().unwrap();
        plugins.get(name).map(|plugin| f(plugin.as_ref()))
    }

    /// Get parameter info for a plugin
    pub fn get_plugin_parameters(&self, name: &str) -> Option<Vec<ParameterInfo>> {
        self.with_plugin(name, |plugin| plugin.get_parameter_info())
    }

    /// Execute a plugin with the given parameters
    pub fn execute_plugin(
        &self,
        name: &str,
        params: HashMap<String, String>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        match self.with_plugin(name, |plugin| plugin.execute(params.clone())) {
            Some(result) => result,
            None => Err(Box::new(PluginError::NotFound(name.to_string()))),
        }
    }

    /// Get metadata for all loaded plugins
    pub fn get_all_plugin_metadata(&self) -> Vec<PluginMetadata> {
        let plugins = self.plugins.lock().unwrap();
        plugins.values().map(|p| p.metadata()).collect()
    }

    /// Unload a plugin by name
    pub fn unload_plugin(&self, name: &str) -> Result<(), PluginError> {
        // Get the plugin
        let mut plugins = self.plugins.lock().unwrap();
        let plugin = plugins
            .remove(name)
            .ok_or_else(|| PluginError::NotFound(name.to_string()))?;

        // Shut down the plugin
        plugin
            .shutdown()
            .map_err(|e| PluginError::InitError(e.to_string()))?;

        // Remove the library
        let mut libraries = self.libraries.lock().unwrap();
        libraries.remove(name);

        Ok(())
    }
}

impl Drop for PluginManager {
    fn drop(&mut self) {
        // Make sure all plugins are properly shut down
        let plugin_names: Vec<String> = self.plugins.lock().unwrap().keys().cloned().collect();
        for name in plugin_names {
            let _ = self.unload_plugin(&name);
        }
    }
}

/// Macro to help plugin libraries export their creation function
#[macro_export]
macro_rules! export_plugin {
    ($plugin_type:ty) => {
        #[no_mangle]
        #[allow(improper_ctypes_definitions)]
        pub extern "C" fn create_plugin() -> *mut dyn $crate::Plugin {
            let plugin = Box::new(<$plugin_type>::new());
            Box::into_raw(plugin)
        }
    };
}

/// Example of how to implement a plugin
pub mod example {
    use super::*;
    use std::collections::HashMap;

    pub struct ExamplePlugin {
        metadata: PluginMetadata,
    }

    impl Default for ExamplePlugin {
        fn default() -> Self {
            Self::new()
        }
    }

    impl ExamplePlugin {
        pub fn new() -> Self {
            Self {
                metadata: PluginMetadata {
                    name: "example_plugin".to_string(),
                    version: "0.1.0".to_string(),
                    author: "Video Toolkit Team".to_string(),
                    description: "An example plugin that demonstrates the plugin system"
                        .to_string(),
                    api_version: PLUGIN_API_VERSION,
                },
            }
        }
    }

    impl Plugin for ExamplePlugin {
        fn metadata(&self) -> PluginMetadata {
            self.metadata.clone()
        }

        fn initialize(&mut self) -> Result<(), Box<dyn std::error::Error>> {
            println!("Example plugin initialized");
            Ok(())
        }

        fn execute(
            &self,
            params: HashMap<String, String>,
        ) -> Result<(), Box<dyn std::error::Error>> {
            println!("Example plugin executed with parameters: {:?}", params);
            Ok(())
        }

        fn get_parameter_info(&self) -> Vec<ParameterInfo> {
            vec![
                ParameterInfo {
                    name: "input_file".to_string(),
                    description: "Input file to process".to_string(),
                    required: true,
                    default_value: None,
                    parameter_type: ParameterType::FilePath,
                },
                ParameterInfo {
                    name: "output_file".to_string(),
                    description: "Where to save the result".to_string(),
                    required: true,
                    default_value: None,
                    parameter_type: ParameterType::FilePath,
                },
            ]
        }

        fn shutdown(&self) -> Result<(), Box<dyn std::error::Error>> {
            println!("Example plugin shut down");
            Ok(())
        }

        fn as_any(&self) -> &dyn Any {
            self
        }
    }
}
