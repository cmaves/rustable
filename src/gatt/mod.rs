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
        let fds: Vec<PollFd> = polls.iter().map(|fd| PollFd::new(*fd, PollFlags::POLLIN | PollFlags::POLLERR)).collect();
        let indices = Vec::with_capacity(fds.len());
        NotifyPoller { fds, indices }
    }
    pub fn poll(&mut self, timeout: Option<Duration>) -> Result<&[usize] ,Error> {
        let timeout = if let Some(dur) = timeout {
            if dur.subsec_millis() % 1000 == 0 {
                dur.as_millis().min(std::i32::MAX as u128) as i32
            } else {
                dur.as_millis().min(std::i32::MAX as u128 + 1) as i32
            }
        } else {
            -1
        };
        self.indices.clear();
        let mut res = poll(&mut self.fds, timeout)? as usize;
        if res > 0 {
            for (i, fd) in self.fds.iter().enumerate() {
                if let Some(events) = fd.revents() {
                    if events.intersects(PollFlags::POLLIN | PollFlags::POLLERR) {
                        self.indices.push(i);
                        res -= 1;
                        if res <= 0 {
                            break;
                        }
                    }
                }
            }
        }
        debug_assert_eq!(res, 0);
        Ok(&self.indices[..])
    }
    pub fn get_ready(&self) -> &[usize] {
        &self.indices[..]
    }
    pub fn get_flags(&self, idx: usize) -> Option<PollFlags> {
        self.fds[idx].revents()
    }
}
