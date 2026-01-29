//! Proc-macros for common crate.

use proc_macro::TokenStream;
use quote::quote;
use syn::{FnArg, ItemFn, LitInt, Pat, Token, parse::Parse, parse::ParseStream, parse_macro_input};

/// Arguments for the quick_bench attribute.
struct QuickBenchArgs {
    warmup_time_ms: Option<u64>,
    bench_time_ms: Option<u64>,
    warmup_iters: Option<u64>,
    iters: Option<u64>,
    ignore: bool,
}

impl Parse for QuickBenchArgs {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        if input.is_empty() {
            return Ok(QuickBenchArgs {
                warmup_time_ms: None,
                bench_time_ms: None,
                warmup_iters: None,
                iters: None,
                ignore: true,
            });
        }

        let mut warmup_time_ms = None;
        let mut bench_time_ms = None;
        let mut warmup_iters = None;
        let mut iters = None;
        let mut ignore = true;

        while !input.is_empty() {
            let ident: syn::Ident = input.parse()?;
            input.parse::<Token![=]>()?;

            if ident == "warmup_time_ms" {
                let lit: LitInt = input.parse()?;
                warmup_time_ms = Some(lit.base10_parse()?);
            } else if ident == "bench_time_ms" {
                let lit: LitInt = input.parse()?;
                bench_time_ms = Some(lit.base10_parse()?);
            } else if ident == "warmup_iters" {
                let lit: LitInt = input.parse()?;
                warmup_iters = Some(lit.base10_parse()?);
            } else if ident == "iters" {
                let lit: LitInt = input.parse()?;
                iters = Some(lit.base10_parse()?);
            } else if ident == "ignore" {
                let lit: syn::LitBool = input.parse()?;
                ignore = lit.value();
            }

            if input.peek(Token![,]) {
                input.parse::<Token![,]>()?;
            }
        }

        Ok(QuickBenchArgs {
            warmup_time_ms,
            bench_time_ms,
            warmup_iters,
            iters,
            ignore,
        })
    }
}

/// Attribute macro for creating benchmark tests.
///
/// # Usage
///
/// ```ignore
/// use bench::quick_bench;
/// use bench::Bencher;
///
/// #[quick_bench]
/// fn bench_something(b: Bencher) {
///     b.bench(|| {
///         // code to benchmark
///     });
/// }
///
/// // Time-based: runs for specified duration
/// #[quick_bench(warmup_time_ms = 500, bench_time_ms = 2000)]
/// fn bench_time_based(b: Bencher) {
///     b.bench(|| {
///         // code to benchmark
///     });
/// }
///
/// // Iteration-based: runs exact number of iterations
/// #[quick_bench(warmup_iters = 100, iters = 1000)]
/// fn bench_iteration_based(b: Bencher) {
///     b.bench(|| {
///         // code to benchmark
///     });
/// }
///
/// // Combined: stops at whichever limit is reached first
/// #[quick_bench(warmup_time_ms = 500, warmup_iters = 100, bench_time_ms = 2000, iters = 500)]
/// fn bench_combined(b: Bencher) {
///     b.bench(|| {
///         // code to benchmark
///     });
/// }
///
/// // Run as a normal test (not ignored)
/// #[quick_bench(ignore = false)]
/// fn bench_not_ignored(b: Bencher) {
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

    let ignore_attr = if args.ignore {
        quote! { #[ignore] }
    } else {
        quote! {}
    };

    // When only iters are specified, disable time limits
    let disable_warmup_time = if args.warmup_iters.is_some() && args.warmup_time_ms.is_none() {
        quote! { .without_warmup_time() }
    } else {
        quote! {}
    };

    let disable_bench_time = if args.iters.is_some() && args.bench_time_ms.is_none() {
        quote! { .without_bench_time() }
    } else {
        quote! {}
    };

    let warmup_time_call = args.warmup_time_ms.map(|ms| {
        quote! { .with_warmup_time_ms(#ms) }
    });

    let bench_time_call = args.bench_time_ms.map(|ms| {
        quote! { .with_bench_time_ms(#ms) }
    });

    let warmup_iters_call = args.warmup_iters.map(|iters| {
        quote! { .with_warmup_iters(#iters) }
    });

    let iters_call = args.iters.map(|iters| {
        quote! { .with_iters(#iters) }
    });

    let expanded = quote! {
        #[test]
        #ignore_attr
        #fn_vis fn #fn_name() {
            let #param_name = ::bench_impl::Bencher::new(#fn_name_str)
                #disable_warmup_time
                #disable_bench_time
                #warmup_time_call
                #bench_time_call
                #warmup_iters_call
                #iters_call
                .with_output_dir(env!("CARGO_MANIFEST_DIR"));
            #fn_body
        }
    };

    expanded.into()
}
