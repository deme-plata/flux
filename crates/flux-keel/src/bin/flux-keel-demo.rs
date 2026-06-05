//! Live demo — a backend service is DOWN for ticks 3..8, then recovers.
//! flux-keel's breaker stops hammering it; cluster health degrades + recovers.
use flux_keel::{aggregate, CircuitBreaker, CircuitState, Health};
fn main() {
    let mut cb = CircuitBreaker::new(3, 2); // trip after 3 fails, probe after 2 ticks
    println!("tick │ service │ breaker    │ call?  │ cluster(5 nodes, quorum 3)");
    println!("─────┼─────────┼────────────┼────────┼───────────────────────────");
    for t in 0..14u64 {
        let down = (3..8).contains(&t);                 // the outage window
        let allowed = cb.allow(t);
        // we only actually call when the breaker allows it
        let outcome = if allowed { if down { cb.on_failure(t); "FAIL" } else { cb.on_success(); "ok" } } else { "—skip" };
        // cluster: 1 node IS this service; the other 4 stay Up
        let svc = if down { Health::Down } else { Health::Up };
        let cluster = aggregate(&[svc, Health::Up, Health::Up, Health::Up, Health::Up], 3);
        let st = match cb.state() { CircuitState::Closed=>"Closed", CircuitState::Open=>"Open  ", CircuitState::HalfOpen=>"HalfOpen" };
        println!("  {t:>2} │ {:<7} │ {st:<10} │ {outcome:<6} │ {:?}", if down {"DOWN"} else {"up"}, cluster);
    }
    println!("\n→ breaker tripped during the outage (stopped hammering), probed on recovery, closed when the service came back. Cluster degraded then recovered — no caller meltdown.");
}
