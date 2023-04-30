use std::{collections::HashMap, fs, path::Path};

use anyhow::{bail, Context, Result};
use nalgebra::Vector2;
use uom::si::length::{mil, millimeter, Length};

use crate::{
    geometry::Shape,
    parsing::{
        self,
        drill::{self, DrillCommand, HeaderCommand},
        UnitMode,
    },
};

#[derive(Debug, Default)]
pub struct DrillFile {
    holes: Vec<DrillHole>,
    paths: Vec<DrillPath>,
}

#[derive(Debug)]
pub struct DrillHole {
    position: Vector2<f64>,
    diameter: f64,
}

#[derive(Debug)]
pub struct DrillPath {
    shape: Shape,
    diameter: f64,
}

#[derive(Debug, Eq, PartialEq)]
enum CoordinateMode {
    Absolute,
    Incremental,
}

#[derive(Debug, Eq, PartialEq)]
enum CutMode {
    Drill,
    Route,
}

struct DrillingContext {
    unit_mode: UnitMode,
    tools: HashMap<usize, f64>,
    coordinate_mode: CoordinateMode,
    cut_mode: CutMode,
    position: Vector2<f64>,
    tool_diameter: Option<f64>,
}

impl DrillingContext {
    fn internalize_axis(&self, axis: f64) -> f64 {
        // Convert to mm for internal representation.
        match self.unit_mode {
            UnitMode::Metric => Length::<uom::si::SI<f64>, f64>::new::<millimeter>(axis),
            UnitMode::Imperial => Length::<uom::si::SI<f64>, f64>::new::<mil>(axis),
        }
        .get::<millimeter>()
    }

    fn internalize_coordinate(&self, coordinate: Vector2<f64>) -> Vector2<f64> {
        Vector2::new(
            self.internalize_axis(coordinate.x),
            self.internalize_axis(coordinate.y),
        )
    }
}

pub fn load(drill_file: &mut DrillFile, path: &Path) -> Result<()> {
    let drill_file_content =
        fs::read_to_string(path).context("Failed to read drill file from disk.")?;
    match parsing::drill::parse_drill_file(parsing::drill::Span::new(&drill_file_content)) {
        Ok((_remainder, (header, commands))) => {
            let mut tools = HashMap::new();
            let mut unit_mode = None;

            for command in header.iter() {
                let location_info = command.location_info();

                process_header_command(&command.command, &mut tools, &mut unit_mode).with_context(
                    move || {
                        format!(
                            "error processing header command: {}:{}",
                            path.to_string_lossy(),
                            location_info
                        )
                    },
                )?;
            }

            let unit_mode = unit_mode.context("Unit mode is missing from file header.")?;

            let mut drilling_context = DrillingContext {
                unit_mode,
                tools,
                coordinate_mode: CoordinateMode::Absolute,
                cut_mode: CutMode::Drill,
                position: Vector2::zeros(),
                tool_diameter: None,
            };

            for command in commands.iter() {
                let location_info = command.location_info();

                process_drill_command(
                    &command.command,
                    &mut drilling_context,
                    &mut drill_file.holes,
                    &mut drill_file.paths,
                )
                .with_context(move || {
                    format!(
                        "error processing drill command: {}:{}",
                        path.to_string_lossy(),
                        location_info
                    )
                })?;
            }
        }
        Err(error) => match error {
            nom::Err::Error(error) | nom::Err::Failure(error) => {
                let _ = error;
                bail!(
                    "Failed to parse drill file {}:{}:{} - {:?}",
                    path.to_string_lossy(),
                    error.input.location_line(),
                    error.input.get_utf8_column(),
                    error.code,
                )
            }
            nom::Err::Incomplete(_) => {
                bail!("Failed to parse drill file: Unexpected EOF")
            }
        },
    }

    Ok(())
}

fn process_drill_command(
    command: &DrillCommand,
    drilling_context: &mut DrillingContext,
    holes: &mut Vec<DrillHole>,
    paths: &mut Vec<DrillPath>,
) -> Result<()> {
    match command {
        DrillCommand::Comment(_comment) => {}
        DrillCommand::AbsoluteMode => drilling_context.coordinate_mode = CoordinateMode::Absolute,
        DrillCommand::IncrementalMode => {
            drilling_context.coordinate_mode = CoordinateMode::Incremental
        }
        DrillCommand::DrillMode => drilling_context.cut_mode = CutMode::Drill,
        DrillCommand::RouteMode => drilling_context.cut_mode = CutMode::Route,
        DrillCommand::SelectTool(index) => {
            if *index != 0 {
                let diameter = drilling_context
                    .tools
                    .get(index)
                    .context("Command referenced undefined tool.")?;
                drilling_context.tool_diameter = Some(drilling_context.internalize_axis(*diameter));
            } else {
                drilling_context.tool_diameter = None;
            }
        }
        DrillCommand::DrillHit { target } => {
            let target = drilling_context.internalize_coordinate(*target);

            if drilling_context.cut_mode == CutMode::Drill {
                match drilling_context.coordinate_mode {
                    CoordinateMode::Absolute => {
                        holes.push(DrillHole {
                            position: target,
                            diameter: drilling_context
                                .tool_diameter
                                .context("No tool equipped.")?,
                        });

                        drilling_context.position = target;
                    }
                    CoordinateMode::Incremental => {
                        let new_position = drilling_context.position + target;

                        holes.push(DrillHole {
                            position: new_position,
                            diameter: drilling_context
                                .tool_diameter
                                .context("No tool equipped.")?,
                        });

                        drilling_context.position = new_position;
                    }
                }
            } else {
                bail!("Drill hit specified while in routing mode.");
            }
        }
        DrillCommand::ToolDown => bail!("Unimplemented 1"),
        DrillCommand::ToolUp => bail!("Unimplemented 2"),
        DrillCommand::LinearMove { target } => bail!("Unimplemented 3"),
        DrillCommand::ClockwiseCurve { target, diameter } => bail!("Unimplemented 4"),
        DrillCommand::CounterClockwiseCurve { target, diameter } => bail!("Unimplemented 5"),
    }

    Ok(())
}

fn process_header_command(
    command: &HeaderCommand,
    tools: &mut HashMap<usize, f64>,
    unit_mode: &mut Option<UnitMode>,
) -> Result<()> {
    match command {
        HeaderCommand::Comment(_comment) => {}
        HeaderCommand::UnitMode(new_unit_mode) => {
            if unit_mode.is_some() {
                log::warn!("Unit mode for drill file was set more than once.");
            }

            *unit_mode = Some(*new_unit_mode);
        }
        HeaderCommand::Format(_version) => {
            // Unique to KiCad, not something we pay attention to.
        }
        HeaderCommand::ToolDeclaration { index, diameter } => {
            if tools.insert(*index, *diameter).is_some() {
                log::warn!("Tool {} has been defined multiple times.", index);
            }
        }
    }

    Ok(())
}
