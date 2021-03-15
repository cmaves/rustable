use async_std::channel::{bounded, Sender};
use async_std::os::unix::net::UnixDatagram;
use async_std::task::{spawn, JoinHandle, Context, Poll};
use futures::future::{select, Either, try_join_all};
use futures::pin_mut;
use futures::prelude::*;
use std::collections::HashMap;
use std::num::NonZeroU16;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::io::{AsRawFd, IntoRawFd};
use std::path::Components;
use std::pin::Pin;

use super::*;
use crate::properties::{PropError, Properties};
use crate::*;
use async_rustbus::rustbus_core::message_builder::MessageBuilder;
use async_rustbus::RpcConn;
use futures_enum::*;

mod one_time;
use one_time::{one_time_channel, OneSender, OneReceiver};

mod chrc;
pub use chrc::Characteristic;
use chrc::*;

mod service;
pub use service::Service;
use service::*;

mod descriptor;
pub use descriptor::Descriptor;
use descriptor::*;

pub struct Application {
    services: Vec<Service>,
    dest: Option<String>,
    hci: ObjectPathBuf,
    base_path: ObjectPathBuf,
    filter: bool,
}

struct WorkerData {
    heirarchy: WorkerHeirarchy,
    base_path: ObjectPathBuf
}
impl WorkerData {
    fn find_serv(&mut self, uuid: UUID) -> Option<&mut ServWorker> {
        let idx = self.heirarchy.binary_search_by_key(&uuid, |(s,_)| s.uuid()).ok()?;
        Some(&mut self.heirarchy[idx].0)
    }
    fn find_serv_chrc(&mut self, serv: UUID, chrc: UUID) 
        -> Option<&mut ChrcWorker> 
    {
        let idx = self.heirarchy.binary_search_by_key(&serv, |(s,_)| s.uuid()).ok()?;
        Some(&mut find_chrc(&mut self.heirarchy[idx].1, chrc)?.0)
    }
    fn find_serv_chrc_desc(&mut self, serv: UUID, chrc: UUID, desc: UUID)
        -> Option<&mut DescWorker> 
    {
        let idx = self.heirarchy.binary_search_by_key(&serv, |(s,_)| s.uuid()).ok()?;
        let desc_workers = &mut find_chrc(&mut self.heirarchy[idx].1, chrc)?.1;
        find_desc(desc_workers, desc)
    }
    async fn handle_call(
        &mut self, 
        conn: &RpcConn, 
        call: MarshalledMessage,
    ) -> Result<(), Error> {
        match self.match_app(&call) {
            Ok(attr_ref) => match attr_ref {
                AttrRef::Prefix(pre, idx) => pre.handle_prefix(&call, idx, conn).await?,
                AttrRef::App(app) => app.handle_app(&call, conn).await?,
                AttrRef::Serv(service) => service.send(ServMsg::DbusCall(call)).await?,
                AttrRef::Chrc(chrc) => chrc.send(ChrcMsg::DbusCall(call)).await?,
                AttrRef::Desc(desc) => desc.send(DescMsg::DbusCall(call)).await?,
            },
            Err(e) => {
                let res = call.dynheader.make_error_response(e.to_string(), None);
                conn.send_message(&res).await?.await?;
            }
        }
        Ok(())
    }
    fn match_app<'a>(
        &'a mut self,
        call: &MarshalledMessage,
    ) -> Result<AttrRef<'a>, &'static str> {
        debug_assert!(matches!(call.typ, MessageType::Call));
        eprintln!("match_app()");
        let interface = call.dynheader.interface.as_ref().unwrap().as_str();
        let tar_path = ObjectPathBuf::try_from(call.dynheader.object.clone().unwrap()).unwrap();
        let mut tar_comps = tar_path.components();
        let mut pre_comps = self.base_path.components().enumerate();
        while let Some((idx, pre_comp)) = pre_comps.next() {
            match tar_comps.next() {
                Some(tar_comp) => {
                    if tar_comp != pre_comp {
                        return Err("UnknownObject");
                    }
                }
                None => {
                    return if interface == INTRO_IF {
                        Ok(AttrRef::Prefix(self, idx - 1))
                    } else {
                        Err("UnknownInterface")
                    }
                }
            }
        }
        eprintln!("match_app(): 2");
        let tar_comp = match tar_comps.next() {
            Some(tc) => tc,
            None => {
                return if matches!(interface, INTRO_IF | OBJMGR_IF) {
                    Ok(AttrRef::App(self))
                } else {
                    Err("UnknownInterface")
                }
            }
        };
        eprintln!("match_app(): 3");
        let comp_str = unsafe {
            // SAFETY: Dbus paths are always valid UTF-8
            std::str::from_utf8_unchecked(tar_comp.as_os_str().as_bytes())
        };
        eprintln!("match_app(): 4");
        if !comp_str.starts_with("service") {
            return Err("UnknownObject");
        }
        let child = |serv_workers: ServWorkerSlice<'a>| {
            let idx = u16::from_str_radix(comp_str.get(7..)?, 16).ok()? as usize;
            eprintln!("match_app(): idx: {}, len(): {}", idx, serv_workers.len());
            serv_workers.get_mut(idx)
        };
        let (serv, chrc_workers) = child(&mut self.heirarchy).ok_or("UnknownObject")?;
        serv.match_service(tar_comps, interface, chrc_workers)
    }
    async fn handle_app(
        &mut self,
        call: &MarshalledMessage,
        conn: &RpcConn,
    ) -> Result<(), Error> {
        let reply = match call.dynheader.interface.as_ref().unwrap().as_str() {
            INTRO_IF => self.handle_app_intro(call),
            //PROPS_IF => self.handle_prop(call),
            OBJMGR_IF => self.handle_obj_mgr(call).await?,
            _ => unimplemented!(),
        };
        conn.send_message(&reply).await?.await?;
        Ok(())
    }
    fn handle_app_intro(&self, call: &MarshalledMessage) -> MarshalledMessage {
        let mut reply = call.dynheader.make_response();
        let mut s = String::from(introspect::INTROSPECT_FMT_P1);
        //s.push_str(self.base_path.as_str());
        //s.push_str(introspect::INTROSPECT_FMT_P2);
        //s.push_str(introspect::PROP_STR);
        s.push_str(introspect::MANGAGER_STR);
        let children: Vec<String> = (0..self.heirarchy.len())
            .map(|u| format!("service{:04x}", u))
            .collect();
        introspect::child_nodes(&children, &mut s);
        s.push_str(introspect::INTROSPECT_FMT_P3);
        reply.body.push_param(s).unwrap();
        reply
    }
    async fn handle_obj_mgr(
        &mut self, 
        call: &MarshalledMessage,
    ) -> Result<MarshalledMessage, Error> {
        type IfAndProps = HashMap<&'static str, HashMap<&'static str, BluezOptions<'static, 'static>>>;
        type FutTuple = (ObjectPathBuf, IfAndProps);
        let mut receivers = Vec::new();
        for (serv, chrc_workers) in &mut self.heirarchy {
            for (chrc, desc_workers) in chrc_workers {
                for desc in desc_workers {
                    let (sender, recv) = one_time_channel::<FutTuple>();
                    desc.send(DescMsg::ObjMgr(sender)).await?;
                    receivers.push(recv);
                }
                let (sender, recv) = one_time_channel::<FutTuple>();
                chrc.send(ChrcMsg::ObjMgr(sender)).await?;
                receivers.push(recv);
            }
            let (sender, recv) = one_time_channel::<FutTuple>();
            serv.send(ServMsg::ObjMgr(sender)).await?;
            receivers.push(recv);
        }
        let map: HashMap<ObjectPathBuf, IfAndProps>= try_join_all(receivers.into_iter()
            .map(|r| r.recv())).await?.into_iter().collect();
        eprintln!("{:?}", map);
        let mut res = call.dynheader.make_response();
        res.body.push_param(map).unwrap();
        Ok(res)
    }
    async fn handle_prefix(
        &self,
        call: &MarshalledMessage,
        idx: usize,
        conn: &RpcConn,
    ) -> std::io::Result<()> {
        let mut pre_comps = self.base_path.iter();
        let mut reply = call.dynheader.make_response();
        let mut s = String::from(introspect::INTROSPECT_FMT_P1);
        pre_comps.nth(idx).unwrap(); // advance
                                     //let comp_str = unsafe { std::str::from_utf8_unchecked(comp.as_bytes()) };
                                     //s.push_str(comp_str);
                                     //s.push_str(introspect::INTROSPECT_FMT_P2);
        let child = std::str::from_utf8(pre_comps.next().unwrap().as_bytes()).unwrap();
        introspect::child_nodes(&[child], &mut s);
        s.push_str(introspect::INTROSPECT_FMT_P3);
        reply.body.push_param(s).unwrap();
        conn.send_message(&reply).await?.await?;
        Ok(())
    }
}
type WorkerHeirarchy = Vec<(ServWorker, Vec<(ChrcWorker, Vec<DescWorker>)>)>;
type ServWorkerSlice<'a> = &'a mut [(ServWorker, Vec<(ChrcWorker, Vec<DescWorker>)>)];
type ChrcWorkerSlice<'a> = &'a mut [(ChrcWorker, Vec<DescWorker>)];
impl Application {
    pub fn new(hci: &str) -> Self {
        let mut path = ObjectPathBuf::from_str("/org/bluez").unwrap();
        let hci = format!("/{}", hci);
        path.push(ObjectPath::new(&hci).unwrap());
        Self {
            hci: path,
            services: Vec::new(),
            base_path: ObjectPathBuf::from_str("/io/maves/rustable").unwrap(),
            dest: None,
            filter: true,
        }
    }
    pub fn set_dest(&mut self, dest: Option<String>) {
        self.dest = dest;
    }
    pub fn add_service(&mut self, mut service: Service) {
        match self.find_serv_unsorted(service.uuid()) {
            Some(old) => std::mem::swap(old, &mut service),
            None => self.services.push(service),
        }
    }
    pub fn remove_service(&mut self, uuid: UUID) -> Option<Service> {
        let idx = self.services.iter().position(|s| s.uuid() == uuid)?;
        Some(self.services.remove(idx))
    }
    pub fn set_filter(&mut self, filter: bool) {
        self.filter = filter;
    }
    pub fn zero_handles(&mut self) {
        unimplemented!()
    }
    fn sort_services(&mut self) {
        for service in self.services.iter_mut() {
            service.sort_chars();
        }
        self.services.sort_by_key(|serv| serv.uuid());
    }
    fn find_serv_unsorted(&mut self, uuid: UUID) -> Option<&mut Service> {
        self.services.iter_mut().find(|s| s.uuid() == uuid)
    }
    async fn begin_reg_call<'a>(
        &self,
        conn: &'a RpcConn,
    ) -> std::io::Result<impl Future<Output = std::io::Result<Option<MarshalledMessage>>> + 'a>
    {
        let mut call = MessageBuilder::new()
            .call(String::from("RegisterApplication"))
            .at(String::from("org.bluez"))
            .on(String::from(self.hci.clone()))
            .with_interface(String::from(BLUEZ_MGR_IF))
            .build();
        call.body.push_param(&*self.base_path).unwrap();
        let options: HashMap<&str, BluezOptions> = HashMap::new();
        call.body.push_param(&options).unwrap();
        Ok(conn.send_message(&call).await?)
    }
    pub async fn register(mut self) -> Result<AppWorker, Error> {
        let conn = RpcConn::system_conn(true).await?;
        let conn = Arc::new(conn);
        if let Some(s) = self.dest.as_ref() {
            let call = async_rustbus::rustbus_core::standard_messages::request_name(s.clone(), 0);
            let res = conn.send_message(&call).await?.await?.unwrap();
            is_msg_err_empty(&res)?;
        }
        let mut res_fut = self.begin_reg_call(&conn).await?.boxed();
        self.sort_services();
        let base_path = &mut self.base_path;
        let serv_workers: WorkerHeirarchy = self.services.drain(..).enumerate().map(|(i, mut serv)| {
            let serv_path = format!("{}/service{:04x}", base_path, i);
            let chrc_workers: Vec<_> = serv.drain_chrcs().enumerate().map(|(j, mut chrc)| {
                let chrc_path = format!("{}/char{:04x}", serv_path, j);
                let desc_workers: Vec<_> = chrc.drain_descs().enumerate().map(|(k, desc)| {
                    let desc_path = format!("{}/desc{:04x}", chrc_path, k);
                    DescWorker::new(desc,&conn,ObjectPathBuf::try_from(desc_path).unwrap())
                }).collect();
                let c_cnt = desc_workers.len();
                (ChrcWorker::new(chrc, &conn, ObjectPathBuf::try_from(chrc_path).unwrap(), c_cnt), desc_workers)
            }).collect();
            let c_cnt = chrc_workers.len();
            (ServWorker::new(serv, &conn, ObjectPathBuf::try_from(serv_path).unwrap(), c_cnt),chrc_workers)
        }).collect();
        let mut app_data = WorkerData {
            heirarchy: serv_workers,
            base_path: self.base_path
        };
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
                    eprintln!("call received: {:?}", call);
                    app_data.handle_call(&conn, call?).await?;
                    eprintln!("call handled\n");
                    res_fut = res_f;
                }
            }
        }
        let (sender, recv) = bounded(2);
        let worker = spawn(async move {
            let mut recv_fut = recv.recv().boxed();
            let mut call_fut = conn.get_call().boxed();
            loop {
                match select(recv_fut, call_fut).await {
                    Either::Left((msg, call_f)) => {
                        let msg = msg.unwrap();
                        match msg {
                            WorkerMsg::Unregister => break,
                            WorkerMsg::UpdateChar(serv, cha, val, notify) => {
                                let chrc = app_data.find_serv_chrc(serv, cha)
                                    .ok_or_else(|| Error::UnknownChar(serv, cha))?;
                                chrc.send(ChrcMsg::Update(val, notify)).await?;
                            }
                            WorkerMsg::UpdateDesc(serv, cha, desc, val) => {
                                let desc = app_data.find_serv_chrc_desc(serv, cha, desc)
                                    .ok_or_else(|| Error::UnknownDesc(serv, cha, desc))?;
                                desc.send(DescMsg::Update(val)).await?;
                            }
                            WorkerMsg::NotifyChar(serv, cha, att_val) => {
                                let chrc = app_data.find_serv_chrc(serv, cha)
                                    .ok_or_else(|| Error::UnknownChar(serv, cha))?;
                                chrc.send(ChrcMsg::Notify(att_val)).await?;
                            }
                            WorkerMsg::GetChar(serv, cha, sender) => {
                                let chrc = app_data.find_serv_chrc(serv, cha)
                                    .ok_or_else(|| Error::UnknownChar(serv, cha))?;
                                chrc.send(ChrcMsg::Get(sender)).await?;
                            }
                            WorkerMsg::GetDesc(serv, cha, desc, sender) => {
                                let desc = app_data.find_serv_chrc_desc(serv, cha, desc)
                                    .ok_or_else(|| Error::UnknownDesc(serv, cha, desc))?;
                                desc.send(DescMsg::Get(sender)).await?;
                            }
                            WorkerMsg::GetServHandle(serv, sender) => {
                                let serv = app_data.find_serv(serv)
                                    .ok_or_else(|| Error::UnknownServ(serv))?;
                                serv.send(ServMsg::GetHandle(sender)).await?;
                            }
                            WorkerMsg::GetCharHandle(serv, cha, sender) => {
                                let chrc = app_data.find_serv_chrc(serv, cha)
                                    .ok_or_else(|| Error::UnknownChar(serv, cha))?;
                                chrc.send(ChrcMsg::GetHandle(sender)).await?;
                            }
                            WorkerMsg::GetDescHandle(serv, cha, desc, sender) => {
                                let desc = app_data.find_serv_chrc_desc(serv, cha, desc)
                                    .ok_or_else(|| Error::UnknownDesc(serv, cha, desc))?;
                                desc.send(DescMsg::GetHandle(sender)).await?;
                            }
                        }

                        call_fut = call_f;
                        recv_fut = recv.recv().boxed();
                    }
                    Either::Right((call, recv_f)) => {
                        let call = call.unwrap();
                        app_data.handle_call(&conn, call).await?;
                        recv_fut = recv_f;
                        call_fut = conn.get_call().boxed();
                    }
                }
            }
            unimplemented!()
        });
        Ok(AppWorker { worker, sender })
    }
}
fn find_chrc<'a>(chrc_workers: ChrcWorkerSlice<'a>, uuid: UUID) 
    -> Option<&'a mut (ChrcWorker, Vec<DescWorker>)>
{
    let idx = chrc_workers.binary_search_by_key(&uuid, |(s,_)| s.uuid()).ok()?;
    Some(&mut chrc_workers[idx])
}
fn find_desc<'a>(desc_workers: &'a mut [DescWorker], uuid: UUID) -> Option<&'a mut DescWorker> {
    let idx = desc_workers.binary_search_by_key(&uuid, |s| s.uuid()).ok()?;
    Some(&mut desc_workers[idx])
}

