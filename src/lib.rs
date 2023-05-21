pub mod error;
pub mod types;

use crate::error::ActorError;
pub use crate::types::{JobSpec, Message};
use log::error;
use std::collections::HashMap;
use std::error::Error;

#[async_trait::async_trait]
pub trait Handler<T, R, E>
where
    Self: Sized + Send + Sync + 'static,
    T: Sized + Send + Clone + Sync + 'static,
    R: Sized + Send + 'static,
    E: Error + Send,
{
    fn address(&self) -> String;

    async fn handler(&mut self, msg: T) -> Result<R, E>;

    fn register(mut self, actor: &mut Actor<T, R>, kill_in_error: bool) {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Message<T, R>>();
        let _ = actor.register(self.address(), tx);
        let _ = tokio::spawn(async move {
            while let Some(msg) = rx.recv().await {
                match self.handler(msg.inner()).await {
                    Ok(result) => {
                        if let Some(result_tx) = msg.result_tx() {
                            let _ = result_tx.send(result);
                        }
                    }
                    Err(e) => {
                        error!("Handler's result has error: {:?}", e);
                        if kill_in_error {
                            break;
                        }
                    }
                }
            }
        });
    }
}

#[derive(Clone)]
pub struct Actor<T, R> {
    map: HashMap<String, tokio::sync::mpsc::UnboundedSender<Message<T, R>>>,
}

impl<T, R> Actor<T, R>
where
    T: Sized + Send + Clone + Sync + 'static,
    R: Sized + Send + 'static,
{
    pub fn new() -> Self {
        Self {
            map: HashMap::new(),
        }
    }

    pub fn register(
        &mut self,
        address: String,
        tx: tokio::sync::mpsc::UnboundedSender<Message<T, R>>,
    ) {
        self.map.insert(address, tx);
    }

    pub fn unregister(&mut self, address: String) {
        self.map.remove(&address);
    }

    pub fn send(&self, address: String, msg: T) -> Result<(), ActorError<T, R>> {
        if let Some(tx) = self.map.get(&address) {
            let _ = tx.send(Message::new(msg, None))?;
            Ok(())
        } else {
            Err(ActorError::AddressNotFound(address))
        }
    }

    pub async fn send_and_recv(&self, address: String, msg: T) -> Result<R, ActorError<T, R>> {
        if let Some(tx) = self.map.get(&address) {
            let (result_tx, result_rx) = tokio::sync::oneshot::channel::<R>();
            let _ = tx.send(Message::new(msg, Some(result_tx)))?;
            Ok(result_rx.await?)
        } else {
            Err(ActorError::AddressNotFound(address))
        }
    }

    pub async fn run_job(
        &self,
        address: String,
        subscript: bool,
        job: JobSpec,
        msg: T,
    ) -> Result<
        Option<
            tokio::sync::mpsc::UnboundedReceiver<Result<R, tokio::sync::oneshot::error::RecvError>>,
        >,
        ActorError<T, R>,
    > {
        if let Some(tx) = self.map.get(&address) {
            let tx = tx.clone();
            if subscript {
                let (sub_tx, sub_rx) = tokio::sync::mpsc::unbounded_channel();
                tokio::spawn(async move {
                    let mut i = 0;
                    if let Some(interval) = job.interval() {
                        loop {
                            i += 1;
                            if job.start_at() <= std::time::SystemTime::now() {
                                let (result_tx, result_rx) = tokio::sync::oneshot::channel::<R>();
                                let _ = tx.send(Message::new(msg.clone(), Some(result_tx)));
                                let _ = sub_tx.send(result_rx.await);
                                tokio::time::sleep(interval).await;
                                if let Some(max_iter) = job.max_iter() && i >= max_iter{
                                    break;
                                }
                            }
                        }
                    } else {
                        if job.start_at() <= std::time::SystemTime::now() {
                            let (result_tx, result_rx) = tokio::sync::oneshot::channel::<R>();
                            let _ = tx.send(Message::new(msg.clone(), Some(result_tx)));
                            let _ = sub_tx.send(result_rx.await);
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
                                if let Some(max_iter) = job.max_iter() && i >= max_iter{
                                    break;
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
            Err(ActorError::AddressNotFound(address))
        }
    }
}
