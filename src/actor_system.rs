use crate::{Actor, ActorError, JobSpec, LifeCycle, Message};
use std::collections::{HashMap, HashSet};

/// Commands for the ActorSystem to handle various operations
/// You can send these commands to the ActorSystem's handler channel directly.
pub enum ActorSystemCmd {
    Register(
        String,
        tokio::sync::mpsc::UnboundedSender<Message>,
        tokio::sync::mpsc::UnboundedSender<()>,
        tokio::sync::mpsc::UnboundedSender<()>,
        LifeCycle,
        tokio::sync::oneshot::Sender<Result<(), ActorError>>,
        bool,
    ),
    Restart(String),
    Unregister(String),
    FilterAddress(String, tokio::sync::oneshot::Sender<Vec<String>>),
    FindActor(
        String,
        tokio::sync::oneshot::Sender<
            Option<(tokio::sync::mpsc::UnboundedSender<Message>, bool)>, // tx, ready
        >,
    ),
    SetLifeCycle(String, LifeCycle),
}

#[derive(Clone)]
/// The ActorSystem is the main entry point for managing actors.
/// It contains a handler channel to send commands to the actor system.
/// It's clonable so that it can be shared across different parts of the application.
pub struct ActorSystem {
    handler_tx: tokio::sync::mpsc::UnboundedSender<ActorSystemCmd>,
    pub blocking: bool,
}

impl Default for ActorSystem {
    fn default() -> Self {
        let (handler_tx, handler_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut me = Self {
            handler_tx,
            blocking: true,
        };
        me.run(handler_rx);
        me
    }
}

impl ActorSystem {
    /// Creates a new ActorSystem instance
    pub fn new(blocking: bool) -> Self {
        let (handler_tx, handler_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut me = Self {
            handler_tx,
            blocking,
        };
        me.run(handler_rx);
        me
    }

    /// Returns the handler channel sender for the ActorSystem.
    /// You can use this to send commands to the ActorSystem directly.
    pub fn handler_tx(&self) -> tokio::sync::mpsc::UnboundedSender<ActorSystemCmd> {
        self.handler_tx.clone()
    }

    /// Filters the addresses of actors based on a regex pattern.
    pub async fn filter_address(&mut self, address_regex: String) -> Vec<String> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let _ = self
            .handler_tx
            .send(ActorSystemCmd::FilterAddress(address_regex, tx));
        match rx.await {
            Ok(addresses) => addresses,
            Err(e) => {
                error!("Receive address list failed: {:?}", e);
                Vec::new()
            }
        }
    }

    /// Registers an actor.
    pub fn restart(&mut self, address_regex: String) {
        let _ = self.handler_tx.send(ActorSystemCmd::Restart(address_regex));
    }

    /// Registers an actor.
    pub fn unregister(&mut self, address_regex: String) {
        let _ = self
            .handler_tx
            .send(ActorSystemCmd::Unregister(address_regex));
    }

    /// Send a message to a specific actor by its address.
    /// It doesn't wait for the actor to be ready.
    pub async fn send<T>(
        &self,
        address: String,
        msg: <T as Actor>::Message,
    ) -> Result<(), ActorError>
    where
        T: Actor,
    {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let _ = self
            .handler_tx
            .send(ActorSystemCmd::FindActor(address.clone(), tx));
        if let Ok(Some((tx, ready))) = rx.await {
            if ready {
                let _ = tx.send(Message::new(rmp_serde::to_vec(&msg)?, None))?;
                Ok(())
            } else {
                Err(ActorError::ActorNotReady(address))
            }
        } else {
            Err(ActorError::AddressNotFound(address))
        }
    }

    /// Sends a message to all actors that match the given address regex.
    /// It returns a vector of results, success or error for each actor.
    /// It doesn't returns results from the actors, only whether the message was sent successfully or not.
    pub async fn send_broadcast<T>(
        &self,
        address_regex: String,
        msg: <T as Actor>::Message,
    ) -> Vec<Result<(), ActorError>>
    where
        T: Actor,
    {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let _ = self
            .handler_tx
            .send(ActorSystemCmd::FilterAddress(address_regex, tx));
        let addresses = match rx.await {
            Ok(addresses) => addresses,
            Err(e) => {
                error!("Receive address list failed: {:?}", e);
                return vec![Err(ActorError::from(e))];
            }
        };
        let mut result = Vec::new();
        for address in addresses {
            let (tx, rx) = tokio::sync::oneshot::channel();
            let _ = self
                .handler_tx
                .send(ActorSystemCmd::FindActor(address.clone(), tx));
            if let Ok(Some((tx, ready))) = rx.await {
                if ready {
                    match rmp_serde::to_vec(&msg) {
                        Ok(x) => {
                            let message = Message::new(x, None);
                            result.push(
                                tx.send(message)
                                    .map(|_| ())
                                    .map_err(|e| ActorError::UnboundedChannelSend(e)),
                            );
                        }
                        Err(e) => {
                            result.push(Err(ActorError::from(e)));
                        }
                    }
                } else {
                    result.push(Err(ActorError::ActorNotReady(address)));
                }
            } else {
                result.push(Err(ActorError::AddressNotFound(address)));
            }
        }
        result
    }

