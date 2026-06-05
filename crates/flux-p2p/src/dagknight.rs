// DAGKnight Protocol — Asynchronous BFT DAG Consensus
//
// Formal model from \"The DAG-Knight Consensus Protocol\" (Quillon Research Foundation, 2026).
//
// Architecture:
//   DAGKnight manages a local DAG of vertices.
//   Each validator produces one vertex per round.
//   Vertices reference 2f+1 vertices from previous round (implicit voting).
//   VDF-based leader election selects a leader per round.
//   Commit rule: 2f+1 round r+1 vertices, each referencing 2f+1 round r vertices
//                that all support the leader → commit.
//
// Key properties:
//   - Asynchronous (no timeout assumptions for safety)
//   - Leaderless (any validator can propose)
//   - O(n) message complexity per round (zero-message voting)
//   - Post-quantum (Dilithium-5 signatures, BLAKE3 hashing)

use std::collections::{HashMap, HashSet};
use std::time::{SystemTime, UNIX_EPOCH};

use sha3::{Digest, Sha3_256};

use super::entanglement::GaussCode;

/// Configuration for a DAGKnight validator node.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct DAGKnightConfig {
    pub node_id: String,
    pub validator_count: u32,       // n = 48 in production
    pub byzantine_tolerance: u32,   // f < n/3 = 15
    pub vdf_difficulty: u32,        // VDF iterations (2^difficulty)
}

/// A DAGKnight validator node — maintains a local DAG and participates in consensus.
pub struct DAGKnight {
    config: DAGKnightConfig,
    dag: DAG,
    current_round: u64,
    committed_blocks: Vec<Block>,
}

/// The Directed Acyclic Graph — the core data structure.
#[derive(Clone, Debug)]
pub struct DAG {
    /// All vertices, indexed by (round, validator_id).
    vertices: HashMap<(u64, String), Vertex>,
    /// Round → set of validator IDs who produced vertices in that round.
    round_map: HashMap<u64, HashSet<String>>,
}

/// A single vertex in the DAG — one validator's contribution to a round.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Vertex {
    pub round: u64,
    pub validator: String,          // Who created this vertex
    pub block: Option<Block>,       // Optional block payload (leader vertices have blocks)
    pub references: Vec<VertexRef>, // References to 2f+1 vertices from round-1
    pub signature: Vec<u8>,         // Dilithium-5 signature over (round || validator || block_hash || refs_hash)
    pub timestamp_ms: u64,
    pub vdf_proof: Option<VDFProof>, // VDF proof if this vertex is a leader proposal
    /// Gauss code for the transaction knot from this block (QtFT Path C).
    /// Populated when the vertex carries a block with transactions.
    pub gauss_code: Option<GaussCode>,
}

/// Lightweight reference to a DAG vertex (used in edges).
#[derive(Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct VertexRef {
    pub round: u64,
    pub validator: String,
    pub hash: String, // BLAKE3 hash of the referenced vertex
}

/// A committed block — final output of DAGKnight consensus.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Block {
    pub round: u64,
    pub leader: String,
    pub transactions: Vec<Vec<u8>>,
    pub commit_time_ms: u64,
    pub proof: CommitProof,
}

/// Proof that a block was committed — cryptographic evidence.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct CommitProof {
    pub leader_vertex_hash: String,
    pub supporting_vertices: Vec<VertexRef>, // 2f+1 vertices at round+1
    pub merkle_root: String,
}

/// Verifiable Delay Function proof — used for leader election.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct VDFProof {
    pub input: Vec<u8>,
    pub output: Vec<u8>,
    pub iterations: u64,
    pub proof: Vec<u8>, // Wesolowski proof
}

impl DAGKnight {
    /// Create a new DAGKnight validator.
    pub fn new(config: DAGKnightConfig) -> Self {
        DAGKnight {
            config,
            dag: DAG {
                vertices: HashMap::new(),
                round_map: HashMap::new(),
            },
            current_round: 0,
            committed_blocks: Vec::new(),
        }
    }

    /// Get the validator count.
    pub fn validator_count(&self) -> u32 {
        self.config.validator_count
    }

    /// Get the Byzantine tolerance.
    pub fn byzantine_tolerance(&self) -> u32 {
        self.config.byzantine_tolerance
    }

    /// Get the current round.
    pub fn current_round(&self) -> u64 {
        self.current_round
    }

    /// Get the number of committed blocks.
    pub fn committed_count(&self) -> usize {
        self.committed_blocks.len()
    }

