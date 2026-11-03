//! Thermal Zone Temperature from ACPI, via the performance counter
//! `\Thermal Zone Information(*)\Temperature` (Kelvin). Readable without
//! administrator rights. Usually one zone per board; it is not a Core
//! Temperature. Core Temperature needs the Sensor Provider (PawnIO) phase.

use crate::pdh::CounterQuery;
use anyhow::Result;

#[derive(Debug, Clone, PartialEq)]
pub struct ThermalZone {
    pub name: String,
    pub celsius: f64,
}

pub struct ThermalSampler {
    query: CounterQuery,
}

impl ThermalSampler {
    /// `Ok(None)` when the machine exposes no thermal zone counters.
    pub fn new() -> Result<Option<Self>> {
        match CounterQuery::new(&[r"\Thermal Zone Information(*)\Temperature"]) {
            Ok(query) => Ok(Some(Self { query })),
            Err(_) => Ok(None),
        }
    }

    pub fn sample(&mut self) -> Result<Vec<ThermalZone>> {
        self.query.collect()?;
        let zones = self
            .query
            .values(0)?
            .into_iter()
            .filter(|(name, _)| name != "_Total")
            .map(|(name, kelvin)| ThermalZone {
                name: name.trim_start_matches('\\').to_string(),
                celsius: kelvin - 273.15,
            })
            .collect();
        Ok(zones)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_zone_values_are_plausible() {
        let Some(mut s) = ThermalSampler::new().unwrap() else {
            return; // no thermal zone on this machine
        };
        let zones = s.sample().unwrap();
        for z in zones {
            assert!((-20.0..=120.0).contains(&z.celsius), "{z:?}");
        }
    }
}
