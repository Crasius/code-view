//! Grammar lookup for file paths, and the rules that turn grammar scopes into [`TokenKind`]s
//! (the scope selectors VS Code's Light+ and Dark+ themes use).
//!
//! The grammar set is `review_grammars`': bat's on the pure-Rust regex engine, with our own C# and
//! PowerShell grammars in place of bat's, one set that Markdown's fences share. It loads on the
//! first highlight, off the UI thread, and stays for the life of the process in its uncompiled
//! form.
//! The compiled form is one set per language, held once per process and shared by every thread
//! and every pull request that highlights (see [`compiled`]). Compiling is the expensive part
//! (TypeScript about 30 MB), so a set stays after its last use, within a process
//! budget of [`BUDGET_BYTES`] (the languages used longest ago go first) and for at most
//! `KEEP_UNUSED` without a use. A set a thread is highlighting with right now never goes.

use std::cmp::Ordering;
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering as AtomicOrdering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use cv_base::theme::syntax::TokenKind;
use cv_runtime::cache::memory::Holder;
use syntect::highlighting::ScopeSelectors;
use syntect::parsing::{Scope, SyntaxReference, SyntaxSet};

#[derive(Clone, Copy)]
pub struct Grammar {
    set: &'static SyntaxSet,
    pub syntax: &'static SyntaxReference,
}

impl Grammar {
    /// A fresh copy of the grammar set, shared: the threads that read a long text in parts
    /// highlight with it too.
    pub fn working_set(self) -> Arc<WorkingSet> {
        Arc::new(WorkingSet {
            source: self.set,
            set: self.set.clone(),
            language: self.syntax.name.clone(),
            warm_up_bytes: AtomicUsize::new(0),
        })
    }

    /// The same language (grammars of different sets never are).
    pub fn is(self, other: Grammar) -> bool {
        std::ptr::eq(self.syntax, other.syntax)
    }
}

/// A copy of a grammar set to highlight one language with. syntect compiles each regex into the
/// set on first use and keeps it, with its match caches, as long as the set lives: about 30 MB for
/// the TypeScript grammar. A copy starts uncompiled and frees all of it when dropped.
pub struct WorkingSet {
    source: &'static SyntaxSet,
    pub set: SyntaxSet,
    language: String,
    /// What the warm-up compiled into the set, as the heap counted it; 0 before.
    warm_up_bytes: AtomicUsize,
}

impl WorkingSet {
    /// About how much memory the set's compiled regexes take, for keeping a budget.
    pub fn estimated_bytes(&self) -> usize {
        self.warm_up_bytes.load(AtomicOrdering::Relaxed).max(UNMEASURED_BYTES)
    }
}

/// The copy of the grammar set itself, measured in release: about 3 MB on top of what the
/// warm-up compiles into it.
const SET_BYTES: usize = 3_000_000;
/// A set the warm-up has not run in yet counts as a mid-sized grammar.
const UNMEASURED_BYTES: usize = 16_000_000;

/// The compiled grammars the process keeps, as estimated: our budget, TypeScript (32 MB measured)
/// beside a few mid-sized languages (Rust 24 MB, C# 19 MB, Markdown 10 MB, JSON 5 MB), so a web or
/// a .NET pull request switches between its files without compiling again.
pub const BUDGET_BYTES: usize = 96 * 1024 * 1024;
/// How long a compiled set stays without a use: our number, a coffee break. Compiling again costs
/// up to a second on the next file of that language.
const KEEP_UNUSED: Duration = Duration::from_mins(5);
/// How often the sweeper looks for sets to drop while any are kept.
const SWEEP_EVERY: Duration = Duration::from_secs(10);

#[derive(Clone, Copy, PartialEq, Eq)]
enum Warm {
    Cold,
    Warming,
    Warmed,
}

struct Compiled {
    set: Arc<WorkingSet>,
    last_used: Instant,
    warm: Warm,
}

impl Compiled {
    fn is_for(&self, grammar: Grammar) -> bool {
        std::ptr::eq(self.set.source, grammar.set) && self.set.language == grammar.syntax.name
    }
}

