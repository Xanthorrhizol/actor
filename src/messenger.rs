pub fn send_msg(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let parsed_args = syn::parse_macro_input!(input as SenderParsedArguments);

    let actor_system = parsed_args.actor_system;
    let address = parsed_args.address;
    let message = parsed_args.message;

    let tokens = quote! {
        {
            let actor_system = #actor_system;
            let address = #address;
            let message = #message;

            let mut response_rxs = Vec::new();

            for crate::RegisterForm {
                message_tx,
                ..
            } in crate::ActorSystem::whois(actor_system.map(), address) {
                let (response_tx, response_rx) = tokio::sync::oneshot::channel();

                let _ = message_tx.send(
                    (
                        bincode::serialize(message)
                            .expect("Failed to serialize message"),
                        response_tx,
                    ),
                );
                response_rxs.push(response_rx);
            }
            response_rxs
        }
    };

    tokens.into()
}
pub fn recv_res(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let parsed_args = syn::parse_macro_input!(input as ReceiverParsedArguments);

    let res_type = parsed_args.res_type;
    let response_rxs = parsed_args.response_rxs;

    let tokens = quote! {
        {
            let mut responses = Vec::new();
            let response_rxs = #response_rxs;
            for response_rx in response_rxs {
            let response = response_rx.await.expect("Failed to receive response");
                responses.push(bincode::deserialize::<#res_type>(&response)
                    .expect("Failed to deserialize response"));
            }
            responses
        }
    };

    tokens.into()
}

struct SenderParsedArguments {
    actor_system: syn::Expr,
    address: syn::Expr,
    message: syn::Expr,
}

struct ReceiverParsedArguments {
    res_type: syn::Ident,
    response_rxs: syn::Expr,
}

impl syn::parse::Parse for SenderParsedArguments {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        let actor_system: syn::Expr = input.parse()?;
        input.parse::<syn::Token![,]>()?;
        let address: syn::Expr = input.parse()?;
        input.parse::<syn::Token![,]>()?;
        let message: syn::Expr = input.parse()?;

        Ok(SenderParsedArguments {
            actor_system,
            address,
            message,
        })
    }
}

impl syn::parse::Parse for ReceiverParsedArguments {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        let res_type: syn::Ident = input.parse()?;
        input.parse::<syn::Token![,]>()?;
        let response_rxs: syn::Expr = input.parse()?;

        Ok(ReceiverParsedArguments {
            res_type,
            response_rxs,
        })
    }
}
