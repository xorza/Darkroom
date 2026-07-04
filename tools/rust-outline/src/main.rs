//! Emit a Markdown structural outline of Rust source: type definitions with
//! their fields (type + visibility) and `impl`/`trait` blocks with method
//! signatures. Parses the real AST via `syn` and renders each fragment through
//! `prettyplease`, so types and signatures match the source exactly.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use ignore::WalkBuilder;
use syn::{
    Fields, ImplItem, Item, ItemImpl, Signature, TraitItem, Type, Variant, Visibility, parse_quote,
};

struct Config {
    input: PathBuf,
    output: Option<PathBuf>,
    include_tests: bool,
}

fn parse_args() -> Result<Config, String> {
    let mut input: Option<PathBuf> = None;
    let mut output: Option<PathBuf> = None;
    let mut include_tests = false;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => return Err(usage()),
            "--tests" => include_tests = true,
            "-o" | "--output" => {
                let v = args.next().ok_or("`-o` needs a path")?;
                output = Some(PathBuf::from(v));
            }
            other if other.starts_with('-') => {
                return Err(format!("unknown flag `{other}`\n\n{}", usage()));
            }
            other => {
                if input.is_some() {
                    return Err(format!(
                        "unexpected extra argument `{other}`\n\n{}",
                        usage()
                    ));
                }
                input = Some(PathBuf::from(other));
            }
        }
    }
    Ok(Config {
        input: input.unwrap_or_else(|| PathBuf::from(".")),
        output,
        include_tests,
    })
}

fn usage() -> String {
    "rust-outline — Markdown outline of Rust types, fields and method signatures\n\n\
     Usage: rust-outline [PATH] [-o OUTPUT.md] [--tests]\n\n\
     PATH       file or directory to scan (default: current dir; respects .gitignore)\n\
     -o OUTPUT  write Markdown to OUTPUT (default: stdout)\n\
     --tests    include `#[cfg(test)]` modules (skipped by default)"
        .to_string()
}

fn main() -> ExitCode {
    let cfg = match parse_args() {
        Ok(c) => c,
        Err(msg) => {
            eprintln!("{msg}");
            return ExitCode::FAILURE;
        }
    };

    let files = match collect_rs_files(&cfg.input) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("error scanning `{}`: {e}", cfg.input.display());
            return ExitCode::FAILURE;
        }
    };

    let mut md = String::new();
    let _ = writeln!(md, "# Code outline\n");
    let _ = writeln!(md, "`{}` — {} file(s)\n", cfg.input.display(), files.len());

    for path in &files {
        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("skip {}: {e}", path.display());
                continue;
            }
        };
        let ast = match syn::parse_file(&content) {
            Ok(a) => a,
            Err(e) => {
                eprintln!("skip {} (parse error): {e}", path.display());
                continue;
            }
        };
        let mut body = String::new();
        walk_items(&ast.items, "", cfg.include_tests, &mut body);
        if !body.trim().is_empty() {
            let _ = writeln!(md, "## `{}`\n", display_path(&cfg.input, path));
            md.push_str(&body);
        }
    }

    match &cfg.output {
        Some(out) => {
            if let Err(e) = std::fs::write(out, &md) {
                eprintln!("error writing `{}`: {e}", out.display());
                return ExitCode::FAILURE;
            }
            eprintln!("wrote {} ({} bytes)", out.display(), md.len());
        }
        None => print!("{md}"),
    }
    ExitCode::SUCCESS
}

fn collect_rs_files(root: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    // `ignore` respects .gitignore and skips hidden dirs (.git, .tmp) by default,
    // so vendored build output and scratch dirs stay out of the outline.
    for result in WalkBuilder::new(root).build() {
        let entry = result.map_err(|e| std::io::Error::other(e.to_string()))?;
        let path = entry.path();
        if path.extension().is_some_and(|e| e == "rs") && path.is_file() {
            files.push(path.to_path_buf());
        }
    }
    files.sort();
    Ok(files)
}

fn display_path(root: &Path, path: &Path) -> String {
    // When `root` is a single file, strip against its parent so the heading
    // shows the file name rather than an empty string.
    let base = if root.is_file() {
        root.parent().unwrap_or(root)
    } else {
        root
    };
    path.strip_prefix(base)
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned()
}

