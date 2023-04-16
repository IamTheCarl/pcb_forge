use camino::Utf8PathBuf;
use std::{collections::HashMap, str::FromStr};
use uom::si::{
    angular_velocity::AngularVelocity, length::Length, power::Power, velocity::Velocity, Quantity,
};

use nalgebra::Vector2;
use serde::{Deserialize, Deserializer};

fn parse_quantity<'de, U, V, D, DE>(deserializer: DE) -> Result<Quantity<D, U, V>, DE::Error>
where
    DE: Deserializer<'de>,
    D: uom::si::Dimension + ?Sized,
    U: uom::si::Units<V> + ?Sized,
    V: uom::num_traits::Num + uom::Conversion<V>,
    Quantity<D, U, V>: FromStr,
    <uom::si::Quantity<D, U, V> as std::str::FromStr>::Err: std::fmt::Debug,
{
    use serde::de::Error;

    let s = String::deserialize(deserializer)?;
    let quantity = Quantity::from_str(&s)
        .map_err(|error| DE::Error::custom(format!("Number formatting: {:?}", error)))?;

    Ok(quantity)
}

#[derive(Debug, Deserialize)]
pub struct Machine {
    pub tools: HashMap<String, Tool>,

    /// Configurations for materials and tools that can be used for engraving.
    pub engraving_configs: HashMap<String, EngravingConfig>,

    /// Configurations for materials and tools that can be used for cutting.
    pub cutting_configs: HashMap<String, CuttingConfig>,

    /// The safe working area of the machine.
    pub workspace_area: WorkspaceSize,
}

#[derive(Debug, Deserialize, Clone, Copy)]
pub struct WorkspaceSize {
    #[serde(deserialize_with = "parse_quantity")]
    pub width: Length<uom::si::SI<f32>, f32>,
    #[serde(deserialize_with = "parse_quantity")]
    pub height: Length<uom::si::SI<f32>, f32>,
}

impl From<WorkspaceSize> for Vector2<Length<uom::si::SI<f32>, f32>> {
    fn from(value: WorkspaceSize) -> Self {
        Self::new(value.width, value.height)
    }
}

#[derive(Debug, Deserialize)]
pub struct EngravingConfig {
    /// The tool installed in the machine. For a milling machine, this would be the bit you installed.
    /// For a laser cutter, this should represent the laser.
    pub tool: Utf8PathBuf,

    /// The power of the tool. The unit depends on the tool.
    #[serde(flatten)]
    pub tool_power: ToolConfig,
}

#[derive(Debug, Deserialize)]
pub struct CuttingConfig {
    /// The tool installed in the machine. For a milling machine, this would be the bit you installed.
    /// For a laser cutter, this should represent the laser.
    pub tool: Utf8PathBuf,

    /// The power of the tool. The unit depends on the tool.
    #[serde(flatten)]
    pub tool_config: ToolConfig,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum ToolConfig {
    Laser {
        #[serde(deserialize_with = "parse_quantity")]
        laser_power: Power<uom::si::SI<f32>, f32>,

        #[serde(deserialize_with = "parse_quantity")]
        work_speed: Velocity<uom::si::SI<f32>, f32>,
    },
    Drill {
        #[serde(deserialize_with = "parse_quantity")]
        spindle_rpm: AngularVelocity<uom::si::SI<f32>, f32>,

        #[serde(deserialize_with = "parse_quantity")]
        plunge_speed: Velocity<uom::si::SI<f32>, f32>,
    },
    EndMill {
        #[serde(deserialize_with = "parse_quantity")]
        spindle_rpm: AngularVelocity<uom::si::SI<f32>, f32>,

        /// The max depth that the end mill should plunge into the board.
        #[serde(deserialize_with = "parse_quantity")]
        max_cut_depth: Length<uom::si::SI<f32>, f32>,

        #[serde(deserialize_with = "parse_quantity")]
        plunge_speed: Velocity<uom::si::SI<f32>, f32>,

        #[serde(deserialize_with = "parse_quantity")]
        work_speed: Velocity<uom::si::SI<f32>, f32>,
    },
}

#[derive(Debug, Deserialize)]
pub enum Tool {
    #[serde(rename = "laser")]
    Laser {
        #[serde(deserialize_with = "parse_quantity")]
        point_diameter: Length<uom::si::SI<f32>, f32>,

        #[serde(deserialize_with = "parse_quantity")]
        max_power: Power<uom::si::SI<f32>, f32>,
    },

    #[serde(rename = "spindle")]
    Spindle {
        #[serde(deserialize_with = "parse_quantity")]
        max_speed: AngularVelocity<uom::si::SI<f32>, f32>,

        bits: HashMap<String, SpindleBit>,
    },
}

#[derive(Debug, Deserialize)]
pub enum SpindleBit {
    #[serde(rename = "drill")]
    Drill {
        #[serde(deserialize_with = "parse_quantity")]
        diameter: Length<uom::si::SI<f32>, f32>,
    },

    #[serde(rename = "end_mill")]
    EndMill {
        #[serde(deserialize_with = "parse_quantity")]
        diameter: Length<uom::si::SI<f32>, f32>,
    },
}
