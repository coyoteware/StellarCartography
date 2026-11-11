//! The commander's ship, read from the journal's `Loadout` event, reduced to
//! the frame-shift-drive fuel model that Spansh's galaxy plotter asks for
//! (<https://spansh.co.uk/exact-plotter>).
//!
//! The journal names the drive (`int_hyperdrive_size5_class5`, or
//! `int_hyperdrive_overcharge_...` for an SCO drive) but only carries its
//! *engineered* numbers as modifiers, so the base stats come from a built-in
//! table (the same numbers Coriolis and EDSY use). Engineering overrides the
//! optimal mass (Increased Range, Mass Manager) and the fuel ceiling (Deep
//! Charge) where the journal says so.

use serde::{Deserialize, Serialize};

/// Everything the galaxy plotter needs to know about a ship, plus labels for
/// showing which ship it is.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShipSpec {
    /// When the loadout was written (ISO).
    pub timestamp: String,
    /// Internal ship type ("corsair", "python_nx", ...).
    pub ship: String,
    /// The name the player gave it, often empty.
    pub ship_name: String,
    /// The six-character ID plate.
    pub ship_ident: String,
    /// Hull and modules without fuel or cargo (t).
    pub base_mass: f64,
    /// Main fuel tank (t).
    pub tank_size: f64,
    /// Reserve tank (t).
    pub internal_tank_size: f64,
    pub cargo_capacity: u32,
    /// The game's own current-fuel maximum range (ly), a handy default.
    pub max_jump_range: f64,
    /// FSD fuel curve: fuel = multiplier * (distance * mass / optimal_mass) ^ power.
    pub fuel_power: f64,
    pub fuel_multiplier: f64,
    pub optimal_mass: f64,
    pub max_fuel_per_jump: f64,
    /// Guardian FSD booster range (ly), 0 without one.
    pub range_boost: f64,
    /// Neutron-cone range multiplier (4 for every drive).
    pub supercharge_multiplier: f64,
}

#[derive(Deserialize)]
struct RawLoadout {
    timestamp: Option<String>,
    #[serde(rename = "Ship")]
    ship: Option<String>,
    #[serde(rename = "ShipName", default)]
    ship_name: String,
    #[serde(rename = "ShipIdent", default)]
    ship_ident: String,
    #[serde(rename = "UnladenMass")]
    unladen_mass: Option<f64>,
    #[serde(rename = "CargoCapacity", default)]
    cargo_capacity: u32,
    #[serde(rename = "MaxJumpRange", default)]
    max_jump_range: f64,
    #[serde(rename = "FuelCapacity")]
    fuel_capacity: Option<FuelCapacity>,
    #[serde(rename = "Modules", default)]
    modules: Vec<RawModule>,
}

#[derive(Deserialize)]
struct FuelCapacity {
    #[serde(rename = "Main")]
    main: f64,
    #[serde(rename = "Reserve", default)]
    reserve: f64,
}

#[derive(Deserialize)]
struct RawModule {
    #[serde(rename = "Slot", default)]
    slot: String,
    #[serde(rename = "Item", default)]
    item: String,
    #[serde(rename = "Engineering")]
    engineering: Option<RawEngineering>,
}

#[derive(Deserialize)]
struct RawEngineering {
    #[serde(rename = "Modifiers", default)]
    modifiers: Vec<RawModifier>,
}

#[derive(Deserialize)]
struct RawModifier {
    #[serde(rename = "Label", default)]
    label: String,
    #[serde(rename = "Value")]
    value: Option<f64>,
}

/// Base stats of one drive: (fuel power, fuel multiplier, optimal mass, max fuel per jump).
type FsdStats = (f64, f64, f64, f64);

/// Base stats for `int_hyperdrive_size<size>_class<class>` (class 1 = E ... 5 = A).
fn fsd_stats(size: u32, class: u32) -> Option<FsdStats> {
    let power = fuel_power(size)?;
    let multiplier = [0.011, 0.010, 0.008, 0.010, 0.012].get(class as usize - 1)?;
    #[rustfmt::skip]
    let (optimal, max_fuel): ([f64; 5], [f64; 5]) = match size {
        2 => ([48.0, 54.0, 60.0, 75.0, 90.0],           [0.6, 0.6, 0.6, 0.8, 0.9]),
        3 => ([80.0, 90.0, 100.0, 125.0, 150.0],        [1.2, 1.2, 1.2, 1.5, 1.8]),
        4 => ([280.0, 315.0, 350.0, 437.5, 525.0],      [2.0, 2.0, 2.0, 2.5, 3.0]),
        5 => ([560.0, 630.0, 700.0, 875.0, 1050.0],     [3.3, 3.3, 3.3, 4.1, 5.0]),
        6 => ([960.0, 1080.0, 1200.0, 1500.0, 1800.0],  [5.3, 5.3, 5.3, 6.6, 8.0]),
        7 => ([1440.0, 1620.0, 1800.0, 2250.0, 2700.0], [8.5, 8.5, 8.5, 10.6, 12.8]),
        _ => return None,
    };
    Some((power, *multiplier, optimal[class as usize - 1], max_fuel[class as usize - 1]))
}

