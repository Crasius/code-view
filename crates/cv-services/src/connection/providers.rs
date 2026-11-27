use std::collections::HashMap;
use std::sync::Arc;

use review_core::{Connection, Provider, ProviderError};

use super::access_issues::{AccessIssues, WatchedProvider};

/// Builds a provider for a connection. Injected by `main` so this module stays platform-neutral.
pub type ProviderFactory = fn(Connection, reqwest::Client) -> Arc<dyn Provider>;

/// One provider handle per connection, kept for the connection's lifetime: when the connection
/// changes (a new token, other organizations), the provider underneath is rebuilt and swapped in,
/// so every page, load and cache that holds the handle uses the new settings from its next call
/// on. Stored connections' providers report token problems to [`ProviderRegistry::issues`].
pub struct ProviderRegistry {
    /// Any closure, so a test hands the fixture it captured instead of looking it up in a static.
    factory: Box<dyn Fn(Connection, reqwest::Client) -> Arc<dyn Provider> + Send + Sync>,
    http: reqwest::Client,
    providers: HashMap<String, (Connection, Arc<WatchedProvider>)>,
    issues: AccessIssues,
}

impl ProviderRegistry {
    pub fn new(
        factory: impl Fn(Connection, reqwest::Client) -> Arc<dyn Provider> + Send + Sync + 'static,
    ) -> Self {
        Self {
            factory: Box::new(factory),
            http: http_client(),
            providers: HashMap::new(),
            issues: AccessIssues::default(),
        }
    }

    /// The connection's handle, its provider rebuilt first when `connection` differs from the one it
    /// was built for.
    pub fn get(&mut self, connection: &Connection) -> Arc<dyn Provider> {
        match self.providers.get_mut(&connection.id) {
            Some((cached, provider)) if cached == connection => provider.clone(),
            // Same platform: the new provider goes underneath the handle everybody holds.
            Some((cached, provider)) if cached.kind == connection.kind => {
                provider.replace((self.factory)(connection.clone(), self.http.clone()));
                *cached = connection.clone();
                provider.clone()
            }
            _ => {
                let inner = (self.factory)(connection.clone(), self.http.clone());
                let provider = Arc::new(WatchedProvider::new(
                    inner,
                    connection.id.clone(),
                    self.issues.clone(),
                ));
                self.providers
                    .insert(connection.id.clone(), (connection.clone(), provider.clone()));
                provider
            }
        }
    }

    /// A provider for a connection that is not stored yet (checking a draft). Its errors are not recorded.
    pub fn build_uncached(&self, connection: &Connection) -> Arc<dyn Provider> {
        (self.factory)(connection.clone(), self.http.clone())
    }

    /// After `connection` was saved with new settings: its handle takes them at once (no page has
    /// to be reopened) and its recorded token problem is cleared.
    pub fn update(&mut self, connection: &Connection) {
        self.get(connection);
        self.issues.clear(&connection.id);
    }

    /// Drops the provider and its recorded token problem, after the connection was removed.
    pub fn remove(&mut self, connection_id: &str) {
        self.providers.remove(connection_id);
        self.issues.clear(connection_id);
    }

    /// Token problems seen on stored connections (401/403 explained by the provider).
    pub fn issues(&self) -> &AccessIssues {
        &self.issues
    }
}

#[cfg(not(any(test, feature = "test-support")))]
fn http_client() -> reqwest::Client {
    review_core::http::build_client()
}

/// Tests reach their mock servers only (see `cv_testkit`).
#[cfg(any(test, feature = "test-support"))]
fn http_client() -> reqwest::Client {
    cv_testkit::http_client()
}

pub fn unknown_connection(connection_id: &str) -> ProviderError {
    ProviderError::Local(format!("The connection '{connection_id}' no longer exists."))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use review_core::{Connection, ProviderKind};
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::ProviderRegistry;

    /// What an open page holds keeps working with the connection's newest token after a save.
    #[tokio::test]
    async fn a_held_provider_takes_the_saved_token() {
        let server = MockServer::start().await;
        let user = serde_json::json!({ "id": 1, "login": "mona" });
        Mock::given(method("GET"))
            .and(path("/api/v3/user"))
            .and(header("authorization", "Bearer fixed-token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(user))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/v3/user"))
            .respond_with(ResponseTemplate::new(401))
            .mount(&server)
            .await;
        let connection = Connection {
            id: "gh".to_owned(),
            name: "mock".to_owned(),
            kind: ProviderKind::GitHub,
            base_url: server.uri(),
            organizations: Vec::new(),
            token: Some("stale-token".to_owned()),
        };
        let mut registry = ProviderRegistry::new(cv_providers::create_provider);
        let held = registry.get(&connection);
        assert_eq!(held.current_user().await.unwrap_err().status(), Some(401));

        registry.update(&Connection { token: Some("fixed-token".to_owned()), ..connection });
        assert_eq!(
            held.current_user().await.expect("the new token").login.as_deref(),
            Some("mona")
        );
    }

    /// A test hands its fixture to the registry through the closure, not through a static.
    #[tokio::test]
    async fn a_registry_built_from_a_closure_hands_out_the_provider_it_captured() {
        let server = MockServer::start().await;
        let user = serde_json::json!({ "id": 1, "login": "mona" });
        Mock::given(method("GET"))
            .and(path("/api/v3/user"))
            .respond_with(ResponseTemplate::new(200).set_body_json(user))
            .mount(&server)
            .await;
        let fixture = Connection {
            id: "fixture".to_owned(),
            name: "mock".to_owned(),
            kind: ProviderKind::GitHub,
            base_url: server.uri(),
            organizations: Vec::new(),
            token: Some("token".to_owned()),
        };
        let captured = cv_providers::create_provider(fixture.clone(), cv_testkit::http_client());
        let handed = Arc::clone(&captured);
        let mut registry = ProviderRegistry::new(move |_, _| Arc::clone(&handed));
        // Nothing listens on the discard port: only the captured provider reaches the mock.
        let asked = Connection {
            id: "asked".to_owned(),
            base_url: "http://127.0.0.1:9".to_owned(),
            ..fixture
        };

        assert!(Arc::ptr_eq(&registry.build_uncached(&asked), &captured));
        assert_eq!(
            registry
                .get(&asked)
                .current_user()
                .await
                .expect("the captured provider")
                .login
                .as_deref(),
            Some("mona")
        );
    }
}
