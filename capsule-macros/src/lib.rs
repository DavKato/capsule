use proc_macro::TokenStream;
use quote::quote;
use syn::{parse_macro_input, ItemFn};

/// Skip a test silently when no Docker daemon is reachable.
///
/// Equivalent to the manual inline guard:
///   if !common::docker_available() { return; }
///
/// Usage:
///   #[test]
///   #[requires_docker]
///   fn my_test() { ... }
#[proc_macro_attribute]
pub fn requires_docker(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let mut func = parse_macro_input!(item as ItemFn);

    let guard = quote! {
        {
            let _docker_ok = std::process::Command::new("docker")
                .args(["info"])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .map(|s| s.success())
                .unwrap_or(false);
            if !_docker_ok { return; }
        }
    };

    let guard_stmt: syn::Stmt = syn::parse2(guard).expect("guard is valid syntax");
    func.block.stmts.insert(0, guard_stmt);

    quote! { #func }.into()
}
