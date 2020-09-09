//! # rustable
//! rustable is yet another library for interfacing bluez over DBus.
//!
//! ## Supported Features
//! ### GATT Server
//! - Creating local services
//! - Reading/Writing local characteristics
//! - Notifying/Indicating local characteristics with sockets.
//! - Notifying/Indicating local characteristics with DBus signals.
//!
//!  **To Do:**
//!
//! - Descriptors as a server.
//! ### GATT Client
//! - Receiving remote notification/indications with sockets.
//!
//!  **To Do:**
//!
//! - Descriptors as a client.
//! ## Development status
//! This library is unstable in *alpha*. There are planned functions
//! in the API that have yet to be implemented. Unimplemented function are noted.
//! The API is also subject to breaking changes.
//!
use gatt::*;
use nix::unistd::close;
use rustbus::client_conn;
use rustbus::client_conn::{Conn, RpcConn, Timeout};
use rustbus::message_builder::{DynamicHeader, MarshalledMessage, MessageBuilder, MessageType};
use rustbus::params;
use rustbus::params::message::Message;
use rustbus::params::{Base, Container, Param};
use rustbus::signature;
use rustbus::standard_messages;
use rustbus::wire::marshal::traits::{Marshal, Signature};
use rustbus::wire::unmarshal;
use rustbus::wire::unmarshal::traits::Unmarshal;
use rustbus::wire::unmarshal::Error as UnmarshalError;
use rustbus::{get_system_bus_path, ByteOrder};
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};
use std::convert::{TryFrom, TryInto};
use std::ffi::OsString;
use std::fmt::Write;
use std::fmt::{Debug, Display, Formatter};
use std::num::ParseIntError;
use std::os::unix::prelude::{AsRawFd, RawFd};
use std::path::{Component, Path, PathBuf};
use std::rc::{Rc, Weak};

mod bluetooth_cb;
pub mod path;

enum PendingType<T: 'static, U: 'static> {
    MessageCb(&'static dyn Fn(MarshalledMessage, U) -> T),
    PreResolved(T),
}
pub struct Pending<T: 'static, U: 'static> {
    dbus_res: u32,
    typ: Option<PendingType<T, U>>,
    data: Option<U>, // this option allows it to be take
    leaking: Weak<RefCell<VecDeque<(u32, Box<dyn FnOnce(MarshalledMessage)>)>>>,
}
impl<T: 'static, U: 'static> Drop for Pending<T, U> {
    fn drop(&mut self) {
        if let Some(PendingType::MessageCb(cb)) = self.typ.take() {
            if let Some(leaking) = self.leaking.upgrade() {
                let data = self.data.take().unwrap();
                let fo_cb = move |call: MarshalledMessage| {
                    (cb)(call, data);
                };
                leaking
                    .borrow_mut()
                    .push_back((self.dbus_res, Box::new(fo_cb)));
            }
        }
    }
}
pub enum ResolveError<T: 'static, U: 'static> {
    StillPending(Pending<T, U>),
    Error(Pending<T, U>, Error),
}

pub mod interfaces;

mod advertisement;
pub use advertisement::*;
mod device;
pub use device::*;

use interfaces::*;
pub mod gatt;

mod introspect;
use introspect::Introspectable;

#[cfg(test)]
mod tests;

pub type UUID = Rc<str>;
pub type MAC = Rc<str>;

pub const MAX_APP_MTU: usize = 244;
pub const MAX_CHAR_LEN: usize = 512;

pub trait ToUUID {
    fn to_uuid(self) -> UUID;
}
impl ToUUID for &str {
    fn to_uuid(self) -> UUID {
        assert!(validate_uuid(self));
        self.into()
    }
}
impl ToUUID for String {
    fn to_uuid(self) -> UUID {
        assert!(validate_uuid(&self));
        self.into()
    }
}
impl ToUUID for UUID {
    fn to_uuid(self) -> UUID {
        self
    }
}
impl ToUUID for &UUID {
    fn to_uuid(self) -> UUID {
        self.clone()
    }
}
impl ToUUID for u128 {
    fn to_uuid(self) -> UUID {
        format!(
            "{:08x}-{:04x}-{:04x}-{:04x}-{:012x}",
            self >> 24,
            (self >> 20) & 0xFFFF,
            (self >> 16) & 0xFFFF,
            (self >> 12) & 0xFFFF,
            self & 0xFFFFFFFFFFFF
        )
        .into()
    }
}
fn validate_mac(mac: &str) -> bool {
    if mac.len() != 17 {
        return false;
    }
    let mut char_iter = mac.chars();
    for _ in 0..5 {
        if char_iter.next().unwrap().is_lowercase() {
            return false;
        }
        if char_iter.next().unwrap().is_lowercase() {
            return false;
        }
        if char_iter.next().unwrap() != ':' {
            return false;
        }
    }
    for i in 0..6 {
        let tar = i * 3;
        if u8::from_str_radix(&mac[tar..tar + 2], 16).is_err() {
            return false;
        }
    }
    true
}
pub trait ToMAC {
    fn to_mac(self) -> MAC;
}
impl ToMAC for &str {
    fn to_mac(self) -> MAC {
        assert!(validate_mac(self));
        self.into()
    }
}
impl ToMAC for String {
    fn to_mac(self) -> MAC {
        assert!(validate_mac(&self));
        self.into()
    }
}

enum DbusObject {
    Gatt(UUID, Option<(UUID, Option<UUID>)>),
    Ad(usize),
    Appl,
    None,
}

#[derive(Debug)]
pub enum Error {
    DbusClient(client_conn::Error),
    DbusReqErr(String),
    Bluez(String),
    BadInput(String),
    NoFd(String),
    Unix(nix::Error),
    Timeout,
}
impl From<nix::Error> for Error {
    fn from(err: nix::Error) -> Self {
        match err {
            nix::Error::Sys(nix::errno::Errno::EAGAIN) => Error::Timeout,
            err => Error::Unix(err),
        }
    }
}
impl Display for Error {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        Debug::fmt(&self, f)
    }
}
impl std::error::Error for Error {}
impl From<client_conn::Error> for Error {
    fn from(err: client_conn::Error) -> Self {
        Error::DbusClient(err)
    }
}
impl From<rustbus::wire::unmarshal::Error> for Error {
    fn from(err: rustbus::wire::unmarshal::Error) -> Self {
        Error::DbusReqErr(format!("Parameter failed to unmarshal: {:?}", err))
    }
}

impl TryFrom<&'_ Message<'_, '_>> for Error {
    type Error = &'static str;
    fn try_from(msg: &Message) -> Result<Self, Self::Error> {
        match msg.typ {
            MessageType::Error => (),
            _ => return Err("Message was not an error"),
        }
        let err_name = match &msg.dynheader.error_name {
            Some(name) => name,
            None => return Err("Message was missing error name"),
        };
        let err_text = if let Some(Param::Base(Base::String(err_text))) = msg.params.get(0) {
            Some(err_text)
        } else {
            None
        };
        Ok(Error::DbusReqErr(format!(
            "Dbus request error: {}, text: {:?}",
            err_name, err_text
        )))
    }
}

