//! Where the time goes between opening a file and its first colored row: loading the grammar
//! set, compiling a grammar's regexes on first use, reading the blobs and parsing up to the
//! visible lines, the whole file from line 0 (the file's own colors) against the diff's hunks
//! (the colors by diff line). `#[ignore]`d, numbers only make sense in release:
//! `cargo test --release -p cv-pr-diff coloring_latency -- --ignored --nocapture`.

use std::fmt::Write as _;
use std::sync::Arc;
use std::time::{Duration, Instant};

use review_git::FileDiff;

use crate::patch_colors;
use crate::word_highlights::WordHighlights;
use cv_syntax::blob_text::BlobText;
use cv_syntax::grammar;
use cv_syntax::highlight::{self, LineSpans};

fn ms(start: Instant) -> f64 {
    start.elapsed().as_secs_f64() * 1000.0
}

fn typescript_source() -> String {
    let block = r#"import { Injectable, type Provider } from "@angular/core";
import * as path from "node:path";

/** Keeps the open documents and their versions. */
export interface DocumentStore<T extends object = Record<string, unknown>> {
    readonly documents: Map<string, T>;
    open(uri: string, contents: T): Promise<void>;
}

@Injectable({ providedIn: "root" })
export class Workspace<T extends object> implements DocumentStore<T> {
    private readonly pattern = /^(?:file|untitled):\/\/(.*)$/u;
    public documents = new Map<string, T>();

    constructor(private readonly root: string, private provider?: Provider) {}

    async open(uri: string, contents: T): Promise<void> {
        const match = this.pattern.exec(uri);
        if (!match) {
            throw new Error(`unsupported uri ${uri} in ${this.root}`);
        }
        for (const [key, value] of Object.entries(contents)) {
            console.debug(key, typeof value === "string" ? value.length : 0, 0x1f, 3.5e2);
        }
        this.documents.set(path.join(this.root, match[1] ?? ""), { ...contents });
    }
}

export const enumerate = <T,>(items: readonly T[]): Array<[number, T]> => items.map((item, index) => [index, item]);
"#;
    block.repeat(40)
}

fn csharp_source() -> String {
    let block = r#"using System;
using System.Collections.Generic;
using System.Threading.Tasks;

namespace Review.Core.Documents;

/// <summary>Keeps the open documents and their versions.</summary>
public sealed class Workspace<T> : IDocumentStore<T> where T : class, new()
{
    private readonly Dictionary<string, T> _documents = new(StringComparer.Ordinal);
    private static readonly Regex Pattern = new(@"^(?:file|untitled)://(.*)$", RegexOptions.Compiled);

    public int Count => _documents.Count;

    public async Task OpenAsync(string uri, T contents, CancellationToken cancellationToken = default)
    {
        var match = Pattern.Match(uri);
        if (!match.Success)
        {
            throw new ArgumentException($"unsupported uri {uri}", nameof(uri));
        }
        foreach (var (key, value) in Describe(contents))
        {
            Console.WriteLine("{0} = {1}", key, value is string text ? text.Length : 0x1F);
        }
        await Task.Delay(TimeSpan.FromMilliseconds(3.5), cancellationToken).ConfigureAwait(false);
        _documents[match.Groups[1].Value] = contents;
    }
}
"#;
    block.repeat(50)
}

struct Timing {
    first_chunk: f64,
    full: f64,
}

/// Highlights `text` with `working`, timing the first delivered chunk and the whole file.
fn time_highlight(
    text: &BlobText,
    grammar: grammar::Grammar,
    working: &Arc<grammar::WorkingSet>,
) -> Timing {
    let start = Instant::now();
    let mut first_chunk = None;
    let mut spans = LineSpans::default();
    highlight::highlight(text, grammar.syntax, working, |chunk| {
        first_chunk.get_or_insert_with(|| ms(start));
        spans.append(chunk);
        true
    });
    Timing { first_chunk: first_chunk.unwrap_or_default(), full: ms(start) }
}

