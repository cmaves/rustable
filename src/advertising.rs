use crate::properties::{PropError, Properties};
use crate::*;
use std::collections::HashMap;

use async_rustbus::CallAction;
use async_std::channel::{bounded, Sender};
use async_std::task::JoinHandle;
use futures::future::{select, Either};
use futures::pin_mut;

#[derive(Clone, Copy)]
pub enum AdType {
    Peripheral,
    Broadcast,
}

impl AdType {
    pub fn to_str(&self) -> &'static str {
        match self {
            AdType::Peripheral => "peripheral",
            AdType::Broadcast => "broadcast",
        }
    }
}
#[derive(Clone)]
pub struct Advertisement {
    hci: ObjectPathBuf,
    // base_path: ObjectPathBuf,
    pub services: Vec<UUID>,
    pub solicit: Vec<UUID>,
    conn: Arc<RpcConn>,
    dest: Option<String>,
    pub name: Option<String>,
    pub typ: AdType,
    pub duration: u16,
    pub timeout: u16,
    manu_data: HashMap<String, Vec<u8>>,
    serv_data: HashMap<UUID, Vec<u8>>,
}

impl Advertisement {
    pub fn new_with_conn(hci: &Adapter, conn: Arc<RpcConn>) -> Self {
        let path = hci.path().to_owned();
        Self {
            services: Vec::new(),
            solicit: Vec::new(),
            //base_path: ObjectPathBuf::from_str(base_path).unwrap(),
            typ: AdType::Peripheral,
            manu_data: HashMap::new(),
            serv_data: HashMap::new(),
            duration: 0,
            timeout: 0,
            dest: None,
            name: None,
            hci: path,
            conn,
        }
    }
    pub async fn new(hci: &Adapter) -> Result<Self, Error> {
        let conn = RpcConn::system_conn(true).await?;
        let conn = Arc::new(conn);
        Ok(Self::new_with_conn(hci, conn))
    }
    pub async fn set_dest(&mut self, dest: Option<String>) -> Result<(), Error> {
        if self.dest == dest {
            return Ok(());
        }
        if let Some(dest) = &self.dest {
            let mut call = MessageBuilder::new()
                .call("ReleaseName".to_string())
                .at("org.freedesktop.DBus".to_string())
                .on("/org/freedesktop.DBus".to_string())
                .with_interface("org.freedesktop.Dbus".to_string())
                .build();
            call.body.push_param(dest).unwrap();
            let res = self.conn.send_msg_with_reply(&call).await?.await?;
            is_msg_err_empty(&res)?;
            self.dest = None;
        }
        if let Some(dest) = dest {
            let call = rustbus_core::standard_messages::request_name(dest.clone(), 4);
            let res = self.conn.send_msg_with_reply(&call).await?.await?;
            let flag: u32 = is_msg_err(&res).unwrap();
            if flag == 2 || flag == 3 {
                return Err(Error::Dbus("Name taken!".to_string()));
            }
            self.dest = Some(dest);
        }
        Ok(())
    }
    pub async fn register(mut self, base_path: &str) -> Result<AdWorker, Error> {
        let base_path = ObjectPathBuf::from_str(base_path).unwrap();
        let (sender, recv) = bounded(1);
        let mut call = MessageBuilder::new()
            .call("RegisterAdvertisement".to_string())
            .on(self.hci.to_string())
            .at("org.bluez".to_string())
            .with_interface("org.bluez.LEAdvertisingManager1".to_string())
            .build();
        call.body.push_param(&base_path).unwrap();
        let options: HashMap<String, BluezOptions> = HashMap::new();
        call.body.push_param(options).unwrap();
        eprintln!("Registering ad at: {}", base_path);
        let conn = self.conn.clone();
        conn.insert_call_path(&base_path, CallAction::Exact).await;
        {
            // new block limits lifetime of res_fut
            let res_fut = conn.send_msg_with_reply(&call).await?;
            pin_mut!(res_fut);
            loop {
                let call_fut = conn.get_call(&base_path);
                pin_mut!(call_fut);
                match select(call_fut, res_fut).await {
                    Either::Left((call, res_f)) => {
                        let res = self.handle_call(&call?)?;
                        match res {
                            Some(res) => self.conn.send_msg_no_reply(&res).await?,
                            None => {
                                return Err(Error::Bluez(
                                    "Released before ad could start.".to_string(),
                                ))
                            }
                        }
                        res_fut = res_f;
                    }
                    Either::Right((res, _)) => {
                        is_msg_err_empty(&res?)?;
                        break;
                    }
                }
            }
        }
        eprintln!("Ad Registered");
        let worker = async_std::task::spawn(async move {
            let mut call_fut = conn.get_call(&base_path).boxed();
            let mut msg_fut = recv.recv();
            loop {
                match select(call_fut, msg_fut).await {
                    Either::Left((call, msg_f)) => {
                        let res = self.handle_call(&call?)?;
                        match res {
                            Some(res) => self.conn.send_msg_no_reply(&res).await?,
                            None => break,
                        }
                        msg_fut = msg_f;
                        call_fut = conn.get_call(&base_path).boxed();
                    }
                    Either::Right((msg, call_f)) => {
                        match msg? {
                            WorkerMsg::Release => {
                                call.dynheader.member = Some("UnregisterAdvertisement".to_string());
                                call.body.reset();
                                call.body.push_param(&base_path).unwrap();
                                let res = self.conn.send_msg_with_reply(&call).await?.await?;
                                is_msg_err_empty(&res)?;
                                break;
                            }
                            WorkerMsg::Alive => {}
                        }
                        call_fut = call_f;
                        msg_fut = recv.recv();
                    }
                }
            }
            eprintln!("Ad Released");
            conn.insert_call_path(base_path, CallAction::Nothing).await;
            Ok(self)
        });
        Ok(AdWorker { sender, worker })
    }
    fn handle_call(
        &mut self,
        call: &MarshalledMessage,
    ) -> Result<Option<MarshalledMessage>, Error> {
        let interface = call.dynheader.interface.as_ref().unwrap();
        match &interface[..] {
            PROPS_IF => Ok(Some(self.properties_call(&call))),
            INTRO_IF => Ok(Some(self.handle_intro(&call))),
            BLUEZ_ADV_IF => match call.dynheader.member.as_ref().unwrap().as_str() {
                "Release" => Ok(None),
                _ => Ok(Some(
                    call.dynheader
                        .make_error_response("UnknownMethod".to_string(), None),
                )),
            },
            _ => Ok(Some(
                call.dynheader
                    .make_error_response("UnknownInterface".to_string(), None),
            )),
        }
    }
    fn handle_intro(&self, call: &MarshalledMessage) -> MarshalledMessage {
        let mut s = String::from(introspect::INTROSPECT_FMT_P1);
        s.push_str(introspect::PROP_STR);
        s.push_str(introspect::ADV_STR);
        s.push_str(introspect::INTROSPECT_FMT_P3);
        let mut reply = call.dynheader.make_response();
        reply.body.push_param(s).unwrap();
        reply
    }
}

