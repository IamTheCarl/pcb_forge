use nalgebra::Vector2;
use serde::{Deserialize, Deserializer};
use std::str::FromStr;
use uom::si::{length::Length, Quantity};

pub mod drill;
pub mod gerber;

#[derive(Debug, Clone, Copy)]
pub enum UnitMode {
    Metric,
    Imperial,
}

#[derive(Debug)]
pub struct LocationInfo {
    pub line: u32,
    pub column: usize,
}

pub fn parse_quantity<'de, U, V, D, DE>(deserializer: DE) -> Result<Quantity<D, U, V>, DE::Error>
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

// pub fn parse_length_unit<'de, DE>(deserializer: DE) -> Result<Units, DE::Error>
// where
//     DE: Deserializer<'de>,
// {
//     use serde::de::Error;

//     let s = String::deserialize(deserializer)?;
//     for unit in units() {
//         if s == unit.abbreviation() || s == unit.singular() || s == unit.plural() {
//             return Ok(unit);
//         }
//     }

//     Err(Error::unknown_variant(&s, &["mm", "mil", "in"])) // TODO there are a lot more units we support than this.
// }

#[derive(Debug)]
pub struct VectorVolumeBounds {
    pub start: Vector2<Length<uom::si::SI<f64>, f64>>,
    pub end: Vector2<Length<uom::si::SI<f64>, f64>>,
}

pub trait VectorVolume {
    fn bounds(&self) -> VectorVolumeBounds;
    fn is_point_in_volume(&self, point: Vector2<Length<uom::si::SI<f64>, f64>>) -> bool;
    fn cast_line(
        &self,
        start: Vector2<Length<uom::si::SI<f64>, f64>>,
        end: Vector2<Length<uom::si::SI<f64>, f64>>,
    );
    // fn trace_boundaries(&self) -> impl Iterator<>,
}
