use anyhow::{Context, Result};
use camino::Utf8PathBuf;
use serde::Deserialize;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

pub mod machine;
use machine::Machine;

#[derive(Debug, Deserialize, Default)]
pub struct Config {
    /// Machines in your fleet at your disposal.
    pub machines: HashMap<String, Machine>,

    /// When no machine is specified in a project's forge file, use this one for engraving.
    pub default_engraver: Option<Utf8PathBuf>,

    /// When no machine is specified in a project's forge file, use this one for cutting.
    pub default_cutter: Option<Utf8PathBuf>,
}

impl Config {
    pub fn load() -> Result<Self> {
        Self::load_from_path(&Self::get_path()?)
    }

    pub fn get_path() -> Result<PathBuf> {
        let home_dir = home::home_dir().context("Failed to get user's home directory.")?;
        Ok(home_dir.join(".config/pcb_forge/config.yaml"))
    }

    pub fn load_from_path(path: &Path) -> Result<Self> {
        let config = std::fs::read_to_string(path).context("Failed to read config file.")?;
        let config: Self =
            serde_yaml::from_str(&config).context("Failed to decode config file.")?;

        Ok(config)
    }
}