/// One hunk over `lines`, which start at line `first` of the file: mostly context, every fifth
/// line changed (deleted and added again with a word changed).
fn hunk_over(lines: &[&str], first: usize) -> FileDiff {
    let mut body = String::new();
    for (offset, line) in lines.iter().enumerate() {
        if offset % 5 == 2 {
            writeln!(body, "-{line}\n+{line} // changed").unwrap();
        } else {
            writeln!(body, " {line}").unwrap();
        }
    }
    let count = lines.len();
    FileDiff::parse_unified(format!("@@ -{first},{count} +{first},{count} @@\n{body}").as_bytes())
}

/// A one-hunk diff over 60 lines from the middle of `source`, as [`hunk_over`] builds it.
fn middle_hunk(source: &str) -> FileDiff {
    let lines: Vec<&str> = source.lines().collect();
    let start = lines.len() / 2;
    hunk_over(&lines[start..start + 60], start + 1)
}

fn time_patch(
    grammar: grammar::Grammar,
    working: &Arc<grammar::WorkingSet>,
    diff: &FileDiff,
) -> f64 {
    let start = Instant::now();
    patch_colors::highlight_patch(grammar, working, diff, |_| true);
    ms(start)
}

/// How long until the lines around `line` of both sides are colored, both sides at once.
fn time_both_sides(text: &BlobText, grammar: grammar::Grammar, line: usize) -> f64 {
    let working = grammar.working_set();
    highlight::highlight(text, grammar.syntax, &working, |_| true);
    let side = |working: &Arc<grammar::WorkingSet>| {
        let start = Instant::now();
        let mut done = 0;
        let mut reached = None;
        highlight::highlight(text, grammar.syntax, working, |chunk| {
            done += chunk.line_count();
            if done > line && reached.is_none() {
                reached = Some(ms(start));
            }
            true
        });
        reached.unwrap_or_default()
    };
    let (new, old) = std::thread::scope(|scope| {
        let new = scope.spawn(|| side(&working));
        let old = scope.spawn(|| side(&working));
        (new.join().unwrap(), old.join().unwrap())
    });
    new.max(old)
}

/// From opening a file to its first colored diff lines, with the warm-up of its grammar started
/// `warmed_before` earlier (as the pull request opened) and running next to the coloring.
fn time_open(grammar: grammar::Grammar, diff: &FileDiff, warmed_before: Duration) -> f64 {
    let working = grammar.working_set();
    std::thread::scope(|scope| {
        scope.spawn(|| highlight::warm_up_set(grammar.syntax, &working));
        #[expect(
            clippy::disallowed_methods,
            reason = "the probe's own thread stands in for the time between the warm-up and the click"
        )]
        std::thread::sleep(warmed_before);
        let start = Instant::now();
        let mut first = None;
        patch_colors::highlight_patch(grammar, &working, diff, |_| {
            first.get_or_insert_with(|| ms(start));
            true
        });
        first.unwrap_or_default()
    })
}

fn git_cat_file(path: &str) -> std::process::Command {
    let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../..");
    let mut command = std::process::Command::new("git");
    command.args(["-C", root, "cat-file", "blob", &format!("HEAD:{path}")]);
    command.stdout(std::process::Stdio::null());
    // As the app starts git: without a console window, which costs about half a second.
    cv_base::platform::hide_std_console(&mut command);
    command
}

fn time_blobs(concurrent: bool) -> f64 {
    let paths = ["crates/cv-pr-diff/src/diff_view.rs", "Cargo.lock"];
    let start = Instant::now();
    if concurrent {
        let children: Vec<_> =
            paths.iter().filter_map(|path| git_cat_file(path).spawn().ok()).collect();
        for mut child in children {
            child.wait().expect("git cat-file ran");
        }
    } else {
        for path in paths {
            #[expect(
                clippy::disallowed_methods,
                reason = "measures git's start-up on the probe's own thread; the app runs git on Background"
            )]
            git_cat_file(path).status().expect("git cat-file ran");
        }
    }
    ms(start)
}

