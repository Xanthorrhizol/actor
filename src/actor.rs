use crate::LifeCycle;
use crate::error::ActorError;
use crate::types::{JobSpec, Message};
use futures::FutureExt;
use std::collections::{HashMap, HashSet};
use std::error::Error;

pub enum ActorSystemCmd {
    Register(
        String,
        tokio::sync::mpsc::UnboundedSender<Message>,
        tokio::sync::mpsc::UnboundedSender<()>,
        tokio::sync::mpsc::UnboundedSender<()>,
        LifeCycle,
        tokio::sync::oneshot::Sender<()>,
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

#[async_trait::async_trait]
pub trait Actor<T, R, E>
where
    Self: Sized + 'static,
    T: Sized + Send + serde::de::DeserializeOwned,
    R: Sized + Send + serde::Serialize,
    E: Error + Send,
{
    fn address(&self) -> &str;

    async fn actor(&mut self, msg: T) -> Result<R, E>;

    async fn pre_start(&mut self) {}

    async fn pre_restart(&mut self) {}

    async fn post_stop(&mut self) {}

    async fn post_restart(&mut self) {}

    async fn run_actor(
        &mut self,
        actor_system_tx: tokio::sync::mpsc::UnboundedSender<ActorSystemCmd>,
        kill_in_error: bool,
        ready_tx: tokio::sync::mpsc::UnboundedSender<()>,
    ) -> Result<(), ActorError> {
        let mut restarted = false;
        loop {
            if restarted {
                self.post_restart().await;
            }
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Message>();
            let (kill_tx, mut kill_rx) = tokio::sync::mpsc::unbounded_channel::<()>();
            let (restart_tx, mut restart_rx) = tokio::sync::mpsc::unbounded_channel::<()>();
            let (result_tx, result_rx) = tokio::sync::oneshot::channel();

            let _ = actor_system_tx.send(ActorSystemCmd::Register(
                self.address().to_string(),
                tx,
                restart_tx,
                kill_tx,
                if restarted {
                    LifeCycle::Restarting
                } else {
                    LifeCycle::Starting
                },
                result_tx,
            ));
            let _ = result_rx.await;
            self.pre_start().await;
            restarted = true;
            let _ = actor_system_tx.send(ActorSystemCmd::SetLifeCycle(
                self.address().to_string(),
                LifeCycle::Receiving,
            ));
            let _ = ready_tx.send(());
            if let Some(_) = loop {
                let rx_fut = rx.recv().fuse();
                tokio::pin!(rx_fut);
                let kill_rx_fut = kill_rx.recv().fuse();
                tokio::pin!(kill_rx_fut);
                let restart_rx_fut = restart_rx.recv().fuse();
                tokio::pin!(restart_rx_fut);

                tokio::select! {
                    Some(mut msg) = &mut rx_fut => {
                        let result_tx = msg.result_tx();
                        let msg_de = match rmp_serde::from_slice::<T>(msg.inner()) {
                            Ok(msg) => msg,
                            Err(e) => {
                                if kill_in_error {
                                    error!("Deserialize message failed: {:?}", e);
                                    break Some(());
                                }
                                debug!("Deserialize message failed: {:?}", e);
                                break None;
                            }
                        };
                        match self.actor(msg_de).await {
                           Ok(result) => {
                                if let Some(result_tx) = result_tx {
                                    let result = rmp_serde::to_vec(&result)?;
                                    let _ = result_tx.send(result);
                                }
                            }
                           Err(e) => {
                                if kill_in_error {
                                    error!("Handler's result has error: {:?}", e);
                                    break Some(());
                                }
                                debug!("Handler's result has error: {:?}", e);
                                break None;
                           }
                       }
                    }
                    Some(_) = &mut kill_rx_fut => {
                        info!("Kill actor: address={}", self.address());
                        break Some(());
                    }
                    Some(_) = &mut restart_rx_fut => {
                        info!("Restart actor: address={}", self.address());
                        break None;
                    }
                };
            } {
                let _ = actor_system_tx.send(ActorSystemCmd::SetLifeCycle(
                    self.address().to_string(),
                    LifeCycle::Stopping,
                ));
                self.post_stop().await;
                let _ = actor_system_tx.send(ActorSystemCmd::SetLifeCycle(
                    self.address().to_string(),
                    LifeCycle::Terminated,
                ));
                break Ok(());
            }
            let _ = actor_system_tx.send(ActorSystemCmd::SetLifeCycle(
                self.address().to_string(),
                LifeCycle::Stopping,
            ));
            self.pre_restart().await;
            let _ = actor_system_tx.send(ActorSystemCmd::SetLifeCycle(
                self.address().to_string(),
                LifeCycle::Restarting,
            ));
        }
    }
    async fn register(mut self, actor_system: &mut ActorSystem, kill_in_error: bool) {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let actor_system_tx = actor_system.handler_tx();
        let _ = tokio::task::spawn_blocking(move || {
            tokio::runtime::Handle::current().block_on(self.run_actor(
                actor_system_tx,
                kill_in_error,
                tx,
            ))
        });
        let _ = rx.recv().await;
    }
}

#[derive(Clone)]
pub struct ActorSystem {
    handler_tx: tokio::sync::mpsc::UnboundedSender<ActorSystemCmd>,
}

impl Default for ActorSystem {
    fn default() -> Self {
        let (handler_tx, handler_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut me = Self { handler_tx };
        me.run(handler_rx);
        me
    }
}

impl ActorSystem {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn handler_tx(&self) -> tokio::sync::mpsc::UnboundedSender<ActorSystemCmd> {
        self.handler_tx.clone()
    }

    fn run(
        &mut self,
        mut handler_rx: tokio::sync::mpsc::UnboundedReceiver<ActorSystemCmd>,
    ) -> tokio::task::JoinHandle<()> {
        tokio::task::spawn_blocking(move || {
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
            while let Some(msg) = tokio::runtime::Handle::current().block_on(handler_rx.recv()) {
                match msg {
                    ActorSystemCmd::Register(
                        address,
                        tx,
                        restart_tx,
                        kill_tx,
                        life_cycle,
                        result_tx,
                    ) => {
                        debug!("Register actor with address {}", address);
                        map.insert(address.clone(), (tx, restart_tx, kill_tx, life_cycle));
                        address_list.insert(address);
                        let _ = result_tx.send(());
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
                            if let Some((_tx, restart_tx, _kill_tx, _life_cycle)) =
                                map.get(&address)
                            {
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
        })
    }

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

    pub async fn register(
        &mut self,
        address: String,
        tx: tokio::sync::mpsc::UnboundedSender<Message>,
        restart_tx: tokio::sync::mpsc::UnboundedSender<()>,
        kill_tx: tokio::sync::mpsc::UnboundedSender<()>,
        life_cycle: LifeCycle,
    ) {
        let (result_tx, result_rx) = tokio::sync::oneshot::channel();
        let _ = self.handler_tx.send(ActorSystemCmd::Register(
            address, tx, restart_tx, kill_tx, life_cycle, result_tx,
        ));
        let _ = result_rx.await;
    }

    pub fn set_lifecycle(&mut self, address: &str, life_cycle: LifeCycle) {
        let _ = self.handler_tx.send(ActorSystemCmd::SetLifeCycle(
            address.to_string(),
            life_cycle,
        ));
    }

    pub fn restart(&mut self, address_regex: String) {
        let _ = self.handler_tx.send(ActorSystemCmd::Restart(address_regex));
    }

    pub fn unregister(&mut self, address_regex: String) {
        let _ = self
            .handler_tx
            .send(ActorSystemCmd::Unregister(address_regex));
    }

    pub async fn send<T>(&self, address: String, msg: T) -> Result<(), ActorError>
    where
        T: serde::Serialize + serde::de::DeserializeOwned,
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
    pub async fn send_broadcast<T>(
        &self,
        address_regex: String,
        msg: T,
    ) -> Vec<Result<(), ActorError>>
    where
        T: serde::Serialize + serde::de::DeserializeOwned,
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

    pub async fn send_and_recv<T, R>(&self, address: String, msg: T) -> Result<R, ActorError>
    where
        T: serde::Serialize + serde::de::DeserializeOwned,
        R: serde::Serialize + serde::de::DeserializeOwned,
    {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let _ = self
            .handler_tx
            .send(ActorSystemCmd::FindActor(address.clone(), tx));
        if let Ok(Some((tx, ready))) = rx.await {
            if ready {
                let (result_tx, result_rx) = tokio::sync::oneshot::channel();
                let _ = tx.send(Message::new(rmp_serde::to_vec(&msg)?, Some(result_tx)))?;
                Ok(rmp_serde::from_slice::<R>(&result_rx.await?)?)
            } else {
                Err(ActorError::ActorNotReady(address))
            }
        } else {
            Err(ActorError::AddressNotFound(address))
        }
    }

    pub async fn run_job<T, R>(
        &self,
        address: String,
        subscript: bool,
        job: JobSpec,
        msg: T,
    ) -> Result<
        Option<tokio::sync::mpsc::UnboundedReceiver<Result<R, rmp_serde::decode::Error>>>,
        ActorError,
    >
    where
        T: serde::Serialize + Clone,
        R: serde::de::DeserializeOwned + Send + 'static,
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
                if subscript {
                    let (sub_tx, sub_rx) = tokio::sync::mpsc::unbounded_channel();
                    let msg = msg.clone();
                    tokio::spawn(async move {
                        let mut i = 0;
                        if let Some(interval) = job.interval() {
                            loop {
                                i += 1;
                                if job.start_at() <= std::time::SystemTime::now() {
                                    let (result_tx, result_rx) = tokio::sync::oneshot::channel();
                                    let _ = tx.send(Message::new(msg.clone(), Some(result_tx)));
                                    let result = match result_rx.await {
                                        Ok(result) => result,
                                        Err(e) => {
                                            error!("Receive result failed: {:?}", e);
                                            break;
                                        }
                                    };
                                    let _ = sub_tx.send(rmp_serde::from_slice::<R>(&result));
                                    tokio::time::sleep(interval).await;
                                    if let Some(max_iter) = job.max_iter() {
                                        if i >= max_iter {
                                            break;
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
                                        return;
                                    }
                                };
                                let _ = tx.send(Message::new(msg, Some(result_tx)));
                                let result =
                                    match result_rx.await.map(|x| rmp_serde::from_slice::<R>(&x)) {
                                        Ok(result) => result,
                                        Err(e) => {
                                            error!("Receive result failed: {:?}", e);
                                            return;
                                        }
                                    };
                                let _ = sub_tx.send(result);
                            }
                        }
                    });
                    Ok(Some(sub_rx))
                } else {
                    tokio::spawn(async move {
                        let mut i = 0;
                        if let Some(interval) = job.interval() {
                            loop {
                                i += 1;
                                if job.start_at() <= std::time::SystemTime::now() {
                                    let _ = tx.send(Message::new(msg.clone(), None));
                                    tokio::time::sleep(interval).await;
                                    if let Some(max_iter) = job.max_iter() {
                                        if i >= max_iter {
                                            break;
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
}

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
