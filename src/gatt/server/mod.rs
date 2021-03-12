use async_std::channel::{bounded, Receiver, Sender};
use async_std::os::unix::net::UnixDatagram;
use async_std::task::{spawn, JoinHandle};
use futures::future::{select, Either};
use futures::pin_mut;
use futures::prelude::*;
use futures::task::{Context, Poll};
use std::convert::Infallible;
use std::num::NonZeroU16;
use std::os::unix::ffi::OsStrExt;
use std::path::Components;
use std::pin::Pin;

use super::{AttValue, CharFlags, ValOrFn};
use crate::*;
use async_rustbus::RpcConn;

mod one_time;

use one_time::{one_time_channel, OneSender};

enum Status {
    Unregistered,
    Registering,
    Registered,
    Unregistering,
}
pub struct Application {
    services: Vec<Service>,
    dest: Option<String>,
    hci: ObjectPathBuf,
    base_path: ObjectPathBuf,
    path: ObjectPathBuf,
}
impl Application {
    fn sort_services(&mut self) {
        for service in self.services.iter_mut() {
            service.sort_chars();
        }
        self.services.sort_by_key(|serv| serv.uuid);
    }
    fn find_serv_unsorted(&mut self, uuid: UUID) -> Option<&mut Service> {
        self.services.iter_mut().find(|s| s.uuid == uuid)
    }
    fn find_serv(&mut self, uuid: UUID) -> Option<&mut Service> {
        let idx = self.services.binary_search_by_key(&uuid, |s| s.uuid).ok()?;
        Some(&mut self.services[idx])
    }
    fn find_char(&mut self, serv: UUID, cha: UUID) -> Option<&mut Characteristic> {
        self.find_serv(serv)?.find_char(cha)
    }
    fn find_desc(&mut self, serv: UUID, cha: UUID, desc: UUID) -> Option<&mut Descriptor> {
        self.find_char(serv, cha)?.find_desc(desc)
    }
    async fn handle_call(
        &mut self,
        conn: &RpcConn,
        call: &MarshalledMessage,
    ) -> std::io::Result<()> {
        let res = match self.match_app(&call) {
            Some(attr_ref) => match attr_ref {
                AttrRef::Prefix(pre, idx) => pre.handle_prefix(&call, idx),
                AttrRef::App(app) => app.handle_app(&call),
                _ => unimplemented!(),
            },
            None => {
                let err_name = String::from("UnknownInterface");
                call.dynheader.make_error_response(err_name, None)
            }
        };

        conn.send_message(&res).await?.await?;
        Ok(())
    }
    async fn begin_reg_call<'a>(
        &self,
        conn: &'a RpcConn,
    ) -> std::io::Result<impl Future<Output = std::io::Result<Option<MarshalledMessage>>> + 'a>
    {
        let call = MessageBuilder::new()
            .call(String::from("RegisterApplication"))
            .at(String::from("org.bluez"))
            .on(String::from(self.hci.clone()))
            .with_interface(String::from(BLUEZ_MGR_IF))
            .build();
        Ok(conn.send_message(&call).await?)
    }
    pub fn add_service(&mut self, mut service: Service) {
        match self.find_serv_unsorted(service.uuid) {
            Some(old) => std::mem::swap(old, &mut service),
            None => self.services.push(service),
        }
    }
    pub fn remove_service(&mut self, uuid: UUID) -> Option<Service> {
        let idx = self.services.iter().position(|s| s.uuid == uuid)?;
        Some(self.services.remove(idx))
    }
    pub fn zero_handles(&mut self) {
        unimplemented!()
    }
    pub async fn register(mut self) -> Result<AppWorker, Error> {
        let conn = RpcConn::system_conn(true).await?;
        self.sort_services();
        let mut res_fut = self.begin_reg_call(&conn).await?.boxed();
        loop {
            let call_fut = conn.get_call();
            pin_mut!(call_fut);
            match select(res_fut, call_fut).await {
                Either::Left((res, _)) => {
                    let res = res?.unwrap();
                    is_msg_err_empty(&res)?;
                    break;
                }
                Either::Right((call, res_f)) => {
                    self.handle_call(&conn, &call?).await?;
                    res_fut = res_f;
                }
            }
        }
        let (sender, recv) = bounded(2);
        Ok(AppWorker {
            worker: spawn(async move {
                let mut recv_fut = recv.recv().boxed();
                let mut call_fut = conn.get_call().boxed();
                loop {
                    match select(recv_fut, call_fut).await {
                        Either::Left((msg, call_f)) => {
                            let msg = msg.unwrap();
                            match msg {
                                WorkerMsg::Unregister => break,
                                WorkerMsg::UpdateChar(serv, cha, val, notify) => {
                                    let cha = self
                                        .find_char(serv, cha)
                                        .ok_or_else(|| Error::UnknownChar(serv, cha))?;
                                    cha.value = val;
                                    if notify {
                                        cha.notify(&conn, None).await?;
                                    }
                                }
                                WorkerMsg::UpdateDesc(serv, cha, desc, val) => {
                                    let desc = self.find_desc(serv, cha, desc).ok_or_else(|| Error::UnknownDesc(serv, cha, desc))?;
                                    desc.value = val;
                                }
                                WorkerMsg::NotifyChar(serv, cha, att_val) => {
                                    let cha = self
                                        .find_char(serv, cha)
                                        .ok_or_else(|| Error::UnknownChar(serv, cha))?;
                                    cha.notify(&conn, att_val).await?;
                                }
                                WorkerMsg::GetChar(serv, cha, sender) => {
                                    let cha = self
                                        .find_char(serv, cha)
                                        .ok_or_else(|| Error::UnknownChar(serv, cha))?;
                                    sender.send(cha.value.to_value()).await.ok();
                                }
                                WorkerMsg::GetDesc(serv, cha, desc, sender) => {
                                    let desc = self.find_desc(serv, cha, desc).ok_or_else(|| Error::UnknownDesc(serv, cha, desc))?;
                                    sender.send(desc.value.to_value()).await.ok();
                                }
                                _ => unimplemented!()
                            }

                            call_fut = call_f;
                            recv_fut = recv.recv().boxed();
                        }
                        Either::Right((call, recv_f)) => {
                            self.handle_call(&conn, &call?).await?;
                            recv_fut = recv_f;
                            call_fut = conn.get_call().boxed();
                        }
                    }
                }
                Ok(self)
            }),
            sender,
        })
    }
    fn handle_prefix(&self, call: &MarshalledMessage, idx: usize) -> MarshalledMessage {
        let mut pre_comps = self.base_path.iter();
        let mut reply = call.dynheader.make_response();
        let mut s = String::from(introspect::INTROSPECT_FMT_P1);
        for comp in pre_comps.by_ref().take(idx - 1) {
            let comp_str = unsafe { std::str::from_utf8_unchecked(comp.as_bytes()) };
            s.push_str(comp_str);
        }
        s.push_str(introspect::INTROSPECT_FMT_P2);
        let child = std::str::from_utf8(pre_comps.next().unwrap().as_bytes()).unwrap();
        introspect::child_nodes(&[child], &mut s);
        s.push_str(introspect::INTROSPECT_FMT_P3);
        reply.body.push_param(s).unwrap();
        reply
    }
    fn handle_app_intro(&self, call: &MarshalledMessage) -> MarshalledMessage {
        let mut reply = call.dynheader.make_response();
        let mut s = String::from(introspect::INTROSPECT_FMT_P1);
        s.push_str(self.base_path.as_str());
        s.push_str(introspect::INTROSPECT_FMT_P2);
        s.push_str(introspect::PROP_STR);
        s.push_str(introspect::MANGAGER_STR);
        let children: Vec<String> = (0..self.services.len()).map(|u| format!("service{:04}", u)).collect();
        introspect::child_nodes(&children, &mut s);
        s.push_str(introspect::INTROSPECT_FMT_P3);
        reply.body.push_param(s).unwrap();
        reply
    }
    fn handle_prop(&self, call: &MarshalledMessage) -> MarshalledMessage {
        unimplemented!()
    }
    fn handle_obj_mgr(&self, call: &MarshalledMessage) -> MarshalledMessage {
        unimplemented!()
    }
    fn handle_app(&self, call: &MarshalledMessage) -> MarshalledMessage {
        match call.dynheader.interface.as_ref().unwrap().as_str() {
            INTRO_IF => self.handle_app_intro(call),
            PROPS_IF => self.handle_prop(call),
            OBJMGR_IF => self.handle_obj_mgr(call),
            _ => unimplemented!()
        }
    }
    fn match_app(&mut self, call: &MarshalledMessage) -> Option<AttrRef<'_>> {
        if !matches!(call.typ, MessageType::Call) {
            return None;
        }
        let interface = call.dynheader.interface.as_ref()?.as_str();
        let tar_path = ObjectPathBuf::try_from(call.dynheader.object.clone().unwrap()).unwrap();
        let mut tar_comps = tar_path.components();
        let mut pre_comps = self.base_path.components().enumerate();
        while let Some((idx, pre_comp)) = pre_comps.next() {
            match tar_comps.next() {
                Some(tar_comp) => {
                    if tar_comp != pre_comp {
                        return None;
                    }
                }
                None => {
                    return if interface == INTRO_IF {
                        Some(AttrRef::Prefix(self, idx))
                    } else {
                        None
                    }
                }
            }
        }
        let tar_comp = match tar_comps.next() {
            Some(tc) => tc,
            None => {
                return if matches!(interface, INTRO_IF | OBJMGR_IF) {
                    Some(AttrRef::App(self))
                } else {
                    None
                }
            }
        };
        let comp_str = unsafe {
            // SAFETY: Dbus paths are always valid UTF-8
            std::str::from_utf8_unchecked(tar_comp.as_os_str().as_bytes())
        };
        if !comp_str.starts_with("service") {
            return None;
        }
        let idx = u16::from_str(comp_str.get(7..)?).ok()? as usize;
        self.services
            .get_mut(idx)?
            .match_service(tar_comps, interface)
    }
}

