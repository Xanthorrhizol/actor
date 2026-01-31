use std::sync::Arc;

#[derive(Debug)]
/// A message that can be sent to a worker thread.
/// > You don't need to implement this trait.
/// > It is a wrapper for rmp_serde encoded data that you sent.
/// This message contains the data to be processed and an result channel.
/// The result channel is used to send the result back to the sender.
pub struct Message {
    inner: Arc<[u8]>,
    result_tx: Option<tokio::sync::oneshot::Sender<Vec<u8>>>,
}
impl Clone for Message {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            result_tx: None,
        }
    }
}

impl Message {
    pub fn new(
        inner: Arc<[u8]>,
        result_tx: Option<tokio::sync::oneshot::Sender<Vec<u8>>>,
    ) -> Self {
        Self { inner, result_tx }
    }

    pub fn inner(&self) -> &[u8] {
        &self.inner
    }

    pub fn result_tx(&mut self) -> Option<tokio::sync::oneshot::Sender<Vec<u8>>> {
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
