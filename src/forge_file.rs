use anyhow::{Context, Result};
use camino::Utf8PathBuf;
use semver::Version;
use serde::Deserialize;
use std::{
    collections::HashMap,
    fmt::Display,
    path::{Path, PathBuf},
};
use uom::si::length::Length;

use crate::{config::machine::Machine, parsing::parse_quantity};

#[derive(Debug, Deserialize)]
pub struct ForgeFile {
    pub project_name: String,
    pub board_version: Version,

    #[serde(deserialize_with = "parse_quantity")]
    pub board_thickness: Length<uom::si::SI<f64>, f64>,

    #[serde(default)]
    /// Projects can specify machines as well, to speed up team onboarding.
    pub machines: HashMap<String, Machine>,

    pub stages: Vec<Stage>,
}

impl ForgeFile {
    pub fn load_from_path(path: &Path) -> Result<Self> {
        let forge = std::fs::read_to_string(path).context("Failed to read forge file.")?;
        let forge: Self = serde_yaml::from_str(&forge).context("Failed to decode forge file.")?;

        Ok(forge)
    }
}

#[derive(Debug, Deserialize)]
pub enum Stage {
    #[serde(rename = "engrave_mask")]
    EngraveMask {
        machine_config: Option<Utf8PathBuf>,
        gerber_file: PathBuf,
    },
    #[serde(rename = "cut_board")]
    CutBoard {
        machine_config: Option<Utf8PathBuf>,

        #[serde(flatten)]
        file: CutBoardFile,
    },
}

#[derive(Debug, Deserialize)]
pub enum CutBoardFile {
    #[serde(rename = "gerber_file")]
    Gerber(PathBuf),
    #[serde(rename = "drill_file")]
    Drill(PathBuf),
}

impl Display for CutBoardFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CutBoardFile::Gerber(file_path) => write!(f, "gerber file: {:?}", file_path),
            CutBoardFile::Drill(file_path) => write!(f, "drill file: {:?}", file_path),
        }
    }
}
