use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Connection settings for one S3-compatible endpoint.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Profile {
    pub name: String,
    pub endpoint: String,
    pub region: String,
    pub access_key: String,
    pub secret_key: String,
    pub path_style: bool,
}

impl Default for Profile {
    fn default() -> Self {
        Self {
            name: "local".into(),
            endpoint: std::env::var("LARDER_ENDPOINT")
                .unwrap_or_else(|_| "http://localhost:9000".into()),
            region: std::env::var("AWS_REGION").unwrap_or_else(|_| "us-east-1".into()),
            access_key: std::env::var("AWS_ACCESS_KEY_ID").unwrap_or_default(),
            secret_key: std::env::var("AWS_SECRET_ACCESS_KEY").unwrap_or_default(),
            path_style: true,
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Config {
    pub profiles: Vec<Profile>,
    pub last_profile: Option<String>,
}

impl Config {
    pub fn path() -> Option<PathBuf> {
        dirs::config_dir().map(|d| d.join("larder").join("config.json"))
    }

    /// Where profiles lived before the app was renamed from s3gui; read if no new config exists yet.
    fn legacy_path() -> Option<PathBuf> {
        dirs::config_dir().map(|d| d.join("s3gui").join("config.json"))
    }

    pub fn load() -> Self {
        [Self::path(), Self::legacy_path()]
            .into_iter()
            .flatten()
            .find(|p| p.exists())
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) -> anyhow::Result<()> {
        let path = Self::path().ok_or_else(|| anyhow::anyhow!("no config directory available"))?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_string_pretty(self)?)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
        }
        std::fs::rename(tmp, path)?;
        Ok(())
    }

    pub fn find(&self, name: &str) -> Option<&Profile> {
        self.profiles.iter().find(|p| p.name == name)
    }

    pub fn upsert(&mut self, profile: Profile) {
        match self.profiles.iter_mut().find(|p| p.name == profile.name) {
            Some(existing) => *existing = profile,
            None => self.profiles.push(profile),
        }
    }
}
