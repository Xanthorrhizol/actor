use crate::{ActorError, actor::Actor};
use async_trait::async_trait;
use std::any::Any;
use std::sync::Arc;

pub(crate) trait Messenger {
    type TargetActor: Actor;

    #[cfg(feature = "unbounded-channel")]
    fn send(
        tx: tokio::sync::mpsc::UnboundedSender<Message<Self::TargetActor>>,
        msg: Arc<<Self::TargetActor as Actor>::Message>,
    ) -> Result<(), ActorError> {
        tx.send(Message::new(msg, None))
            .map_err(|e| ActorError::UnboundedChannelSend(e.to_string()))
    }
    #[cfg(feature = "bounded-channel")]
    async fn send(
        tx: tokio::sync::mpsc::Sender<Message<Self::TargetActor>>,
        msg: Arc<<Self::TargetActor as Actor>::Message>,
    ) -> Result<(), ActorError> {
        tx.send(Message::new(msg, None))
            .await
            .map_err(|e| ActorError::BoundedChannelSend(e.to_string()))
    }

    #[cfg(feature = "unbounded-channel")]
    async fn send_and_recv(
        tx: tokio::sync::mpsc::UnboundedSender<Message<Self::TargetActor>>,
        msg: Arc<<Self::TargetActor as Actor>::Message>,
    ) -> Result<<Self::TargetActor as Actor>::Result, ActorError> {
        let (result_tx, result_rx) = tokio::sync::oneshot::channel();
        tx.send(Message::new(msg, Some(result_tx)))
            .map_err(|e| ActorError::UnboundedChannelSend(e.to_string()))?;
        Ok(result_rx.await?)
    }
    #[cfg(feature = "bounded-channel")]
    async fn send_and_recv(
        tx: tokio::sync::mpsc::Sender<Message<Self::TargetActor>>,
        msg: Arc<<Self::TargetActor as Actor>::Message>,
    ) -> Result<<Self::TargetActor as Actor>::Result, ActorError> {
        let (result_tx, result_rx) = tokio::sync::oneshot::channel();
        tx.send(Message::new(msg, Some(result_tx)))
            .await
            .map_err(|e| ActorError::BoundedChannelSend(e.to_string()))?;
        Ok(result_rx.await?)
    }
}

#[async_trait]
pub trait Mailbox: Send + Sync {
    async fn send(&self, msg: Arc<dyn Any + Send + Sync>) -> Result<(), ActorError>;
    async fn send_and_recv(
        &self,
        msg: Arc<dyn Any + Send + Sync>,
    ) -> Result<Box<dyn Any + Send>, ActorError>;
}
#[cfg(feature = "unbounded-channel")]
pub struct TypedMailbox<A: Actor + Send + Sync> {
    tx: tokio::sync::mpsc::UnboundedSender<Message<A>>,
}
#[cfg(feature = "bounded-channel")]
pub struct TypedMailbox<A: Actor + Send + Sync> {
    tx: tokio::sync::mpsc::Sender<Message<A>>,
}

#[cfg(feature = "unbounded-channel")]
impl<A: Actor + Send + Sync> TypedMailbox<A> {
    pub fn new(tx: tokio::sync::mpsc::UnboundedSender<Message<A>>) -> Self {
        Self { tx }
    }
}
#[cfg(feature = "bounded-channel")]
impl<A: Actor + Send + Sync> TypedMailbox<A> {
    pub fn new(tx: tokio::sync::mpsc::Sender<Message<A>>) -> Self {
        Self { tx }
    }
}

impl<A: Actor + Send + Sync> Messenger for TypedMailbox<A> {
    type TargetActor = A;
}

#[async_trait]
impl<A> Mailbox for TypedMailbox<A>
where
    A: Actor + Send + Sync + 'static,
    A::Message: Any + Send + Sync + 'static,
    A::Result: Any + Send + 'static,
{
    #[cfg(feature = "unbounded-channel")]
    async fn send(&self, msg: Arc<dyn Any + Send + Sync>) -> Result<(), ActorError> {
        let msg = Arc::downcast::<A::Message>(msg).map_err(|_| ActorError::MessageTypeMismatch)?;
        <Self as Messenger>::send(self.tx.clone(), msg)
    }

    #[cfg(feature = "bounded-channel")]
    async fn send(&self, msg: Arc<dyn Any + Send + Sync>) -> Result<(), ActorError> {
        let msg = Arc::downcast::<A::Message>(msg).map_err(|_| ActorError::MessageTypeMismatch)?;
        <Self as Messenger>::send(self.tx.clone(), msg).await
    }

    async fn send_and_recv(
        &self,
        msg: Arc<dyn Any + Send + Sync>,
    ) -> Result<Box<dyn Any + Send>, ActorError> {
        let msg = Arc::downcast::<A::Message>(msg).map_err(|_| ActorError::MessageTypeMismatch)?;
        let result = <Self as Messenger>::send_and_recv(self.tx.clone(), msg).await?;
        Ok(Box::new(result))
    }
}

#[derive(Debug)]
/// A message that can be sent to a worker thread.
/// > You don't need to implement this trait.
/// > It is a wrapper for data that you sent.
/// This message contains the data to be processed and a result channel.
/// The result channel is used to send the result back to the sender.
pub struct Message<T: Actor> {
    inner: Arc<<T as Actor>::Message>,
    result_tx: Option<tokio::sync::oneshot::Sender<<T as Actor>::Result>>,
}
impl<T> Clone for Message<T>
where
    T: Actor,
{
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            result_tx: None,
        }
    }
}

impl<T> Message<T>
where
    T: Actor,
{
    pub fn new(
        inner: Arc<<T as Actor>::Message>,
        result_tx: Option<tokio::sync::oneshot::Sender<<T as Actor>::Result>>,
    ) -> Self {
        Self { inner, result_tx }
    }

    pub fn inner(&self) -> Arc<<T as Actor>::Message> {
        self.inner.clone()
    }

    pub fn result_tx(&mut self) -> Option<tokio::sync::oneshot::Sender<<T as Actor>::Result>> {
        let tx = self.result_tx.take();
        self.result_tx = None;
        tx
    }
}

#[derive(Debug, Clone)]
/// A specification for a job that can be executed by a worker thread.
/// The `max_iter` is the maximum number of iterations the job will be executed.
/// If `max_iter` is `None`, the job will be executed indefinitely.
/// The `interval` is the time between two iterations of the job.
/// If the `interval` is `None`, the job will be executed only once.
/// The `start_at` is the time when the job will start executing.
/// If the `start_at` is in the past, the job will start executing immediately.
pub struct JobSpec {
    max_iter: Option<usize>,
    interval: Option<std::time::Duration>,
    start_at: std::time::SystemTime,
}

impl JobSpec {
    pub fn new(
        max_iter: Option<usize>,
        interval: Option<std::time::Duration>,
        start_at: std::time::SystemTime,
    ) -> Self {
        if let None = interval {
            Self {
                max_iter: Some(1),
                interval,
                start_at,
            }
        } else {
            Self {
                max_iter,
                interval,
                start_at,
            }
        }
    }

    pub fn max_iter(&self) -> Option<usize> {
        self.max_iter
    }

    pub fn start_at(&self) -> std::time::SystemTime {
        self.start_at
    }

    pub fn interval(&self) -> Option<std::time::Duration> {
        self.interval
    }
}

impl Default for JobSpec {
    fn default() -> Self {
        Self {
            max_iter: Some(1),
            interval: None,
            start_at: std::time::SystemTime::now(),
        }
    }
}
