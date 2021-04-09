use async_std::channel::{bounded, Sender};
use async_std::task::{spawn, JoinHandle};
use std::collections::HashMap;
use std::num::NonZeroU16;

use super::*;
use crate::properties::{PropError, Properties};
use async_rustbus::RpcConn;

pub struct Service {
    chars: Vec<Characteristic>,
    handle: u16,
    uuid: UUID,
}
struct ServData {
    handle: u16,
    uuid: UUID,
    children: usize,
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
    pub fn new(uuid: UUID) -> Self {
        Self {
            uuid,
            handle: 0,
            chars: Vec::new(),
        }
    }
    pub fn set_handle(&mut self, handle: Option<NonZeroU16>) {
        self.handle = handle.map_or(0, |u| u.into());
    }
    pub fn add_char(&mut self, mut character: Characteristic) {
        match self.find_char_unsorted(character.uuid()) {
            Some(c) => std::mem::swap(c, &mut character),
            None => self.chars.push(character),
        }
    }
    pub fn remove_char(&mut self, uuid: UUID) -> Option<Characteristic> {
        let idx = self.chars.iter().position(|s| s.uuid() == uuid)?;
        Some(self.chars.remove(idx))
    }
    pub fn uuid(&self) -> UUID {
        self.uuid
    }
    pub fn drain_chrcs(&mut self) -> std::vec::Drain<Characteristic> {
        self.chars.drain(..)
    }
    fn find_char_unsorted(&mut self, uuid: UUID) -> Option<&mut Characteristic> {
        self.chars.iter_mut().find(|c| c.uuid() == uuid)
    }
    pub fn sort_chars(&mut self) {
        for cha in self.chars.iter_mut() {
            cha.sort_descs();
        }
        self.chars.sort_by_key(|c| c.uuid());
    }
}
/*impl Properties for Application {

}*/
impl Properties for ServData {
    const INTERFACES: &'static [(&'static str, &'static [&'static str])] =
        &[(BLUEZ_SER_IF, &[UUID_STR, PRY_STR, HANDLE_STR])];
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
            PRY_STR => Ok(BluezOptions::Bool(true)),
            HANDLE_STR => Ok(BluezOptions::U16(self.handle)),
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

pub enum ServMsg {
    Shutdown,
    GetHandle(OneSender<NonZeroU16>),
    ObjMgr(
        OneSender<(
            ObjectPathBuf,
            HashMap<&'static str, HashMap<&'static str, BluezOptions<'static, 'static>>>,
        )>,
    ),
}
pub struct ServWorker {
    worker: JoinHandle<Result<Service, Error>>,
    sender: Sender<ServMsg>,
    uuid: UUID,
}
impl ServWorker {
    pub fn new(
        serv: Service,
        conn: &Arc<RpcConn>,
        path: ObjectPathBuf,
        children: usize,
        filter: Option<String>,
    ) -> Self {
        let (sender, recv) = bounded(8);
        let conn = conn.clone();
        let uuid = serv.uuid;
        let mut serv_data = ServData {
            uuid: serv.uuid,
            handle: serv.handle,
            children,
        };
        let worker = spawn(async move {
            let call_recv = conn.get_call_recv(&*path).await.unwrap();
            let mut msg_fut = recv.recv();
            let mut call_fut = call_recv.recv();
            loop {
                match select(msg_fut, call_fut).await {
                    Either::Left((msg, call_f)) => {
                        match msg? {
                            ServMsg::Shutdown => break,
                            ServMsg::ObjMgr(sender) => {
                                sender.send((path.clone(), serv_data.get_all_interfaces(&path)))?;
                            }
                            ServMsg::GetHandle(sender) => {
                                sender.send(NonZeroU16::new(serv.handle).unwrap())?;
                            }
                        }
                        call_fut = call_f;
                        msg_fut = recv.recv();
                    }
                    Either::Right((call, msg_f)) => {
                        let call = call?;
                        let res = if is_msg_bluez(&call, &filter) {
                            serv_data.handle_call(&call)
                        } else {
                            call.dynheader.make_error_response("PermissionDenied", None)
                        };
                        conn.send_msg_no_reply(&res).await?;
                        msg_fut = msg_f;
                        call_fut = call_recv.recv();
                    }
                }
            }
            Ok(serv)
        });
        ServWorker {
            worker,
            sender,
            uuid,
        }
    }
    pub fn uuid(&self) -> UUID {
        self.uuid
    }
    pub async fn send(&self, msg: ServMsg) -> Result<(), Error> {
        self.sender.send(msg).await?;
        Ok(())
    }
    pub async fn shutdown(self) -> Result<Service, Error> {
        self.sender.send(ServMsg::Shutdown).await?;
        self.worker.await
    }
}
