use async_std::channel::{bounded, Sender};
use async_std::os::unix::net::UnixDatagram;
use async_std::task::{spawn, JoinHandle};
use futures::future::{select, Either};
use futures::pin_mut;
use futures::prelude::*;
use std::collections::HashMap;
use std::num::NonZeroU16;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::io::{AsRawFd, IntoRawFd};
use std::path::Components;

use super::*;
use crate::properties::{PropError, Properties};
use crate::*;
use async_rustbus::rustbus_core::message_builder::MessageBuilder;
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
    fn handle_call(&mut self, call: &MarshalledMessage) -> MarshalledMessage {
        let interface = call.dynheader.interface.as_ref().unwrap();
        match &**interface {
            PROPS_IF => self.properties_call(call),
            INTRO_IF => self.handle_intro(call),
            BLUEZ_DES_IF => unimplemented!(),
            _ => unreachable!(),
        }
    }
    fn handle_intro(&self, call: &MarshalledMessage) -> MarshalledMessage {
        let mut s = String::from(introspect::INTROSPECT_FMT_P1);
        s.push_str(introspect::PROP_STR);
        s.push_str(introspect::SERVICE_STR);
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
pub enum DescMsg {
    Shutdown,
    DbusCall(MarshalledMessage),
    Update(ValOrFn),
    Get(OneSender<AttValue>),
    GetHandle(OneSender<NonZeroU16>),
    ObjMgr(OneSender<(ObjectPathBuf, HashMap<&'static str, HashMap<&'static str, BluezOptions<'static, 'static>>>)>)
}
pub struct DescWorker {
    worker: JoinHandle<Result<Descriptor, Error>>,
    sender: Sender<DescMsg>,
    uuid: UUID,
}
impl DescWorker {
    pub fn new(mut desc: Descriptor, conn: &Arc<RpcConn>, path: ObjectPathBuf) -> Self {
        let (sender, recv) = bounded(8);
        let conn = conn.clone();
        let uuid = desc.uuid;
        let worker = spawn(async move {
            loop {
                let msg = recv.recv().await?;
                match msg {
                    DescMsg::Shutdown => break,
                    DescMsg::DbusCall(call) => {
                        let res = desc.handle_call(&call);
                        conn.send_message(&res).await?.await?;
                    }
                    DescMsg::Update(vf) => {
                        desc.value = vf;
                    }
                    DescMsg::Get(sender) => {
                        sender.send(desc.value.to_value()).await?;
                    }
                    DescMsg::GetHandle(sender) => {
                        sender.send(NonZeroU16::new(desc.handle).unwrap()).await?;
                    }
                    DescMsg::ObjMgr(sender) => {
                        sender.send((path.clone(), desc.get_all_interfaces(&path))).await?;
                    }
                };
            }
            Ok(desc)
        });
        DescWorker {
            worker,
            sender,
            uuid
        }
    }
    pub fn uuid(&self) -> UUID { self.uuid }
    pub async fn send(&self, msg: DescMsg) -> Result<(), Error> {
        self.sender.send(msg).await?;
        Ok(())
    }
}
