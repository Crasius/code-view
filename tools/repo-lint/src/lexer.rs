//! A small Rust lexer for the source rules. It splits a file into code and comments, line by line,
//! and blanks the contents of string and char literals, so a keyword inside a string or a comment
//! never counts as code. It knows exactly what the rules need: line and nested block comments,
//! escaped and raw strings (`r#"..."#`, `br"..."`), and char literals versus lifetimes.

/// One source file, split into code and comments.
pub(crate) struct Scanned {
    /// Each line with comments removed and literal contents replaced by spaces; the quotes stay.
    pub code: Vec<String>,
    /// Comment text (without the `//` or `/*`) with its 1-based line. A block comment yields one
    /// entry per line it covers.
    pub comments: Vec<(usize, String)>,
}

#[derive(Clone, Copy)]
enum State {
    Code,
    LineComment,
    BlockComment(usize),
    Str,
    RawStr(usize),
    Char,
}

struct Lexer {
    code: Vec<String>,
    comments: Vec<(usize, String)>,
    comment: Option<String>,
}

impl Lexer {
    fn line(&self) -> usize {
        self.code.len()
    }

    fn push_code(&mut self, c: char) {
        if let Some(line) = self.code.last_mut() {
            line.push(c);
        }
    }

    fn push_comment(&mut self, c: char) {
        self.comment.get_or_insert_with(String::new).push(c);
    }

    fn end_comment_line(&mut self) {
        if let Some(text) = self.comment.take() {
            self.comments.push((self.line(), text));
        }
    }

    fn newline(&mut self) {
        self.end_comment_line();
        self.code.push(String::new());
    }
}

/// Splits `text` into code and comments.
pub(crate) fn scan(text: &str) -> Scanned {
    let chars: Vec<char> = text.chars().collect();
    let mut lexer = Lexer { code: vec![String::new()], comments: Vec::new(), comment: None };
    let mut state = State::Code;
    let mut i = 0;
    while let Some(&c) = chars.get(i) {
        let next = chars.get(i + 1).copied();
        if c == '\n' {
            lexer.newline();
            if matches!(state, State::LineComment | State::Char) {
                state = State::Code;
            }
            i += 1;
            continue;
        }
        let (new_state, used) = step(&mut lexer, state, &chars, i, c, next);
        state = new_state;
        i += used;
    }
    lexer.end_comment_line();
    if lexer.code.last().is_some_and(String::is_empty) && text.ends_with('\n') {
        lexer.code.pop();
    }
    Scanned { code: lexer.code, comments: lexer.comments }
}

/// Handles the char at `i` in `state`; returns the next state and how many chars it consumed.
fn step(
    lexer: &mut Lexer,
    state: State,
    chars: &[char],
    i: usize,
    c: char,
    next: Option<char>,
) -> (State, usize) {
    match state {
        State::Code => code_step(lexer, chars, i, c, next),
        State::LineComment => {
            lexer.push_comment(c);
            (State::LineComment, 1)
        }
        State::BlockComment(depth) => block_comment_step(lexer, depth, c, next),
        State::Str => match c {
            '\\' => {
                lexer.push_code(' ');
                if next.is_some_and(|n| n != '\n') {
                    lexer.push_code(' ');
                    return (State::Str, 2);
                }
                (State::Str, 1)
            }
            '"' => {
                lexer.push_code('"');
                (State::Code, 1)
            }
            _ => {
                lexer.push_code(' ');
                (State::Str, 1)
            }
        },
        State::RawStr(hashes) => {
            let closes = c == '"' && (1..=hashes).all(|k| chars.get(i + k) == Some(&'#'));
            if closes {
                lexer.push_code('"');
                return (State::Code, 1 + hashes);
            }
            lexer.push_code(' ');
            (State::RawStr(hashes), 1)
        }
        State::Char => match c {
            '\\' => {
                lexer.push_code(' ');
                lexer.push_code(' ');
                (State::Char, 2)
            }
            '\'' => {
                lexer.push_code('\'');
                (State::Code, 1)
            }
            _ => {
                lexer.push_code(' ');
                (State::Char, 1)
            }
        },
    }
}

