/// Creates a lazily initialized static value.
macro_rules! lazy {
    ($ty:ty = $init:expr) => {{
        static VAL: ::std::sync::LazyLock<$ty> = ::std::sync::LazyLock::new(|| $init);
        &*VAL
    }};
}

/// Creates a lazily initialized regular expression.
macro_rules! re {
    ($s:expr) => {
        lazy!(Regex = Regex::new($s).unwrap())
    };
}

mod md;
mod visitor;

use std::fmt::Write;
use std::path::Path;
use std::sync::LazyLock;

use serde::Deserialize;
use syn::visit::Visit;
use typstyle_core::{Config, Typstyle};

use crate::visitor::DocsVisitor;

static ROOT: LazyLock<String> =
    LazyLock::new(|| std::env::args().nth(1).expect("path to docs content"));

fn main() {
    convert_markdown_files();
    convert_in_source_comments();
    convert_groups();
}

fn convert_groups() {
    #[derive(Deserialize)]
    struct Group {
        name: String,
        details: String,
    }

    let path = Path::new(ROOT.as_str()).join("reference/groups.yml");
    let data = std::fs::read_to_string(&path).unwrap();
    let groups: Vec<Group> = serde_yaml::from_str(&data).unwrap();

    let mut out = String::new();
    for group in groups {
        let opt = md::Options { path: None, heading_offset: 0 };
        let conv = md::convert_md_to_typ(&group.details, &opt);
        writeln!(out, "// {}", group.name).unwrap();
        writeln!(out, "{conv}\n").unwrap();
    }
    std::fs::write(path.with_extension("typ"), out).unwrap();
    std::fs::remove_file(path).unwrap();
}

fn convert_markdown_files() {
    for entry in walkdir::WalkDir::new(ROOT.as_str()) {
        let entry = entry.unwrap();
        let path = entry.path();
        if path.extension().is_none_or(|ext| ext != "md") {
            continue;
        }

        println!("Converting Markdown file: {}", entry.path().display());
        convert_markdown_file(path);
        std::fs::remove_file(path).unwrap();
    }
}

fn convert_markdown_file(path: &Path) {
    let mut string = std::fs::read_to_string(path).unwrap();
    string = doc_file_passes(&string, path);
    let stem = path.file_stem().unwrap().to_str().unwrap();
    let output = path.with_file_name(format!("{stem}.typ"));
    std::fs::write(output, string).unwrap();
}

fn convert_in_source_comments() {
    for entry in walkdir::WalkDir::new("/Users/laurenz/Code/typst/crates") {
        let entry = entry.unwrap();
        if entry.path().extension().is_none_or(|ext| ext != "rs") {
            continue;
        }

        println!("Converting source file: {}", entry.path().display());

        let string = std::fs::read_to_string(entry.path()).unwrap();
        let file = syn::parse_file(&string).unwrap();
        let mut d = DocsVisitor::new();
        d.visit_file(&file);

        let mut cursor = 0;
        let mut rewritten = String::new();

        for (range, docs) in d.finish() {
            let indent =
                string[..range.start].chars().rev().take_while(|&c| c == ' ').count();

            let budget = 80 - indent - 4;
            let wrapped = doc_comment_passes(&docs, budget);
            let indent_str = " ".repeat(indent);

            rewritten.push_str(&string[cursor..range.start - indent]);

            for (i, line) in wrapped.lines().enumerate() {
                if i != 0 {
                    rewritten.push('\n');
                }

                rewritten.push_str(&indent_str);
                rewritten.push_str("///");
                let l = line.trim_end();
                if !l.is_empty() {
                    rewritten.push(' ');
                    rewritten.push_str(l);
                }
            }

            cursor = range.end;
        }

        rewritten.push_str(&string[cursor..]);
        std::fs::write(entry.path(), rewritten).unwrap();
    }
}

fn doc_file_passes(string: &str, path: &Path) -> String {
    let path = path.strip_prefix(ROOT.as_str()).ok().and_then(|path| path.to_str());

    let heading_offset = if let Some(path) = path
        && (path.contains("reference/library") || path.contains("reference/export"))
    {
        0
    } else {
        -1
    };

    let opt = md::Options { path, heading_offset };
    let processed = md::convert_md_to_typ(string, &opt);

    let mut out = String::new();
    for line in processed.lines() {
        out.push_str(line.trim_end());
        out.push('\n');
    }
    out
}

fn doc_comment_passes(string: &str, width: usize) -> String {
    let opt = md::Options::default();
    format_width(&md::convert_md_to_typ(string, &opt), width)
}

fn format_width(markup: &str, width: usize) -> String {
    Typstyle::new(Config::new().with_width(width).with_wrap_text(true))
        .format_text(markup)
        .render()
        .unwrap_or_else(|e| panic!("{e}\n----\n{markup}"))
}
