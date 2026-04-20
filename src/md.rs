use std::collections::{BTreeSet, HashMap};
use std::fmt::Write;
use std::path::Path;

use heck::ToKebabCase;
use markdown::{Constructs, ParseOptions, mdast};
use regex::{Regex, Replacer};
use serde::Deserialize;

#[derive(Default)]
pub struct Options<'a> {
    pub path: Option<&'a str>,
    pub heading_offset: isize,
}

struct Context<'a> {
    string: &'a str,
    opt: &'a Options<'a>,
    imports: BTreeSet<&'static str>,
    link_defs: HashMap<String, String>,
    footnote_defs: HashMap<String, Vec<mdast::Node>>,
}

pub fn convert_md_to_typ(string: &str, opt: &Options) -> String {
    let node = markdown::to_mdast(
        string,
        &ParseOptions {
            constructs: Constructs {
                autolink: true,
                block_quote: true,
                frontmatter: true,
                gfm_autolink_literal: true,
                gfm_footnote_definition: true,
                gfm_label_start_footnote: true,
                gfm_table: true,
                hard_break_trailing: false,
                heading_setext: false,
                math_flow: true,
                math_text: true,
                html_flow: false,
                html_text: false,
                mdx_jsx_flow: true,
                mdx_jsx_text: true,
                ..Default::default()
            },
            ..Default::default()
        },
    )
    .unwrap();

    let mdast::Node::Root(root) = node else { panic!() };

    let mut ctx = Context {
        string,
        opt,
        imports: Default::default(),
        link_defs: Default::default(),
        footnote_defs: Default::default(),
    };
    let ctx = &mut ctx;

    collect_link_defs(&root.children, ctx);

    let mut meta = None;
    let mut children = root.children.as_slice();
    if let [mdast::Node::Yaml(yaml), rest @ ..] = children {
        let mut data = serde_yaml::from_str::<Metadata>(&yaml.value).unwrap();
        if data.title.is_none() {
            data.title = children.iter().find_map(|node| match node {
                mdast::Node::Heading(h) if h.depth == 1 => {
                    Some(convert_inlines(&h.children, ctx))
                }
                _ => None,
            });
        }
        meta = Some(data);
        children = rest;
    }

    let flow = convert_flow(children, ctx);

    let mut preamble = String::new();
    if let Some(path) = ctx.opt.path {
        if path.contains("changelog") {
            writeln!(preamble, r#"#import "utils.typ": *"#).unwrap();
            writeln!(preamble).unwrap();
        } else {
            ctx.imports.insert("docs-chapter");

            let depth = Path::new(path).components().count();
            writeln!(
                preamble,
                r#"#import "{}components/index.typ": {}"#,
                "../".repeat(depth),
                ctx.imports.iter().copied().collect::<Vec<_>>().join(", ")
            )
            .unwrap();
            writeln!(preamble).unwrap();

            writeln!(preamble, "#show: docs-chapter.with(").unwrap();

            if let Some(data) = &meta
                && let Some(title) = &data.title
            {
                writeln!(preamble, "  title: {title:?},").unwrap();
            }

            let route = Path::new(path).with_extension("").to_str().unwrap().to_string();
            writeln!(preamble, "  route: \"/{route}\",").unwrap();

            if let Some(data) = &meta
                && let Some(description) = &data.description
            {
                writeln!(
                    preamble,
                    "  description: {:?},",
                    description.replace("\n", " ")
                )
                .unwrap();
            }

            writeln!(preamble, ")").unwrap();
            writeln!(preamble).unwrap();
        }
    }

    preamble + &flow
}

fn collect_link_defs(nodes: &[mdast::Node], ctx: &mut Context) {
    for node in nodes {
        match node {
            mdast::Node::Definition(def) => {
                ctx.link_defs.insert(def.identifier.clone(), def.url.clone());
            }
            mdast::Node::FootnoteDefinition(def) => {
                ctx.footnote_defs.insert(def.identifier.clone(), def.children.clone());
            }
            _ => {}
        }
    }
}

/// Front matter metadata.
#[derive(Deserialize)]
struct Metadata {
    title: Option<String>,
    description: Option<String>,
}

fn convert_flow(nodes: &[mdast::Node], ctx: &mut Context) -> String {
    let mut buf = String::new();
    let mut sep = "";
    let mut iter = nodes.iter().peekable();
    while let Some(node) = iter.next() {
        let text = convert_flow_node(node, ctx);
        if text.is_empty() {
            continue;
        }

        buf.push_str(sep);
        buf += &text;

        if let Some(mdast::Node::List(list)) = iter.peek()
            && let Some(curr_pos) = node.position()
            && let Some(next_pos) = &list.position
            && curr_pos.end.line + 1 == next_pos.start.line
        {
            sep = "\n";
        } else if matches!(node, mdast::Node::Heading(_)) {
            sep = "\n";
        } else {
            sep = "\n\n";
        }
    }
    buf
}

fn convert_flow_node(node: &mdast::Node, ctx: &mut Context) -> String {
    match node {
        mdast::Node::MdxJsxFlowElement(node) => convert_flow_jsx(node, ctx),
        mdast::Node::Heading(node) => convert_heading(node, ctx),
        mdast::Node::Code(node) => convert_code_block(node, ctx),
        mdast::Node::ThematicBreak(node) => convert_thematic_break(node, ctx),
        mdast::Node::Blockquote(node) => convert_blockquote(node, ctx),
        mdast::Node::List(node) => convert_list(node, ctx),
        mdast::Node::Table(node) => convert_table(node, ctx),
        mdast::Node::Paragraph(node) => convert_inlines(&node.children, ctx),
        // These are link definitions, not definition lists. They are not needed
        // in the Typst version if the link is already resolved.
        mdast::Node::Definition(_) => String::new(),
        mdast::Node::FootnoteDefinition(_) => String::new(),
        node => unimplemented!("flow: {node:?}"),
    }
}

fn convert_flow_jsx(node: &mdast::MdxJsxFlowElement, ctx: &mut Context) -> String {
    let body = convert_flow(&node.children, ctx);
    match node.name.as_deref() {
        // Intentionally ignored, not needed anymore in the Typst version.
        Some("contributors") => String::new(),
        Some("div") => {
            if let [mdast::AttributeContent::Property(property)] =
                node.attributes.as_slice()
                && property.name == "class"
                && let Some(mdast::AttributeValue::Literal(lit)) = &property.value
                && lit == "info-box"
            {
                ctx.imports.insert("info");
                call_blocky("info", &body)
            } else {
                unimplemented!("div: {node:?}")
            }
        }
        Some("details") => {
            ctx.imports.insert("details");
            call_blocky("details", &body)
        }
        Some("summary") => call_blocky("summary", &body),
        node => unimplemented!("flow jsx: {node:?}"),
    }
}

fn convert_heading(node: &mdast::Heading, ctx: &mut Context) -> String {
    let depth = (node.depth as isize) + ctx.opt.heading_offset;
    if depth < 1 {
        return String::new();
    }

    let eqs = "=".repeat(depth as usize);
    let body = convert_inlines(&node.children, ctx);
    let patched = replace(
        &body,
        r#"(?m)^\s*(.*?)\s*(\{\s*\\#(\S*)\s*\})?\s*$"#,
        |cap: &regex::Captures| {
            let body = &cap[1];
            let label = match cap.get(3) {
                Some(id) => id.as_str().to_string(),
                None => body.to_kebab_case(),
            };
            format!("{body} <{label}>")
        },
    );
    assert!(!patched.contains('\n'));

    // This changelog heading is not needed in the new docs.
    if ctx.opt.path.is_some_and(|p| p.contains("changelog"))
        && patched == "Contributors <contributors>"
    {
        return String::new();
    }

    format!("{eqs} {patched}")
}

fn convert_code_block(node: &mdast::Code, ctx: &mut Context) -> String {
    let mut title = None;
    let mut single = false;
    let mut zoom = None;
    let mut tag = "";
    if let Some(lang) = &node.lang {
        if let Some((before, after)) = lang.split_once(':')
            && !after.is_empty()
        {
            assert!(matches!(before, "example" | "preview"));
            if after.starts_with('"') {
                let count = node.position.as_ref().unwrap().start.offset;
                let offset = ctx.string.chars().take(count).map(char::len_utf8).count();
                let start = offset + ctx.string[offset..].find('"').unwrap() + 1;
                let end = start + ctx.string[start..].find('"').unwrap();
                title = Some(&ctx.string[start..end]);
            } else if after == "single" {
                single = true;
            } else {
                let mut iter = after.split(",");
                let mut f = || iter.next().unwrap();
                zoom = Some([f(), f(), f(), f()]);
            }
        } else {
            tag = lang;
        }
    }

    let consecutive = max_consecutive_backticks(&node.value);
    let delim = "`".repeat((consecutive + 1).max(3));
    let raw = format!("{delim}{tag}\n{}\n{delim}", node.value);

    if title.is_some() || single || zoom.is_some() {
        ctx.imports.insert("example");
        let mut buf = String::new();
        writeln!(buf, "#example(").unwrap();
        if let Some(title) = title {
            writeln!(buf, "  title: {title:?},").unwrap();
        }
        if single {
            writeln!(buf, "  single: true,").unwrap();
        }
        if let Some([x, y, w, h]) = zoom {
            writeln!(buf, "  zoom: ({x}pt, {y}pt, {w}pt, {h}pt),").unwrap();
        }
        writeln!(buf, "{}", indent(&raw)).unwrap();
        write!(buf, ")").unwrap();
        buf
    } else {
        raw
    }
}

fn convert_thematic_break(_: &mdast::ThematicBreak, _: &mut Context) -> String {
    "#divider()".into()
}

fn convert_blockquote(node: &mdast::Blockquote, ctx: &mut Context) -> String {
    call_blocky("quote(block: true)", &convert_flow(&node.children, ctx))
}

fn convert_list(node: &mdast::List, ctx: &mut Context) -> String {
    assert!(matches!(node.start, None | Some(1)));

    let items = node
        .children
        .iter()
        .map(|node| match node {
            mdast::Node::ListItem(item) => item,
            node => unimplemented!("list: {node:?}"),
        })
        .collect::<Vec<_>>();

    let mut buf = String::new();

    for (i, item) in items.iter().enumerate() {
        if i > 0 && node.spread {
            writeln!(buf).unwrap();
        }

        let mut head = if node.ordered { "+ " } else { "- " };
        let text = convert_flow(&item.children, ctx);
        for line in text.lines() {
            writeln!(buf, "{head}{line}").unwrap();
            head = "  ";
        }
    }

    assert_eq!(buf.pop(), Some('\n'));
    buf
}

fn convert_table(node: &mdast::Table, ctx: &mut Context) -> String {
    let rows = node
        .children
        .iter()
        .map(|node| match node {
            mdast::Node::TableRow(row) => row
                .children
                .iter()
                .map(|node| match node {
                    mdast::Node::TableCell(cell) => cell,
                    node => unimplemented!("table row: {node:?}"),
                })
                .collect::<Vec<_>>(),
            node => unimplemented!("table: {node:?}"),
        })
        .collect::<Vec<_>>();

    ctx.imports.insert("docs-table");

    let mut buf = String::new();
    writeln!(buf, "#docs-table(").unwrap();

    let header = &rows[0];
    write!(buf, "  table.header").unwrap();
    for cell in header {
        let cell = convert_inlines(&cell.children, ctx);
        write!(buf, "[{cell}]").unwrap();
    }
    writeln!(buf, ",").unwrap();

    for row in &rows[1..] {
        writeln!(buf).unwrap();
        for cell in row {
            let cell = convert_inlines(&cell.children, ctx);
            writeln!(buf, "  [{cell}],").unwrap();
        }
    }

    write!(buf, ")").unwrap();

    buf
}

fn convert_inlines(nodes: &[mdast::Node], ctx: &mut Context) -> String {
    let mut buf = String::new();
    for node in nodes {
        buf += &convert_inline_node(node, ctx);
    }
    buf
}

fn convert_inline_node(node: &mdast::Node, ctx: &mut Context) -> String {
    match node {
        mdast::Node::MdxJsxTextElement(node) => convert_inline_jsx(node, ctx),
        mdast::Node::Strong(node) => convert_strong(node, ctx),
        mdast::Node::Emphasis(node) => convert_emphasis(node, ctx),
        mdast::Node::Text(node) => convert_text(&node.value, ctx),
        mdast::Node::InlineCode(node) => convert_inline_code(node, ctx),
        mdast::Node::Link(node) => convert_link(node, ctx),
        mdast::Node::LinkReference(node) => convert_link_ref(node, ctx),
        mdast::Node::FootnoteReference(node) => convert_footnote_ref(node, ctx),
        mdast::Node::Image(node) => convert_image(node, ctx),
        mdast::Node::Break(node) => convert_break(node, ctx),
        node => unimplemented!("inline: {node:?}"),
    }
}

fn convert_inline_jsx(node: &mdast::MdxJsxTextElement, ctx: &mut Context) -> String {
    let body = convert_inlines(&node.children, ctx);
    match node.name.as_deref() {
        Some("sup") => format!("#super[{body}]"),
        Some("kbd") => {
            ctx.imports.insert("kbd");
            format!("#kbd[{body}]")
        }
        node => unimplemented!("inline jsx: {node:?}"),
    }
}

fn convert_strong(node: &mdast::Strong, ctx: &mut Context) -> String {
    let body = convert_inlines(&node.children, ctx);
    assert!(!body.contains('*'));
    format!("*{body}*")
}

fn convert_emphasis(node: &mdast::Emphasis, ctx: &mut Context) -> String {
    let body = convert_inlines(&node.children, ctx);
    assert!(!body.contains('_'));
    format!("_{body}_")
}

fn convert_text(text: &str, _: &mut Context) -> String {
    let mut buf = String::new();
    for c in text.chars() {
        match c {
            ' ' | '\t' | '\n' | '\x0b' | '\x0c' | '\r' => buf.push(' '),
            '\\' | '[' | ']' | '~' | '*' | '_' | '`' | '$' | '@' | '#' => {
                buf.push('\\');
                buf.push(c);
            }
            _ => buf.push(c),
        }
    }
    buf
}

fn convert_inline_code(node: &mdast::InlineCode, _: &mut Context) -> String {
    let consecutive = max_consecutive_backticks(&node.value);
    let count = if consecutive == 0 { 1 } else { (consecutive + 1).max(3) };
    let delim = "`".repeat(count);
    let sep1 = if node.value.starts_with('`') || count >= 3 { " " } else { "" };
    let sep2 = if node.value.ends_with('`') { " " } else { "" };
    format!("{delim}{sep1}{}{sep2}{delim}", node.value.replace("\n", " "))
}

fn convert_link(node: &mdast::Link, ctx: &mut Context) -> String {
    assert_eq!(node.title, None);
    let body = convert_inlines(&node.children, ctx);

    if let Some(caps1) = re!("^https://github.com/([\\w0-9\\-]+)$").captures(&node.url) {
        let caps2 = re!("^\\\\@([\\w0-9\\-]+)$").captures(&body).unwrap();
        assert_eq!(&caps1[1], &caps2[1]);
        return format!("#gh({:?})", &caps1[1]);
    }

    if node.url.starts_with("http") {
        return format!("#link({:?})[{body}]", node.url);
    }

    let Some(target) = node.url.strip_prefix('$') else {
        panic!("bad link: {}", node.url);
    };

    resolve_link(target, &body, ctx)
}

fn convert_link_ref(node: &mdast::LinkReference, ctx: &mut Context) -> String {
    let body = convert_inlines(&node.children, ctx);

    if let Some(url) = ctx.link_defs.get(&node.identifier) {
        return format!("#link({url:?})[{body}]");
    }

    if let Some(caps) = re!("^(\\w+/\\w+)?#(\\d+)$").captures(&node.identifier) {
        let nr = &caps[2];
        if let Some(repo) = caps.get(1) {
            return format!("#pr({nr}, repo: {:?})", repo.as_str(),);
        } else {
            return format!("#pr({nr})");
        }
    }

    resolve_link(node.identifier.trim_matches('`'), &body, ctx)
}

fn resolve_link(target: &str, body: &str, _: &mut Context) -> String {
    let mut parts = re!("/#?").split(target).collect::<Vec<_>>();

    match parts.as_slice() {
        [
            "context" | "styling" | "syntax" | "scripting" | "bundle" | "svg" | "png",
            ..,
        ] => parts.insert(0, "reference"),
        ["category", "math", ..] => {
            parts.remove(0);
        }
        ["category", "symbols", "sym" | "emoji", ..]
        | ["category", "foundations", "calc" | "std" | "sys", ..] => {
            parts.splice(0..2, []);
        }
        ["category", ..] => parts[0] = "reference",
        _ => {}
    }

    let mut dest = parts.join(":");

    if let Some(before) = dest.strip_suffix(":constructor") {
        dest = format!("{before}.constructor");
    }

    if body == format!("`{}`", dest) {
        format!("@{dest}")
    } else {
        format!("@{dest}[{body}]")
    }
}

fn convert_footnote_ref(node: &mdast::FootnoteReference, ctx: &mut Context) -> String {
    let nodes = ctx
        .footnote_defs
        .get(&node.identifier)
        .expect("missing footnote def")
        .clone();
    // Leading space is intentional, since Markdown footnotes are always tightly
    // following the text, but ours don't need that.
    format!(" #footnote[{}]", convert_flow(&nodes, ctx))
}

fn convert_image(node: &mdast::Image, ctx: &mut Context) -> String {
    assert!(!node.url.contains('"'));
    assert!(!node.alt.contains('"'));
    assert!(!node.alt.is_empty());
    ctx.imports.insert("docs-figure");
    format!("#docs-figure(\n  \"{}\",\n  alt: \"{}\",\n)", node.url, node.alt)
}

fn convert_break(_: &mdast::Break, _: &mut Context) -> String {
    "\\\n".into()
}

fn call_blocky(func: &str, body: &str) -> String {
    format!("#{func}[\n{}\n]", indent(body))
}

fn indent(body: &str) -> String {
    let mut out = String::new();
    let mut sep = "";
    for line in body.lines() {
        out.push_str(sep);
        if !line.is_empty() {
            out.push_str("  ");
        }
        out.push_str(line);
        sep = "\n";
    }
    out
}

fn max_consecutive_backticks(text: &str) -> usize {
    let (consecutive, _) = text.chars().fold((0, 0), |(max, current), c| match c {
        '`' => (max.max(current + 1), current + 1),
        _ => (max, 0),
    });
    consecutive
}

fn replace(input: &str, re: &str, rep: impl Replacer) -> String {
    Regex::new(re).unwrap().replace_all(input, rep).into()
}