/// The compiled sets, one per language, so each language's regexes are kept or freed on their
/// own. Each regex compiles once, on whichever thread needs it first (the others wait for it), so
/// the file view, the folder summary and the warm-up all profit from what any of them compiled.
static COMPILED: Mutex<Vec<Compiled>> = Mutex::new(Vec::new());
/// Whether the sweeper thread runs; it does while compiled sets are kept.
static SWEEPING: AtomicBool = AtomicBool::new(false);

fn compiled_sets() -> MutexGuard<'static, Vec<Compiled>> {
    COMPILED.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The shared compiled set for `grammar`, compiled as far as earlier highlighting needed it.
pub fn compiled(grammar: Grammar) -> Arc<WorkingSet> {
    let mut sets = compiled_sets();
    let now = Instant::now();
    if let Some(entry) = sets.iter_mut().find(|entry| entry.is_for(grammar)) {
        entry.last_used = now;
        return entry.set.clone();
    }
    let set = grammar.working_set();
    sets.push(Compiled { set: set.clone(), last_used: now, warm: Warm::Cold });
    keep_within(&mut sets, now, BUDGET_BYTES);
    drop(sets);
    sweep_later();
    set
}

/// Claims the warm-up of `grammar`'s compiled set; false when it ran or runs already.
pub(crate) fn begin_warm_up(grammar: Grammar) -> bool {
    let mut sets = compiled_sets();
    let Some(entry) = sets.iter_mut().find(|entry| entry.is_for(grammar)) else {
        return true;
    };
    let is_cold = entry.warm == Warm::Cold;
    if is_cold {
        entry.warm = Warm::Warming;
    }
    is_cold
}

/// The warm-up of `working`, the compiled set of `grammar`, left `compiled_bytes` on the heap.
/// Counted, not timed: a busy machine compiles slower, not more.
pub(crate) fn finish_warm_up(grammar: Grammar, working: &WorkingSet, compiled_bytes: usize) {
    working.warm_up_bytes.store(SET_BYTES + compiled_bytes, AtomicOrdering::Relaxed);
    let mut sets = compiled_sets();
    if let Some(entry) = sets.iter_mut().find(|entry| entry.is_for(grammar)) {
        entry.warm = Warm::Warmed;
    }
    keep_within(&mut sets, Instant::now(), BUDGET_BYTES);
}

/// The warm-up of `grammar`'s compiled set finished.
#[cfg(any(test, feature = "test-support"))]
pub fn is_warmed(grammar: Grammar) -> bool {
    compiled_sets().iter().any(|entry| entry.is_for(grammar) && entry.warm == Warm::Warmed)
}

/// A warm-up compiles `grammar` right now, so its first colors are on their way.
pub fn is_warming(grammar: Grammar) -> bool {
    compiled_sets().iter().any(|entry| entry.is_for(grammar) && entry.warm == Warm::Warming)
}

/// Forgets the compiled set of `grammar`, so the next use starts cold (holders keep theirs).
#[cfg(any(test, feature = "test-support"))]
pub fn forget(grammar: Grammar) {
    let mut sets = compiled_sets();
    sets.retain(|entry| !entry.is_for(grammar));
    publish(&sets);
}

/// Drops every compiled set no thread highlights with right now, as the sweeper does once they
/// were unused for [`KEEP_UNUSED`]: for tests that check what is left once nothing is shown.
#[cfg(any(test, feature = "test-support"))]
pub fn release_unused() {
    let mut sets = compiled_sets();
    sets.retain(|entry| Arc::strong_count(&entry.set) > 1);
    publish(&sets);
}

/// The languages whose compiled sets the process keeps.
#[cfg(any(test, feature = "test-support"))]
pub fn kept_languages() -> Vec<String> {
    compiled_sets().iter().map(|entry| entry.set.language.clone()).collect()
}