/// `Bluetooth` is created to interact with Bluez over DBus and file descriptors.
pub struct Bluetooth {
    rpc_con: RpcConn,
    blue_path: Rc<Path>,
    name: String,
    path: PathBuf,
    pub verbose: u8,
    services: HashMap<UUID, LocalServiceBase>,
    registered: bool,
    filter_dest: Option<(String, Option<String>)>,
    ads: VecDeque<Advertisement>,
    service_index: u8,
    ad_index: u16,
    devices: HashMap<MAC, RemoteDeviceBase>,
    comp_map: HashMap<OsString, MAC>,
    powered: Rc<Cell<bool>>,
    discoverable: Rc<Cell<bool>>,
    leaking: Rc<RefCell<VecDeque<(u32, Box<dyn FnOnce(MarshalledMessage)>)>>>,
    addr: MAC,
}
impl Bluetooth {
    /// Creates a new `Bluetooth` and setup a DBus client to interact with Bluez.
    pub fn new(dbus_name: String, blue_path: String) -> Result<Self, Error> {
        let session_path = get_system_bus_path()?;
        let conn = Conn::connect_to_bus(session_path, true)?;
        let mut rpc_con = RpcConn::new(conn);
        rpc_con.send_message(&mut standard_messages::hello(), Timeout::Infinite)?;
        let namereq = rpc_con.send_message(
            &mut standard_messages::request_name(dbus_name.clone(), 0),
            Timeout::Infinite,
        )?;
        let res = rpc_con.wait_response(namereq, Timeout::Infinite)?;
        if let Some(_) = &res.dynheader.error_name {
            return Err(Error::DbusReqErr(format!(
                "Error Dbus client name {:?}",
                res
            )));
        }
        let services = HashMap::new();
        let mut path = String::new();
        path.push('/');
        path.push_str(&dbus_name.replace(".", "/"));
        let path = PathBuf::from(path);

        let blue_path: &Path = blue_path.as_ref();
        let mut ret = Bluetooth {
            rpc_con,
            name: dbus_name,
            verbose: 0,
            services,
            registered: false,
            blue_path: blue_path.into(),
            path,
            filter_dest: None,
            ads: VecDeque::new(),
            service_index: 0,
            ad_index: 0,
            devices: HashMap::new(),
            comp_map: HashMap::new(),
            leaking: Rc::new(RefCell::new(VecDeque::new())),
            powered: Rc::new(Cell::new(false)),
            discoverable: Rc::new(Cell::new(false)),
            addr: "00:00:00:00:00:00".into(),
        };
        ret.rpc_con.set_filter(Box::new(move |msg| match msg.typ {
            MessageType::Call => true,
            MessageType::Error => true,
            MessageType::Reply => true,
            MessageType::Invalid => false,
            MessageType::Signal => true,
        }));
        ret.set_filter(Some(BLUEZ_DEST.to_string()))?;
        ret.update_adapter_props()?;
        Ok(ret)
    }
    pub fn update_adapter_props(&mut self) -> Result<(), Error> {
        // TODO: should this return a Pending
        // get properties of local adapter
        let mut msg = MessageBuilder::new()
            .call("GetAll".to_string())
            .with_interface(PROP_IF_STR.to_string())
            .on(self.blue_path.to_str().unwrap().to_string())
            .at(BLUEZ_DEST.to_string())
            .build();
        msg.body.push_param(ADAPTER_IF_STR.to_string()).unwrap();
        let res_idx = self.rpc_con.send_message(&mut msg, Timeout::Infinite)?;
        let res = self.rpc_con.wait_response(res_idx, Timeout::Infinite)?;
        match res.typ {
            MessageType::Reply => {
                let blue_props: HashMap<String, Variant> = res.body.parser().get()?;
                self.update_from_props(blue_props)
            }
            MessageType::Error => Err(Error::DbusReqErr(format!(
                "Error getting dbus adapter props: {:?}",
                res
            ))),
            _ => unreachable!(),
        }
    }
    fn update_from_props(&mut self, mut blue_props: HashMap<String, Variant>) -> Result<(), Error> {
        let powered = match blue_props.remove("Powered") {
            Some(var) => var.get()?,
            None => {
                return Err(Error::DbusReqErr(
                    "No 'Powered' property was present on adapter!".to_string(),
                ))
            }
        };
        let addr = match blue_props.remove("Address") {
            Some(var) => {
                let addr_str: String = var.get()?;
                if validate_mac(&addr_str) {
                    addr_str.to_mac()
                } else {
                    return Err(Error::DbusReqErr(
                        "'Address' property was in invalid format!".to_string(),
                    ));
                }
            }
            None => {
                return Err(Error::DbusReqErr(
                    "No 'Address' property was present on adapter!".to_string(),
                ))
            }
        };
        let discoverable = match blue_props.remove("Discoverable") {
            Some(var) => var.get()?,
            None => {
                return Err(Error::DbusReqErr(
                    "No 'Discoverable' property was present on adapter!".to_string(),
                ))
            }
        };
        self.powered.replace(powered);
        self.discoverable.replace(discoverable);
        self.addr = addr;
        Ok(())
    }
    fn update_from_changed(&mut self, blue_props: HashMap<String, Variant>) -> Result<(), Error> {
        for (prop, var) in blue_props {
            match prop.as_str() {
                "Powered" => self.powered.set(var.get()?),
                "Address" => self.addr = var.get::<String>()?.to_uuid(),
                "Discoverable" => self.discoverable.set(var.get()?),
                _ => (),
            }
        }
        Ok(())
    }
    pub fn addr(&self) -> &MAC {
        &self.addr
    }
    pub fn set_filter(&mut self, filter: Option<String>) -> Result<(), Error> {
        match filter {
            Some(name) => {
                let mut nameowner = MessageBuilder::new()
                    .call("GetNameOwner".to_string())
                    .with_interface("org.freedesktop.DBus".to_string())
                    .at("org.freedesktop.DBus".to_string())
                    .on("/org/freedesktop/DBus".to_string())
                    .build();
                nameowner.body.push_param(name.clone()).unwrap();
                let res_idx = self
                    .rpc_con
                    .send_message(&mut nameowner, Timeout::Infinite)?;
                let res = self.rpc_con.wait_response(res_idx, Timeout::Infinite)?;
                match res.typ {
                    MessageType::Reply => {
                        let owner = res.body.parser().get()?;
                        if owner == name {
                            self.filter_dest = Some((name, None));
                        } else {
                            self.filter_dest = Some((name, Some(owner)));
                        }
                    }
                    MessageType::Error => self.filter_dest = Some((name, None)),
                    _ => unreachable!(),
                }
            }
            None => self.filter_dest = None,
        }
        Ok(())
    }
    /// Gets the path of the DBus client
    pub fn get_path(&self) -> &Path {
        &self.path
    }
    /// Adds a service to the `Bluetooth` instance. Once registered with [`register_application()`],
    /// this service will be a local service that can be interacted with by remote devices.
    ///
    /// If `register_application()` has already called, the service will not be visible to
    /// Bluez (or other devices) until the application in reregistered.
    ///
    /// [`register_application()`]: ./struct.Bluetooth.html#method.register_application
    pub fn add_service(&mut self, mut service: LocalServiceBase) -> Result<(), Error> {
        if self.services.len() >= 255 {
            panic!("Cannot add more than 255 services");
        }
        service.index = self.service_index;
        self.service_index += 1;
        let path = self.path.to_owned();
        service.update_path(path);
        self.services.insert(service.uuid.clone(), service);
        Ok(())
    }
    /// Access a service that has been added to the `Bluetooth` instance.
    pub fn get_service<T: ToUUID>(&mut self, uuid: T) -> Option<LocalService<'_>> {
        let uuid = uuid.to_uuid();
        if self.services.contains_key(&uuid) {
            Some(LocalService {
                uuid: uuid.clone(),
                bt: self,
            })
        } else {
            None
        }
    }
    /// Gets a remote device by `MAC`. The device must be discovered using `discover_devices()` from bluez,
    /// before it can be gotten with this device.
    pub fn get_device<'c>(&'c mut self, mac: &MAC) -> Option<RemoteDevice<'c>> {
        let _base = self.devices.get_mut(mac)?;
        Some(RemoteDevice {
            blue: self,
            mac: mac.clone(),
            #[cfg(feature = "unsafe-opt")]
            ptr: _base,
        })
    }
    /// Gets a `HashSet` of known devices' `MAC` addresses.
    pub fn devices(&self) -> Vec<MAC> {
        self.devices.keys().map(|x| x.clone()).collect()
    }
    fn register_adv(&mut self, adv_idx: usize) -> Result<(), Error> {
        let mut msg = MessageBuilder::new()
            .call("RegisterAdvertisement".to_string())
            .with_interface("org.bluez.LEAdvertisingManager1".to_string())
            .on(self.blue_path.to_str().unwrap().to_string())
            .at(BLUEZ_DEST.to_string())
            .build();
        let dict = params::Dict {
            key_sig: signature::Base::String,
            value_sig: signature::Type::Container(signature::Container::Variant),
            map: HashMap::new(),
        };
        let adv_path_str = self.ads[adv_idx].path.to_str().unwrap().to_string();
        msg.body
            .push_old_params(&[
                Param::Base(Base::ObjectPath(adv_path_str)),
                Param::Container(Container::Dict(dict)),
            ])
            .unwrap();
        let res_idx = self.rpc_con.send_message(&mut msg, Timeout::Infinite)?;
        loop {
            self.process_requests()?;
            if let Some(res) = self.rpc_con.try_get_response(res_idx) {
                return match res.typ {
                    MessageType::Error => {
                        let mut err = None;
                        let res = res.unmarshall_all().unwrap();
                        if let Some(err_str) = res.params.get(0) {
                            if let Param::Base(Base::String(err_str)) = err_str {
                                err = Some(err_str);
                            }
                        }
                        let err_str = if let Some(err) = err {
                            format!(
                                "Failed to register application with bluez: {}: {:?}",
                                res.dynheader.error_name.unwrap(),
                                err
                            )
                        } else {
                            format!(
                                "Failed to register application with bluez: {}",
                                res.dynheader.error_name.unwrap()
                            )
                        };
                        // eprintln!("error: {}", err_str);
                        Err(Error::Bluez(err_str))
                    }
                    MessageType::Reply => {
                        if self.verbose >= 1 {
                            eprintln!("Registered application with bluez.");
                        };
                        self.ads.back_mut().unwrap().active = true;
                        Ok(())
                    }
                    _ => unreachable!(),
                };
            }
        }
    }
    /// Registers an advertisement with Bluez.
    /// After a successful call, it will persist until the `remove_advertise()`/`remove_advertise_no_dbus()`
    /// is called or Bluez releases the advertisement (this is typically done on device connect).
    ///
    /// **Calls process_requests()**
    pub fn start_adv(&mut self, mut adv: Advertisement) -> Result<u16, (u16, Error)> {
        let idx = self.ads.len();
        adv.index = self.ad_index;
        self.ad_index += 1;
        let adv_path = self.path.join(format!("adv{:04x}", adv.index));
        adv.path = adv_path;
        self.ads.push_back(adv);
        self.register_adv(idx)
            .map(|_| idx as u16)
            .map_err(|err| (idx as u16, err))
    }
    /// Checks if an advertisement is still active, or if Bluez has signaled it has ended.
    pub fn is_adv_active(&self, index: u16) -> Option<bool> {
        let adv = self.ads.iter().find(|ad| ad.index == index)?;
        Some(adv.active)
    }
    /// Restart an inactive advertisement.
    ///
    /// If the advertisement is already active this method,
    /// does nothing and returns `false`. If an advertisement is not active it tries to
    /// reregister the advertisement and returns `true` on success otherwise it returns and `Err`.
    pub fn restart_adv(&mut self, index: u16) -> Result<bool, Error> {
        let idx = match self.ads.iter().position(|ad| ad.index == index) {
            Some(idx) => idx,
            None => {
                return Err(Error::BadInput(format!(
                    "Advertisement index {} not found.",
                    index
                )))
            }
        };
        if self.ads[idx].active {
            Ok(false)
        } else {
            self.register_adv(idx).map(|_| true)
        }
    }
    /// Unregisters an advertisement with Bluez. Returns the `Advertisement` if successful.
    ///
    /// **Calls process_requests()**
    pub fn remove_adv(&mut self, index: u16) -> Result<Advertisement, Error> {
        let idx = match self.ads.iter().position(|ad| ad.index == index) {
            Some(idx) => idx,
            None => {
                return Err(Error::BadInput(format!(
                    "Advertisement index {} not found.",
                    index
                )))
            }
        };
        if !self.ads[idx].active {
            return Ok(self.ads.remove(idx).unwrap());
        }
        let mut msg = MessageBuilder::new()
            .call("UnregisterAdvertisement".to_string())
            .with_interface("org.bluez.LEAdvertisingManager1".to_string())
            .on(self.blue_path.to_str().unwrap().to_string())
            .at(BLUEZ_DEST.to_string())
            .build();
        let path = self.ads[idx].path.to_str().unwrap().to_string();
        msg.body
            .push_old_param(&Param::Base(Base::ObjectPath(path)))
            .unwrap();
        let res_idx = self.rpc_con.send_message(&mut msg, Timeout::Infinite)?;
        loop {
            self.process_requests()?;
            if let Some(res) = self.rpc_con.try_get_response(res_idx) {
                match res.typ {
                    MessageType::Reply => {
                        let mut adv = self.ads.remove(idx).unwrap();
                        adv.active = false;
                        return Ok(adv);
                    }
                    MessageType::Error => {
                        return Err(Error::DbusReqErr(format!(
                            "UnregisterAdvertisement call failed: {:?}",
                            res
                        )))
                    }
                    _ => unreachable!(),
                }
            }
        }
    }
    pub fn remove_all_adv(&mut self) -> Result<(), Error> {
        while self.ads.len() > 0 {
            self.remove_adv(self.ads[0].index)?;
        }
        Ok(())
    }
    /// Removes the advertisement from the `Bluetooth` instance but does not unregister the
    /// advertisement with Bluez. It is recommended that this is not used.
    pub fn remove_adv_no_dbus(&mut self, index: u16) -> Option<Advertisement> {
        let idx = self.ads.iter().position(|ad| ad.index == index)?;
        self.ads.remove(idx)
    }
    /// Set the Bluez controller power on (`true`) or off.
    ///
    /// **Calls process_requests()**
    pub fn set_power(
        &mut self,
        on: bool,
    ) -> Result<Pending<Result<(), Error>, (Rc<Cell<bool>>, bool, &'static str)>, Error> {
        let mut msg = MessageBuilder::new()
            .call("Set".to_string())
            .on(self.blue_path.to_str().unwrap().to_string())
            .with_interface(PROP_IF_STR.to_string())
            .at(BLUEZ_DEST.to_string())
            .build();
        msg.body.push_param2(ADAPTER_IF_STR, "Powered").unwrap();
        msg.body.push_variant(on).unwrap();
        let dbus_res = self.rpc_con.send_message(&mut msg, Timeout::Infinite)?;
        Ok(Pending {
            typ: Some(PendingType::MessageCb(&bluetooth_cb::set_power_cb)),
            dbus_res,
            data: Some((self.powered.clone(), on, bluetooth_cb::POWER)),
            leaking: Rc::downgrade(&self.leaking),
        })
    }
    pub fn set_power_wait(&mut self, on: bool) -> Result<(), Error> {
        let pend = self.set_discoverable(on)?;
        self.wait_result_variant(pend)
    }
    /// Set whether the Bluez controller should be discoverable (`true`) or not.
    ///
    /// **Calls process_requests()**
    pub fn set_discoverable(
        &mut self,
        on: bool,
    ) -> Result<Pending<Result<(), Error>, (Rc<Cell<bool>>, bool, &'static str)>, Error> {
        let mut msg = MessageBuilder::new()
            .call("Set".to_string())
            .on(self.blue_path.to_str().unwrap().to_string())
            .with_interface(PROP_IF_STR.to_string())
            .at(BLUEZ_DEST.to_string())
            .build();
        msg.body
            .push_param2(ADAPTER_IF_STR, "Discoverable")
            .unwrap();
        msg.body.push_variant(on).unwrap();
        let dbus_res = self.rpc_con.send_message(&mut msg, Timeout::Infinite)?;
        Ok(Pending {
            typ: Some(PendingType::MessageCb(&bluetooth_cb::set_power_cb)),
            dbus_res,
            data: Some((self.discoverable.clone(), on, bluetooth_cb::DISCOVERABLE)),
            leaking: Rc::downgrade(&self.leaking),
        })
    }
    pub fn set_discoverable_wait(&mut self, on: bool) -> Result<(), Error> {
        let pend = self.set_discoverable(on)?;
        self.wait_result_variant(pend)
    }
    fn wait_result_variant<O, U>(
        &mut self,
        pend: Pending<Result<O, Error>, U>,
    ) -> Result<O, Error> {
        match self.resolve(pend) {
            Ok(res) => res,
            Err((pend, err)) => Err(err),
        }
    }
    pub fn power(&mut self) -> bool {
        self.powered.get()
    }
    /// Registers the local application's GATT services/characteristics (TODO: descriptors)
    /// with the Bluez controller.
    ///
    /// **Calls process_requests()**
    pub fn register_application(&mut self) -> Result<(), Error> {
        let path = self.get_path();
        let empty_dict = HashMap::new();
        let dict = params::Dict {
            key_sig: signature::Base::String,
            value_sig: signature::Type::Container(signature::Container::Variant),
            map: empty_dict,
        };
        let call_builder = MessageBuilder::new().call(REGISTER_CALL.to_string());
        /*call_builder.add_param2(
            Param::Base(Base::ObjectPath(
                path.as_os_str().to_str().unwrap().to_string(),
            )),
            Param::Container(Container::Dict(dict)),
        );*/
        let mut msg = call_builder
            .with_interface(MANAGER_IF_STR.to_string())
            .on(self.blue_path.to_str().unwrap().to_string())
            .at(BLUEZ_DEST.to_string())
            .build();

        msg.body
            .push_old_params(&[
                Param::Base(Base::ObjectPath(
                    path.as_os_str().to_str().unwrap().to_string(),
                )),
                Param::Container(Container::Dict(dict)),
            ])
            .unwrap();

        // eprintln!("registration msg: {:#?}", msg);
        let msg_idx = self.rpc_con.send_message(&mut msg, Timeout::Infinite)?;
        // we expect there to be no response
        loop {
            self.process_requests()?;
            if let Some(res) = self.rpc_con.try_get_response(msg_idx) {
                return if let MessageType::Error = res.typ {
                    let mut err = None;
                    let res = res.unmarshall_all().unwrap();
                    if let Some(err_str) = res.params.get(0) {
                        if let Param::Base(Base::String(err_str)) = err_str {
                            err = Some(err_str);
                        }
                    }
                    let err_str = if let Some(err) = err {
                        format!(
                            "Failed to register application with bluez: {}: {:?}",
                            res.dynheader.error_name.unwrap(),
                            err
                        )
                    } else {
                        format!(
                            "Failed to register application with bluez: {}",
                            res.dynheader.error_name.unwrap()
                        )
                    };
                    // eprintln!("error: {}", err_str);
                    Err(Error::Bluez(err_str))
                } else {
                    if self.verbose >= 1 {
                        eprintln!("Registered application with bluez.");
                    };
                    Ok(())
                };
            }
        }
    }
    /// **Unimplemented**
    ///
    /// Unregisters the local GATT services from the Bluez controller.
    ///
    /// **Calls process_requests()**
    pub fn unregister_application(&mut self) -> Result<(), Error> {
        unimplemented!();
        self.registered = false;
        Ok(())
    }

    fn check_incoming(&self, sender: &str) -> bool {
        match &self.filter_dest {
            Some(dest) => {
                !(&dest.0 != sender && (dest.1 == None || dest.1.as_ref().unwrap() != sender))
            }
            None => true,
        }
    }
    /// Process incoming DBus requests for the local application.
    ///
    /// When using `Bluetooth` this function should be called on a regular basis.
    /// Bluez uses DBus to handle read/write requests to characteristics and descriptors, as wells
    /// advertisement. Failure to call this method frequently enough could result in requests from
    /// GATT clients to timeout. Some other functions are guaranteed to call this function at least
    /// once while waiting for a responses from the Bluez controller. This property is noted in these
    /// functions' descriptions.
    ///
    pub fn process_requests(&mut self) -> Result<(), Error> {
        let responses = self.rpc_con.refill_all()?;
        for mut response in responses {
            self.rpc_con
                .send_message(&mut response, Timeout::Infinite)?;
        }
        let mut leaking_bm = self.leaking.borrow_mut();
        while leaking_bm.len() > 0 {
            match self.rpc_con.try_get_response(leaking_bm[0].0) {
                Some(call) => {
                    let (_, cb) = leaking_bm.pop_front().unwrap();
                    (cb)(call);
                }
                None => break,
            }
        }
        drop(leaking_bm);
        while let Some(call) = self.rpc_con.try_get_call() {
            // eprintln!("received call {:?}", call);
            let interface = (&call.dynheader.interface).as_ref().unwrap();
            let sender = call.dynheader.sender.as_ref().unwrap();
            if !self.check_incoming(sender) {
                let mut msg = call.dynheader.make_error_response(
                    BLUEZ_NOT_PERM.to_string(),
                    Some("Sender is not allowed to perform this action.".to_string()),
                );
                self.rpc_con.send_message(&mut msg, Timeout::Infinite)?;
                continue;
            }
            let mut reply = match self.match_root(&call.dynheader) {
                DbusObject::Appl => match interface.as_ref() {
                    PROP_IF_STR => self.properties_call(call),
                    OBJ_MANAGER_IF_STR => self.objectmanager_call(call),
                    INTRO_IF_STR => self.introspectable(call),
                    _ => standard_messages::unknown_method(&call.dynheader),
                },
                DbusObject::Gatt(serv_uuid, child_uuid) => {
                    let serv_base = self.services.get_mut(&serv_uuid).unwrap();
                    match child_uuid {
                        Some((char_uuid, child_uuid)) => {
                            let char_base = serv_base.chars.get_mut(&char_uuid).unwrap();
                            match child_uuid {
                                Some(desc_uuid) => {
                                    // Descriptor
                                    let desc_base = char_base.descs.get_mut(&desc_uuid).unwrap();
                                    match interface.as_ref() {
                                        PROP_IF_STR => desc_base.properties_call(call),
                                        DESC_IF_STR => {
                                            let mut serv = LocalService::new(self, serv_uuid);
                                            let mut character =
                                                LocalCharactersitic::new(&mut serv, char_uuid);
                                            let mut desc =
                                                LocalDescriptor::new(&mut character, desc_uuid);
                                            desc.desc_call(call)
                                        }
                                        INTRO_IF_STR => desc_base.introspectable(call),
                                        _ => standard_messages::unknown_method(&call.dynheader),
                                    }
                                }
                                None => {
                                    // Charactersitic
                                    match interface.as_ref() {
                                        PROP_IF_STR => char_base.properties_call(call),
                                        CHAR_IF_STR => {
                                            let mut serv = LocalService::new(self, serv_uuid);
                                            let mut character =
                                                LocalCharactersitic::new(&mut serv, char_uuid);
                                            character.char_call(call)
                                        }
                                        INTRO_IF_STR => char_base.introspectable(call),
                                        _ => standard_messages::unknown_method(&call.dynheader),
                                    }
                                }
                            }
                        }
                        None => {
                            // Service
                            match interface.as_ref() {
                                PROP_IF_STR => serv_base.properties_call(call),
                                SERV_IF_STR => serv_base.service_call(call),
                                INTRO_IF_STR => serv_base.introspectable(call),
                                _ => standard_messages::unknown_method(&call.dynheader),
                            }
                        }
                    }
                }
                DbusObject::Ad(ad_idx) => {
                    let adv = &mut self.ads[ad_idx];
                    match interface.as_ref() {
                        PROP_IF_STR => adv.properties_call(call),
                        LEAD_IF_STR => match call.dynheader.member.as_ref().unwrap().as_str() {
                            "Release" => {
                                adv.active = false;
                                call.dynheader.make_response()
                            }
                            _ => standard_messages::unknown_method(&call.dynheader),
                        },
                        INTRO_IF_STR => adv.introspectable(call),
                        _ => standard_messages::unknown_method(&call.dynheader),
                    }
                }
                DbusObject::None => standard_messages::unknown_method(&call.dynheader),
            };
            /*
            // eprintln!("replying: {:?}", reply);
            match reply.body.parser().get_param() {
                // Ok(param) => eprintln!("reply body: first param: {:#?}", param),
                // Err(_) => eprintln!("reply body: no params"),
            }
            */
            self.rpc_con.send_message(&mut reply, Timeout::Infinite)?;
            for fd in reply.raw_fds {
                close(fd).ok();
            }
        }
        while let Some(sig) = self.rpc_con.try_get_signal() {
            match sig.dynheader.interface.as_ref().unwrap().as_str() {
                OBJ_MANAGER_IF_STR => {
                    if !self.check_incoming(sig.dynheader.sender.as_ref().unwrap()) {
                        continue;
                    }
                    match sig.dynheader.member.as_ref().unwrap().as_str() {
                        IF_ADDED_SIG => self.interface_added(sig)?,
                        IF_REMOVED_SIG => self.interface_removed(sig)?,
                        _ => (),
                    }
                }
                DBUS_IF_STR => {
                    if sig.dynheader.sender.unwrap() != "org.freedesktop.Dbus" {
                        continue;
                    }
                    match sig.dynheader.member.as_ref().unwrap().as_str() {
                        NAME_LOST_SIG => {
                            if let Some(filter) = &mut self.filter_dest {
                                let lost_name: &str = sig.body.parser().get()?;
                                if filter.0 == lost_name {
                                    filter.1 = None;
                                    if self.verbose > 0 {
                                        eprintln!(
                                            "{} has disconnected for DBus?",
                                            self.filter_dest.as_ref().unwrap().0
                                        );
                                    }
                                    self.clear_devices();
                                }
                            }
                        }
                        NAME_OWNER_CHANGED => {}
                        _ => (),
                    }
                }
                PROP_IF_STR => {
                    if !self.check_incoming(sig.dynheader.sender.as_ref().unwrap()) {
                        continue;
                    }
                    match sig.dynheader.member.as_ref().unwrap().as_str() {
                        PROP_CHANGED_SIG => self.properties_changed(sig)?,
                        _ => (),
                    }
                }
                _ => (),
            }
        }
        Ok(())
    }
    fn properties_changed(&mut self, sig: MarshalledMessage) -> Result<(), Error> {
        if let Some(child) = self.match_remote(&sig.dynheader) {
            let mut parser = sig.body.parser();
            let interface: &str = parser.get()?;
            let changed = parser.get()?;
            match child {
                Some((dev_mac, child)) => {
                    let dev = self.devices.get_mut(&dev_mac).unwrap();
                    match child {
                        Some((serv_uuid, child)) => {
                            let serv = dev.get_child(&serv_uuid).unwrap();
                            match child {
                                Some((char_uuid, child)) => {
                                    let mut character = serv.get_child(&char_uuid).unwrap();
                                    match child {
                                        Some(desc_uuid) => unimplemented!(),
                                        None => {
                                            if interface == CHAR_IF_STR {
                                                character.update_from_changed(changed)?;
                                            }
                                        }
                                    }
                                }
                                None => {
                                    if interface == SERV_IF_STR {
                                        serv.update_from_changed(changed)?;
                                    }
                                }
                            }
                        }
                        None => {
                            if interface == DEV_IF_STR {
                                dev.update_from_changed(changed)?;
                            }
                        }
                    }
                }
                None => {
                    if interface == ADAPTER_IF_STR {
                        self.update_from_changed(changed)?;
                    }
                }
            }
        }
        Ok(())
        /*
        Some(None) => {
            let mut parser = sig.body.parser();
        }
        Some(Some((mac, None))) => {
            let mut parser = sig.body.parser();
            let interface: &str = parser.get()?;
            if interface == DEV_IF_STR {
            } else {
                Ok(())
            }

        }
        Some(Some((mac, Some((serv_uuid, child_uuid))))) => unimplemented!(),
        _ => Ok(()),
        */
    }
    fn interface_added(&mut self, sig: MarshalledMessage) -> Result<(), Error> {
        match self.match_remote(&sig.dynheader) {
            Some(Some((mac, Some((serv_uuid, child_uuid))))) => {
                let dev = self.devices.get_mut(&mac).unwrap();
                let serv = dev.services.get_mut(&serv_uuid).unwrap();
                match child_uuid {
                    Some((char_uuid, child_uuid)) => unimplemented!(),
                    None => {
                        let mut i_and_p: HashMap<String, HashMap<String, Variant>> =
                            sig.body.parser().get()?;
                        match i_and_p.remove("org.bluez.GattService1") {
                            Some(props) => serv.update_all(props),
                            None => Ok(()),
                        }
                    }
                }
            }
            _ => Ok(()),
        }
    }
    fn interface_removed(&mut self, sig: MarshalledMessage) -> Result<(), Error> {
        match self.match_remote(&sig.dynheader) {
            Some(Some((mac, child_uuid))) => match child_uuid {
                Some((serv_uuid, child_uuid)) => unimplemented!(),
                None => unimplemented!(),
            },
            Some(None) | None => Ok(()),
        }
    }
    fn match_root(&mut self, dynheader: &DynamicHeader) -> DbusObject {
        let path = self.get_path();
        if let None = &dynheader.interface {
            return DbusObject::None;
        }
        if let None = &dynheader.member {
            return DbusObject::None;
        }
        // eprintln!("For path: {:?}, Checking msg for match", path);
        let object = &dynheader.object.as_ref().unwrap();
        let obj_path: &Path = object.as_ref();

        if path.starts_with(obj_path) {
            DbusObject::Appl
        } else {
            let serv_path = match obj_path.strip_prefix(path) {
                Ok(path) => path,
                Err(_) => return DbusObject::None,
            };
            if let Some(matc) = self.match_services(serv_path) {
                return DbusObject::Gatt(matc.0, matc.1);
            }
            match self.match_advertisement(serv_path) {
                Some(idx) => DbusObject::Ad(idx),
                None => DbusObject::None,
            }
        }
    }
    fn match_remote(
        &mut self,
        header: &DynamicHeader,
    ) -> Option<Option<(MAC, Option<(UUID, Option<(UUID, Option<UUID>)>)>)>> {
        let path: &Path = header.object.as_ref().unwrap().as_ref();
        let path = match path.strip_prefix(self.blue_path.as_ref()) {
            Ok(p) => p,
            Err(_) => return None,
        };
        let first_comp = match path.components().next() {
            Some(Component::Normal(p)) => p.to_str().unwrap(),
            None => return Some(None),
            _ => return None,
        };
        let mac = devmac_to_mac(first_comp)?;
        let dev = self.devices.get_mut(&mac)?;
        let uuids = dev.match_dev(path.strip_prefix(&first_comp).unwrap())?;
        Some(Some((mac, uuids)))
    }
    fn match_services(&mut self, path: &Path) -> Option<(UUID, Option<(UUID, Option<UUID>)>)> {
        let r_str = path.to_str().unwrap();
        if (r_str.len() != 9 && r_str.len() != 18 && r_str.len() != 27) || &r_str[..4] != "serv" {
            return None;
        }
        for uuid in self.get_children() {
            if let Some(matc) = match_serv(&mut self.get_child(&uuid).unwrap(), path) {
                return Some((uuid, matc));
                //return Some(Some((uuid, matc)));
            }
        }
        None
    }
    fn match_advertisement(&self, path: &Path) -> Option<usize> {
        let r_str = path.to_str().unwrap();
        if r_str.len() != 7 || &r_str[..4] != "adv" {
            return None;
        }
        for (i, adv) in self.ads.iter().enumerate() {
            if adv.path.file_name().unwrap() == path {
                return Some(i);
            }
        }
        None
    }
    pub fn clear_devices(&mut self) {
        self.devices.clear()
    }
    /// Used to get devices devices known to Bluez. This function does *not* trigger scan/discovery
    /// on the Bluez controller. Use `set_scan()` to initiate actual device discovery.
    ///
    /// **Calls process_requests()**
    pub fn discover_devices(&mut self) -> Result<Vec<MAC>, Error> {
        self.discover_devices_filter(self.blue_path.clone())
    }

    /*
    /// **Unimplemented**
    pub fn set_scan(&mut self, on: bool) -> Result<(), Error> {
        let mut msg = MessageBuilder::new()
            .call("Set".to_string())
            .on(self.blue_path.to_str().unwrap().to_string())
            .with_interface(PROP_IF_STR.to_string())
            .at(BLUEZ_DEST.to_string())
            .build();
        msg.body.push_param2(ADAPTER_IF_STR, "Scan").unwrap();
        let variant = Param::Container(Container::Variant(Box::new(params::Variant {
            sig: rustbus::signature::Type::Base(rustbus::signature::Base::Boolean),
            value: Param::Base(Base::Boolean(on)),
        })));
        msg.body.push_old_param(&variant).unwrap();
        let res_idx = self.rpc_con.send_message(&mut msg, Timeout::Infinite)?;
        loop {
            self.process_requests()?;
            if let Some(res) = self.rpc_con.try_get_response(res_idx) {
                match res.typ {
                    MessageType::Reply => return Ok(()),
                    MessageType::Error => {
                        return Err(Error::DbusReqErr(format!(
                            "Set power call failed: {:?}",
                            res
                        )))
                    }
                    _ => unreachable!(),
                }
            }
        }
    }
    */
    pub fn try_resolve<T, V, U>(
        &mut self,
        mut pend: Pending<T, U>,
    ) -> Result<T, ResolveError<T, U>> {
        debug_assert_eq!(Rc::as_ptr(&self.leaking), pend.leaking.as_ptr());
        match pend.typ.as_ref().unwrap() {
            PendingType::PreResolved(_) => match pend.typ.take().unwrap() {
                PendingType::PreResolved(t) => Ok(t),
                _ => unreachable!(),
            },
            PendingType::MessageCb(cb) => {
                if let Err(e) = self.process_requests() {
                    return Err(ResolveError::Error(pend, e));
                }
                match self.rpc_con.try_get_response(pend.dbus_res) {
                    Some(res) => {
                        let data = pend.data.take().unwrap();
                        let ret = (cb)(res, data);
                        pend.typ.take();
                        Ok(ret)
                    }
                    None => Err(ResolveError::StillPending(pend)),
                }
            }
        }
    }
    pub fn resolve<T, U>(&mut self, mut pend: Pending<T, U>) -> Result<T, (Pending<T, U>, Error)> {
        debug_assert_eq!(Rc::as_ptr(&self.leaking), pend.leaking.as_ptr());
        match pend.typ.take().unwrap() {
            PendingType::PreResolved(t) => Ok(t),
            PendingType::MessageCb(cb) => loop {
                if let Err(e) = self.process_requests() {
                    break Err((pend, e));
                }
                if let Some(res) = self.rpc_con.try_get_response(pend.dbus_res) {
                    let data = pend.data.take().unwrap();
                    let ret = (cb)(res, data);
                    break Ok(ret);
                }
            },
        }
    }

    fn discover_devices_filter<'a, T: AsRef<Path>>(
        &mut self,
        filter_path: T,
    ) -> Result<Vec<MAC>, Error> {
        self.devices.clear();
        let mut msg = MessageBuilder::new()
            .call(MANGAGED_OBJ_CALL.to_string())
            .at(BLUEZ_DEST.to_string())
            .on("/".to_string())
            .with_interface(OBJ_MANAGER_IF_STR.to_string())
            .build();

        let res_idx = self.rpc_con.send_message(&mut msg, Timeout::Infinite)?;
        loop {
            self.process_requests()?;
            if let Some(res) = self.rpc_con.try_get_response(res_idx) {
                if let MessageType::Error = res.typ {
                    return Err(Error::DbusReqErr(format!(
                        "Failed to discover device: {:?}",
                        res
                    )));
                }
                let filter_path = filter_path.as_ref();
                let path_map: HashMap<
                    path::ObjectPathBuf,
                    HashMap<String, HashMap<String, Variant>>,
                > = res.body.parser().get()?;
                let mut pairs: Vec<(
                    path::ObjectPathBuf,
                    HashMap<String, HashMap<String, Variant>>,
                )> = path_map
                    .into_iter()
                    .filter(|pair| pair.0.starts_with(filter_path))
                    .collect();
                pairs.sort_by(|a, b| a.0.cmp(&b.0));
                let mut ret = Vec::new();
                let mut device_base: Option<RemoteDeviceBase> = None;
                let mut service_base: Option<RemoteServiceBase> = None;
                let mut character_base: Option<RemoteCharBase> = None;
                let mut descriptor_base: Option<RemoteDescBase> = None;
                for (path, mut if_map) in pairs {
                    if let Some(dev_base_props) = if_map.remove(DEV_IF_STR) {
                        // handle adding remote device
                        if let Some(mut dev) = device_base {
                            if let Some(mut serv_base) = service_base.take() {
                                if let Some(mut char_base) = character_base.take() {
                                    if let Some(desc_base) = descriptor_base.take() {
                                        char_base.descs.insert(desc_base.uuid().clone(), desc_base);
                                    }
                                    serv_base.chars.insert(char_base.uuid().clone(), char_base);
                                }
                                dev.services.insert(serv_base.uuid().clone(), serv_base);
                            }
                            ret.push(dev.uuid().clone());
                            self.insert_device(dev);
                        }
                        device_base =
                            Some(RemoteDeviceBase::from_props(dev_base_props, path.into())?);
                    } else if let Some(dev_base) = &mut device_base {
                        if let Some(serv_base_props) = if_map.remove(SERV_IF_STR) {
                            // add remote service
                            if !path.starts_with(&dev_base.path) {
                                continue;
                            }
                            if let Some(mut serv_base) = service_base {
                                if let Some(mut char_base) = character_base.take() {
                                    if let Some(desc_base) = descriptor_base.take() {
                                        char_base.descs.insert(desc_base.uuid().clone(), desc_base);
                                    }
                                    serv_base.chars.insert(char_base.uuid().clone(), char_base);
                                }
                                dev_base
                                    .services
                                    .insert(serv_base.uuid().clone(), serv_base);
                            }
                            service_base =
                                Some(RemoteServiceBase::from_props(serv_base_props, path.into())?);
                        } else if let Some(serv_base) = &mut service_base {
                            if let Some(char_base_props) = if_map.remove(CHAR_IF_STR) {
                                if !path.starts_with(serv_base.path()) {
                                    continue;
                                }
                                if let Some(mut char_base) = character_base {
                                    if let Some(desc_base) = descriptor_base.take() {
                                        char_base.descs.insert(desc_base.uuid().clone(), desc_base);
                                    }
                                    serv_base.chars.insert(char_base.uuid().clone(), char_base);
                                }
                                character_base =
                                    Some(RemoteCharBase::from_props(char_base_props, path.into())?);
                            } else if let Some(char_base) = &mut character_base {
                                if let Some(desc_base_props) = if_map.remove(DESC_IF_STR) {
                                    if !path.starts_with(char_base.path()) {
                                        continue;
                                    }
                                    if let Some(desc_base) = descriptor_base {
                                        char_base.descs.insert(desc_base.uuid().clone(), desc_base);
                                    }
                                    descriptor_base = Some(RemoteDescBase::from_props(
                                        desc_base_props,
                                        path.into(),
                                    )?);
                                }
                            }
                        }
                    }
                }
                // handle stragalers
                if let Some(mut dev_base) = device_base {
                    if let Some(mut serv_base) = service_base {
                        if let Some(mut char_base) = character_base {
                            if let Some(desc_base) = descriptor_base {
                                char_base.descs.insert(desc_base.uuid().clone(), desc_base);
                            }
                            serv_base.chars.insert(char_base.uuid().clone(), char_base);
                        }
                        dev_base
                            .services
                            .insert(serv_base.uuid().clone(), serv_base);
                    }
                    self.devices.insert(dev_base.uuid().clone(), dev_base);
                }
                return Ok(ret);
            }
        }
    }
    fn insert_device(&mut self, device: RemoteDeviceBase) {
        let devmac = device.mac.clone();
        let comp = device.path.file_name().unwrap().to_os_string();
        self.devices.insert(devmac.clone(), device);
        self.comp_map.insert(comp, devmac);
    }
    /// Get a device from the Bluez controller.
    ///
    /// **Calls process_requests()**
    pub fn discover_device(&mut self, mac: &MAC) -> Result<(), Error> {
        let devmac: PathBuf = match mac_to_devmac(mac) {
            Some(devmac) => devmac,
            None => return Err(Error::BadInput("Invalid mac was given".to_string())),
        }
        .into();
        self.discover_devices_filter(&self.blue_path.join(devmac))
            .map(|_| ())
    }
}

