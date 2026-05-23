//! Thin wrapper over `tokio::sync::mpsc` that hides the bounded /
//! unbounded distinction behind a single API.
//!
//! Exactly one of the `bounded-channel` / `unbounded-channel` features
//! must be enabled. Call sites use the re-exported [`Sender`] /
//! [`Receiver`] types and the [`channel`] / [`send`] / [`try_send`]
//! helpers; the feature flag controls the underlying tokio type.

#[cfg(all(feature = "bounded-channel", feature = "unbounded-channel"))]
compile_error!(
    "xan-actor: enable exactly one of `bounded-channel` or `unbounded-channel`, not both"
);

#[cfg(not(any(feature = "bounded-channel", feature = "unbounded-channel")))]
compile_error!(
    "xan-actor: enable one of `bounded-channel` (default) or `unbounded-channel`"
);

pub use tokio::sync::mpsc::error::SendError;

/// Unified mpsc sender. Aliases `mpsc::Sender<T>` under `bounded-channel`
/// and `mpsc::UnboundedSender<T>` under `unbounded-channel`.
#[cfg(feature = "bounded-channel")]
pub type Sender<T> = tokio::sync::mpsc::Sender<T>;
/// Unified mpsc receiver paired with [`Sender`].
#[cfg(feature = "bounded-channel")]
pub type Receiver<T> = tokio::sync::mpsc::Receiver<T>;

#[cfg(feature = "unbounded-channel")]
pub type Sender<T> = tokio::sync::mpsc::UnboundedSender<T>;
#[cfg(feature = "unbounded-channel")]
pub type Receiver<T> = tokio::sync::mpsc::UnboundedReceiver<T>;

/// Construct a paired sender/receiver. `size` is the buffer capacity
/// under `bounded-channel`; ignored under `unbounded-channel`.
#[cfg(feature = "bounded-channel")]
pub fn channel<T>(size: usize) -> (Sender<T>, Receiver<T>) {
    tokio::sync::mpsc::channel(size)
}
#[cfg(feature = "unbounded-channel")]
pub fn channel<T>(_size: usize) -> (Sender<T>, Receiver<T>) {
    tokio::sync::mpsc::unbounded_channel()
}

/// Send a value, awaiting back-pressure on the bounded backend. The
/// unbounded backend completes synchronously; the `.await` is a no-op.
#[cfg(feature = "bounded-channel")]
pub async fn send<T>(tx: &Sender<T>, value: T) -> Result<(), SendError<T>> {
    tx.send(value).await
}
#[cfg(feature = "unbounded-channel")]
pub async fn send<T>(tx: &Sender<T>, value: T) -> Result<(), SendError<T>> {
    tx.send(value)
}

/// Extension trait letting call sites write `tx.send_async(x).await`
/// regardless of backend. Bridges over the bounded `Sender::send`
/// (async) vs. unbounded `UnboundedSender::send` (sync) shape mismatch.
#[async_trait::async_trait]
pub trait SendAsync<T: Send> {
    async fn send_async(&self, value: T) -> Result<(), SendError<T>>;
}

#[cfg(feature = "bounded-channel")]
#[async_trait::async_trait]
impl<T: Send> SendAsync<T> for tokio::sync::mpsc::Sender<T> {
    async fn send_async(&self, value: T) -> Result<(), SendError<T>> {
        self.send(value).await
    }
}

#[cfg(feature = "unbounded-channel")]
#[async_trait::async_trait]
impl<T: Send> SendAsync<T> for tokio::sync::mpsc::UnboundedSender<T> {
    async fn send_async(&self, value: T) -> Result<(), SendError<T>> {
        self.send(value)
    }
}

/// Non-blocking send. Maps the two backends' distinct error types to a
/// `String` so call sites that only log the failure don't need to care.
#[cfg(feature = "bounded-channel")]
pub fn try_send<T>(tx: &Sender<T>, value: T) -> Result<(), String> {
    tx.try_send(value).map_err(|e| format!("{:?}", e))
}
#[cfg(feature = "unbounded-channel")]
pub fn try_send<T>(tx: &Sender<T>, value: T) -> Result<(), String> {
    tx.send(value).map_err(|e| format!("{:?}", e))
}
