//! **`recursion-check`:** fails when a function in the engine, lsp or cli sources can reach itself.
//!
//! Name-resolved over-approximation of the call graph. Calls on receivers of unknown type,
//! closures, trait objects and function pointers are not followed, so a clean scan is
//! necessary but not sufficient for recursion freedom. Everything is iterative, the gate
//! itself follows the rule it enforces.

use std::collections::HashMap;
use std::fmt::Write;
use std::fs;
use std::path::{Path, PathBuf};

const SCAN_ROOTS: &[&str] = &[
    "engine/src",
    "engine/lsp/src",
    "cli/src",
    "engine/packages/maven/native/lemma_jni/src",
    "engine/packages/hex/native/lemma_hex/src",
    "openapi/src",
];

pub fn run(workspace: &Path) -> Result<(), String> {
    let mut sources = Vec::new();
    for root in SCAN_ROOTS {
        for file in source_files(&workspace.join(root))? {
            let text =
                fs::read_to_string(&file).map_err(|e| format!("read {}: {e}", file.display()))?;
            sources.push((file, text));
        }
    }
    check(workspace, &sources)
}

fn check(workspace: &Path, sources: &[(PathBuf, String)]) -> Result<(), String> {
    let mut defs = Vec::new();
    let mut hidden_dispatch = Vec::new();
    for (file, source) in sources {
        scan_file(file, source, &mut defs, &mut hidden_dispatch);
    }
    let index = CallIndex::new(&defs);
    let edges: Vec<Vec<usize>> = defs.iter().map(|def| index.callees(def)).collect();

    let mut recursive = Vec::new();
    for component in strongly_connected(&edges) {
        let cyclic = component.len() > 1 || edges[component[0]].contains(&component[0]);
        if cyclic {
            recursive.push(component);
        }
    }
    if recursive.is_empty() && hidden_dispatch.is_empty() {
        println!(
            "recursion-check: no recursive functions among {} scanned functions, no function pointers or dyn Fn",
            defs.len()
        );
        return Ok(());
    }

    let mut message = String::new();
    if !hidden_dispatch.is_empty() {
        message.push_str("recursion-check: function pointers and dyn Fn are not allowed:");
        for hit in &hidden_dispatch {
            writeln!(message, "\n{hit}").expect("BUG: writing to String cannot fail");
        }
    }
    if recursive.is_empty() {
        return Err(message);
    }
    writeln!(
        message,
        "recursion-check: {} recursive components among {} scanned functions",
        recursive.len(),
        defs.len()
    )
    .expect("BUG: writing to String cannot fail");
    for mut component in recursive {
        component
            .sort_by(|a, b| (&defs[*a].file, defs[*a].line).cmp(&(&defs[*b].file, defs[*b].line)));
        message.push_str("\n---");
        for id in component {
            let def = &defs[id];
            let file = def.file.strip_prefix(workspace).unwrap_or(&def.file);
            let owner = def.owner.as_deref().unwrap_or("-");
            writeln!(
                message,
                "\n{}:{} {}::{}",
                file.display(),
                def.line,
                owner,
                def.name
            )
            .expect("BUG: writing to String cannot fail");
        }
    }
    Err(message)
}

struct FnDef {
    name: String,
    file: PathBuf,
    line: usize,
    owner: Option<String>,
    body: String,
}

struct Block {
    start: usize,
    end: usize,
    owner: String,
}

fn source_files(dir: &Path) -> Result<Vec<PathBuf>, String> {
    let mut pending = vec![dir.to_path_buf()];
    let mut files = Vec::new();
    while let Some(next) = pending.pop() {
        let entries =
            fs::read_dir(&next).map_err(|e| format!("read_dir {}: {e}", next.display()))?;
        for entry in entries {
            let path = entry
                .map_err(|e| format!("read_dir {}: {e}", next.display()))?
                .path();
            if path.is_dir() {
                pending.push(path);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                files.push(path);
            }
        }
    }
    files.sort();
    Ok(files)
}

