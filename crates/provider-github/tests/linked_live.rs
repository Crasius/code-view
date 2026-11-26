//! Anonymous live check of linked item states against public github.com repositories. Sends two
//! unauthenticated requests, so it is ignored by default (RS-TEST-06):
//! `cargo test -p provider-github --test linked_live -- --ignored --nocapture`

use provider_github::GitHubProvider;
use review_core::{Connection, ItemKind, ItemRef, LinkedState, Provider, ProviderKind};

#[tokio::test]
#[ignore = "asks github.com anonymously; run with --ignored"]
async fn anonymous_states_of_public_items() {
    let connection = Connection {
        id: "github-live".to_owned(),
        name: "github.com".to_owned(),
        kind: ProviderKind::GitHub,
        base_url: ProviderKind::GitHub.default_base_url().to_owned(),
        organizations: Vec::new(),
        token: None,
    };
    let provider = GitHubProvider::new(connection, review_core::http::build_client());
    // rust-lang/rust#1 is an issue closed long ago; cli/cli#1 is the first merged pull request.
    let asked = [
        ItemRef {
            kind: ItemKind::IssueOrPullRequest,
            repository_key: "rust-lang/rust".to_owned(),
            number: 1,
        },
        ItemRef {
            kind: ItemKind::IssueOrPullRequest,
            repository_key: "cli/cli".to_owned(),
            number: 1,
        },
    ];
    let items = provider.linked_items(&asked).await.expect("anonymous linked items");
    for item in &items {
        println!(
            "{}#{} {:?} {:?} {:?} {}",
            item.repository_key,
            item.reference.number,
            item.kind,
            item.state,
            item.state_label,
            item.title
        );
    }
    assert_eq!(items.len(), 2);
    assert!(
        items
            .iter()
            .all(|item| item.kind != ItemKind::IssueOrPullRequest && !item.title.is_empty())
    );
    assert!(
        items
            .iter()
            .all(|item| item.state != LinkedState::Open && item.state != LinkedState::Draft),
        "both are long finished"
    );
}
