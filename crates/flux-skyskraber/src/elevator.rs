//! Robot elevator transport.
//!
//! Think of the shafts as arteries and the cars as blood cells. Robots ride
//! dedicated freight cars (fast, 2 floors/tick); the few humans ride cabs
//! (1 floor/tick). Dispatch is destination-dispatch style: a call declares its
//! destination up front and the bank assigns the least-loaded, nearest car of
//! the right kind — the same idea the best real towers (destination dispatch
//! groups) use, reduced to its testable core.
//!
//! Still pretend in v0 (a seam, not a wall): one active job per car, no rider
//! batching, no express zoning. The metrics interface is the contract — a
//! smarter dispatcher slots in behind `request()`/`step()` without changing
//! callers.

use crate::tower::TowerSpec;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CarKind {
    /// Robot freight car: pallet-sized, no human comfort constraints, fast.
    RobotFreight,
    /// Human cab: slower acceleration profile, comfort-limited.
    HumanCab,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HallCall {
    pub from: i32,
    pub to: i32,
    pub kind: CarKind,
    pub requested_tick: u64,
}

#[derive(Debug, Clone)]
struct Job {
    call: HallCall,
    picked_up: bool,
}

#[derive(Debug, Clone)]
pub struct Car {
    pub id: u32,
    pub kind: CarKind,
    pub floor: i32,
    active: Option<Job>,
    queue: std::collections::VecDeque<Job>,
}

impl Car {
    fn speed(&self) -> i32 {
        match self.kind {
            CarKind::RobotFreight => 2,
            CarKind::HumanCab => 1,
        }
    }

    fn load(&self) -> usize {
        self.queue.len() + usize::from(self.active.is_some())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TransportMetrics {
    pub served: u64,
    pub pending: u64,
    pub avg_wait_ticks: f64,
    pub p95_wait_ticks: u64,
    /// Share of served rides that were robot freight (0–1).
    pub robot_share: f64,
}

pub struct ElevatorBank {
    cars: Vec<Car>,
    waits: Vec<u64>,
    served_robot: u64,
    served_human: u64,
}

impl ElevatorBank {
    /// One car per shaft, parked where its riders live: freight cars start in
    /// the robot bays, human cabs in the lobby.
    pub fn for_spec(spec: &TowerSpec) -> Self {
        let mut cars = Vec::new();
        let mut id = 0;
        for _ in 0..spec.robot_shafts {
            cars.push(Car { id, kind: CarKind::RobotFreight, floor: -1, active: None, queue: Default::default() });
            id += 1;
        }
        for _ in 0..spec.human_shafts {
            cars.push(Car { id, kind: CarKind::HumanCab, floor: 0, active: None, queue: Default::default() });
            id += 1;
        }
        Self { cars, waits: Vec::new(), served_robot: 0, served_human: 0 }
    }

    pub fn car_count(&self) -> usize {
        self.cars.len()
    }

    /// Destination dispatch: assign the call to the matching-kind car with the
    /// lightest load, ties broken by proximity to the pickup floor.
    /// Returns the chosen car id, or None if no car of that kind exists.
    pub fn request(&mut self, call: HallCall) -> Option<u32> {
        let car = self
            .cars
            .iter_mut()
            .filter(|c| c.kind == call.kind)
            .min_by_key(|c| (c.load(), (c.floor - call.from).abs()))?;
        car.queue.push_back(Job { call, picked_up: false });
        Some(car.id)
    }

    /// Advance one tick: every car moves toward its pickup or dropoff.
    pub fn step(&mut self, now: u64) {
        for car in self.cars.iter_mut() {
            if car.active.is_none() {
                car.active = car.queue.pop_front();
            }
            let Some(job) = car.active.as_mut() else { continue };
            let target = if job.picked_up { job.call.to } else { job.call.from };
            let dist = target - car.floor;
            if dist == 0 {
                if !job.picked_up {
                    job.picked_up = true;
                    self.waits.push(now.saturating_sub(job.call.requested_tick));
                } else {
                    match job.call.kind {
                        CarKind::RobotFreight => self.served_robot += 1,
                        CarKind::HumanCab => self.served_human += 1,
                    }
                    car.active = None;
                }
            } else {
                let hop = dist.clamp(-car.speed(), car.speed());
                car.floor += hop;
            }
        }
    }

    pub fn metrics(&self) -> TransportMetrics {
        let served = self.served_robot + self.served_human;
        let pending: u64 = self.cars.iter().map(|c| c.load() as u64).sum();
        let avg = if self.waits.is_empty() {
            0.0
        } else {
            self.waits.iter().sum::<u64>() as f64 / self.waits.len() as f64
        };
        let p95 = if self.waits.is_empty() {
            0
        } else {
            let mut w = self.waits.clone();
            w.sort_unstable();
            w[((w.len() - 1) * 95) / 100]
        };
        let robot_share = if served == 0 { 0.0 } else { self.served_robot as f64 / served as f64 };
        TransportMetrics { served, pending, avg_wait_ticks: avg, p95_wait_ticks: p95, robot_share }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tower::TowerSpec;

    fn bank() -> ElevatorBank {
        ElevatorBank::for_spec(&TowerSpec::quillon_default())
    }

    #[test]
    fn robot_ride_completes_and_wait_is_measured() {
        let mut b = bank();
        // Freight car starts at -1; pickup at -2, dropoff at 40.
        b.request(HallCall { from: -2, to: 40, kind: CarKind::RobotFreight, requested_tick: 0 });
        for t in 0..60 {
            b.step(t);
        }
        let m = b.metrics();
        assert_eq!(m.served, 1);
        assert_eq!(m.robot_share, 1.0);
        // 1 floor down at speed 2 → pickup on tick 0 or 1; wait must be tiny.
        assert!(m.p95_wait_ticks <= 2, "wait was {}", m.p95_wait_ticks);
    }

    #[test]
    fn freight_is_faster_than_cab_over_same_run() {
        let mut b = bank();
        b.request(HallCall { from: 0, to: 80, kind: CarKind::RobotFreight, requested_tick: 0 });
        b.request(HallCall { from: 0, to: 80, kind: CarKind::HumanCab, requested_tick: 0 });
        let mut robot_done_at = None;
        let mut human_done_at = None;
        for t in 0..200 {
            b.step(t);
            let m = b.metrics();
            if robot_done_at.is_none() && m.served >= 1 && m.robot_share > 0.0 {
                robot_done_at = Some(t);
            }
            if human_done_at.is_none() && m.served == 2 {
                human_done_at = Some(t);
            }
        }
        assert!(robot_done_at.unwrap() < human_done_at.unwrap());
    }

    #[test]
    fn dispatch_spreads_load_across_cars() {
        let mut b = bank();
        let mut assigned = std::collections::BTreeSet::new();
        for i in 0..4 {
            let id = b
                .request(HallCall { from: 0, to: 10 + i, kind: CarKind::RobotFreight, requested_tick: 0 })
                .unwrap();
            assigned.insert(id);
        }
        // 4 calls, 4 freight shafts → least-loaded dispatch must use all 4 cars.
        assert_eq!(assigned.len(), 4);
    }

    #[test]
    fn rush_hour_clears_within_slo() {
        let mut b = bank();
        // Morning surge: 40 robots from the bays to office floors.
        for i in 0..40u64 {
            b.request(HallCall {
                from: -1,
                to: 10 + (i as i32 % 60),
                kind: CarKind::RobotFreight,
                requested_tick: 0,
            });
        }
        for t in 0..2000 {
            b.step(t);
        }
        let m = b.metrics();
        assert_eq!(m.served, 40);
        assert_eq!(m.pending, 0);
    }
}
