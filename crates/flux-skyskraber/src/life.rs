//! A lifetime of days on ONE carried building — the chronos run.
//!
//! OK, so here's the deal. [`crate::sim`] builds a fresh tower every morning,
//! plays a scripted day and proves the day is deterministic. Notice what that
//! design can never see: anything that *accumulates*. Gold that arrives faster
//! than it leaves. A treasury that pays wages and takes nothing in. Trust
//! tables that remember. Custody history that gets longer every day. Every one
//! of those is invisible to a one-day simulation and decisive over a lifetime.
//!
//! So this module keeps ONE `Building` and plays the very same day script
//! (`sim::play_day` — the same function `run_day` calls, not a copy) on it,
//! day after day, for as many Julian years as you ask — 256 by default, the
//! Quillon emission horizon. Day 0 on the fresh building IS `run_day(seed_0)`,
//! fingerprint for fingerprint; a test pins that. After that the building
//! remembers.
//!
//! ## What is recorded
//!
//! * `days.jsonl` — one record per day: transport, vault, treasury, payroll,
//!   operating index, cortex health, the day's fingerprint, and a running
//!   BLAKE3 **head** chained over every day's fingerprint — the whole life
//!   folds to one 32-byte value, like the vault's own custody chain.
//! * `hours.jsonl` — every cortex reading (24/day): senses, decisions, the
//!   X-algo health of each organ.
//! * `ticks/year-NNNN.bin` — one 60-byte row per SECOND (see [`TICK_MAGIC`]):
//!   each car's floor / queue / load, pending calls, treasury, vault bars,
//!   rides served. This is the stream that makes the dataset large, and it is
//!   real state, not padding: a car's floor is different every second.
//! * `years.jsonl` — once a year the FULL custody chain is re-verified from
//!   genesis, and at every power-of-two year (and the last) every external
//!   anchor is replayed too; wall-clock and RSS are logged beside the result so
//!   the harness's own cost is on the record.
//! * `manifest.json` — every file with its byte count and BLAKE3, the config,
//!   the head, and the first day each cliff was reached.
//!
//! ## What is deliberately different from `run_day`, and why
//!
//! * The hourly `vault_witnessed` sense uses [`IncrementalCheck`]: only entries
//!   appended since the last check are re-hashed, and "witnessed" carries the
//!   result of the last full anchor replay. Verifying a 256-year log in full
//!   every hour is O(n²) — days of hashing that proves nothing extra.
//! * `ElevatorBank::reset_wait_window` runs at midnight, so `avg` / `p95` wait
//!   describe that day, as they do for a fresh building.
//! * Refused vault intake and failed settlements are COUNTED instead of
//!   panicking — finding the day they start is the job.
//!
//! Everything else — arrival script, gold flow, anchors, payroll, faults,
//! cortex cadence — is the live day script, untouched.

use crate::bank::TREASURY;
use crate::cortex::{CortexDecision, SubsystemSenses, SUBSYSTEMS};
use crate::culture::OperatingReport;
use crate::elevator::TransportMetrics;
use crate::sim::{self, DayOutcome, TickSink, VaultCheck};
use crate::Building;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;

/// A Julian year — the same year Quillon's emission eras count in.
pub const DAYS_PER_YEAR: f64 = 365.25;
/// Tick-file magic. Header is [`TICK_HEADER_LEN`] bytes:
/// magic[8] · row_len u16 · cars u16 · year u32 · day0 u64 · days u32 · pad.
pub const TICK_MAGIC: &[u8; 8] = b"SKYTICK1";
pub const TICK_HEADER_LEN: usize = 64;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LifeConfig {
    /// A label for the manifest: `canonical`, `endowed`, or whatever you name it.
    pub scenario: String,
    pub years: u32,
    pub seed: u64,
    pub hardened: bool,
    /// Credited to the treasury ONCE at day 0, on top of the canonical 10 M uQUG.
    pub endowment_uqug: u128,
    /// Credited to the treasury at 00:00 EVERY day. 0 = the building as written.
    pub daily_income_uqug: u128,
    pub out: PathBuf,
    /// Write the per-second tick stream (the ~1.9 GB/year part).
    pub ticks: bool,
}