fn scan_file(path: &Path, source: &str, defs: &mut Vec<FnDef>, hidden_dispatch: &mut Vec<String>) {
    let text = sanitize(source);
    let lines: Vec<&str> = text.lines().collect();
    let skipped = test_module_lines(&lines);
    let blocks = owner_blocks(&lines, &skipped);

    for (i, line) in lines.iter().enumerate() {
        if !skipped[i] && (line.contains("dyn Fn") || line.contains("fn(")) {
            hidden_dispatch.push(format!("{}:{}", path.display(), i + 1));
        }
    }

    for (i, (line, is_skipped)) in lines.iter().zip(&skipped).enumerate() {
        if *is_skipped {
            continue;
        }
        let Some(rest) = strip_qualifiers(line).strip_prefix("fn ") else {
            continue;
        };
        let name = leading_ident(rest);
        if name.is_empty() {
            continue;
        }
        if let Some(open) = find_body_open(&lines, i) {
            let end = block_end(&lines, open.0, open.1);
            defs.push(FnDef {
                name,
                file: path.to_path_buf(),
                line: i + 1,
                owner: owner_at(&blocks, i),
                body: body_text(&lines, open, end),
            });
        }
    }
}

/// Blanks comments and literal contents, keeps line structure.
fn sanitize(src: &str) -> String {
    let chars: Vec<char> = src.chars().collect();
    let mut out = String::with_capacity(src.len());
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let next = chars.get(i + 1).copied();
        if c == '/' && next == Some('/') {
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
        } else if c == '/' && next == Some('*') {
            i += 2;
            while i < chars.len() && !(chars[i] == '*' && chars.get(i + 1) == Some(&'/')) {
                if chars[i] == '\n' {
                    out.push('\n');
                }
                i += 1;
            }
            i += 2;
        } else if c == 'r' && is_raw_string_start(&chars, i) {
            let mut j = i + 1;
            let mut hashes = 0;
            while chars[j] == '#' {
                hashes += 1;
                j += 1;
            }
            j += 1;
            out.push('"');
            while j < chars.len() {
                if chars[j] == '"' && closes_raw(&chars, j + 1, hashes) {
                    j += 1 + hashes;
                    break;
                }
                if chars[j] == '\n' {
                    out.push('\n');
                }
                j += 1;
            }
            out.push('"');
            i = j;
        } else if c == '"' {
            out.push('"');
            i += 1;
            while i < chars.len() && chars[i] != '"' {
                if chars[i] == '\\' {
                    if chars.get(i + 1) == Some(&'\n') {
                        out.push('\n');
                    }
                    i += 2;
                } else {
                    if chars[i] == '\n' {
                        out.push('\n');
                    }
                    i += 1;
                }
            }
            out.push('"');
            i += 1;
        } else if c == '\'' {
            if next == Some('\\') {
                let mut j = i + 3;
                while j < chars.len() && chars[j] != '\'' {
                    j += 1;
                }
                out.push_str("''");
                i = j + 1;
            } else if chars.get(i + 2) == Some(&'\'') {
                out.push_str("''");
                i += 3;
            } else {
                out.push('\'');
                i += 1;
            }
        } else {
            out.push(c);
            i += 1;
        }
    }
    out
}

fn is_ident_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

fn is_raw_string_start(chars: &[char], i: usize) -> bool {
    if i > 0 && is_ident_char(chars[i - 1]) {
        return false;
    }
    let mut j = i + 1;
    while chars.get(j) == Some(&'#') {
        j += 1;
    }
    chars.get(j) == Some(&'"')
}

fn closes_raw(chars: &[char], from: usize, hashes: usize) -> bool {
    (0..hashes).all(|k| chars.get(from + k) == Some(&'#'))
}

fn strip_qualifiers(line: &str) -> &str {
    let mut s = line.trim_start();
    if let Some(rest) = s.strip_prefix("pub") {
        if rest.starts_with(' ') || rest.starts_with('(') {
            s = rest.trim_start();
            if s.starts_with('(') {
                if let Some(end) = s.find(')') {
                    s = s[end + 1..].trim_start();
                }
            }
        }
    }
    loop {
        if let Some(rest) = s.strip_prefix("extern ") {
            s = rest.trim_start();
            s = s.strip_prefix("\"\"").unwrap_or(s).trim_start();
            continue;
        }
        match ["const ", "async ", "unsafe "]
            .iter()
            .find_map(|prefix| s.strip_prefix(prefix))
        {
            Some(rest) => s = rest.trim_start(),
            None => return s,
        }
    }
}

