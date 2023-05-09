//! Tools to generate GCode.
//! Fantastic documentation of GCode commands can be found [here](https://marlinfw.org/meta/gcode/).

use std::{fmt::Write, fs, path::PathBuf};

use anyhow::{bail, Context, Result};
use geo::Coord;
use uom::{
    num_traits::Zero,
    si::{
        angular_velocity::AngularVelocity,
        length::{mil, millimeter, Length},
        power::Power,
        ratio::ratio,
        velocity::{inch_per_second, millimeter_per_second, Velocity},
    },
};

use crate::{
    config::machine::{JobConfig, LaserConfig, Machine, SpindleBit, SpindleConfig},
    parsing::UnitMode,
};

#[derive(Debug, Clone, Copy)]
pub enum Tool {
    None,
    Laser {
        max_power: Power<uom::si::SI<f64>, f64>,
    },
    Spindle {
        max_spindle_speed: AngularVelocity<uom::si::SI<f64>, f64>,
        plunge_speed: Velocity<uom::si::SI<f64>, f64>,
        travel_height: Length<uom::si::SI<f64>, f64>,
        cut_depth: Length<uom::si::SI<f64>, f64>,
        pass_depth: Option<Length<uom::si::SI<f64>, f64>>,
    },
}

#[derive(Debug, Clone)]
pub enum GCommand {
    EquipTool(Tool),
    SetRapidTransverseSpeed(Velocity<uom::si::SI<f64>, f64>),
    SetWorkSpeed(Velocity<uom::si::SI<f64>, f64>),
    SetPower(Power<uom::si::SI<f64>, f64>),
    SetSpindleSpeed(AngularVelocity<uom::si::SI<f64>, f64>),
    Cut {
        pass_index: usize,
        movement: MovementType,
        target: (Length<uom::si::SI<f64>, f64>, Length<uom::si::SI<f64>, f64>),
    },
    MoveTo {
        target: (Length<uom::si::SI<f64>, f64>, Length<uom::si::SI<f64>, f64>),
    },
    UnitMode(UnitMode),
    IncludeFile(PathBuf),
    SetSide(BoardSide),
}

#[derive(Debug, Clone, Copy)]
pub enum BoardSide {
    Front,
    Back,
}

#[derive(Debug, Clone)]
pub enum MovementType {
    Linear,
}

pub struct GCodeFile {
    commands: Vec<GCommand>,
}

