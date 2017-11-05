use std::default::Default;
use std::fs::File;
use std::io::{self, Read};
use std::path::Path;

use toml;

#[derive(Deserialize)]
pub struct Webhooks {
    pub stream_start: Option<String>,
    pub stream_end: Option<String>,
    pub listener_start: Option<String>,
    pub listener_end: Option<String>,
}

impl Default for Webhooks {
    fn default() -> Self {
        Webhooks {
            stream_start: None,
            stream_end: None,
            listener_start: None,
            listener_end: None,
        }
    }
}

#[derive(Deserialize)]
pub struct Config {
    pub listen: String,
    pub stream_dump: String,
    pub session_cookie: Option<String>,
    #[serde(default)]
    pub webhooks: Webhooks,
}

#[derive(Debug)]
pub enum ConfigError {
    Io(io::Error),
    Toml(toml::de::Error),
}

pub fn open(path: &Path) -> Result<Config, ConfigError> {
    let mut file = File::open(path).map_err(ConfigError::Io)?;
    let mut buff = String::new();
    file.read_to_string(&mut buff).map_err(ConfigError::Io)?;
    toml::from_str(&buff).map_err(ConfigError::Toml)
}
