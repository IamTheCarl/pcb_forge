use anyhow::{Context, Result};

mod arguments;
mod config;
use config::Config;
mod parsing;

use crate::forge_file::ForgeFile;
mod forge_file;

fn main() {
    simple_logger::SimpleLogger::new()
        .init()
        .expect("Failed to initialize logger.");

    if let Err(error) = trampoline() {
        log::error!("Fatal error: {:?}", error);
    }
}

fn trampoline() -> Result<()> {
    let arguments: arguments::Arguments = argh::from_env();

    let config = match Config::load() {
        Ok(config) => config,
        Err(error) => {
            log::warn!(
                "Failed to read config file at {}: {:?}",
                Config::get_path()
                    .map(|path| path.to_string_lossy().to_string())
                    .unwrap_or(String::from("'unavailable'")),
                error
            );
            config::Config::default()
        }
    };

    match arguments.command {
        arguments::CommandEnum::Build(build_configuration) => build(build_configuration, config),
    }
}

fn build(build_configuration: arguments::BuildCommand, global_config: Config) -> Result<()> {
    log::info!("Read Forge File: {:?}", build_configuration.forge_file_path);
    let forge_file = ForgeFile::load_from_path(&build_configuration.forge_file_path)
        .context("Failed to load forge file.")?;

    Ok(())
}
