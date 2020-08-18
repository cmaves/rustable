//! Module containing structures and traits for interacting with remote
//! GATT services/characteristics/descriptors and creating local GATT services.
use crate::{Error, ToUUID, UUID};
use nix::poll::{poll, PollFd, PollFlags};
use std::os::unix::io::RawFd;
use std::path::Path;
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
        let fds: Vec<PollFd> = polls
            .iter()
            .map(|fd| PollFd::new(*fd, PollFlags::POLLIN | PollFlags::POLLERR))
            .collect();
        let indices = Vec::with_capacity(fds.len());
        NotifyPoller { fds, indices }
    }
    pub fn poll(&mut self, timeout: Option<Duration>) -> Result<&[usize], Error> {
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

pub trait GattDbusObject {
    fn path(&self) -> &Path;
    fn uuid(&self) -> &UUID;
}
impl<T: GattDbusObject> GattDbusObject for &T {
    fn path(&self) -> &Path {
        T::path(self)
    }
    fn uuid(&self) -> &UUID {
        T::uuid(self)
    }
}
impl<T: GattDbusObject> GattDbusObject for &mut T {
    fn path(&self) -> &Path {
        T::path(self)
    }
    fn uuid(&self) -> &UUID {
        T::uuid(self)
    }
}
fn match_char(gdo: &mut LocalCharBase, path: &Path) -> Option<Option<UUID>> {
    match path.strip_prefix(gdo.path().file_name().unwrap()) {
        Ok(remaining) => {
            if remaining == Path::new("") {
                Some(None)
            } else {
                let r_str = remaining.to_str().unwrap();
                if r_str.len() != 8 || &r_str[..4] != "desc" {
                    return None;
                }
                for uuid in gdo.get_children() {
                    if match_object(&gdo.get_child(&uuid).unwrap(), remaining) {
                        return Some(Some(uuid));
                    }
                }
                None
            }
        }
        Err(_) => None,
    }
}
pub fn match_serv(gdo: &mut LocalServiceBase, path: &Path) -> Option<Option<(UUID, Option<UUID>)>> {
    match path.strip_prefix(gdo.path().file_name().unwrap()) {
        Ok(remaining) => {
            if remaining == Path::new("") {
                Some(None)
            } else {
                let r_str = remaining.to_str().unwrap();
                if (r_str.len() != 8 && r_str.len() != 17) || &r_str[..4] != "char" {
                    return None;
                }
                for uuid in gdo.get_children() {
                    if let Some(matc) = match_char(&mut gdo.get_child(&uuid).unwrap(), remaining) {
                        return Some(Some((uuid, matc)));
                    }
                }
                None
            }
        }
        Err(_) => None,
    }
}
fn match_remote_char(gdo: &mut RemoteCharBase, path: &Path) -> Option<(UUID, Option<UUID>)> {
    match path.strip_prefix(gdo.path().file_name().unwrap()) {
        Ok(remaining) => {
            if remaining == Path::new("") {
                Some((gdo.uuid().clone(), None))
            } else {
                for uuid in gdo.get_children() {
                    if match_object(&gdo.get_child(&uuid).unwrap(), remaining) {
                        return Some((gdo.uuid().clone(), Some(uuid)));
                    }
                }
                None
            }
        }
        Err(_) => None,
    }
}
fn match_remote_serv(
    gdo: &mut RemoteServiceBase,
    path: &Path,
) -> Option<(UUID, Option<(UUID, Option<UUID>)>)>
//U: HasChildren<'b> + GattDbusObject
{
    match path.strip_prefix(gdo.path().file_name().unwrap()) {
        Ok(remaining) => {
            if remaining == Path::new("") {
                Some((gdo.uuid().clone(), None))
            } else {
                for uuid in gdo.get_children() {
                    if let Some(matc) =
                        match_remote_char(&mut gdo.get_child(&uuid).unwrap(), remaining)
                    {
                        return Some((gdo.uuid().clone(), Some(matc)));
                    }
                }
                None
            }
        }
        Err(_) => None,
    }
}

fn match_object<T: GattDbusObject>(gdo: &T, path: &Path) -> bool {
    gdo.path().file_name().unwrap() == path
}

pub trait HasChildren<'a> {
    type Child: GattDbusObject;
    fn get_children(&self) -> Vec<UUID>;
    fn get_child<T: ToUUID>(&'a mut self, uuid: T) -> Option<Self::Child>;
}
/*
pub trait HasChildrenMut<'a>: HasChildren<'a> {
    fn get_child<T: ToUUID>(&'a mut self, uuid: T) -> Option<Self::Child>;

}
*/
impl<'a, U: HasChildren<'a>> HasChildren<'a> for &mut U {
    type Child = U::Child;
    fn get_children(&self) -> Vec<UUID> {
        U::get_children(self)
    }
    fn get_child<T: ToUUID>(&'a mut self, uuid: T) -> Option<Self::Child> {
        U::get_child(self, uuid)
    }
}

