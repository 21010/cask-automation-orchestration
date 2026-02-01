use serde::Deserialize;
use std::fs;
use std::path::Path;
use anyhow::Result;

#[derive(Debug, Deserialize)]
pub struct Blueprint {
    pub name: Option<String>,
    pub description: Option<String>,
    
    // Default to "3.10" if missing
    #[serde(default = "default_python")]
    pub python: String,
    
    // The list of pip packages
    pub dependencies: Vec<String>,
}

fn default_python() -> String {
    "3.10".to_string()
}

impl Blueprint {
    pub fn load(path: &Path) -> Result<Self> {
        let content = fs::read_to_string(path)?;
        let blueprint: Blueprint = serde_yaml::from_str(&content)?;
        Ok(blueprint)
    }

    /// Converts the struct back into requirements.txt format for uv
    pub fn to_requirements_txt(&self) -> String {
        self.dependencies.join("\n")
    }
}