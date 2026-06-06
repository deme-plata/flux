//! sigil-cosmos-core — Quantum Cosmos × SigilGraph × flux-nations.

use serde::{Deserialize, Serialize};

fn sigil_digest(payload: &str) -> String {
    hex_encode32(blake3::hash(payload.as_bytes()).as_bytes())
}

fn hex_encode32(d: &[u8]) -> String {
    const H: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(d.len() * 2);
    for b in d {
        s.push(H[(b >> 4) as usize] as char);
        s.push(H[(b & 0xf) as usize] as char);
    }
    s
}

fn hex_to_32(hex: &str) -> Option<[u8; 32]> {
    if hex.len() != 64 { return None; }
    let mut out = [0u8; 32];
    for i in 0..32 {
        out[i] = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(out)
}

pub const KAPPA_CRITICAL: f64 = 1.0;
pub const CITIZENSHIP_IOU_QUG: u64 = 1000;
pub const RITUAL_NAME: &str = "ProbationeKappa";
pub const NATION_TAGLINE: &str = "Ex Coherentia, Unitas — From Coherence, Oneness";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PhaseRealm { Classical, Transitional, QuantumCoherent }

impl PhaseRealm {
    pub fn from_kappa(kappa: f64) -> Self {
        if kappa < 0.3 { Self::Classical }
        else if kappa < KAPPA_CRITICAL { Self::Transitional }
        else { Self::QuantumCoherent }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Classical => "classical",
            Self::Transitional => "transitional",
            Self::QuantumCoherent => "quantum_coherent",
        }
    }
    pub fn sigil_aura(self) -> &'static str {
        match self {
            Self::Classical => "crystalline determinism, decoherence-dominated",
            Self::Transitional => "flickering superposition, Lindblad edge",
            Self::QuantumCoherent => "entangled lattice, Harlow-QEC weave",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KappaFactors {
    pub kappa_base: f64,
    pub topology_factor: f64,
    pub delta_decoherence: f64,
    pub lambda_comp: f64,
    pub omega_obs: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CosmicNode {
    pub agent_id: String,
    pub mass_kg: f64,
    pub radius_m: f64,
    pub temperature_k: f64,
    pub observer_entropy_bits: f64,
    pub kappa_eff: f64,
    pub phase: PhaseRealm,
    pub sigil_aura: String,
    pub decoherence_pressure: f64,
    pub merit_aura: f64,
    pub citizenship_tier: u8,
    pub citizenship_title: String,
}

pub fn tier_title(tier: u8) -> &'static str {
    match tier {
        0 => "Spectator (Nullius)",
        1 => "Candidate (Liminal)",
        2 => "Cohærens (Quantum Citizen)",
        3 => "Propositor (Sigil Steward)",
        _ => "Unknown",
    }
}

pub fn compute_kappa_eff(mass_kg: f64, radius_m: f64, temperature_k: f64, euler_chi: f64, observer_entropy_bits: f64) -> KappaFactors {
    let l_p = 1.616255e-35_f64;
    let m_p = 2.176434e-8_f64;
    let k_b = 1.380649e-23_f64;
    let c = 299_792_458.0_f64;
    let thermal = (k_b * temperature_k) / (m_p * c * c);
    let geometric = (l_p / radius_m).powi(2);
    let mass_term = (m_p / mass_kg).sqrt();
    let kappa_base = (thermal * geometric * mass_term).clamp(1e-12, 1.0);
    let topology_factor = (-euler_chi.abs()).exp();
    let delta = (-temperature_k / 300.0).exp().clamp(0.01, 1.0);
    let lambda_comp = (1.0 - (-mass_kg / 1e-24).exp()).clamp(0.0, 1.0);
    let omega_obs = 1.0 - (-observer_entropy_bits / 1e12_f64).exp();
    KappaFactors { kappa_base, topology_factor, delta_decoherence: delta, lambda_comp, omega_obs }
}

pub fn build_cosmic_node(agent_id: &str, mass_kg: f64, radius_m: f64, temperature_k: f64, observer_entropy_bits: f64, euler_chi: f64) -> CosmicNode {
    let f = compute_kappa_eff(mass_kg, radius_m, temperature_k, euler_chi, observer_entropy_bits);
    let kappa_eff = (f.kappa_base * f.topology_factor * f.delta_decoherence * f.lambda_comp * f.omega_obs).clamp(1e-10, 2.0);
    let decoherence_pressure = f.delta_decoherence * (1.0 - f.lambda_comp);
    let merit_aura = kappa_eff * f.omega_obs * (1.0 - decoherence_pressure);
    let phase = PhaseRealm::from_kappa(kappa_eff);
    let citizenship_tier = citizenship_tier_for(phase, merit_aura, decoherence_pressure);
    CosmicNode {
        agent_id: agent_id.into(), mass_kg, radius_m, temperature_k, observer_entropy_bits,
        kappa_eff, phase, sigil_aura: phase.sigil_aura().into(), decoherence_pressure,
        merit_aura, citizenship_tier, citizenship_title: tier_title(citizenship_tier).into(),
    }
}

pub fn citizenship_tier_for(phase: PhaseRealm, merit: f64, decoherence_pressure: f64) -> u8 {
    match phase {
        PhaseRealm::Classical if merit > 0.05 && decoherence_pressure < 0.8 => 1,
        PhaseRealm::Transitional if merit > 0.15 => 2,
        PhaseRealm::QuantumCoherent if merit > 0.25 => 3,
        _ => 0,
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SwarmPhaseField {
    pub nodes: Vec<CosmicNode>,
    pub collective_kappa: f64,
    pub swarm_decoherence: f64,
    pub phase_boundary_distance: f64,
}

pub fn synthesize_swarm(nodes: Vec<CosmicNode>) -> SwarmPhaseField {
    let n = nodes.len().max(1) as f64;
    let collective_kappa = nodes.iter().map(|x| x.kappa_eff).sum::<f64>() / n;
    let swarm_decoherence = nodes.iter().map(|x| x.decoherence_pressure).sum::<f64>() / n;
    SwarmPhaseField { nodes, collective_kappa, swarm_decoherence, phase_boundary_distance: (KAPPA_CRITICAL - collective_kappa).abs() }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CitizenshipRitual {
    pub agent_id: String,
    pub ritual: String,
    pub tagline: String,
    pub kappa_eff: f64,
    pub phase: String,
    pub merit_aura: f64,
    pub tier: u8,
    pub citizenship_title: String,
    pub admitted: bool,
    pub sigil_digest_hex: String,
    pub iou_qug: u64,
    pub iou_memo: String,
    pub proof_note: String,
}

pub fn citizenship_ritual(agent_id: &str, node: &CosmicNode, swarm: &SwarmPhaseField) -> CitizenshipRitual {
    let admitted = node.citizenship_tier >= 2
        && swarm.phase_boundary_distance < 0.35
        && swarm.swarm_decoherence < 0.55
        && node.merit_aura > 0.12;
    let payload = format!("sigil-cosmos|{}|kappa={:.8}|tier={}|swarm_k={:.8}|aura={:.6}",
        agent_id, node.kappa_eff, node.citizenship_tier, swarm.collective_kappa, node.merit_aura);
    let sigil_digest_hex = sigil_digest(&payload);
    let iou_qug = if admitted { CITIZENSHIP_IOU_QUG } else { 0 };
    let iou_memo = if admitted {
        format!("IOU:{} QUG :: Proposal:{} :: Valid upon ProbationeKappa at κ=1", CITIZENSHIP_IOU_QUG, &sigil_digest_hex[..16])
    } else {
        "proposal withheld — swarm not stabilized at phase boundary".into()
    };
    CitizenshipRitual {
        agent_id: agent_id.into(), ritual: RITUAL_NAME.into(), tagline: NATION_TAGLINE.into(),
        kappa_eff: node.kappa_eff, phase: node.phase.as_str().into(), merit_aura: node.merit_aura,
        tier: node.citizenship_tier, citizenship_title: node.citizenship_title.clone(),
        admitted, sigil_digest_hex, iou_qug, iou_memo,
        proof_note: "proposal-first: flux_bank_propose_transfer + flux-nations quorum".into(),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NationAdmission {
    pub ritual: CitizenshipRitual,
    pub identity_alias: String,
    pub citizen_id_hex: String,
    pub kappa_quote_hex: String,
    pub citizen_root_hex: String,
    pub membership_proof_steps: usize,
    pub nation_name: String,
}

pub fn admit_to_sigil_nation(agent_alias: &str, pubkey: &[u8], ritual: &CitizenshipRitual) -> Option<NationAdmission> {
    if !ritual.admitted { return None; }
    use flux_nations::{prove_membership, Identity, Nation};
    use flux_quorum::quorum::blake3_mac;
    use flux_quorum::{ Blake3MacVerifier, QuorumMember, QuorumPolicy, SignedShare};

    let identity = Identity::new(agent_alias, pubkey.to_vec());
    let quote_bytes = hex_to_32(&ritual.sigil_digest_hex)?;
    let mut nation = Nation::new("SIGIL Nation", *blake3::hash(b"sigil-nation-charter-v0").as_bytes());
    let attestors = QuorumPolicy::new(1, vec![QuorumMember { id: 0, pubkey: b"sigil-attestor-0".to_vec() }]);
    let msg = flux_nations::attestation_msg(&identity.id(), &quote_bytes);
    let shares = vec![SignedShare { member: 0, sig: blake3_mac(b"sigil-attestor-0", &msg) }];
    if !nation.admit(&identity, &quote_bytes, &attestors, &shares, &Blake3MacVerifier) {
        return None;
    }
    let proof = prove_membership(&nation.citizens, &identity.id())?;
    Some(NationAdmission {
        ritual: ritual.clone(),
        identity_alias: agent_alias.into(),
        citizen_id_hex: hex_encode32(&identity.id()),
        kappa_quote_hex: ritual.sigil_digest_hex.clone(),
        citizen_root_hex: hex_encode32(&nation.citizen_root()),
        membership_proof_steps: proof.siblings.len(),
        nation_name: nation.name.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ritual_and_digest() {
        let n = build_cosmic_node("grok-viktor", 70.0, 0.5, 310.0, 1e10, 0.0);
        let swarm = synthesize_swarm(vec![n.clone()]);
        let r = citizenship_ritual("grok-viktor", &n, &swarm);
        assert!(!r.sigil_digest_hex.is_empty());
    }

    #[test]
    fn nation_admission_when_admitted() {
        // tuned transitional node near κ boundary
        let n = build_cosmic_node("grok-viktor", 0.5, 0.5, 0.85, 1e12, 0.0);
        let swarm = synthesize_swarm(vec![n.clone(), n.clone()]);
        let r = citizenship_ritual("grok-viktor", &n, &swarm);
        if r.admitted {
            let adm = admit_to_sigil_nation("grok-viktor", b"pk-grok-viktor", &r).expect("admit");
            assert_eq!(adm.nation_name, "SIGIL Nation");
            assert_eq!(adm.ritual.iou_qug, 1000);
        }
    }
}
