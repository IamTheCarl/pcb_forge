//! Tools to generate GCode.
//! Fantastic documentation of GCode commands can be found [here](https://marlinfw.org/meta/gcode/).

use std::fmt::Write;

use anyhow::{Context, Result};
use uom::si::{
    f64::Ratio,
    length::{mil, millimeter, Length},
    power::Power,
    ratio::ratio,
    velocity::{inch_per_second, millimeter_per_second, Velocity},
};

use crate::{
    config::machine::{JobConfig, LaserConfig, Machine, SpindleBit, SpindleConfig},
    parsing::UnitMode,
};

#[derive(Debug, Clone)]
pub enum GCommand {
    EquipLaser {
        max_power: Power<uom::si::SI<f64>, f64>,
    },
    RemoveTool,
    SetRapidTransverseSpeed(Velocity<uom::si::SI<f64>, f64>),
    SetWorkSpeed(Velocity<uom::si::SI<f64>, f64>),
    SetPower(Power<uom::si::SI<f64>, f64>),
    Cut {
        movement: MovementType,
        target: (Length<uom::si::SI<f64>, f64>, Length<uom::si::SI<f64>, f64>),
    },
    MoveTo {
        target: (Length<uom::si::SI<f64>, f64>, Length<uom::si::SI<f64>, f64>),
    },
    UnitMode(UnitMode),
    SetFanPower {
        index: usize,
        power: Ratio,
    },
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

        let x_offset = match unit_mode {
            UnitMode::Metric => x_offset.get::<millimeter>(),
            UnitMode::Imperial => x_offset.get::<mil>(),
        };

        let mut max_power = None;

        let mut output = String::default();

        // Put the machine into absolute mode.
        writeln!(&mut output, "G90")?;

        for command in self.commands.iter() {
            match command {
                GCommand::EquipLaser {
                    max_power: new_max_power,
                } => {
                    max_power = Some(*new_max_power);
                    Ok(())
                }
                GCommand::RemoveTool => {
                    max_power = None;
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
                GCommand::SetWorkSpeed(speed) => writeln!(
                    &mut output,
                    "G1 F{}",
                    match unit_mode {
                        UnitMode::Metric => speed.get::<millimeter_per_second>(),
                        UnitMode::Imperial => speed.get::<inch_per_second>(),
                    }
                ),
                GCommand::SetPower(power) => {
                    let power_ratio = *power / max_power.context("Laser was not equipped")?;
                    let percentage = (100.0 * power_ratio.get::<ratio>()) as usize;
                    let pwm_scale = (255.0 * power_ratio.get::<ratio>()) as usize;

                    writeln!(&mut output, "M3 P{} S{}", percentage, pwm_scale)
                }
                GCommand::Cut {
                    movement,
                    target: (x, y),
                } => {
                    writeln!(&mut output, "M3")?;

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
                    writeln!(&mut output, "M5")?;

                    let (x, y) = match unit_mode {
                        UnitMode::Metric => (x.get::<millimeter>(), y.get::<millimeter>()),
                        UnitMode::Imperial => (x.get::<mil>(), y.get::<mil>()),
                    };

                    let x = match board_side {
                        BoardSide::Front => x,
                        BoardSide::Back => -x + x_offset,
                    };

                    writeln!(&mut output, "G0 X{} Y{}", x, y)
                }
                GCommand::UnitMode(new_mode) => {
                    unit_mode = *new_mode;
                    match new_mode {
                        UnitMode::Metric => writeln!(&mut output, "G21"),
                        UnitMode::Imperial => writeln!(&mut output, "G22"),
                    }
                }
                GCommand::SetFanPower { index, power } => {
                    if *power > Ratio::new::<ratio>(0.0) {
                        let power = (255.0 * power.get::<ratio>()) as usize;
                        writeln!(&mut output, "G106 P{}, S{}", index, power)
                    } else {
                        writeln!(&mut output, "G107 P{}", index)
                    }
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
                SpindleBit::Drill { diameter } => *diameter,
                SpindleBit::EndMill { diameter } => *diameter,
            },
        }
    }
}

pub struct GCodeConfig<'a> {
    pub commands: &'a mut Vec<GCommand>,
    pub job_config: &'a JobConfig,
    pub tool_config: &'a ToolSelection<'a>,
    pub machine_config: &'a Machine,
}
