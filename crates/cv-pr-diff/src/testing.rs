//! The shared fixture of the diff view's tests: a real Rust file in two versions, colored the way
//! the app colors it, shown through [`DiffView`] in a kittest harness that records what the view
//! asks for. It lives here, behind the `test-support` feature, because the tests of the rendered
//! diff, the folder summary and the Files tab drive the same view from their own crates.

use std::sync::Arc;

use cv_base::theme;
use cv_code::contents::{Coloring, FileContents, SideContents};
use cv_code::diff_rows::Layout;
use cv_pr_threads::ThreadsUi;
use cv_syntax::blob_text::BlobText;
use cv_syntax::grammar;
use cv_syntax::highlight::{self, LineSpans};
use egui_kittest::Harness;
use review_core::{
    NewThread, ProviderKind, ReviewThread, ThreadAnchor, ThreadComment, ThreadStatus, UserIdentity,
};
use review_git::FileDiff;
use review_store::{PaneSizes, ThemePreference};

use crate::diff_view::{DiffEvent, DiffInputs, DiffView};
use crate::find::FindInputs;

pub const PATH: &str = "src/cart.rs";

pub const OLD: &str = r#"use std::collections::HashMap;
use std::fmt;

/// A shopping cart that totals prices in cents.
pub struct Cart {
    items: Vec<Item>,
    discounts: HashMap<String, u32>,
}

pub struct Item {
    pub name: String,
    pub price_cents: u64,
    pub quantity: u32,
}

impl Cart {
    pub fn new() -> Self {
        Self { items: Vec::new(), discounts: HashMap::new() }
    }

    pub fn add(&mut self, name: &str, price_cents: u64) {
        println!("adding {name}");
        self.items.push(Item { name: name.to_owned(), price_cents, quantity: 1 });
    }

    pub fn total(&self) -> u64 {
        let mut total = 0;
        for item in &self.items {
            total += item.price_cents * item.quantity as u64;
        }
        total
    }

    pub fn apply_discount(&mut self, code: &str, percent: u32) {
        // Codes are case-insensitive.
        let code = code.to_lowercase();
        self.discounts.insert(code, percent);
    }

    pub fn clear(&mut self) {
        self.items.clear();
        self.discounts.clear();
    }
}

impl fmt::Display for Cart {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for item in &self.items {
            writeln!(f, "{} x{}", item.name, item.quantity)?;
        }
        write!(f, "total: {}", self.total())
    }
}
"#;

pub const NEW: &str = r#"use std::collections::HashMap;
use std::fmt;

/// A shopping cart that totals prices in cents.
pub struct Cart {
    items: Vec<Item>,
    discounts: HashMap<String, u32>,
}

pub struct Item {
    pub name: String,
    pub price_cents: u64,
    pub quantity: u32,
}

impl Cart {
    pub fn new() -> Self {
        Self { items: Vec::new(), discounts: HashMap::new() }
    }

    pub fn add(&mut self, name: &str, price_cents: u64, quantity: u32) {
        self.items.push(Item { name: name.to_owned(), price_cents, quantity });
    }

    pub fn total(&self) -> u64 {
        self.items.iter().map(|item| item.price_cents * u64::from(item.quantity)).sum()
    }

    pub fn apply_discount(&mut self, code: &str, percent: u32) {
        let code = code.to_lowercase();
        self.discounts.insert(code, percent);
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn clear(&mut self) {
        self.items.clear();
        self.discounts.clear();
    }
}

impl fmt::Display for Cart {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for item in &self.items {
            writeln!(f, "{} x{}", item.name, item.quantity)?;
        }
        write!(f, "total: {}", self.total())
    }
}
"#;

pub struct Fixture {
    pub view: DiffView,
    pub diff: Arc<FileDiff>,
    pub contents: FileContents,
    pub threads: Vec<ReviewThread>,
    pub layout: Layout,
    pub theme: ThemePreference,
    pub threads_ui: ThreadsUi,
    /// The old side's share of a side-by-side diff.
    pub split: f32,
    pub events: Vec<&'static str>,
    /// Comments the view asked to create.
    pub created: Vec<NewThread>,
    /// GitHub takes comments only on lines of the diff's hunks.
    pub can_comment_outside_hunks: bool,
    /// Long lines wrap at the pane's width.
    pub wrap: bool,
    /// The code size, as the code zoom sets it.
    pub font_size: f32,
    /// "Show comments in the diff".
    pub show_threads: bool,
    /// "Highlight other occurrences of the selection".
    pub find: FindInputs,
}