fn leading_ident(s: &str) -> String {
    s.chars().take_while(|c| is_ident_char(*c)).collect()
}

/// Lines covered by `#[cfg(test)] mod ...` blocks.
fn test_module_lines(lines: &[&str]) -> Vec<bool> {
    let mut skipped = vec![false; lines.len()];
    let mut i = 0;
    while i < lines.len() {
        if lines[i].trim_start().starts_with("#[cfg(test)]") {
            let mut k = i + 1;
            while k < lines.len()
                && (lines[k].trim().is_empty() || lines[k].trim_start().starts_with('#'))
            {
                k += 1;
            }
            if k < lines.len() && strip_qualifiers(lines[k]).starts_with("mod ") {
                let last = match find_body_open(lines, k) {
                    Some(open) => block_end(lines, open.0, open.1).0,
                    None => k,
                };
                for flag in &mut skipped[i..=last] {
                    *flag = true;
                }
                i = last + 1;
                continue;
            }
        }
        i += 1;
    }
    skipped
}

fn owner_blocks(lines: &[&str], skipped: &[bool]) -> Vec<Block> {
    let mut blocks = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        if skipped[i] {
            continue;
        }
        let head = strip_qualifiers(line);
        let owner = if let Some(rest) = head.strip_prefix("impl") {
            if rest.starts_with(' ') || rest.starts_with('<') {
                type_head(rest)
            } else {
                None
            }
        } else if let Some(rest) = head.strip_prefix("trait ") {
            Some(leading_ident(rest)).filter(|name| !name.is_empty())
        } else {
            None
        };
        let Some(owner) = owner else { continue };
        if let Some(open) = find_body_open(lines, i) {
            let end = block_end(lines, open.0, open.1).0;
            blocks.push(Block {
                start: i,
                end,
                owner,
            });
        }
    }
    blocks
}

fn owner_at(blocks: &[Block], line: usize) -> Option<String> {
    blocks
        .iter()
        .filter(|block| block.start < line && line <= block.end)
        .max_by_key(|block| block.start)
        .map(|block| block.owner.clone())
}

/// Type name an impl block is for: `impl Trait for Type<T>` gives `Type`.
fn type_head(rest: &str) -> Option<String> {
    let mut s = rest.trim_start();
    if s.starts_with('<') {
        s = skip_angles(s);
    }
    let head = match s.find(" for ") {
        Some(idx) => &s[idx + 5..],
        None => s,
    };
    let head = head.trim_start().trim_start_matches('&').trim_start();
    let head = head.strip_prefix("dyn ").unwrap_or(head).trim_start();
    let path: String = head
        .chars()
        .take_while(|c| is_ident_char(*c) || *c == ':')
        .collect();
    let name = path.rsplit("::").next()?.to_string();
    if name.is_empty() {
        None
    } else {
        Some(name)
    }
}

/// `s` starts with `<`; returns the text after the matching `>` (`->` does not close).
fn skip_angles(s: &str) -> &str {
    let mut depth = 0usize;
    let mut prev = ' ';
    for (idx, ch) in s.char_indices() {
        match ch {
            '<' => depth += 1,
            '>' if prev != '-' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return &s[idx + 1..];
                }
            }
            _ => {}
        }
        prev = ch;
    }
    ""
}

/// Position of the body `{` of the item starting at `start`, unless a `;` ends it first.
fn find_body_open(lines: &[&str], start: usize) -> Option<(usize, usize)> {
    let mut nesting = 0i32;
    for (line_idx, line) in lines.iter().enumerate().skip(start) {
        for (col, ch) in line.char_indices() {
            match ch {
                '(' | '[' => nesting += 1,
                ')' | ']' => nesting -= 1,
                '{' if nesting <= 0 => return Some((line_idx, col)),
                ';' if nesting <= 0 => return None,
                _ => {}
            }
        }
    }
    None
}

