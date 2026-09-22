//! Scenario arithmetic, not a forecast or structural engineering model.
use serde::{Serialize, Deserialize};
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Scenario {
    pub it_mw: f64, pub pue: f64, pub utilization: f64,
    pub site_design_months: f64, pub grid_months: f64, pub procurement_months: f64,
    pub construction_months: f64, pub commissioning_months: f64,
    pub robot_eligible_fraction: f64, pub robot_productivity_multiplier: f64,
}
impl Default for Scenario {
    fn default()->Self { Self {it_mw:20.,pue:1.2,utilization:0.8,site_design_months:12.,
        grid_months:36.,procurement_months:18.,construction_months:24.,commissioning_months:6.,
        robot_eligible_fraction:0.3,robot_productivity_multiplier:1.5} }
}
pub fn estimate(s: &Scenario)->super::Result<serde_json::Value> {
    let nonnegative=[s.site_design_months,s.grid_months,s.procurement_months,s.construction_months,s.commissioning_months];
    if !s.it_mw.is_finite() || s.it_mw<=0. || !s.pue.is_finite() || s.pue<1.
        || !s.utilization.is_finite() || !(0. ..=1.).contains(&s.utilization)
        || !s.robot_eligible_fraction.is_finite() || !(0. ..=1.).contains(&s.robot_eligible_fraction)
        || !s.robot_productivity_multiplier.is_finite() || s.robot_productivity_multiplier<1.
        || nonnegative.iter().any(|v|!v.is_finite() || *v<0.) { return Err("invalid scenario assumptions".into()); }
    let factor=1.-s.robot_eligible_fraction+s.robot_eligible_fraction/s.robot_productivity_multiplier;
    let civil=s.construction_months*factor;
    let baseline=(s.site_design_months+s.construction_months).max(s.grid_months).max(s.procurement_months)+s.commissioning_months;
    let robot=(s.site_design_months+civil).max(s.grid_months).max(s.procurement_months)+s.commissioning_months;
    Ok(serde_json::json!({"classification":"illustrative scenario, not a site forecast", "assumptions":s,
        "peak_facility_mw":s.it_mw*s.pue,"annual_energy_gwh":s.it_mw*s.pue*s.utilization*8.76,
        "baseline_months":baseline,"robot_assisted_months":robot,"months_saved":baseline-robot,
        "robot_assisted_construction_months":civil,
        "dependency_model":"max(site/design + construction, grid from project start, procurement from project start) + commissioning",
        "excluded":"financing, land disputes, permitting overruns, water, hardware export rules, first-of-kind tower engineering, robot deployment learning"}))
}
#[cfg(test)] mod tests { use super::*;
    #[test] fn power_bottleneck_erases_robot_schedule_gain() {let r=estimate(&Scenario::default()).unwrap(); assert_eq!(r["baseline_months"],42.); assert_eq!(r["robot_assisted_months"],42.);}
    #[test] fn robot_gain_is_limited_to_eligible_work() {let mut s=Scenario::default();s.grid_months=20.; let r=estimate(&s).unwrap();assert!((r["robot_assisted_months"].as_f64().unwrap()-39.6).abs()<1e-8);s.robot_eligible_fraction=1.1;assert!(estimate(&s).is_err());}
}
