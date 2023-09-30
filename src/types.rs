#[cfg(feature = "tokio")]
#[derive(Debug)]
pub struct Message<T, R> {
    inner: T,
    result_tx: Option<tokio::sync::oneshot::Sender<R>>,
}

#[cfg(feature = "tokio")]
impl<T, R> Message<T, R>
where
    T: Sized + Send + Clone,
    R: Sized + Send,
{
    pub fn new(inner: T, result_tx: Option<tokio::sync::oneshot::Sender<R>>) -> Self {
        Self { inner, result_tx }
    }

    pub fn inner(&self) -> T {
        self.inner.clone()
    }

    pub fn result_tx(self) -> Option<tokio::sync::oneshot::Sender<R>> {
        self.result_tx
    }
}

#[cfg(feature = "std")]
#[derive(Debug)]
pub struct Message<T, R> {
    inner: T,
    result_tx: Option<std::sync::mpsc::Sender<R>>,
}

#[cfg(feature = "std")]
impl<T, R> Message<T, R>
where
    T: Sized + Send + Clone,
    R: Sized + Send,
{
    pub fn new(inner: T, result_tx: Option<std::sync::mpsc::Sender<R>>) -> Self {
        Self { inner, result_tx }
    }

    pub fn inner(&self) -> T {
        self.inner.clone()
    }

    pub fn result_tx(self) -> Option<std::sync::mpsc::Sender<R>> {
        self.result_tx
    }
}

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
