// Network interface monitoring for real traffic statistics
use std::collections::HashMap;
use std::fs;
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub struct NetworkInterfaceStats {
    pub bytes_received: u64,
    pub bytes_transmitted: u64,
    pub packets_received: u64,
    pub packets_transmitted: u64,
    pub rx_errors: u64,
    pub tx_errors: u64,
    pub last_update: Instant,
}

#[derive(Debug, Clone)]
pub struct NetworkMonitor {
    interface_stats: HashMap<String, NetworkInterfaceStats>,
    last_measurement: Instant,
}

impl NetworkMonitor {
    pub fn new() -> Self {
        Self {
            interface_stats: HashMap::new(),
            last_measurement: Instant::now(),
        }
    }

    /// Read network interface statistics from /proc/net/dev (Linux)
    pub fn update_interface_stats(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        #[cfg(target_os = "linux")]
        {
            let proc_net_dev = fs::read_to_string("/proc/net/dev")?;
            self.parse_proc_net_dev(&proc_net_dev)?;
        }
        
        #[cfg(not(target_os = "linux"))]
        {
            // For non-Linux systems, we can implement alternative methods
            // For now, return empty stats
            println!("⚠️ Network monitoring not implemented for this OS");
        }
        
        self.last_measurement = Instant::now();
        Ok(())
    }

    #[cfg(target_os = "linux")]
    fn parse_proc_net_dev(&mut self, content: &str) -> Result<(), Box<dyn std::error::Error>> {
        for line in content.lines().skip(2) { // Skip header lines
            let fields: Vec<&str> = line.split_whitespace().collect();
            if fields.len() >= 17 {
                let interface = fields[0].trim_end_matches(':');
                
                // Parse statistics (from /proc/net/dev format)
                let rx_bytes: u64 = fields[1].parse()?;
                let rx_packets: u64 = fields[2].parse()?;
                let rx_errors: u64 = fields[3].parse()?;
                let tx_bytes: u64 = fields[9].parse()?;
                let tx_packets: u64 = fields[10].parse()?;
                let tx_errors: u64 = fields[11].parse()?;

                let stats = NetworkInterfaceStats {
                    bytes_received: rx_bytes,
                    bytes_transmitted: tx_bytes,
                    packets_received: rx_packets,
                    packets_transmitted: tx_packets,
                    rx_errors,
                    tx_errors,
                    last_update: Instant::now(),
                };

                self.interface_stats.insert(interface.to_string(), stats);
            }
        }
        Ok(())
    }

    pub fn get_interface_stats(&self, interface: &str) -> Option<&NetworkInterfaceStats> {
        self.interface_stats.get(interface)
    }

    pub fn get_all_interfaces(&self) -> &HashMap<String, NetworkInterfaceStats> {
        &self.interface_stats
    }

    /// Calculate bandwidth for a specific interface since last measurement
    pub fn calculate_bandwidth(&self, interface: &str, previous_stats: &NetworkInterfaceStats) -> Option<(u64, u64)> {
        if let Some(current_stats) = self.get_interface_stats(interface) {
            let time_delta = current_stats.last_update.duration_since(previous_stats.last_update).as_secs_f64();
            
            if time_delta > 0.0 {
                let rx_bps = ((current_stats.bytes_received - previous_stats.bytes_received) as f64 / time_delta) as u64;
                let tx_bps = ((current_stats.bytes_transmitted - previous_stats.bytes_transmitted) as f64 / time_delta) as u64;
                return Some((rx_bps, tx_bps));
            }
        }
        None
    }

    /// Get traffic for VPN-related interfaces (tun, wg, etc.)
    pub fn get_vpn_traffic(&self) -> (u64, u64, u64, u64) {
        let mut total_rx_bytes = 0;
        let mut total_tx_bytes = 0;
        let mut total_rx_packets = 0;
        let mut total_tx_packets = 0;

        for (interface, stats) in &self.interface_stats {
            // Look for VPN-related interfaces
            if interface.starts_with("tun") || 
               interface.starts_with("wg") || 
               interface.starts_with("vpn") ||
               interface.contains("iron") {
                total_rx_bytes += stats.bytes_received;
                total_tx_bytes += stats.bytes_transmitted;
                total_rx_packets += stats.packets_received;
                total_tx_packets += stats.packets_transmitted;
            }
        }

        (total_rx_bytes, total_tx_bytes, total_rx_packets, total_tx_packets)
    }

    /// Estimate connection quality based on packet loss
    pub fn calculate_packet_loss(&self, interface: &str) -> f32 {
        if let Some(stats) = self.get_interface_stats(interface) {
            let total_packets = stats.packets_received + stats.packets_transmitted;
            let total_errors = stats.rx_errors + stats.tx_errors;
            
            if total_packets > 0 {
                return (total_errors as f32 / total_packets as f32) * 100.0;
            }
        }
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_network_monitor_creation() {
        let monitor = NetworkMonitor::new();
        assert!(monitor.interface_stats.is_empty());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn test_proc_net_dev_parsing() {
        let mut monitor = NetworkMonitor::new();
        let sample_data = r#"Inter-|   Receive                                                |  Transmit
 face |bytes    packets errs drop fifo frame compressed multicast|bytes    packets errs drop fifo colls carrier compressed
    lo:    1234        10    0    0    0     0          0         0     5678        20    0    0    0     0       0          0
  eth0:  123456      1000    1    0    0     0          0         0   789012      2000    2    0    0     0       0          0
"#;
        
        monitor.parse_proc_net_dev(sample_data).unwrap();
        
        let lo_stats = monitor.get_interface_stats("lo").unwrap();
        assert_eq!(lo_stats.bytes_received, 1234);
        assert_eq!(lo_stats.bytes_transmitted, 5678);
        
        let eth0_stats = monitor.get_interface_stats("eth0").unwrap();
        assert_eq!(eth0_stats.bytes_received, 123456);
        assert_eq!(eth0_stats.bytes_transmitted, 789012);
    }
}