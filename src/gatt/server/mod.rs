use futures::future::{select, try_join_all, Either};
use futures::prelude::*;
use std::collections::HashMap;
use std::num::NonZeroU16;
use tokio::sync::mpsc::{channel as bounded, Sender};
use tokio::task::{spawn, JoinHandle};

use super::*;
use crate::*;
use async_rustbus::rustbus_core::message_builder::MessageBuilder;
use async_rustbus::rustbus_core::path::{ObjectPath, ObjectPathBuf};
use async_rustbus::{CallAction, RpcConn};

use tokio::sync::oneshot::{channel as one_time_channel, Sender as OneSender};

mod chrc;
pub use chrc::{Characteristic, ShouldNotify};

mod service;
pub use service::Service;

mod descriptor;
pub use descriptor::Descriptor;

/// Use to build an application containing local GATT services that can be used by remote devices.
pub struct Application {
    services: Vec<Service>,
    dest: Option<String>,
    hci: ObjectPathBuf,
    base_path: ObjectPathBuf,
    conn: Arc<RpcConn>,
    filter: bool,
}

struct WorkerData {
    senders: Vec<Sender<WorkerMsg>>,
    serv_cnt: usize,
    //base_path: ObjectPathBuf,
    conn: Arc<RpcConn>,
    filter: Option<Arc<str>>,
}
enum WorkerJoin {
    App(Application),
    Serv(Service),
    Chrc(Characteristic),
    Desc(Descriptor),
}
struct Worker {
    sender: Sender<WorkerMsg>,
    handle: JoinHandle<Result<WorkerJoin, Error>>,
}
impl WorkerData {
    async fn handle_app(&mut self, call: &MarshalledMessage) -> Result<(), Error> {
        let reply = if is_msg_bluez(call, self.filter.as_deref()) {
            match call.dynheader.interface.as_ref().unwrap().as_str() {
                INTRO_IF => self.handle_app_intro(call),
                //PROPS_IF => self.handle_prop(call),
                OBJMGR_IF => self.handle_obj_mgr(call).await?,
                _ => unimplemented!(),
            }
        } else {
            call.dynheader.make_error_response("PermissionDenied", None)
        };
        self.conn.send_msg_wo_rsp(&reply).await?;
        Ok(())
    }
    fn handle_app_intro(&self, call: &MarshalledMessage) -> MarshalledMessage {
        let mut reply = call.dynheader.make_response();
        let mut s = String::from(introspect::INTROSPECT_FMT_P1);
        s.push_str(introspect::MANGAGER_STR);
        let children: Vec<String> = (0..self.serv_cnt)
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
        let obj_iter = self.senders.iter().map(|sender| async move {
            let (send, recv) = one_time_channel::<FutTuple>();
            sender.send(WorkerMsg::ObjMgr(send)).await?;
            let ret = recv.await?;
            Result::<_, Error>::Ok(ret)
        });
        let map: HashMap<ObjectPathBuf, IfAndProps> =
            try_join_all(obj_iter).await?.into_iter().collect();
        eprintln!("{:?}", map);
        let mut res = call.dynheader.make_response();
        res.body.push_param(map).unwrap();
        Ok(res)
    }
}
impl Application {
    /// Create new `Application` that is assocated with the given adapter.
    ///
    /// This app will use the given `conn` to interact with Bluez to provide the services.
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
    /// Create new `Application` that is assocated with the given adapter.
    ///
    /// The connection used to interact with Bluez daemon is the same as the one used in `hci`.
    pub fn new(hci: &Adapter, base_path: &str) -> Self {
        let conn = hci.conn.clone();
        Self::new_with_conn(hci, base_path, conn)
    }
    /// Requests a DBus name for the `Application` using its DBus connection.
    ///
    /// If the destination is in use this will fail and not place the connection in the name queue.
    /// # Notes
    /// * When the application is dropped, the underlying connection will not drop the name.
    pub async fn set_dbus_name(&mut self, dest: Option<String>) -> Result<(), Error> {
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
            let res = self.conn.send_msg_w_rsp(&call).await?.await?;
            is_msg_err_empty(&res)?;
            self.dest = None;
        }
        if let Some(dest) = dest {
            let call = rustbus_core::standard_messages::request_name(&dest, 4);
            let res = self.conn.send_msg_w_rsp(&call).await?.await?;
            let flag: u32 = is_msg_err(&res).unwrap();
            if flag == 2 || flag == 3 {
                return Err(Error::Dbus("Name taken!".to_string()));
            }
            self.dest = Some(dest);
        }
        Ok(())
    }
    /// Get the DBus name currently in use for the `Application`.
    #[inline]
    pub fn get_dbus_dest(&self) -> Option<&str> {
        self.dest.as_deref()
    }
    /// Add a GATT service to the `Application`.
    pub fn add_service(&mut self, mut service: Service) {
        assert!(
            !service.characteristics().is_empty(),
            "Bluez doesn't handle services without chrcs correctly."
        );
        match self.find_serv_unsorted(service.uuid()) {
            Some(old) => std::mem::swap(old, &mut service),
            None => self.services.push(service),
        }
    }
    /// Remove a GATT service from the `Application`.
    pub fn remove_service(&mut self, uuid: UUID) -> Option<Service> {
        let idx = self.services.iter().position(|s| s.uuid() == uuid)?;
        Some(self.services.remove(idx))
    }
    /// Set whether the `Application` should filter out DBus messages coming from sources
    /// other than the Bluez daemon.
    ///
    /// `true` (the default) will filter out messages while `false` will allow all messages.
    /// This can be useful debugging,
    /// but users should be cautious
    /// as this will give all users on the local device access to the application.
    #[inline]
    pub fn set_filter(&mut self, filter: bool) {
        self.filter = filter;
    }
    /// Get whether the `Application` is filtering out incoming DBus messages from sources
    /// other than the Bluez daemon.
    #[inline]
    pub fn get_filter(&self) -> bool {
        self.filter
    }
    /// Get a reference to `Arc<async_rustbus::RpcConn>` used to communicate with the Bluez daemon.
    pub fn conn(&self) -> &Arc<RpcConn> {
        &self.conn
    }
    #[doc(hidden)]
    pub fn zero_handles(&mut self) {
        unimplemented!()
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
        Ok(self.conn.send_msg_w_rsp(&call).await?)
    }
    /// Register the application with Bluez daemon, and begin a worker thread to run the service.
    ///
    /// # Notes
    /// * This makes the org.bluez.GattManager1.RegisterApplication DBus call, starting the application.
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
            let res = self.conn.send_msg_w_rsp(&call).await?.await?;
            let name: String = is_msg_err(&res)?;
            if name == "" {
                unimplemented!()
            }
            Some(name.into())
        } else {
            None
        };
        if matches!(
            self.conn.get_call_path_action("/").await,
            Some(CallAction::Drop) | Some(CallAction::Nothing)
        ) {
            self.conn
                .insert_call_path("/", CallAction::Intro)
                .await
                .unwrap();
        }
        self.conn
            .insert_call_path(&*self.base_path, CallAction::Exact)
            .await
            .unwrap();
        let call_recv = self.conn.get_call_recv(&*self.base_path).await.unwrap();
        let mut workers = HashMap::new();
        let serv_cnt = self.services.len();
        let mut includes = Vec::new(); // used to setup  service Includes property
        for (i, mut serv) in self.services.drain(..).enumerate() {
            let serv_path = format!("{}/service{:04x}", self.base_path, i);
            let serv_path = ObjectPathBuf::try_from(serv_path).unwrap();
            let serv_uuid = serv.uuid();
            if !serv.includes().is_empty() {
                includes.push((serv_uuid, Vec::<UUID>::from(serv.includes())));
            }
            self.conn
                .insert_call_path(&*serv_path, CallAction::Exact)
                .await
                .unwrap();
            let chrc_drain = serv.drain_chrcs();
            let c_cnt = chrc_drain.len();
            for (j, mut chrc) in chrc_drain.enumerate() {
                let chrc_path = format!("{}/char{:04x}", serv_path, j);
                let chrc_path = ObjectPathBuf::try_from(chrc_path).unwrap();
                let chrc_uuid = chrc.uuid();
                self.conn
                    .insert_call_path(&*chrc_path, CallAction::Exact)
                    .await
                    .unwrap();
                let desc_drain = chrc.drain_descs();
                let d_cnt = desc_drain.len();
                for (k, desc) in desc_drain.enumerate() {
                    let desc_path = format!("{}/desc{:04x}", chrc_path, k);
                    let desc_path = ObjectPathBuf::try_from(desc_path).unwrap();
                    let desc_uuid = desc.uuid();
                    self.conn
                        .insert_call_path(&*desc_path, CallAction::Exact)
                        .await
                        .unwrap();
                    let desc_worker = desc.start_worker(&self.conn, &desc_path, filter.clone());
                    workers.insert((serv_uuid, chrc_uuid, desc_uuid), (desc_worker, desc_path));
                }
                let chrc_worker = chrc.start_worker(&self.conn, &chrc_path, d_cnt, filter.clone());
                workers.insert((serv_uuid, chrc_uuid, UUID(0)), (chrc_worker, chrc_path));
            }
            let serv_worker = serv.start_worker(&self.conn, &serv_path, c_cnt, filter.clone());
            workers.insert((serv_uuid, UUID(0), UUID(0)), (serv_worker, serv_path));
        }
        for (serv_uuid, included) in includes {
            // serv_worker
            let paths = included
                .into_iter()
                .map(|uuid| workers[&(uuid, UUID(0), UUID(0))].1.to_owned())
                .collect();
            let msg = WorkerMsg::IncludedPaths(paths);
            workers[&(serv_uuid, UUID(0), UUID(0))]
                .0
                .sender
                .send(msg)
                .await
                .unwrap();
        }
        let senders = workers
            .values()
            .map(|(worker, _)| worker.sender.clone())
            .collect();
        let mut res_fut = self.begin_reg_call().await?;

        let mut app_data = WorkerData {
            serv_cnt,
            senders,
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
        let (sender, mut recv) = bounded(2);
        let handle = spawn(async move {
            loop {
                tokio::select! {
                    biased;
                    opt = recv.recv() => {
                        let msg = opt.unwrap();
                        match msg {
                            WorkerMsg::Unregister => break,
                            _ => unreachable!(),
                        }

                    }
                    call = self.conn.get_call(&*self.base_path) => {
                        app_data.handle_app(&call?).await?;
                    }
                }
            }
            Ok(WorkerJoin::App(self))
        });
        let app_worker = Worker { handle, sender };
        let mut workers: HashMap<_, _> = workers.into_iter().map(|(k, (v, _))| (k, v)).collect();
        workers.insert((UUID(0), UUID(0), UUID(0)), app_worker);
        Ok(AppWorker { workers })
    }
}
/// Given an optional filter, check if this message should be allowed,
/// or rejected because it is not from the Bluez daemon.
fn is_msg_bluez(call: &MarshalledMessage, filter: Option<&str>) -> bool {
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

/// An active GATT application that is registered with the Bluez daemon.
///
/// This worker will continue to work even if this handle to it is dropped.
pub struct AppWorker {
    workers: HashMap<(UUID, UUID, UUID), Worker>,
}
impl AppWorker {
    /// Deregister the application with Bluez, shutdown the workers, and return the original `Application`.
    pub async fn unregister(self) -> Result<Application, Error> {
        struct SortableWorkers((UUID, UUID, UUID), Worker);
        impl PartialEq<SortableWorkers> for SortableWorkers {
            fn eq(&self, other: &SortableWorkers) -> bool {
                self.0.eq(&other.0)
            }
        }
        impl Eq for SortableWorkers {}
        impl PartialOrd<SortableWorkers> for SortableWorkers {
            fn partial_cmp(&self, other: &SortableWorkers) -> Option<std::cmp::Ordering> {
                self.0.partial_cmp(&other.0).map(|o| o.reverse())
            }
        }
        impl Ord for SortableWorkers {
            fn cmp(&self, other: &Self) -> std::cmp::Ordering {
                self.0.cmp(&other.0).reverse()
            }
        }
        let heap: std::collections::BinaryHeap<_> = self
            .workers
            .into_iter()
            .map(|(k, v)| SortableWorkers(k, v))
            .collect();
        let mut finished = try_join_all(heap.into_iter().map(|w| async {
            w.1.sender.send(WorkerMsg::Unregister).await?;
            let ret = w.1.handle.await.map_err(|_| Error::ThreadPanicked)??;
            Result::<_, Error>::Ok(ret)
        }))
        .await?
        .into_iter();
        let mut app = match finished.next() {
            Some(WorkerJoin::App(a)) => a,
            _ => unreachable!(),
        };
        let mut cur_serv = None;
        let mut cur_chrc = None;
        let mut cur_desc = None;
        for attr in finished {
            match attr {
                WorkerJoin::Serv(serv) => {
                    if let Some(serv) = cur_serv.replace(serv) {
                        app.add_service(serv);
                    }
                }
                WorkerJoin::Chrc(chrc) => {
                    if let Some(chrc) = cur_chrc.replace(chrc) {
                        cur_serv.as_mut().unwrap().add_char(chrc);
                    }
                }
                WorkerJoin::Desc(desc) => {
                    if let Some(desc) = cur_desc.replace(desc) {
                        cur_chrc.as_mut().unwrap().add_desc(desc);
                    }
                }
                WorkerJoin::App(_) => unreachable!(),
            }
        }
        Ok(app)
    }
    /// Update the value of the given characteristic with `val`.
    pub async fn update_characteristic(
        &self,
        service: UUID,
        character: UUID,
        val: ValOrFn,
        notify: bool,
    ) -> Result<(), Error> {
        let worker = self
            .workers
            .get(&(service, character, UUID(0)))
            .ok_or(Error::UnknownChrc(service, character))?;
        worker.sender.send(WorkerMsg::Update(val, notify)).await?;
        Ok(())
    }
    /// Update the value of the given descriptor with `val`.
    pub async fn update_descriptor(
        &self,
        service: UUID,
        character: UUID,
        descriptor: UUID,
        val: ValOrFn,
    ) -> Result<(), Error> {
        let worker = self
            .workers
            .get(&(service, character, descriptor))
            .ok_or(Error::UnknownDesc(service, character, descriptor))?;

        worker.sender.send(WorkerMsg::Update(val, false)).await?;
        Ok(())
    }
    /// Trigger a notification for the Bluetooth service.
    pub async fn notify_char(
        &self,
        service: UUID,
        character: UUID,
        val: Option<AttValue>,
    ) -> Result<(), Error> {
        let worker = self
            .workers
            .get(&(service, character, UUID(0)))
            .ok_or(Error::UnknownChrc(service, character))?;
        worker.sender.send(WorkerMsg::Notify(val)).await?;
        Ok(())
    }
    /// Get the current value of the characteristic.
    ///
    /// If the value of the characteristic is a `ValOrFn::Function` the callback will be called.
    /// If this is the case, note that this could affect what remote devices see
    /// if the callback changes based on number/timing of reads.
    pub async fn get_char(&self, serv: UUID, cha: UUID) -> Result<AttValue, Error> {
        let worker = self
            .workers
            .get(&(serv, cha, UUID(0)))
            .ok_or(Error::UnknownChrc(serv, cha))?;
        let (sender, recv) = one_time_channel();
        worker.sender.send(WorkerMsg::Get(sender)).await?;
        let res = recv.await?;
        Ok(res)
    }
    /// Get the ATT handle for the given service.
    pub async fn get_serv_handle(&self, serv: UUID) -> Result<NonZeroU16, Error> {
        let worker = self
            .workers
            .get(&(serv, UUID(0), UUID(0)))
            .ok_or(Error::UnknownServ(serv))?;
        let (sender, recv) = one_time_channel();
        worker.sender.send(WorkerMsg::GetHandle(sender)).await?;
        let res = recv.await?;
        Ok(res)
    }
    /// Get the ATT handle for the given characteristic.
    pub async fn get_char_handle(&self, serv: UUID, cha: UUID) -> Result<NonZeroU16, Error> {
        let worker = self
            .workers
            .get(&(serv, cha, UUID(0)))
            .ok_or(Error::UnknownChrc(serv, cha))?;
        let (sender, recv) = one_time_channel();
        worker.sender.send(WorkerMsg::GetHandle(sender)).await?;
        let res = recv.await?;
        Ok(res)
    }
    /// Check if the characteristic is notifying.
    ///
    /// This just indicates that Bluez has called `org.bluez.GattCharacteristic1.AcquireNotify`
    /// and has not hungup the socket, or `org.bluez.GattCharacteristic1.StartNotify` was called
    /// and has not been stopped.
    /// This does not guarantee that the device(s) that requested notifications are still connected
    /// or listening.
    pub async fn char_notifying(&self, serv: UUID, cha: UUID) -> Result<bool, Error> {
        let worker = self
            .workers
            .get(&(serv, cha, UUID(0)))
            .ok_or(Error::UnknownChrc(serv, cha))?;
        let (sender, recv) = one_time_channel();
        worker.sender.send(WorkerMsg::Notifying(sender)).await?;
        let res = recv.await?;
        Ok(res)
    }
    /// Check if the characteristic is notifying through a file descriptor.
    ///
    /// This indicates that Bluez has acquired a file descriptor
    /// with `org.bluez.GattCharacteristic1.AcquireNotify` and it hasn't been closed yet.
    /// This does not guarantee that the device(s) that requested notifications are still connected
    /// or listening.
    ///
    /// # Notes
    /// * When remote devices acquire notifications, Bluez will almost always call
    /// `org.bluez.GattCharacteristic1.AcquireNotify` to services them.
    pub async fn char_notify_acquired(&self, serv: UUID, cha: UUID) -> Result<bool, Error> {
        let worker = self
            .workers
            .get(&(serv, cha, UUID(0)))
            .ok_or(Error::UnknownChrc(serv, cha))?;
        let (sender, recv) = one_time_channel();
        worker
            .sender
            .send(WorkerMsg::NotifyAcquired(sender))
            .await?;
        let res = recv.await?;
        Ok(res)
    }
    /// Check if the characteristic is notifying via DBus signals.
    ///
    /// This indicates that Bluez has started a notification session by with
    /// `org.bluez.GattCharacteristic1.StartNotify`.
    /// # Notes
    /// * Bluez will almost never uses `org.bluez.GattCharacteristic1.StartNotify`,
    /// but it can be useful for debugging.
    pub async fn char_notify_signaling(&self, serv: UUID, cha: UUID) -> Result<bool, Error> {
        let worker = self
            .workers
            .get(&(serv, cha, UUID(0)))
            .ok_or(Error::UnknownChrc(serv, cha))?;
        let (sender, recv) = one_time_channel();
        worker
            .sender
            .send(WorkerMsg::NotifyingSignal(sender))
            .await?;
        let res = recv.await?;
        Ok(res)
    }
    /// Get the current value of the given descriptor.
    ///
    /// If the value of the descriptor is a `ValOrFn::Function` the callback will be called.
    /// If this is the case, note that this could affect what remote devices see
    /// if the callback changes based on number/timing of reads.
    pub async fn get_desc(&self, serv: UUID, cha: UUID, desc: UUID) -> Result<AttValue, Error> {
        let worker = self
            .workers
            .get(&(serv, cha, desc))
            .ok_or(Error::UnknownDesc(serv, cha, desc))?;
        let (sender, recv) = one_time_channel();
        worker.sender.send(WorkerMsg::Get(sender)).await?;
        let res = recv.await?;
        Ok(res)
    }
    /// Get the ATT handle for the given descriptor.
    pub async fn get_desc_handle(
        &self,
        serv: UUID,
        cha: UUID,
        desc: UUID,
    ) -> Result<NonZeroU16, Error> {
        let worker = self
            .workers
            .get(&(serv, cha, desc))
            .ok_or(Error::UnknownDesc(serv, cha, desc))?;
        let (sender, recv) = one_time_channel();
        worker.sender.send(WorkerMsg::GetHandle(sender)).await?;
        let res = recv.await?;
        Ok(res)
    }
}

#[derive(Debug)]
enum WorkerMsg {
    Unregister,
    Update(ValOrFn, bool),
    Get(OneSender<AttValue>),
    GetHandle(OneSender<NonZeroU16>),
    Notify(Option<AttValue>),
    Notifying(OneSender<bool>),
    NotifyAcquired(OneSender<bool>),
    NotifyingSignal(OneSender<bool>),
    IncludedPaths(Vec<ObjectPathBuf>),
    ObjMgr(
        OneSender<(
            ObjectPathBuf,
            HashMap<&'static str, HashMap<&'static str, BluezOptions<'static, 'static>>>,
        )>,
    ),
}
