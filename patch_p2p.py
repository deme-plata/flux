"""Enhance fluxmux P2P tab v2 — live SAP/X-Algo scores, DAG gauge, rich peer list."""
path = "/home/storage/deepseek-codewhale/flux/crates/fluxmux/src/main.rs"
with open(path) as f:
    c = f.read()

# 1. Add SAP/X-Algo fields to App struct
old_struct = '    mempool_txs: u64,\n    // Self-heal'
new_struct = '    mempool_txs: u64,\n    // P2P detail\n    sap_score_avg: f64,\n    x_algo_score: f64,\n    peers_detail: Vec<(String, String, f64, String)>,\n    p2p_topics: Vec<String>,\n    // Self-heal'
c = c.replace(old_struct, new_struct)

# 2. Init new fields
old_init = '            mempool_txs: 0,\n            heals_applied: 0,'
new_init = '            mempool_txs: 0,\n            sap_score_avg: 0.0,\n            x_algo_score: 0.0,\n            peers_detail: Vec::new(),\n            p2p_topics: vec!["/flux/0/blocks".into(), "/flux/0/txs".into()],\n            heals_applied: 0,'
c = c.replace(old_init, new_init)

# 3. Enhance tick() to read P2P detail from state file
old_tick_p2p = '''                self.mempool_txs = state["mempool_txs"].as_u64().unwrap_or(0);
            }'''
new_tick_p2p = '''                self.mempool_txs = state["mempool_txs"].as_u64().unwrap_or(0);
                self.sap_score_avg = state["sap_avg"].as_f64().unwrap_or(self.sap_score_avg);
                self.x_algo_score = state["x_algo"].as_f64().unwrap_or(self.x_algo_score);
                // Parse peer details
                if let Some(peers) = state["peers"].as_array() {
                    self.peers_detail = peers.iter().filter_map(|p| {
                        let id = p["id"].as_str().unwrap_or("?");
                        let addr = p["addr"].as_str().unwrap_or("?");
                        let sap = p["sap"].as_f64().unwrap_or(0.0);
                        let agent = p["agent"].as_str().unwrap_or("?");
                        Some((id.to_string(), addr.to_string(), sap, agent.to_string()))
                    }).collect();
                }
                if let Some(topics) = state["topics"].as_array() {
                    self.p2p_topics = topics.iter().filter_map(|t| t.as_str().map(String::from)).collect();
                }
            }'''
c = c.replace(old_tick_p2p, new_tick_p2p)

# 4. Replace render_p2p with enhanced version
old_render_p2p = '''fn render_p2p(frame: &mut ratatui::Frame, area: Rect, app: &App) {
    let cols = Layout::default().direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)]).split(area);

    let p2p_text = vec![
        Line::from(vec![Span::styled("🌐 P2P Mesh Status", Style::default().fg(Color::Rgb(212, 175, 55)).add_modifier(Modifier::BOLD))]),
        Line::from(""),
        Line::from(vec![Span::raw("  Peers:         "), Span::styled(format!("{}", app.peer_count), Style::default().fg(Color::Green))]),
        Line::from(vec![Span::raw("  Latency:       "), Span::styled(format!("{:.1}ms", app.latency_ms), Style::default().fg(Color::Cyan))]),
        Line::from(vec![Span::raw("  Bandwidth:     "), Span::styled(format!("{:.0} Kbps", app.bandwidth_kbps), Style::default().fg(Color::Rgb(6, 182, 212)))]),
        Line::from(vec![Span::raw("  Transport:     "), Span::styled("TCP + Noise + Yamux", Style::default().fg(Color::Green))]),
        Line::from(vec![Span::raw("  Gossipsub:     "), Span::styled("v1.1 (5 topics)", Style::default().fg(Color::Cyan))]),
        Line::from(""),
        Line::from(vec![Span::styled("  Delta (5.79.79.158):    ", Style::default().fg(Color::DarkGray)), Span::styled("⏳ connecting", Style::default().fg(Color::Yellow))]),
        Line::from(vec![Span::styled("  Epsilon (89.149.241):   ", Style::default().fg(Color::DarkGray)), Span::styled("✅ synced 18.2M", Style::default().fg(Color::Green))]),
        Line::from(""),
        Line::from(vec![Span::styled("  [🔄 Restart Swarm] ", Style::default().bg(Color::Rgb(180, 120, 0)).fg(Color::Black))]),
        Line::from(vec![Span::styled("  [📊 Sniff Traffic] ", Style::default().bg(Color::Rgb(6, 182, 212)).fg(Color::Black))]),
    ];
    frame.render_widget(
        Paragraph::new(p2p_text).block(Block::default().borders(Borders::ALL).title(" Swarm ").border_style(Style::default().fg(Color::Rgb(212, 175, 55)))),
        cols[0],
    );

    // Right: peer list
    let peers = vec![
        ListItem::new(Line::from(vec![Span::styled("✅ 12D3KooW...MpxM", Style::default().fg(Color::Green)), Span::raw("  Epsilon  |  12ms  |  v10.11.31")])),
        ListItem::new(Line::from(vec![Span::styled("⏳ 12D3KooW...Delta", Style::default().fg(Color::Yellow)), Span::raw("  Delta    |  ???   |  ???")])),
    ];
    frame.render_widget(
        List::new(peers).block(Block::default().borders(Borders::ALL).title(" Peers ")),
        cols[1],
    );
}'''

