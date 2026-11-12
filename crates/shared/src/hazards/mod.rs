//! Hazards: radiation dose, body heat, oxygen and the reactor core.
//!
//! Every formula here is pure and testable. The server applies them once per fixed tick.

use crate::content::types::{BalanceDef, RadiationSpec};

/// A player's hazard exposure for one tick.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct HazardExposure {
    /// Radiation dose added this tick, before protection.
    pub radiation: f32,
    /// Body heat added this tick, before protection.
    pub heat: f32,
    /// Oxygen drained this tick.
    pub oxygen_drain: f32,
    /// Direct health damage this tick.
    pub damage: f32,
}

/// A player's current hazard state.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HazardState {
    /// Accumulated radiation dose. 100 = grain, 300 = no regen, 600 = damage.
    pub dose: f32,
    /// Body heat in degrees Celsius above ambient.
    pub heat: f32,
    /// Oxygen remaining in seconds of breath.
    pub oxygen: f32,
}

impl Default for HazardState {
    fn default() -> Self {
        Self {
            dose: 0.0,
            heat: 0.0,
            oxygen: 45.0,
        }
    }
}

/// Protection worn by a player.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Protection {
    /// Fraction of radiation blocked, 0..1.
    pub radiation_resist: f32,
    /// Fraction of heat blocked, 0..1.
    pub heat_resist: f32,
    /// Whether the player has breathable air supplied.
    pub breathing: bool,
    /// Whether the player is in a decon shower.
    pub decon: bool,
}

impl Protection {
    /// Combines a wearable's protection with a suit's, taking the best of each.
    #[must_use]
    pub fn combine(self, other: Self) -> Self {
        Self {
            radiation_resist: self.radiation_resist.max(other.radiation_resist),
            heat_resist: self.heat_resist.max(other.heat_resist),
            breathing: self.breathing || other.breathing,
            decon: self.decon || other.decon,
        }
    }
}

/// Radiation dose contributed by a source at a distance.
///
/// Falls off with the square of distance beyond one block, and reaches zero at the spec radius.
#[must_use]
pub fn dose_from_source(spec: &RadiationSpec, distance: f32) -> f32 {
    if distance > spec.radius || distance <= 0.0 {
        return 0.0;
    }
    let falloff = if distance <= 1.0 {
        1.0
    } else {
        (1.0 / (distance * distance)).max(0.0)
    };
    // Blend so the source is at full strength up close and tapers to zero at the radius.
    let edge = 1.0 - (distance / spec.radius).powi(2);
    spec.dose_per_sec * falloff * edge.clamp(0.0, 1.0)
}

/// Applies one tick of exposure to a player's hazard state.
///
/// Returns the health damage the hazards caused this tick.
pub fn apply_exposure(
    state: &mut HazardState,
    exposure: &HazardExposure,
    protection: Protection,
    balance: &BalanceDef,
    dt: f32,
) -> f32 {
    // Radiation.
    let dose_gain = exposure.radiation * (1.0 - protection.radiation_resist.clamp(0.0, 1.0));
    state.dose = (state.dose + dose_gain * dt).max(0.0);
    if protection.decon {
        // A decon shower strips dose quickly.
        state.dose = (state.dose - 40.0 * dt).max(0.0);
    } else {
        // Dose decays very slowly on its own.
        state.dose = (state.dose - 0.05 * dt).max(0.0);
    }

    // Body heat rises with exposure and falls back toward ambient.
    let heat_gain = exposure.heat * (1.0 - protection.heat_resist.clamp(0.0, 1.0));
    state.heat += heat_gain * dt;
    let cooling = if protection.heat_resist > 0.5 {
        3.0
    } else {
        1.0
    };
    state.heat = (state.heat - cooling * dt).max(0.0);

    // Oxygen.
    if protection.breathing {
        state.oxygen = balance.max_oxygen;
    } else if exposure.oxygen_drain > 0.0 {
        state.oxygen = (state.oxygen - exposure.oxygen_drain * dt).max(0.0);
    } else {
        state.oxygen = (state.oxygen + balance.oxygen_drain * 3.0 * dt).min(balance.max_oxygen);
    }

    // Damage from the three lethal conditions.
    let mut damage = exposure.damage * dt;
    if state.dose > balance.dose_damage {
        let over = (state.dose - balance.dose_damage) / 100.0;
        damage += balance.dose_damage_rate * (1.0 + over) * dt;
    }
    if state.heat > balance.heat_limit {
        let over = (state.heat - balance.heat_limit) / 10.0;
        damage += balance.heat_damage_rate * (1.0 + over) * dt;
    }
    if state.oxygen <= 0.0 {
        damage += 8.0 * dt;
    }
    damage
}

/// Whether the player can regenerate health this tick.
#[must_use]
pub fn can_regenerate(state: &HazardState, balance: &BalanceDef) -> bool {
    state.dose < balance.dose_no_regen && state.heat <= balance.heat_limit && state.oxygen > 0.0
}

