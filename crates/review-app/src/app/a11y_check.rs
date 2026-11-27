//! Accessibility names check for every harness that runs the app ([`super::ReviewApp::new`]
//! installs it in test builds): a frame whose accessibility tree names a node with an empty text
//! or with an icon glyph fails the test that painted it.
//!
//! A screen reader speaks a node's name: its label, a label widget's value (egui puts a
//! [`Role::Label`]'s text there), its placeholder and its description. A Phosphor glyph lives in
//! the private-use area and reads as nothing or as a code point, so an icon is either kept out of
//! the tree or named in words ([`cv_widgets::glyph`]). Text runs are left out: they mirror
//! the painted text of a field or a selectable label for caret navigation, and an empty field's
//! run is empty by design.
//!
//! The check runs on egui's output of every frame (a [`egui::Plugin`] output hook) rather than
//! once per walkthrough, so every page and state a walkthrough passes through is covered without
//! each walkthrough calling it, and the frame that broke it is the one that fails.

use egui::accesskit::{Node, Role};
use egui::{Context, FullOutput};

/// Adds the check to `ctx`; each later frame's tree is checked.
pub fn install(ctx: &Context) {
    ctx.add_plugin(A11yCheck);
}

struct A11yCheck;

impl egui::Plugin for A11yCheck {
    fn debug_name(&self) -> &'static str {
        "a11y_check"
    }

    fn output_hook(&mut self, _ctx: &Context, output: &mut FullOutput) {
        let Some(update) = &output.platform_output.accesskit_update else {
            return;
        };
        let found = findings(update.nodes.iter().map(|(_, node)| node));
        assert!(
            found.is_empty(),
            "screen readers would meet an icon glyph or an empty name (see cv_widgets::glyph): {found:#?}"
        );
    }
}

/// One line per name that is empty or holds an icon glyph: role, field, text and bounds. An empty
/// placeholder or description is egui's way of having none (a field without a hint reports `""`),
/// so only an empty label or label value counts as a missing name.
pub fn findings<'a>(nodes: impl Iterator<Item = &'a Node>) -> Vec<String> {
    let mut found = Vec::new();
    for node in nodes.filter(|node| node.role() != Role::TextRun) {
        for (field, text) in names(node) {
            let is_missing = matches!(field, "label" | "value") && text.trim().is_empty();
            if is_missing || text.chars().any(is_icon_glyph) {
                let bounds = node.bounds().map(|rect| (rect.x0, rect.y0));
                found.push(format!("{:?} {field} {text:?} at {bounds:?}", node.role()));
            }
        }
    }
    found
}

/// The texts a screen reader speaks for `node`, with the field each comes from.
fn names(node: &Node) -> impl Iterator<Item = (&'static str, &str)> {
    let value = (node.role() == Role::Label).then(|| node.value()).flatten();
    [
        ("label", node.label()),
        ("value", value),
        ("placeholder", node.placeholder()),
        ("description", node.description()),
    ]
    .into_iter()
    .filter_map(|(field, text)| Some((field, text?)))
}

/// A private-use code point: where the bundled icon fonts put their glyphs.
fn is_icon_glyph(c: char) -> bool {
    ('\u{E000}'..='\u{F8FF}').contains(&c)
}

#[cfg(test)]
mod tests {
    use egui::accesskit::{Node, Role};
    use egui_kittest::kittest::{NodeT, Queryable};

    use super::super::github_mock::{GitHubHost, open_diff, open_pull_request};
    use super::super::walkthrough::app;
    use super::findings;
    use cv_testkit::ScratchDir;

    #[test]
    fn a11y_pull_request_header_and_files_tab_name_no_glyph() {
        let scratch = ScratchDir::new("a11y-labels");
        let host = GitHubHost::with_pull_request(scratch.path());
        let mut harness = app(&scratch, &[host.connection("gh", "Acme")]);
        open_pull_request(&mut harness);
        open_diff(&mut harness);

        let nodes: Vec<&Node> =
            harness.query_all_by(|_| true).map(|node| node.accesskit_node().data()).collect();
        let found = findings(nodes.iter().copied());
        assert!(found.is_empty(), "{found:#?}");
        // The arrow between the branches reads as the word it stands for.
        assert!(
            harness
                .query_by(|node| {
                    node.role() == Role::Label && node.value().as_deref() == Some("into")
                })
                .is_some(),
            "the author line reads \"feature into main\""
        );
        assert!(
            harness.query_by(|node| node.placeholder() == Some("Filter files")).is_some(),
            "the file filter's hint reads without its magnifier"
        );
        assert!(
            harness.query_by_label("Copy branch name: feature").is_some()
                && harness.query_by_label("Copy branch name: main").is_some(),
            "both branch chips are on the page"
        );
    }
}
