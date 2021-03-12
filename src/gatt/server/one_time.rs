use futures::prelude::*;
use futures::task::{Context, Poll};
use std::pin::Pin;

use async_std::channel::{RecvError, SendError};
use async_std::sync::{Arc, Condvar, Mutex, Weak};

pub struct OneSender<T> {
    inner: Weak<(Mutex<Option<T>>, Condvar)>,
}

impl<T> OneSender<T> {
    pub async fn send(self, val: T) -> Result<(), SendError<T>> {
        let arc = match self.inner.upgrade() {
            Some(a) => a,
            None => return Err(SendError(val)),
        };
        *arc.0.lock().await = Some(val);
        arc.1.notify_all();
        Ok(())
    }
}
impl<T> Drop for OneSender<T> {
    fn drop(&mut self) {
        if let Some(arc) = self.inner.upgrade() {
            arc.1.notify_all();
        }
    }
}
/*
pub struct OneReceiver<T> {
    inner: Arc<(Mutex<Option<T>>, Condvar)>
}

impl<T> Future for OneReceiver<T> {
    type Output = Result<(), RecvError>;
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        unimplemented!();
    }
}
*/

pub fn one_time_channel<T>() -> (OneSender<T>, impl Future<Output = Result<T, RecvError>>) {
    let inner = Arc::new((Mutex::new(None), Condvar::new()));

    let sender = OneSender {
        inner: Arc::downgrade(&inner),
    };
    let recv = async move {
        let mut val = inner.0.lock().await;
        while val.is_none() {
            let val_fut = inner.1.wait(val);
            if Arc::weak_count(&inner) == 0 {
                return val_fut
                    .now_or_never()
                    .ok_or(RecvError)?
                    .take()
                    .ok_or(RecvError);
            }
            val = val_fut.await;
        }
        Ok(val.take().unwrap())
    };
    (sender, recv)
}
