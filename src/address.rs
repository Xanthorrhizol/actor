pub fn address(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let parsed_args = syn::parse_macro_input!(input as ParsedArguments).v;
    let tokens = quote! {
        vec![#(#parsed_args),*]
    };
    tokens.into()
}

struct ParsedArguments {
    v: Vec<syn::Expr>,
}

impl syn::parse::Parse for ParsedArguments {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        let mut v = Vec::new();
        while let Ok(expr) = input.parse::<syn::Expr>() {
            v.push(expr);
            let _ = input.parse::<syn::Token![,]>();
        }
        Ok(Self { v })
    }
}
