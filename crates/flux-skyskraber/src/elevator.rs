//! Robot elevator transport — v0.2, in real physical units.
//!
//! v0.1 moved cars in "floors per tick" with one job per car; its queueing
//! model correctly predicted its own collapse, but its physics were toy
//! (a 70-floor human ride took 70 minutes). v0.2 replaces that with the
//! model an elevator engineer would recognize: **one tick = one second**,
//! and a trip is
//!
//!   T_route = Σ legs T_kinematic(d) + stops × (T_door + T_load)
//!
//! where each leg follows a trapezoidal velocity profile: accelerate at `a`
//! to at most `v_max`, cruise, decelerate. Short hops never reach cruise
//! (triangular profile, T = 2·√(d/a)); long runs do (T = d/v + v/a).
//!
//! Batching is now REAL, not assumed: an idle car pops up to `capacity`
//! same-direction jobs from its queue and serves them as one multi-stop
//! route. The realized mean batch E[b] is *measured* and reported — the
//! whitepaper's stability condition ρ = λ·E[T_route]/(N·E[b]) < 1 is
//! checked against what the simulation actually achieved, not against the
//! nameplate capacity.

use crate::tower::TowerSpec;
use serde::{Deserialize, Serialize};

/// Nominal floor-to-floor height.
pub const FLOOR_HEIGHT_M: f64 = 4.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CarKind {
    /// Robot freight car: pallet-sized, no comfort constraints, fast.
    RobotFreight,
    /// Human cab: comfort-limited acceleration.
    HumanCab,
}

impl CarKind {
    pub fn v_max_ms(self) -> f64 {
        match self {
            CarKind::RobotFreight => 8.0,
            CarKind::HumanCab => 6.0,
        }
    }
    pub fn accel_ms2(self) -> f64 {
        match self {
            CarKind::RobotFreight => 1.2,
            CarKind::HumanCab => 1.0,
        }
    }
    /// Door open+close per stop, seconds.
    pub fn door_s(self) -> f64 {
        match self {
            CarKind::RobotFreight => 4.0,
            CarKind::HumanCab => 3.0,
        }
    }
    /// Loading/boarding per stop, seconds.
    pub fn load_s(self) -> f64 {
        match self {
            CarKind::RobotFreight => 6.0,
            CarKind::HumanCab => 8.0,
        }
    }
    /// Batch capacity: pallets or riders sharing one route.
    pub fn capacity(self) -> usize {
        match self {
            CarKind::RobotFreight => 8,
            CarKind::HumanCab => 12,
        }
    }
}

