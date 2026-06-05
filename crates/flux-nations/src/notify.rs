//! Email a citizen via flux-email (e.g. on admission).
use flux_email::{EmailAddress, EmailEvent, NotificationEvent, SendError, EmailSender};
use crate::nations::Nation;

fn hex8(h: &[u8; 32]) -> String { format!("{:02x}{:02x}{:02x}{:02x}", h[0], h[1], h[2], h[3]) }

/// Email `to` that they've been admitted to `nation` as `alias`, through any
/// flux-email [`Sender`] (test sender, SMTP relay, …). Includes the committed
/// citizen-root so the citizen can independently verify their membership.
pub fn notify_admitted(sender: &dyn EmailSender, to: &EmailAddress, nation: &Nation, alias: &str) -> Result<(), SendError> {
    let evt = EmailEvent::Notification(NotificationEvent {
        subject: format!("Welcome to {} — citizenship confirmed", nation.name),
        body: format!(
            "{alias}, your citizenship in {} is confirmed.\nCitizen root: {} (verify your membership proof against it).",
            nation.name, hex8(&nation.citizen_root())
        ),
    });
    sender.send(to, &evt)
}

#[cfg(test)]
mod tests {
    use super::*;
    use flux_email::InMemorySender;
    #[test]
    fn admitted_citizen_gets_an_email() {
        let nation = Nation::new("SIGIL Nation", [9u8; 32]);
        let sender = InMemorySender::new();
        let to = EmailAddress::parse("rocky@sigilgraph.com").unwrap();
        assert!(notify_admitted(&sender, &to, &nation, "rocky").is_ok());
        assert_eq!(sender.count(), 1);
        let (addr, evt) = sender.sent().into_iter().next().unwrap();
        assert_eq!(addr.as_str(), "rocky@sigilgraph.com");
        match evt { EmailEvent::Notification(n) => assert!(n.subject.contains("SIGIL Nation")), _ => panic!("wrong event") }
    }
}