/// Drops the sets unused for [`KEEP_UNUSED`], then the least recently used ones until the
/// estimated bytes fit `budget`. A set a thread holds (it highlights right now) counts as used
/// now and never goes; neither does the one used last.
fn keep_within(sets: &mut Vec<Compiled>, now: Instant, budget: usize) {
    for entry in sets.iter_mut().filter(|entry| Arc::strong_count(&entry.set) > 1) {
        entry.last_used = now;
    }
    sets.retain(|entry| now.duration_since(entry.last_used) < KEEP_UNUSED);
    let mut bytes: usize = sets.iter().map(|entry| entry.set.estimated_bytes()).sum();
    let newest = sets.iter().map(|entry| entry.last_used).max();
    while bytes > budget {
        let oldest = sets
            .iter()
            .enumerate()
            .filter(|(_, entry)| {
                Arc::strong_count(&entry.set) == 1 && Some(entry.last_used) != newest
            })
            .min_by_key(|(_, entry)| entry.last_used)
            .map(|(index, _)| index);
        let Some(oldest) = oldest else {
            break;
        };
        bytes -= sets.swap_remove(oldest).set.estimated_bytes();
    }
    publish(sets);
}

/// Shows the kept sets' estimated bytes in the memory readout.
fn publish(sets: &[Compiled]) {
    Holder::Grammars.set(sets.iter().map(|entry| entry.set.estimated_bytes()).sum());
}

/// Applies [`keep_within`] every [`SWEEP_EVERY`] while sets are kept. The sweeper ends once no set
/// is left, so an idle app does no work.
fn sweep_later() {
    if SWEEPING.swap(true, AtomicOrdering::SeqCst) {
        return;
    }
    let sweeper = std::thread::Builder::new().name("grammar-sweep".into()).spawn(|| {
        loop {
            #[expect(
                clippy::disallowed_methods,
                reason = "the sweeper owns this thread and only waits between sweeps; no UI or task waits on it"
            )]
            std::thread::sleep(SWEEP_EVERY);
            let mut sets = compiled_sets();
            keep_within(&mut sets, Instant::now(), BUDGET_BYTES);
            if sets.is_empty() {
                SWEEPING.store(false, AtomicOrdering::SeqCst);
                return;
            }
        }
    });
    if sweeper.is_err() {
        SWEEPING.store(false, AtomicOrdering::SeqCst);
    }
}

struct Grammars {
    set: SyntaxSet,
    rules: Vec<(ScopeSelectors, TokenKind)>,
}

static GRAMMARS: OnceLock<Grammars> = OnceLock::new();

fn grammars() -> &'static Grammars {
    GRAMMARS.get_or_init(|| Grammars { set: review_grammars::syntax_set(), rules: parse_rules() })
}

/// The grammar for `path`, falling back to the first line (shebangs, `<?xml`). Loads the grammar
/// set on first use, which takes a moment, so only call this from background work.
pub fn for_path(path: &str, first_line: &str) -> Option<Grammar> {
    let grammars = grammars();
    let file_name = path.rsplit('/').next().unwrap_or(path).to_ascii_lowercase();
    let extension = file_name.rsplit_once('.').map_or("", |(_, extension)| extension);
    if let Some(name) = name_override(&file_name, extension) {
        return find_by_name(grammars, name);
    }
    let set = &grammars.set;
    let syntax = set
        .find_syntax_by_extension(&file_name)
        .or_else(|| set.find_syntax_by_extension(extension))
        .or_else(|| set.find_syntax_by_first_line(first_line))?;
    let is_plain = syntax.name == "Plain Text";
    (!is_plain).then_some(Grammar { set, syntax })
}

