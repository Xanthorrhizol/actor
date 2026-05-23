use crate::channel;
use crate::{ActorError, actor::Actor};
use async_trait::async_trait;
use std::any::Any;
use std::sync::Arc;

/// Internal helper trait that bridges the channel-specific sender into the
/// `Mailbox` abstraction. Implemented automatically for `TypedMailbox<A>`;
/// you should never need to implement it manually.
pub(crate) trait Messenger {
    type TargetActor: Actor;

    async fn send(
        tx: channel::Sender<Message<Self::TargetActor>>,
        msg: Arc<<Self::TargetActor as Actor>::Message>,
    ) -> Result<(), ActorError> {
        channel::send(&tx, Message::new(msg, None))
            .await
            .map_err(|e| ActorError::ChannelSend(e.to_string()))
    }

    async fn send_and_recv(
        tx: channel::Sender<Message<Self::TargetActor>>,
        msg: Arc<<Self::TargetActor as Actor>::Message>,
    ) -> Result<<Self::TargetActor as Actor>::Result, ActorError> {
        let (result_tx, result_rx) = tokio::sync::oneshot::channel();
        channel::send(&tx, Message::new(msg, Some(result_tx)))
            .await
            .map_err(|e| ActorError::ChannelSend(e.to_string()))?;
        Ok(result_rx.await?)
    }
}

/// Type-erased mailbox interface stored in the system's actor map.
///
/// `actor_system_loop` keeps `Arc<dyn Mailbox>` per address so the loop
/// can route arbitrary actor types through one HashMap. The payload type
/// is `Arc<dyn Any + Send + Sync>`; the concrete `TypedMailbox<A>` impl
/// downcasts it to `A::Message` before sending.
#[async_trait]
pub trait Mailbox: Send + Sync {
    /// Fire-and-forget send. Downcast the payload to `A::Message` and push
    /// it onto the actor's mpsc channel.
    async fn send(&self, msg: Arc<dyn Any + Send + Sync>) -> Result<(), ActorError>;
    /// Send and await the actor's reply. The reply is returned as a boxed
    /// `Any`; the caller downcasts to `A::Result`.
    async fn send_and_recv(
        &self,
        msg: Arc<dyn Any + Send + Sync>,
    ) -> Result<Box<dyn Any + Send>, ActorError>;
}

/// Concrete `Mailbox` for a specific actor type `A`. Holds the channel
/// sender to the actor's run loop and performs the type-safe downcast on
/// each send. Constructed in `Actor::run_actor` and erased to
/// `Arc<dyn Mailbox>` for storage in the system's actor map.
pub struct TypedMailbox<A: Actor + Send + Sync> {
    tx: channel::Sender<Message<A>>,
}