    /// Select the VDF-based leader for a given round.
    /// In production: leader = VDF.eval(round_seed).mod(n)
    /// For prototype: deterministic hash-based selection.
    pub fn leader_for_round(&self, round: u64) -> String {
        let mut hasher = blake3::Hasher::new();
        hasher.update(&round.to_le_bytes());
        hasher.update(b"dagknight-leader-v1");
        let hash = hasher.finalize();
        let leader_idx = (u64::from_le_bytes(hash.as_bytes()[..8].try_into().unwrap())
            % self.config.validator_count as u64) as u32;
        format!("validator-{}", leader_idx)
    }

    /// Receive a batch of vertices in parallel — the core speed path.
    ///
    /// Validates all vertices concurrently using multiple threads, then inserts
    /// them into the DAG sequentially. This is the hot path for DAGKnight sync:
    /// a node receiving n=48 validator vertices from the gossipsub mesh.
    ///
    /// Returns all newly committed blocks (may be zero, one, or several).
    pub fn receive_vertices_batch(&mut self, vertices: Vec<Vertex>) -> Result<Vec<Block>, String> {
        if vertices.is_empty() {
            return Ok(Vec::new());
        }

        // Phase 1: Parallel validation (CPU-bound: BLAKE3, signature checks)
        let validation_results: Vec<Result<(), String>> = if vertices.len() > 1 {
            let this: &DAGKnight = self;
            std::thread::scope(|s| {
                let mut handles = Vec::with_capacity(vertices.len());
                for v in &vertices {
                    handles.push(s.spawn(move || this.validate_vertex(v)));
                }
                handles.into_iter().map(|h| h.join().unwrap()).collect()
            })
        } else {
            vec![self.validate_vertex(&vertices[0])]
        };

        // Phase 2: Sequential insertion (shared DAG state requires ordering)
        let mut committed = Vec::new();
        for (vertex, result) in vertices.into_iter().zip(validation_results) {
            result?;

            let key = (vertex.round, vertex.validator.clone());
            let validator_id = vertex.validator.clone();
            let round = vertex.round;
            self.dag.vertices.insert(key, vertex);
            self.dag.round_map
                .entry(round)
                .or_default()
                .insert(validator_id);

            if round > self.current_round {
                self.current_round = round;
            }

            if round > 1 {
                if let Some(block) = self.try_commit(round - 1) {
                    committed.push(block);
                }
            }
        }

        if !committed.is_empty() {
            tracing::info!(
                count = committed.len(),
                total = self.committed_blocks.len(),
                "DAGKnight batch committed"
            );
        }

        Ok(committed)
    }

    /// Receive a vertex from the network — add it to the local DAG.
    /// Returns Some(Block) if this vertex causes a commit.
    pub fn receive_vertex(&mut self, vertex: Vertex) -> Result<Option<Block>, String> {
        // Validate the vertex
        self.validate_vertex(&vertex)?;

        // Add to DAG
        let key = (vertex.round, vertex.validator.clone());
        self.dag.vertices.insert(key, vertex.clone());
        self.dag.round_map
            .entry(vertex.round)
            .or_default()
            .insert(vertex.validator.clone());

        // Update current round
        if vertex.round > self.current_round {
            self.current_round = vertex.round;
        }

        // Try to commit the leader for round-1 (if we have enough vertices at this round)
        if vertex.round > 1 {
            Ok(self.try_commit(vertex.round - 1))
        } else {
            Ok(None)
        }
    }

    /// Create a new vertex for the current round.
    pub fn create_vertex(&mut self, transactions: Vec<Vec<u8>>, is_leader: bool) -> Result<Vertex, String> {
        let round = self.current_round + 1;
        let leader = self.leader_for_round(round);
        let is_my_leader = is_leader || (self.config.node_id == leader);

        // Collect references to 2f+1 vertices from round-1
        let prev_round = round.saturating_sub(1);
        let references = self.collect_references(prev_round)?;

        // Generate VDF proof if we're the leader
        let vdf_proof = if is_my_leader {
            Some(self.generate_vdf_proof(round))
        } else {
            None
        };

        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        let block = if is_my_leader {
            Some(Block {
                round,
                leader: self.config.node_id.clone(),
                transactions,
                commit_time_ms: 0, // Will be set when committed
                proof: CommitProof {
                    leader_vertex_hash: String::new(),
                    supporting_vertices: Vec::new(),
                    merkle_root: String::new(),
                },
            })
        } else {
            None
        };

        let vertex = Vertex {
            round,
            validator: self.config.node_id.clone(),
            block,
            references,
            signature: vec![], // Sign with Dilithium-5 in production
            timestamp_ms: timestamp,
            vdf_proof,
            gauss_code: None, // Populated by entanglement router when block txs are fed
        };

        // Compute hash for self-referencing
        let _vhash = hash_vertex(&vertex);

        // Add to own DAG immediately
        self.dag.vertices.insert((round, self.config.node_id.clone()), vertex.clone());
        self.dag.round_map.entry(round).or_default().insert(self.config.node_id.clone());
        self.current_round = round;

        Ok(vertex)
    }

