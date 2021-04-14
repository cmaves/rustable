use async_std::channel::bounded;
use async_std::task::spawn;
use futures::future::{select, Either};
use std::collections::HashMap;

use super::*;
use crate::properties::{PropError, Properties};
use async_rustbus::RpcConn;

pub struct Descriptor {
    value: ValOrFn,
    uuid: UUID,
    flags: DescFlags,
    handle: u16,
}

impl Descriptor {
    pub fn new(uuid: UUID, flags: DescFlags) -> Self {
        Self {
            uuid,
            flags,
            handle: 0,
            value: ValOrFn::default(),
        }
    }
    pub fn set_value(&mut self, vf: ValOrFn) {
        self.value = vf;
    }
    pub fn set_handle(&mut self, handle: Option<NonZeroU16>) {
        self.handle = handle.map_or(0, |h| h.into());
    }
    pub fn uuid(&self) -> UUID {
        self.uuid
    }
	pub(super) fn start_worker(
		mut self,
        conn: &Arc<RpcConn>,
        path: ObjectPathBuf,
        filter: Option<Arc<str>>
	) -> Worker {
        let (sender, msg_recv) = bounded(8);
		let conn = conn.clone();
		let handle = spawn(async move {
            let call_recv = conn.get_call_recv(&*path).await.unwrap();
            let mut call_fut = call_recv.recv();
            let mut msg_fut = msg_recv.recv();
            loop {
                match select(msg_fut, call_fut).await {
                    Either::Left((msg, call_f)) => {
                        match msg? {
							WorkerMsg::Unregister=> break,
							WorkerMsg::Update(vf, _) => {
								self.value = vf;
							}
                            WorkerMsg::Get(sender) => {
                                sender.send(self.value.to_value())?;
                            }
                            WorkerMsg::GetHandle(sender) => {
                                sender.send(NonZeroU16::new(self.handle).unwrap())?;
                            }
                            WorkerMsg::ObjMgr(sender) => {
                                sender.send((path.clone(), self.get_all_interfaces(&path)))?;
                            }
							WorkerMsg::Notify(_) => unreachable!()
                        }
                        call_fut = call_f;
                        msg_fut = msg_recv.recv();
                    }
                    Either::Right((call, msg_f)) => {
                        let call = call?;
                        let res = if is_msg_bluez(&call, filter.as_deref()) {
                            self.handle_call(&call)
                        } else {
                            call.dynheader.make_error_response("PermissionDenied", None)
                        };
                        conn.send_message(&res).await?;
                        msg_fut = msg_f;
                        call_fut = call_recv.recv();
                    }
                }
            }
            Ok(WorkerJoin::Desc(self))
		});
		Worker {
			handle,
			sender
		}
	}
    fn handle_call(&mut self, call: &MarshalledMessage) -> MarshalledMessage {
        let interface = call.dynheader.interface.as_ref().unwrap();
        match &**interface {
            PROPS_IF => self.properties_call(call),
            INTRO_IF => self.handle_intro(call),
            BLUEZ_DES_IF => {
                let member = call.dynheader.member.as_ref().unwrap();
                match &**member {
                    "ReadValue" => {
                        if !(self.flags.read
                            || self.flags.secure_read
                            || self.flags.encrypt_read
                            || self.flags.encrypt_auth_read)
                        {
                            return call.dynheader.make_error_response("PermissionDenied", None);
                        }
                        let options: HashMap<&str, BluezOptions> = match call.body.parser().get() {
                            Ok(o) => o,
                            Err(_) => {
                                return call.dynheader.make_error_response("UnknownType", None);
                            }
                        };
                        let mut offset = 0;
                        if let Some(BluezOptions::U16(off)) = options.get("offset") {
                            offset = *off as usize;
                        }
                        let att_val = self.value.to_value();
                        let val = att_val.get(offset..).unwrap_or(&[]);
                        let mut reply = call.dynheader.make_response();
                        reply.body.push_param(val).unwrap();
                        reply
                    }
                    "WriteValue" => {
                        if !(self.flags.write
                            || self.flags.encrypt_write
                            || self.flags.encrypt_auth_write)
                        {
                            return call.dynheader.make_error_response("PermissionDenied", None);
                        }
                        let (mut att_val, options): (AttValue, HashMap<&str, BluezOptions>) =
                            match call.body.parser().get() {
                                Ok(o) => o,
                                Err(_) => {
                                    return call.dynheader.make_error_response("UnknownType", None);
                                }
                            };
                        let mut offset = 0;
                        if let Some(BluezOptions::U16(off)) = options.get("offset") {
                            offset = *off as usize;
                        }
                        if offset != 0 {
                            let mut old = self.value.to_value();
                            old.update(&att_val, offset);
                            att_val = old;
                        }
                        self.value = ValOrFn::Value(att_val);
                        call.dynheader.make_response()
                    }
                    _ => call.dynheader.make_error_response("UnknownMethod", None),
                }
            }
            _ => unreachable!(),
        }
    }
    fn handle_intro(&self, call: &MarshalledMessage) -> MarshalledMessage {
        let mut s = String::from(introspect::INTROSPECT_FMT_P1);
        s.push_str(introspect::PROP_STR);
        s.push_str(introspect::DESC_STR);
        s.push_str(introspect::INTROSPECT_FMT_P3);
        let mut reply = call.dynheader.make_response();
        reply.body.push_param(s).unwrap();
        reply
    }
}
impl Properties for Descriptor {
    const INTERFACES: &'static [(&'static str, &'static [&'static str])] = &[(
        BLUEZ_DES_IF,
        &[UUID_STR, HANDLE_STR, CHAR_STR, VAL_STR, FLAG_STR],
    )];
    fn get_inner(
        &mut self,
        path: &ObjectPath,
        interface: &str,
        prop: &str,
    ) -> Result<BluezOptions<'static, 'static>, PropError> {
        eprintln!("Descriptor::get_inner(): interface: {}", interface);
        if !matches!(interface, BLUEZ_DES_IF) {
            return Err(PropError::InterfaceNotFound);
        }
        match prop {
            UUID_STR => Ok(BluezOptions::OwnedStr(self.uuid.to_string())),
            HANDLE_STR => Ok(BluezOptions::U16(self.handle)),
            CHAR_STR => Ok(BluezOptions::OwnedPath(path.parent().unwrap().into())),
            VAL_STR => Ok(BluezOptions::OwnedBuf((&*self.value.to_value()).into())),
            FLAG_STR => Ok(BluezOptions::Flags(self.flags.to_strings())),
            _ => Err(PropError::PropertyNotFound),
        }
    }
    fn set_inner(
        &mut self,
        _p: &ObjectPath,
        interface: &str,
        prop: &str,
        val: BluezOptions,
    ) -> Result<(), PropError> {
        if !matches!(interface, BLUEZ_CHR_IF) {
            return Err(PropError::InterfaceNotFound);
        }
        match prop {
            HANDLE_STR => match val {
                BluezOptions::U16(h) => {
                    self.handle = h;
                    Ok(())
                }
                _ => Err(PropError::InvalidValue),
            },
            UUID_STR | CHAR_STR | VAL_STR | FLAG_STR => Err(PropError::PermissionDenied),
            _ => Err(PropError::PropertyNotFound),
        }
    }
}
