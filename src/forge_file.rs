use anyhow::{Context, Result};
use camino::Utf8PathBuf;
use semver::Version;
use serde::Deserialize;
use std::{
    collections::HashMap,
    fmt::Display,
    path::{Path, PathBuf},
};

use crate::config::machine::Machine;

#[derive(Debug, Deserialize)]
pub struct ForgeFile {
    pub project_name: String,
    pub board_version: Version,

    #[serde(default = "ForgeFile::align_backside_default")]
    pub align_backside: bool,

    #[serde(default)]
    /// Projects can specify machines as well, to speed up team onboarding.
    pub machines: HashMap<String, Machine>,

    pub gcode_files: HashMap<PathBuf, Vec<Stage>>,
}

impl ForgeFile {
    pub fn load_from_path(path: &Path) -> Result<Self> {
        let forge = std::fs::read_to_string(path).context("Failed to read forge file.")?;
        let forge: Self = serde_yaml::from_str(&forge).context("Failed to decode forge file.")?;

        Ok(forge)
    }

    fn align_backside_default() -> bool {
        true
    }
}

#[derive(Debug, Deserialize)]
pub enum Stage {
    #[serde(rename = "engrave_mask")]
    EngraveMask {
        machine_config: Option<Utf8PathBuf>,
        gerber_file: PathBuf,

        #[serde(default)]
        backside: bool,

        #[serde(default)]
        invert: bool,
    },
    #[serde(rename = "cut_board")]
    CutBoard {
        machine_config: Option<Utf8PathBuf>,

        #[serde(flatten)]
        file: CutBoardFile,

        #[serde(default)]
        backside: bool,
    },
}

#[derive(Debug, Deserialize, Clone, Copy)]
pub enum LineSelection {
    #[serde(rename = "all")]
    All,
    #[serde(rename = "inner")]
    Inner,
    #[serde(rename = "outer")]
    Outer,
}

impl Default for LineSelection {
    fn default() -> Self {
        Self::All
    }
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum CutBoardFile {
    Gerber {
        gerber_file: PathBuf,

        #[serde(default)]
        select_lines: LineSelection,
    },
    Drill {
        drill_file: PathBuf,
    },
}

impl Display for CutBoardFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CutBoardFile::Gerber {
                gerber_file,
                select_lines: _,
            } => write!(f, "gerber file: {:?}", gerber_file),
            CutBoardFile::Drill { drill_file } => write!(f, "drill file: {:?}", drill_file),
        }
    }
}