impl GCodeFile {
    pub fn to_string(&self, x_offset: Length<uom::si::SI<f64>, f64>) -> Result<String> {
        let mut unit_mode = UnitMode::Metric;
        let mut board_side = BoardSide::Front;
        let mut tool_is_ready_to_cut = false;
        let mut work_speed = Velocity::zero();

        let x_offset = match unit_mode {
            UnitMode::Metric => x_offset.get::<millimeter>(),
            UnitMode::Imperial => x_offset.get::<mil>(),
        };

        let mut tool = Tool::None;

        let mut output = String::default();

        // Put the machine into absolute mode.
        writeln!(&mut output, "G90")?;

        // Move the X-Y axis to the origin so we can lower with minimized risk of hitting a clamp
        // and be confident of our starting position.
        writeln!(&mut output, "G0 X0 Y0")?;

        let mut position = (
            Length::<uom::si::SI<f64>, f64>::zero(),
            Length::<uom::si::SI<f64>, f64>::zero(),
        );

        for command in self.commands.iter() {
            match command {
                GCommand::EquipTool(new_tool) => {
                    // Disengage the tool.
                    match tool {
                        Tool::None => {} // Nothing needs to be done.
                        Tool::Laser { max_power: _ } => {
                            if tool_is_ready_to_cut {
                                writeln!(&mut output, "M5")?;
                                tool_is_ready_to_cut = false;
                            }
                        }
                        Tool::Spindle {
                            max_spindle_speed: _,
                            travel_height,
                            cut_depth: _,
                            pass_depth: _,
                            plunge_speed: _,
                        } => {
                            if tool_is_ready_to_cut {
                                writeln!(
                                    &mut output,
                                    "G0 Z{}",
                                    match unit_mode {
                                        UnitMode::Metric => travel_height.get::<millimeter>(),
                                        UnitMode::Imperial => travel_height.get::<mil>(),
                                    }
                                )?;
                                tool_is_ready_to_cut = false;
                            }
                        }
                    }

                    tool = *new_tool;

                    // Make sure that tool is still disengaged.
                    match tool {
                        Tool::None => {} // Nothing needs to be done.
                        Tool::Laser { max_power: _ } => {
                            writeln!(&mut output, "M5")?;
                            tool_is_ready_to_cut = false;
                        }
                        Tool::Spindle {
                            max_spindle_speed: _,
                            travel_height,
                            cut_depth: _,
                            pass_depth: _,
                            plunge_speed: _,
                        } => {
                            writeln!(
                                &mut output,
                                "G0 Z{}",
                                match unit_mode {
                                    UnitMode::Metric => travel_height.get::<millimeter>(),
                                    UnitMode::Imperial => travel_height.get::<mil>(),
                                }
                            )?;
                            tool_is_ready_to_cut = false;
                        }
                    }

                    Ok(())
                }
                GCommand::SetRapidTransverseSpeed(speed) => writeln!(
                    &mut output,
                    "G0 F{}",
                    match unit_mode {
                        UnitMode::Metric => speed.get::<millimeter_per_second>(),
                        UnitMode::Imperial => speed.get::<inch_per_second>(),
                    }
                ),
                GCommand::SetWorkSpeed(speed) => {
                    work_speed = *speed;
                    writeln!(
                        &mut output,
                        "G1 F{}",
                        match unit_mode {
                            UnitMode::Metric => speed.get::<millimeter_per_second>(),
                            UnitMode::Imperial => speed.get::<inch_per_second>(),
                        }
                    )
                }
                GCommand::SetPower(power) => {
                    if let Tool::Laser { max_power } = &tool {
                        let power_ratio = *power / *max_power;
                        let percentage = (100.0 * power_ratio.get::<ratio>()) as usize;
                        let pwm_scale = (255.0 * power_ratio.get::<ratio>()) as usize;

                        tool_is_ready_to_cut = false;
                        writeln!(&mut output, "M3 P{} S{}", percentage, pwm_scale)?;
                        writeln!(&mut output, "M5") // Don't power on the laser just yet.
                    } else {
                        bail!("Attempt to set power of non-laser tool.");
                    }
                }
                GCommand::SetSpindleSpeed(speed) => {
                    if let Tool::Spindle {
                        max_spindle_speed,
                        travel_height: _,
                        cut_depth: _,
                        pass_depth: _,
                        plunge_speed: _,
                    } = &tool
                    {
                        let power_ratio = *speed / *max_spindle_speed;
                        let percentage = (100.0 * power_ratio.get::<ratio>().abs()) as usize;
                        let pwm_scale = (255.0 * power_ratio.get::<ratio>().abs()) as usize;

                        // Note that we let the tool start spinning immediately.
                        tool_is_ready_to_cut = false;
                        if power_ratio.is_sign_positive() {
                            writeln!(&mut output, "M3 P{} S{}", percentage, pwm_scale)
                        } else {
                            writeln!(&mut output, "M4 P{} S{}", percentage, pwm_scale)
                        }
                    } else {
                        bail!("Attempt to set speed of non-spindle tool.");
                    }
                }
                GCommand::Cut {
                    pass_index,
                    movement,
                    target: (x, y),
                } => {
                    match tool {
                        Tool::None => bail!("No tool is equipped."),
                        Tool::Laser { max_power: _ } => {
                            if !tool_is_ready_to_cut {
                                writeln!(&mut output, "M3")?;
                                tool_is_ready_to_cut = true;
                            }
                        }
                        Tool::Spindle {
                            max_spindle_speed: _,
                            travel_height,
                            cut_depth,
                            pass_depth,
                            plunge_speed,
                        } => {
                            if !tool_is_ready_to_cut {
                                let target_depth = pass_depth.map_or(cut_depth, |pass_depth| {
                                    travel_height - pass_depth * *pass_index as f64
                                });

                                writeln!(
                                    &mut output,
                                    "G1 Z{} F{}",
                                    match unit_mode {
                                        UnitMode::Metric => target_depth.get::<millimeter>(),
                                        UnitMode::Imperial => target_depth.get::<mil>(),
                                    },
                                    match unit_mode {
                                        UnitMode::Metric =>
                                            plunge_speed.get::<millimeter_per_second>(),
                                        UnitMode::Imperial => plunge_speed.get::<inch_per_second>(),
                                    }
                                )?;
                                writeln!(
                                    &mut output,
                                    "G1 F{}",
                                    match unit_mode {
                                        UnitMode::Metric =>
                                            work_speed.get::<millimeter_per_second>(),
                                        UnitMode::Imperial => work_speed.get::<inch_per_second>(),
                                    }
                                )?;
                                tool_is_ready_to_cut = true;
                            }
                        }
                    }

                    position = (*x, *y);

                    let (x, y) = match unit_mode {
                        UnitMode::Metric => (x.get::<millimeter>(), y.get::<millimeter>()),
                        UnitMode::Imperial => (x.get::<mil>(), y.get::<mil>()),
                    };

                    let x = match board_side {
                        BoardSide::Front => x,
                        BoardSide::Back => -x + x_offset,
                    };

                    match movement {
                        MovementType::Linear => writeln!(&mut output, "G1 X{} Y{}", x, y),
                    }
                }
                GCommand::MoveTo { target: (x, y) } => {
                    if position != (*x, *y) {
                        match tool {
                            Tool::None => bail!("No tool is equipped."),
                            Tool::Laser { max_power: _ } => {
                                if tool_is_ready_to_cut {
                                    writeln!(&mut output, "M5")?;
                                    tool_is_ready_to_cut = false;
                                }
                            }
                            Tool::Spindle {
                                max_spindle_speed: _,
                                travel_height,
                                cut_depth: _,
                                pass_depth: _,
                                plunge_speed: _,
                            } => {
                                if tool_is_ready_to_cut {
                                    writeln!(
                                        &mut output,
                                        "G0 Z{}",
                                        match unit_mode {
                                            UnitMode::Metric => travel_height.get::<millimeter>(),
                                            UnitMode::Imperial => travel_height.get::<mil>(),
                                        }
                                    )?;
                                    tool_is_ready_to_cut = false;
                                }
                            }
                        }

                        position = (*x, *y);

                        let (x, y) = match unit_mode {
                            UnitMode::Metric => (x.get::<millimeter>(), y.get::<millimeter>()),
                            UnitMode::Imperial => (x.get::<mil>(), y.get::<mil>()),
                        };

                        let x = match board_side {
                            BoardSide::Front => x,
                            BoardSide::Back => -x + x_offset,
                        };

                        writeln!(&mut output, "G0 X{} Y{}", x, y)
                    } else {
                        // We're already there.
                        tool_is_ready_to_cut = false;
                        Ok(())
                    }
                }
                GCommand::UnitMode(new_mode) => {
                    unit_mode = *new_mode;
                    match new_mode {
                        UnitMode::Metric => writeln!(&mut output, "G21"),
                        UnitMode::Imperial => writeln!(&mut output, "G22"),
                    }
                }
                GCommand::IncludeFile(file_path) => {
                    let file_content = fs::read_to_string(file_path)
                        .with_context(|| format!("Failed to read include file: {:?}", file_path))?;

                    output += &file_content;

                    // It must end with a new line.
                    if !output.ends_with('\n') {
                        output += "\n";
                    }

                    Ok(())
                }
                GCommand::SetSide(new_side) => {
                    board_side = *new_side;
                    Ok(())
                }
            }?;
        }

        Ok(output)
    }
}

