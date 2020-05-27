use gatt::*;
use rustbus::client_conn;
use rustbus::client_conn::{Conn, RpcConn};
use rustbus::get_session_bus_path;
use rustbus::message::{Message, MessageType};
use rustbus::message_builder::MessageBuilder;
use rustbus::params;
use rustbus::signature;
use rustbus::standard_messages;
use rustbus::{get_system_bus_path, Base, Container, Param};
use std::cell::{RefCell, RefMut};
use std::collections::{HashMap, VecDeque};
use std::convert::{TryFrom, TryInto};
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

use interfaces::*;
pub mod gatt;

mod introspect;
use introspect::Introspectable;

#[cfg(test)]
mod tests;

pub type UUID = Rc<str>;

pub trait ToUUID {
	fn to_uuid(&self) -> UUID;
}
impl ToUUID for &str {
	fn to_uuid(&self) -> UUID {
		assert!(validate_uuid(self));
		let deref = *self;
		deref.into()
	}
}
impl ToUUID for UUID {
	fn to_uuid(&self) -> UUID {
		self.clone()
	}
}
impl ToUUID for u128 {
	fn to_uuid(&self) -> UUID {
		format!("{:08x}-{:04x}-{:04x}-{:04x}-{:012x}", self >> 24, (self >> 20) & 0xFFFF, (self >> 16) & 0xFFFF, (self >> 12) & 0xFFFF, self & 0xFFFFFFFFFFFF).into()
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

/*
pub struct Device {
    services: Service
}*/
pub struct Bluetooth<'a, 'b> {
    rpc_con: RpcConn<'a, 'b>,
    blue_path: String,
    name: String,
    path: PathBuf,
    pub verbose: u8,
    services: HashMap<UUID, LocalServiceBase>,
    registered: bool,
    pub filter_dest: Option<String>,
    pub ads: VecDeque<Advertisement>,
    service_index: u8,
}

impl<'a, 'b> Bluetooth<'a, 'b> {
    pub fn new(dbus_name: &str, blue_path: String) -> Result<Self, Error> {
        let session_path = get_system_bus_path()?;
        let conn = Conn::connect_to_bus(session_path, true)?;
        let mut rpc_con = RpcConn::new(conn);
        let mut name = "io.rustable.".to_string();
        name.push_str(dbus_name);
        rpc_con.send_message(&mut standard_messages::hello(), None)?;
        let namereq =
            rpc_con.send_message(&mut standard_messages::request_name(name.clone(), 0), None)?;
        let res = rpc_con.wait_response(namereq, None)?;
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
        let mut ret = Bluetooth {
            rpc_con,
            name,
            verbose: 0,
            services,
            registered: false,
            blue_path,
            path,
            filter_dest: Some(BLUEZ_DEST.to_string()),
            ads: VecDeque::new(),
            service_index: 0,
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
    pub fn local_service<T: ToUUID>(&mut self, uuid: &T) -> Option<LocalService<'_, 'a, 'b>> {
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
        call_builder.add_param2(
            Param::Base(Base::ObjectPath(
                path.as_os_str().to_str().unwrap().to_string(),
            )),
            Param::Container(Container::Dict(dict)),
        );
        let mut msg = call_builder
            .with_interface(MANAGER_IF_STR.to_string())
            .on(self.blue_path.to_string())
            .at(BLUEZ_DEST.to_string())
            .build();

        eprintln!("registration msg: {:#?}", msg);
        let msg_idx = self.rpc_con.send_message(&mut msg, None)?;
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
        loop {
            match self.rpc_con.wait_call(Some(Duration::from_micros(500))) {
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
                    self.rpc_con.send_message(&mut reply, None)?;
                }
                Err(e) => match e {
                    client_conn::Error::TimedOut => return Ok(()),
                    _ => return Err(e.into()),
                },
            }
        }
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
trait ObjectManager<'a, 'b> {
    fn objectmanager_call(&mut self, msg: &Message<'a, 'b>) -> Message<'a, 'b> {
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

    fn get_managed_object(&mut self, msg: &Message<'a, 'b>) -> Message<'a, 'b>;
}

impl<'a, 'b> ObjectManager<'a, 'b> for Bluetooth<'a, 'b> {
    fn get_managed_object(&mut self, msg: &Message<'a, 'b>) -> Message<'a, 'b> {
        let mut reply = msg.make_response();
        let mut outer_dict: HashMap<Base, Param> = HashMap::new();
		let path = self.get_path().to_path_buf();
        for service in self.services.values_mut() {
			let service_path = path.join(format!("service{:02x}", service.index));
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
                    outer_dict.insert(Base::ObjectPath(desc.path.to_str().unwrap().to_string()), middle_cont.into());
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
                outer_dict.insert(Base::ObjectPath(characteristic.path.to_str().unwrap().to_string()), middle_cont.into());
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
            outer_dict.insert(Base::ObjectPath(service.path.to_str().unwrap().to_string()), middle_cont.into());
        }
        //let outer_param: Result<Param, std::convert::Infallible> = outer_dict.try_into();
        let outer_cont: Container = (
            signature::Base::ObjectPath,
            Self::object_manager_type(),
            outer_dict,
        )
            .try_into()
            .unwrap();
        reply.push_param(outer_cont);
        reply
    }
}
trait Properties<'a, 'b> {
    const GET_ALL_ITEM: signature::Type = signature::Type::Container(signature::Container::Variant);
    fn get_all_type() -> signature::Type {
        signature::Type::Container(signature::Container::Dict(
            signature::Base::String,
            Box::new(Self::GET_ALL_ITEM),
        ))
    }
    const INTERFACES: &'static [(&'static str, &'static [&'static str])];
    fn properties_call(&mut self, msg: &Message<'a, 'b>) -> Message<'a, 'b> {
        match msg.member.as_ref().unwrap().as_ref() {
            "Get" => self.get(msg),
            "Set" => self.set(msg),
            "GetAll" => self.get_all(msg),
            _ => standard_messages::unknown_method(&msg),
        }
    }
    fn get_all_inner(&mut self, interface: &str) -> Option<Param<'a, 'b>> {
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
    fn get_all(&mut self, msg: &Message<'a, 'b>) -> Message<'a, 'b> {
        let interface = if let Some(interface) = &msg.interface {
            interface
        } else {
            return msg.make_error_response("Missing interface".to_string(), None);
        };
        if let Some(param) = self.get_all_inner(&interface) {
            let mut res = msg.make_response();
            res.push_param(param);
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

    fn get(&mut self, msg: &Message<'a, 'b>) -> Message<'a, 'b> {
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
            reply.push_param(param);
            reply
        } else {
            let s = format!("Property {} on interface {} not found.", prop, interface);
            msg.make_error_response("PropertyNotFound".to_string(), Some(s))
        }
    }
    /// Should returng a variant containing if the property is found. If it is not found then it returns None.
    fn get_inner(&mut self, interface: &str, prop: &str) -> Option<Param<'a, 'b>>;
    fn set_inner(&mut self, interface: &str, prop: &str, val: &params::Variant) -> Option<String>;
    fn set(&mut self, msg: &Message<'a, 'b>) -> Message<'a, 'b> {
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

impl<'a, 'b> Properties<'a, 'b> for Bluetooth<'_, '_> {
    const INTERFACES: &'static [(&'static str, &'static [&'static str])] = &[];
    fn get_inner(&mut self, interface: &str, prop: &str) -> Option<Param<'a, 'b>> {
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
pub struct Advertisement {
    pub typ: AdType,
    pub service_uuids: Vec<String>,
    // pub manu_data: HashMap<String, ()>
    pub solicit_uuids: Vec<String>,
    pub appearance: u16,
    pub timeout: u16,
    pub duration: u16,
    pub localname: String,
    pub index: u16,
}
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

impl<'a, 'b> Properties<'a, 'b> for Advertisement {
    const INTERFACES: &'static [(&'static str, &'static [&'static str])] = &[LEAD_IF, PROP_IF];
    fn get_inner(&mut self, interface: &str, prop: &str) -> Option<Param<'a, 'b>> {
        match interface {
            LEAD_IF_STR => match prop {
                TYPE_PROP => unimplemented!(),
                SERV_UUIDS_PROP => unimplemented!(),
                MANU_DATA_PROP => unimplemented!(),
                SOLICIT_UUIDS_PROP => unimplemented!(),
                SERV_DATA_PROP => unimplemented!(),
                DATA_PROP => unimplemented!(),
                DISCOVERABLE_PROP => unimplemented!(),
                DISCOVERABLE_TO_PROP => unimplemented!(),
                INCLUDES_PROP => unimplemented!(),
                LOCAL_NAME_PROP => unimplemented!(),
                APPEARANCE_PROP => unimplemented!(),
                DURATION_PROP => unimplemented!(),
                TO_PROP => unimplemented!(),
                SND_CHANNEL_PROP => unimplemented!(),
                _ => None,
            },
            _ => None,
        }
    }
    fn set_inner(&mut self, interface: &str, prop: &str, val: &params::Variant) -> Option<String> {
        unimplemented!()
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
