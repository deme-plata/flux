//! Quorum health aggregation across the supercluster.

/// A node/service health.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Health {
    /// Fully serving.
    Up,
    /// Serving with reduced capacity.
    Degraded,
    /// Not serving.
    Down,
}

/// Aggregate node healths into one cluster health: **Up** iff ≥ `quorum` are Up;
/// **Down** iff fewer than `quorum` are Up *or* Degraded; otherwise **Degraded**.
/// This is the supercluster's "are we ok" signal for dashboards + failover.
pub fn aggregate(nodes: &[Health], quorum: usize) -> Health {
    let up = nodes.iter().filter(|h| **h == Health::Up).count();
    let serving = nodes.iter().filter(|h| **h != Health::Down).count();
    if up >= quorum {
        Health::Up
    } else if serving >= quorum {
        Health::Degraded
    } else {
        Health::Down
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use Health::*;
    #[test]
    fn quorum_up_is_up() {
        assert_eq!(aggregate(&[Up, Up, Up, Down], 3), Up);
    }
    #[test]
    fn below_up_quorum_but_serving_is_degraded() {
        assert_eq!(aggregate(&[Up, Degraded, Degraded, Down], 3), Degraded);
    }
    #[test]
    fn below_serving_quorum_is_down() {
        assert_eq!(aggregate(&[Up, Down, Down, Down], 3), Down);
    }
}
