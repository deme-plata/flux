// flux-api-macros — the `#[api]` attribute macro (v0.13-A).
//
// Applied to a handler `fn`, it leaves the function body alone and additionally
// expands to an `inventory::submit!` of `::flux_api::ApiEndpointDescriptor`,
// which `flux-api` collects at runtime via `inventory::iter`.
//
// Usage:
//
//   #[flux_api_macros::api(GET, "/v1/workspace")]
//   pub async fn workspace_handler(...) -> ... { ... }
//
//   #[flux_api_macros::api(POST, "/v1/webhook", summary = "Receive a webhook")]
//   pub async fn webhook_handler(...) -> ... { ... }
//
// Expansion (conceptual):
//
//   pub async fn workspace_handler(...) -> ... { ... }
//   ::flux_api::__inventory_submit_workspace_handler!();
//
// The actual emission uses `::inventory::submit!`, gated on the consumer crate
// having both `flux-api` (provides the descriptor type + collector) and
// `inventory` in its dep graph. The descriptor type is wired in v0.13-B.

use proc_macro::TokenStream;
use proc_macro2::Span;
use quote::quote;
use syn::{
    parse::{Parse, ParseStream},
    parse_macro_input, Error, Ident, ItemFn, LitStr, Result, Token,
};

/// Parsed form of `#[api(METHOD, "path", summary = "...")]`.
struct ApiAttrArgs {
    method: Ident,
    path: LitStr,
    summary: Option<LitStr>,
}

impl Parse for ApiAttrArgs {
    fn parse(input: ParseStream) -> Result<Self> {
        let method: Ident = input.parse()?;
        let _: Token![,] = input.parse()?;
        let path: LitStr = input.parse()?;
        let mut summary = None;
        while !input.is_empty() {
            let _: Token![,] = input.parse()?;
            if input.is_empty() {
                break; // trailing comma
            }
            let key: Ident = input.parse()?;
            let _: Token![=] = input.parse()?;
            if key == "summary" {
                summary = Some(input.parse()?);
            } else {
                return Err(Error::new(
                    key.span(),
                    format!("unknown key `{key}` (expected `summary`)"),
                ));
            }
        }
        Ok(Self { method, path, summary })
    }
}

/// `#[api(METHOD, "path"[, summary = "..."])]` — register a handler fn as an
/// API endpoint at compile time.
#[proc_macro_attribute]
pub fn api(attr: TokenStream, item: TokenStream) -> TokenStream {
    let args = parse_macro_input!(attr as ApiAttrArgs);
    let func = parse_macro_input!(item as ItemFn);

    let allowed_methods = ["GET", "POST", "PUT", "DELETE", "PATCH"];
    let method_str = args.method.to_string();
    if !allowed_methods.contains(&method_str.as_str()) {
        return Error::new(
            args.method.span(),
            format!(
                "method `{method_str}` is not supported; expected one of {}",
                allowed_methods.join(", ")
            ),
        )
        .to_compile_error()
        .into();
    }

    let fn_name = func.sig.ident.clone();
    let operation_id = fn_name.to_string();
    let summary_str = args
        .summary
        .map(|s| s.value())
        .unwrap_or_else(|| operation_id.clone());
    let path_str = args.path.value();

    // A submission ident unique-per-fn so two #[api]s in the same scope don't
    // collide on the implicit `__submit_<name>` static inventory makes.
    let submit_marker = Ident::new(
        &format!("__FLUX_API_SUBMIT_{}", fn_name.to_string().to_uppercase()),
        Span::call_site(),
    );

    let expanded = quote! {
        #func

        #[doc(hidden)]
        #[allow(non_upper_case_globals)]
        const #submit_marker: () = {
            ::inventory::submit! {
                ::flux_api::ApiEndpointDescriptor {
                    method: #method_str,
                    path: #path_str,
                    summary: #summary_str,
                    operation_id: #operation_id,
                    crate_name: ::core::env!("CARGO_PKG_NAME"),
                }
            }
        };
    };

    expanded.into()
}
