//! Local checkouts of pull requests: the settings (Settings › Local checkouts) and the record of
//! every checkout the app made, which the pull request page reads to show where its branch is and
//! the live updates use to keep it current.

mod records;
mod settings;

pub use records::{CheckoutKind, CheckoutRecord, CheckoutRecords};
pub use settings::{
    CheckoutPlace, CheckoutSettings, DEFAULT_WORKTREE_PATTERN, OpenByDefault, OpenWith,
    RememberedCheckout, TerminalChoice, WhenFinished,
};

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use review_core::AgentKind;

    use super::records::MAX_LOG_LINES;
    use super::{
        CheckoutKind, CheckoutPlace, CheckoutRecord, CheckoutRecords, CheckoutSettings,
        OpenByDefault, OpenWith, RememberedCheckout, WhenFinished,
    };
    use crate::AppSettings;
    use crate::test_support::TestDir;

    fn record(number: u64) -> CheckoutRecord {
        CheckoutRecord {
            connection_id: "gh".into(),
            repository_key: "acme/app".into(),
            number,
            clone: PathBuf::from("/dev/app"),
            path: PathBuf::from(format!("/dev/app-wt/pr-{number}-x")),
            kind: CheckoutKind::Worktree,
            branch: "feature/x".into(),
            is_own_branch: true,
            pull_ref: format!("refs/codeview/pull/{number}/head"),
            head: "abc".into(),
            last_opened: OpenWith::Agent(AgentKind::Codex),
            took_ms: 3_100,
            merged_here: false,
            log: vec!["checked out".into()],
        }
    }

    #[test]
    fn records_survive_a_restart_and_replace_their_pull_requests_earlier_one() {
        let dir = TestDir::new("checkout-records");
        let file = dir.path().join("checkouts.json");
        let mut records = CheckoutRecords::load(file.clone()).expect("no file is no records");
        assert_eq!(records.all(), []);
        records.put(record(12)).expect("saved");
        records.put(record(13)).expect("saved");
        let mut newer = record(12);
        newer.head = "def".into();
        records.put(newer).expect("saved");
        let changed = records
            .change("gh", "acme/app", 13, |record| record.append_log(["updated".to_owned()]))
            .expect("saved");
        assert!(changed);

        let loaded = CheckoutRecords::load(file).expect("loads");
        assert_eq!(loaded.all().len(), 2, "one record per pull request");
        assert_eq!(
            loaded.find("gh", "acme/app", 12).map(|record| record.head.as_str()),
            Some("def")
        );
        assert_eq!(loaded.find("gh", "acme/app", 13).unwrap().log, ["checked out", "updated"]);
        assert_eq!(loaded.worktrees_of(&PathBuf::from("/dev/app")), 2);
    }

    #[test]
    fn a_records_log_keeps_its_newest_lines() {
        let mut record = record(1);
        record.append_log((0..MAX_LOG_LINES + 5).map(|index| format!("line {index}")));
        assert_eq!(record.log.len(), MAX_LOG_LINES);
        assert_eq!(
            record.log.last().map(String::as_str),
            Some(format!("line {}", MAX_LOG_LINES + 4).as_str())
        );
    }

    #[test]
    fn an_unreadable_checkout_value_resets_only_itself() {
        let broken: AppSettings =
            serde_json::from_str(r#"{"checkouts":{"open_by_default":"Sometimes","terminal":7}}"#)
                .expect("an unknown value resets only itself");
        assert_eq!(broken.checkouts.open_by_default, OpenByDefault::Ask);
    }

    #[test]
    fn the_old_removal_switch_carries_over_to_the_choice() {
        let finished = |json: &str| {
            let settings: AppSettings = serde_json::from_str(json).expect("loads");
            settings.checkouts.when_finished
        };
        assert_eq!(
            finished(r#"{"checkouts":{"offer_removal":true}}"#),
            WhenFinished::RemoveAutomatically
        );
        assert_eq!(finished(r#"{"checkouts":{"offer_removal":false}}"#), WhenFinished::Keep);
        assert_eq!(finished(r#"{"checkouts":{}}"#), WhenFinished::RemoveAutomatically);
        assert_eq!(finished(r#"{"checkouts":{"when_finished":"Ask"}}"#), WhenFinished::Ask);
        assert_eq!(finished(r#"{"checkouts":{"when_finished":"Never"}}"#), WhenFinished::default());

        let mut settings =
            CheckoutSettings { when_finished: WhenFinished::Keep, ..CheckoutSettings::default() };
        let saved = serde_json::to_value(&settings).expect("saves");
        assert_eq!(saved["when_finished"], "Keep");
        assert!(saved.get("offer_removal").is_none(), "no old field is written");
        settings = serde_json::from_value(saved).expect("loads");
        assert_eq!(settings.when_finished, WhenFinished::Keep);
    }

    #[test]
    fn the_popover_preselects_what_was_remembered_then_the_default() {
        let mut settings =
            CheckoutSettings { last_opened: OpenWith::Terminal, ..CheckoutSettings::default() };
        assert_eq!(
            settings.preselected("gh|acme/app"),
            (CheckoutPlace::Worktree, OpenWith::Terminal)
        );
        settings.open_by_default = OpenByDefault::Always(OpenWith::Agent(AgentKind::Claude));
        assert_eq!(
            settings.preselected("gh|acme/app"),
            (CheckoutPlace::Worktree, OpenWith::Agent(AgentKind::Claude))
        );
        let remembered =
            RememberedCheckout { place: CheckoutPlace::Clone, then: OpenWith::Nothing };
        settings.remembered.insert("gh|acme/app".into(), remembered);
        assert_eq!(settings.preselected("gh|acme/app"), (CheckoutPlace::Clone, OpenWith::Nothing));

        settings.pinned_clones.insert("gone|acme/app".into(), PathBuf::from("/dev/app"));
        settings.retain_connections(|id| id == "gh");
        assert!(settings.pinned_clones.is_empty());
        assert_eq!(settings.remembered.len(), 1);
    }
}
