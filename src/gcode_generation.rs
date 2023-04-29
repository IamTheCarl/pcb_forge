use std::fmt::Display;

use uom::si::{
    f32::Ratio,
    length::{mil, millimeter, Length},
    power::Power,
    ratio::ratio,
    velocity::{inch_per_second, millimeter_per_second, Velocity},
};

use crate::{
    config::machine::{LaserConfig, SpindleBit, SpindleConfig},
    parsing::gerber::UnitMode,
};

pub enum GCommand {
    AbsoluteMode,
    SetRapidTransverseSpeed(Velocity<uom::si::SI<f32>, f32>),
    SetWorkSpeed(Velocity<uom::si::SI<f32>, f32>),
    SetPower(Power<uom::si::SI<f32>, f32>),
    Cut {
        movement: MovementType,
        target: (Length<uom::si::SI<f32>, f32>, Length<uom::si::SI<f32>, f32>),
    },
    MoveTo {
        target: (Length<uom::si::SI<f32>, f32>, Length<uom::si::SI<f32>, f32>),
    },
    UnitMode(UnitMode),
    SetFanPower {
        index: usize,
        power: Ratio,
    },
}

pub enum MovementType {
    Linear,
    ClockwiseCurve {
        diameter: Length<uom::si::SI<f32>, f32>,
    },
    CounterClockwiseCurve {
        diameter: Length<uom::si::SI<f32>, f32>,
    },
}

pub struct GCodeFile {
    max_power: Power<uom::si::SI<f32>, f32>,
    commands: Vec<GCommand>,
}

impl Display for GCodeFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut unit_mode = UnitMode::Metric;

        for command in self.commands.iter() {
            match command {
                GCommand::AbsoluteMode => writeln!(f, "G90"),
                GCommand::SetRapidTransverseSpeed(speed) => writeln!(
                    f,
                    "G0 F{}",
                    match unit_mode {
                        UnitMode::Metric => speed.get::<millimeter_per_second>(),
                        UnitMode::Imperial => speed.get::<inch_per_second>(),
                    }
                ),
                GCommand::SetWorkSpeed(speed) => writeln!(
                    f,
                    "G1 F{}",
                    match unit_mode {
                        UnitMode::Metric => speed.get::<millimeter_per_second>(),
                        UnitMode::Imperial => speed.get::<inch_per_second>(),
                    }
                ),
                GCommand::SetPower(power) => {
                    let power_ratio = *power / self.max_power;
                    let percentage = (100.0 * power_ratio.get::<ratio>()) as usize;
                    let pwm_scale = (255.0 * power_ratio.get::<ratio>()) as usize;

                    writeln!(f, "M3 P{} S{}", percentage, pwm_scale)
                }
                GCommand::Cut {
                    movement,
                    target: (x, y),
                } => {
                    writeln!(f, "M3")?;

                    let (x, y) = match unit_mode {
                        UnitMode::Metric => (x.get::<millimeter>(), y.get::<millimeter>()),
                        UnitMode::Imperial => (x.get::<mil>(), y.get::<mil>()),
                    };

                    match movement {
                        MovementType::Linear => writeln!(f, "G1 X{} Y{}", x, y),
                        MovementType::ClockwiseCurve { diameter } => {
                            let radius = *diameter / 2.0;
                            let radius = match unit_mode {
                                UnitMode::Metric => radius.get::<millimeter>(),
                                UnitMode::Imperial => radius.get::<mil>(),
                            };

                            writeln!(f, "G2 X{} Y{} R{}", x, y, radius)
                        }
                        MovementType::CounterClockwiseCurve { diameter } => {
                            let radius = *diameter / 2.0;
                            let radius = match unit_mode {
                                UnitMode::Metric => radius.get::<millimeter>(),
                                UnitMode::Imperial => radius.get::<mil>(),
                            };

                            writeln!(f, "G3 X{} Y{} R{}", x, y, radius)
                        }
                    }
                }
                GCommand::MoveTo { target: (x, y) } => {
                    writeln!(f, "M5")?;

                    let (x, y) = match unit_mode {
                        UnitMode::Metric => (x.get::<millimeter>(), y.get::<millimeter>()),
                        UnitMode::Imperial => (x.get::<mil>(), y.get::<mil>()),
                    };

                    writeln!(f, "G0 X{} Y{}", x, y)
                }
                GCommand::UnitMode(new_mode) => {
                    unit_mode = *new_mode;
                    match new_mode {
                        UnitMode::Metric => writeln!(f, "G21"),
                        UnitMode::Imperial => writeln!(f, "G22"),
                    }
                }
                GCommand::SetFanPower { index, power } => {
                    if *power > Ratio::new::<ratio>(0.0) {
                        let power = (255.0 * power.get::<ratio>()) as usize;
                        writeln!(f, "G106 P{}, S{}", index, power)
                    } else {
                        writeln!(f, "G107 P{}", index)
                    }
                }
            }?;
        }

        Ok(())
    }
}

impl GCodeFile {
    pub fn new(max_power: Power<uom::si::SI<f32>, f32>, commands: Vec<GCommand>) -> Self {
        Self {
            max_power,
            commands,
        }
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
    pub fn diameter(&self) -> Length<uom::si::SI<f32>, f32> {
        match self {
            ToolSelection::Laser { laser } => laser.point_diameter,
            ToolSelection::Spindle { spindle: _, bit } => match bit {
                SpindleBit::Drill { diameter } => *diameter,
                SpindleBit::EndMill { diameter } => *diameter,
            },
        }
    }
}
