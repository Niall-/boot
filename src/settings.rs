use failure::Error;
use irc::client::data::Config as IRCConfig;
use serde::Deserialize;
use std::fs;
use std::path::Path;

#[derive(Debug, Default, Deserialize)]
pub struct BotConfig {
    pub db: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct Settings {
    #[serde(default)]
    pub bot: BotConfig,
    pub irc: IRCConfig,
}

impl Settings {
    pub fn load(path: impl AsRef<Path>) -> Result<Self, Error> {
        let conf = fs::read_to_string(path)?;
        let settings: Settings = toml::de::from_str(&conf)?;
        Ok(settings)
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            bot: BotConfig {
                db: None,
            },
            irc: IRCConfig {
                ..IRCConfig::default()
            },
        }
    }
}
