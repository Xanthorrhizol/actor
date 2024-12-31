pub fn actor(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let parsed_args = syn::parse_macro_input!(input as ParsedArguments);

    let actor_struct_name = parsed_args.actor_struct_name;
    let message_type = parsed_args.message_type;
    let resource_struct = parsed_args.resource_struct;
    let handle_fn = parsed_args.handle_fn;
    let pre_start_fn = parsed_args.pre_start_fn;
    let post_stop_fn = parsed_args.post_stop_fn;
    let pre_restart_fn = parsed_args.pre_restart_fn;
    let post_restart_fn = parsed_args.post_restart_fn;
    let kill_in_error = parsed_args.kill_in_error;
    let resource_struct_name = resource_struct.clone().ident;
    let handle_fn_name = handle_fn.clone().sig.ident;

    let tokens = quote! {
        #resource_struct

        pub struct #actor_struct_name {
            actor_ref: crate::ActorAddress,
            kill_in_error: bool,
            resource_struct: #resource_struct_name,
            message_tx: tokio::sync::mpsc::UnboundedSender<(Vec<u8>, tokio::sync::oneshot::Sender<Vec<u8>>)>,
            message_rx: tokio::sync::mpsc::UnboundedReceiver<(Vec<u8>, tokio::sync::oneshot::Sender<Vec<u8>>)>,
            kill_tx: tokio::sync::mpsc::UnboundedSender<()>,
            kill_rx: tokio::sync::mpsc::UnboundedReceiver<()>,
            restart_tx: tokio::sync::mpsc::UnboundedSender<()>,
            restart_rx: tokio::sync::mpsc::UnboundedReceiver<()>,
            register_tx: tokio::sync::broadcast::Sender<crate::RegisterForm>,
            life_cycle: LifeCycle,
        }
        impl #actor_struct_name {
            /// Create a new actor
            /// # Arguments
            /// * `actor_ref` - The address of the actor
            /// * `resource_struct` - The resource struct that the actor will use
            /// * `register_tx` - The sender to register the actor
            ///
            /// # Returns
            /// * The actor
            pub fn new(
                actor_ref: crate::ActorAddress,
                resource_struct: #resource_struct_name,
                register_tx: tokio::sync::broadcast::Sender<crate::RegisterForm>,
            ) -> Self {
                let (message_tx, message_rx) = tokio::sync::mpsc::unbounded_channel();
                let (kill_tx, kill_rx) = tokio::sync::mpsc::unbounded_channel();
                let (restart_tx, restart_rx) = tokio::sync::mpsc::unbounded_channel();
                let life_cycle = LifeCycle::Created;
                register_tx.send(
                    crate::RegisterForm {
                        address: actor_ref.clone(),
                        message_tx: message_tx.clone(),
                        kill_tx: kill_tx.clone(),
                        restart_tx: restart_tx.clone(),
                        life_cycle,
                    },
                ).expect("Failed to send whois response");
                Self {
                    actor_ref,
                    kill_in_error: #kill_in_error,
                    resource_struct,
                    message_tx,
                    message_rx,
                    kill_tx,
                    kill_rx,
                    restart_tx,
                    restart_rx,
                    register_tx,
                    life_cycle,
                }
            }

            /// Run the actor
            /// # Returns
            /// * A handle to the actor task
            /// * A receiver that will be signalled when the actor is ready
            pub fn run(
                mut self,
            ) -> (tokio::task::JoinHandle<()>, tokio::sync::oneshot::Receiver<()>) {
                let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
                let handle = tokio::spawn(async move {
                    let mut first = true;
                    self.life_cycle = LifeCycle::Starting;
                    self.pre_start();
                    self.life_cycle = LifeCycle::Receiving;
                    self.register_tx.send(
                        crate::RegisterForm {
                            address: self.actor_ref.clone(),
                            message_tx: self.message_tx.clone(),
                            kill_tx: self.kill_tx.clone(),
                            restart_tx: self.restart_tx.clone(),
                            life_cycle: self.life_cycle,
                        },
                    ).expect("Failed to send whois response");
                    ready_tx.send(()).expect("Failed to send ready signal");
                    loop {
                        if first {
                            first = false;
                        } else {
                            self.post_restart();
                            self.life_cycle = LifeCycle::Receiving;
                        }
                        self.register_tx.send(
                            crate::RegisterForm {
                                address: self.actor_ref.clone(),
                                message_tx: self.message_tx.clone(),
                                kill_tx: self.kill_tx.clone(),
                                restart_tx: self.restart_tx.clone(),
                                life_cycle: self.life_cycle,
                            },
                        ).expect("Failed to send register request");
                        let kill = loop {
                            tokio::select! {
                                kill_signal = self.kill_rx.recv() => {
                                    if let Some(_) = kill_signal {
                                        log::info!("Killing actor {:?}", self.actor_ref);
                                        break true;
                                    }
                                }
                                restart_signal = self.restart_rx.recv() => {
                                    if let Some(_) = restart_signal {
                                        log::info!("Restarting actor {:?}", self.actor_ref);
                                        break false;
                                    }
                                }
                                message = self.message_rx.recv() => {
                                    if let Some((message, response_tx)) = message {
                                        let response = self.#handle_fn_name(
                                            bincode::deserialize::<#message_type>(&message).expect("Failed to deserialize message"),
                                        );
                                        #[allow(unused_must_use)]
                                        response_tx.send(bincode::serialize(&response).expect("Failed to serialize response"));
                                    } else {
                                        log::error!("The sender for {:?} has been dropped", self.actor_ref);
                                        break true;
                                    }
                                }
                            }
                        };
                        if kill {
                            self.life_cycle = LifeCycle::Stopping;
                            self.register_tx.send(
                                crate::RegisterForm {
                                    address: self.actor_ref.clone(),
                                    message_tx: self.message_tx.clone(),
                                    kill_tx: self.kill_tx.clone(),
                                    restart_tx: self.restart_tx.clone(),
                                    life_cycle: self.life_cycle,
                                },
                            ).expect("Failed to send register request");
                            self.post_stop();
                            self.life_cycle = LifeCycle::Terminated;
                            self.register_tx.send(
                                crate::RegisterForm {
                                    address: self.actor_ref.clone(),
                                    message_tx: self.message_tx.clone(),
                                    kill_tx: self.kill_tx.clone(),
                                    restart_tx: self.restart_tx.clone(),
                                    life_cycle: self.life_cycle,
                                },
                            ).expect("Failed to send register request");
                            break;
                        }
                        self.pre_restart();
                        self.life_cycle = LifeCycle::Restarting;
                        self.register_tx.send(
                            crate::RegisterForm {
                                address: self.actor_ref.clone(),
                                message_tx: self.message_tx.clone(),
                                kill_tx: self.kill_tx.clone(),
                                restart_tx: self.restart_tx.clone(),
                                life_cycle: self.life_cycle,
                            },
                        ).expect("Failed to send register request");
                    }
                });
                (handle, ready_rx)
            }

            fn address(&self) -> &crate::ActorAddress {
                &self.actor_ref
            }

            fn resource(&mut self) -> &mut #resource_struct_name {
                &mut self.resource_struct
            }

            #handle_fn

            #pre_start_fn

            #post_stop_fn

            #pre_restart_fn

            #post_restart_fn
        }
    };
    tokens.into()
}

