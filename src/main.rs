use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
};

use anyhow::{bail, Context, Result};

mod arguments;
mod config;
use camino::Utf8PathBuf;
use config::{
    machine::{JobConfig, Machine},
    Config,
};
use gcode_generation::GCommand;
mod drill_file;
mod gcode_generation;
mod geometry;
mod gerber_file;
mod parsing;

use crate::{
    config::machine::Tool,
    forge_file::ForgeFile,
    gcode_generation::{BoardSide, GCodeFile, ToolSelection},
    gerber_file::GerberFile,
};
mod forge_file;

fn main() {
    simple_logger::SimpleLogger::new()
        .with_level(log::LevelFilter::Info)
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

    let mut gcode_files = HashMap::new();

    for (stage_index, stage) in forge_file.stages.iter().enumerate() {
        let debug_output_directory = if build_configuration.debug {
            let debug_output_directory = build_configuration
                .target_directory
                .join("debug")
                .join(format!("stage{}", stage_index));
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
                gcode_file,
                backside,
                invert,
            } => {
                log::info!("Process engrave stage: {:?}", gerber_file);

                let gcode: &mut Vec<GCommand> = gcode_files.entry(gcode_file.clone()).or_default();

                gcode.push(GCommand::SetSide(if *backside {
                    BoardSide::Back
                } else {
                    BoardSide::Front
                }));

                let machine_config_path = machine_config
                    .as_ref()
                    .or(global_config.default_engraver.as_ref())
                    .context("An engraver was not specified and a global default is not set.")?;
                log::info!("Using machine configuration: {}", machine_config_path);

                let mut machine_config_path = machine_config_path.iter();
                let machine_name = machine_config_path
                    .next()
                    .context("Machine name not provided by machine config path.")?
                    .to_string();
                let machine_profile = machine_config_path
                    .next()
                    .context("Machine profile not provided by machine config path.")?
                    .to_string();

                if machine_config_path.next().is_some() {
                    bail!("Too many parts to machine config path.");
                }

                let machine_config = forge_file
                    .machines
                    .get(&machine_name)
                    .or(global_config.machines.get(&machine_name))
                    .context("Failed to find machine configuration.")?;

                let job_config = machine_config
                    .engraving_configs
                    .get(&machine_profile)
                    .context("Failed to find machine profile.")?;

                process_gerber_file(GerberConfig {
                    build_configuration: &build_configuration,
                    machine_config,
                    job_config,
                    invert: *invert,
                    gerber_file: gerber_file.as_ref(),
                    debug_output_directory: debug_output_directory.as_ref(),
                    generate_infill: true,
                    gcode,
                })?;
            }
            forge_file::Stage::CutBoard {
                machine_config,
                gcode_file,
                file,
                backside,
            } => {
                log::info!("Process cutting stage: {}", file);

                let gcode = gcode_files.entry(gcode_file.clone()).or_default();

                gcode.push(GCommand::SetSide(if *backside {
                    BoardSide::Back
                } else {
                    BoardSide::Front
                }));

                let machine_config_path = machine_config
                    .as_ref()
                    .or(global_config.default_cutter.as_ref())
                    .context("An engraver was not specified and a global default is not set.")?;
                log::info!("Using machine configuration: {}", machine_config_path);

                let mut machine_config_path = machine_config_path.iter();
                let machine_name = machine_config_path
                    .next()
                    .context("Machine name not provided by machine config path.")?
                    .to_string();
                let machine_profile = machine_config_path
                    .next()
                    .context("Machine profile not provided by machine config path.")?
                    .to_string();

                if machine_config_path.next().is_some() {
                    bail!("Too many parts to machine config path.");
                }

                let machine_config = forge_file
                    .machines
                    .get(&machine_name)
                    .or(global_config.machines.get(&machine_name))
                    .context("Failed to find machine configuration.")?;

                let job_config = machine_config
                    .cutting_configs
                    .get(&machine_profile)
                    .context("Failed to find machine profile.")?;

                match file {
                    forge_file::CutBoardFile::Gerber(gerber_file) => {
                        process_gerber_file(GerberConfig {
                            build_configuration: &build_configuration,
                            machine_config,
                            job_config,
                            invert: false,
                            gerber_file: gerber_file.as_ref(),
                            debug_output_directory: debug_output_directory.as_ref(),
                            generate_infill: false,
                            gcode,
                        })?;
                    }
                    forge_file::CutBoardFile::Drill(drill_file) => {
                        let file_path = build_configuration
                            .forge_file_path
                            .parent()
                            .context("Could not get working directory of forge file.")?
                            .join(drill_file);

                        let mut drill_file = drill_file::DrillFile::default();
                        drill_file::load(&mut drill_file, &file_path)
                            .context("Failed to load drill file.")?;

                        let tool_selection = get_tool_selection(machine_config, &job_config.tool)?;

                        drill_file
                            .generate_gcode(gcode, job_config, &tool_selection)
                            .context("Failed to generate gcode file.")?;
                    }
                }
            }
        }
    }

    for (path, commands) in gcode_files {
        let output_file = build_configuration.target_directory.join(&path);
        let gcode_file = GCodeFile::new(commands);
        let output = gcode_file
            .to_string()
            .with_context(|| format!("Failed to produce GCode for file: {:?}", path))?;
        fs::write(output_file, output).context("Failed to save GCode file.")?;
    }

    Ok(())
}