/// Base stats for the SCO drives, `int_hyperdrive_overcharge_size<size>_class<class>`.
fn fsd_overcharge_stats(size: u32, class: u32) -> Option<FsdStats> {
    let power = fuel_power(size)?;
    let multiplier = [0.008, 0.012, 0.012, 0.012, 0.013].get(class as usize - 1)?;
    #[rustfmt::skip]
    let (optimal, max_fuel): ([f64; 5], [f64; 5]) = match size {
        2 => ([60.0, 90.0, 90.0, 90.0, 100.0],              [0.6, 0.9, 0.9, 0.9, 1.0]),
        3 => ([100.0, 150.0, 150.0, 150.0, 167.0],          [1.2, 1.8, 1.8, 1.8, 1.9]),
        4 => ([350.0, 525.0, 525.0, 525.0, 585.0],          [2.0, 3.0, 3.0, 3.0, 3.2]),
        5 => ([700.0, 1050.0, 1050.0, 1050.0, 1175.0],      [3.3, 5.0, 5.0, 5.0, 5.2]),
        6 => ([1200.0, 1800.0, 1800.0, 1800.0, 2000.0],     [5.3, 8.0, 8.0, 8.0, 8.3]),
        7 => ([1800.0, 2700.0, 2700.0, 2700.0, 3000.0],     [8.5, 12.8, 12.8, 12.8, 13.1]),
        8 => ([2800.0, 4200.0, 4200.0, 4200.0, 4670.0],     [13.6, 20.4, 20.4, 20.4, 20.7]),
        _ => return None,
    };
    Some((power, *multiplier, optimal[class as usize - 1], max_fuel[class as usize - 1]))
}

fn fuel_power(size: u32) -> Option<f64> {
    (2..=8).contains(&size).then(|| 2.0 + 0.15 * (size as f64 - 2.0))
}

/// Guardian FSD booster range by module size.
fn booster_range(size: u32) -> f64 {
    [4.0, 6.0, 7.75, 9.25, 10.5].get(size as usize - 1).copied().unwrap_or(0.0)
}

/// `int_hyperdrive_size5_class5` → (false, 5, 5); `int_hyperdrive_overcharge_...` → (true, ...).
/// Pre-engineered variants carry a suffix after the class, which is ignored:
/// their differences arrive as engineering modifiers.
fn parse_fsd_item(item: &str) -> Option<(bool, u32, u32)> {
    let rest = item.strip_prefix("int_hyperdrive_")?;
    let (overcharge, rest) = match rest.strip_prefix("overcharge_") {
        Some(rest) => (true, rest),
        None => (false, rest),
    };
    let rest = rest.strip_prefix("size")?;
    let (size, rest) = rest.split_once("_class")?;
    let class: String = rest.chars().take_while(char::is_ascii_digit).collect();
    Some((overcharge, size.parse().ok()?, class.parse().ok()?))
}