impl AsRawFd for Bluetooth {
    fn as_raw_fd(&self) -> RawFd {
        unsafe {
            // SAFETY: This is safe because as_raw_fd() use an immutable receiver (&self)
            // We need this unsafe code because conn_mut() is only accessible with a mutable reference
            let rpc_con = &self.rpc_con as *const RpcConn as *mut RpcConn;
            let rpc_con = &mut *rpc_con;
            rpc_con.conn_mut().as_raw_fd()
        }
    }
}
pub fn mac_to_devmac(mac: &MAC) -> Option<String> {
    if !validate_mac(mac) {
        return None;
    }
    let mut ret = String::with_capacity(21);
    ret.push_str("dev");
    for i in 0..6 {
        let tar = i * 3;
        ret.push('_');
        ret.push_str(&mac[tar..tar + 2]);
    }
    Some(ret)
}
pub fn validate_devmac(devmac: &str) -> bool {
    if devmac.len() != 21 {
        return false;
    }
    if !devmac.starts_with("dev") {
        return false;
    }
    let devmac = &devmac[3..];
    let mut chars = devmac.chars();
    for _ in 0..6 {
        if chars.next().unwrap() != '_' {
            return false;
        }
        if chars.next().unwrap().is_lowercase() || chars.next().unwrap().is_lowercase() {
            return false;
        }
    }
    true
}
pub fn devmac_to_mac(devmac: &str) -> Option<MAC> {
    if !validate_devmac(&devmac) {
        return None;
    }
    let mut ret = String::with_capacity(17);
    let devmac = &devmac[3..];
    for i in 0..5 {
        let tar = i * 3 + 1;
        ret.push_str(&devmac[tar..tar + 2]);
        ret.push(':');
    }
    ret.push_str(&devmac[16..18]);
    Some(ret.into())
}
/*
pub fn unknown_method<'a, 'b>(call: &Message<'_,'_>) -> Message<'a,'b> {
    let text = format!(
        "No calls to {}.{} are accepted for object {}",
        call.interface.clone().unwrap_or_else(|| "".to_owned()),
        call.member.clone().unwrap_or_else(|| "".to_owned()),
        call.object.clone().unwrap_or_else(|| "".to_owned()),
    );
    let err_name = "org.freedesktop.DBus.Error.UnknownMethod".to_owned();
    let mut err_resp = Message {
            typ: MessageType::Reply,
            interface: None,
            member: None,
            params: Vec::new(),
            object: None,
            destination: call.sender.clone(),
            serial: None,
            raw_fds: Vec::new(),
            num_fds: None,
            sender: None,
            response_serial: call.serial,
            dynheader.error_name: Some(err_name),
            flags: 0,
        };
        err_resp.push_param(text);
        err_resp

}
*/
trait ObjectManager {
    fn objectmanager_call(&mut self, msg: MarshalledMessage) -> MarshalledMessage {
        match msg.dynheader.member.as_ref().unwrap().as_ref() {
            MANGAGED_OBJ_CALL => self.get_managed_object(msg),
            _ => standard_messages::unknown_method(&msg.dynheader),
        }
    }
    fn object_manager_type() -> signature::Type {
        signature::Type::Container(signature::Container::Dict(
            signature::Base::String,
            Box::new(LocalServiceBase::get_all_type()),
        ))
    }

