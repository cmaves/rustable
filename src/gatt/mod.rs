use crate::{Error, UUID};
use nix::poll::{poll, PollFd, PollFlags};
use std::os::unix::io::RawFd;
use std::time::Duration;

mod characteristic;
mod descriptor;
mod service;

//pub use characteristic::{Charactersitic, LocalCharBase, LocalCharactersitic, CharFlags};
pub use characteristic::*;
pub use descriptor::*;
pub use service::*;

/*
pub struct DbusNotifier<'a> {
    bt: Bluetooth,
    bt
}
pub struct SocketNotifier {

}
*/
pub struct NotifyPoller {
    fds: Vec<PollFd>,
    indices: Vec<usize>,
}

impl NotifyPoller {
    pub fn new(polls: &[RawFd]) -> Self {
        let mut fds = Vec::new();
        let mut indices = Vec::new();
        for (i, poll) in polls.iter().enumerate() {
            fds.push(PollFd::new(*poll, PollFlags::POLLIN));
            indices.push(i);
        }
        NotifyPoller { fds, indices }
    }
    pub fn poll(&mut self, timeout: Option<Duration>) -> Result<Vec<usize>, Error> {
        let timeout = if let Some(dur) = timeout {
            if dur.subsec_millis() % 1000 == 0 {
                dur.as_millis().min(std::i32::MAX as u128) as i32
            } else {
                dur.as_millis().min(std::i32::MAX as u128 + 1) as i32
            }
        } else {
            -1
        };
        let mut res = poll(&mut self.fds, timeout)? as usize;
        let mut ret1 = Vec::with_capacity(res);
        if res > 0 {
            for (i, fd) in self.fds.iter().enumerate() {
                if let Some(events) = fd.revents() {
                    if events.contains(PollFlags::POLLIN) {
                        ret1.push(self.indices[i]);
                        if res <= 0 {
                            break;
                        }
                        res -= 1;
                    }
                }
            }
        }
        debug_assert_eq!(res, 0);
        Ok(ret1)
    }
}
