use gatt::*;
use rustbus::client_conn;
use rustbus::client_conn::{Conn, RpcConn, Timeout};
use rustbus::get_session_bus_path;
use rustbus::message::{Message, MessageType};
use rustbus::message_builder::{MessageBuilder, OutMessage, OutMessageBody};
use rustbus::params;
use rustbus::signature;
use rustbus::standard_messages;
use rustbus::{get_system_bus_path, Base, Container, Param};
use std::cell::{RefCell, RefMut};
use std::collections::hash_map::Keys;
use std::collections::{HashMap, HashSet, VecDeque};
use std::convert::{TryFrom, TryInto};
use std::ffi::OsString;
use std::fmt::Write;
use std::fmt::{Debug, Display, Formatter};
use std::io::ErrorKind;
use std::num::ParseIntError;
use std::os::unix::io::AsRawFd;
use std::os::unix::net::UnixDatagram;
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
    Ad(&'a Advertisement),
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

impl TryFrom<&'_ Message<'_, '_>> for Error {
    type Error = &'static str;
    fn try_from(msg: &Message) -> Result<Self, Self::Error> {
        match msg.typ {
            MessageType::Error => (),
            _ => return Err("Message was not an error"),
        }
        let err_name = match &msg.error_name {
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

pub struct Bluetooth<'a, 'b> {
    rpc_con: RpcConn<'a, 'b>,
    blue_path: Rc<Path>,
    name: String,
    path: PathBuf,
    pub verbose: u8,
    services: HashMap<UUID, LocalServiceBase>,
    registered: bool,
    pub filter_dest: Option<String>,
    pub ads: VecDeque<Advertisement>,
    service_index: u8,
    devices: HashMap<MAC, RemoteDeviceBase>,
    comp_map: HashMap<OsString, MAC>,
}

impl<'a, 'b> Bluetooth<'a, 'b> {
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
        if let Some(_) = &res.error_name {
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
    pub fn get_path(&self) -> &Path {
        &self.path
    }
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
    pub fn get_service<T: ToUUID>(&mut self, uuid: T) -> Option<LocalService<'_, 'a, 'b>> {
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
    pub fn get_device<'c>(&'c mut self, mac: &MAC) -> Option<RemoteDevice<'c, 'a, 'b>> {
        let base = self.devices.get_mut(mac)?;
        Some(RemoteDevice {
            blue: self,
            mac: mac.clone(),
            #[cfg(feature = "unsafe-opt")]
            ptr: base,
        })
    }
    pub fn devices(&self) -> HashSet<MAC> {
        self.devices.keys().map(|x| x.clone()).collect()
    }
    pub fn start_advertise(&mut self, adv: Advertisement) -> Result<(), Error> {
        unimplemented!()
    }
    pub fn remove_service(&mut self, uuid: &str) -> Result<LocalService, Error> {
        unimplemented!()
    }
    fn register_advertisement(&mut self, advert: Advertisement) -> Result<u16, Error> {
        unimplemented!()
    }
    fn unregister_advertisement(&mut self, advert: Advertisement) -> Result<u16, Error> {
        unimplemented!()
    }
    pub fn register_application(&mut self) -> Result<(), Error> {
        let path = self.get_path();
        let empty_dict = HashMap::new();
        let dict = params::Dict {
            key_sig: signature::Base::String,
            value_sig: signature::Type::Container(signature::Container::Variant),
            map: empty_dict,
        };
        let mut call_builder = MessageBuilder::new().call(REGISTER_CALL.to_string());
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

        let mut body = OutMessageBody::new();
        body.push_old_params(&[
            Param::Base(Base::ObjectPath(
                path.as_os_str().to_str().unwrap().to_string(),
            )),
            Param::Container(Container::Dict(dict)),
        ])
        .unwrap();
        msg.body = body;

        eprintln!("registration msg: {:#?}", msg);
        let msg_idx = self.rpc_con.send_message(&mut msg, Timeout::Infinite)?;
        // we expect there to be no response
        loop {
            self.process_requests()?;
            if let Some(res) = self.rpc_con.try_get_response(msg_idx) {
                return if let MessageType::Error = res.typ {
                    let mut err = None;
                    if let Some(err_str) = res.params.get(0) {
                        if let Param::Base(Base::String(err_str)) = err_str {
                            err = Some(err_str);
                        }
                    }
                    let err_str = if let Some(err) = err {
                        format!(
                            "Failed to register application with bluez: {}: {:?}",
                            res.error_name.unwrap(),
                            err
                        )
                    } else {
                        format!(
                            "Failed to register application with bluez: {}",
                            res.error_name.unwrap()
                        )
                    };
                    eprintln!("error: {}", err_str);
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
    pub fn unregister_application(&mut self) -> Result<(), Error> {
        unimplemented!();
        self.registered = false;
        Ok(())
    }
    pub fn process_requests(&mut self) -> Result<(), Error> {
        let half_milli = Duration::from_micros(500);
        loop {
            let mut done = false;
            match self.rpc_con.wait_call(Timeout::Duration(half_milli)) {
                Ok(call) => {
                    eprintln!("received call {:#?}", call);
                    let interface = (&call.interface).as_ref().unwrap();
                    if let Some(dest) = &self.filter_dest {
                        if dest != call.destination.as_ref().unwrap() {
                            let msg = call.make_error_response(
                                BLUEZ_NOT_PERM.to_string(),
                                Some("Sender is not allowed to perform this action.".to_string()),
                            );
                            continue;
                        }
                    }
                    let mut reply = match self.match_root(&call) {
                        Some(v) => match v {
                            DbusObject::Appl => match interface.as_ref() {
                                "org.freedesktop.DBus.Properties" => self.properties_call(&call),
                                "org.freedesktop.DBus.ObjectManager" => {
                                    self.objectmanager_call(&call)
                                }
                                "org.freedesktop.DBus.Introspectable" => self.introspectable(&call),
                                _ => unimplemented!(), // TODO: Added interface not found
                            },
                            DbusObject::Serv(v) => match interface.as_ref() {
                                "org.freedesktop.DBus.Properties" => v.properties_call(&call),
                                "org.bluez.GattService1" => v.service_call(&call),
                                "org.freedesktop.DBus.Introspectable" => v.introspectable(&call),
                                _ => unimplemented!(), // TODO: Added interface not found
                            },
                            DbusObject::Char(v) => match interface.as_ref() {
                                "org.freedesktop.DBus.Properties" => v.properties_call(&call),
                                "org.bluez.GattCharacteristic1" => v.char_call(&call),
                                "org.freedesktop.DBus.Introspectable" => v.introspectable(&call),
                                _ => unimplemented!(), // TODO: Added interface not found
                            },
                            DbusObject::Desc(v) => match interface.as_ref() {
                                "org.freedesktop.DBus.Properties" => v.properties_call(&call),
                                "org.bluez.GattDescriptor1" => unimplemented!(),
                                "org.freedesktop.DBus.Introspectable" => v.introspectable(&call),
                                _ => unimplemented!(), // TODO: Added interface not found
                            },
                            DbusObject::Ad(ad) => unimplemented!(),
                        },
                        None => standard_messages::unknown_method(&call),
                    };
                    eprintln!("replying: {:#?}", reply);
                    self.rpc_con.send_message(&mut reply, Timeout::Infinite)?;
                }
                Err(e) => match e {
                    client_conn::Error::TimedOut => done = true,
                    _ => return Err(e.into()),
                },
            }
            if let Some(sig) = self.rpc_con.try_get_signal() {
                match sig.interface.as_ref().unwrap().as_str() {
                    OBJ_MANAGER_IF_STR => match sig.member.as_ref().unwrap().as_str() {
                        IF_ADDED_SIG => self.interface_added(sig)?,
                        IF_REMOVED_SIG => unimplemented!(),
                        _ => (),
                    },
                    _ => (),
                }
            } else {
                if done {
                    return Ok(());
                }
            }
        }
    }
    fn interface_added(&mut self, sig: Message) -> Result<(), Error> {
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
        let character = comps.next();
        let desc = comps.next();
        let end = comps.next();
        if let Some(Component::Normal(device)) = device {
            if let Some(Component::Normal(service)) = service {
                unimplemented!()
            } else {
                unimplemented!()
            }
        }
        Ok(())
    }
    fn match_root(&mut self, msg: &Message) -> Option<DbusObject> {
        let path = self.get_path();
        if let None = &msg.interface {
            return None;
        }
        if let None = &msg.member {
            return None;
        }
        eprintln!("For path: {:?}, Checking msg for match", path);
        let object = &msg.object.as_ref()?;
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
                    if let Some(object) = self.match_service(service_path, msg) {
                        return Some(object);
                    }
                    ptr.as_mut().unwrap()
                };
                self0.match_advertisement(service_path, msg)
            //unimplemented!()
            } else {
                None
            }
        }
    }
    fn match_advertisement(&self, msg_path: &Path, msg: &Message) -> Option<DbusObject> {
        eprintln!("Checking for advertisement for match");
        let mut components = msg_path.components();
        let comp = components.next()?.as_os_str().to_str().unwrap();
        if let None = components.next() {
            return None;
        }
        if comp.len() != 10 {
            return None;
        }
        if &comp[0..6] != "advert" {
            return None;
        }
        if let Ok(u) = u16::from_str_radix(&comp[6..10], 16) {
            if let Some(ad) = self.ads.iter().find(|x| x.index == u) {
                Some(DbusObject::Ad(ad))
            } else {
                None
            }
        } else {
            None
        }
    }
    fn match_service(&mut self, msg_path: &Path, msg: &Message) -> Option<DbusObject> {
        eprintln!("Checking for service for match");
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
    pub fn discover_devices(&mut self) -> Result<HashSet<MAC>, Error> {
        self.discover_devices_filter(self.blue_path.clone())
    }
    fn get_managed_objects(
        &mut self,
        dest: String,
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
            if let Some(mut res) = self.rpc_con.try_get_response(res_idx) {
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
                }
            }
        }
    }
    fn discover_devices_filter<T: AsRef<Path>>(
        &mut self,
        filter_path: T,
    ) -> Result<HashSet<MAC>, Error> {
        let pairs = self.get_managed_objects(
            BLUEZ_DEST.to_string(),
            "/".to_string(),
            filter_path.as_ref(),
        )?;
        let mut set = HashSet::new();
        for (path, if_map) in pairs {
            if let Some(props) = if_map.get(DEV_IF_STR) {
                let mut dev_comps = path.strip_prefix(&self.blue_path).unwrap().components();
                /*if let None = match dev_comps.next() {
                    Some(comp) => comp,
                    None => return Err(Error::Bluez("Bluez returned invalid device".to_string()))
                };*/
                if let None = dev_comps.next() {
                    return Err(Error::Bluez("Bluez returned invalid device".to_string()));
                }
                let device = RemoteDeviceBase::from_props(props, path)?;
                set.insert(device.mac.clone());
                self.insert_device(device);
            } else if let Some(props) = if_map.get(SERV_IF_STR) {
                unimplemented!();
            } else if let Some(props) = if_map.get(CHAR_IF_STR) {
                unimplemented!()
            }
        }
        Ok(set)
    }
    fn insert_device(&mut self, device: RemoteDeviceBase) {
        let devmac = device.mac.clone();
        let comp = device.path.file_name().unwrap().to_os_string();
        self.devices.insert(devmac.clone(), device);
        self.comp_map.insert(comp, devmac);
    }
    pub fn discover_device(&mut self, mac: &MAC) -> Result<(), Error> {
        let devmac: PathBuf = match mac_to_devmac(mac) {
            Some(devmac) => devmac,
            None => return Err(Error::BadInput("Invalid mac was given".to_string())),
        }
        .into();
        self.discover_devices_filter(&self.blue_path.join(devmac))
            .map(|_| ())
        /*
        let mut msg = MessageBuilder::new().call(GET_ALL_CALL.to_string())
            .on(self.blue_path.join(&devmac).to_str().unwrap().to_string())
            .at(BLUEZ_DEST.to_string()).with_interface(PROP_IF_STR.to_string()).build();
        msg.add_param(Param::Base(DEV_IF_STR.into()));
        let res_idx = self.rpc_con.send_message(&mut msg, None)?;
        loop {
            self.process_requests()?;
            if let Some(res) = self.rpc_con.try_get_response(res_idx) {
                match res.typ {
                    MessageType::Reply => {
                        let device = if let Some(props) = res.params.get(0) {
                            RemoteDevice::from_props(props)?
                        } else {
                            let err_str = format!("Response returned for GetAll call to bluez is missing parameter: {:?}", res);
                            return Err(Error::Bluez(err_str));
                        };
                        self.insert_device(device, devmac.into());
                        return Ok(());
                    },
                    MessageType::Error => {
                        let err_str = format!("Error returned for GetAll call to bluez: {:?}", res);
                        return Err(Error::Bluez(err_str));
                    },
                    _ => unreachable!()
                }
            }
        }
        */
    }
}
fn mac_to_devmac(mac: &MAC) -> Option<String> {
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
fn validate_devmac(devmac: &str) -> bool {
    if devmac.len() != 21 {
        return false;
    }
    if !devmac.starts_with("dev") {
        return false;
    }
    let devmac = &devmac[3..];
    let mut chars = devmac.chars();
    for i in 0..6 {
        if chars.next().unwrap() != '_' {
            return false;
        }
        if chars.next().unwrap().is_lowercase() || chars.next().unwrap().is_lowercase() {
            return false;
        }
    }
    true
}
fn devmac_to_mac(devmac: &str) -> Option<MAC> {
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
            error_name: Some(err_name),
            flags: 0,
        };
        err_resp.push_param(text);
        err_resp

}
*/
trait ObjectManager {
    fn objectmanager_call(&mut self, msg: &Message<'_, '_>) -> OutMessage {
        match msg.member.as_ref().unwrap().as_ref() {
            MANGAGED_OBJ_CALL => self.get_managed_object(msg),
            _ => standard_messages::unknown_method(&msg),
        }
    }
    fn object_manager_type() -> signature::Type {
        signature::Type::Container(signature::Container::Dict(
            signature::Base::String,
            Box::new(LocalServiceBase::get_all_type()),
        ))
    }

    fn get_managed_object(&mut self, msg: &Message<'_, '_>) -> OutMessage;
}

impl ObjectManager for Bluetooth<'_, '_> {
    fn get_managed_object(&mut self, msg: &Message) -> OutMessage {
        let mut reply = msg.make_response();
        let mut outer_dict: HashMap<Base, Param> = HashMap::new();
        let path = self.get_path().to_path_buf();
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
        reply.body.push_old_param(&outer_cont.into());
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
    fn properties_call(&mut self, msg: &Message) -> OutMessage {
        match msg.member.as_ref().unwrap().as_ref() {
            "Get" => self.get(msg),
            "Set" => self.set(msg),
            GET_ALL_CALL => self.get_all(msg),
            _ => standard_messages::unknown_method(&msg),
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
        let prop_cont: Container = (signature::Base::String, Self::GET_ALL_ITEM, prop_map)
            .try_into()
            .unwrap();
        Some(prop_cont.into())
    }
    fn get_all(&mut self, msg: &Message) -> OutMessage {
        let interface = if let Some(interface) = &msg.interface {
            interface
        } else {
            return msg.make_error_response("Missing interface".to_string(), None);
        };
        if let Some(param) = self.get_all_inner(&interface) {
            let mut res = msg.make_response();
            res.body.push_old_param(&param);
            res
        } else {
            let err_msg = format!(
                "Interface {} is not known on {}",
                interface,
                msg.object.as_ref().unwrap()
            );
            msg.make_error_response("InterfaceNotFound".to_string(), Some(err_msg))
        }
    }

    fn get(&mut self, msg: &Message) -> OutMessage {
        if msg.params.len() < 2 {
            let err_str = "Expected two string arguments".to_string();
            return msg.make_error_response("Invalid arguments".to_string(), Some(err_str));
        }
        let interface = if let Param::Base(Base::String(interface)) = &msg.params[0] {
            interface
        } else {
            let err_str = "Expected string interface as first argument!".to_string();
            return msg.make_error_response("Invalid arguments".to_string(), Some(err_str));
        };
        let prop = if let Param::Base(Base::String(prop)) = &msg.params[1] {
            prop
        } else {
            let err_str = "Expected string property as second argument!".to_string();
            return msg.make_error_response("Invalid arguments".to_string(), Some(err_str));
        };
        if let Some(param) = self.get_inner(interface, prop) {
            let mut reply = msg.make_response();
            reply.body.push_old_param(&param);
            reply
        } else {
            let s = format!("Property {} on interface {} not found.", prop, interface);
            msg.make_error_response("PropertyNotFound".to_string(), Some(s))
        }
    }
    /// Should returng a variant containing if the property is found. If it is not found then it returns None.
    fn get_inner<'a, 'b>(&mut self, interface: &str, prop: &str) -> Option<Param<'a, 'b>>;
    fn set_inner(&mut self, interface: &str, prop: &str, val: &params::Variant) -> Option<String>;
    fn set(&mut self, msg: &Message) -> OutMessage {
        if msg.params.len() < 3 {
            return msg.make_error_response(
                "InvalidParameters".to_string(),
                Some("Expected three parameters".to_string()),
            );
        }
        let interface = if let Param::Base(Base::String(interface)) = &msg.params[0] {
            interface
        } else {
            return msg.make_error_response(
                "InvalidParameters".to_string(),
                Some("Expected string (interface) as first parameter".to_string()),
            );
        };
        let prop = if let Param::Base(Base::String(prop)) = &msg.params[1] {
            prop
        } else {
            return msg.make_error_response(
                "InvalidParameters".to_string(),
                Some("Expected string (property) as second parameter".to_string()),
            );
        };
        if let Param::Container(Container::Variant(val)) = &msg.params[2] {
            if let Some(err_str) = self.set_inner(&interface, &prop, &val) {
                msg.make_error_response(err_str, None)
            } else {
                msg.make_response()
            }
        } else {
            unimplemented!()
        }
    }
}

impl Properties for Bluetooth<'_, '_> {
    const INTERFACES: &'static [(&'static str, &'static [&'static str])] = &[];
    fn get_inner<'a, 'b>(&mut self, interface: &str, prop: &str) -> Option<Param<'a, 'b>> {
        None
    }
    fn set_inner(&mut self, interface: &str, prop: &str, val: &params::Variant) -> Option<String> {
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
    Value([u8; 255], usize),
    Function(Box<dyn FnMut() -> ([u8; 255], usize)>),
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
    pub fn to_value(&mut self) -> ([u8; 255], usize) {
        match self {
            ValOrFn::Value(v, l) => (*v, *l),
            ValOrFn::Function(f) => f(),
        }
    }
}

fn pair_to_key<'a, 'b, 'c>(
    pair: &'a (
        PathBuf,
        HashMap<String, HashMap<String, params::Variant<'b, 'c>>>,
    ),
) -> &'a Path {
    &pair.0
}