/// The grammar for the language of a fenced code block's info string (`rust`, `ts`, `c#`,
/// `shell`, `rust,ignore`); `None` for plain text and languages without a grammar. Loads the
/// grammar set on first use, so only call this from background work.
pub fn for_language(info: &str) -> Option<Grammar> {
    let info = info.trim().to_ascii_lowercase();
    // Info strings may carry attributes after the language: `rust,ignore`, `js {.line-numbers}`.
    let token = info
        .split(|ch: char| ch.is_whitespace() || matches!(ch, ',' | '{'))
        .next()
        .unwrap_or_default();
    let token = token.strip_prefix('.').unwrap_or(token);
    if matches!(
        token,
        "" | "text" | "txt" | "plain" | "plaintext" | "none" | "output" | "nohighlight"
    ) {
        return None;
    }
    let grammars = grammars();
    let alias = match token {
        "c#" | "csharp" | "cs" => Some("C#"),
        "pwsh" | "powershell" | "ps" => Some("PowerShell"),
        "sh" | "shell" | "bash" | "zsh" | "console" | "shellscript" | "shell-session" => {
            Some("Bourne Again Shell (bash)")
        }
        "golang" => Some("Go"),
        "c++" | "cpp" => Some("C++"),
        "docker" => Some("Dockerfile"),
        "jsx" | "node" | "javascript" => Some("JavaScript"),
        "typescript" => Some("TypeScript"),
        "md" | "markdown" | "gfm" => Some("Markdown"),
        "yml" => Some("YAML"),
        _ => None,
    };
    if let Some(name) = alias.or_else(|| name_override(token, token)) {
        return find_by_name(grammars, name);
    }
    let set = &grammars.set;
    let syntax = set.find_syntax_by_token(token)?;
    (syntax.name != "Plain Text").then_some(Grammar { set, syntax })
}

fn find_by_name(grammars: &'static Grammars, name: &str) -> Option<Grammar> {
    let set = &grammars.set;
    let syntax = set.find_syntax_by_name(name)?;
    Some(Grammar { set, syntax })
}

/// File types the grammars do not claim themselves, mostly .NET and build files.
fn name_override(file_name: &str, extension: &str) -> Option<&'static str> {
    let is_dockerfile = file_name == "dockerfile"
        || file_name == "containerfile"
        || file_name.starts_with("dockerfile.")
        || extension == "dockerfile";
    if is_dockerfile {
        return Some("Dockerfile");
    }
    let by_name = match file_name {
        "makefile" | "gnumakefile" => Some("Makefile"),
        "cmakelists.txt" => Some("CMake"),
        "cargo.lock" | "poetry.lock" => Some("TOML"),
        "jenkinsfile" => Some("Groovy"),
        "gemfile" | "rakefile" | "podfile" | "vagrantfile" => Some("Ruby"),
        ".editorconfig" | ".gitconfig" | ".npmrc" => Some("INI"),
        _ => None,
    };
    by_name.or(match extension {
        "csproj" | "vbproj" | "fsproj" | "vcxproj" | "proj" | "props" | "targets" | "nuspec"
        | "resx" | "xaml" | "axaml" | "config" | "manifest" | "slnx" | "wxs" | "wxi"
        | "runsettings" | "ruleset" | "natvis" | "xsd" | "xslt" | "svg" | "plist"
        | "storyboard" | "xlf" | "xliff" => Some("XML"),
        "ts" | "mts" | "cts" => Some("TypeScript"),
        "tsx" => Some("TypeScriptReact"),
        "js" | "mjs" | "cjs" => Some("JavaScript"),
        "jsonc" | "json5" | "code-workspace" => Some("JSON"),
        "razor" | "cshtml" => Some("HTML"),
        "cake" => Some("C#"),
        "gradle" => Some("Groovy"),
        "kts" => Some("Kotlin"),
        "h" | "hpp" | "hxx" | "inl" => Some("C++"),
        _ => None,
    })
}

