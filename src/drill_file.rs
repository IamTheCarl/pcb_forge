use std::{collections::HashMap, fs, path::Path};

use anyhow::{bail, Context, Result};
use nalgebra::Vector2;
use uom::si::length::{inch, millimeter, Length};

use crate::{
    gcode_generation::{GCodeConfig, GCommand, MovementType, Tool, ToolSelection},
    geometry::Shape,
    parsing::{
        self,
        drill::{DrillCommand, HeaderCommand},
        UnitMode,
    },
};

#[derive(Debug, Default)]
pub struct DrillFile {
    holes: Vec<DrillHole>,
    paths: Vec<DrillPath>,
}

impl DrillFile {
    pub fn generate_gcode(&self, config: GCodeConfig) -> Result<()> {
        let passes = match config.job_config.tool_power {
            crate::config::machine::ToolConfig::Laser {
                laser_power,
                work_speed,
                passes,
            } => {
                if let ToolSelection::Laser { laser } = config.tool_config {
                    config.commands.extend(
                        [
                            GCommand::EquipTool(Tool::Laser {
                                max_power: laser.max_power,
                            }),
                            GCommand::UnitMode(UnitMode::Metric),
                            GCommand::SetRapidTransverseSpeed(config.machine_config.jog_speed),
                            GCommand::SetWorkSpeed(work_speed),
                            GCommand::SetPower(laser_power),
                        ]
                        .iter()
                        .cloned(),
                    );

                    passes
                } else {
                    bail!("Job was configured for a laser but selected tool is not a laser.");
                }
            }
            crate::config::machine::ToolConfig::EndMill {
                spindle_speed: spindle_rpm,
                max_cut_depth,
                plunge_speed,
                work_speed,
            } => {
                if let ToolSelection::Spindle { spindle, bit: _ } = config.tool_config {
                    config.commands.extend(
                        [
                            GCommand::EquipTool(Tool::Spindle {
                                max_spindle_speed: spindle.max_speed,
                                plunge_speed,
                                plunge_depth: max_cut_depth,
                            }),
                            GCommand::UnitMode(UnitMode::Metric),
                            GCommand::SetRapidTransverseSpeed(config.machine_config.jog_speed),
                            GCommand::SetWorkSpeed(work_speed),
                            GCommand::SetSpindleSpeed(spindle_rpm),
                        ]
                        .iter()
                        .cloned(),
                    );

                    // We only ever do one pass.
                    1
                } else {
                    bail!("Job was configured for a laser but selected tool is not a laser.");
                }
            }
        };

        if let Some(init_gcode) = config.tool_config.init_gcode() {
            config.commands.push(GCommand::IncludeFile(
                config.include_file_search_directory.join(init_gcode),
            ));
        }

        let distance_per_step = config.job_config.distance_per_step.get::<millimeter>();

        let mut holes = self.holes.clone();
        let mut last_position = Vector2::new(0.0, 0.0);

        while !holes.is_empty() {
            let mut last_distance = f64::INFINITY;
            let mut hole_selection = 0;

            for (hole_index, hole) in holes.iter().enumerate() {
                let distance_to_hole = (hole.position - last_position).norm();
                if distance_to_hole < last_distance {
                    last_distance = distance_to_hole;
                    hole_selection = hole_index;
                }
            }

            let hole = holes.remove(hole_selection);

            for _pass in 0..passes {
                hole.generate_gcode(
                    distance_per_step,
                    config.commands,
                    config.tool_config.diameter().get::<millimeter>(),
                );
            }

            last_position = hole.position;
        }

        // TODO render paths.

        if let Some(shutdown_gcode) = config.tool_config.shutdown_gcode() {
            config.commands.push(GCommand::IncludeFile(
                config.include_file_search_directory.join(shutdown_gcode),
            ));
        }

        config.commands.push(GCommand::EquipTool(Tool::None));

        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct DrillHole {
    position: Vector2<f64>,
    diameter: f64,
}

impl DrillHole {
    /// Create the hole using a laser or router bit.
    fn generate_gcode(
        &self,
        distance_per_step: f64,
        commands: &mut Vec<GCommand>,
        tool_diameter: f64,
    ) {
        let tool_radius = tool_diameter / 2.0;
        let inner_diameter = self.diameter - tool_radius;
        let inner_radius = inner_diameter / 2.0;

        let starting_point = self.position + Vector2::new(inner_radius, 0.0);

        commands.push(GCommand::MoveTo {
            target: (
                Length::new::<millimeter>(starting_point.x),
                Length::new::<millimeter>(starting_point.y),
            ),
        });

        let arch_length = std::f64::consts::PI * 2.0 * inner_radius;
        let steps = (arch_length / distance_per_step).ceil();

        let angle_step = std::f64::consts::PI * 2.0 / steps;

        let steps = steps as usize;

        for step_index in 0..steps {
            let angle = angle_step * step_index as f64;

            let (sin, cos) = angle.sin_cos();
            let offset = Vector2::new(cos, sin) * inner_radius;

            let new_position = self.position + offset;
            commands.push(GCommand::Cut {
                movement: MovementType::Linear,
                target: (
                    Length::new::<millimeter>(new_position.x),
                    Length::new::<millimeter>(new_position.y),
                ),
            });
        }

        commands.push(GCommand::Cut {
            movement: MovementType::Linear,
            target: (
                Length::new::<millimeter>(starting_point.x),
                Length::new::<millimeter>(starting_point.y),
            ),
        });
    }
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
            UnitMode::Imperial => Length::<uom::si::SI<f64>, f64>::new::<inch>(axis),
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
