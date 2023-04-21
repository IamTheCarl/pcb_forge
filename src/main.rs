use std::fs;

use anyhow::{Context, Result};

mod arguments;
mod config;
use config::Config;
mod gerber_file;
mod parsing;

use crate::{forge_file::ForgeFile, gerber_file::GerberFile};
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

    for (index, stage) in forge_file.stages.iter().enumerate() {
        let debug_output_directory = if build_configuration.debug {
            let debug_output_directory = build_configuration
                .target_directory
                .join("debug")
                .join(format!("stage{}", index));
            fs::create_dir_all(&debug_output_directory)
                .context("Failed to create directory for debug output.")?;

            log::info!("Debug output directory: {:?}", debug_output_directory);

            Some(debug_output_directory)
        } else {
            None
        };

        match stage {
            forge_file::Stage::EngraveMask {
                machine_config,
                gerber_file,
            } => {
                log::info!("Process engrave stage: {:?}", gerber_file);

                let file_path = build_configuration
                    .forge_file_path
                    .parent()
                    .context("Could not get working directory of forge file.")?
                    .join(gerber_file);

                let mut gerber = GerberFile::default();

                // We load the file, or at least attempt to. We'll handle an error condition later.
                let load_result = gerber_file::load(&mut gerber, &file_path)
                    .context("Failed to load gerber file.");

                // dbg!(&gerber);

                // Debug render if applicable.
                if let Some(debug_output_directory) = debug_output_directory {
                    let output_file = debug_output_directory.join("gerber.svg");
                    let bounds = gerber.calculate_bounds();

                    let mut document = svg_composer::Document::new(
                        Vec::new(),
                        Some([bounds.0, bounds.1, bounds.2, bounds.3]),
                    );
                    gerber
                        .debug_render(&mut document)
                        .context("Failed to render gerber debug SVG file.")?;

                    fs::write(output_file, document.render())
                        .context("Failed to save gerber debug SVG file.")?;
                }

                // Okay cool, now you can handle the error.
                load_result?;

                // TODO
            }
            forge_file::Stage::CutBoard {
                machine_config,
                file,
            } => {
                // TODO
                log::info!("Process cutting stage: {}", file);
            }
        }
    }

    Ok(())
}
