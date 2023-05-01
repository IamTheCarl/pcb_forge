use std::{collections::HashMap, fs, path::Path};

use anyhow::{bail, Context, Result};
use nalgebra::Vector2;
use uom::si::{
    length::{inch, millimeter, Length},
    ratio::{percent, Ratio},
    velocity::{millimeter_per_second, Velocity},
};

use crate::{
    config::machine::JobConfig,
    gcode_generation::{GCommand, MovementType, ToolSelection},
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
    pub fn generate_gcode(
        &self,
        commands: &mut Vec<GCommand>,
        job_config: &JobConfig,
        tool_config: &ToolSelection,
    ) -> Result<()> {
        match job_config.tool_power {
            crate::config::machine::ToolConfig::Laser {
                laser_power,
                work_speed,
            } => {
                let distance_per_step = job_config.distance_per_step.get::<millimeter>();

                if let ToolSelection::Laser { laser } = tool_config {
                    commands.extend(
                        [
                            GCommand::EquipLaser {
                                max_power: laser.max_power,
                            },
                            GCommand::AbsoluteMode,
                            GCommand::UnitMode(UnitMode::Metric),
                            GCommand::SetRapidTransverseSpeed(
                                Velocity::new::<millimeter_per_second>(
                                    3000.0, // TODO this should come from the config file.
                                ),
                            ),
                            GCommand::SetWorkSpeed(work_speed),
                            GCommand::SetPower(laser_power),
                            GCommand::SetFanPower {
                                index: 0,
                                power: Ratio::new::<percent>(100.0), // TODO fan configurations should come from the machine config.
                            },
                        ]
                        .iter()
                        .cloned(),
                    );

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

                        hole.generate_gcode(
                            distance_per_step,
                            commands,
                            tool_config.diameter().get::<millimeter>(),
                        );
                        last_position = hole.position;
                    }

                    // TODO render paths.

                    commands.push(GCommand::RemoveTool);

                    Ok(())
                } else {
                    bail!("Job was configured for a laser but selected tool is not a laser.");
                }
            }
            crate::config::machine::ToolConfig::Drill {
                spindle_rpm,
                plunge_speed,
            } => bail!("drilling drill files is not yet supported"),
            crate::config::machine::ToolConfig::EndMill {
                spindle_rpm,
                max_cut_depth,
                plunge_speed,
                work_speed,
            } => bail!("milling drill files is not yet supported"),
        }
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
        let intermediate_point = self.position - Vector2::new(inner_radius, 0.0);

        commands.push(GCommand::MoveTo {
            target: (
                Length::new::<millimeter>(starting_point.x),
                Length::new::<millimeter>(starting_point.y),
            ),
        });
        // Works right on simulators, works wrong on actual machine.
        // commands.push(GCommand::Cut {
        //     movement: MovementType::ClockwiseCurve(CurveType::Center(
        //         Length::new::<millimeter>(self.position.x),
        //         Length::new::<millimeter>(self.position.y),
        //     )),
        //     target: (
        //         Length::new::<millimeter>(starting_point.x),
        //         Length::new::<millimeter>(starting_point.y),
        //     ),
        // });

        let center_to_start = starting_point;

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

        // Use an approximation for now.
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