struct GerberConfig<'a> {
    build_configuration: &'a arguments::BuildCommand,
    machine_config: &'a Machine,
    job_config: &'a JobConfig,
    invert: bool,
    gerber_file: &'a Path,
    debug_output_directory: Option<&'a PathBuf>,
    generate_infill: bool,
    gcode: &'a mut Vec<GCommand>,
}

fn process_gerber_file(config: GerberConfig) -> Result<()> {
    log::info!("Tool Info: {}", config.job_config.tool_power);

    let tool_selection = get_tool_selection(config.machine_config, &config.job_config.tool)?;

    let file_path = config
        .build_configuration
        .forge_file_path
        .parent()
        .context("Could not get working directory of forge file.")?
        .join(config.gerber_file);

    let mut gerber = GerberFile::default();

    // We load the file, or at least attempt to. We'll handle an error condition later.
    let load_result =
        gerber_file::load(&mut gerber, &file_path).context("Failed to load gerber file.");

    // Debug render if applicable.
    if let Some(debug_output_directory) = config.debug_output_directory.as_ref() {
        let output_file = debug_output_directory.join("gerber.svg");
        let bounds = gerber.calculate_svg_bounds();

        let mut document = svg_composer::Document::new(
            Vec::new(),
            Some([
                bounds.0 as f32,
                bounds.1 as f32,
                bounds.2 as f32,
                bounds.3 as f32,
            ]),
        );
        gerber
            .debug_render(&mut document, false)
            .context("Failed to render gerber debug SVG file.")?;

        fs::write(output_file, document.render())
            .context("Failed to save gerber debug SVG file.")?;
    }

    // Okay cool, now you can handle the error.
    load_result?;

    // Debug render if applicable.
    if let Some(debug_output_directory) = config.debug_output_directory.as_ref() {
        let output_file = debug_output_directory.join("gerber_simplified.svg");
        let bounds = gerber.calculate_svg_bounds();

        let mut document = svg_composer::Document::new(
            Vec::new(),
            Some([
                bounds.0 as f32,
                bounds.1 as f32,
                bounds.2 as f32,
                bounds.3 as f32,
            ]),
        );

        gerber
            .debug_render(&mut document, true)
            .context("Failed to render gerber debug SVG file.")?;

        fs::write(output_file, document.render())
            .context("Failed to save gerber debug SVG file.")?;
    }

    gerber
        .generate_gcode(
            config.gcode,
            config.job_config,
            &tool_selection,
            config.generate_infill,
            config.invert,
        )
        .context("Failed to generate GCode file.")?;

    Ok(())
}

fn get_tool_selection<'a>(
    machine_config: &'a Machine,
    tool_path: &Utf8PathBuf,
) -> Result<ToolSelection<'a>> {
    log::info!("Using tool: {}", tool_path);

    let mut tool_path = tool_path.ancestors();

    let tool_name = tool_path
        .next()
        .context("no tool name provided")?
        .to_string();

    let tool = machine_config
        .tools
        .get(&tool_name)
        .context("Could not find specified tool.")?;

    let bit_name = tool_path.next().map(|name| name.to_string());

    Ok(match tool {
        Tool::Laser(laser) => ToolSelection::Laser { laser },
        Tool::Spindle(spindle) => {
            let bit_name = bit_name.context("No bit name provided for spindle.")?;
            log::info!("Using bit: {}", bit_name);
            ToolSelection::Spindle {
                spindle,
                bit: spindle
                    .bits
                    .get(&bit_name)
                    .context("Spindle does not have a bit with requested name.")?,
            }
        }
    })
}