#[cfg(test)]
mod tests {
    use crate::gatt::{match_char, match_serv};
    use crate::gatt::{CharFlags, DescFlags, LocalCharBase, LocalDescBase, LocalServiceBase};
    use crate::ToUUID;
    use std::path::{Path, PathBuf};
    #[test]
    fn test_match_char() {
        let desc1_uuid = "00000000-0000-0000-0000-000000000001".to_uuid();
        let mut desc1 = LocalDescBase::new(&desc1_uuid, DescFlags::default());
        desc1.path = PathBuf::from("char0001/desc0001");

        let char1_uuid = "00000000-0000-0000-0001-000000000000".to_uuid();
        let mut char1 = LocalCharBase::new(&char1_uuid, CharFlags::default());
        char1.path = PathBuf::from("char0001");
        char1.add_desc(desc1);

        assert_eq!(
            match_char(&mut char1, Path::new("char0001")),
            Some((char1_uuid.clone(), None))
        );
        assert_eq!(
            match_char(&mut char1, Path::new("char0001/desc0001")),
            Some((char1_uuid.clone(), Some(desc1_uuid)))
        );
        assert_eq!(match_char(&mut char1, Path::new("char0002")), None);
        assert_eq!(match_char(&mut char1, Path::new("char0001/desc0002")), None);
    }
    #[test]
    fn test_match_serv() {
        let desc1_uuid = "00000000-0000-0000-0000-000000000001".to_uuid();
        let mut desc1 = LocalDescBase::new(&desc1_uuid, DescFlags::default());
        desc1.path = PathBuf::from("serv0001/char0001/desc0001");

        let char1_uuid = "00000000-0000-0000-0001-000000000000".to_uuid();
        let mut char1 = LocalCharBase::new(&char1_uuid, CharFlags::default());
        char1.path = PathBuf::from("serv0001/char0001");
        char1.add_desc(desc1);

        let serv1_uuid = "00000000-0000-0001-0000-000000000000".to_uuid();
        let mut serv1 = LocalServiceBase::new(&serv1_uuid, true);
        serv1.path = PathBuf::from("serv0001");
        serv1.add_char(char1);
        assert_eq!(
            match_serv(&mut serv1, Path::new("serv0001")),
            Some((serv1_uuid.clone(), None))
        );
        assert_eq!(match_serv(&mut serv1, Path::new("char0001")), None);
        assert_eq!(
            match_serv(&mut serv1, Path::new("serv0001/char0001")),
            Some((serv1_uuid.clone(), Some((char1_uuid.clone(), None))))
        );
        assert_eq!(
            match_serv(&mut serv1, Path::new("serv000001/char0002")),
            None
        );
        assert_eq!(
            match_serv(&mut serv1, Path::new("serv0001/char0001/desc0001")),
            Some((
                serv1_uuid.clone(),
                Some((char1_uuid.clone(), Some(desc1_uuid)))
            ))
        );
        assert_eq!(
            match_serv(&mut serv1, Path::new("serv0001/char0001/desc0002")),
            None
        );
    }
}
