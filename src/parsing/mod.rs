use serde::{Deserialize, Deserializer};
use std::str::FromStr;
use uom::si::{
    length::{units, Units},
    Quantity,
};

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

pub fn parse_length_unit<'de, DE>(deserializer: DE) -> Result<Units, DE::Error>
where
    DE: Deserializer<'de>,
{
    use serde::de::Error;

    let s = String::deserialize(deserializer)?;
    for unit in units() {
        if s == unit.abbreviation() || s == unit.singular() || s == unit.plural() {
            return Ok(unit);
        }
    }

    Err(Error::unknown_variant(&s, &["mm", "mil", "in"])) // TODO there are a lot more units we support than this.
}