/// Position just after the `}` closing the `{` at `(open_line, open_col)`.
fn block_end(lines: &[&str], open_line: usize, open_col: usize) -> (usize, usize) {
    let mut depth = 0usize;
    for (line_idx, line) in lines.iter().enumerate().skip(open_line) {
        let from = if line_idx == open_line { open_col } else { 0 };
        for (col, ch) in line[from..].char_indices() {
            match ch {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        return (line_idx, from + col + 1);
                    }
                }
                _ => {}
            }
        }
    }
    let last = lines.len() - 1;
    (last, lines[last].len())
}

fn body_text(lines: &[&str], open: (usize, usize), end: (usize, usize)) -> String {
    let (open_line, open_col) = open;
    let (end_line, end_col) = end;
    if open_line == end_line {
        return lines[open_line][open_col..end_col].to_string();
    }
    let mut out = lines[open_line][open_col..].to_string();
    for line in &lines[open_line + 1..end_line] {
        out.push('\n');
        out.push_str(line);
    }
    out.push('\n');
    out.push_str(&lines[end_line][..end_col]);
    out
}

#[derive(Clone, PartialEq, Eq)]
enum Tok {
    Ident(String),
    Sep,
    Dot,
    Bang,
    LParen,
    Other,
}

enum Call {
    Bare(String),
    SelfMethod(String),
    SelfPath(String),
    TypePath(String, String),
    Module(String, String),
    CrateRoot(String),
    /// `<T>::name(` or `<T as Trait>::name(`: any method called `name` may be the callee.
    QualifiedType(String),
}

fn tokenize(body: &str) -> Vec<Tok> {
    let chars: Vec<char> = body.chars().collect();
    let mut toks = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
        } else if c.is_ascii_alphabetic() || c == '_' {
            let start = i;
            while i < chars.len() && is_ident_char(chars[i]) {
                i += 1;
            }
            toks.push(Tok::Ident(chars[start..i].iter().collect()));
        } else if c.is_ascii_digit() {
            while i < chars.len() && is_ident_char(chars[i]) {
                i += 1;
            }
            toks.push(Tok::Other);
        } else if c == ':' && chars.get(i + 1) == Some(&':') {
            i += 2;
            let mut j = i;
            while j < chars.len() && chars[j].is_whitespace() {
                j += 1;
            }
            if chars.get(j) == Some(&'<') {
                i = skip_turbofish(&chars, j);
            } else {
                toks.push(Tok::Sep);
            }
        } else {
            i += 1;
            toks.push(match c {
                '.' => Tok::Dot,
                '!' => Tok::Bang,
                '(' => Tok::LParen,
                _ => Tok::Other,
            });
        }
    }
    toks
}

/// `chars[open]` is `<`; returns the index after the matching `>`.
fn skip_turbofish(chars: &[char], open: usize) -> usize {
    let mut depth = 0usize;
    let mut k = open;
    while k < chars.len() {
        match chars[k] {
            '<' => depth += 1,
            '>' if chars[k - 1] != '-' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return k + 1;
                }
            }
            _ => {}
        }
        k += 1;
    }
    chars.len()
}

fn is_ident(tok: &Tok, text: &str) -> bool {
    matches!(tok, Tok::Ident(s) if s == text)
}