    /// Sends a message to a specific actor and waits for the result.
    pub async fn send_and_recv<T>(
        &self,
        address: String,
        msg: <T as Actor>::Message,
    ) -> Result<<T as Actor>::Result, ActorError>
    where
        T: Actor,
    {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let _ = self
            .handler_tx
            .send(ActorSystemCmd::FindActor(address.clone(), tx));
        if let Ok(Some((tx, ready))) = rx.await {
            if ready {
                let (result_tx, result_rx) = tokio::sync::oneshot::channel();
                let _ = tx.send(Message::new(rmp_serde::to_vec(&msg)?, Some(result_tx)))?;
                Ok(rmp_serde::from_slice::<<T as Actor>::Result>(
                    &result_rx.await?,
                )?)
            } else {
                Err(ActorError::ActorNotReady(address))
            }
        } else {
            Err(ActorError::AddressNotFound(address))
        }
    }

    /// Runs a job with the specified actor and message.
    /// If you want to subscribe to the results, set `subscribe` to true.
    /// It returns a receiver that you can use to receive the results.
    pub async fn run_job<T>(
        &self,
        address: String,
        subscribe: bool,
        job: JobSpec,
        msg: <T as Actor>::Message,
    ) -> Result<
        Option<
            tokio::sync::mpsc::UnboundedReceiver<
                Result<<T as Actor>::Result, rmp_serde::decode::Error>,
            >,
        >,
        ActorError,
    >
    where
        T: Actor,
    {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let msg = match rmp_serde::to_vec(&msg) {
            Ok(msg) => msg,
            Err(e) => {
                error!("Serialize message failed: {:?}", e);
                return Err(ActorError::from(e));
            }
        };
        let _ = self
            .handler_tx
            .send(ActorSystemCmd::FindActor(address.clone(), tx));
        if let Ok(Some((tx, ready))) = rx.await {
            if ready {
                let tx = tx.clone();
                if subscribe {
                    let (sub_tx, sub_rx) = tokio::sync::mpsc::unbounded_channel();
                    let msg = msg.clone();
                    let _ = tokio::spawn(async move {
                        let mut i = 0;
                        if let Some(interval) = job.interval() {
                            loop {
                                if job.start_at() <= std::time::SystemTime::now() {
                                    i += 1;
                                    let (result_tx, result_rx) = tokio::sync::oneshot::channel();
                                    if let Err(e) =
                                        tx.send(Message::new(msg.clone(), Some(result_tx)))
                                    {
                                        error!("Send message failed: {:?}", e);
                                        drop(sub_tx);
                                        return;
                                    }
                                    let result = match result_rx.await {
                                        Ok(result) => result,
                                        Err(e) => {
                                            error!("Receive result failed: {:?}", e);
                                            drop(sub_tx);
                                            return;
                                        }
                                    };
                                    let _ =
                                        sub_tx.send(rmp_serde::from_slice::<<T as Actor>::Result>(
                                            &result,
                                        ));
                                    tokio::time::sleep(interval).await;
                                    if let Some(max_iter) = job.max_iter() {
                                        if i >= max_iter {
                                            drop(sub_tx);
                                            return;
                                        }
                                    }
                                }
                            }
                        } else {
                            if job.start_at() <= std::time::SystemTime::now() {
                                let (result_tx, result_rx) = tokio::sync::oneshot::channel();
                                let msg = match rmp_serde::to_vec(&msg) {
                                    Ok(msg) => msg,
                                    Err(e) => {
                                        error!("Serialize message failed: {:?}", e);
                                        drop(sub_tx);
                                        return;
                                    }
                                };
                                if let Err(e) = tx.send(Message::new(msg, Some(result_tx))) {
                                    error!("Send message failed: {:?}", e);
                                    return;
                                }
                                let result = match result_rx
                                    .await
                                    .map(|x| rmp_serde::from_slice::<<T as Actor>::Result>(&x))
                                {
                                    Ok(result) => result,
                                    Err(e) => {
                                        error!("Receive result failed: {:?}", e);
                                        drop(sub_tx);
                                        return;
                                    }
                                };
                                let _ = sub_tx.send(result);
                            }
                        }
                    });
                    Ok(Some(sub_rx))
                } else {
                    let _ = tokio::spawn(async move {
                        let mut i = 0;
                        if let Some(interval) = job.interval() {
                            loop {
                                if job.start_at() <= std::time::SystemTime::now() {
                                    i += 1;
                                    if let Err(e) = tx.send(Message::new(msg.clone(), None)) {
                                        error!("Send message failed: {:?}", e);
                                        return;
                                    }
                                    tokio::time::sleep(interval).await;
                                    if let Some(max_iter) = job.max_iter() {
                                        if i >= max_iter {
                                            return;
                                        }
                                    }
                                }
                            }
                        } else {
                            if job.start_at() <= std::time::SystemTime::now() {
                                let _ = tx.send(Message::new(msg.clone(), None));
                            }
                        }
                    });
                    Ok(None)
                }
            } else {
                Err(ActorError::ActorNotReady(address))
            }
        } else {
            Err(ActorError::AddressNotFound(address))
        }
    }