    fn get_managed_object(&mut self, msg: MarshalledMessage) -> MarshalledMessage;
}

impl ObjectManager for Bluetooth {
    fn get_managed_object(&mut self, msg: MarshalledMessage) -> MarshalledMessage {
        let mut reply = msg.dynheader.make_response();
        let mut outer_dict: HashMap<Base, Param> = HashMap::new();
        for service in self.services.values_mut() {
            //let service_path = path.join(format!("service{:02x}", service.index));
            for characteristic in service.chars.values_mut() {
                for desc in characteristic.descs.values_mut() {
                    let mut middle_map = HashMap::new();
                    for interface in LocalDescBase::INTERFACES {
                        let props = desc.get_all_inner(interface.0).unwrap();
                        middle_map.insert(interface.0.to_string().into(), props);
                    }
                    let middle_cont: Container = (
                        signature::Base::String,
                        LocalDescBase::get_all_type(),
                        middle_map,
                    )
                        .try_into()
                        .unwrap();
                    outer_dict.insert(
                        Base::ObjectPath(desc.path.to_str().unwrap().to_string()),
                        middle_cont.into(),
                    );
                }
                let mut middle_map = HashMap::new();
                for interface in LocalCharBase::INTERFACES {
                    let props = characteristic.get_all_inner(interface.0).unwrap();
                    middle_map.insert(interface.0.to_string().into(), props);
                }
                let middle_cont: Container = (
                    signature::Base::String,
                    LocalCharBase::get_all_type(),
                    middle_map,
                )
                    .try_into()
                    .unwrap();
                outer_dict.insert(
                    Base::ObjectPath(characteristic.path.to_str().unwrap().to_string()),
                    middle_cont.into(),
                );
            }
            let mut middle_map = HashMap::new();

            for interface in LocalServiceBase::INTERFACES {
                let props = service.get_all_inner(interface.0).unwrap();
                middle_map.insert(interface.0.to_string().into(), props);
            }
            let middle_cont: Container = (
                signature::Base::String,
                LocalServiceBase::get_all_type(),
                middle_map,
            )
                .try_into()
                .unwrap();
            outer_dict.insert(
                Base::ObjectPath(service.path.to_str().unwrap().to_string()),
                middle_cont.into(),
            );
        }
        //let outer_param: Result<Param, std::convert::Infallible> = outer_dict.try_into();
        let outer_cont: Container = (
            signature::Base::ObjectPath,
            Self::object_manager_type(),
            outer_dict,
        )
            .try_into()
            .unwrap();
        reply.body.push_old_param(&outer_cont.into()).unwrap();
        reply
    }
}
trait Properties {
    const GET_ALL_ITEM: signature::Type = signature::Type::Container(signature::Container::Variant);
    fn get_all_type() -> signature::Type {
        signature::Type::Container(signature::Container::Dict(
            signature::Base::String,
            Box::new(Self::GET_ALL_ITEM),
        ))
    }
    const INTERFACES: &'static [(&'static str, &'static [&'static str])];
    fn properties_call(&mut self, msg: MarshalledMessage) -> MarshalledMessage {
        match msg.dynheader.member.as_ref().unwrap().as_ref() {
            "Get" => self.get(msg),
            "Set" => self.set(msg),
            GET_ALL_CALL => self.get_all(msg),
            _ => standard_messages::unknown_method(&msg.dynheader),
        }
    }
    fn get_all_inner<'a, 'b>(&mut self, interface: &str) -> Option<Param<'a, 'b>> {
        let props = Self::INTERFACES
            .iter()
            .find(|i| interface == i.0)
            .map(|i| i.1)?;
        let mut prop_map = HashMap::new();
        for prop in props {
            //eprintln!("{}: {}", interface, prop);
            let val = self.get_inner(interface, prop).unwrap();
            prop_map.insert(prop.to_string().into(), val);
        }
        let prop_cont = Container::Dict(params::Dict {
            key_sig: signature::Base::String,
            value_sig: Self::GET_ALL_ITEM,
            map: prop_map,
        });
        /*let prop_cont: Container = (signature::Base::String, Self::GET_ALL_ITEM, prop_map)
        .try_into()
        .unwrap();*/
        //Some(prop_cont.into())
        Some(Param::Container(prop_cont))
    }
    fn get_all(&mut self, msg: MarshalledMessage) -> MarshalledMessage {
        let msg = msg.unmarshall_all().unwrap();
        let interface = if let Some(Param::Base(Base::String(interface))) = msg.params.get(0) {
            // eprintln!("get_all() interface: {}", interface);
            interface
        } else {
            return msg
                .dynheader
                .make_error_response("Missing interface".to_string(), None);
        };
        if let Some(param) = self.get_all_inner(&interface) {
            let mut res = msg.make_response();
            res.body.push_old_param(&param).unwrap();
            res
        } else {
            let err_msg = format!(
                "Interface {} is not known on {}",
                interface,
                msg.dynheader.object.as_ref().unwrap()
            );
            msg.dynheader
                .make_error_response("InterfaceNotFound".to_string(), Some(err_msg))
        }
    }

    fn get(&mut self, msg: MarshalledMessage) -> MarshalledMessage {
        let msg = msg.unmarshall_all().unwrap();
        if msg.params.len() < 2 {
            let err_str = "Expected two string arguments".to_string();
            return msg
                .dynheader
                .make_error_response("Invalid arguments".to_string(), Some(err_str));
        }
        let interface = if let Param::Base(Base::String(interface)) = &msg.params[0] {
            interface
        } else {
            let err_str = "Expected string interface as first argument!".to_string();
            return msg
                .dynheader
                .make_error_response("Invalid arguments".to_string(), Some(err_str));
        };
        let prop = if let Param::Base(Base::String(prop)) = &msg.params[1] {
            prop
        } else {
            let err_str = "Expected string property as second argument!".to_string();
            return msg
                .dynheader
                .make_error_response("Invalid arguments".to_string(), Some(err_str));
        };
        if let Some(param) = self.get_inner(interface, prop) {
            let mut reply = msg.make_response();
            reply.body.push_old_param(&param).unwrap();
            reply
        } else {
            let s = format!("Property {} on interface {} not found.", prop, interface);
            msg.dynheader
                .make_error_response("PropertyNotFound".to_string(), Some(s))
        }
    }
    /// Should returng a variant containing if the property is found. If it is not found then it returns None.
    fn get_inner<'a, 'b>(&mut self, interface: &str, prop: &str) -> Option<Param<'a, 'b>>;
    fn set_inner(&mut self, interface: &str, prop: &str, val: Variant) -> Option<String>;
    fn set(&mut self, msg: MarshalledMessage) -> MarshalledMessage {
        let (interface, prop, var): (&str, &str, Variant) = match msg.body.parser().get3() {
            Ok(vals) => vals,
            Err(err) => {
                return msg.dynheader.make_error_response(
                    "InvalidParameters".to_string(),
                    Some(format!("{:?}", err)),
                )
            }
        };
        if let Some(err_str) = self.set_inner(interface, prop, var) {
            msg.dynheader.make_error_response(err_str, None)
        } else {
            msg.dynheader.make_response()
        }
    }
}