fn walk_items(items: &[Item], mod_path: &str, include_tests: bool, out: &mut String) {
    let mut free_fns: Vec<String> = Vec::new();

    for item in items {
        match item {
            Item::Struct(s) => {
                emit_heading(out, &header_of(item), mod_path);
                emit_fields(out, &s.fields);
            }
            Item::Union(u) => {
                emit_heading(out, &header_of(item), mod_path);
                emit_fields(out, &Fields::Named(u.fields.clone()));
            }
            Item::Enum(e) => {
                emit_heading(out, &header_of(item), mod_path);
                for v in &e.variants {
                    let _ = writeln!(out, "- `{}`", render_variant(v));
                }
                out.push('\n');
            }
            Item::Trait(t) => {
                emit_heading(out, &header_of(item), mod_path);
                for ti in &t.items {
                    if let Some(line) = render_trait_item(ti) {
                        let _ = writeln!(out, "- `{line}`");
                    }
                }
                out.push('\n');
            }
            Item::Impl(im) => {
                emit_heading(out, &impl_header(im), mod_path);
                for ii in &im.items {
                    if let Some(line) = render_impl_item(ii) {
                        let _ = writeln!(out, "- `{line}`");
                    }
                }
                out.push('\n');
            }
            Item::Type(_) => {
                emit_heading(out, &header_of(item), mod_path);
            }
            Item::Fn(f) => {
                free_fns.push(render_sig(&f.vis, &f.sig));
            }
            Item::Mod(m) => {
                if let Some((_, sub)) = &m.content {
                    if !include_tests && is_cfg_test(&m.attrs) {
                        continue;
                    }
                    let child = qualify(mod_path, &m.ident.to_string());
                    walk_items(sub, &child, include_tests, out);
                }
            }
            _ => {}
        }
    }

    if !free_fns.is_empty() {
        let scope = if mod_path.is_empty() {
            String::new()
        } else {
            format!(" _(in {mod_path})_")
        };
        let _ = writeln!(out, "### Free functions{scope}\n");
        for sig in &free_fns {
            let _ = writeln!(out, "- `{sig}`");
        }
        out.push('\n');
    }
}

fn emit_heading(out: &mut String, header: &str, mod_path: &str) {
    if mod_path.is_empty() {
        let _ = writeln!(out, "### `{header}`\n");
    } else {
        let _ = writeln!(out, "### `{header}` _(in {mod_path})_\n");
    }
}

fn emit_fields(out: &mut String, fields: &Fields) {
    match fields {
        Fields::Named(named) => {
            for f in &named.named {
                let name = f.ident.as_ref().map(|i| i.to_string()).unwrap_or_default();
                let _ = writeln!(
                    out,
                    "- `{}{name}: {}`",
                    vis_prefix(&f.vis),
                    render_type(&f.ty)
                );
            }
        }
        Fields::Unnamed(unnamed) => {
            for (i, f) in unnamed.unnamed.iter().enumerate() {
                let _ = writeln!(out, "- `{}{i}: {}`", vis_prefix(&f.vis), render_type(&f.ty));
            }
        }
        Fields::Unit => {}
    }
    out.push('\n');
}