/// Kinematic leg time over `floors` floors: trapezoidal velocity profile.
pub fn leg_time_s(floors: i32, kind: CarKind) -> f64 {
    let d = (floors.abs() as f64) * FLOOR_HEIGHT_M;
    if d == 0.0 {
        return 0.0;
    }
    let v = kind.v_max_ms();
    let a = kind.accel_ms2();
    let d_crit = v * v / a; // distance to reach and shed v_max
    if d >= d_crit {
        d / v + v / a
    } else {
        2.0 * (d / a).sqrt()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HallCall {
    pub from: i32,
    pub to: i32,
    pub kind: CarKind,
    /// Request time, in ticks (seconds).
    pub requested_tick: u64,
}

impl HallCall {
    fn dir_up(&self) -> bool {
        self.to >= self.from
    }
}

#[derive(Debug, Clone)]
pub struct Car {
    pub id: u32,
    pub kind: CarKind,
    pub floor: i32,
    queue: std::collections::VecDeque<HallCall>,
    /// When the current route completes (car is busy until then).
    busy_until: u64,
    /// Jobs in the current route (served when busy_until passes).
    in_flight: usize,
}

impl Car {
    fn load(&self) -> usize {
        self.queue.len() + self.in_flight
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TransportMetrics {
    pub served: u64,
    pub pending: u64,
    /// Mean hall-call wait, seconds.
    pub avg_wait_s: f64,
    /// 95th-percentile wait, seconds.
    pub p95_wait_s: u64,
    /// Share of served rides that were robot freight (0–1).
    pub robot_share: f64,
    /// Measured mean realized batch E[b] (jobs per route).
    pub mean_batch: f64,
    /// Measured mean route time E[T_route], seconds.
    pub mean_route_s: f64,
}

pub struct ElevatorBank {
    cars: Vec<Car>,
    waits: Vec<u64>,
    served_robot: u64,
    served_human: u64,
    routes_completed: u64,
    total_route_s: f64,
}

impl ElevatorBank {
    /// One car per shaft, parked where its riders live.
    pub fn for_spec(spec: &TowerSpec) -> Self {
        let mut cars = Vec::new();
        let mut id = 0;
        for _ in 0..spec.robot_shafts {
            cars.push(Car { id, kind: CarKind::RobotFreight, floor: -1, queue: Default::default(), busy_until: 0, in_flight: 0 });
            id += 1;
        }
        for _ in 0..spec.human_shafts {
            cars.push(Car { id, kind: CarKind::HumanCab, floor: 0, queue: Default::default(), busy_until: 0, in_flight: 0 });
            id += 1;
        }
        Self { cars, waits: Vec::new(), served_robot: 0, served_human: 0, routes_completed: 0, total_route_s: 0.0 }
    }

    pub fn car_count(&self) -> usize {
        self.cars.len()
    }

    /// Destination dispatch: least-loaded matching car, ties by proximity.
    pub fn request(&mut self, call: HallCall) -> Option<u32> {
        let car = self
            .cars
            .iter_mut()
            .filter(|c| c.kind == call.kind)
            .min_by_key(|c| (c.load(), (c.floor - call.from).abs()))?;
        car.queue.push_back(call);
        Some(car.id)
    }

    /// Advance to time `now` (seconds). Completed routes settle; idle cars
    /// assemble the next same-direction batch and launch it.
    pub fn step(&mut self, now: u64) {
        for car in self.cars.iter_mut() {
            if car.busy_until > now {
                continue;
            }
            // Settle the finished route.
            if car.in_flight > 0 {
                match car.kind {
                    CarKind::RobotFreight => self.served_robot += car.in_flight as u64,
                    CarKind::HumanCab => self.served_human += car.in_flight as u64,
                }
                car.in_flight = 0;
            }
            if car.queue.is_empty() {
                continue;
            }
            // Assemble a batch: the head job fixes the direction; take up to
            // capacity same-direction jobs (destination-dispatch grouping).
            let head_up = car.queue[0].dir_up();
            let cap = car.kind.capacity();
            let mut batch: Vec<HallCall> = Vec::new();
            let mut i = 0;
            while i < car.queue.len() && batch.len() < cap {
                if car.queue[i].dir_up() == head_up {
                    batch.push(car.queue.remove(i).unwrap());
                } else {
                    i += 1;
                }
            }

            // Route: current floor → pickups (sorted along travel) → dropoffs
            // (sorted along travel). Unique stops each pay door + load time.
            let mut pickups: Vec<i32> = batch.iter().map(|c| c.from).collect();
            let mut drops: Vec<i32> = batch.iter().map(|c| c.to).collect();
            pickups.sort_unstable();
            pickups.dedup();
            drops.sort_unstable();
            drops.dedup();
            if !head_up {
                pickups.reverse();
                drops.reverse();
            }
            let stop_cost = car.kind.door_s() + car.kind.load_s();

            let mut t = now as f64;
            let mut here = car.floor;
            let mut pickup_time: std::collections::BTreeMap<i32, u64> = Default::default();
            for &p in &pickups {
                t += leg_time_s(p - here, car.kind);
                t += stop_cost;
                here = p;
                pickup_time.insert(p, t as u64);
            }
            for &d in &drops {
                t += leg_time_s(d - here, car.kind);
                t += stop_cost;
                here = d;
            }

            for call in &batch {
                let picked = *pickup_time.get(&call.from).unwrap_or(&now);
                self.waits.push(picked.saturating_sub(call.requested_tick));
            }

            car.in_flight = batch.len();
            car.floor = here;
            car.busy_until = t.ceil() as u64;
            self.routes_completed += 1;
            self.total_route_s += t - now as f64;
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
        let mean_batch = if self.routes_completed == 0 {
            0.0
        } else {
            (served + self.cars.iter().map(|c| c.in_flight as u64).sum::<u64>()) as f64
                / self.routes_completed as f64
        };
        let mean_route_s = if self.routes_completed == 0 {
            0.0
        } else {
            self.total_route_s / self.routes_completed as f64
        };
        TransportMetrics { served, pending, avg_wait_s: avg, p95_wait_s: p95, robot_share, mean_batch, mean_route_s }
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
    fn kinematics_match_hand_arithmetic() {
        // Robot, 70 floors = 280 m ≥ d_crit = 8²/1.2 = 53.3 m →
        // T = 280/8 + 8/1.2 = 35 + 6.67 = 41.67 s.
        let t = leg_time_s(70, CarKind::RobotFreight);
        assert!((t - 41.67).abs() < 0.05, "t={t}");
        // Human, 2 floors = 8 m < d_crit = 36 m → T = 2·√(8/1.0) = 5.66 s.
        let t2 = leg_time_s(2, CarKind::HumanCab);
        assert!((t2 - 5.66).abs() < 0.05, "t2={t2}");
        // A 70-floor human ride is now ~53 s, not 70 minutes.
        let t3 = leg_time_s(70, CarKind::HumanCab);
        assert!(t3 < 60.0, "t3={t3}");
    }

    #[test]
    fn single_ride_completes_in_realistic_time() {
        let mut b = bank();
        b.request(HallCall { from: -1, to: 40, kind: CarKind::RobotFreight, requested_tick: 0 });
        for t in 0..300 {
            b.step(t);
        }
        let m = b.metrics();
        assert_eq!(m.served, 1);
        assert_eq!(m.robot_share, 1.0);
        // Pickup at the car's own floor: wait = one stop cost ≈ 10 s.
        assert!(m.p95_wait_s <= 12, "wait {}", m.p95_wait_s);
        // Route: 41 floors ≈ 27 s travel + 2 stops × 10 s ≈ 47 s.
        assert!(m.mean_route_s > 30.0 && m.mean_route_s < 70.0, "route {}", m.mean_route_s);
    }

    #[test]
    fn batching_is_real_and_measured() {
        let mut b = bank();
        // 8 same-direction calls dumped on one instant; 4 cars → the
        // least-loaded dispatch spreads 2 per car, so E[b] ≈ 2.
        for i in 0..8 {
            b.request(HallCall { from: -1, to: 20 + i, kind: CarKind::RobotFreight, requested_tick: 0 });
        }
        for t in 0..600 {
            b.step(t);
        }
        let m = b.metrics();
        assert_eq!(m.served, 8);
        assert!(m.mean_batch >= 1.9, "E[b]={}", m.mean_batch);
    }

    #[test]
    fn one_car_swallows_a_full_batch() {
        let mut spec = TowerSpec::quillon_default();
        spec.robot_shafts = 1;
        spec.human_shafts = 0;
        let mut b = ElevatorBank::for_spec(&spec);
        for i in 0..8 {
            b.request(HallCall { from: -1, to: 30 + i, kind: CarKind::RobotFreight, requested_tick: 0 });
        }
        for t in 0..600 {
            b.step(t);
        }
        let m = b.metrics();
        assert_eq!(m.served, 8);
        // One car, capacity 8, all same direction → exactly one route.
        assert!((m.mean_batch - 8.0).abs() < 1e-9, "E[b]={}", m.mean_batch);
    }

    #[test]
    fn opposite_directions_do_not_share_a_batch() {
        let mut spec = TowerSpec::quillon_default();
        spec.robot_shafts = 1;
        spec.human_shafts = 0;
        let mut b = ElevatorBank::for_spec(&spec);
        b.request(HallCall { from: 0, to: 40, kind: CarKind::RobotFreight, requested_tick: 0 });
        b.request(HallCall { from: 40, to: 0, kind: CarKind::RobotFreight, requested_tick: 0 });
        for t in 0..600 {
            b.step(t);
        }
        let m = b.metrics();
        assert_eq!(m.served, 2);
        assert!((m.mean_batch - 1.0).abs() < 1e-9, "two routes expected, E[b]={}", m.mean_batch);
    }

    #[test]
    fn rush_hour_clears_with_batching() {
        let mut b = bank();
        for i in 0..40u64 {
            b.request(HallCall { from: -1, to: 10 + (i as i32 % 60), kind: CarKind::RobotFreight, requested_tick: 0 });
        }
        for t in 0..3600 {
            b.step(t);
        }
        let m = b.metrics();
        assert_eq!(m.served, 40);
        assert_eq!(m.pending, 0);
        // 40 jobs over 4 cars with capacity 8 → at most ~2 routes per car:
        // the whole surge clears far inside the hour.
        assert!(m.p95_wait_s < 600, "p95 {}", m.p95_wait_s);
    }
}
