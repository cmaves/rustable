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
use rustbus::wire::marshal::traits::Signature;
use rustbus::wire::unmarshal;
use rustbus::wire::unmarshal::traits::Unmarshal;
use rustbus::{get_system_bus_path, ByteOrder};
use std::collections::{HashMap, HashSet, VecDeque};
use std::convert::{TryFrom, TryInto};
use std::ffi::OsString;
use std::fmt::Write;
use std::fmt::{Debug, Display, Formatter};
use std::num::ParseIntError;
use std::path::{Component, Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

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
    fn to_mac(&self) -> MAC;
}
impl ToMAC for &str {
    fn to_mac(&self) -> MAC {
        assert!(validate_mac(self));
        let ret: MAC = self.to_string().into();
        ret
    }
}

enum DbusObject<'a> {
    Char(&'a mut LocalCharBase),
    Serv(&'a mut LocalServiceBase),
    Desc(&'a mut LocalDescriptor),
    Ad(&'a mut Advertisement),
    Appl,
}

#[derive(Debug)]
pub enum Error {
    DbusClient(client_conn::Error),
    DbusReqErr(String),
    Bluez(String),
    BadInput(String),
    Unix(nix::Error),
}
impl From<nix::Error> for Error {
    fn from(err: nix::Error) -> Self {
        Error::Unix(err)
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
    pub filter_dest: Option<String>,
    ads: VecDeque<Advertisement>,
    service_index: u8,
    ad_index: u16,
    devices: HashMap<MAC, RemoteDeviceBase>,
    comp_map: HashMap<OsString, MAC>,
}
impl Bluetooth {
    /// Creates a new `Bluetooth` and setup a DBus client to interact with Bluez.
    pub fn new(dbus_name: &str, blue_path: String) -> Result<Self, Error> {
        let session_path = get_system_bus_path()?;
        let conn = Conn::connect_to_bus(session_path, true)?;
        let mut rpc_con = RpcConn::new(conn);
        let mut name = "io.rustable.".to_string();
        name.push_str(dbus_name);
        rpc_con.send_message(&mut standard_messages::hello(), Timeout::Infinite)?;
        let namereq = rpc_con.send_message(
            &mut standard_messages::request_name(name.clone(), 0),
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
        path.push_str(&name.replace(".", "/"));
        let path = PathBuf::from(path);
        let blue_path: &Path = blue_path.as_ref();
        let mut ret = Bluetooth {
            rpc_con,
            name,
            verbose: 0,
            services,
            registered: false,
            blue_path: blue_path.into(),
            path,
            filter_dest: Some(BLUEZ_DEST.to_string()),
            ads: VecDeque::new(),
            service_index: 0,
            ad_index: 0,
            devices: HashMap::new(),
            comp_map: HashMap::new(),
        };
        ret.rpc_con.set_filter(Box::new(move |msg| match msg.typ {
            MessageType::Call => true,
            MessageType::Error => true,
            MessageType::Reply => true,
            MessageType::Invalid => false,
            MessageType::Signal => false,
        }));
        Ok(ret)
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
    pub fn devices(&self) -> HashSet<MAC> {
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
        let ret_idx = adv.index;
        self.ad_index += 1;
        let adv_path = self.path.join(format!("adv{:04x}", adv.index));
        adv.path = adv_path;
        self.ads.push_back(adv);
		let idx = self.ads.len() - 1;
        self.register_adv(idx).map(|_| idx as u16).map_err(|err| (idx as u16, err))
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
        if (self.ads[idx].active) {
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
    /// Removes the advertisement from the `Bluetooth` instance but does not unregister the
    /// advertisement with Bluez. It is recommended that this is not used.
    pub fn remove_adv_no_dbus(&mut self, index: u16) -> Option<Advertisement> {
        let idx = self.ads.iter().position(|ad| ad.index == index)?;
        self.ads.remove(idx)
    }
    /// Set whether the Bluez controller should be discoverable (`true`) or not.
	///
	/// **Calls process_requests()**
    pub fn set_discoverable(&mut self, on: bool) -> Result<(), Error> {
        let mut msg = MessageBuilder::new()
            .call("Set".to_string())
            .on(self.blue_path.to_str().unwrap().to_string())
            .with_interface(PROP_IF_STR.to_string())
            .at(BLUEZ_DEST.to_string())
            .build();
        msg.body
            .push_param2(ADAPTER_IF_STR, "Discoverable")
            .unwrap();
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
                            "Set discoverable call failed: {:?}",
                            res
                        )))
                    }
                    _ => unreachable!(),
                }
            }
        }
    }
    /// Set the Bluez controller power on (`true`) or off.
	///
	/// **Calls process_requests()**
    pub fn set_power(&mut self, on: bool) -> Result<(), Error> {
        let mut msg = MessageBuilder::new()
            .call("Set".to_string())
            .on(self.blue_path.to_str().unwrap().to_string())
            .with_interface(PROP_IF_STR.to_string())
            .at(BLUEZ_DEST.to_string())
            .build();
        msg.body.push_param2(ADAPTER_IF_STR, "Powered").unwrap();
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

        while let Some(call) = self.rpc_con.try_get_call() {
            // eprintln!("received call {:?}", call);
            let interface = (&call.dynheader.interface).as_ref().unwrap();
            if let Some(dest) = &self.filter_dest {
                if dest != call.dynheader.destination.as_ref().unwrap() {
                    let mut msg = call.dynheader.make_error_response(
                        BLUEZ_NOT_PERM.to_string(),
                        Some("Sender is not allowed to perform this action.".to_string()),
                    );
                    self.rpc_con.send_message(&mut msg, Timeout::Infinite)?;
                    continue;
                }
            }
            let mut reply = match self.match_root(&call.dynheader) {
                Some(v) => match v {
                    DbusObject::Appl => match interface.as_ref() {
                        PROP_IF_STR => self.properties_call(call),
                        OBJ_MANAGER_IF_STR => self.objectmanager_call(call),
                        INTRO_IF_STR => self.introspectable(call),
                        _ => standard_messages::unknown_method(&call.dynheader),
                    },
                    DbusObject::Serv(v) => match interface.as_ref() {
                        PROP_IF_STR => v.properties_call(call),
                        SERV_IF_STR => v.service_call(call),
                        INTRO_IF_STR => v.introspectable(call),
                        _ => standard_messages::unknown_method(&call.dynheader),
                    },
                    DbusObject::Char(v) => {
                        match interface.as_ref() {
                            PROP_IF_STR => v.properties_call(call),
                            CHAR_IF_STR => {
                                let serv_uuid = v.serv_uuid.clone();
                                let char_uuid = v.uuid.clone();
                                let mut serv = LocalService {
                                    bt: self,
                                    uuid: serv_uuid,
                                };
                                let mut local_char = {
                                    // TODO: implement unsafe-opt changes
                                    LocalCharactersitic::new(&mut serv, char_uuid)
                                };
                                local_char.char_call(call)
                            }
                            INTRO_IF_STR => v.introspectable(call),
                            _ => standard_messages::unknown_method(&call.dynheader),
                        }
                    }
                    DbusObject::Desc(v) => match interface.as_ref() {
                        PROP_IF_STR => v.properties_call(call),
                        DESC_IF_STR => unimplemented!(),
                        INTRO_IF_STR => v.introspectable(call),
                        _ => standard_messages::unknown_method(&call.dynheader),
                    },
                    DbusObject::Ad(ad) => match interface.as_ref() {
                        PROP_IF_STR => ad.properties_call(call),
                        LEAD_IF_STR => {
                            if call.dynheader.member.as_ref().unwrap() == "Release" {
                                let index = ad.index;
                                let adv = self.ads.iter_mut().find(|x| x.index == index).unwrap();
                                adv.active = false;
                                call.dynheader.make_response()
                            } else {
                                standard_messages::unknown_method(&call.dynheader)
                            }
                        }
                        INTRO_IF_STR => ad.introspectable(call),
                        _ => standard_messages::unknown_method(&call.dynheader),
                    },
                },
                None => standard_messages::unknown_method(&call.dynheader),
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
                OBJ_MANAGER_IF_STR => match sig.dynheader.member.as_ref().unwrap().as_str() {
                    IF_ADDED_SIG => self.interface_added(sig)?,
                    IF_REMOVED_SIG => unimplemented!(),
                    _ => (),
                },
                DBUS_IF_STR => match sig.dynheader.member.as_ref().unwrap().as_str() {
                    NAME_LOST_SIG => {
                        if let Some(filter) = &self.filter_dest {
                            if let Ok(lost_name) = sig.body.parser().get() {
                                let lost_name: &str = lost_name;
                                if filter == lost_name {
                                    let err = Err(Error::Bluez(format!(
                                        "{} has disconnected from DBus!",
                                        filter
                                    )));
                                    self.clear_devices();
                                    return err;
                                }
                            }
                        }
                    }
                    _ => (),
                },
                _ => (),
            }
        }
        Ok(())
    }
    fn interface_added(&mut self, sig: MarshalledMessage) -> Result<(), Error> {
        let sig = sig.unmarshall_all().unwrap();
        let path: &Path = if let Some(Param::Base(Base::ObjectPath(path))) = sig.params.get(0) {
            path.as_ref()
        } else {
            return Ok(());
        };

        let path = if let Ok(path) = path.strip_prefix(&self.blue_path) {
            path
        } else {
            return Ok(());
        };
        let mut comps = path.components();
        let device = comps.next();
        let service = comps.next();
        let _character = comps.next();
        let _desc = comps.next();
        let _end = comps.next();
        if let Some(Component::Normal(_device)) = device {
            if let Some(Component::Normal(_service)) = service {
                unimplemented!()
            } else {
                unimplemented!()
            }
        }
        Ok(())
    }
    fn match_root(&mut self, dynheader: &DynamicHeader) -> Option<DbusObject> {
        let path = self.get_path();
        if let None = &dynheader.interface {
            return None;
        }
        if let None = &dynheader.member {
            return None;
        }
        // eprintln!("For path: {:?}, Checking msg for match", path);
        let object = &dynheader.object.as_ref()?;
        let obj_path: &Path = object.as_ref();

        if path.starts_with(obj_path) {
            Some(DbusObject::Appl)
        } else {
            if let Ok(service_path) = obj_path.strip_prefix(path) {
                let self0: &mut Self = unsafe {
                    /* As of Rust 1.43, the compiler (as far as I can tell) is not smart enough
                      to know that if-let statement below is false then the mutable borrow from
                      match_service is over.
                    */
                    let ptr: *mut Self = self;
                    if let Some(object) = self.match_service(service_path, dynheader) {
                        return Some(object);
                    }
                    ptr.as_mut().unwrap()
                };
                self0.match_advertisement(service_path, dynheader)
            //unimplemented!()
            } else {
                None
            }
        }
    }
    fn match_advertisement(&mut self, msg_path: &Path, _msg: &DynamicHeader) -> Option<DbusObject> {
        // eprintln!("Checking for advertisement for match");
        let mut components = msg_path.components();
        let comp = components.next()?.as_os_str().to_str().unwrap();
        if let Some(_) = components.next() {
            return None;
        }
        if comp.len() != 7 {
            return None;
        }
        if &comp[0..3] != "adv" {
            return None;
        }
        if let Ok(u) = u16::from_str_radix(&comp[3..7], 16) {
            if let Some(ad) = self.ads.iter_mut().find(|x| x.index == u) {
                Some(DbusObject::Ad(ad))
            } else {
                None
            }
        } else {
            None
        }
    }
    fn match_service(&mut self, msg_path: &Path, msg: &DynamicHeader) -> Option<DbusObject> {
        // eprintln!("Checking for service for match");
        let mut components = msg_path.components().take(2);
        if let Component::Normal(path) = components.next().unwrap() {
            let path = path.to_str()?;
            if !path.starts_with("service") {
                return None;
            }
            let mut service_str = String::new();
            for service in self.services.values_mut() {
                service_str.clear();
                write!(&mut service_str, "service{:02X}/", service.index).unwrap();
                if let Ok(path) = msg_path.strip_prefix(&service_str) {
                    if let Some(_) = path.components().next() {
                        return service.match_chars(path, msg);
                    } else {
                        return Some(DbusObject::Serv(service));
                    }
                }
            }
            None
        } else {
            None
        }
    }
    pub fn clear_devices(&mut self) {
        self.devices.clear()
    }
    /// Used to get devices devices known to Bluez. This function does *not* trigger scan/discovery
    /// on the Bluez controller. Use `set_scan()` to initiate actual device discovery.
	///
	/// **Calls process_requests()**
    pub fn discover_devices(&mut self) -> Result<HashSet<MAC>, Error> {
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
    fn get_managed_objects<'a, 'b>(
        &mut self,
        path: String,
        filter_path: &Path,
    ) -> Result<
        Vec<(
            PathBuf,
            HashMap<String, HashMap<String, params::Variant<'a, 'b>>>,
        )>,
        Error,
    > {
        let mut msg = MessageBuilder::new()
            .call(MANGAGED_OBJ_CALL.to_string())
            .on(path)
            .at(BLUEZ_DEST.to_string())
            .with_interface(OBJ_MANAGER_IF_STR.to_string())
            .build();
        let res_idx = self.rpc_con.send_message(&mut msg, Timeout::Infinite)?;
        loop {
            self.process_requests()?;
            if let Some(res) = self.rpc_con.try_get_response(res_idx) {
                let mut res = match res.typ {
                    MessageType::Reply => res.unmarshall_all().unwrap(),
                    MessageType::Error => {
                        return Err(Error::DbusReqErr(format!(
                            "GetManagedObjects call returned error: {:?}",
                            res
                        )))
                    }
                    _ => unreachable!(),
                };
                if res.params.len() < 1 {
                    return Err(Error::Bluez(
                        "GetManagedObjects called didn't return any parameters".to_string(),
                    ));
                }
                res.params.truncate(1);
                if let Param::Container(Container::Dict(path_dict)) = res.params.remove(0) {
                    let path_map = path_dict.map;
                    let mut pairs: Vec<(
                        PathBuf,
                        HashMap<String, HashMap<String, params::Variant>>,
                    )> = path_map
                        .into_iter()
                        .filter_map(|pair| {
                            if let Base::ObjectPath(path) = pair.0 {
                                let path: PathBuf = path.into();
                                if path.starts_with(filter_path) {
                                    let if_map = if_dict_to_map(pair.1);
                                    return Some((path, if_map));
                                }
                            }
                            None
                        })
                        .collect();
                    pairs.sort_by(|pair1, pair2| pair1.0.cmp(&pair2.0));
                    return Ok(pairs);
                } else {
                    return Err(Error::DbusReqErr(
                        "GetManagedObjects called didn't return unexpected parameters".to_string(),
                    ));
                }
            }
        }
    }
    fn discover_devices_filter<T: AsRef<Path>>(
        &mut self,
        filter_path: T,
    ) -> Result<HashSet<MAC>, Error> {
        self.devices.clear();
        let pairs = self.get_managed_objects("/".to_string(), filter_path.as_ref())?;
        let mut set = HashSet::new();
        let mut device_base: Option<RemoteDeviceBase> = None;
        let mut service_base: Option<RemoteServiceBase> = None;
        let mut characteristic_base: Option<RemoteCharBase> = None;
        for (path, mut if_map) in pairs {
            if let Some(mut props) = if_map.get_mut(DEV_IF_STR) {
                let mut dev_comps = path.strip_prefix(&self.blue_path).unwrap().components();
                /*if let None = match dev_comps.next() {
                    Some(comp) => comp,
                    None => return Err(Error::Bluez("Bluez returned invalid device".to_string()))
                };*/
                if let None = dev_comps.next() {
                    return Err(Error::Bluez("Bluez returned invalid device".to_string()));
                }
                let device = RemoteDeviceBase::from_props(&mut props, path)?;
                set.insert(device.mac.clone());
                if let Some(base) = device_base {
                    self.insert_device(base)
                }
                device_base = Some(device);
            // self.insert_device(device);
            } else if let Some(props) = if_map.get_mut(SERV_IF_STR) {
                match &mut device_base {
                    Some(dev_base) => {
                        let service = RemoteServiceBase::from_props(props, path)?;
                        // base.services.insert(service.uuid.clone(), service);
                        if let Some(serv_base) = service_base {
                            dev_base.services.insert(serv_base.uuid.clone(), serv_base);
                        }
                        service_base = Some(service);
                    }
                    None => {
                        return Err(Error::DbusReqErr(format!(
                            "Received service {:?} without device",
                            path
                        )))
                    }
                }
            } else if let Some(props) = if_map.get_mut(CHAR_IF_STR) {
                match &mut service_base {
                    Some(serv_base) => {
                        let character = RemoteCharBase::from_props(props, path)?;
                        if let Some(char_base) = characteristic_base {
                            serv_base.chars.insert(char_base.uuid.clone(), char_base);
                        }
                        characteristic_base = Some(character);
                    }
                    None => {
                        return Err(Error::DbusReqErr(format!(
                            "Received characteristic {:?} without service",
                            path
                        )))
                    }
                }
            } else if let Some(_props) = if_map.get_mut(DESC_IF_STR) {
                // TODO: implement for descriptor
                if self.verbose >= 2 {
                    eprintln!("Descriptor skipped because it is not implemented.")
                }
            }
        }
        // handle final device
        if let Some(mut dev_base) = device_base {
            if let Some(mut serv_base) = service_base {
                if let Some(char_base) = characteristic_base {
                    // TODO add descriptor
                    serv_base.chars.insert(char_base.uuid.clone(), char_base);
                }
                dev_base.services.insert(serv_base.uuid.clone(), serv_base);
            }
            self.insert_device(dev_base);
        }

        Ok(set)
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
    let mut ret = String::with_capacity(18);
    for i in 0..5 {
        let tar = i * 3;
        ret.push_str(&devmac[tar..tar + 2]);
        ret.push(':');
    }
    ret.push_str(&devmac[15..17]);
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
                    for interface in LocalDescriptor::INTERFACES {
                        let props = desc.get_all_inner(interface.0).unwrap();
                        middle_map.insert(interface.0.to_string().into(), props);
                    }
                    let middle_cont: Container = (
                        signature::Base::String,
                        LocalDescriptor::get_all_type(),
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
    Value([u8; 512], usize),
    Function(Box<dyn FnMut() -> ([u8; 512], usize)>),
}

impl Debug for ValOrFn {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        if let ValOrFn::Value(v, l) = self {
            write!(f, "ValOrFn {{ Value: {:?} }}", &v[..*l])
        } else {
            write!(f, "ValOrFn {{ Fn  }}")
        }
    }
}

impl ValOrFn {
    #[inline]
    pub fn to_value(&mut self) -> ([u8; 512], usize) {
        match self {
            ValOrFn::Value(v, l) => (*v, *l),
            ValOrFn::Function(f) => f(),
        }
    }
    pub fn from_slice(slice: &[u8]) -> Self {
        let mut v = [0; 512];
        v[..slice.len()].copy_from_slice(slice);
        ValOrFn::Value(v, slice.len())
    }
}

pub enum Variant<'a> {
    Double(u64),
    Byte(u8),
    Int16(i16),
    Uint16(u16),
    Int32(i32),
    Uint32(u32),
    UnixFd(u32),
    Int64(i64),
    Uint64(u64),
    String(String),
    StringRef(&'a str),
    Signature(String),
    SigRef(&'a str),
    ObjectPath(PathBuf),
    ObjectPathRef(&'a Path),
    Boolean(bool),
    Variant(usize, Vec<u8>),
    VariantRef(usize, &'a [u8]),
    /*
    Dict(HashMap<K, K>),
    DictRef(&'a HashMap<K, K>),
    Array(Vec<T>),
    ArrayRef(&'a [T])
    */
}
impl ToOwned for Variant<'_> {
    type Owned = Self;
    fn to_owned(&self) -> Self::Owned {
        let test = [0_u8, 1, 2, 3, 4, 5];
        let owned: Vec<u8> = test[1..].to_owned();
        match self {
            Variant::Double(v) => Variant::Double(*v),
            Variant::Byte(v) => Variant::Byte(*v),
            Variant::Int16(v) => Variant::Int16(*v),
            Variant::Uint16(v) => Variant::Uint16(*v),
            Variant::Int32(v) => Variant::Int32(*v),
            Variant::Uint32(v) => Variant::Uint32(*v),
            Variant::UnixFd(v) => Variant::UnixFd(*v),
            Variant::Int64(v) => Variant::Int64(*v),
            Variant::Uint64(v) => Variant::Uint64(*v),
            Variant::String(v) => Variant::String(v.to_string()),
            Variant::StringRef(v) => Variant::String(v.to_string()),
            Variant::Signature(v) => Variant::Signature(v.to_string()),
            Variant::SigRef(v) => Variant::Signature(v.to_string()),
            Variant::ObjectPath(v) => Variant::ObjectPath(v.to_path_buf()),
            Variant::ObjectPathRef(v) => Variant::ObjectPath(v.to_path_buf()),
            Variant::Boolean(v) => Variant::Boolean(*v),
            Variant::Variant(u, v) => Variant::Variant(*u, v.to_owned()),
            Variant::VariantRef(u, v) => Variant::Variant(*u, (*v).to_owned()),
            _ => unimplemented!(),
        }
    }
}
impl Signature for Variant<'_> {
    fn signature() -> signature::Type {
        signature::Type::Container(signature::Container::Variant)
    }
    fn alignment() -> usize {
        Self::signature().get_alignment()
    }
}

impl<'r, 'buf: 'r> Unmarshal<'r, 'buf> for Variant<'buf> {
    fn unmarshal(
        byteorder: ByteOrder,
        buf: &'buf [u8],
        offset: usize,
    ) -> unmarshal::UnmarshalResult<Self> {
        let sig_len = buf[offset];
        let child_offset = offset + 1 + sig_len as usize;
        let sig_str = std::str::from_utf8(&buf[offset + 1..child_offset])
            .map_err(|_| unmarshal::Error::InvalidType)?;
        let types = signature::Type::parse_description(sig_str)
            .map_err(|_| unmarshal::Error::NoSignature)?;
        if types.len() != 1 {
            return Err(unmarshal::Error::NoSignature);
        }
        match &types[0] {
            signature::Type::Base(base) => match base {
                signature::Base::Byte => {
                    let (offset, value) = Unmarshal::unmarshal(byteorder, buf, child_offset)?;
                    Ok((offset, Variant::Byte(value)))
                }
                signature::Base::Int16 => {
                    let (offset, value) = Unmarshal::unmarshal(byteorder, buf, child_offset)?;
                    Ok((offset, Variant::Int16(value)))
                }
                signature::Base::Uint16 => {
                    let (offset, value) = Unmarshal::unmarshal(byteorder, buf, child_offset)?;
                    Ok((offset, Variant::Uint16(value)))
                }
                signature::Base::Int32 => {
                    let (offset, value) = Unmarshal::unmarshal(byteorder, buf, child_offset)?;
                    Ok((offset, Variant::Int32(value)))
                }
                signature::Base::Uint32 => {
                    let (offset, value) = Unmarshal::unmarshal(byteorder, buf, child_offset)?;
                    Ok((offset, Variant::Uint32(value)))
                }
                signature::Base::UnixFd => {
                    let (offset, value) = Unmarshal::unmarshal(byteorder, buf, child_offset)?;
                    Ok((offset, Variant::UnixFd(value)))
                }
                signature::Base::Int64 => {
                    let (offset, value) = Unmarshal::unmarshal(byteorder, buf, child_offset)?;
                    Ok((offset, Variant::Int64(value)))
                }
                signature::Base::Uint64 => {
                    let (offset, value) = Unmarshal::unmarshal(byteorder, buf, child_offset)?;
                    Ok((offset, Variant::Uint64(value)))
                }
                signature::Base::Double => {
                    let (offset, value) = Unmarshal::unmarshal(byteorder, buf, child_offset)?;
                    Ok((offset, Variant::Double(value)))
                }
                signature::Base::String => {
                    let (offset, value) = Unmarshal::unmarshal(byteorder, buf, child_offset)?;
                    Ok((offset, Variant::StringRef(value)))
                }
                signature::Base::Signature => {
                    let (offset, value) = Unmarshal::unmarshal(byteorder, buf, child_offset)?;
                    match params::validate_signature(value) {
                        Ok(_) => Ok((offset, Variant::SigRef(value))),
                        Err(_) => Err(unmarshal::Error::NoSignature),
                    }
                }
                signature::Base::ObjectPath => {
                    let (offset, value) = Unmarshal::unmarshal(byteorder, buf, child_offset)?;
                    match params::validate_object_path(value) {
                        Ok(_) => Ok((offset, Variant::ObjectPathRef(value.as_ref()))),
                        Err(_) => Err(unmarshal::Error::NoSignature),
                    }
                }
                signature::Base::Boolean => {
                    let (offset, value) = Unmarshal::unmarshal(byteorder, buf, child_offset)?;
                    Ok((offset, Variant::Boolean(value)))
                }
            },
            signature::Type::Container(on) => match on {
                signature::Container::Array(_) => unimplemented!(),
                signature::Container::Struct(_) => unimplemented!(),
                signature::Container::Dict(_, _) => unimplemented!(),
                signature::Container::Variant => unimplemented!(),
            },
        }
    }
}