fn calls(body: &str) -> Vec<Call> {
    let toks = tokenize(body);
    let mut out = Vec::new();
    for k in 1..toks.len() {
        if toks[k] != Tok::LParen {
            continue;
        }
        let Tok::Ident(name) = &toks[k - 1] else {
            continue;
        };
        if k >= 2 && (toks[k - 2] == Tok::Bang || is_ident(&toks[k - 2], "fn")) {
            continue;
        }
        let name = name.clone();

        let mut prefix: Vec<String> = Vec::new();
        let mut j = k - 1;
        while j >= 2 && toks[j - 1] == Tok::Sep {
            let Tok::Ident(seg) = &toks[j - 2] else {
                break;
            };
            prefix.push(seg.clone());
            j -= 2;
        }
        prefix.reverse();

        match prefix.last().cloned() {
            None => {
                if j >= 1 && toks[j - 1] == Tok::Sep {
                    out.push(Call::QualifiedType(name));
                } else if j >= 1 && toks[j - 1] == Tok::Dot {
                    if j >= 2 && is_ident(&toks[j - 2], "self") {
                        out.push(Call::SelfMethod(name));
                    }
                } else {
                    out.push(Call::Bare(name));
                }
            }
            Some(last) => {
                if last == "Self" {
                    out.push(Call::SelfPath(name));
                } else if last.starts_with(char::is_uppercase) {
                    out.push(Call::TypePath(last, name));
                } else if matches!(last.as_str(), "crate" | "super" | "self") {
                    out.push(Call::CrateRoot(name));
                } else {
                    out.push(Call::Module(last, name));
                }
            }
        }
    }
    out
}

fn module_names(file: &Path) -> Vec<String> {
    let mut names = Vec::new();
    if let Some(stem) = file.file_stem().and_then(|s| s.to_str()) {
        names.push(stem.to_string());
    }
    if let Some(parent) = file
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|s| s.to_str())
    {
        names.push(parent.to_string());
    }
    names
}

struct CallIndex {
    owner_methods: HashMap<(String, String), Vec<usize>>,
    methods_by_name: HashMap<String, Vec<usize>>,
    free_in_file: HashMap<(PathBuf, String), Vec<usize>>,
    free_by_name: HashMap<String, Vec<usize>>,
    free_by_module: HashMap<(String, String), Vec<usize>>,
}

impl CallIndex {
    fn new(defs: &[FnDef]) -> Self {
        let mut index = CallIndex {
            owner_methods: HashMap::new(),
            methods_by_name: HashMap::new(),
            free_in_file: HashMap::new(),
            free_by_name: HashMap::new(),
            free_by_module: HashMap::new(),
        };
        for (id, def) in defs.iter().enumerate() {
            match &def.owner {
                Some(owner) => {
                    index
                        .owner_methods
                        .entry((owner.clone(), def.name.clone()))
                        .or_default()
                        .push(id);
                    index
                        .methods_by_name
                        .entry(def.name.clone())
                        .or_default()
                        .push(id);
                }
                None => {
                    index
                        .free_in_file
                        .entry((def.file.clone(), def.name.clone()))
                        .or_default()
                        .push(id);
                    index
                        .free_by_name
                        .entry(def.name.clone())
                        .or_default()
                        .push(id);
                    for module in module_names(&def.file) {
                        index
                            .free_by_module
                            .entry((module, def.name.clone()))
                            .or_default()
                            .push(id);
                    }
                }
            }
        }
        index
    }

    fn callees(&self, def: &FnDef) -> Vec<usize> {
        let mut out = Vec::new();
        for call in calls(&def.body) {
            match call {
                Call::SelfMethod(name) | Call::SelfPath(name) => {
                    if let Some(owner) = &def.owner {
                        extend(&mut out, self.owner_methods.get(&(owner.clone(), name)));
                    }
                }
                Call::TypePath(ty, name) => {
                    extend(&mut out, self.owner_methods.get(&(ty, name)));
                }
                Call::CrateRoot(name) => {
                    extend(&mut out, self.free_by_name.get(&name));
                }
                Call::QualifiedType(name) => {
                    extend(&mut out, self.methods_by_name.get(&name));
                }
                Call::Module(module, name) => {
                    extend(&mut out, self.free_by_module.get(&(module, name)));
                }
                Call::Bare(name) => {
                    let local = self.free_in_file.get(&(def.file.clone(), name.clone()));
                    if local.is_some() {
                        extend(&mut out, local);
                    } else {
                        extend(&mut out, self.free_by_name.get(&name));
                    }
                }
            }
        }
        out.sort_unstable();
        out.dedup();
        out
    }
}

fn extend(out: &mut Vec<usize>, found: Option<&Vec<usize>>) {
    if let Some(ids) = found {
        out.extend(ids.iter().copied());
    }
}