impl Properties for Bluetooth {
    const INTERFACES: &'static [(&'static str, &'static [&'static str])] = &[];
    fn get_inner<'a, 'b>(&mut self, _interface: &str, _prop: &str) -> Option<Param<'a, 'b>> {
        None
    }
    fn set_inner(&mut self, _interface: &str, _prop: &str, _val: Variant) -> Option<String> {
        unimplemented!()
    }
}

fn base_param_to_variant(b: Base) -> Param {
    let var = match b {
        Base::String(s) => params::Variant {
            sig: signature::Type::Base(signature::Base::String),
            value: Param::Base(s.into()),
        },
        Base::Boolean(b) => params::Variant {
            sig: signature::Type::Base(signature::Base::Boolean),
            value: Param::Base(b.into()),
        },
        Base::Uint16(u) => params::Variant {
            sig: signature::Type::Base(signature::Base::Uint16),
            value: Param::Base(u.into()),
        },
        Base::ObjectPath(p) => params::Variant {
            sig: signature::Type::Base(signature::Base::ObjectPath),
            value: Param::Base(Base::ObjectPath(p)),
        },
        Base::Byte(b) => params::Variant {
            sig: signature::Type::Base(signature::Base::Byte),
            value: Param::Base(b.into()),
        },
        Base::Uint64(b) => params::Variant {
            sig: rustbus::signature::Type::Base(signature::Base::Uint64),
            value: Param::Base(b.into()),
        },
        _ => unimplemented!(),
    };
    Param::Container(Container::Variant(Box::new(var)))
}