    // ── Internal methods ──

    fn validate_vertex(&self, vertex: &Vertex) -> Result<(), String> {
        // Check round is positive
        if vertex.round == 0 {
            return Err("vertex round must be > 0".into());
        }

        // Check references: must have 2f+1 references to round-1 vertices
        let required_refs = 2 * self.config.byzantine_tolerance + 1;
        if vertex.references.len() < required_refs as usize {
            return Err(format!(
                "vertex has {} references, need at least {}",
                vertex.references.len(),
                required_refs
            ));
        }

        // Check all references point to round-1
        for vref in &vertex.references {
            if vref.round != vertex.round - 1 {
                return Err(format!(
                    "reference at round {} points to round {}",
                    vertex.round, vref.round
                ));
            }
        }

        // In production: verify Dilithium-5 signature, VDF proof
        Ok(())
    }

    fn collect_references(&self, round: u64) -> Result<Vec<VertexRef>, String> {
        let required = 2 * self.config.byzantine_tolerance + 1;
        let mut refs = Vec::new();

        // Collect all vertices from the target round
        if let Some(validators) = self.dag.round_map.get(&round) {
            for validator in validators {
                if let Some(v) = self.dag.vertices.get(&(round, validator.clone())) {
                    refs.push(VertexRef {
                        round,
                        validator: validator.clone(),
                        hash: hash_vertex(v),
                    });
                }
                if refs.len() >= required as usize {
                    break;
                }
            }
        }

        if refs.len() < required as usize {
            return Err(format!(
                "not enough vertices at round {}: have {}, need {}",
                round,
                refs.len(),
                required
            ));
        }

        Ok(refs)
    }

    /// Try to commit the leader's block at the given round.
    /// Returns Some(Block) if committed, None if conditions not met.
    fn try_commit(&mut self, round: u64) -> Option<Block> {
        let leader_id = self.leader_for_round(round);
        let required = 2 * self.config.byzantine_tolerance + 1;

        // Get the leader's vertex at this round
        let leader_key = (round, leader_id.clone());
        let leader_vertex = self.dag.vertices.get(&leader_key)?;
        let leader_hash = hash_vertex(leader_vertex);

        // Get vertices at round+1
        let support_round = round + 1;
        let mut supporting_vertices: Vec<VertexRef> = Vec::new();

        if let Some(validators) = self.dag.round_map.get(&support_round) {
            for validator in validators {
                if let Some(v) = self.dag.vertices.get(&(support_round, validator.clone())) {
                    // Check if this vertex references the leader
                    let _leader_refs: HashSet<_> = v.references.iter()
                        .filter(|r| r.round == round)
                        .map(|r| &r.validator)
                        .collect();

                    // In the formal model: each vertex at round+1 must reference 2f+1
                    // vertices at round r, ALL of which must reference the leader.
                    // Simplified check: just count how many references point to leader-supporting vertices.
                    let leader_support_count = v.references.iter()
                        .filter(|r| r.round == round && r.validator == leader_id)
                        .count();

                    if leader_support_count > 0 {
                        supporting_vertices.push(VertexRef {
                            round: support_round,
                            validator: validator.clone(),
                            hash: hash_vertex(v),
                        });
                    }
                }
            }
        }

        // Commit condition: 2f+1 vertices at round+1 support the leader
        if supporting_vertices.len() >= required as usize {
            let leader_block = leader_vertex.block.clone()?;
            let mut block = leader_block;
            block.commit_time_ms = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64;
            block.proof = CommitProof {
                leader_vertex_hash: leader_hash,
                supporting_vertices,
                merkle_root: compute_merkle_root(&block.transactions),
            };

            self.committed_blocks.push(block.clone());
            Some(block)
        } else {
            None
        }
    }

