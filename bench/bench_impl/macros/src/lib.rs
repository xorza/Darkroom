//! Proc-macros for common crate.

use proc_macro::TokenStream;
use quote::quote;
use syn::{FnArg, ItemFn, LitInt, Pat, Token, parse::Parse, parse::ParseStream, parse_macro_input};

/// Arguments for the quick_bench attribute.
struct QuickBenchArgs {
    iterations: Option<usize>,
}

impl Parse for QuickBenchArgs {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        if input.is_empty() {
            return Ok(QuickBenchArgs { iterations: None });
        }

        let mut iterations = None;

        while !input.is_empty() {
            let ident: syn::Ident = input.parse()?;
            input.parse::<Token![=]>()?;

            if ident == "iterations" {
                let lit: LitInt = input.parse()?;
                iterations = Some(lit.base10_parse()?);
            }

            if input.peek(Token![,]) {
                input.parse::<Token![,]>()?;
            }
        }

        Ok(QuickBenchArgs { iterations })
    }
}

/// Attribute macro for creating benchmark tests.
///
/// # Usage
///
/// ```ignore
/// use common::quick_bench;
/// use common::bench::Bencher;
///
/// #[quick_bench]
/// fn bench_something(b: Bencher) {
///     b.bench(|| {
///         // code to benchmark
///     });
/// }
///
/// #[quick_bench(iterations = 20)]
/// fn bench_with_iterations(b: Bencher) {
///     b.bench(|| {
///         // code to benchmark
///     });
/// }
/// ```
#[proc_macro_attribute]
pub fn quick_bench(attr: TokenStream, item: TokenStream) -> TokenStream {
    let args = parse_macro_input!(attr as QuickBenchArgs);
    let input = parse_macro_input!(item as ItemFn);

    let fn_name = &input.sig.ident;
    let fn_name_str = fn_name.to_string();
    let fn_body = &input.block;
    let fn_vis = &input.vis;

    // Check that function has exactly one parameter
    if input.sig.inputs.len() != 1 {
        return syn::Error::new_spanned(
            &input.sig.inputs,
            "quick_bench function must have exactly one parameter: `b: Bencher`",
        )
        .to_compile_error()
        .into();
    }

    // Extract the parameter name
    let param_name = match &input.sig.inputs[0] {
        FnArg::Typed(pat_type) => match &*pat_type.pat {
            Pat::Ident(pat_ident) => &pat_ident.ident,
            _ => {
                return syn::Error::new_spanned(
                    &pat_type.pat,
                    "parameter must be a simple identifier",
                )
                .to_compile_error()
                .into();
            }
        },
        FnArg::Receiver(_) => {
            return syn::Error::new_spanned(
                &input.sig.inputs[0],
                "quick_bench function cannot have self parameter",
            )
            .to_compile_error()
            .into();
        }
    };

    let iterations = args.iterations.unwrap_or(10);

    let expanded = quote! {
        #[test]
        #[ignore]
        #fn_vis fn #fn_name() {
            let #param_name = ::bench::Bencher::new(#fn_name_str)
                .with_iterations(#iterations)
                .with_output_dir(env!("CARGO_MANIFEST_DIR"));
            #fn_body
        }
    };

    expanded.into()
}