fn code_step(
    lexer: &mut Lexer,
    chars: &[char],
    i: usize,
    c: char,
    next: Option<char>,
) -> (State, usize) {
    match (c, next) {
        ('/', Some('/')) => (State::LineComment, 2),
        ('/', Some('*')) => (State::BlockComment(1), 2),
        ('"', _) => {
            lexer.push_code('"');
            (State::Str, 1)
        }
        ('r', Some('"' | '#')) if !follows_ident(chars, i) => {
            let hashes = chars[i + 1..].iter().take_while(|&&h| h == '#').count();
            if chars.get(i + 1 + hashes) == Some(&'"') {
                lexer.push_code('r');
                lexer.push_code('"');
                return (State::RawStr(hashes), 2 + hashes);
            }
            lexer.push_code(c);
            (State::Code, 1)
        }
        ('\'', _) if is_char_literal(chars, i) => {
            lexer.push_code('\'');
            (State::Char, 1)
        }
        _ => {
            lexer.push_code(c);
            (State::Code, 1)
        }
    }
}

fn block_comment_step(
    lexer: &mut Lexer,
    depth: usize,
    c: char,
    next: Option<char>,
) -> (State, usize) {
    match (c, next) {
        ('*', Some('/')) if depth == 1 => {
            lexer.end_comment_line();
            (State::Code, 2)
        }
        ('*', Some('/')) => (State::BlockComment(depth - 1), 2),
        ('/', Some('*')) => (State::BlockComment(depth + 1), 2),
        _ => {
            lexer.push_comment(c);
            (State::BlockComment(depth), 1)
        }
    }
}

/// True when the char before `i` continues an identifier (so `r` is part of a name like `bar`),
/// except for the byte and C string prefixes `br` and `cr`.
fn follows_ident(chars: &[char], i: usize) -> bool {
    let Some(before) = i.checked_sub(1).and_then(|b| chars.get(b)) else {
        return false;
    };
    let is_prefix = matches!(before, 'b' | 'c')
        && !i.checked_sub(2).and_then(|b| chars.get(b)).is_some_and(|&p| is_ident_char(p));
    is_ident_char(*before) && !is_prefix
}

/// A quote at `i` opens a char literal (`'a'`, `'\n'`) rather than a lifetime (`'a`).
fn is_char_literal(chars: &[char], i: usize) -> bool {
    match chars.get(i + 1) {
        Some('\\') => true,
        Some(_) => chars.get(i + 2) == Some(&'\''),
        None => false,
    }
}

pub(crate) fn is_ident_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

#[cfg(test)]
mod tests {
    use super::*;

    fn code(text: &str) -> Vec<String> {
        code_lines(&scan(text))
    }

    fn code_lines(scanned: &Scanned) -> Vec<String> {
        scanned.code.iter().map(|line| line.trim_end().to_owned()).collect()
    }

    #[test]
    fn strips_line_and_nested_block_comments() {
        let scanned = scan("let a = 1; // unsafe here\n/* outer /* inner */ still */ let b;\n");
        assert_eq!(code_lines(&scanned), ["let a = 1;", " let b;"]);
        assert_eq!(scanned.comments[0], (1, " unsafe here".to_owned()));
        assert_eq!(scanned.comments[1].0, 2);
    }

    #[test]
    fn blanks_string_contents_and_keeps_the_line_count() {
        let blank = " ".repeat(r#"unsafe { \" }"#.len());
        assert_eq!(
            code("let s = \"unsafe { \\\" }\";\nnext"),
            [format!("let s = \"{blank}\";"), "next".to_owned()]
        );
        assert_eq!(code("let s = \"one\ntwo\";"), ["let s = \"", "   \";"]);
    }

    #[test]
    fn blanks_raw_and_byte_strings() {
        let blank = " ".repeat(r#"a "quoted" unsafe"#.len());
        assert_eq!(
            code(r##"let s = r#"a "quoted" unsafe"#; x"##),
            [format!("let s = r\"{blank}\"; x")]
        );
        assert_eq!(code(r#"let b = br"unsafe"; bar"#), ["let b = br\"      \"; bar"]);
        assert_eq!(code("let r#type = 1;"), ["let r#type = 1;"]);
    }

    #[test]
    fn tells_char_literals_from_lifetimes() {
        assert_eq!(
            code("let c = '{'; fn f<'a>(x: &'a str) {}"),
            ["let c = ' '; fn f<'a>(x: &'a str) {}"]
        );
        assert_eq!(code("let q = '\\''; let u = '\"'; done"), ["let q = '  '; let u = ' '; done"]);
    }

    #[test]
    fn comment_markers_inside_strings_stay_code() {
        let scanned = scan("let url = \"https://example.com\"; real();");
        assert!(scanned.comments.is_empty(), "{:?}", scanned.comments);
        assert!(scanned.code[0].ends_with("real();"));
    }
}