/// The day-`d` seed: distinct per day, reproducible, and `day_seed(base, 0)`
/// is what the first day is fingerprinted under.
pub fn day_seed(base: u64, day: u64) -> u64 {
    base ^ (day + 1).wrapping_mul(0x9E37_79B9_7F4A_7C15)
}
pub fn days_in(years: u32) -> u64 {
    (DAYS_PER_YEAR * years as f64).floor() as u64
}
pub fn year_of(day: u64) -> u32 {
    (day as f64 / DAYS_PER_YEAR).floor() as u32
}

/// Hourly custody sense for a carried building: hash only what is new.
pub struct IncrementalCheck {
    pub verified_len: usize,
    /// Result of the most recent FULL anchor replay (`verify_witnessed`).
    pub last_full_ok: bool,
    pub chain_breaks: u64,
}

impl VaultCheck for IncrementalCheck {
    fn witnessed(&mut self, b: &Building) -> bool {
        match b.vault.verify_chain_from(self.verified_len) {
            Ok(n) => {
                self.verified_len = n;
                self.last_full_ok
            }
            Err(_) => {
                self.chain_breaks += 1;
                false
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HourRecord {
    pub day: u64,
    pub hour: u8,
    pub tick: u64,
    pub p95_wait_s: u64,
    pub pending: u64,
    pub vault_witnessed: bool,
    pub payroll_failures: u32,
    pub treasury_uqug: u128,
    pub utilization: f64,
    pub decisions: Vec<CortexDecision>,
    pub health: BTreeMap<String, f64>,
    pub building_health: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DayRecord {
    pub day: u64,
    pub year: u32,
    pub doy: u32,
    pub seed: u64,
    /// Rides completed THIS day (lifetime counter delta).
    pub served_day: u64,
    pub transport: TransportMetrics,
    pub vault_bars: usize,
    pub vault_capacity: usize,
    pub vault_refused: u32,
    pub withdraw_refused: u32,
    pub anchors: usize,
    pub audit_entries: usize,
    pub chain_ok: bool,
    pub treasury_uqug: u128,
    pub payroll_paid: u32,
    pub payroll_failed: u32,
    pub talks_held: usize,
    pub wisdom_index: f64,
    pub operating: OperatingReport,
    pub building_health: f64,
    pub refill_decisions: u32,
    pub add_car_decisions: u32,
    /// What the ACTUATORS actually did (vs the DECISIONS above): revenue billed into
    /// the treasury and bars shipped off-site. Zero every day until the wiring landed.
    pub treasury_refilled_uqug: u128,
    pub refill_actions: u32,
    pub bars_offloaded: u32,
    pub relieve_actions: u32,
    pub state_commitment: String,
    /// `sim::DayReport::report_hash` for this day.
    pub day_hash: String,
    /// BLAKE3(prev_head ‖ day_hash) — the life so far, folded.
    pub head: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct YearRecord {
    pub year: u32,
    pub days_done: u64,
    pub wall_s: f64,
    pub days_per_s: f64,
    pub rss_mb: f64,
    pub audit_entries: usize,
    pub anchors: usize,
    pub full_chain_ok: bool,
    pub witnessed_checked: bool,
    pub witnessed_ok: Option<bool>,
    pub treasury_uqug: u128,
    pub vault_bars: usize,
    pub head: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileRecord {
    pub path: String,
    pub bytes: u64,
    pub blake3: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub twin_version: String,
    pub config: LifeConfig,
    pub days: u64,
    pub head: String,
    pub first_insolvent_day: Option<u64>,
    pub first_vault_refusal_day: Option<u64>,
    pub first_refill_decision_day: Option<u64>,
    pub first_add_car_decision_day: Option<u64>,
    /// First day an actuator actually FIRED (not merely was decided).
    pub first_refill_action_day: Option<u64>,
    pub first_relieve_action_day: Option<u64>,
    /// Whole-life actuator totals.
    pub total_treasury_refilled_uqug: u128,
    pub total_bars_offloaded: u64,
    pub total_payroll_failures: u64,
    pub total_vault_refusals: u64,
    pub chain_breaks: u64,
    pub emsec_total: f64,
    pub emsec_hardened_total: f64,
    pub started_unix: u64,
    pub finished_unix: Option<u64>,
    pub files: Vec<FileRecord>,
}

/// Streams the per-second rows and the hourly cortex readings.
struct Sink {
    ticks: Option<BufWriter<File>>,
    hours: BufWriter<File>,
    day: u64,
    day0: u64,
    cars: usize,
    row: Vec<u8>,
}

impl Sink {
    fn row_len(cars: usize) -> usize {
        4 + 4 * cars + 2 + 8 + 2 + 4
    }
}

impl TickSink for Sink {
    fn on_tick(&mut self, b: &Building, tick: u64) {
        let Some(w) = self.ticks.as_mut() else { return };
        let r = &mut self.row;
        r.clear();
        r.extend_from_slice(&((tick - self.day0) as u32).to_le_bytes());
        for car in b.transport.cars().iter().take(self.cars) {
            r.extend_from_slice(&(car.floor as i16).to_le_bytes());
            r.push(car.queued().min(255) as u8);
            r.push(car.in_flight().min(255) as u8);
        }
        let pending: u64 = b.transport.cars().iter().map(|c| (c.queued() + c.in_flight()) as u64).sum();
        r.extend_from_slice(&(pending.min(u16::MAX as u64) as u16).to_le_bytes());
        r.extend_from_slice(&(b.bank.balance(TREASURY).min(u64::MAX as u128) as u64).to_le_bytes());
        r.extend_from_slice(&(b.vault.bar_count().min(u16::MAX as usize) as u16).to_le_bytes());
        r.extend_from_slice(&(b.transport.metrics_served() as u32).to_le_bytes());
        let _ = w.write_all(r);
    }
    fn on_hour(&mut self, b: &Building, tick: u64, s: &SubsystemSenses, d: &[CortexDecision]) {
        let mut health = BTreeMap::new();
        for sys in SUBSYSTEMS {
            if let Some(h) = b.cortex.system_health(sys) {
                health.insert(sys.to_string(), h);
            }
        }
        let rec = HourRecord {
            day: self.day,
            hour: ((tick - self.day0) / 3_600) as u8,
            tick,
            p95_wait_s: s.p95_wait_s,
            pending: s.pending_calls,
            vault_witnessed: s.vault_witnessed,
            payroll_failures: s.payroll_failures,
            treasury_uqug: s.treasury_uqug,
            utilization: s.utilization_index,
            decisions: d.to_vec(),
            health,
            building_health: b.cortex.building_health(),
        };
        let _ = serde_json::to_writer(&mut self.hours, &rec);
        let _ = self.hours.write_all(b"\n");
    }
}

fn rss_mb() -> f64 {
    let mut s = String::new();
    if File::open("/proc/self/statm").and_then(|mut f| f.read_to_string(&mut s)).is_ok() {
        if let Some(pages) = s.split_whitespace().nth(1).and_then(|p| p.parse::<f64>().ok()) {
            return pages * 4096.0 / 1_048_576.0;
        }
    }
    0.0
}

fn unix_now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn blake3_file(path: &Path) -> std::io::Result<(u64, String)> {
    let mut f = File::open(path)?;
    let mut h = blake3::Hasher::new();
    let mut buf = vec![0u8; 8 << 20];
    let mut n = 0u64;
    loop {
        let k = f.read(&mut buf)?;
        if k == 0 {
            break;
        }
        h.update(&buf[..k]);
        n += k as u64;
    }
    Ok((n, h.finalize().to_hex().to_string()))
}

fn write_json<T: Serialize>(path: &Path, v: &T) -> std::io::Result<()> {
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, serde_json::to_vec_pretty(v).expect("serializes"))?;
    fs::rename(tmp, path)
}

/// Run the life. Returns the manifest (also written to `out/manifest.json`).
pub fn run_life(cfg: &LifeConfig, mut progress: impl FnMut(&YearRecord)) -> std::io::Result<Manifest> {
    fs::create_dir_all(&cfg.out)?;
    if cfg.ticks {
        fs::create_dir_all(cfg.out.join("ticks"))?;
    }
    let mut b = if cfg.hardened {
        Building::quillon_hardened().expect("guardian-ring blueprint validates")
    } else {
        Building::quillon_default().expect("canonical blueprint validates")
    };
    if cfg.endowment_uqug > 0 {
        b.bank.fund_treasury(cfg.endowment_uqug);
    }
    let emsec_cfg = if cfg.hardened {
        crate::emsec::EmsecConfig::doctrine_v0_hardened()
    } else {
        crate::emsec::EmsecConfig::doctrine_v0()
    };
    let emsec_total = crate::emsec::assess(&b.spec, &emsec_cfg).total;
    let emsec_hardened_total =
        crate::emsec::assess(&b.spec, &crate::emsec::EmsecConfig::doctrine_v0_hardened()).total;

    let cars = b.transport.car_count();
    let total_days = days_in(cfg.years);
    let mut tasks: BTreeMap<String, u64> = BTreeMap::new();
    let mut check = IncrementalCheck { verified_len: 0, last_full_ok: true, chain_breaks: 0 };
    let mut sink = Sink {
        ticks: None,
        hours: BufWriter::with_capacity(1 << 20, File::create(cfg.out.join("hours.jsonl"))?),
        day: 0,
        day0: 0,
        cars,
        row: Vec::with_capacity(64),
    };
    let mut days_w = BufWriter::with_capacity(1 << 20, File::create(cfg.out.join("days.jsonl"))?);
    let mut years_w = BufWriter::new(File::create(cfg.out.join("years.jsonl"))?);

    let mut manifest = Manifest {
        twin_version: crate::state_root::TWIN_VERSION.to_string(),
        config: cfg.clone(),
        days: 0,
        head: String::new(),
        first_insolvent_day: None,
        first_vault_refusal_day: None,
        first_refill_decision_day: None,
        first_add_car_decision_day: None,
        first_refill_action_day: None,
        first_relieve_action_day: None,
        total_treasury_refilled_uqug: 0,
        total_bars_offloaded: 0,
        total_payroll_failures: 0,
        total_vault_refusals: 0,
        chain_breaks: 0,
        emsec_total,
        emsec_hardened_total,
        started_unix: unix_now(),
        finished_unix: None,
        files: Vec::new(),
    };

    let mut head = [0u8; 32];
    let mut cur_year: Option<u32> = None;
    let mut year_started = Instant::now();
    let mut year_first_day = 0u64;
    let mut tick_path: Option<PathBuf> = None;

    for day in 0..total_days {
        let year = year_of(day);
        if cur_year != Some(year) {
            // Close the previous tick file and hash it.
            if let Some(mut w) = sink.ticks.take() {
                w.flush()?;
                drop(w);
                if let Some(p) = tick_path.take() {
                    let (bytes, hash) = blake3_file(&p)?;
                    manifest.files.push(FileRecord { path: p.strip_prefix(&cfg.out).unwrap_or(&p).display().to_string(), bytes, blake3: hash });
                }
            }
            if cfg.ticks {
                let p = cfg.out.join(format!("ticks/year-{year:04}.bin"));
                let mut w = BufWriter::with_capacity(8 << 20, File::create(&p)?);
                let days_this_year = days_in(year + 1) - days_in(year);
                let mut hdr = [0u8; TICK_HEADER_LEN];
                hdr[..8].copy_from_slice(TICK_MAGIC);
                hdr[8..10].copy_from_slice(&(Sink::row_len(cars) as u16).to_le_bytes());
                hdr[10..12].copy_from_slice(&(cars as u16).to_le_bytes());
                hdr[12..16].copy_from_slice(&year.to_le_bytes());
                hdr[16..24].copy_from_slice(&day.to_le_bytes());
                hdr[24..28].copy_from_slice(&(days_this_year as u32).to_le_bytes());
                w.write_all(&hdr)?;
                sink.ticks = Some(w);
                tick_path = Some(p);
            }
            cur_year = Some(year);
            year_started = Instant::now();
            year_first_day = day;
        }

        b.transport.reset_wait_window();
        if cfg.daily_income_uqug > 0 {
            b.bank.fund_treasury(cfg.daily_income_uqug);
        }
        let seed = day_seed(cfg.seed, day);
        let day0 = day * sim::TICKS_PER_DAY;
        sink.day = day;
        sink.day0 = day0;
        let served_before = b.transport.metrics_served();

        let out: DayOutcome = sim::play_day(&mut b, seed, day0, &mut tasks, &mut sink, &mut check);

        let chain_ok = matches!(b.vault.verify_chain_from(check.verified_len), Ok(n) if { check.verified_len = n; true });
        let witnessed = chain_ok && check.last_full_ok;
        let report = sim::assemble_report(&b, seed, &out, cfg.hardened, witnessed);

        let mut h = blake3::Hasher::new();
        h.update(&head);
        h.update(report.report_hash.as_bytes());
        head = *h.finalize().as_bytes();

        let refill = out.decisions.iter().filter(|d| matches!(d, CortexDecision::RefillTreasury)).count() as u32;
        let add_car = out.decisions.iter().filter(|d| matches!(d, CortexDecision::AddRobotCar)).count() as u32;
        if out.payroll_failed > 0 && manifest.first_insolvent_day.is_none() {
            manifest.first_insolvent_day = Some(day);
        }
        if out.vault_refused > 0 && manifest.first_vault_refusal_day.is_none() {
            manifest.first_vault_refusal_day = Some(day);
        }
        if refill > 0 && manifest.first_refill_decision_day.is_none() {
            manifest.first_refill_decision_day = Some(day);
        }
        if add_car > 0 && manifest.first_add_car_decision_day.is_none() {
            manifest.first_add_car_decision_day = Some(day);
        }
        if out.refill_actions > 0 && manifest.first_refill_action_day.is_none() {
            manifest.first_refill_action_day = Some(day);
        }
        if out.relieve_actions > 0 && manifest.first_relieve_action_day.is_none() {
            manifest.first_relieve_action_day = Some(day);
        }
        manifest.total_payroll_failures += out.payroll_failed as u64;
        manifest.total_vault_refusals += out.vault_refused as u64;
        manifest.total_treasury_refilled_uqug += out.treasury_refilled_uqug;
        manifest.total_bars_offloaded += out.bars_offloaded as u64;

        let rec = DayRecord {
            day,
            year,
            doy: (day - days_in(year)) as u32,
            seed,
            served_day: report.transport.served - served_before,
            transport: report.transport.clone(),
            vault_bars: report.vault_bars,
            vault_capacity: b.vault.capacity(),
            vault_refused: out.vault_refused,
            withdraw_refused: out.withdraw_refused,
            anchors: report.vault_anchors,
            audit_entries: b.vault.audit_log().len(),
            chain_ok,
            treasury_uqug: report.treasury_uqug,
            payroll_paid: out.payroll_paid,
            payroll_failed: out.payroll_failed,
            talks_held: report.talks_held,
            wisdom_index: b.auditorium.wisdom_index(),
            operating: report.operating.clone(),
            building_health: b.cortex.building_health(),
            refill_decisions: refill,
            add_car_decisions: add_car,
            treasury_refilled_uqug: out.treasury_refilled_uqug,
            refill_actions: out.refill_actions,
            bars_offloaded: out.bars_offloaded,
            relieve_actions: out.relieve_actions,
            state_commitment: report.state_commitment.clone(),
            day_hash: report.report_hash.clone(),
            head: hex::encode(head),
        };
        serde_json::to_writer(&mut days_w, &rec).expect("day record serializes");
        days_w.write_all(b"\n")?;
        manifest.days = day + 1;
        manifest.head = hex::encode(head);

        let last_of_year = day + 1 == days_in(year + 1) || day + 1 == total_days;
        if last_of_year {
            let full_chain_ok = b.vault.verify_chain().is_ok();
            let is_pow2 = (year + 1).is_power_of_two();
            let final_year = day + 1 == total_days;
            let witnessed_ok = if is_pow2 || final_year {
                let ok = b.vault.verify_witnessed().is_ok();
                check.last_full_ok = ok;
                Some(ok)
            } else {
                None
            };
            let wall = year_started.elapsed().as_secs_f64();
            let ndays = (day + 1 - year_first_day) as f64;
            let yr = YearRecord {
                year,
                days_done: day + 1,
                wall_s: wall,
                days_per_s: if wall > 0.0 { ndays / wall } else { 0.0 },
                rss_mb: rss_mb(),
                audit_entries: b.vault.audit_log().len(),
                anchors: b.vault.anchors().len(),
                full_chain_ok,
                witnessed_checked: witnessed_ok.is_some(),
                witnessed_ok,
                treasury_uqug: b.bank.balance(TREASURY),
                vault_bars: b.vault.bar_count(),
                head: hex::encode(head),
            };
            serde_json::to_writer(&mut years_w, &yr).expect("year record serializes");
            years_w.write_all(b"\n")?;
            years_w.flush()?;
            days_w.flush()?;
            sink.hours.flush()?;
            manifest.chain_breaks = check.chain_breaks;
            write_json(&cfg.out.join("manifest.json"), &manifest)?;
            progress(&yr);
        }
    }

    if let Some(mut w) = sink.ticks.take() {
        w.flush()?;
        drop(w);
        if let Some(p) = tick_path.take() {
            let (bytes, hash) = blake3_file(&p)?;
            manifest.files.push(FileRecord { path: p.strip_prefix(&cfg.out).unwrap_or(&p).display().to_string(), bytes, blake3: hash });
        }
    }
    days_w.flush()?;
    years_w.flush()?;
    sink.hours.flush()?;
    drop(days_w);
    drop(years_w);
    drop(sink);
    for name in ["days.jsonl", "hours.jsonl", "years.jsonl"] {
        let p = cfg.out.join(name);
        let (bytes, hash) = blake3_file(&p)?;
        manifest.files.push(FileRecord { path: name.to_string(), bytes, blake3: hash });
    }
    manifest.chain_breaks = check.chain_breaks;
    manifest.finished_unix = Some(unix_now());
    write_json(&cfg.out.join("manifest.json"), &manifest)?;
    Ok(manifest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vault::Vault;

    fn cfg(dir: &Path, years: u32, ticks: bool) -> LifeConfig {
        LifeConfig {
            scenario: "test".into(),
            years,
            seed: 42,
            hardened: false,
            endowment_uqug: 0,
            daily_income_uqug: 0,
            out: dir.to_path_buf(),
            ticks,
        }
    }

    /// A year of ticks is ~1.9 GB: never on the 40 GB root. Honour
    /// SKYSKRABER_TEST_SCRATCH (the runner points it at /home/storage).
    fn scratch(name: &str) -> PathBuf {
        let base = std::env::var_os("SKYSKRABER_TEST_SCRATCH").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
        let p = base.join(format!("skyskraber-life-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&p);
        p
    }

    /// Day 0 of a life on a fresh building IS `run_day(seed_0)` — same script,
    /// same fingerprint. If this ever breaks, `play_day` has drifted from the
    /// day the whole crate is tested against.
    #[test]
    fn first_day_of_a_life_is_run_day() {
        let mut b = Building::quillon_default().unwrap();
        let mut tasks = BTreeMap::new();
        let seed = day_seed(42, 0);
        let out = sim::play_day(&mut b, seed, 0, &mut tasks, &mut sim::NoSink, &mut sim::FullCheck);
        let witnessed = b.vault.verify_chain().is_ok() && b.vault.verify_witnessed().is_ok();
        let life_day0 = sim::assemble_report(&b, seed, &out, false, witnessed);
        let fresh = sim::run_day(seed);
        assert_eq!(life_day0.report_hash, fresh.report_hash);
        assert_eq!(life_day0.state_commitment, fresh.state_commitment);
    }

    #[test]
    fn a_carried_building_remembers_and_the_life_is_deterministic() {
        let mut b = Building::quillon_default().unwrap();
        let mut tasks = BTreeMap::new();
        let mut check = IncrementalCheck { verified_len: 0, last_full_ok: true, chain_breaks: 0 };
        let mut heads = Vec::new();
        for day in 0..3u64 {
            let out = sim::play_day(&mut b, day_seed(42, day), day * sim::TICKS_PER_DAY, &mut tasks, &mut sim::NoSink, &mut check);
            assert_eq!(out.vault_refused, 0);
            heads.push(b.vault.head_hex());
        }
        // Gold accumulates: 3 in, 1 out, every day.
        assert_eq!(b.vault.bar_count(), 6);
        assert_eq!(b.vault.anchors().len(), 6);
        assert_eq!(b.auditorium.talks_held(), 3);
        assert_eq!(b.cortex.rounds(), 72);
        assert!(b.vault.verify_chain().is_ok());
        assert!(b.vault.verify_witnessed().is_ok());
        // Replay → identical custody history.
        let mut b2 = Building::quillon_default().unwrap();
        let mut tasks2 = BTreeMap::new();
        let mut check2 = IncrementalCheck { verified_len: 0, last_full_ok: true, chain_breaks: 0 };
        for day in 0..3u64 {
            sim::play_day(&mut b2, day_seed(42, day), day * sim::TICKS_PER_DAY, &mut tasks2, &mut sim::NoSink, &mut check2);
        }
        assert_eq!(b2.vault.head_hex(), *heads.last().unwrap());
    }

    #[test]
    fn a_full_vault_relieves_itself_instead_of_refusing_forever() {
        // Was `a_full_vault_refuses_and_the_refusal_is_counted`: it pinned the OLD
        // behaviour where a full vault simply refused intake for the rest of its life.
        // With the RelieveVault actuator wired, the tower ships bars off-site the hour
        // it senses the vault at capacity, so the refusal is transient, not terminal.
        let mut b = Building::quillon_default().unwrap();
        b.vault = Vault::new(4);
        let mut tasks = BTreeMap::new();
        let mut check = IncrementalCheck { verified_len: 0, last_full_ok: true, chain_breaks: 0 };
        let d0 = sim::play_day(&mut b, day_seed(1, 0), 0, &mut tasks, &mut sim::NoSink, &mut check);
        assert_eq!(d0.vault_refused, 0); // 3 in, 1 out → 2 held, never near the high-water mark
        assert_eq!(d0.relieve_actions, 0);
        let d1 = sim::play_day(&mut b, day_seed(1, 1), sim::TICKS_PER_DAY, &mut tasks, &mut sim::NoSink, &mut check);
        // 2 held + 3 offered against capacity 4 → the vault briefly fills and the third
        // bar is refused, but the fill_ratio hits 1.0 → RelieveVault fires and offloads.
        assert_eq!(d1.vault_refused, 1, "the moment of fullness is still recorded");
        assert_eq!(d1.relieve_actions, 1, "and the cortex now DOES something about it");
        assert!(d1.bars_offloaded > 0);
        assert!(b.vault.bar_count() < b.vault.capacity(), "capacity was reclaimed");
        assert!(b.vault.verify_chain().is_ok(), "offload is a chained op");
    }

    #[test]
    fn insolvency_is_a_measured_day_not_a_panic() {
        let mut b = Building::quillon_default().unwrap();
        // Drain the treasury to less than one payroll.
        let bal = b.bank.balance(TREASURY);
        b.bank.pay(TREASURY, "sink", bal - 50_000, "test drain").unwrap();
        let mut tasks = BTreeMap::new();
        let out = sim::play_day(&mut b, day_seed(3, 0), 0, &mut tasks, &mut sim::NoSink, &mut sim::FullCheck);
        assert!(out.payroll_failed > 0, "payroll should fail on an empty treasury");
        assert!(out.decisions.iter().any(|d| matches!(d, CortexDecision::RefillTreasury)));
        // The actuator now fires once the 10:00 intake gives it bars to bill, but a
        // handful of bars cannot out-earn a full payroll in one day — recovery is a
        // matter of accumulation, proven over a year below.
        assert_eq!(out.refill_actions, 1);
        assert!(out.treasury_refilled_uqug > 0);
    }

    #[test]
    fn the_treasury_dips_then_the_actuator_earns_it_back_over_a_year() {
        // The headline result of wiring the actuator: with income now flowing, the
        // building still goes insolvent early (too few bars to cover payroll) but
        // then RECOVERS as custody holdings grow, instead of flatlining at zero for
        // 256 years. One year, no ticks — small and fast.
        let dir = scratch("recovery");
        let _ = fs::remove_dir_all(&dir);
        let c = cfg(&dir, 1, false);
        let m = run_life(&c, |_| {}).unwrap();
        assert!(m.first_insolvent_day.is_some(), "the early dip still happens");
        assert!(m.first_refill_action_day.is_some(), "the actuator fires");
        assert!(m.total_treasury_refilled_uqug > 0, "real revenue flowed");
        // Read the final year's treasury: it must have climbed back off the floor.
        let years = fs::read_to_string(dir.join("years.jsonl")).unwrap();
        let last: YearRecord = serde_json::from_str(years.lines().last().unwrap()).unwrap();
        assert!(last.treasury_uqug > 0, "recovered: treasury {} > 0 at year end", last.treasury_uqug);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn run_life_writes_a_consistent_dataset() {
        let dir = scratch("dataset");
        let mut c = cfg(&dir, 1, true);
        c.years = 1;
        // Keep the test fast: 1 year is 365 days ≈ 10 s in debug; trim via seed? No — run 1 year, it is the real thing.
        let mut years = 0;
        let m = run_life(&c, |_| years += 1).unwrap();
        assert_eq!(years, 1);
        assert_eq!(m.days, 365);
        assert_eq!(m.head.len(), 64);
        assert!(m.first_insolvent_day.is_some(), "10 M uQUG cannot fund 365 payrolls");
        assert!(m.first_vault_refusal_day.is_none(), "8 000 bars are not reached in a year");
        // Tick file: header + 365 × 86 400 rows of 60 B.
        let tick = fs::metadata(dir.join("ticks/year-0000.bin")).unwrap().len();
        assert_eq!(tick, TICK_HEADER_LEN as u64 + 365 * 86_400 * 60);
        // Every file in the manifest is on disk with the stated size.
        for f in &m.files {
            assert_eq!(fs::metadata(dir.join(&f.path)).unwrap().len(), f.bytes, "{}", f.path);
        }
        let days = fs::read_to_string(dir.join("days.jsonl")).unwrap();
        assert_eq!(days.lines().count(), 365);
        let hours = fs::read_to_string(dir.join("hours.jsonl")).unwrap();
        assert_eq!(hours.lines().count(), 365 * 24);
        let _ = fs::remove_dir_all(&dir);
    }
}