impl GCodeFile {
    pub fn new(commands: Vec<GCommand>) -> Self {
        Self { commands }
    }
}

pub enum ToolSelection<'a> {
    Laser {
        laser: &'a LaserConfig,
    },
    Spindle {
        spindle: &'a SpindleConfig,
        bit: &'a SpindleBit,
    },
}

impl<'a> ToolSelection<'a> {
    pub fn diameter(&self) -> Length<uom::si::SI<f64>, f64> {
        match self {
            ToolSelection::Laser { laser } => laser.point_diameter,
            ToolSelection::Spindle { spindle: _, bit } => match bit {
                SpindleBit::EndMill { diameter } => *diameter,
            },
        }
    }

    pub fn init_gcode(&self) -> Option<&PathBuf> {
        match self {
            ToolSelection::Laser { laser } => laser.init_gcode.as_ref(),
            ToolSelection::Spindle { spindle, bit: _ } => spindle.init_gcode.as_ref(),
        }
    }

    pub fn shutdown_gcode(&self) -> Option<&PathBuf> {
        match self {
            ToolSelection::Laser { laser } => laser.shutdown_gcode.as_ref(),
            ToolSelection::Spindle { spindle, bit: _ } => spindle.shutdown_gcode.as_ref(),
        }
    }
}

pub struct GCodeConfig<'a> {
    pub commands: &'a mut Vec<GCommand>,
    pub job_config: &'a JobConfig,
    pub tool_config: &'a ToolSelection<'a>,
    pub machine_config: &'a Machine,
    pub include_file_search_directory: PathBuf,
}

pub fn add_point_string_to_gcode_vector<'a>(
    commands: &mut Vec<GCommand>,
    mut point_iter: impl Iterator<Item = &'a Coord<f64>>,
    pass_index: usize,
) {
    if let Some(first_point) = point_iter.next() {
        commands.push(GCommand::MoveTo {
            target: (
                Length::new::<millimeter>(first_point.x),
                Length::new::<millimeter>(first_point.y),
            ),
        })
    }

    for point in point_iter {
        commands.push(GCommand::Cut {
            pass_index,
            movement: MovementType::Linear,
            target: (
                Length::new::<millimeter>(point.x),
                Length::new::<millimeter>(point.y),
            ),
        })
    }
}
