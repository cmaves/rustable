use async_std::channel::bounded;
use async_std::task::spawn;
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
	pub(super) fn start_worker(
		self,
        conn: &Arc<RpcConn>,
        path: ObjectPathBuf,
        children: usize,
        filter: Option<Arc<str>>,
	) -> Worker {
        let (sender, recv) = bounded(8);
        let conn = conn.clone();
        let mut serv_data = ServData {
            uuid: self.uuid,
            handle: self.handle,
            children,
        };
		let handle = spawn(async move {
            let call_recv = conn.get_call_recv(&*path).await.unwrap();
            let mut msg_fut = recv.recv();
            let mut call_fut = call_recv.recv();
            loop {
                match select(msg_fut, call_fut).await {
                    Either::Left((msg, call_f)) => {
                        match msg? {
                            WorkerMsg::Unregister => break,
                            WorkerMsg::ObjMgr(sender) => {
                                sender.send((path.clone(), serv_data.get_all_interfaces(&path)))?;
                            }
                            WorkerMsg::GetHandle(sender) => {
                                sender.send(NonZeroU16::new(self.handle).unwrap())?;
                            }
							_ => unreachable!()
                        }
                        call_fut = call_f;
                        msg_fut = recv.recv();
                    }
                    Either::Right((call, msg_f)) => {
                        let call = call?;
                        let res = if is_msg_bluez(&call, filter.as_deref()) {
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
            Ok(WorkerJoin::Serv(self))
		});
		Worker {
			sender,
			handle
		}
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