/// `block` repeated to `lines` lines.
fn lines_of(block: &str, lines: usize) -> String {
    block.lines().cycle().take(lines).flat_map(|line| [line, "\n"]).collect()
}

/// `source` inside one namespace block, indented as usual: a file that is one long block (a
/// namespace, a test suite), where colors cannot restart at a top-level line.
fn in_namespace(source: &str) -> String {
    let inner: String = source.lines().flat_map(|line| ["    ", line, "\n"]).collect();
    format!("namespace Outer {{\n{inner}}}\n")
}

/// One hunk over all of `source`, as [`hunk_over`] builds it: a large file reworked, the case
/// without hunk starts to split the coloring at.
fn whole_file_hunk(source: &str) -> FileDiff {
    hunk_over(&source.lines().collect::<Vec<_>>(), 1)
}

/// Times one whole coloring: until the first chunk, all of it, and the share spent handing the
/// chunks over (what the receiving side does with each: append it).
struct Run {
    first: f64,
    all: f64,
    hand_off: f64,
}

fn run_patch(
    grammar: grammar::Grammar,
    working: &Arc<grammar::WorkingSet>,
    diff: &FileDiff,
) -> Run {
    let start = Instant::now();
    let (mut first, mut hand_off, mut spans) = (None, 0.0, LineSpans::default());
    patch_colors::highlight_patch(grammar, working, diff, |chunk| {
        first.get_or_insert_with(|| ms(start));
        let handing = Instant::now();
        spans.append(chunk);
        hand_off += ms(handing);
        true
    });
    assert_eq!(spans.line_count(), diff.lines().len(), "every diff line colored");
    Run { first: first.unwrap_or_default(), all: ms(start), hand_off }
}

fn run_file(text: &BlobText, grammar: grammar::Grammar, working: &Arc<grammar::WorkingSet>) -> Run {
    let start = Instant::now();
    let (mut first, mut hand_off, mut spans) = (None, 0.0, LineSpans::default());
    highlight::highlight(text, grammar.syntax, working, |chunk| {
        first.get_or_insert_with(|| ms(start));
        let handing = Instant::now();
        spans.append(chunk);
        hand_off += ms(handing);
        true
    });
    assert_eq!(spans.line_count(), text.line_count(), "every file line colored");
    Run { first: first.unwrap_or_default(), all: ms(start), hand_off }
}

/// Diff lines of the large-diff case: a large reworked file.
const LARGE_DIFF_LINES: usize = 15_000;

/// Prints where the time of coloring a large diff goes, per language: compiling the grammar (cold
/// minus compiled), the colors by diff line, the whole file of one side, handing chunks over and
/// the changed words of every replaced line. Our budget: 1.5 s for the TypeScript cases by diff
/// line with a compiled grammar, release build.
/// `cargo test --release -p cv-pr-diff large_diff_coloring -- --ignored --nocapture`.
#[test]
#[ignore = "release-mode numbers for a large diff's colors; prints them"]
fn large_diff_coloring() {
    let start = Instant::now();
    grammar::for_path("src/main.rs", "").expect("a Rust grammar");
    println!("grammar set load (first lookup): {:.1} ms", ms(start));
    // Every fifth line of the source becomes two diff lines.
    let file_lines = LARGE_DIFF_LINES * 5 / 6;
    let samples = [
        ("workspace.ts", lines_of(&typescript_source(), file_lines)),
        ("diff_view.rs", lines_of(include_str!("diff_view.rs"), file_lines)),
        ("Workspace.cs", lines_of(&csharp_source(), file_lines)),
        ("namespace.ts", in_namespace(&lines_of(&typescript_source(), file_lines - 2))),
    ];
    for (path, source) in &samples {
        let grammar = grammar::for_path(path, "").expect("a grammar");
        let diff = whole_file_hunk(source);
        let text = BlobText::from_bytes(source.clone().into_bytes());
        let cold = run_patch(grammar, &grammar.working_set(), &diff);
        let compiled_set = grammar.working_set();
        highlight::warm_up_set(grammar.syntax, &compiled_set);
        run_patch(grammar, &compiled_set, &diff);
        let compiled = run_patch(grammar, &compiled_set, &diff);
        let file_cold = run_file(&text, grammar, &grammar.working_set());
        let file = run_file(&text, grammar, &compiled_set);
        let words_start = Instant::now();
        let mut words = WordHighlights::new(&diff);
        for (index, line) in diff.lines().iter().enumerate() {
            words.compute(&diff, u32::try_from(index).expect("fits"), line.kind);
        }
        let words_ms = ms(words_start);
        println!(
            "{path}: {} diff lines, {} file lines\n  by diff line: cold {:.0} ms (first chunk {:.1} ms), compiled {:.0} ms (first chunk {:.1} ms, hand-off {:.1} ms)\n  whole file, one side: cold {:.0} ms, compiled {:.0} ms (first chunk {:.1} ms, hand-off {:.1} ms)\n  changed words of every pair: {words_ms:.1} ms",
            diff.lines().len(),
            text.line_count(),
            cold.all,
            cold.first,
            compiled.all,
            compiled.first,
            compiled.hand_off,
            file_cold.all,
            file.all,
            file.first,
            file.hand_off,
        );
    }
}