    /// Generate a VDF proof for leader election.
    fn generate_vdf_proof(&self, round: u64) -> VDFProof {
        let input = {
            let mut hasher = blake3::Hasher::new();
            hasher.update(&round.to_le_bytes());
            hasher.update(self.config.node_id.as_bytes());
            hasher.finalize()
        };

        // Simulate VDF: repeated squaring in a group
        let iterations = 1u64 << self.config.vdf_difficulty;
        let mut current = input.as_bytes().to_vec();
        for _ in 0..iterations.min(1_000_000) {
            let mut hasher = Sha3_256::new();
            Digest::update(&mut hasher, &current);
            current = Digest::finalize(hasher).to_vec();
        }

        VDFProof {
            input: input.as_bytes().to_vec(),
            output: current,
            iterations,
            proof: vec![], // Wesolowski proof in production
        }
    }
}

// ── Helper functions ──

/// Compute the BLAKE3 hash of a vertex (for referencing).
pub fn hash_vertex(v: &Vertex) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(&v.round.to_le_bytes());
    hasher.update(v.validator.as_bytes());
    if let Some(ref block) = v.block {
        for tx in &block.transactions {
            hasher.update(tx);
        }
    }
    for vref in &v.references {
        hasher.update(vref.hash.as_bytes());
    }
    hasher.update(&v.timestamp_ms.to_le_bytes());
    format!("{}", hasher.finalize())
}

/// Compute the BLAKE3 Merkle root of a list of transactions.
pub fn compute_merkle_root(txs: &[Vec<u8>]) -> String {
    if txs.is_empty() {
        return "0".repeat(64);
    }
    let mut leaves: Vec<String> = txs.iter()
        .map(|tx| format!("{}", blake3::hash(tx)))
        .collect();
    while leaves.len() > 1 {
        let mut next = Vec::with_capacity((leaves.len() + 1) / 2);
        for i in (0..leaves.len()).step_by(2) {
            let left = &leaves[i];
            let right = if i + 1 < leaves.len() { &leaves[i + 1] } else { left };
            let combined = format!("{}{}", left, right);
            next.push(format!("{}", blake3::hash(combined.as_bytes())));
        }
        leaves = next;
    }
    leaves[0].clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dagknight_create() {
        let config = DAGKnightConfig {
            node_id: "validator-0".into(),
            validator_count: 48,
            byzantine_tolerance: 15,
            vdf_difficulty: 10, // Smaller for tests
        };
        let dk = DAGKnight::new(config);
        assert_eq!(dk.validator_count(), 48);
        assert_eq!(dk.current_round(), 0);
    }

    #[test]
    fn test_leader_election_deterministic() {
        let config = DAGKnightConfig {
            node_id: "validator-0".into(),
            validator_count: 48,
            byzantine_tolerance: 15,
            vdf_difficulty: 10,
        };
        let dk = DAGKnight::new(config);
        let l1 = dk.leader_for_round(42);
        let l2 = dk.leader_for_round(42);
        assert_eq!(l1, l2, "Leader election must be deterministic");
    }

    #[test]
    fn test_vertex_creation_and_validation() {
        let mut dk = DAGKnight::new(DAGKnightConfig {
            node_id: "validator-0".into(),
            validator_count: 4,
            byzantine_tolerance: 1, // f=1, n=4, requires 3 refs
            vdf_difficulty: 5,
        });

        // Create first vertex (round 1) — won't have 2f+1 refs to round 0
        // For round 1, we need to manually seed round 0 vertices
        // In production, genesis round is pre-seeded

        // Manually seed round 0 with 3+ vertices
        for i in 0..4 {
            let seed_v = Vertex {
                round: 0,
                validator: format!("validator-{}", i),
                block: None,
                references: vec![],
                signature: vec![],
                timestamp_ms: 0,
                vdf_proof: None,
                gauss_code: None,
            };
            dk.dag.vertices.insert((0, seed_v.validator.clone()), seed_v.clone());
            dk.dag.round_map.entry(0).or_default().insert(seed_v.validator.clone());
        }

        // Now create vertex at round 1
        let vertex = dk.create_vertex(vec![b"tx1".to_vec(), b"tx2".to_vec()], true).unwrap();
        assert_eq!(vertex.round, 1);
        assert!(vertex.references.len() >= 3); // 2f+1
    }

    #[test]
    fn test_hash_deterministic() {
        let v1 = Vertex {
            round: 5,
            validator: "validator-7".into(),
            block: None,
            references: vec![],
            signature: vec![],
            timestamp_ms: 1000,
            vdf_proof: None,
            gauss_code: None,
        };
        let v2 = v1.clone();
        assert_eq!(hash_vertex(&v1), hash_vertex(&v2));
    }
}