/// Scope selectors per token kind. A deeper (more specific) selector wins over a shorter one,
/// so `keyword.control` beats `keyword` and `support.type.property-name` beats `support.type`.
const RULES: &[(TokenKind, &str)] = &[
    (TokenKind::Comment, "comment, punctuation.definition.comment"),
    (
        TokenKind::String,
        "string, markup.inline.raw, markup.raw, markup.underline.link, meta.preprocessor.string, support.constant.property-value, \
         support.constant.font-name, support.constant.media-type, support.constant.media, constant.other.color.rgb-value, \
         constant.other.rgb-value, support.constant.color, markup.deleted",
    ),
    (TokenKind::Regex, "string.regexp, constant.regexp"),
    (TokenKind::Escape, "constant.character.escape"),
    (
        TokenKind::Number,
        "constant.numeric, keyword.other.unit, meta.preprocessor.numeric, keyword.operator.plus.exponent, \
         keyword.operator.minus.exponent, markup.inserted",
    ),
    (
        TokenKind::Keyword,
        "keyword, storage, storage.type, storage.modifier, constant.language, variable.language, meta.preprocessor, \
         keyword.operator.new, keyword.operator.expression, keyword.operator.cast, keyword.operator.sizeof, \
         keyword.operator.alignof, keyword.operator.typeid, keyword.operator.instanceof, keyword.operator.logical.python, \
         keyword.operator.wordlike, keyword.operator.word, constant.character, constant.other.option, \
         punctuation.definition.template-expression, punctuation.section.embedded, markup.changed, meta.diff.header",
    ),
    (
        TokenKind::Control,
        "keyword.control, keyword.other.using, keyword.other.directive.using, keyword.other.operator, keyword.operator.delete, \
         keyword.import, keyword.declaration.import, keyword.other.import",
    ),
    (TokenKind::Operator, "keyword.operator"),
    (
        TokenKind::Type,
        "entity.name.type, entity.name.class, entity.name.struct, entity.name.enum, entity.name.union, entity.name.interface, \
         entity.name.trait, entity.name.impl, entity.name.namespace, entity.name.scope-resolution, entity.other.inherited-class, \
         support.class, support.type, meta.type.cast.expr, meta.type.new.expr, support.constant.math, support.constant.dom, \
         support.constant.json, storage.type.primitive, storage.type.numeric.go, storage.type.byte.go, storage.type.boolean.go, \
         storage.type.string.go, storage.type.error.go, storage.type.rune.go, storage.type.uintptr.go",
    ),
    (
        TokenKind::Function,
        "entity.name.function, support.function, variable.function, support.constant.handlebars, entity.name.operator.custom-literal",
    ),
    (
        TokenKind::Variable,
        "variable, meta.definition.variable.name, support.variable, entity.name.variable, constant.other.placeholder, variable.parameter",
    ),
    (
        TokenKind::Constant,
        "variable.other.constant, variable.other.enummember, constant.other, entity.name.constant",
    ),
    (
        TokenKind::Property,
        "meta.object-literal.key, support.type.property-name.json, meta.mapping.key string, meta.structure.dictionary.key.python",
    ),
    (TokenKind::Tag, "entity.name.tag"),
    (
        TokenKind::Attribute,
        "entity.other.attribute-name, support.type.property-name, support.type.vendored.property-name",
    ),
    (
        TokenKind::Selector,
        "entity.name.tag.css, entity.name.tag.less, entity.name.tag.scss, entity.other.attribute-name.class, \
         entity.other.attribute-name.id, entity.other.attribute-name.pseudo-class, entity.other.attribute-name.pseudo-element, \
         entity.other.pseudo-class, entity.other.pseudo-element",
    ),
    (TokenKind::Punctuation, "punctuation.definition.tag"),
    (TokenKind::Heading, "markup.heading, entity.name.section"),
    // No rule for `invalid.illegal`: a grammar's guess is no compiler error, and red reads as
    // "deleted" in a diff, so such text keeps the kind of what surrounds it.
    // Markdown. Fenced and indented code is code, not a string: its embedded language colors it,
    // and text the language leaves alone stays plain, as in VS Code.
    (TokenKind::Plain, "markup.raw.code-fence, markup.raw.block"),
    (TokenKind::Strong, "markup.bold"),
    (TokenKind::Emphasis, "markup.italic"),
    (
        TokenKind::Markup,
        "markup.list.numbered.bullet, markup.list.unnumbered.bullet, punctuation.definition.list_item, \
         punctuation.definition.list.begin, punctuation.definition.blockquote, \
         punctuation.definition.quote, punctuation.definition.thematic-break, meta.separator.thematic-break, \
         punctuation.definition.bold, punctuation.definition.italic, punctuation.definition.strikethrough, \
         punctuation.definition.raw.code-fence, punctuation.definition.link, punctuation.definition.image, \
         punctuation.definition.metadata, punctuation.separator.table-cell, punctuation.section.table-header, \
         punctuation.definition.table-cell",
    ),
    (
        TokenKind::Constant,
        "meta.link.inline.description, meta.link.reference.description, meta.link.reference.literal.description, \
         string.other.link.title, string.other.link.description, constant.other.reference.link",
    ),
];

