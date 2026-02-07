use crate::{
    Actor, ActorError, CHANNEL_SIZE, JobController, JobSpec, LifeCycle, Mailbox, RunJobResult,
};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// Commands for the ActorSystem to handle various operations
/// You can send these commands to the ActorSystem's handler channel directly.
pub enum ActorSystemCmd {
    Register {
        actor_type: String,
        address: String,
        mailbox: Arc<dyn Mailbox>,
        restart_tx: tokio::sync::mpsc::Sender<()>,
        kill_tx: tokio::sync::mpsc::Sender<()>,
        life_cycle: LifeCycle,
        result_tx: tokio::sync::oneshot::Sender<Result<(), ActorError>>,
        is_restarted: bool,
    },
    Restart {
        address_regex: String,
    },
    Unregister {
        address_regex: String,
    },
    FilterAddress {
        address_regex: String,
        result_tx: tokio::sync::oneshot::Sender<Vec<String>>,
    },
    FindActor {
        actor_type: String,
        address: String,
        result_tx: tokio::sync::oneshot::Sender<
            Option<(Arc<dyn Mailbox>, bool)>, // mailbox, ready
        >,
    },
    SetLifeCycle {
        address: String,
        life_cycle: LifeCycle,
    },
    RegisterJob {
        job_id: String,
        controller: JobController,
    },
    FindJob {
        job_id: String,
        result_tx: tokio::sync::oneshot::Sender<Option<JobController>>,
    },
}

#[derive(Clone)]
/// The ActorSystem is the main entry point for managing actors.
/// It contains a handler channel to send commands to the actor system.
/// It's clonable so that it can be shared across different parts of the application.
pub struct ActorSystem {
    handler_tx: tokio::sync::mpsc::Sender<ActorSystemCmd>,
    cache: HashMap<String, (String, Arc<dyn Mailbox>)>,
    channel_size: usize,
}

impl Default for ActorSystem {
    fn default() -> Self {
        let (handler_tx, handler_rx) = tokio::sync::mpsc::channel(CHANNEL_SIZE);
        let mut me = Self {
            handler_tx,
            cache: HashMap::new(),
            channel_size: CHANNEL_SIZE,
        };
        me.run(handler_rx);
        me
    }
}

impl ActorSystem {
    /// Creates a new ActorSystem instance
    pub fn new(channel_size: Option<usize>) -> Self {
        let (handler_tx, handler_rx) =
            tokio::sync::mpsc::channel(channel_size.unwrap_or(CHANNEL_SIZE));
        let mut me = Self {
            handler_tx,
            cache: HashMap::new(),
            channel_size: channel_size.unwrap_or(CHANNEL_SIZE),
        };
        me.run(handler_rx);
        me
    }

    /// Returns the handler channel sender for the ActorSystem.
    /// You can use this to send commands to the ActorSystem directly.
    pub fn handler_tx(&self) -> tokio::sync::mpsc::Sender<ActorSystemCmd> {
        self.handler_tx.clone()
    }

