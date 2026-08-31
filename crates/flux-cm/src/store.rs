//! JSON persistence — the whole desk in one file, written atomically
//! (tmp + rename) so a crash mid-save never leaves a torn roster.

use crate::CommunityDesk;
use std::fs;
use std::path::Path;

impl CommunityDesk {
    pub fn save(&self, path: &Path) -> Result<(), std::io::Error> {
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        let tmp = path.with_extension("tmp");
        fs::write(&tmp, json)?;
        fs::rename(&tmp, path)?;
        Ok(())
    }

    pub fn load(path: &Path) -> Result<Self, std::io::Error> {
        let json = fs::read_to_string(path)?;
        serde_json::from_str(&json)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
    }
}

#[cfg(test)]
mod tests {
    use crate::model::{Chain, Currency, WorkSpec};
    use crate::CommunityDesk;

    #[test]
    fn save_load_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("desk.json");

        let mut desk = CommunityDesk::default();
        let cm = desk
            .onboard(
                "mira",
                "Mira K",
                vec![Chain::Quillon],
                vec!["discord".into()],
                vec!["english".into()],
                Some("qnk_test_addr".into()),
                None,
                10,
            )
            .unwrap();
        desk.activate(&cm).unwrap();
        let wk = desk
            .post_work(WorkSpec {
                title: "Weekly Discord digest".into(),
                brief: "Summarize the week".into(),
                chain: Chain::Quillon,
                skills_required: vec!["discord".into()],
                bounty_base: Currency::Qug.whole(50),
                currency: Currency::Qug,
                hours_estimate: 3,
                deadline_ts_ms: None,
            })
            .unwrap();

        desk.save(&path).unwrap();
        let loaded = CommunityDesk::load(&path).unwrap();
        assert!(loaded.cm(&cm).is_some());
        assert!(loaded.work_item(&wk).is_some());
        assert_eq!(loaded.timeline().all().len(), desk.timeline().all().len());
    }
}