fn parse_rules() -> Vec<(ScopeSelectors, TokenKind)> {
    RULES
        .iter()
        .map(|(kind, selectors)| {
            (ScopeSelectors::from_str(selectors).expect("the scope rules parse"), *kind)
        })
        .collect()
}

/// Turns a scope stack into a [`TokenKind`] the way a VS Code theme picks a color: the rule whose
/// selector matches the stack most specifically wins, the later rule on a tie. A file repeats a
/// few thousand distinct stacks over and over, so each is matched against the rules only once.
pub(crate) struct Classifier {
    rules: &'static [(ScopeSelectors, TokenKind)],
    cache: HashMap<Vec<Scope>, TokenKind>,
}

impl Default for Classifier {
    fn default() -> Self {
        Self { rules: &grammars().rules, cache: HashMap::new() }
    }
}

impl Classifier {
    pub(crate) fn kind(&mut self, stack: &[Scope]) -> TokenKind {
        if let Some(kind) = self.cache.get(stack) {
            return *kind;
        }
        let kind = self
            .rules
            .iter()
            .filter_map(|(selectors, kind)| selectors.does_match(stack).map(|power| (power, *kind)))
            .max_by(|(left, _), (right, _)| left.partial_cmp(right).unwrap_or(Ordering::Equal))
            .map_or(TokenKind::Plain, |(_, kind)| kind);
        self.cache.insert(stack.to_vec(), kind);
        kind
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A compiled set of `path`'s language measured at `megabytes`, last used `ago` before `now`.
    fn kept(path: &str, megabytes: usize, now: Instant, ago: Duration) -> Compiled {
        let set = for_path(path, "").expect("a grammar").working_set();
        set.warm_up_bytes.store(megabytes * 1024 * 1024, AtomicOrdering::Relaxed);
        Compiled {
            set,
            last_used: now.checked_sub(ago).expect("the clock runs longer"),
            warm: Warm::Warmed,
        }
    }

    fn languages(sets: &[Compiled]) -> Vec<&str> {
        let mut languages: Vec<&str> =
            sets.iter().map(|entry| entry.set.language.as_str()).collect();
        languages.sort_unstable();
        languages
    }

    /// The process keeps one set per language within its budget: the languages used longest ago
    /// go first, a set some thread highlights with stays whatever its age, and none outlives
    /// [`KEEP_UNUSED`] without a use.
    #[test]
    fn keeps_compiled_grammars_within_the_process_budget() {
        let now = Instant::now() + KEEP_UNUSED * 2;
        let minute = Duration::from_secs(60);
        let mut sets = vec![
            kept("a.ts", 114, now, minute * 3),
            kept("a.cs", 28, now, minute * 2),
            kept("a.rs", 30, now, minute),
            kept("a.json", 20, now, Duration::ZERO),
        ];
        keep_within(&mut sets, now, 100 * 1024 * 1024);
        assert_eq!(languages(&sets), ["C#", "JSON", "Rust"], "TypeScript, used longest ago, went");

        let in_use = sets[0].set.clone();
        sets.push(kept("a.go", 60, now, Duration::ZERO));
        keep_within(&mut sets, now, 100 * 1024 * 1024);
        assert!(languages(&sets).contains(&in_use.language.as_str()), "a set in use stays");
        assert!(languages(&sets).contains(&"Go"), "the one used last stays");

        drop(in_use);
        keep_within(&mut sets, now + KEEP_UNUSED, usize::MAX);
        assert!(sets.is_empty(), "unused for too long: {:?}", languages(&sets));
    }

    /// Ours take the place of bat's grammars of the same language: one grammar per language.
    #[test]
    fn holds_one_grammar_per_replaced_language() {
        let set = &grammars().set;
        for scope in ["source.cs", "source.powershell"] {
            let scope = Scope::new(scope).expect("a scope");
            let count = set.syntaxes().iter().filter(|syntax| syntax.scope == scope).count();
            assert_eq!(count, 1, "{scope:?}");
        }
    }

    /// `review-grammars` relinks bat's references into the grammars ours replace; none may dangle.
    #[test]
    fn every_grammar_reference_links() {
        assert!(grammars().set.find_unlinked_contexts().is_empty());
    }

    /// A grammar's `invalid.illegal` is its own guess, not a compiler error: the text keeps the
    /// kind of what surrounds it instead of turning red.
    #[test]
    fn illegal_scopes_keep_the_surrounding_kind() {
        let stack = |scopes: &str| -> Vec<Scope> {
            scopes.split(' ').map(|scope| Scope::new(scope).expect("a scope")).collect()
        };
        let mut classifier = Classifier::default();
        let stray_brace = stack("source.cs invalid.illegal.stray.brace.cs");
        assert_eq!(classifier.kind(&stray_brace), TokenKind::Plain);
        let unclosed =
            stack("source.cs string.quoted.double.cs invalid.illegal.unclosed-string.cs");
        assert_eq!(classifier.kind(&unclosed), TokenKind::String);
    }

    #[test]
    fn picks_grammars_for_common_files() {
        let cases = [
            ("src/main.rs", "Rust"),
            ("App/Program.cs", "C#"),
            ("App/App.csproj", "XML"),
            ("Directory.Build.props", "XML"),
            ("web/app.tsx", "TypeScriptReact"),
            ("web/index.ts", "TypeScript"),
            ("tool.py", "Python"),
            ("cmd/main.go", "Go"),
            ("Main.java", "Java"),
            ("build.gradle.kts", "Kotlin"),
            ("native/lib.cpp", "C++"),
            ("package.json", "JSON"),
            (".github/workflows/ci.yml", "YAML"),
            ("Cargo.toml", "TOML"),
            ("README.md", "Markdown"),
            ("site/style.scss", "SCSS"),
            ("db/schema.sql", "SQL"),
            ("scripts/build.ps1", "PowerShell"),
            ("docker/Dockerfile", "Dockerfile"),
            ("index.html", "HTML"),
            ("Makefile", "Makefile"),
            ("CMakeLists.txt", "CMake"),
            ("Jenkinsfile", "Groovy"),
            ("Gemfile", "Ruby"),
            (".editorconfig", "INI"),
            ("Cargo.lock", "TOML"),
            ("Views/Index.cshtml", "HTML"),
            ("settings.jsonc", "JSON"),
        ];
        for (path, expected) in cases {
            let grammar = for_path(path, "").unwrap_or_else(|| panic!("no grammar for {path}"));
            assert_eq!(grammar.syntax.name, expected, "{path}");
        }
        assert_eq!(
            for_path("scripts/run", "#!/bin/bash").map(|grammar| grammar.syntax.name.as_str()),
            Some("Bourne Again Shell (bash)")
        );
        assert!(for_path("notes.unknownext", "").is_none());
    }

    #[test]
    fn picks_grammars_for_fenced_code_languages() {
        let cases = [
            ("rust", "Rust"),
            ("rs", "Rust"),
            ("Rust", "Rust"),
            ("rust,ignore", "Rust"),
            ("ts", "TypeScript"),
            ("typescript", "TypeScript"),
            ("tsx", "TypeScriptReact"),
            ("js", "JavaScript"),
            ("jsx", "JavaScript"),
            ("c#", "C#"),
            ("csharp", "C#"),
            ("cs", "C#"),
            ("py", "Python"),
            ("python", "Python"),
            ("sh", "Bourne Again Shell (bash)"),
            ("console", "Bourne Again Shell (bash)"),
            ("powershell", "PowerShell"),
            ("ps1", "PowerShell"),
            ("json", "JSON"),
            ("yaml", "YAML"),
            ("yml", "YAML"),
            ("toml", "TOML"),
            ("go", "Go"),
            ("cpp", "C++"),
            ("xml", "XML"),
            ("html", "HTML"),
            ("sql", "SQL"),
            ("diff", "Diff"),
            ("dockerfile", "Dockerfile"),
        ];
        for (info, expected) in cases {
            let grammar = for_language(info).unwrap_or_else(|| panic!("no grammar for {info:?}"));
            assert_eq!(grammar.syntax.name, expected, "{info}");
        }
        for info in ["", "  ", "text", "plaintext", "no-such-language"] {
            assert!(for_language(info).is_none(), "{info:?} stays plain");
        }
    }
}