#[test]
#[ignore = "release-mode latency breakdown; prints numbers"]
fn coloring_latency() {
    let start = Instant::now();
    grammar::for_path("src/main.rs", "").expect("a Rust grammar");
    println!("grammar set load (first lookup): {:.1} ms", ms(start));
    let best = |runs: &mut dyn FnMut() -> f64| (0..5).map(|_| runs()).fold(f64::MAX, f64::min);
    println!(
        "two blobs from git: one after the other {:.1} ms, both at once {:.1} ms",
        best(&mut || time_blobs(false)),
        best(&mut || time_blobs(true))
    );
    let samples = [
        ("diff_view.rs", include_str!("diff_view.rs").to_owned()),
        ("workspace.ts", typescript_source()),
        ("Workspace.cs", csharp_source()),
    ];
    for (path, source) in &samples {
        let text = BlobText::from_bytes(source.clone().into_bytes());
        let grammar = grammar::for_path(path, "").expect("a grammar");
        let cold_set = grammar.working_set();
        let cold = time_highlight(&text, grammar, &cold_set);
        let compiled = time_highlight(&text, grammar, &cold_set);
        let hunk = middle_hunk(source);
        let patch_cold = time_patch(grammar, &grammar.working_set(), &hunk);
        let warmed_set = grammar.working_set();
        let warming = Instant::now();
        highlight::warm_up_set(grammar.syntax, &warmed_set);
        let warm_up = ms(warming);
        let patch_warmed = time_patch(grammar, &warmed_set, &hunk);
        let patch_compiled = time_patch(grammar, &cold_set, &hunk);
        let middle = text.line_count() / 2;
        println!(
            "{path} ({} lines)\n  whole file from line 0, cold: first chunk {:.1} ms, all {:.1} ms; compiled: first chunk {:.1} ms, all {:.1} ms; both sides to line {middle} at once: {:.1} ms\n  by diff line: hunk of {} diff lines cold {:.1} ms; warm-up {:.1} ms, then the hunk {:.1} ms; compiled {:.1} ms",
            text.line_count(),
            cold.first_chunk,
            cold.full,
            compiled.first_chunk,
            compiled.full,
            time_both_sides(&text, grammar, middle),
            hunk.lines().len(),
            patch_cold,
            warm_up,
            patch_warmed,
            patch_compiled,
        );
        let opened = [0, 250, 500]
            .map(|delay| (delay, time_open(grammar, &hunk, Duration::from_millis(delay))));
        let in_first_frame = |colors: f64, wait: Duration| {
            if colors <= wait.as_secs_f64() * 1000.0 { "yes" } else { "no" }
        };
        println!(
            "  opened 0/250/500 ms after the pull request: {}",
            opened
                .iter()
                .map(|(delay, colors)| format!(
                    "{delay} ms: {colors:.1} ms ({})",
                    in_first_frame(*colors, patch_colors::WARMING_WAIT)
                ))
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
}