fn container_param_to_variant<'a, 'b>(c: Container<'a, 'b>) -> Param<'a, 'b> {
    let var = match c {
        Container::Dict(dict) => params::Variant {
            sig: signature::Type::Container(rustbus::signature::Container::Dict(
                dict.key_sig.clone(),
                Box::new(dict.value_sig.clone()),
            )),
            value: Param::Container(Container::Dict(dict)),
        },
        Container::Array(array) => params::Variant {
            sig: rustbus::signature::Type::Container(rustbus::signature::Container::Array(
                Box::new(array.element_sig.clone()),
            )),
            value: Param::Container(Container::Array(array)),
        },
        _ => unimplemented!(),
    };
    Param::Container(Container::Variant(Box::new(var)))
}

pub fn validate_uuid(uuid: &str) -> bool {
    if uuid.len() != 36 {
        return false;
    }
    let mut uuid_chars = uuid.chars();
    if uuid_chars.nth(8).unwrap() != '-' {
        return false;
    }
    for _ in 0..3 {
        if uuid_chars.nth(4).unwrap() != '-' {
            return false;
        }
    }
    let parse = |uuid: &str| -> Result<(), ParseIntError> {
        u128::from_str_radix(&uuid[..8], 16)?;
        u128::from_str_radix(&uuid[9..13], 16)?;
        u128::from_str_radix(&uuid[14..18], 16)?;
        u128::from_str_radix(&uuid[19..23], 16)?;
        u128::from_str_radix(&uuid[24..36], 16)?;
        Ok(())
    };
    if let Ok(_) = parse(uuid) {
        true
    } else {
        false
    }
}