impl<A: Actor + Send + Sync> TypedMailbox<A> {
    /// Wrap an mpsc sender. Only called from `Actor::run_actor`.
    pub fn new(tx: channel::Sender<Message<A>>) -> Self {
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

/// Internal channel envelope wrapping the user's `<T as Actor>::Message`
/// plus an optional `oneshot` reply channel.
///
/// Users never construct this directly — `send` / `send_and_recv` wrap
/// the user's message into a `Message<T>` before pushing it onto the
/// mailbox channel. Exposed so advanced callers can drive the mailbox
/// directly via `handler_tx`. (Not in the prelude — the name is too
/// close to `Self::Message` and would shadow user types.)
///
/// `Clone` is implemented but drops `result_tx` on the clone (you can't
/// clone a oneshot sender). Clones therefore become fire-and-forget
/// envelopes regardless of the original's reply channel.
#[derive(Debug)]
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
    /// Build an envelope. `result_tx = Some(_)` makes the send wait for a
    /// reply (`send_and_recv` flow); `None` is fire-and-forget (`send`).
    pub fn new(
        inner: Arc<<T as Actor>::Message>,
        result_tx: Option<tokio::sync::oneshot::Sender<<T as Actor>::Result>>,
    ) -> Self {
        Self { inner, result_tx }
    }

    /// Clone-out the inner payload `Arc` for the receiving handler. The
    /// envelope keeps its own reference until dropped.
    pub fn inner(&self) -> Arc<<T as Actor>::Message> {
        self.inner.clone()
    }

    /// Take the reply channel (if any), leaving `None` behind. The actor
    /// loop calls this once after `handle` produces its `Ok` result and
    /// uses the returned `Sender` to deliver the reply.
    pub fn result_tx(&mut self) -> Option<tokio::sync::oneshot::Sender<<T as Actor>::Result>> {
        let tx = self.result_tx.take();
        self.result_tx = None;
        tx
    }
}

/// Schedule for a job submitted via `ActorSystem::run_job`.
///
/// - `max_iter` — cap on iterations. `None` = run forever (until
///   `abort_job` or the actor disappears). `Some(n)` exits after `n`
///   successful iterations.
/// - `interval` — wait between iterations. `None` is a single-shot job
///   (`max_iter` is forced to `Some(1)` in `new`).
/// - `start_at` — earliest moment the first iteration may run. If in the
///   past, the job starts immediately. If in the future, the job loop
///   sleeps until then before the first dispatch.
#[derive(Debug, Clone)]
pub struct JobSpec {
    max_iter: Option<usize>,
    interval: Option<std::time::Duration>,
    start_at: std::time::SystemTime,
}

impl JobSpec {
    /// Build a `JobSpec`. If `interval` is `None`, `max_iter` is forced
    /// to `Some(1)` regardless of the value you pass — single-shot jobs
    /// always run exactly once.
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

    /// Maximum number of iterations, or `None` for infinite.
    pub fn max_iter(&self) -> Option<usize> {
        self.max_iter
    }

    /// Earliest moment the first iteration may run.
    pub fn start_at(&self) -> std::time::SystemTime {
        self.start_at
    }

    /// Time between iterations, or `None` for single-shot.
    pub fn interval(&self) -> Option<std::time::Duration> {
        self.interval
    }
}

impl Default for JobSpec {
    /// Single-shot, runs immediately. Equivalent to
    /// `JobSpec::new(Some(1), None, SystemTime::now())`.
    fn default() -> Self {
        Self {
            max_iter: Some(1),
            interval: None,
            start_at: std::time::SystemTime::now(),
        }
    }
}

/// Handle returned by `run_job`.
///
/// `job_id` is stable for the life of the job — pass it to
/// `abort_job` / `stop_job` / `resume_job`. `result_subscriber_rx` is
/// `Some` only when `run_job` was called with `subscribe = true`; each
/// iteration's outcome is pushed onto it. When the job ends (max_iter
/// reached, aborted, or single-shot completion) the channel is closed,
/// so `recv()` returning `None` is the natural termination signal.
#[derive(Debug)]
pub struct RunJobResult<T: Actor> {
    /// Identifier you pass to `abort_job` / `stop_job` / `resume_job`.
    /// Defaults to a fresh UUID v4 unless an explicit id was supplied.
    pub job_id: String,
    /// Per-iteration result stream. `None` when the job was submitted
    /// with `subscribe = false` (fire-and-forget).
    pub result_subscriber_rx: Option<channel::Receiver<Result<<T as Actor>::Result, ActorError>>>,
}

/// Control-plane senders for a running job, kept by `actor_system_loop`
/// and looked up by `job_id` when the user calls `abort_job` / `stop_job`
/// / `resume_job`. Each channel carries `()` signals only; the job loop
/// awakens via `tokio::select!` and decides what to do.
///
/// Users don't construct or inspect this directly — it lives behind the
/// `abort_job` / `stop_job` / `resume_job` methods on `ActorSystem`.
#[derive(Clone)]
pub struct JobController {
    /// Signal the job loop to exit immediately. Honored at every wait
    /// point (`start_at`, pause-for-resume, inter-iteration sleep).
    pub abort_tx: channel::Sender<()>,
    /// Signal the job loop to pause after the current iteration. The
    /// loop then waits on `resume_tx` (or `abort_tx`) before continuing.
    pub stop_tx: channel::Sender<()>,
    /// Signal a stopped job to continue iterating.
    pub resume_tx: channel::Sender<()>,
}
