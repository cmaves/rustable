use async_std::channel::{bounded, Sender};
use async_std::task::{spawn, JoinHandle};
use futures::future::{select, try_join_all, Either};
use futures::prelude::*;
use std::collections::HashMap;
use std::num::NonZeroU16;

use super::*;
use crate::*;
use async_rustbus::rustbus_core::message_builder::MessageBuilder;
use async_rustbus::{CallAction, RpcConn};
use async_rustbus::rustbus_core::path::{ObjectPathBuf, ObjectPath};

mod one_time;
use one_time::{one_time_channel, OneSender};

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
    conn: Arc<RpcConn>,
    filter: bool,
}

struct WorkerData {
    heirarchy: WorkerHeirarchy,
    //base_path: ObjectPathBuf,
    conn: Arc<RpcConn>,
    filter: Option<String>,
}
impl WorkerData {
    fn find_serv(&mut self, uuid: UUID) -> Option<&mut ServWorker> {
        let idx = self
            .heirarchy
            .binary_search_by_key(&uuid, |(s, _)| s.uuid())
            .ok()?;
        Some(&mut self.heirarchy[idx].0)
    }
    fn find_serv_chrc(&mut self, serv: UUID, chrc: UUID) -> Option<&mut ChrcWorker> {
        let idx = self
            .heirarchy
            .binary_search_by_key(&serv, |(s, _)| s.uuid())
            .ok()?;
        Some(&mut find_chrc(&mut self.heirarchy[idx].1, chrc)?.0)
    }
    fn find_serv_chrc_desc(
        &mut self,
        serv: UUID,
        chrc: UUID,
        desc: UUID,
    ) -> Option<&mut DescWorker> {
        let idx = self
            .heirarchy
            .binary_search_by_key(&serv, |(s, _)| s.uuid())
            .ok()?;
        let desc_workers = &mut find_chrc(&mut self.heirarchy[idx].1, chrc)?.1;
        find_desc(desc_workers, desc)
    }
    async fn handle_app(&mut self, call: &MarshalledMessage) -> Result<(), Error> {
        let reply = if is_msg_bluez(call, &self.filter) {
            match call.dynheader.interface.as_ref().unwrap().as_str() {
                INTRO_IF => self.handle_app_intro(call),
                //PROPS_IF => self.handle_prop(call),
                OBJMGR_IF => self.handle_obj_mgr(call).await?,
                _ => unimplemented!(),
            }
        } else {
            call.dynheader.make_error_response("PermissionDenied", None)
        };
        self.conn.send_msg_no_reply(&reply).await?;
        Ok(())
    }
    fn handle_app_intro(&self, call: &MarshalledMessage) -> MarshalledMessage {
        let mut reply = call.dynheader.make_response();
        let mut s = String::from(introspect::INTROSPECT_FMT_P1);
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
        type IfAndProps =
            HashMap<&'static str, HashMap<&'static str, BluezOptions<'static, 'static>>>;
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
        let map: HashMap<ObjectPathBuf, IfAndProps> =
            try_join_all(receivers.into_iter().map(|r| r.recv()))
                .await?
                .into_iter()
                .collect();
        eprintln!("{:?}", map);
        let mut res = call.dynheader.make_response();
        res.body.push_param(map).unwrap();
        Ok(res)
    }
}
type WorkerHeirarchy = Vec<(ServWorker, Vec<(ChrcWorker, Vec<DescWorker>)>)>;
type ChrcWorkerSlice<'a> = &'a mut [(ChrcWorker, Vec<DescWorker>)];
impl Application {
    pub fn new_with_conn(hci: &Adapter, base_path: &str, conn: Arc<RpcConn>) -> Self {
        let hci = hci.path.clone();
        Self {
            services: Vec::new(),
            base_path: ObjectPathBuf::from_str(base_path).unwrap(),
            dest: None,
            filter: true,
            hci,
            conn,
        }
    }
    pub fn new(hci: &Adapter, base_path: &str) -> Result<Self, Error> {
        let conn = hci.conn.clone();
        Ok(Self::new_with_conn(hci, base_path, conn))
    }
    pub async fn set_dest(&mut self, dest: Option<String>) -> Result<(), Error> {
        if self.dest == dest {
            return Ok(());
        }
        if let Some(dest) = &self.dest {
            let mut call = MessageBuilder::new()
                .call("ReleaseName")
                .at("org.freedesktop.DBus")
                .on("/org/freedesktop.DBus")
                .with_interface("org.freedesktop.Dbus")
                .build();
            call.body.push_param(dest).unwrap();
            let res = self.conn.send_msg_with_reply(&call).await?.await?;
            is_msg_err_empty(&res)?;
            self.dest = None;
        }
        if let Some(dest) = dest {
            let call = rustbus_core::standard_messages::request_name(&dest, 4);
            let res = self.conn.send_msg_with_reply(&call).await?.await?;
            let flag: u32 = is_msg_err(&res).unwrap();
            if flag == 2 || flag == 3 {
                return Err(Error::Dbus("Name taken!".to_string()));
            }
            self.dest = Some(dest);
        }
        Ok(())
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
    pub fn get_conn(&self) -> &Arc<RpcConn> {
        &self.conn
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
    async fn begin_reg_call(
        &self,
    ) -> std::io::Result<impl Future<Output = std::io::Result<MarshalledMessage>> + '_> {
        let mut call = MessageBuilder::new()
            .call(String::from("RegisterApplication"))
            .at(String::from("org.bluez"))
            .on(String::from(self.hci.clone()))
            .with_interface(String::from(BLUEZ_MGR_IF))
            .build();
        call.body.push_param(&*self.base_path).unwrap();
        let options: HashMap<&str, BluezOptions> = HashMap::new();
        call.body.push_param(&options).unwrap();
        Ok(self.conn.send_msg_with_reply(&call).await?)
    }
    pub async fn register(mut self) -> Result<AppWorker, Error> {
        assert_ne!(self.services.len(), 0);
        let filter = if self.filter {
            let mut call = MessageBuilder::new()
                .call("GetNameOwner")
                .on("/org/freedesktop/DBus")
                .with_interface("org.freedesktop.DBus")
                .at("org.freedesktop.DBus")
                .build();
            call.body.push_param(BLUEZ_DEST).unwrap();
            let res = self.conn.send_msg_with_reply(&call).await?.await?;
            let name: String = is_msg_err(&res)?;
            if name == "" {
                unimplemented!()
            }
            Some(name)
        } else {
            None
        };
        if matches!(
            self.conn.get_call_path_action("/").await,
            Some(CallAction::Drop) | Some(CallAction::Nothing)
        ) {
            self.conn.insert_call_path("/", CallAction::Intro).await.unwrap();
        }
        self.conn
            .insert_call_path(&*self.base_path, CallAction::Exact)
            .await
            .unwrap();
        let call_recv = self.conn.get_call_recv(&*self.base_path).await.unwrap();
        self.sort_services();
        let mut serv_workers = Vec::with_capacity(self.services.len());
        for (i, mut serv) in self.services.drain(..).enumerate() {
            let serv_path = format!("{}/service{:04x}", self.base_path, i);
            self.conn
                .insert_call_path(&*serv_path, CallAction::Exact)
                .await.unwrap();
            let chrc_drain = serv.drain_chrcs();
            let c_cnt = chrc_drain.len();
            let mut chrc_workers = Vec::with_capacity(c_cnt);
            for (j, mut chrc) in chrc_drain.enumerate() {
                let chrc_path = format!("{}/char{:04x}", serv_path, j);
                self.conn
                    .insert_call_path(&*chrc_path, CallAction::Exact)
                    .await.unwrap();
                let desc_drain = chrc.drain_descs();
                let d_cnt = desc_drain.len();
                let mut desc_workers = Vec::with_capacity(d_cnt);
                for (k, desc) in desc_drain.enumerate() {
                    let desc_path = format!("{}/desc{:04x}", chrc_path, k);
                    let desc_path = ObjectPathBuf::try_from(desc_path).unwrap();
                    self.conn
                        .insert_call_path(&*desc_path, CallAction::Exact)
                        .await
                        .unwrap();
                    let desc = DescWorker::new(
                        desc,
                        &self.conn,
                        desc_path,
                        filter.clone(),
                    );
                    desc_workers.push(desc);
                }
                let chrc = ChrcWorker::new(
                    chrc,
                    &self.conn,
                    ObjectPathBuf::try_from(chrc_path).unwrap(),
                    d_cnt,
                    filter.clone(),
                );
                chrc_workers.push((chrc, desc_workers));
            }
            //let serv = ServWorker::new(serv, &self.conn,
            let serv = ServWorker::new(
                serv,
                &self.conn,
                ObjectPathBuf::try_from(serv_path).unwrap(),
                c_cnt,
                filter.clone(),
            );
            serv_workers.push((serv, chrc_workers));
        }
        let mut res_fut = self.begin_reg_call().await?;

        let mut app_data = WorkerData {
            heirarchy: serv_workers,
            conn: self.conn.clone(),
            filter, //base_path: self.base_path.clone()
        };
        loop {
            let call_fut = call_recv.recv();
            match select(res_fut, call_fut).await {
                Either::Left((res, _)) => {
                    let res = res?;
                    is_msg_err_empty(&res)?;
                    break;
                }
                Either::Right((call, res_f)) => {
                    eprintln!("call received: {:?}", call);
                    app_data.handle_app(&call?).await?;
                    eprintln!("call handled\n");
                    res_fut = res_f;
                }
            }
        }
        let (sender, recv) = bounded(2);
        let worker = spawn(async move {
            let mut recv_fut = recv.recv();
            let mut call_fut = self.conn.get_call(&*self.base_path).boxed();
            loop {
                match select(recv_fut, call_fut).await {
                    Either::Left((msg, call_f)) => {
                        let msg = msg.unwrap();
                        match msg {
                            WorkerMsg::Unregister => break,
                            WorkerMsg::UpdateChar(serv, cha, val, notify) => {
                                let chrc = app_data
                                    .find_serv_chrc(serv, cha)
                                    .ok_or_else(|| Error::UnknownChar(serv, cha))?;
                                chrc.send(ChrcMsg::Update(val, notify)).await?;
                            }
                            WorkerMsg::UpdateDesc(serv, cha, desc, val) => {
                                let desc = app_data
                                    .find_serv_chrc_desc(serv, cha, desc)
                                    .ok_or_else(|| Error::UnknownDesc(serv, cha, desc))?;
                                desc.send(DescMsg::Update(val)).await?;
                            }
                            WorkerMsg::NotifyChar(serv, cha, att_val) => {
                                let chrc = app_data
                                    .find_serv_chrc(serv, cha)
                                    .ok_or_else(|| Error::UnknownChar(serv, cha))?;
                                chrc.send(ChrcMsg::Notify(att_val)).await?;
                            }
                            WorkerMsg::GetChar(serv, cha, sender) => {
                                let chrc = app_data
                                    .find_serv_chrc(serv, cha)
                                    .ok_or_else(|| Error::UnknownChar(serv, cha))?;
                                chrc.send(ChrcMsg::Get(sender)).await?;
                            }
                            WorkerMsg::GetDesc(serv, cha, desc, sender) => {
                                let desc = app_data
                                    .find_serv_chrc_desc(serv, cha, desc)
                                    .ok_or_else(|| Error::UnknownDesc(serv, cha, desc))?;
                                desc.send(DescMsg::Get(sender)).await?;
                            }
                            WorkerMsg::GetServHandle(serv, sender) => {
                                let serv = app_data
                                    .find_serv(serv)
                                    .ok_or_else(|| Error::UnknownServ(serv))?;
                                serv.send(ServMsg::GetHandle(sender)).await?;
                            }
                            WorkerMsg::GetCharHandle(serv, cha, sender) => {
                                let chrc = app_data
                                    .find_serv_chrc(serv, cha)
                                    .ok_or_else(|| Error::UnknownChar(serv, cha))?;
                                chrc.send(ChrcMsg::GetHandle(sender)).await?;
                            }
                            WorkerMsg::GetDescHandle(serv, cha, desc, sender) => {
                                let desc = app_data
                                    .find_serv_chrc_desc(serv, cha, desc)
                                    .ok_or_else(|| Error::UnknownDesc(serv, cha, desc))?;
                                desc.send(DescMsg::GetHandle(sender)).await?;
                            }
                        }

                        call_fut = call_f;
                        recv_fut = recv.recv();
                    }
                    Either::Right((call, recv_f)) => {
                        app_data.handle_app(&call?).await?;
                        recv_fut = recv_f;
                        call_fut = self.conn.get_call(&*self.base_path).boxed();
                    }
                }
            }
            for (i, (serv, chrc_workers)) in app_data.heirarchy.into_iter().enumerate() {
                let serv_path = format!("{}/service{:04x}", self.base_path, i);
                serv.send(ServMsg::Shutdown).await?;
                let mut serv = serv.shutdown().await?;
                for (j, (chrc, desc_workers)) in chrc_workers.into_iter().enumerate() {
                    let chrc_path = format!("{}/char{:04x}", serv_path, j);
                    let shut_fut = desc_workers.into_iter().map(|d| d.shutdown());
                    let descs = try_join_all(shut_fut).await?;
                    let mut chrc = chrc.shutdown().await?;
                    for (k, desc) in descs.into_iter().enumerate() {
                        let desc_path = format!("{}/desc{:04x}", chrc_path, k);
                        self.conn
                            .insert_call_path(&*desc_path, CallAction::Nothing)
                            .await
                            .unwrap();
                        chrc.add_desc(desc);
                    }
                    serv.add_char(chrc);
                }
                self.services.push(serv);
            }
            Ok(self)
        });
        Ok(AppWorker { worker, sender })
    }
}
fn is_msg_bluez(call: &MarshalledMessage, filter: &Option<String>) -> bool {
    let self_dest = match filter {
        Some(d) => d,
        None => return true,
    };
    //let dest = call.dynheader.sender.as_ref().map(|s| s.as_str());
    match &call.dynheader.sender {
        Some(d) => d == BLUEZ_DEST || d == self_dest,
        None => false,
    }
}

fn find_chrc<'a>(
    chrc_workers: ChrcWorkerSlice<'a>,
    uuid: UUID,
) -> Option<&'a mut (ChrcWorker, Vec<DescWorker>)> {
    let idx = chrc_workers
        .binary_search_by_key(&uuid, |(s, _)| s.uuid())
        .ok()?;
    Some(&mut chrc_workers[idx])
}
fn find_desc<'a>(desc_workers: &'a mut [DescWorker], uuid: UUID) -> Option<&'a mut DescWorker> {
    let idx = desc_workers
        .binary_search_by_key(&uuid, |s| s.uuid())
        .ok()?;
    Some(&mut desc_workers[idx])
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
    pub fn notify_char(
        &self,
        service: UUID,
        character: UUID,
        val: Option<AttValue>,
    ) -> impl Future<Output = Result<(), Error>> + Unpin + '_ {
        self.sender
            .send(WorkerMsg::NotifyChar(service, character, val))
            .err_into()
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