new_render_p2p = '''fn render_p2p(frame: &mut ratatui::Frame, area: Rect, app: &App) {
    let cols = Layout::default().direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(40), Constraint::Percentage(60)]).split(area);

    // Left: status + gauges
    let left_items = Layout::default().direction(Direction::Vertical)
        .constraints([Constraint::Length(9), Constraint::Length(3), Constraint::Min(0)])
        .split(cols[0]);

    let p2p_text = vec![
        Line::from(vec![Span::styled("🌐 P2P Mesh v2", Style::default().fg(Color::Rgb(212, 175, 55)).add_modifier(Modifier::BOLD))]),
        Line::from(""),
        Line::from(vec![Span::raw("  Peers:    "), Span::styled(format!("{}", app.peer_count), Style::default().fg(Color::Green)),
                       Span::raw("  │  DAG: "), Span::styled(format!("r{}", app.dag_round), Style::default().fg(Color::Cyan))]),
        Line::from(vec![Span::raw("  Mempool:  "), Span::styled(format!("{} tx", app.mempool_txs), Style::default().fg(Color::Yellow)),
                       Span::raw("  │  BW:  "), Span::styled(format!("{:.0}Kbps", app.bandwidth_kbps), Style::default().fg(Color::Rgb(6, 182, 212)))]),
        Line::from(vec![Span::raw("  SAP avg:  "), Span::styled(format!("{:.2}", app.sap_score_avg), Style::default().fg(if app.sap_score_avg > 0.5 { Color::Green } else { Color::Yellow }))]),
        Line::from(vec![Span::raw("  X-Algo:   "), Span::styled(format!("{:.2}", app.x_algo_score), Style::default().fg(Color::Rgb(212, 175, 55)))]),
        Line::from(vec![Span::raw("  Topics:   "), Span::styled(format!("{}", app.p2p_topics.len()), Style::default().fg(Color::Cyan))]),
    ];
    frame.render_widget(
        Paragraph::new(p2p_text).block(Block::default().borders(Borders::ALL).title(" Status ")),
        left_items[0],
    );

    // SAP gauge
    let sap_pct = (app.sap_score_avg * 100.0).min(100.0) as u16;
    let sap_gauge = Gauge::default()
        .block(Block::default().borders(Borders::ALL).title(" SAP Score "))
        .gauge_style(Style::default().fg(if sap_pct > 60 { Color::Green } else { Color::Yellow }).bg(Color::Rgb(20, 20, 40)))
        .percent(sap_pct);
    frame.render_widget(sap_gauge, left_items[1]);

    // Topics list
    let topic_items: Vec<ListItem> = app.p2p_topics.iter().map(|t| {
        ListItem::new(Line::from(vec![Span::styled("📡 ", Style::default().fg(Color::Cyan)), Span::raw(t)]))
    }).collect();
    frame.render_widget(
        List::new(topic_items).block(Block::default().borders(Borders::ALL).title(" Gossipsub Topics ")),
        left_items[2],
    );

    // Right: peer list with SAP scores
    let mut peer_items: Vec<ListItem> = app.peers_detail.iter().map(|(id, addr, sap, agent)| {
        let icon = if *sap > 0.5 { "✅" } else { "⏳" };
        let color = if *sap > 0.5 { Color::Green } else { Color::Yellow };
        let sap_bar = "█".repeat((sap * 10.0).min(10.0) as usize);
        ListItem::new(Line::from(vec![
            Span::styled(format!("{} {} ", icon, &id[..id.len().min(12)]), Style::default().fg(color)),
            Span::raw(format!(" {}  ", addr)),
            Span::styled(format!("SAP:{:.1} ", sap), Style::default().fg(Color::Rgb(212, 175, 55))),
            Span::styled(sap_bar, Style::default().fg(Color::Rgb(6, 182, 212))),
        ]))
    }).collect();
    if peer_items.is_empty() {
        peer_items.push(ListItem::new("No peers connected — start node to populate"));
    }
    frame.render_widget(
        List::new(peer_items).block(Block::default().borders(Borders::ALL).title(" Peers (SAP-ranked) ")),
        cols[1],
    );
}'''

c = c.replace(old_render_p2p, new_render_p2p)

with open(path, 'w') as f:
    f.write(c)
print("OK: Enhanced fluxmux P2P tab v2")