pub fn fixture(layout: Layout) -> Fixture {
    let unified =
        similar::TextDiff::from_lines(OLD, NEW).unified_diff().context_radius(3).to_string();
    Fixture {
        view: DiffView::default(),
        diff: Arc::new(FileDiff::parse_unified(unified.as_bytes())),
        contents: FileContents {
            old: Some(side(OLD)),
            new: Some(side(NEW)),
            coloring: Coloring::Done,
            error: None,
        },
        threads: Vec::new(),
        layout,
        theme: ThemePreference::Light,
        threads_ui: ThreadsUi::for_tests(ProviderKind::GitHub),
        split: PaneSizes::DEFAULT.diff_split,
        events: Vec::new(),
        created: Vec::new(),
        can_comment_outside_hunks: true,
        wrap: false,
        font_size: 13.0,
        show_threads: true,
        find: FindInputs::default(),
    }
}

/// One side of the file, highlighted the way the app does it.
pub fn side(source: &str) -> SideContents {
    let text = BlobText::from_bytes(source.as_bytes().to_vec());
    let grammar =
        grammar::for_path(PATH, text.line(0).unwrap_or_default()).expect("a Rust grammar");
    let mut spans = LineSpans::default();
    highlight::highlight(&text, grammar.syntax, &grammar.working_set(), |chunk| {
        spans.append(chunk);
        true
    });
    SideContents { text: Arc::new(text), spans: Arc::new(spans) }
}

pub fn harness(fixture: Fixture) -> Harness<'static, Fixture> {
    Harness::builder()
        .with_size([1180.0, 760.0])
        .with_context_setup(|ctx| {
            theme::fonts::install(ctx);
            theme::motion::set_reduced(ctx, false);
        })
        .build_ui_state(
            |ui, state: &mut Fixture| {
                theme::activate(ui.ctx(), state.theme);
                show_diff(ui, state);
            },
            fixture,
        )
}

/// Shows the fixture's diff the way the files tab does and records what the view asks for.
pub fn show_diff(ui: &mut egui::Ui, state: &mut Fixture) {
    let threads: Vec<&ReviewThread> = state.threads.iter().collect();
    let inputs = DiffInputs {
        diff: &state.diff,
        contents: Some(&state.contents),
        threads: &threads,
        layout: state.layout,
        font_size: state.font_size,
        wrap: state.wrap,
        can_comment: true,
        can_comment_outside_hunks: state.can_comment_outside_hunks,
        path: PATH,
        old_path: None,
        line_spans: None,
        show_threads: state.show_threads,
        find: state.find,
    };
    if let Some(event) = state.view.show(ui, &inputs, &mut state.split, &mut state.threads_ui) {
        state.events.push(match event {
            DiffEvent::Thread(_) => "thread",
            DiffEvent::Create { thread, .. } => {
                state.created.push(thread);
                "create"
            }
            DiffEvent::ColorAnyway => "color anyway",
            DiffEvent::RetryContents => "retry",
            DiffEvent::SwitchLayout(Layout::Inline) => "show inline",
            DiffEvent::SwitchLayout(_) => "switch layout",
            DiffEvent::SplitResized => "split resized",
        });
    }
}

/// Steps long enough for hover delays and eased transitions to finish.
pub fn settle<State>(harness: &mut Harness<'_, State>) {
    harness.run_steps(6);
}

/// 1-based line of `needle` in `text`.
pub fn line_of(text: &str, needle: &str) -> u32 {
    text.lines().position(|line| line.contains(needle)).expect("line in the sample") as u32 + 1
}

pub fn thread_on_old_line(old_line: u32, body: &str) -> ReviewThread {
    ReviewThread {
        id: "removed-line-thread".into(),
        status: ThreadStatus::Active,
        anchor: Some(ThreadAnchor {
            path: PATH.into(),
            old_path: None,
            old_line: Some(old_line),
            new_line: None,
            range: None,
        }),
        is_outdated: false,
        origin: None,
        is_system: false,
        can_reply: true,
        can_resolve: true,
        comments: vec![ThreadComment {
            id: "c1".into(),
            author: UserIdentity {
                id: "u1".into(),
                display_name: "Mara Schulz".into(),
                login: Some("mschulz".into()),
                avatar_url: None,
            },
            body: body.into(),
            created_at: time::OffsetDateTime::UNIX_EPOCH,
            updated_at: None,
        }],
    }
}