fn is_cfg_test(attrs: &[syn::Attribute]) -> bool {
    attrs
        .iter()
        .any(|a| a.path().is_ident("cfg") && quote::quote!(#a).to_string().contains("test"))
}

fn qualify(mod_path: &str, name: &str) -> String {
    if mod_path.is_empty() {
        name.to_string()
    } else {
        format!("{mod_path}::{name}")
    }
}

fn render_trait_item(ti: &TraitItem) -> Option<String> {
    match ti {
        TraitItem::Fn(f) => Some(render_sig(&Visibility::Inherited, &f.sig)),
        TraitItem::Const(c) => Some(format!("const {}: {}", c.ident, render_type(&c.ty))),
        TraitItem::Type(t) => Some(format!("type {}", t.ident)),
        _ => None,
    }
}

fn render_impl_item(ii: &ImplItem) -> Option<String> {
    match ii {
        ImplItem::Fn(f) => Some(render_sig(&f.vis, &f.sig)),
        ImplItem::Const(c) => Some(format!(
            "{}const {}: {}",
            vis_prefix(&c.vis),
            c.ident,
            render_type(&c.ty)
        )),
        ImplItem::Type(t) => Some(format!(
            "{}type {} = {}",
            vis_prefix(&t.vis),
            t.ident,
            render_type(&t.ty)
        )),
        _ => None,
    }
}

fn render_variant(v: &Variant) -> String {
    match &v.fields {
        Fields::Unit => v.ident.to_string(),
        Fields::Unnamed(u) => {
            let inner: Vec<String> = u.unnamed.iter().map(|f| render_type(&f.ty)).collect();
            format!("{}({})", v.ident, inner.join(", "))
        }
        Fields::Named(n) => {
            let inner: Vec<String> = n
                .named
                .iter()
                .map(|f| {
                    format!(
                        "{}{}: {}",
                        vis_prefix(&f.vis),
                        f.ident.as_ref().map(|i| i.to_string()).unwrap_or_default(),
                        render_type(&f.ty)
                    )
                })
                .collect();
            format!("{} {{ {} }}", v.ident, inner.join(", "))
        }
    }
}

/// The declaration line of an item (up to its body), attributes stripped.
fn header_of(item: &Item) -> String {
    let mut item = item.clone();
    strip_attrs(&mut item);
    let full = pretty_item(item);
    let cut = full
        .find('{')
        .or_else(|| full.find(';'))
        .unwrap_or(full.len());
    tighten(&collapse(&full[..cut]))
}

fn impl_header(im: &ItemImpl) -> String {
    let mut empty = im.clone();
    empty.attrs.clear();
    empty.items.clear();
    let full = pretty_item(Item::Impl(empty));
    let cut = full.find('{').unwrap_or(full.len());
    tighten(&collapse(&full[..cut]))
}

fn strip_attrs(item: &mut Item) {
    match item {
        Item::Struct(s) => s.attrs.clear(),
        Item::Enum(e) => e.attrs.clear(),
        Item::Union(u) => u.attrs.clear(),
        Item::Trait(t) => t.attrs.clear(),
        Item::Type(t) => t.attrs.clear(),
        Item::Impl(i) => i.attrs.clear(),
        _ => {}
    }
}

/// Render a function signature (no body) via a synthetic `fn … {}`.
fn render_sig(vis: &Visibility, sig: &Signature) -> String {
    let item = syn::ItemFn {
        attrs: vec![],
        vis: vis.clone(),
        sig: sig.clone(),
        block: Box::new(parse_quote!({})),
    };
    let full = tighten(&collapse(&pretty_item(Item::Fn(item))));
    full.trim_end_matches("{}").trim_end().to_string()
}

/// Render a type via a synthetic `type _ = <ty>;`, so nested generics/lifetimes
/// come back exactly as `prettyplease` would format them.
fn render_type(ty: &Type) -> String {
    let item: syn::ItemType = parse_quote! { type Ty = #ty; };
    let full = tighten(&collapse(&pretty_item(Item::Type(item))));
    full.trim_start_matches("type Ty =")
        .trim_end_matches(';')
        .trim()
        .to_string()
}

fn pretty_item(item: Item) -> String {
    let file = syn::File {
        shebang: None,
        attrs: vec![],
        items: vec![item],
    };
    prettyplease::unparse(&file)
}

fn collapse(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Remove the space artifacts left when `prettyplease` line-wraps a long item
/// and `collapse` rejoins it: `( &mut self, … , )` → `(&mut self, …)`.
fn tighten(s: &str) -> String {
    s.replace(", )", ")").replace("( ", "(").replace(" )", ")")
}

fn vis_prefix(vis: &Visibility) -> String {
    if let Visibility::Inherited = vis {
        return String::new();
    }
    // `quote` emits token-spaced visibility (`pub (crate)`, `pub (in a :: b)`);
    // tighten the spaces around `(` and `::`.
    let s = collapse(&quote::quote!(#vis).to_string())
        .replace(" ::", "::")
        .replace(":: ", "::")
        .replace(" (", "(");
    format!("{s} ")
}
