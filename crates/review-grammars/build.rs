//! Builds the app's one syntax set: bat's grammars (via two-face) with ours from `grammars/` in
//! place of bat's of the same name, dumped into `OUT_DIR` for `lib.rs` to load.

use std::collections::HashMap;
use std::path::Path;

use syntect::parsing::SyntaxSetBuilder;
use syntect::parsing::syntax_definition::{Context, ContextReference, MatchOperation, Pattern};

const GRAMMARS: &str = "grammars";

/// Per index of a replaced grammar: the scope of ours, and the context names of bat's in the
/// order `build` gave them their ids.
type Replaced = HashMap<usize, (String, Vec<String>)>;

fn main() {
    dump_grammars();
}

/// One set, so an embedding grammar (Markdown's C# fences) and a file of the embedded language
/// use the very same grammar. Lines include their newline, as the app parses them.
///
/// bat's set comes linked: its references to other grammars are context ids. Each of our grammars
/// takes the place of bat's of the same name, and each id into a replaced grammar becomes a
/// reference by scope and context name, which `build` links to ours.
fn dump_grammars() {
    println!("cargo:rerun-if-changed={GRAMMARS}");
    let mut own = SyntaxSetBuilder::new();
    own.add_from_folder(GRAMMARS, true).expect("our grammars are valid");
    let mut syntaxes = two_face::syntax::extra_newlines().into_builder().syntaxes().to_vec();
    let mut replaced = Replaced::new();
    for ours in own.syntaxes() {
        match syntaxes.iter().position(|syntax| syntax.name == ours.name) {
            Some(index) => {
                let mut names: Vec<String> = syntaxes[index].contexts.keys().cloned().collect();
                names.sort_unstable();
                replaced.insert(index, (ours.scope.build_string(), names));
                syntaxes[index] = ours.clone();
            }
            None => syntaxes.push(ours.clone()),
        }
    }
    for context in syntaxes.iter_mut().flat_map(|syntax| syntax.contexts.values_mut()) {
        relink_context(context, &replaced);
    }
    let mut builder = SyntaxSetBuilder::new();
    for syntax in syntaxes {
        builder.add(syntax);
    }
    let out_dir = std::env::var("OUT_DIR").expect("cargo sets OUT_DIR");
    let dump = Path::new(&out_dir).join("grammars.packdump");
    syntect::dumps::dump_to_uncompressed_file(&builder.build(), dump)
        .expect("write the grammar dump");
}

/// Relinks every reference of `context` (see [`relink`]).
fn relink_context(context: &mut Context, replaced: &Replaced) {
    for pattern in &mut context.patterns {
        match pattern {
            Pattern::Include(reference) => relink(reference, replaced),
            Pattern::Match(pattern) => {
                if let MatchOperation::Push(references) | MatchOperation::Set(references) =
                    &mut pattern.operation
                {
                    for reference in references {
                        relink(reference, replaced);
                    }
                }
                if let Some(reference) = &mut pattern.with_prototype {
                    relink(reference, replaced);
                }
            }
        }
    }
}

/// Turns a context id into a replaced grammar into a reference by its scope and context name.
/// syntect keeps both kinds of reference opaque, so they go through their serde form.
fn relink(reference: &mut ContextReference, replaced: &Replaced) {
    let value = serde_json::to_value(&*reference).expect("a context reference serializes");
    let Some(id) = value.get("Direct") else {
        return;
    };
    let index = |field: &str| {
        let index = id[field].as_u64().expect("context id fields are indexes");
        usize::try_from(index).expect("an index fits a usize")
    };
    let Some((scope, names)) = replaced.get(&index("syntax_index")) else {
        return;
    };
    let by_scope = serde_json::json!({ "ByScope": {
        "scope": scope,
        "sub_context": names[index("context_index")],
        "with_escape": false,
    }});
    *reference = serde_json::from_value(by_scope).expect("a reference by scope deserializes");
}