/// Whether the screen should show radiation grain.
#[must_use]
pub fn screen_grain(state: &HazardState, balance: &BalanceDef) -> f32 {
    if state.dose <= balance.dose_grain {
        return 0.0;
    }
    ((state.dose - balance.dose_grain) / balance.dose_damage).clamp(0.0, 1.0)
}

/// The reactor core's live state.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ReactorCore {
    /// Core temperature in degrees Celsius.
    pub temperature: f32,
    /// How far the control rods are withdrawn, 0 (inserted) to 1 (fully out).
    pub rod_withdrawal: f32,
    /// Coolant flow rate, 0..1, supplied by the coolant pump.
    pub coolant_flow: f32,
    /// Whether the reactor is running.
    pub running: bool,
    /// Whether the core has been destroyed by a meltdown.
    pub melted_down: bool,
}

impl Default for ReactorCore {
    fn default() -> Self {
        Self {
            temperature: 300.0,
            rod_withdrawal: 0.0,
            coolant_flow: 0.0,
            running: false,
            melted_down: false,
        }
    }
}

/// What a reactor tick decided.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReactorStatus {
    /// Cold and shut down.
    Offline,
    /// Running at a safe temperature.
    Nominal,
    /// Above the warning line: the alarm sounds.
    Warning,
    /// Above the limit: a scram panel tripped, or the core melted down.
    Scrammed,
    /// The core was destroyed.
    Meltdown,
}

/// Advances the reactor core by one tick.
///
/// Heat comes from fission scaled by rod withdrawal; cooling comes from coolant flow.
/// Above the limit the core scrams if a scram panel exists, otherwise it melts down.
pub fn tick_reactor(
    core: &mut ReactorCore,
    balance: &BalanceDef,
    has_scram_panel: bool,
    dt: f32,
) -> ReactorStatus {
    if core.melted_down {
        return ReactorStatus::Meltdown;
    }
    if !core.running {
        // Passive cooling toward ambient when shut down.
        core.temperature += (300.0 - core.temperature) * 0.05 * dt;
        return ReactorStatus::Offline;
    }

    let fission_heat = 420.0 * core.rod_withdrawal;
    let cooling = core.coolant_flow * 300.0;
    core.temperature += (fission_heat - cooling) * dt;
    // The core never cools below ambient, however much coolant flows.
    core.temperature = core.temperature.max(300.0);

    if core.temperature >= balance.reactor_limit_temp {
        if has_scram_panel {
            core.running = false;
            core.rod_withdrawal = 0.0;
            core.temperature = (core.temperature - 60.0).max(300.0);
            return ReactorStatus::Scrammed;
        }
        core.melted_down = true;
        core.running = false;
        return ReactorStatus::Meltdown;
    }
    if core.temperature >= balance.reactor_warn_temp {
        return ReactorStatus::Warning;
    }
    ReactorStatus::Nominal
}

/// Power a running reactor produces at a given temperature.
#[must_use]
pub fn reactor_output(core: &ReactorCore, balance: &BalanceDef) -> f32 {
    if !core.running || core.melted_down {
        return 0.0;
    }
    let heat_factor = (core.temperature / balance.reactor_warn_temp).clamp(0.0, 1.0);
    balance.reactor_kw * heat_factor
}

#[cfg(test)]
mod tests {
    use super::*;

    fn balance() -> BalanceDef {
        BalanceDef::default()
    }

    #[test]
    fn dose_falls_off_with_distance() {
        let spec = RadiationSpec {
            radius: 4.0,
            dose_per_sec: 0.5,
        };
        let near = dose_from_source(&spec, 1.0);
        let mid = dose_from_source(&spec, 2.0);
        let far = dose_from_source(&spec, 3.5);
        assert!(near > mid, "{near} vs {mid}");
        assert!(mid > far, "{mid} vs {far}");
        assert_eq!(dose_from_source(&spec, 5.0), 0.0, "outside the radius");
    }

    #[test]
    fn hazmat_suit_blocks_eighty_percent() {
        let b = balance();
        let exposure = HazardExposure {
            radiation: 1.0,
            ..Default::default()
        };
        let mut bare = HazardState::default();
        apply_exposure(&mut bare, &exposure, Protection::default(), &b, 1.0);
        let mut suited = HazardState::default();
        let protection = Protection {
            radiation_resist: 0.8,
            ..Default::default()
        };
        apply_exposure(&mut suited, &exposure, protection, &b, 1.0);
        let ratio = suited.dose / bare.dose;
        assert!((ratio - 0.2).abs() < 0.05, "got {ratio}");
    }

    #[test]
    fn dose_thresholds_drive_damage() {
        let b = balance();
        let mut state = HazardState {
            dose: 700.0,
            oxygen: 45.0,
            heat: 0.0,
        };
        let damage = apply_exposure(
            &mut state,
            &HazardExposure::default(),
            Protection::default(),
            &b,
            1.0,
        );
        assert!(damage > 0.0, "high dose must hurt");
    }