enum AttrRef<'a> {
    Prefix(&'a mut WorkerData, usize),
    App(&'a mut WorkerData),
    Serv(&'a mut ServWorker),
    Chrc(&'a mut ChrcWorker),
    Desc(&'a mut DescWorker),
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
    pub async fn get_char(&self, serv: UUID, cha: UUID) -> Result<AttValue, Error> {
        let (sender, recv) = one_time_channel();
        self.sender
            .send(WorkerMsg::GetChar(serv, cha, sender))
            .await?;
        let res = recv.recv().await?;
        Ok(res)
    }
    pub async fn get_serv_handle(&self, serv: UUID) -> Result<NonZeroU16, Error> {
        let (sender, recv) = one_time_channel();
        self.sender
            .send(WorkerMsg::GetServHandle(serv, sender))
            .await?;
        let res = recv.recv().await?;
        Ok(res)
    }
    pub async fn get_char_handle(&self, serv: UUID, cha: UUID) -> Result<NonZeroU16, Error> {
        let (sender, recv) = one_time_channel();
        self.sender
            .send(WorkerMsg::GetCharHandle(serv, cha, sender))
            .await?;
        let res = recv.recv().await?;
        Ok(res)
    }
    pub async fn get_desc(&self, serv: UUID, cha: UUID, desc: UUID) -> Result<AttValue, Error> {
        let (sender, recv) = one_time_channel();
        self.sender
            .send(WorkerMsg::GetDesc(serv, cha, desc, sender))
            .await?;
        let res = recv.recv().await?;
        Ok(res)
    }
    pub async fn get_desc_handle(
        &self,
        serv: UUID,
        cha: UUID,
        desc: UUID,
    ) -> Result<NonZeroU16, Error> {
        let (sender, recv) = one_time_channel();
        self.sender
            .send(WorkerMsg::GetDescHandle(serv, cha, desc, sender))
            .await?;
        let res = recv.recv().await?;
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
