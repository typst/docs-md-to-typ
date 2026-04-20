use std::ops::Range;

use syn::spanned::Spanned;

pub struct DocsVisitor(Vec<(Range<usize>, String)>);

impl DocsVisitor {
    pub fn new() -> Self {
        Self(Vec::new())
    }

    pub fn finish(self) -> Vec<(Range<usize>, String)> {
        self.0
    }

    fn process_func(&mut self, attrs: &[syn::Attribute], sig: &syn::Signature) {
        if is_definition(attrs) {
            self.process_docs(attrs);
            for input in &sig.inputs {
                match input {
                    syn::FnArg::Receiver(_) => {}
                    syn::FnArg::Typed(pat_type) => {
                        self.process_docs(&pat_type.attrs);
                    }
                }
            }
        }
    }

    fn process_docs(&mut self, attrs: &[syn::Attribute]) {
        let mut full_range: Option<Range<usize>> = None;
        for attr in attrs {
            let range = attr.span().byte_range();
            if attr.path().is_ident("doc") {
                match full_range.as_mut() {
                    None => full_range = Some(range),
                    Some(prev) => {
                        prev.start = prev.start.min(range.start);
                        prev.end = prev.end.max(range.end);
                    }
                }
            }
        }
        let Some(range) = full_range else { return };
        self.0.push((range, documentation(attrs)));
    }
}

impl<'ast> syn::visit::Visit<'ast> for DocsVisitor {
    fn visit_item_struct(&mut self, i: &'ast syn::ItemStruct) {
        if is_definition(&i.attrs) {
            self.process_docs(&i.attrs);
            for field in &i.fields {
                self.process_docs(&field.attrs);
            }
        }
        syn::visit::visit_item_struct(self, i);
    }

    fn visit_item_enum(&mut self, i: &'ast syn::ItemEnum) {
        if is_definition(&i.attrs) {
            self.process_docs(&i.attrs);
        }
        syn::visit::visit_item_enum(self, i);
    }

    fn visit_item_fn(&mut self, i: &'ast syn::ItemFn) {
        self.process_func(&i.attrs, &i.sig);
        syn::visit::visit_item_fn(self, i);
    }

    fn visit_impl_item_fn(&mut self, i: &'ast syn::ImplItemFn) {
        self.process_func(&i.attrs, &i.sig);
        syn::visit::visit_impl_item_fn(self, i);
    }

    fn visit_item(&mut self, i: &'ast syn::Item) {
        if let syn::Item::Verbatim(verbatim) = i
            && let Ok(i) = syn::parse2::<BareType>(verbatim.clone())
            && is_definition(&i.attrs)
        {
            self.process_docs(&i.attrs);
        }
        syn::visit::visit_item(self, i);
    }
}

fn is_definition(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|attr| {
        let p = attr.path();
        p.is_ident("ty") || p.is_ident("elem") || p.is_ident("func")
    })
}

/// Extract documentation comments from an attribute list.
fn documentation(attrs: &[syn::Attribute]) -> String {
    let mut doc = String::new();

    // Parse doc comments.
    for attr in attrs {
        if let syn::Meta::NameValue(meta) = &attr.meta
            && meta.path.is_ident("doc")
            && let syn::Expr::Lit(lit) = &meta.value
            && let syn::Lit::Str(string) = &lit.lit
        {
            let full = string.value();
            let line = full.strip_prefix(' ').unwrap_or(&full);
            doc.push_str(line);
            doc.push('\n');
        }
    }

    doc.trim().into()
}

/// Parse a bare `type Name;` item.
#[allow(dead_code)]
struct BareType {
    attrs: Vec<syn::Attribute>,
    type_token: syn::Token![type],
    ident: syn::Ident,
    semi_token: syn::Token![;],
}

impl syn::parse::Parse for BareType {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        Ok(Self {
            attrs: input.call(syn::Attribute::parse_outer)?,
            type_token: input.parse()?,
            ident: input.parse()?,
            semi_token: input.parse()?,
        })
    }
}
