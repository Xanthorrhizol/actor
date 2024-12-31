pub fn actor_system(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let types = syn::parse_macro_input!(input as ParsedArguments).types;
    let mut tokens = quote! {};
    types.iter().for_each(|ty| {
        tokens.extend(quote! {
            #[derive(serde::Deserialize)]
            #ty
        })
    });
    tokens.extend(quote! {
        pub type ActorAddress = Vec<String>;

        #[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
        pub enum LifeCycle {
            Created,
            Starting,
            Receiving,
            Stopping,
            Terminated,
            Restarting,
        }

        #[derive(Debug, Clone)]
        pub struct RegisterForm {
            pub address: ActorAddress,
            pub message_tx: tokio::sync::mpsc::UnboundedSender<(Vec<u8>, tokio::sync::oneshot::Sender<Vec<u8>>)>,
            pub kill_tx: tokio::sync::mpsc::UnboundedSender<()>,
            pub restart_tx: tokio::sync::mpsc::UnboundedSender<()>,
            pub life_cycle: LifeCycle,
        }

        #[derive(Debug)]
        pub struct ActorSystem {
            register_tx:
                tokio::sync::broadcast::Sender<RegisterForm>,
            register_rx: tokio::sync::broadcast::Receiver<
                RegisterForm,
            >,
            handle: tokio::task::JoinHandle<()>,
            map: std::sync::Arc<
                std::sync::Mutex<
                    std::collections::HashMap<ActorAddress, RegisterForm>
                >
            >,
        }
        impl Clone for ActorSystem {
            fn clone(&self) -> Self {
                Self::from(self.register_tx.clone(), self.register_rx.resubscribe(), self.map())
            }
        }
        impl ActorSystem { 
            /// create a new actor system
            /// # Returns
            /// * The actor system
            /// * The register tx
            ///
            /// # Example
            /// ```rust
            /// let (actor_system, register_tx) = ActorSystem::new();
            /// ```
            pub fn new() -> (
                Self,
                tokio::sync::broadcast::Sender<RegisterForm>,
            ) {
                let (register_tx, mut register_rx) = tokio::sync::broadcast::channel(1024);
                let map = std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));
                (
                    Self {
                        handle: tokio::spawn(Self::actor_system_loop(map.clone(), register_rx.resubscribe())),
                        register_tx: register_tx.clone(),
                        register_rx,
                        map,
                    },
                    register_tx,
                )
            } 

            /// Kill the actors match with the given address
            /// # Arguments
            /// * `address` - The address of the actors
            pub fn kill(&mut self, address: ActorAddress) {
                for RegisterForm { kill_tx, .. } in Self::whois(&self.map, address) {
                    let _ = kill_tx.send(());
                }
            }

            /// Restart the actors match with the given address
            /// # Arguments
            /// * `address` - The address of the actors
            pub fn restart(&mut self, address: ActorAddress) {
                for RegisterForm { restart_tx, .. } in Self::whois(&self.map, address) {
                    let _ = restart_tx.send(());
                }
            }

            /// Get the life cycle of the actors match with the given address
            /// # Arguments
            /// * `address` - The address of the actors
            ///
            /// # Returns
            /// * The address of the actors
            /// * The life cycle of the actors
            pub fn life_cycle(&self, address: ActorAddress) -> Vec<(ActorAddress, LifeCycle)> {
                let mut v = Vec::new();
                for RegisterForm { address, life_cycle, .. } in Self::whois(&self.map, address) {
                    v.push((address, life_cycle));
                }
                v
            }

            // ================== private ==================

            fn from(
                register_tx: tokio::sync::broadcast::Sender<RegisterForm>,
                register_rx: tokio::sync::broadcast::Receiver<RegisterForm>,
                map: &std::sync::Arc<
                    std::sync::Mutex<
                        std::collections::HashMap<ActorAddress, RegisterForm>
                    >
                >,
            ) -> Self {
                Self {
                    handle: tokio::spawn(Self::actor_system_loop(map.clone(), register_rx.resubscribe())),
                    register_tx,
                    register_rx,
                    map: map.clone(),
                }
            }

            async fn actor_system_loop(
                map: std::sync::Arc<
                    std::sync::Mutex<
                        std::collections::HashMap<ActorAddress, RegisterForm>
                    >
                >,
                mut rx: tokio::sync::broadcast::Receiver<RegisterForm>,
            ) {
                while let Ok(register_form) = rx.recv().await {
                    let addr = register_form.address.clone();
                    let mut map = map.lock().expect("Failed to lock map");
                    match register_form.life_cycle {
                        LifeCycle::Created => {
                            map.insert(addr, register_form);
                        }
                        LifeCycle::Terminated => {
                            map.remove(&addr);
                        }
                        life_cycle => {
                            if let Some(form) = map.get_mut(&addr) {
                                form.life_cycle = life_cycle;
                            }
                        }
                    }
                }
            }

            fn map(&self) -> &std::sync::Arc<
                std::sync::Mutex<
                    std::collections::HashMap<ActorAddress, RegisterForm>,
                >
            > {
                &self.map
            }

            fn whois(
                map: &std::sync::Arc<
                    std::sync::Mutex<
                        std::collections::HashMap<ActorAddress, RegisterForm>,
                    >
                >,
                address: ActorAddress,
            ) -> Vec<RegisterForm> {
                let mut result = Vec::new();
                for (key, value) in map.lock().expect("Failed to lock map").iter() {
                    if compare_actor_address(&address, key) {
                        result.push(value.clone());
                    }
                }
                result
            }
        }

        fn compare_actor_address(criteria: &ActorAddress, target: &ActorAddress) -> bool {
            for i in 0..criteria.len().max(target.len()) {
                if (i >= criteria.len() && criteria[i - 1] != "*")
                    || (criteria[i] != "*" && criteria[i] != target[i])
                {
                    return false;
                }
            }
            true
        }
    });
    tokens.into()
}

struct ParsedArguments {
    types: Vec<syn::Item>,
}

impl syn::parse::Parse for ParsedArguments {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        let mut types = Vec::new();
        while let Ok(expr) = input.parse::<syn::Item>() {
            match expr {
                syn::Item::Enum(e) => {
                    types.push(syn::Item::Enum(e));
                }
                syn::Item::Struct(s) => {
                    types.push(syn::Item::Struct(s));
                }
                syn::Item::Type(t) => {
                    types.push(syn::Item::Type(t));
                }
                _ => {
                    return Err(syn::Error::new_spanned(
                        expr,
                        "Only enum, type alias and struct are allowed",
                    ));
                }
            }
            let _ = input.parse::<syn::Token![,]>();
        }
        Ok(Self { types })
    }
}