/// Reads one journal line; `None` unless it's a usable `Loadout` event (one
/// with a mass, a tank and a drive the table knows).
pub fn parse_loadout(line: &str) -> Option<ShipSpec> {
    if !line.contains("\"Loadout\"") {
        return None;
    }
    let raw: RawLoadout = serde_json::from_str(line).ok()?;
    let fuel = raw.fuel_capacity?;
    let fsd = raw.modules.iter().find(|m| m.slot == "FrameShiftDrive")?;
    let (overcharge, size, class) = parse_fsd_item(&fsd.item.to_lowercase())?;
    let (fuel_power, fuel_multiplier, mut optimal_mass, mut max_fuel_per_jump) = if overcharge {
        fsd_overcharge_stats(size, class)?
    } else {
        fsd_stats(size, class)?
    };
    for m in fsd.engineering.iter().flat_map(|e| &e.modifiers) {
        match (m.label.as_str(), m.value) {
            ("FSDOptimalMass", Some(v)) => optimal_mass = v,
            ("MaxFuelPerJump", Some(v)) => max_fuel_per_jump = v,
            _ => {}
        }
    }
    let range_boost = raw
        .modules
        .iter()
        .filter_map(|m| m.item.to_lowercase().strip_prefix("int_guardianfsdbooster_size")?.parse().ok())
        .map(booster_range)
        .next_back()
        .unwrap_or(0.0);
    Some(ShipSpec {
        timestamp: raw.timestamp.unwrap_or_default(),
        ship: raw.ship?,
        ship_name: raw.ship_name,
        ship_ident: raw.ship_ident,
        base_mass: raw.unladen_mass?,
        tank_size: fuel.main,
        internal_tank_size: fuel.reserve,
        cargo_capacity: raw.cargo_capacity,
        max_jump_range: raw.max_jump_range,
        fuel_power,
        fuel_multiplier,
        optimal_mass,
        max_fuel_per_jump,
        range_boost,
        supercharge_multiplier: 4.0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A trimmed Loadout: an engineered SCO 5A drive and a guardian booster.
    const LOADOUT: &str = r#"{ "timestamp":"2026-11-10T05:38:35Z", "event":"Loadout", "Ship":"corsair", "ShipName":"Wanderer", "ShipIdent":"CO-31C", "UnladenMass":738.099976, "CargoCapacity":64, "MaxJumpRange":29.397066, "FuelCapacity":{ "Main":32.0, "Reserve":0.41 }, "Modules":[ { "Slot":"FrameShiftDrive", "Item":"int_hyperdrive_overcharge_size5_class5", "On":true, "Priority":0, "Engineering":{ "BlueprintName":"FSD_LongRange", "Level":5, "Modifiers":[ { "Label":"Mass", "Value":26.0, "OriginalValue":20.0 }, { "Label":"FSDOptimalMass", "Value":1894.099976, "OriginalValue":1175.0 } ] } }, { "Slot":"Slot04_Size3", "Item":"int_guardianfsdbooster_size3", "On":true, "Priority":0 } ] }"#;

    #[test]
    fn reads_an_engineered_loadout() {
        let s = parse_loadout(LOADOUT).unwrap();
        assert_eq!(s.ship, "corsair");
        assert_eq!(s.ship_name, "Wanderer");
        assert_eq!(s.ship_ident, "CO-31C");
        assert_eq!(s.base_mass, 738.099976);
        assert_eq!(s.tank_size, 32.0);
        assert_eq!(s.internal_tank_size, 0.41);
        assert_eq!(s.cargo_capacity, 64);
        assert_eq!(s.fuel_power, 2.45);
        assert_eq!(s.fuel_multiplier, 0.013, "SCO A-rated burns hotter");
        assert_eq!(s.optimal_mass, 1894.099976, "engineering wins over the base 1175");
        assert_eq!(s.max_fuel_per_jump, 5.2, "not modified, so the base value");
        assert_eq!(s.range_boost, 7.75);
        assert_eq!(s.supercharge_multiplier, 4.0);
    }

    #[test]
    fn plain_drives_use_the_regular_table() {
        let line = LOADOUT
            .replace("int_hyperdrive_overcharge_size5_class5", "int_hyperdrive_size5_class5")
            .replace(r#""Engineering":{ "BlueprintName":"FSD_LongRange", "Level":5, "Modifiers":[ { "Label":"Mass", "Value":26.0, "OriginalValue":20.0 }, { "Label":"FSDOptimalMass", "Value":1894.099976, "OriginalValue":1175.0 } ] }"#, r#""Health":1.0"#);
        let s = parse_loadout(&line).unwrap();
        assert_eq!((s.fuel_power, s.fuel_multiplier, s.optimal_mass, s.max_fuel_per_jump), (2.45, 0.012, 1050.0, 5.0));
    }

    #[test]
    fn fsd_names_parse() {
        assert_eq!(parse_fsd_item("int_hyperdrive_size2_class1"), Some((false, 2, 1)));
        assert_eq!(parse_fsd_item("int_hyperdrive_overcharge_size8_class5"), Some((true, 8, 5)));
        // Pre-engineered drives carry a suffix; the modifiers carry their stats.
        assert_eq!(parse_fsd_item("int_hyperdrive_overcharge_size8_class5_overchargebooster_mkii"), Some((true, 8, 5)));
        assert_eq!(parse_fsd_item("int_engine_size5_class5"), None);
        assert_eq!(parse_fsd_item("int_hyperdrive_size9_class1").and_then(|(o, s, c)| if o { fsd_overcharge_stats(s, c) } else { fsd_stats(s, c) }), None);
    }

    #[test]
    fn non_loadout_lines_are_skipped() {
        assert_eq!(parse_loadout(r#"{ "event":"Music" }"#), None);
        assert_eq!(parse_loadout("{ \"event\":\"Loadout\", broken"), None);
        // A loadout without an FSD (taxi, on foot) is no use for plotting.
        assert_eq!(parse_loadout(r#"{ "event":"Loadout", "Ship":"corsair", "UnladenMass":1.0, "FuelCapacity":{"Main":2.0}, "Modules":[] }"#), None);
    }
}
