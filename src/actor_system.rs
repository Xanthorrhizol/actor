pub fn actor_system(_input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let token = quote! {
        #[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
        pub enum LifeCycle {
            Created,
            Starting,
            Receiving,
            Stopping,
            Terminated,
            Restarting,
        }

        pub struct WhoisResponse(
            pub tokio::sync::mpsc::UnboundedSender<(Vec<u8>, tokio::sync::oneshot::Sender<Vec<u8>>)>,
            pub tokio::sync::mpsc::UnboundedSender<()>,
            pub tokio::sync::mpsc::UnboundedSender<()>,
            pub LifeCycle,
        );

        #[derive(Debug)]
        pub struct ActorSystem {
            whois_channel_tx:
                tokio::sync::broadcast::Sender<(String, tokio::sync::mpsc::UnboundedSender<WhoisResponse>)>,
            whois_channel_rx: tokio::sync::broadcast::Receiver<(
                String,
                tokio::sync::mpsc::UnboundedSender<WhoisResponse>,
            )>,
        }
        impl Clone for ActorSystem {
            fn clone(&self) -> Self {
                Self {
                    whois_channel_tx: self.whois_channel_tx.clone(),
                    whois_channel_rx: self.whois_channel_rx.resubscribe(),
                }
            }
        }
        impl ActorSystem {
            pub fn new() -> (
                tokio::sync::broadcast::Receiver<(
                    String,
                    tokio::sync::mpsc::UnboundedSender<WhoisResponse>,
                )>,
                Self,
            ) {
                let (whois_channel_tx, mut whois_channel_rx) = tokio::sync::broadcast::channel(1024);
                (
                    whois_channel_rx.resubscribe(),
                    Self {
                        whois_channel_tx,
                        whois_channel_rx,
                    },
                )
            }
            pub async fn kill(&mut self, address: String) {
                let WhoisResponse(_, kill_tx, _, _) = self.whois(address).await;
                let _ = kill_tx.send(());
            }
            pub async fn restart(&mut self, address: String) {
                let WhoisResponse(_, _, restart_tx, _) = self.whois(address).await;
                let _ = restart_tx.send(());
            }
            async fn whois(&mut self, address: String) -> WhoisResponse {
                let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
                let _ = self.whois_channel_tx.send((address, tx));
                rx.recv().await.expect("Failed to receive whois response")
            }
        }
    };
    token.into()
}
