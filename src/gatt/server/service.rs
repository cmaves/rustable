use std::num::NonZeroU16;
use tokio::sync::mpsc::channel as bounded;
use tokio::task::spawn;

use super::*;
use crate::properties::{PropError, Properties};
use async_rustbus::RpcConn;

/// A GATT service that is to be added to an [`Application`].
pub struct Service {
    chars: Vec<Characteristic>,
    includes: Vec<UUID>,
    handle: u16,
    uuid: UUID,
    primary: bool,
}
struct ServData {
    handle: u16,
    uuid: UUID,
    children: usize,
    includes: Vec<ObjectPathBuf>,
    primary: bool,
}
impl ServData {
    fn handle_call(&mut self, call: &MarshalledMessage) -> MarshalledMessage {
        let interface = call.dynheader.interface.as_ref().unwrap();
        match &**interface {
            PROPS_IF => self.properties_call(call),
            INTRO_IF => self.handle_intro(call),
            _ => unimplemented!(),
        }
    }
    fn handle_intro(&self, call: &MarshalledMessage) -> MarshalledMessage {
        let mut s = String::from(introspect::INTROSPECT_FMT_P1);
        s.push_str(introspect::PROP_STR);
        s.push_str(introspect::SERVICE_STR);
        let children = (0..self.children).map(|u| format!("char{:04x}", u));
        introspect::child_nodes(children, &mut s);
        s.push_str(introspect::INTROSPECT_FMT_P3);
        let mut reply = call.dynheader.make_response();
        reply.body.push_param(s).unwrap();
        reply
    }
}
impl Service {
    /// Create new `Service`.
    ///
    /// A non-`primary` services indicates that it is intended to be included by another service.
    pub fn new(uuid: UUID, primary: bool) -> Self {
        Self {
            uuid,
            primary,
            handle: 0,
            chars: Vec::new(),
            includes: Vec::new(),
        }
    }
    /// Set the handle to requested by the service.
    ///
    /// If this is not set then one will be requested from Bluez.
    /// Setting this can be useful because some remote devices (like Android devices) do not
    /// handle changing handles very well.
    /// When [`Application`] is finally registered, if the requested handles are not unique
    /// or they are already in use Bluez will reject the `Application` registration.
    pub fn set_handle(&mut self, handle: Option<NonZeroU16>) {
        self.handle = handle.map_or(0, |u| u.into());
    }
    pub fn add_char(&mut self, mut character: Characteristic) {
        match self.find_char_unsorted(character.uuid()) {
            Some(c) => std::mem::swap(c, &mut character),
            None => self.chars.push(character),
        }
    }
    pub fn includes(&self) -> &[UUID] {
        &self.includes
    }
    pub fn characteristics(&self) -> &[Characteristic] {
        &self.chars
    }
    pub fn add_includes(&mut self, service: UUID) {
        if !self.includes.contains(&service) {
            self.includes.push(service);
        }
    }
    pub fn remove_includes(&mut self, service: UUID) {
        if let Some(idx) = self.includes.iter().position(|u| *u == service) {
            self.includes.remove(idx);
        }
    }
    pub fn remove_char(&mut self, uuid: UUID) -> Option<Characteristic> {
        let idx = self.chars.iter().position(|s| s.uuid() == uuid)?;
        Some(self.chars.remove(idx))
    }
    /// Get the `UUID` of `Service`.
    pub fn uuid(&self) -> UUID {
        self.uuid
    }
    /// Removes all of the `Characteristic`s in the service and returns then in an iterator.
    ///
    /// Even if not all of the `Characteristic`s are not consumed, they are still removed.
    pub fn drain_chrcs(&mut self) -> std::vec::Drain<Characteristic> {
        self.chars.drain(..)
    }
    pub fn chrc_cnt(&self) -> usize {
        self.chars.len()
    }
    fn find_char_unsorted(&mut self, uuid: UUID) -> Option<&mut Characteristic> {
        self.chars.iter_mut().find(|c| c.uuid() == uuid)
    }
    /*pub fn sort_chars(&mut self) {
        for cha in self.chars.iter_mut() {
            cha.sort_descs();
        }
        self.chars.sort_by_key(|c| c.uuid());
    }*/
    pub(super) fn start_worker(
        self,
        conn: &Arc<RpcConn>,
        path: &ObjectPath,
        children: usize,
        filter: Option<Arc<str>>,
    ) -> Worker {
        let path = path.to_owned();
        let (sender, mut recv) = bounded(8);
        let conn = conn.clone();
        let mut serv_data = ServData {
            uuid: self.uuid,
            handle: self.handle,
            primary: self.primary,
            includes: Vec::new(),
            children,
        };
        let handle = spawn(async move {
            let call_recv = conn.get_call_recv(&*path).await.unwrap();
            let mut call_fut = call_recv.recv();
            loop {
                tokio::select! {
                    biased;

                    opt = recv.recv() => {
                        match opt.ok_or(Error::ThreadClosed)? {
                            WorkerMsg::Unregister => break,
                            WorkerMsg::ObjMgr(sender) => {
                                sender.send(
                                    (path.clone(), serv_data.get_all_interfaces(&path))
                                ).map_err(|_| Error::ThreadClosed)?;
                            }
                            WorkerMsg::GetHandle(sender) => {
                                sender.send(
                                    NonZeroU16::new(self.handle).unwrap()
                                ).map_err(|_| Error::ThreadClosed)?;
                            }
                            WorkerMsg::IncludedPaths(includes) => {
                                serv_data.includes = includes;
                            }
                            _ => unreachable!(),
                        }
                    }
                    res = &mut call_fut => {
                        let call = res?;
                        let res = if is_msg_bluez(&call, filter.as_deref()) {
                            serv_data.handle_call(&call)
                        } else {
                            call.dynheader.make_error_response("PermissionDenied", None)
                        };
                        conn.send_msg_wo_rsp(&res).await?;

                        call_fut = call_recv.recv();
                    }
                }
            }
            Ok(WorkerJoin::Serv(self))
        });
        Worker { sender, handle }
    }
}
/*impl Properties for Application {

}*/
impl Properties for ServData {
    const INTERFACES: &'static [(&'static str, &'static [&'static str])] =
        &[(BLUEZ_SER_IF, &[UUID_STR, PRY_STR, HANDLE_STR, INC_STR])];
    fn get_inner(
        &mut self,
        _path: &ObjectPath,
        interface: &str,
        prop: &str,
    ) -> Result<BluezOptions<'static, 'static>, PropError> {
        if !matches!(interface, BLUEZ_SER_IF) {
            return Err(PropError::InterfaceNotFound);
        }
        match prop {
            UUID_STR => Ok(BluezOptions::OwnedStr(self.uuid.to_string())),
            PRY_STR => Ok(BluezOptions::Bool(self.primary)),
            HANDLE_STR => Ok(BluezOptions::U16(self.handle)),
            INC_STR => Ok(BluezOptions::OwnedPaths(self.includes.clone())),
            _ => Err(PropError::PropertyNotFound),
        }
    }
    fn set_inner(
        &mut self,
        _path: &ObjectPath,
        interface: &str,
        prop: &str,
        val: BluezOptions,
    ) -> Result<(), PropError> {
        if !matches!(interface, BLUEZ_SER_IF) {
            return Err(PropError::InterfaceNotFound);
        }
        match prop {
            UUID_STR => Err(PropError::PermissionDenied),
            PRY_STR => Err(PropError::PermissionDenied),
            HANDLE_STR => match val {
                BluezOptions::U16(handle) => {
                    self.handle = handle;
                    Ok(())
                }
                _ => Err(PropError::InvalidValue),
            },
            _ => Err(PropError::PropertyNotFound),
        }
    }
}