struct ParsedArguments {
    actor_struct_name: syn::Ident,
    message_type: syn::Ident,
    resource_struct: syn::ItemStruct,
    handle_fn: syn::ItemFn,
    pre_start_fn: syn::ItemFn,
    post_stop_fn: syn::ItemFn,
    pre_restart_fn: syn::ItemFn,
    post_restart_fn: syn::ItemFn,
    kill_in_error: syn::LitBool,
}

impl syn::parse::Parse for ParsedArguments {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        let actor_struct_name: syn::Ident = input.parse()?;
        input.parse::<syn::Token![,]>()?;
        let message_type: syn::Ident = input.parse()?;
        input.parse::<syn::Token![,]>()?;
        let resource_struct: syn::ItemStruct = input.parse()?;
        input.parse::<syn::Token![,]>()?;
        let handle_fn: syn::ItemFn = input.parse()?;
        input.parse::<syn::Token![,]>()?;
        let pre_start_fn: syn::ItemFn = input.parse()?;
        input.parse::<syn::Token![,]>()?;
        let post_stop_fn: syn::ItemFn = input.parse()?;
        input.parse::<syn::Token![,]>()?;
        let pre_restart_fn: syn::ItemFn = input.parse()?;
        input.parse::<syn::Token![,]>()?;
        let post_restart_fn: syn::ItemFn = input.parse()?;
        input.parse::<syn::Token![,]>()?;
        let kill_in_error: syn::LitBool = input.parse()?;

        Ok(ParsedArguments {
            actor_struct_name,
            message_type,
            resource_struct,
            handle_fn,
            pre_start_fn,
            post_stop_fn,
            pre_restart_fn,
            post_restart_fn,
            kill_in_error,
        })
    }
}