pub struct AdWorker {
    worker: JoinHandle<Result<Advertisement, Error>>,
    sender: Sender<WorkerMsg>,
}

impl AdWorker {
    pub async fn release(self) -> Result<Advertisement, Error> {
        self.sender.send(WorkerMsg::Release).await.ok();
        self.worker.await
    }
    pub async fn is_alive(&self) -> bool {
        matches!(self.sender.send(WorkerMsg::Alive).await, Ok(_))
    }
}

impl Properties for Advertisement {
    const INTERFACES: &'static [(&'static str, &'static [&'static str])] = &[(
        BLUEZ_ADV_IF,
        &[
            "ServiceUUIDs",
            "Type",
            "ManufacturerData",
            "SolicitUUIDs",
            "ServiceData",
        ],
    )];
    fn get_inner(
        &mut self,
        _: &ObjectPath,
        interface: &str,
        prop: &str,
    ) -> Result<BluezOptions<'static, 'static>, PropError> {
        match interface {
            BLUEZ_ADV_IF => match prop {
                "Type" => Ok(BluezOptions::Str(self.typ.to_str())),
                "ServiceUUIDs" => Ok(BluezOptions::UUIDs(self.services.clone())),
                "ManufacturerData" => Ok(BluezOptions::DataMap(self.manu_data.clone())),
                "SolicitUUIDs" => Ok(BluezOptions::UUIDs(self.solicit.clone())),
                "ServiceData" => Ok(BluezOptions::UUIDMap(self.serv_data.clone())),
                _ => Err(PropError::PropertyNotFound),
            },
            _ => Err(PropError::InterfaceNotFound),
        }
    }
    fn set_inner(
        &mut self,
        _: &ObjectPath,
        interface: &str,
        prop: &str,
        _: BluezOptions,
    ) -> Result<(), PropError> {
        Err(match interface {
            BLUEZ_ADV_IF => {
                if Self::INTERFACES[0].1.contains(&prop) {
                    PropError::PermissionDenied
                } else {
                    PropError::PropertyNotFound
                }
            }
            _ => PropError::InterfaceNotFound,
        })
    }
}

enum WorkerMsg {
    Release,
    Alive,
}