pub enum ValOrFn {
    Value(CharValue),
    Function(Box<dyn FnMut() -> CharValue>),
}
impl Default for ValOrFn {
    fn default() -> Self {
        ValOrFn::Value(CharValue::default())
    }
}
impl AsRef<CharValue> for CharValue {
    fn as_ref(&self) -> &CharValue {
        self
    }
}
impl<T: AsRef<CharValue>> From<T> for ValOrFn {
    fn from(cv: T) -> Self {
        ValOrFn::Value(*cv.as_ref())
    }
}

impl Debug for ValOrFn {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        if let ValOrFn::Value(cv) = self {
            write!(f, "ValOrFn {{ Value: {:?} }}", cv)
        } else {
            write!(f, "ValOrFn {{ Fn  }}")
        }
    }
}

impl ValOrFn {
    #[inline]
    pub fn to_value(&mut self) -> CharValue {
        match self {
            ValOrFn::Value(cv) => (*cv),
            ValOrFn::Function(f) => f(),
        }
    }
    pub fn from_slice(slice: &[u8]) -> Self {
        ValOrFn::Value(slice.into())
    }
}
pub struct Variant<'buf> {
    sig: signature::Type,
    byteorder: ByteOrder,
    offset: usize,
    buf: &'buf [u8],
}
impl<'r, 'buf: 'r> Variant<'buf> {
    pub fn get_value_sig(&self) -> &signature::Type {
        &self.sig
    }
    pub fn get<T: Unmarshal<'r, 'buf>>(&self) -> Result<T, UnmarshalError> {
        if self.sig != T::signature() {
            return Err(UnmarshalError::WrongSignature);
        }
        T::unmarshal(self.byteorder, self.buf, self.offset).map(|r| r.1)
    }
}
impl Signature for Variant<'_> {
    fn signature() -> signature::Type {
        signature::Type::Container(signature::Container::Variant)
    }
    fn alignment() -> usize {
        Variant::signature().get_alignment()
    }
}
impl<'r, 'buf: 'r> Unmarshal<'r, 'buf> for Variant<'buf> {
    fn unmarshal(
        byteorder: ByteOrder,
        buf: &'buf [u8],
        offset: usize,
    ) -> unmarshal::UnmarshalResult<Self> {
        // let padding = rustbus::wire::util::align_offset(Self::get_alignment());
        let (mut used, desc) = rustbus::wire::util::unmarshal_signature(&buf[offset..])?;
        let mut sigs = match signature::Type::parse_description(desc) {
            Ok(sigs) => sigs,
            Err(_) => return Err(UnmarshalError::WrongSignature),
        };
        if sigs.len() != 1 {
            return Err(UnmarshalError::WrongSignature);
        }
        let sig = sigs.remove(0);
        used += rustbus::wire::util::align_offset(sig.get_alignment(), buf, offset + used)?;
        let start_loc = offset + used;
        used += rustbus::wire::validate_raw::validate_marshalled(byteorder, start_loc, buf, &sig)
            .map_err(|e| e.1)?;
        Ok((
            used,
            Variant {
                sig,
                buf: &buf[..offset + used],
                offset: start_loc,
                byteorder,
            },
        ))
    }
}
impl Marshal for Variant<'_> {
    fn marshal(&self, byteorder: ByteOrder, buf: &mut Vec<u8>) -> Result<(), rustbus::Error> {
        if let ByteOrder::LittleEndian = byteorder {
            if let ByteOrder::BigEndian = self.byteorder {
                panic!("Byte order mismatch");
            }
        } else {
            if let ByteOrder::LittleEndian = self.byteorder {
                panic!("Byte order mismatch");
            }
        }
        let mut sig_str = String::new();
        self.sig.to_str(&mut sig_str);
        debug_assert!(sig_str.len() <= 255);
        buf.push(sig_str.len() as u8);
        buf.extend_from_slice(sig_str.as_bytes());
        rustbus::wire::util::pad_to_align(self.sig.get_alignment(), buf);
        buf.extend_from_slice(&self.buf[self.offset..]);
        Ok(())
    }
}