    #[test]
    fn regeneration_stops_above_the_dose_limit() {
        let b = balance();
        let state = HazardState {
            dose: 350.0,
            heat: 0.0,
            oxygen: 45.0,
        };
        assert!(!can_regenerate(&state, &b));
        let healthy = HazardState::default();
        assert!(can_regenerate(&healthy, &b));
    }

    #[test]
    fn screen_grain_appears_at_the_threshold() {
        let b = balance();
        let mut state = HazardState::default();
        assert_eq!(screen_grain(&state, &b), 0.0);
        state.dose = 250.0;
        assert!(screen_grain(&state, &b) > 0.0);
    }

    #[test]
    fn suffocation_damages_when_oxygen_runs_out() {
        let b = balance();
        let mut state = HazardState {
            dose: 0.0,
            heat: 0.0,
            oxygen: 0.5,
        };
        let exposure = HazardExposure {
            oxygen_drain: 10.0,
            ..Default::default()
        };
        let mut total = 0.0;
        for _ in 0..10 {
            total += apply_exposure(&mut state, &exposure, Protection::default(), &b, 0.2);
        }
        assert!(state.oxygen <= 0.0);
        assert!(total > 0.0, "no suffocation damage");
    }

    #[test]
    fn respirator_prevents_oxygen_drain() {
        let b = balance();
        let mut state = HazardState::default();
        let exposure = HazardExposure {
            oxygen_drain: 10.0,
            ..Default::default()
        };
        let protection = Protection {
            breathing: true,
            ..Default::default()
        };
        apply_exposure(&mut state, &exposure, protection, &b, 1.0);
        assert_eq!(state.oxygen, b.max_oxygen);
    }

    #[test]
    fn decon_shower_clears_dose() {
        let b = balance();
        let mut state = HazardState {
            dose: 400.0,
            heat: 0.0,
            oxygen: 45.0,
        };
        let protection = Protection {
            decon: true,
            ..Default::default()
        };
        for _ in 0..60 {
            apply_exposure(&mut state, &HazardExposure::default(), protection, &b, 0.1);
        }
        assert!(state.dose < 400.0, "dose did not fall: {}", state.dose);
    }

    #[test]
    fn heat_rises_and_cools() {
        let b = balance();
        let mut state = HazardState::default();
        let hot = HazardExposure {
            heat: 5.0,
            ..Default::default()
        };
        for _ in 0..20 {
            apply_exposure(&mut state, &hot, Protection::default(), &b, 0.5);
        }
        assert!(state.heat > 10.0, "heat did not build: {}", state.heat);
        for _ in 0..200 {
            apply_exposure(
                &mut state,
                &HazardExposure::default(),
                Protection::default(),
                &b,
                0.5,
            );
        }
        assert!(state.heat < 5.0, "heat did not fall: {}", state.heat);
    }

    #[test]
    fn cooling_suit_reduces_heat_gain() {
        let b = balance();
        let exposure = HazardExposure {
            heat: 4.0,
            ..Default::default()
        };
        let mut bare = HazardState::default();
        apply_exposure(&mut bare, &exposure, Protection::default(), &b, 1.0);
        let mut suited = HazardState::default();
        let protection = Protection {
            heat_resist: 0.75,
            ..Default::default()
        };
        apply_exposure(&mut suited, &exposure, protection, &b, 1.0);
        assert!(suited.heat < bare.heat);
    }

    #[test]
    fn reactor_heats_up_and_needs_cooling() {
        let b = balance();
        let mut core = ReactorCore {
            running: true,
            rod_withdrawal: 1.0,
            coolant_flow: 0.0,
            ..Default::default()
        };
        let mut status = ReactorStatus::Nominal;
        for _ in 0..200 {
            status = tick_reactor(&mut core, &b, false, 0.1);
            if status == ReactorStatus::Meltdown {
                break;
            }
        }
        assert_eq!(
            status,
            ReactorStatus::Meltdown,
            "uncooled core must melt down"
        );
    }

    #[test]
    fn scram_panel_prevents_meltdown() {
        let b = balance();
        let mut core = ReactorCore {
            running: true,
            rod_withdrawal: 1.0,
            coolant_flow: 0.0,
            ..Default::default()
        };
        let mut saw_scram = false;
        for _ in 0..400 {
            match tick_reactor(&mut core, &b, true, 0.1) {
                ReactorStatus::Scrammed => {
                    saw_scram = true;
                    break;
                }
                ReactorStatus::Meltdown => panic!("must not melt down with a scram panel"),
                _ => {}
            }
        }
        assert!(saw_scram, "reactor never scrammed");
        assert!(!core.melted_down);
        assert!(!core.running);
    }

    #[test]
    fn cooled_reactor_runs_nominally() {
        let b = balance();
        let mut core = ReactorCore {
            running: true,
            rod_withdrawal: 0.5,
            coolant_flow: 1.0,
            temperature: 400.0,
            ..Default::default()
        };
        for _ in 0..200 {
            let status = tick_reactor(&mut core, &b, true, 0.1);
            assert_ne!(status, ReactorStatus::Meltdown);
        }
        assert!(reactor_output(&core, &b) > 0.0);
    }
}