enum AttrRef<'a> {
    Prefix(&'a mut Application, usize),
    App(&'a mut Application),
    Serv(&'a mut Service),
    Char(&'a mut Characteristic),
    Desc(&'a mut Descriptor),
}

pub struct AppWorker {
    worker: JoinHandle<Result<Application, Error>>,
    sender: Sender<WorkerMsg>,
}
impl AppWorker {
    pub async fn unregister(self) -> Result<Application, Error> {
        self.sender.send(WorkerMsg::Unregister).await?;
        self.worker.await
    }
    pub async fn update_characteristic(
        &self,
        service: UUID,
        character: UUID,
        val: ValOrFn,
        notify: bool,
    ) -> Result<(), Error> {
        self.sender
            .send(WorkerMsg::UpdateChar(service, character, val, notify))
            .await?;
        Ok(())
    }
    pub async fn update_descriptor(
        &self,
        service: UUID,
        character: UUID,
        descriptor: UUID,
        val: ValOrFn,
    ) -> Result<(), Error> {
        self.sender
            .send(WorkerMsg::UpdateDesc(service, character, descriptor, val))
            .await?;
        Ok(())
    }
    pub async fn notify_char(
        &self,
        service: UUID,
        character: UUID,
        val: Option<AttValue>,
    ) -> Result<(), Error> {
        self.sender
            .send(WorkerMsg::NotifyChar(service, character, val))
            .await?;
        Ok(())
    }
    pub async fn get_char(&mut self, serv: UUID, cha: UUID) -> Result<AttValue, Error> {
        let (sender, recv) = one_time_channel();
        self.sender
            .send(WorkerMsg::GetChar(serv, cha, sender))
            .await?;
        let res = recv.await?;
        Ok(res)
    }
    pub async fn get_serv_handle(&mut self, serv: UUID) -> Result<NonZeroU16, Error> {
        let (sender, recv) = one_time_channel();
        self.sender
            .send(WorkerMsg::GetServHandle(serv, sender))
            .await?;
        let res = recv.await?;
        Ok(res)
    }
    pub async fn get_char_handle(&mut self, serv: UUID, cha: UUID) -> Result<NonZeroU16, Error> {
        let (sender, recv) = one_time_channel();
        self.sender
            .send(WorkerMsg::GetCharHandle(serv, cha, sender))
            .await?;
        let res = recv.await?;
        Ok(res)
    }
    pub async fn get_desc(&mut self, serv: UUID, cha: UUID, desc: UUID) -> Result<AttValue, Error> {
        let (sender, recv) = one_time_channel();
        self.sender
            .send(WorkerMsg::GetDesc(serv, cha, desc, sender))
            .await?;
        let res = recv.await?;
        Ok(res)
    }
    pub async fn get_desc_handle(&mut self, serv: UUID, cha: UUID, desc: UUID) -> Result<NonZeroU16, Error> {
        let (sender, recv) = one_time_channel();
        self.sender
            .send(WorkerMsg::GetDescHandle(serv, cha, desc, sender))
            .await?;
        let res = recv.await?;
        Ok(res)
    }
}

enum WorkerMsg {
    Unregister,
    UpdateChar(UUID, UUID, ValOrFn, bool),
    NotifyChar(UUID, UUID, Option<AttValue>),
    UpdateDesc(UUID, UUID, UUID, ValOrFn),
    GetServHandle(UUID, OneSender<NonZeroU16>),
    GetChar(UUID, UUID, OneSender<AttValue>),
    GetCharHandle(UUID, UUID, OneSender<NonZeroU16>),
    GetDesc(UUID, UUID, UUID, OneSender<AttValue>),
    GetDescHandle(UUID, UUID, UUID, OneSender<NonZeroU16>),
}

pub struct Service {
    chars: Vec<Characteristic>,
    handle: u16,
    uuid: UUID,
}
impl Service {
    pub fn new(uuid: UUID, handle: Option<NonZeroU16>) -> Self {
        Self {
            uuid,
            handle: handle.map_or(0, |u| u.into()),
            chars: Vec::new(),
        }
    }
    pub fn set_handle(&mut self, handle: Option<NonZeroU16>) {
        self.handle = handle.map_or(0, |u| u.into());
    }
    fn find_char(&mut self, uuid: UUID) -> Option<&mut Characteristic> {
        let idx = self.chars.binary_search_by_key(&uuid, |s| s.uuid).ok()?;
        Some(&mut self.chars[idx])
    }
    fn sort_chars(&mut self) {
        unimplemented!()
    }
    fn match_service(
        &mut self,
        mut tar_comps: Components<'_>,
        interface: &str,
    ) -> Option<AttrRef<'_>> {
        let tar_comp = match tar_comps.next() {
            Some(tc) => tc,
            None => {
                return if matches!(interface, PROPS_IF | BLUEZ_SER_IF | INTRO_IF) {
                    Some(AttrRef::Serv(self))
                } else {
                    None
                }
            }
        };
        let comp_str = unsafe {
            // SAFETY: Dbus paths are always valid UTF-8
            std::str::from_utf8_unchecked(tar_comp.as_os_str().as_bytes())
        };
        if !comp_str.starts_with("char") {
            return None;
        }
        let idx = u16::from_str(comp_str.get(4..)?).ok()? as usize;
        self.chars.get_mut(idx)?.match_char(tar_comps, interface)
    }
}
enum Notify {
    Socket(UnixDatagram),
    Signal,
    None,
}
pub struct Characteristic {
    descs: Vec<Descriptor>,
    value: ValOrFn,
    uuid: UUID,
    notify: Notify,
    flags: CharFlags,
    handle: u16,
}
impl Characteristic {
    pub fn new(uuid: UUID, handle: Option<NonZeroU16>, flags: CharFlags) -> Self {
        Self {
            uuid,
            flags,
            descs: Vec::new(),
            handle: handle.map_or(0, |u| u.into()),
            value: ValOrFn::default(),
            notify: Notify::None,
        }
    }
    pub fn set_value(&mut self, value: ValOrFn) {
        self.value = value;
    }
    async fn notify(&mut self, conn: &RpcConn, opt_att: Option<AttValue>) -> std::io::Result<()> {
        let att_val = opt_att.ok_or(|| self.value.to_value());
        match self.notify {
            Notify::None => Ok(()),
            _ => unimplemented!(),
        }
    }
    fn match_char(
        &mut self,
        mut tar_comps: Components<'_>,
        interface: &str,
    ) -> Option<AttrRef<'_>> {
        let tar_comp = match tar_comps.next() {
            Some(tc) => tc,
            None => {
                return if matches!(interface, PROPS_IF | BLUEZ_CHR_IF | INTRO_IF) {
                    Some(AttrRef::Char(self))
                } else {
                    None
                }
            }
        };
        let comp_str = unsafe {
            // SAFETY: Dbus paths are always valid UTF-8
            std::str::from_utf8_unchecked(tar_comp.as_os_str().as_bytes())
        };
        if !comp_str.starts_with("desc") {
            return None;
        }
        let idx = u16::from_str(comp_str.get(4..)?).ok()? as usize;
        if matches!(interface, PROPS_IF | BLUEZ_DES_IF | INTRO_IF) {
            Some(AttrRef::Desc(self.descs.get_mut(idx)?))
        } else {
            None
        }
    }
    fn find_desc(&mut self, uuid: UUID) -> Option<&mut Descriptor> {
        let idx = self.descs.binary_search_by_key(&uuid, |s| s.uuid).ok()?;
        Some(&mut self.descs[idx])
    }
}

struct Descriptor {
    value: ValOrFn,
    uuid: UUID,
    name: String,
}
