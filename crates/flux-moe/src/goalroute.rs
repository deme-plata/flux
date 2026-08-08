//! goalroute.rs — route inference from flux_goal consensus + goal text.

use crate::{classify, registry, route as pick_expert};
use crate::skillroute;

#[derive(Debug, Clone, serde::Serialize)]
pub struct GoalRoutePlan {
    pub goal_text: String,
    pub task: String,
    pub expert: String,
    pub model: String,
    pub skill: Option<String>,
    pub skill_start: Option<String>,
    pub notes: Vec<String>,
}

pub fn route_from_goal_text(goal: &str) -> GoalRoutePlan {
    let task = classify(goal);
    let pool = registry();
    let expert = pick_expert(&pool, task);
    let skill_hit = skillroute::route(goal);
    let (skill, skill_start) = skill_hit
        .map(|s| (Some(s.name.to_string()), Some(s.start.to_string())))
        .unwrap_or((None, None));

    let mut notes = vec![
        "read flux_goal_list for live consensus".into(),
        "money paths: mandate + two_mind before spend".into(),
    ];
    let gl = goal.to_lowercase();
    if gl.contains("bank") {
        notes.push("start: flux_bank_status -> flux_bank_propose_transfer (dry)".into());
    }
    if gl.contains("vast") || gl.contains("gpu") {
        notes.push("spend-gate: flux_vast_recommend only; operator flux_vast_create".into());
    }

    GoalRoutePlan {
        goal_text: goal.into(),
        task: format!("{:?}", task),
        expert: expert.name.to_string(),
        model: expert.model.to_string(),
        skill,
        skill_start,
        notes,
    }
}

pub fn route_from_consensus() -> Option<GoalRoutePlan> {
    let path = std::path::Path::new("/tmp/flux-goals.json");
    let bytes = std::fs::read(path).ok()?;
    let store = fluxc_serve::goals::GoalStore::load(&bytes);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs();
    let g = store.consensus(now)?;
    Some(route_from_goal_text(&g.text))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routes_bank_goal() {
        let p = route_from_goal_text("port Quillon bank to Flux native");
        assert!(p.notes.iter().any(|n| n.contains("flux_bank")));
    }
}