use std::path::PathBuf;

use argh::FromArgs;

#[derive(FromArgs, PartialEq, Debug)]
/// A tool to generate GCode for machines that manufacture Printed Circuit Boards.
pub struct Arguments {
    #[argh(subcommand)]
    pub command: CommandEnum,
}

#[derive(FromArgs, PartialEq, Debug)]
#[argh(subcommand)]
pub enum CommandEnum {
    Build(BuildCommand),
}

#[derive(FromArgs, PartialEq, Debug)]
/// Generate gcode files for project.
#[argh(subcommand, name = "build")]
pub struct BuildCommand {
    #[argh(option, default = "PathBuf::from(\"forge.yaml\")")]
    /// path to the project forge file.
    pub forge_file_path: PathBuf,

    #[argh(option, default = "PathBuf::from(\"forge\")")]
    /// path to the folder to place output files into.
    pub target_directory: PathBuf,
}