/// Tarjan's SCC with an explicit stack.
fn strongly_connected(edges: &[Vec<usize>]) -> Vec<Vec<usize>> {
    let n = edges.len();
    let mut index = vec![usize::MAX; n];
    let mut low = vec![0usize; n];
    let mut on_stack = vec![false; n];
    let mut stack: Vec<usize> = Vec::new();
    let mut components = Vec::new();
    let mut counter = 0usize;

    for root in 0..n {
        if index[root] != usize::MAX {
            continue;
        }
        index[root] = counter;
        low[root] = counter;
        counter += 1;
        stack.push(root);
        on_stack[root] = true;
        let mut work: Vec<(usize, usize)> = vec![(root, 0)];

        while let Some(&(v, edge)) = work.last() {
            if edge < edges[v].len() {
                work.last_mut().expect("BUG: work frame exists").1 += 1;
                let w = edges[v][edge];
                if index[w] == usize::MAX {
                    index[w] = counter;
                    low[w] = counter;
                    counter += 1;
                    stack.push(w);
                    on_stack[w] = true;
                    work.push((w, 0));
                } else if on_stack[w] {
                    low[v] = low[v].min(index[w]);
                }
            } else {
                work.pop();
                if low[v] == index[v] {
                    let mut component = Vec::new();
                    loop {
                        let w = stack.pop().expect("BUG: tarjan stack holds v");
                        on_stack[w] = false;
                        component.push(w);
                        if w == v {
                            break;
                        }
                    }
                    components.push(component);
                }
                if let Some(&(parent, _)) = work.last() {
                    low[parent] = low[parent].min(low[v]);
                }
            }
        }
    }
    components
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gate(source: &str) -> Result<(), String> {
        check(
            Path::new(""),
            &[(PathBuf::from("src/lib.rs"), source.to_string())],
        )
    }

    #[test]
    fn self_recursion_is_rejected() {
        let err = gate("fn walk(n: u32) -> u32 {\n    if n == 0 { 0 } else { walk(n - 1) }\n}\n")
            .expect_err("self recursion must fail");
        assert!(err.contains("recursive components"), "{err}");
        assert!(err.contains("::walk"), "{err}");
    }

    #[test]
    fn mutual_recursion_is_rejected() {
        let err = gate("fn ping(n: u32) -> u32 { pong(n) }\nfn pong(n: u32) -> u32 { ping(n) }\n")
            .expect_err("mutual recursion must fail");
        assert!(err.contains("recursive components"), "{err}");
    }

    #[test]
    fn self_method_recursion_is_rejected() {
        let err = gate("struct S;\nimpl S {\n    fn f(&self) { self.f() }\n}\n")
            .expect_err("method recursion must fail");
        assert!(err.contains("S::f"), "{err}");
    }

    #[test]
    fn iterative_code_is_accepted() {
        gate("fn walk(n: u32) -> u32 {\n    let mut total = 0;\n    for i in 0..n { total += i; }\n    total\n}\n")
            .expect("iterative code is clean");
    }

    #[test]
    fn extern_system_functions_are_indexed() {
        let err = gate(
            "pub extern \"system\" fn ping(n: u32) -> u32 { pong(n) }\nfn pong(n: u32) -> u32 { ping(n) }\n",
        )
        .expect_err("extern cycle must fail");
        assert!(err.contains("recursive components"), "{err}");
    }

    #[test]
    fn test_modules_are_skipped() {
        gate("#[cfg(test)]\nmod tests {\n    fn walk(n: u32) -> u32 { walk(n) }\n}\n")
            .expect("test module is skipped");
    }

    #[test]
    fn dyn_fn_and_fn_pointers_are_rejected() {
        let err = gate("fn apply(f: &dyn Fn(u32) -> u32) -> u32 { f(1) }\n")
            .expect_err("dyn Fn must fail");
        assert!(err.contains("dyn Fn are not allowed"), "{err}");
        let err = gate("fn apply(f: fn(u32) -> u32) -> u32 { f(1) }\n")
            .expect_err("fn pointer must fail");
        assert!(err.contains("function pointers"), "{err}");
    }
}