    /// Filters the addresses of actors based on a regex pattern.
    pub async fn filter_address(&mut self, address_regex: String) -> Vec<String> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let _ = self
            .handler_tx
            .send(ActorSystemCmd::FilterAddress {
                address_regex,
                result_tx: tx,
            })
            .await;
        match rx.await {
            Ok(addresses) => addresses,
            Err(e) => {
                error!("Receive address list failed: {:?}", e);
                Vec::new()
            }
        }
    }

    /// Restarts an actor.
    pub fn restart(&mut self, address_regex: String) {
        if let Err(e) = self
            .handler_tx
            .try_send(ActorSystemCmd::Restart { address_regex })
        {
            error!("Send restart command failed: {:?}", e);
        }
    }

    /// Unregisters an actor.
    pub fn unregister(&mut self, address_regex: String) {
        if let Err(e) = self
            .handler_tx
            .try_send(ActorSystemCmd::Unregister { address_regex })
        {
            error!("Send unregister command failed: {:?}", e);
        }
    }

    /// Send a message to a specific actor by its address.
    /// It doesn't wait for the actor to be ready.
    pub async fn send<T>(
        &mut self,
        address: String,
        msg: <T as Actor>::Message,
    ) -> Result<(), ActorError>
    where
        T: Actor,
    {
        let mut retry_count = 0;
        let actor_type = std::any::type_name::<T>();
        let payload: Arc<dyn std::any::Any + Send + Sync> = Arc::new(msg);
        loop {
            let (tx, rx) = tokio::sync::oneshot::channel();
            match self.cache.entry(address.clone()) {
                std::collections::hash_map::Entry::Occupied(o) => {
                    let (cached_actor_type, mailbox) = o.get().clone();
                    if actor_type == cached_actor_type {
                        match mailbox.send(payload.clone()).await {
                            Ok(()) => {
                                debug!(
                                    "Send message to actor {} through cached_tx succeeded",
                                    address
                                );
                                return Ok(());
                            }
                            Err(e) => {
                                warn!(
                                    "Send message to actor {} through cached_tx failed: {:?} ... removing from cache",
                                    address, e
                                );
                                self.cache.remove(&address);
                            }
                        }
                    } else {
                        warn!(
                            "Send message with cached tx failed: cached tx of address {} and target actor {} is mismatched ... removing from cache",
                            address, actor_type,
                        );
                        self.cache.remove(&address);
                    }
                }
                _ => {}
            }
            let _ = self
                .handler_tx
                .send(ActorSystemCmd::FindActor {
                    actor_type: actor_type.to_string(),
                    address: address.clone(),
                    result_tx: tx,
                })
                .await;
            if let Ok(Some((tx, ready))) = rx.await {
                if ready {
                    debug!("Saving actor {} tx to cache", address);
                    self.cache
                        .insert(address.clone(), (actor_type.to_string(), tx.clone()));
                    let _ = tx.send(payload.clone()).await?;
                    return Ok(());
                } else {
                    retry_count += 1;
                    debug!(
                        "Actor {} not ready, retrying... ({}/10)",
                        address, retry_count
                    );
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                    if retry_count < 10 {
                        continue;
                    } else {
                        error!("Actor {} not ready after 10 retries, giving up", address);
                        return Err(ActorError::ActorNotReady(address));
                    }
                }
            } else {
                return Err(ActorError::AddressNotFound(address));
            }
        }
    }

    /// Send a message to a specific actor by its address.
    /// It doesn't wait for the actor to be ready.
    /// It doesn't cache the actor's tx for future use.
    pub async fn send_without_tx_cache<T>(
        &self,
        address: String,
        msg: <T as Actor>::Message,
    ) -> Result<(), ActorError>
    where
        T: Actor,
    {
        let mut retry_count = 0;
        let actor_type = std::any::type_name::<T>();
        let payload: Arc<dyn std::any::Any + Send + Sync> = Arc::new(msg);
        loop {
            let (tx, rx) = tokio::sync::oneshot::channel();
            let _ = self
                .handler_tx
                .send(ActorSystemCmd::FindActor {
                    actor_type: actor_type.to_string(),
                    address: address.clone(),
                    result_tx: tx,
                })
                .await;
            if let Ok(Some((tx, ready))) = rx.await {
                if ready {
                    let _ = tx.send(payload.clone()).await?;
                    return Ok(());
                } else {
                    retry_count += 1;
                    debug!(
                        "Actor {} not ready, retrying... ({}/10)",
                        address, retry_count
                    );
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                    if retry_count < 10 {
                        continue;
                    } else {
                        error!("Actor {} not ready after 10 retries, giving up", address);
                        return Err(ActorError::ActorNotReady(address));
                    }
                }
            } else {
                return Err(ActorError::AddressNotFound(address));
            }
        }
    }

    /// Sends a message to all actors that match the given address regex.
    /// It returns a vector of results, success or error for each actor.
    /// It doesn't returns results from the actors, only whether the message was sent successfully or not.
    pub async fn send_broadcast<T>(
        &mut self,
        address_regex: String,
        msg: <T as Actor>::Message,
    ) -> Vec<Result<(), ActorError>>
    where
        T: Actor,
    {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let actor_type = std::any::type_name::<T>();
        let _ = self
            .handler_tx
            .send(ActorSystemCmd::FilterAddress {
                address_regex,
                result_tx: tx,
            })
            .await;
        let addresses = match rx.await {
            Ok(addresses) => addresses,
            Err(e) => {
                error!("Receive address list failed: {:?}", e);
                return vec![Err(ActorError::from(e))];
            }
        };
        let payload: Arc<dyn std::any::Any + Send + Sync> = Arc::new(msg);
        let mut result = Vec::new();
        for address in addresses.iter() {
            match self.cache.entry(address.clone()) {
                std::collections::hash_map::Entry::Occupied(o) => {
                    let (cached_actor_type, tx) = o.get().clone();
                    if cached_actor_type == actor_type {
                        debug!(
                            "Send message to actor {} through cached_tx succeeded",
                            address
                        );
                        match tx.send(payload.clone()).await {
                            Ok(()) => {
                                result.push(Ok(()));
                                continue;
                            }
                            Err(e) => {
                                warn!(
                                    "Send message to actor {} through cached_tx failed: {:?} ... removing from cache",
                                    address, e
                                );
                                self.cache.remove(address);
                            }
                        }
                    } else {
                        warn!(
                            "Send message with cached tx failed: cached tx of address {} and target actor {} is mismatched ... removing from cache",
                            address, actor_type,
                        );
                        self.cache.remove(address);
                    }
                }
                _ => {}
            }
            let mut retry_count = 0;
            loop {
                let (tx, rx) = tokio::sync::oneshot::channel();
                let _ = self
                    .handler_tx
                    .send(ActorSystemCmd::FindActor {
                        actor_type: actor_type.to_string(),
                        address: address.clone(),
                        result_tx: tx,
                    })
                    .await;
                if let Ok(Some((tx, ready))) = rx.await {
                    if ready {
                        debug!("Saving actor {} tx to cache", address);
                        self.cache
                            .insert(address.clone(), (actor_type.to_string(), tx.clone()));
                        result.push(tx.send(payload.clone()).await);
                        break;
                    } else {
                        retry_count += 1;
                        debug!(
                            "Actor {} not ready, retrying... ({}/10)",
                            address, retry_count
                        );
                        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                        if retry_count < 10 {
                            continue;
                        } else {
                            error!("Actor {} not ready after 10 retries, giving up", address);
                            result.push(Err(ActorError::ActorNotReady(address.clone())));
                            break;
                        }
                    }
                } else {
                    result.push(Err(ActorError::AddressNotFound(address.clone())));
                    break;
                }
            }
        }
        result
    }

    /// Sends a message to all actors that match the given address regex.
    /// It returns a vector of results, success or error for each actor.
    /// It doesn't returns results from the actors, only whether the message was sent successfully or not.
    /// It doesn't cache the actor's tx for future use.
    pub async fn send_broadcast_without_tx_cache<T>(
        &self,
        address_regex: String,
        msg: <T as Actor>::Message,
    ) -> Vec<Result<(), ActorError>>
    where
        T: Actor,
    {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let actor_type = std::any::type_name::<T>();
        let _ = self
            .handler_tx
            .send(ActorSystemCmd::FilterAddress {
                address_regex,
                result_tx: tx,
            })
            .await;
        let addresses = match rx.await {
            Ok(addresses) => addresses,
            Err(e) => {
                error!("Receive address list failed: {:?}", e);
                return vec![Err(ActorError::from(e))];
            }
        };
        let payload: Arc<dyn std::any::Any + Send + Sync> = Arc::new(msg);
        let mut result = Vec::new();
        for address in addresses.iter() {
            let mut retry_count = 0;
            loop {
                let (tx, rx) = tokio::sync::oneshot::channel();
                let _ = self
                    .handler_tx
                    .send(ActorSystemCmd::FindActor {
                        actor_type: actor_type.to_string(),
                        address: address.clone(),
                        result_tx: tx,
                    })
                    .await;
                if let Ok(Some((tx, ready))) = rx.await {
                    if ready {
                        result.push(tx.send(payload.clone()).await);
                        break;
                    } else {
                        retry_count += 1;
                        debug!(
                            "Actor {} not ready, retrying... ({}/10)",
                            address, retry_count
                        );
                        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                        if retry_count < 10 {
                            continue;
                        } else {
                            error!("Actor {} not ready after 10 retries, giving up", address);
                            result.push(Err(ActorError::ActorNotReady(address.clone())));
                            break;
                        }
                    }
                } else {
                    result.push(Err(ActorError::AddressNotFound(address.clone())));
                    break;
                }
            }
        }
        result
    }

    /// Sends a message to a specific actor and waits for the result.
    pub async fn send_and_recv<T>(
        &mut self,
        address: String,
        msg: <T as Actor>::Message,
    ) -> Result<<T as Actor>::Result, ActorError>
    where
        T: Actor,
    {
        let payload: Arc<dyn std::any::Any + Send + Sync> = Arc::new(msg);
        let actor_type = std::any::type_name::<T>();
        let mut retry_count = 0;
        loop {
            match self.cache.entry(address.clone()) {
                std::collections::hash_map::Entry::Occupied(o) => {
                    let (cached_actor_type, tx) = o.get().clone();
                    if cached_actor_type == actor_type {
                        match tx.send_and_recv(payload.clone()).await {
                            Ok(result_any) => {
                                debug!(
                                    "Send message to actor {} through cached_tx succeeded",
                                    address
                                );
                                let result = result_any
                                    .downcast::<T::Result>()
                                    .map_err(|_| ActorError::MessageTypeMismatch)?;
                                return Ok(*result);
                            }
                            Err(e) => {
                                warn!(
                                    "Send message to actor {} through cached_tx failed: {:?} ... removing from cache",
                                    address, e
                                );
                                self.cache.remove(&address);
                            }
                        }
                    } else {
                        warn!(
                            "Send message with cached tx failed: cached tx of address {} and target actor {} is mismatched ... removing from cache",
                            address, actor_type,
                        );
                        self.cache.remove(&address);
                    }
                }
                _ => {}
            }
            let (tx, rx) = tokio::sync::oneshot::channel();
            let _ = self
                .handler_tx
                .send(ActorSystemCmd::FindActor {
                    actor_type: actor_type.to_string(),
                    address: address.clone(),
                    result_tx: tx,
                })
                .await;
            if let Ok(Some((tx, ready))) = rx.await {
                if ready {
                    debug!("Saving actor {} tx to cache", address);
                    self.cache
                        .insert(address.clone(), (actor_type.to_string(), tx.clone()));
                    let result_any = tx.send_and_recv(payload.clone()).await?;
                    let result = result_any
                        .downcast::<T::Result>()
                        .map_err(|_| ActorError::MessageTypeMismatch)?;
                    return Ok(*result);
                } else {
                    retry_count += 1;
                    debug!(
                        "Actor {} not ready, retrying... ({}/10)",
                        address, retry_count
                    );
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                    if retry_count < 10 {
                        continue;
                    } else {
                        error!("Actor {} not ready after 10 retries, giving up", address);
                        return Err(ActorError::ActorNotReady(address));
                    }
                }
            } else {
                return Err(ActorError::AddressNotFound(address));
            }
        }
    }

    /// Sends a message to a specific actor and waits for the result.
    /// It doesn't cache the actor's tx for future use.
    pub async fn send_and_recv_without_tx_cache<T>(
        &self,
        address: String,
        msg: <T as Actor>::Message,
    ) -> Result<<T as Actor>::Result, ActorError>
    where
        T: Actor,
    {
        let payload: Arc<dyn std::any::Any + Send + Sync> = Arc::new(msg);
        let actor_type = std::any::type_name::<T>();
        let mut retry_count = 0;
        loop {
            let (tx, rx) = tokio::sync::oneshot::channel();
            let _ = self
                .handler_tx
                .send(ActorSystemCmd::FindActor {
                    actor_type: actor_type.to_string(),
                    address: address.clone(),
                    result_tx: tx,
                })
                .await;
            if let Ok(Some((tx, ready))) = rx.await {
                if ready {
                    let result_any = tx.send_and_recv(payload.clone()).await?;
                    let result = result_any
                        .downcast::<T::Result>()
                        .map_err(|_| ActorError::MessageTypeMismatch)?;
                    return Ok(*result);
                } else {
                    retry_count += 1;
                    debug!(
                        "Actor {} not ready, retrying... ({}/10)",
                        address, retry_count
                    );
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                    if retry_count < 10 {
                        continue;
                    } else {
                        error!("Actor {} not ready after 10 retries, giving up", address);
                        return Err(ActorError::ActorNotReady(address));
                    }
                }
            } else {
                return Err(ActorError::AddressNotFound(address));
            }
        }
    }

    /// Runs a job with the specified actor and message.
    /// If you want to subscribe to the results, set `subscribe` to true.
    /// It returns a receiver that you can use to receive the results.
    /// If you want to set a job_id, set it to `Some(job_id)`.
    pub async fn run_job<T>(
        &mut self,
        address: String,
        subscribe: bool,
        job: JobSpec,
        msg: <T as Actor>::Message,
        job_id: Option<String>,
    ) -> Result<RunJobResult<T>, ActorError>
    where
        T: Actor,
    {
        let job_id = job_id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        let mut retry_count = 0;
        let payload: Arc<dyn std::any::Any + Send + Sync> = Arc::new(msg);
        let actor_type = std::any::type_name::<T>();
        let mailbox = loop {
            let (tx, rx) = tokio::sync::oneshot::channel();
            let _ = self
                .handler_tx
                .send(ActorSystemCmd::FindActor {
                    actor_type: actor_type.to_string(),
                    address: address.clone(),
                    result_tx: tx,
                })
                .await;
            if let Ok(Some((mailbox, ready))) = rx.await {
                if ready {
                    debug!("Saving actor {} tx to cache", address);
                    if let None = self.cache.get(&address) {
                        self.cache
                            .insert(address.clone(), (actor_type.to_string(), mailbox.clone()));
                    }
                    break mailbox;
                } else {
                    retry_count += 1;
                    debug!(
                        "Actor {} not ready, retrying... ({}/10)",
                        address, retry_count
                    );
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                    if retry_count < 10 {
                        continue;
                    } else {
                        error!("Actor {} not ready after 10 retries, giving up", address);
                        return Err(ActorError::ActorNotReady(address));
                    }
                }
            } else {
                return Err(ActorError::AddressNotFound(address.clone()));
            }
        };
        if subscribe {
            let (sub_tx, sub_rx) = tokio::sync::mpsc::channel(self.channel_size);
            let (abort_tx, abort_rx) = tokio::sync::mpsc::channel(self.channel_size);
            let (stop_tx, stop_rx) = tokio::sync::mpsc::channel(self.channel_size);
            let (resume_tx, resume_rx) = tokio::sync::mpsc::channel(self.channel_size);
            let payload = payload.clone();
            let _ = tokio::spawn(async move {
                let mut i = 0;
                let mut abort_rx = abort_rx;
                let mut stop_rx = stop_rx;
                let mut resume_rx = resume_rx;
                if let Some(interval) = job.interval() {
                    loop {
                        if abort_rx.try_recv().is_ok() {
                            drop(sub_tx);
                            return;
                        }
                        if stop_rx.try_recv().is_ok() {
                            resume_rx.recv().await;
                        }
                        if job.start_at() <= std::time::SystemTime::now() {
                            i += 1;
                            let result = match mailbox.send_and_recv(payload.clone()).await {
                                Ok(result_any) => result_any
                                    .downcast::<T::Result>()
                                    .map(|x| Ok(*x))
                                    .unwrap_or_else(|_| Err(ActorError::MessageTypeMismatch)),
                                Err(e) => Err(e),
                            };
                            let _ = sub_tx.send(result).await;
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
                    if abort_rx.try_recv().is_ok() {
                        drop(sub_tx);
                        return;
                    }
                    if stop_rx.try_recv().is_ok() {
                        resume_rx.recv().await;
                    }
                    if job.start_at() <= std::time::SystemTime::now() {
                        let result = match mailbox.send_and_recv(payload.clone()).await {
                            Ok(result_any) => result_any
                                .downcast::<T::Result>()
                                .map(|x| Ok(*x))
                                .unwrap_or_else(|_| Err(ActorError::MessageTypeMismatch)),
                            Err(e) => Err(e),
                        };
                        let _ = sub_tx.send(result).await;
                    }
                }
            });
            let _ = self
                .handler_tx
                .send(ActorSystemCmd::RegisterJob {
                    job_id: job_id.clone(),
                    controller: JobController {
                        abort_tx,
                        stop_tx,
                        resume_tx,
                    },
                })
                .await;
            return Ok(RunJobResult {
                job_id,
                result_subscriber_rx: Some(sub_rx),
            });
        } else {
            let (abort_tx, abort_rx) = tokio::sync::mpsc::channel(self.channel_size);
            let (stop_tx, stop_rx) = tokio::sync::mpsc::channel(self.channel_size);
            let (resume_tx, resume_rx) = tokio::sync::mpsc::channel(self.channel_size);
            let _ = tokio::spawn(async move {
                let mut i = 0;
                let mut abort_rx = abort_rx;
                let mut stop_rx = stop_rx;
                let mut resume_rx = resume_rx;
                if let Some(interval) = job.interval() {
                    loop {
                        if abort_rx.try_recv().is_ok() {
                            return;
                        }
                        if stop_rx.try_recv().is_ok() {
                            resume_rx.recv().await;
                        }
                        if job.start_at() <= std::time::SystemTime::now() {
                            i += 1;
                            let _ = mailbox.send(payload.clone()).await;
                            tokio::time::sleep(interval).await;
                            if let Some(max_iter) = job.max_iter() {
                                if i >= max_iter {
                                    return;
                                }
                            }
                        }
                    }
                } else {
                    if abort_rx.try_recv().is_ok() {
                        return;
                    }
                    if stop_rx.try_recv().is_ok() {
                        resume_rx.recv().await;
                    }
                    if job.start_at() <= std::time::SystemTime::now() {
                        let _ = mailbox.send(payload.clone()).await;
                    }
                }
            });
            let _ = self
                .handler_tx
                .send(ActorSystemCmd::RegisterJob {
                    job_id: job_id.clone(),
                    controller: JobController {
                        abort_tx,
                        stop_tx,
                        resume_tx,
                    },
                })
                .await;
            return Ok(RunJobResult {
                job_id,
                result_subscriber_rx: None,
            });
        }
    }

    /// Runs a job with the specified actor and message.
    /// If you want to subscribe to the results, set `subscribe` to true.
    /// It returns a receiver that you can use to receive the results.
    /// It doesn't cache the actor's tx for future use.
    /// If you want to set a job_id, set it to `Some(job_id)`.
    pub async fn run_job_without_tx_cache<T>(
        &self,
        address: String,
        subscribe: bool,
        job: JobSpec,
        msg: <T as Actor>::Message,
        job_id: Option<String>,
    ) -> Result<RunJobResult<T>, ActorError>
    where
        T: Actor,
    {
        let job_id = job_id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        let mut retry_count = 0;
        let payload: Arc<dyn std::any::Any + Send + Sync> = Arc::new(msg);
        let actor_type = std::any::type_name::<T>();
        let mailbox = loop {
            let (tx, rx) = tokio::sync::oneshot::channel();
            let _ = self
                .handler_tx
                .send(ActorSystemCmd::FindActor {
                    actor_type: actor_type.to_string(),
                    address: address.clone(),
                    result_tx: tx,
                })
                .await;
            if let Ok(Some((mailbox, ready))) = rx.await {
                if ready {
                    break mailbox;
                } else {
                    retry_count += 1;
                    debug!(
                        "Actor {} not ready, retrying... ({}/10)",
                        address, retry_count
                    );
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                    if retry_count < 10 {
                        continue;
                    } else {
                        error!("Actor {} not ready after 10 retries, giving up", address);
                        return Err(ActorError::ActorNotReady(address));
                    }
                }
            } else {
                return Err(ActorError::AddressNotFound(address.clone()));
            }
        };
        if subscribe {
            let (sub_tx, sub_rx) = tokio::sync::mpsc::channel(self.channel_size);
            let payload = payload.clone();
            let (abort_tx, abort_rx) = tokio::sync::mpsc::channel(self.channel_size);
            let (stop_tx, stop_rx) = tokio::sync::mpsc::channel(self.channel_size);
            let (resume_tx, resume_rx) = tokio::sync::mpsc::channel(self.channel_size);
            let _ = tokio::spawn(async move {
                let mut i = 0;
                let mut abort_rx = abort_rx;
                let mut stop_rx = stop_rx;
                let mut resume_rx = resume_rx;
                if let Some(interval) = job.interval() {
                    loop {
                        if abort_rx.try_recv().is_ok() {
                            drop(sub_tx);
                            return;
                        }
                        if stop_rx.try_recv().is_ok() {
                            resume_rx.recv().await;
                        }
                        if job.start_at() <= std::time::SystemTime::now() {
                            i += 1;
                            let result = match mailbox.send_and_recv(payload.clone()).await {
                                Ok(result_any) => result_any
                                    .downcast::<T::Result>()
                                    .map(|x| Ok(*x))
                                    .unwrap_or_else(|_| Err(ActorError::MessageTypeMismatch)),
                                Err(e) => Err(e),
                            };
                            let _ = sub_tx.send(result).await;
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
                    if abort_rx.try_recv().is_ok() {
                        drop(sub_tx);
                        return;
                    }
                    if stop_rx.try_recv().is_ok() {
                        resume_rx.recv().await;
                    }
                    if job.start_at() <= std::time::SystemTime::now() {
                        let result = match mailbox.send_and_recv(payload.clone()).await {
                            Ok(result_any) => result_any
                                .downcast::<T::Result>()
                                .map(|x| Ok(*x))
                                .unwrap_or_else(|_| Err(ActorError::MessageTypeMismatch)),
                            Err(e) => Err(e),
                        };
                        let _ = sub_tx.send(result).await;
                    }
                }
            });
            let _ = self
                .handler_tx
                .send(ActorSystemCmd::RegisterJob {
                    job_id: job_id.clone(),
                    controller: JobController {
                        abort_tx,
                        stop_tx,
                        resume_tx,
                    },
                })
                .await;
            return Ok(RunJobResult {
                job_id,
                result_subscriber_rx: Some(sub_rx),
            });
        } else {
            let (abort_tx, abort_rx) = tokio::sync::mpsc::channel(self.channel_size);
            let (stop_tx, stop_rx) = tokio::sync::mpsc::channel(self.channel_size);
            let (resume_tx, resume_rx) = tokio::sync::mpsc::channel(self.channel_size);
            let _ = tokio::spawn(async move {
                let mut i = 0;
                let mut abort_rx = abort_rx;
                let mut stop_rx = stop_rx;
                let mut resume_rx = resume_rx;
                if let Some(interval) = job.interval() {
                    loop {
                        if abort_rx.try_recv().is_ok() {
                            return;
                        }
                        if stop_rx.try_recv().is_ok() {
                            resume_rx.recv().await;
                        }
                        if job.start_at() <= std::time::SystemTime::now() {
                            i += 1;
                            let _ = mailbox.send(payload.clone()).await;
                            tokio::time::sleep(interval).await;
                            if let Some(max_iter) = job.max_iter() {
                                if i >= max_iter {
                                    return;
                                }
                            }
                        }
                    }
                } else {
                    if abort_rx.try_recv().is_ok() {
                        return;
                    }
                    if stop_rx.try_recv().is_ok() {
                        resume_rx.recv().await;
                    }
                    if job.start_at() <= std::time::SystemTime::now() {
                        let _ = mailbox.send(payload.clone()).await;
                    }
                }
            });
            let _ = self
                .handler_tx
                .send(ActorSystemCmd::RegisterJob {
                    job_id: job_id.clone(),
                    controller: JobController {
                        abort_tx,
                        stop_tx,
                        resume_tx,
                    },
                })
                .await;
            return Ok(RunJobResult {
                job_id,
                result_subscriber_rx: None,
            });
        }
    }

    pub async fn abort_job(&self, job_id: String) {
        info!("Aborting job {}", job_id);
        let (tx, rx) = tokio::sync::oneshot::channel();
        let _ = self
            .handler_tx
            .send(ActorSystemCmd::FindJob {
                job_id: job_id.clone(),
                result_tx: tx,
            })
            .await;
        match rx.await {
            Ok(Some(controller)) => {
                let _ = controller.abort_tx.send(());
            }
            Ok(None) => {
                error!("Job {} not found", job_id);
            }
            Err(e) => {
                error!("Find job {} failed: {}", job_id, e);
            }
        }
    }

    pub async fn stop_job(&self, job_id: String) {
        info!("Stopping job {}", job_id);
        let (tx, rx) = tokio::sync::oneshot::channel();
        let _ = self
            .handler_tx
            .send(ActorSystemCmd::FindJob {
                job_id: job_id.clone(),
                result_tx: tx,
            })
            .await;
        match rx.await {
            Ok(Some(controller)) => {
                let _ = controller.stop_tx.send(());
            }
            Ok(None) => {
                error!("Job {} not found", job_id);
            }
            Err(e) => {
                error!("Find job {} failed: {}", job_id, e);
            }
        }
    }

    pub async fn resume_job(&self, job_id: String) {
        info!("Resuming job {}", job_id);
        let (tx, rx) = tokio::sync::oneshot::channel();
        let _ = self
            .handler_tx
            .send(ActorSystemCmd::FindJob {
                job_id: job_id.clone(),
                result_tx: tx,
            })
            .await;
        match rx.await {
            Ok(Some(controller)) => {
                let _ = controller.resume_tx.send(());
            }
            Ok(None) => {
                error!("Job {} not found", job_id);
            }
            Err(e) => {
                error!("Find job {} failed: {}", job_id, e);
            }
        }
    }

    fn run(
        &mut self,
        handler_rx: tokio::sync::mpsc::Receiver<ActorSystemCmd>,
    ) -> tokio::task::JoinHandle<()> {
        let handle = tokio::task::spawn_blocking(|| {
            tokio::runtime::Handle::current().block_on(actor_system_loop(handler_rx))
        });
        handle
    }
}

// {{{ fn actor_system_loop
async fn actor_system_loop(mut handler_rx: tokio::sync::mpsc::Receiver<ActorSystemCmd>) {
    let mut address_list = HashSet::<String>::new();
    let mut actor_map = HashMap::<
        String,
        (
            String,
            Arc<dyn Mailbox>,
            tokio::sync::mpsc::Sender<()>,
            tokio::sync::mpsc::Sender<()>,
            LifeCycle,
        ),
    >::new();
    let mut job_controllers = HashMap::new();
    while let Some(msg) = handler_rx.recv().await {
        match msg {
            ActorSystemCmd::Register {
                actor_type,
                address,
                mailbox,
                restart_tx,
                kill_tx,
                life_cycle,
                result_tx,
                is_restarted,
            } => {
                debug!(
                    "Register actor with address {} with type {}",
                    address, actor_type
                );
                if actor_map.contains_key(&address) && !is_restarted {
                    let _ = result_tx.send(Err(ActorError::AddressAlreadyExist(address)));
                    continue;
                }
                actor_map.insert(
                    address.clone(),
                    (actor_type, mailbox, restart_tx, kill_tx, life_cycle),
                );
                address_list.insert(address);
                let _ = result_tx.send(Ok(()));
            }
            ActorSystemCmd::Restart { address_regex } => {
                debug!("Restart actor with address {}", address_regex);
                let addresses = match filter_address(&address_list, &address_regex) {
                    Ok(addresses) => addresses,
                    Err(e) => {
                        error!("Filter address failed: {:?}", e);
                        continue;
                    }
                };
                for address in addresses {
                    if let Some((_actor_type, _tx, restart_tx, _kill_tx, life_cycle)) =
                        actor_map.get_mut(&address)
                    {
                        *life_cycle = LifeCycle::Restarting;
                        let _ = restart_tx.send(()).await;
                    }
                }
            }
            ActorSystemCmd::Unregister { address_regex } => {
                debug!("Unregister actor with address {}", address_regex);
                let addresses = match filter_address(&address_list, &address_regex) {
                    Ok(addresses) => addresses,
                    Err(e) => {
                        error!("Filter address failed: {:?}", e);
                        continue;
                    }
                };
                for address in addresses {
                    match actor_map.entry(address.to_string()) {
                        std::collections::hash_map::Entry::Occupied(mut entry) => {
                            let _ = entry.get_mut().3.send(()).await;
                            entry.remove_entry();
                            address_list.remove(&address);
                        }
                        std::collections::hash_map::Entry::Vacant(_) => {
                            continue;
                        }
                    }
                }
            }
            ActorSystemCmd::FilterAddress {
                address_regex,
                result_tx,
            } => {
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
            ActorSystemCmd::FindActor {
                actor_type,
                address,
                result_tx,
            } => {
                debug!(
                    "FindActor with address {} with type {}",
                    address, actor_type
                );
                if let Some((target_actor_type, tx, _restart_tx, _kill_tx, life_cycle)) =
                    actor_map.get(&address)
                {
                    match life_cycle {
                        LifeCycle::Receiving => {
                            if *target_actor_type == actor_type {
                                let _ = result_tx.send(Some((tx.clone(), true)));
                            } else {
                                let _ = result_tx.send(None);
                            }
                        }
                        _ => {
                            let _ = result_tx.send(Some((tx.clone(), false)));
                        }
                    }
                } else {
                    let _ = result_tx.send(None);
                }
            }
            ActorSystemCmd::SetLifeCycle {
                address,
                life_cycle,
            } => {
                debug!(
                    "SetLifecycle with address {} into {:?}",
                    address, life_cycle
                );
                if let Some(actor) = actor_map.get_mut(&address) {
                    actor.4 = life_cycle;
                };
            }
            ActorSystemCmd::RegisterJob { job_id, controller } => {
                debug!("RegisterJob with id {}", job_id);
                let _ = job_controllers.insert(job_id, controller);
            }
            ActorSystemCmd::FindJob { job_id, result_tx } => {
                debug!("FindJob with id {}", job_id);
                let _ = result_tx.send(job_controllers.get(&job_id).cloned());
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
