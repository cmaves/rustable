use futures::prelude::*;

use async_std::channel::{RecvError, SendError};
use async_std::sync::{Arc, Condvar, Mutex, Weak};

pub struct OneSender<T> {
    inner: Weak<(Mutex<Option<T>>, Condvar)>,
}

impl<T> OneSender<T> {
    pub fn send(self, val: T) -> Result<(), SendError<T>> {
        let arc = match self.inner.upgrade() {
            Some(a) => a,
            None => return Err(SendError(val)),
        };
        let mut backoff = 0;
        loop {
            if let Some(mut lock) = arc.0.try_lock() {
                *lock = Some(val);
                arc.1.notify_all();
                return Ok(());
            }
            if backoff < 8 {
                backoff += 1;
            }
            for _ in 0..(1 << backoff) {
                std::hint::spin_loop();
            }
        }
    }
}
impl<T> Drop for OneSender<T> {
    fn drop(&mut self) {
        if let Some(arc) = self.inner.upgrade() {
            arc.1.notify_all();
        }
    }
}
pub struct OneReceiver<T> {
    inner: Arc<(Mutex<Option<T>>, Condvar)>,
}
/*pub enum TryRecvError<T> {
    Closed,
    WouldBlock(OneReceiver<T>),
    Empty(OneReceiver<T>),
}*/
impl<T> OneReceiver<T> {
    pub async fn recv(self) -> Result<T, RecvError> {
        let mut val = self.inner.0.lock().await;
        while val.is_none() {
            let val_fut = self.inner.1.wait(val);
            if Arc::weak_count(&self.inner) == 0 {
                return val_fut
                    .now_or_never()
                    .ok_or(RecvError)?
                    .take()
                    .ok_or(RecvError);
            }
            val = val_fut.await;
        }
        Ok(val.take().unwrap())
    }
    /*pub fn try_recv(self) -> Result<T, TryRecvError<T>> {
        // TODO is it possible to eliminate this clone
        let mut res = self.inner.0.try_lock();
        let guard = match &mut res {
            Some(l) => l,
            None => {
                drop(res);
                return Err(TryRecvError::WouldBlock(self));
            }
        };
        if let Some(ret) = guard.take() {
            return Ok(ret);
        }
        if Arc::weak_count(&self.inner) == 0 {
            Err(TryRecvError::Closed)
        } else {
            //Err(TryRecvError::Closed)
            drop(res);
            Err(TryRecvError::WouldBlock(self))
        }
    }*/
}

pub fn one_time_channel<T>() -> (OneSender<T>, OneReceiver<T>) {
    let inner = Arc::new((Mutex::new(None), Condvar::new()));

    let sender = OneSender {
        inner: Arc::downgrade(&inner),
    };
    let recv = OneReceiver { inner };
    (sender, recv)
}
