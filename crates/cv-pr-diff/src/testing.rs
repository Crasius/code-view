//! The shared fixture of the diff view's tests: a real Rust file in two versions, colored the way
//! the app colors it, shown through [`DiffView`] in a kittest harness that records what the view
//! asks for. It lives here, behind the `test-support` feature, because the tests of the rendered
//! diff, the folder summary and the Files tab drive the same view from their own crates.

use std::sync::Arc;

use cv_base::theme;
use cv_code::contents::{Coloring, FileContents, SideContents};
use cv_code::diff_rows::Layout;
use cv_pr_threads::{ThreadsUi, samples};
use cv_syntax::blob_text::BlobText;
use cv_syntax::grammar;
use cv_syntax::highlight::{self, LineSpans};
use egui::{Event, Modifiers, PointerButton, Pos2};
use egui_kittest::Harness;
use review_core::{
    DiffSide, ProviderKind, ReviewThread, ThreadAnchor, ThreadComment, ThreadStatus,
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

#[expect(
    clippy::struct_excessive_bools,
    reason = "independent inputs of the diff view a test sets one by one (contents arrived, wrap, comments outside hunks, show comments, scroll past the end), no state machine"
)]
pub struct Fixture {
    pub view: DiffView,
    pub diff: Arc<FileDiff>,
    pub contents: FileContents,
    /// The file's contents arrived. Without them the view shows the diff's own lines.
    pub has_contents: bool,
    pub threads: Vec<ReviewThread>,
    pub layout: Layout,
    pub theme: ThemePreference,
    pub threads_ui: ThreadsUi,
    /// The old side's share of a side-by-side diff.
    pub split: f32,
    /// What the view asked for, in order.
    pub events: Vec<DiffEvent>,
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
    /// "Scroll past the end", which the strip's menu switches.
    pub past_end: bool,
}

impl Fixture {
    /// Shows the diff the way the Files tab does and answers what the view asks for.
    pub fn show(&mut self, ui: &mut egui::Ui) -> Option<DiffEvent> {
        let Self {
            view,
            diff,
            contents,
            has_contents,
            threads,
            layout,
            threads_ui,
            split,
            can_comment_outside_hunks,
            wrap,
            font_size,
            show_threads,
            find,
            past_end,
            ..
        } = self;
        let threads: Vec<&ReviewThread> = threads.iter().collect();
        let inputs = DiffInputs {
            diff,
            contents: has_contents.then_some(&*contents),
            threads: &threads,
            layout: *layout,
            font_size: *font_size,
            wrap: *wrap,
            can_comment: true,
            can_comment_outside_hunks: *can_comment_outside_hunks,
            path: PATH,
            old_path: None,
            line_spans: None,
            show_threads: *show_threads,
            find: *find,
            past_end: *past_end,
        };
        view.show(ui, &inputs, split, threads_ui)
    }
}

pub fn fixture(layout: Layout) -> Fixture {
    fixture_of(layout, Some(OLD), Some(NEW))
}

/// The unified diff of two texts as the app parses it, with 3 lines of context.
pub fn diff_of(old: &str, new: &str) -> Arc<FileDiff> {
    let unified =
        similar::TextDiff::from_lines(old, new).unified_diff().context_radius(3).to_string();
    Arc::new(FileDiff::parse_unified(unified.as_bytes()))
}

/// The whole file as one side, as the Files tab shows an added or deleted file.
pub fn whole_file_fixture(side: DiffSide) -> Fixture {
    let layout = Layout::WholeFile(side);
    match side {
        DiffSide::Old => fixture_of(layout, Some(OLD), None),
        DiffSide::New => fixture_of(layout, None, Some(NEW)),
    }
}

/// [`fixture`] for other texts: `None` is a side the file does not have.
pub fn fixture_of(layout: Layout, old: Option<&str>, new: Option<&str>) -> Fixture {
    Fixture {
        view: DiffView::default(),
        diff: diff_of(old.unwrap_or_default(), new.unwrap_or_default()),
        contents: FileContents {
            old: old.map(side),
            new: new.map(side),
            coloring: Coloring::Done,
            error: None,
        },
        has_contents: true,
        threads: Vec::new(),
        layout,
        theme: ThemePreference::Light,
        threads_ui: ThreadsUi::for_tests(ProviderKind::GitHub),
        split: PaneSizes::DEFAULT.diff_split,
        events: Vec::new(),
        can_comment_outside_hunks: true,
        wrap: false,
        font_size: 13.0,
        show_threads: true,
        find: FindInputs::default(),
        past_end: true,
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
    harness_sized(fixture, [1180.0, 760.0])
}

/// [`harness`] with a window of another `size`. Eight steps, 2 s simulated, cover the hover delay
/// and the eased transitions; [`settle`] fails past them.
pub fn harness_sized(fixture: Fixture, size: [f32; 2]) -> Harness<'static, Fixture> {
    Harness::builder()
        .with_size(size)
        .with_max_steps(8)
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

/// Shows the fixture's diff the way the files tab does and records what the view asks for; a
/// switch of "Scroll past the end" sets it, as the tab saves it.
pub fn show_diff(ui: &mut egui::Ui, state: &mut Fixture) {
    if let Some(event) = state.show(ui) {
        if let DiffEvent::SwitchPastEnd(is_on) = event {
            state.past_end = is_on;
        }
        state.events.push(event);
    }
}

/// Steps until no repaint is asked for; panics with the repaint causes when the view keeps asking
/// past the harness's `max_steps`.
pub fn settle<State>(harness: &mut Harness<'_, State>) {
    harness.run();
}

/// A click at `pos` with `modifiers` held (shift-click, ctrl-click).
pub fn click_with<State>(harness: &mut Harness<'_, State>, pos: Pos2, modifiers: Modifiers) {
    let button =
        |pressed| Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers };
    harness.hover_at(pos);
    harness.event(Event::ModifiersChanged(modifiers));
    harness.event(button(true));
    harness.event(button(false));
    harness.event(Event::ModifiersChanged(Modifiers::NONE));
    harness.step();
}

/// 1-based line of `needle` in `text`.
pub fn line_of(text: &str, needle: &str) -> u32 {
    text.lines().position(|line| line.contains(needle)).expect("line in the sample") as u32 + 1
}

pub fn thread_on_old_line(old_line: u32, body: &str) -> ReviewThread {
    let anchor = ThreadAnchor {
        path: PATH.into(),
        old_path: None,
        old_line: Some(old_line),
        new_line: None,
        range: None,
    };
    samples::thread(
        "removed-line-thread",
        ThreadStatus::Active,
        Some(anchor),
        vec![ThreadComment {
            created_at: time::OffsetDateTime::UNIX_EPOCH,
            ..samples::comment("c1", &samples::mara(), 0, body)
        }],
    )
}