    fn run(
        &mut self,
        handler_rx: tokio::sync::mpsc::UnboundedReceiver<ActorSystemCmd>,
    ) -> tokio::task::JoinHandle<()> {
        let handle = tokio::task::spawn_blocking(|| {
            tokio::runtime::Handle::current().block_on(actor_system_loop(handler_rx))
        });
        handle
    }
}

// {{{ fn actor_system_loop
async fn actor_system_loop(mut handler_rx: tokio::sync::mpsc::UnboundedReceiver<ActorSystemCmd>) {
    let mut address_list = HashSet::<String>::new();
    let mut map = HashMap::<
        String,
        (
            tokio::sync::mpsc::UnboundedSender<Message>,
            tokio::sync::mpsc::UnboundedSender<()>,
            tokio::sync::mpsc::UnboundedSender<()>,
            LifeCycle,
        ),
    >::new();
    while let Some(msg) = handler_rx.recv().await {
        match msg {
            ActorSystemCmd::Register(
                address,
                tx,
                restart_tx,
                kill_tx,
                life_cycle,
                result_tx,
                is_restarted,
            ) => {
                debug!("Register actor with address {}", address);
                if map.contains_key(&address) && !is_restarted {
                    let _ = result_tx.send(Err(ActorError::AddressAlreadyExist(address)));
                    continue;
                }
                map.insert(address.clone(), (tx, restart_tx, kill_tx, life_cycle));
                address_list.insert(address);
                let _ = result_tx.send(Ok(()));
            }
            ActorSystemCmd::Restart(address_regex) => {
                debug!("Restart actor with address {}", address_regex);
                let addresses = match filter_address(&address_list, &address_regex) {
                    Ok(addresses) => addresses,
                    Err(e) => {
                        error!("Filter address failed: {:?}", e);
                        continue;
                    }
                };
                for address in addresses {
                    if let Some((_tx, restart_tx, _kill_tx, _life_cycle)) = map.get(&address) {
                        let _ = restart_tx.send(());
                    }
                }
            }
            ActorSystemCmd::Unregister(address_regex) => {
                debug!("Unregister actor with address {}", address_regex);
                let addresses = match filter_address(&address_list, &address_regex) {
                    Ok(addresses) => addresses,
                    Err(e) => {
                        error!("Filter address failed: {:?}", e);
                        continue;
                    }
                };
                for address in addresses {
                    match map.entry(address.to_string()) {
                        std::collections::hash_map::Entry::Occupied(mut entry) => {
                            let _ = entry.get_mut().2.send(());
                            entry.remove_entry();
                            address_list.remove(&address);
                        }
                        std::collections::hash_map::Entry::Vacant(_) => {
                            continue;
                        }
                    }
                }
            }
            ActorSystemCmd::FilterAddress(address_regex, result_tx) => {
                debug!("FilterAddress with regex {}", address_regex);
                let addresses = match filter_address(&address_list, &address_regex) {
                    Ok(addresses) => addresses,
                    Err(e) => {
                        error!("Filter address failed: {:?}", e);
                        continue;
                    }
                };
                let _ = result_tx.send(addresses);
            }
            ActorSystemCmd::FindActor(address, result_tx) => {
                debug!("FindActor with address {}", address);
                if let Some((tx, _restart_tx, _kill_tx, life_cycle)) = map.get(&address) {
                    let _ = result_tx.send(Some((
                        tx.clone(),
                        match life_cycle {
                            LifeCycle::Receiving => true,
                            _ => false,
                        },
                    )));
                } else {
                    let _ = result_tx.send(None);
                }
            }
            ActorSystemCmd::SetLifeCycle(address, life_cycle) => {
                debug!(
                    "SetLifecycle with address {} into {:?}",
                    address, life_cycle
                );
                if let Some(actor) = map.get_mut(&address) {
                    actor.3 = life_cycle;
                };
            }
        };
    }
}
// }}}

// {{{ fn filter_address
fn filter_address(
    address_list: &HashSet<String>,
    regex: &str,
) -> Result<Vec<String>, regex::Error> {
    let regex = regex::Regex::new(&format!("^{}$", regex.replace("*", "(\\S+)"))).map_err(|e| {
        error!("Regex error: {:?}", e);
        e
    })?;
    Ok(address_list
        .iter()
        .filter(|x| regex.is_match(x))
        .map(|x| x.to_string())
        .collect())
}
// }}}
