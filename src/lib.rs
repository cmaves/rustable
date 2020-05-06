use rustbus::client_conn;
use rustbus::client_conn::{Conn, RpcConn};
use rustbus::message::{Message, MessageType};
use rustbus::message_builder::MessageBuilder;
use rustbus::params;
use rustbus::signature;
use rustbus::standard_messages;
use rustbus::{get_system_bus_path, Base, Container, Param};
use rustbus::get_session_bus_path;
use std::cell::{RefCell, RefMut};
use std::collections::HashMap;
use std::convert::{TryFrom, TryInto};
use std::fmt::{Debug, Display, Formatter};
use std::rc::Rc;
use std::time::Duration;

const PROP_IF_STR: &'static str = "org.freedesktop.Properties";
const SERV_IF_STR: &'static str = "org.bluez.GattService1";
const CHAR_IF_STR: &'static str = "org.bluez.Characteristic1";
const DESC_IF_STR: &'static str = "org.bluez.GattDescriptor1";
const MANAGER_IF_STR: &'static str = "org.bluez.GattManager1";
const SERV_IF_PROPS: &[&'static str] = &[UUID_PROP, PRIMARY_PROP, DEVICE_PROP, HANDLE_PROP]; // HANDLE_PROP is not used
const CHAR_IF_PROPS: &[&'static str] = &[
    UUID_PROP,
    SERVICE_PROP,
    VALUE_PROP,
    WRITE_ACQUIRED_PROP,
    NOTIFY_ACQUIRED_PROP,
    NOTIFYING_PROP,
    FLAGS_PROP,
    HANDLE_PROP,
];
const DESC_IF_PROPS: &[&'static str] = &[UUID_PROP, VALUE_PROP, FLAGS_PROP, HANDLE_PROP, CHAR_PROP];

const PROP_IF: (&'static str, &[&'static str])  = (PROP_IF_STR, &[]);
const SERV_IF: (&'static str, &[&'static str]) = (SERV_IF_STR, SERV_IF_PROPS);
const CHAR_IF: (&'static str, &[&'static str]) = (CHAR_IF_STR, CHAR_IF_PROPS);
const DESC_IF: (&'static str, &[&'static str]) = (DESC_IF_STR, DESC_IF_PROPS);
const BLUEZ_DEST: &'static str = "org.bluez";
const REGISTER_CALL: &'static str = "RegisterApplication";

const UUID_PROP: &'static str = "UUID";
const SERVICE_PROP: &'static str = "Service";
const VALUE_PROP: &'static str = "Value";
const WRITE_ACQUIRED_PROP: &'static str = "WriteAcquired";
const NOTIFY_ACQUIRED_PROP: &'static str = "NotifyAcquired";
const NOTIFYING_PROP: &'static str = "Notifying";
const FLAGS_PROP: &'static str = "Flags";
const HANDLE_PROP: &'static str = "Handle";
const CHAR_PROP: &'static str = "Characteristic";
const PRIMARY_PROP: &'static str = "Primary";
const DEVICE_PROP: &'static str = "Device";
const INCLUDES_PROP: &'static str = "Includes";

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
pub enum ValOrFn {
    Value([u8; 255], usize),
    Function(Box<dyn FnMut() -> ([u8; 255], usize)>),
}

impl ValOrFn {
#[inline]
	pub fn to_value(&mut self) -> ([u8; 255], usize) {
		match self {
			ValOrFn::Value(v, l) => (*v, *l),
			ValOrFn::Function(f) => f()
		}
	}
}
pub struct Descriptor {
	path: String,
    index: u16,
	uuid: String
}
impl Descriptor {
	pub fn new(uuid: String) -> Self {
		unimplemented!()
	}
	fn update_path(&mut self, base: &str) {
		self.path.clear();
		self.path.push_str(base);
		self.path.push_str("/char");
		let index_str = u16_to_ascii(self.index);
		for i in &index_str {
			self.path.push(*i);
		}
	}
}
#[derive(Clone, Copy, Default)]
pub struct CharFlags {
	pub broadcast: bool,
	pub read: bool,
	pub write_wo_response: bool,
	pub write: bool,
	pub notify: bool,
	pub indicate: bool,
	pub auth_signed_writes: bool,
	pub extended_properties: bool,
	pub reliable_write: bool,
	pub writable_auxiliaries: bool,
	pub encrypt_read: bool,
	pub encrypt_write: bool,
	pub encrypt_auth_read: bool,
	pub encrypt_auth_write: bool,
	pub secure_read: bool,
	pub secure_write: bool,
	pub authorize: bool
}
impl CharFlags {
	fn to_strings(&self) -> Vec<String> {
		let mut ret = Vec::new();
		if self.broadcast {
			ret.push("broadcast".to_string());
		}
		if self.read {
			ret.push("read".to_string());
		}
		if self.write {
			ret.push("write".to_string())
		}
		if self.write_wo_response{
			ret.push("write-without-response".to_string());
		}
		if self.notify {
			ret.push("notify".to_string());
		}
		if self.indicate {
			ret.push("indicate".to_string());
		}
		if self.auth_signed_writes {
			ret.push("authenticated-signed-writes".to_string());
		}
		if self.extended_properties {
			ret.push("extended-properties".to_string());
		}
		if self.reliable_write {
			ret.push("reliable-write".to_string());
		}
		if self.writable_auxiliaries {
			ret.push("writable-auxiliaries".to_string());
		}
		if self.encrypt_read {
			ret.push("encrypt-read".to_string());
		}
		if self.encrypt_write {
			ret.push("encrypt-write".to_string());
		}
		if self.encrypt_auth_read {
			ret.push("encrypt-authenticated-read".to_string());
		}
		if self.encrypt_auth_write {
			ret.push("encrypt-authenticated-write".to_string());
		}
		if self.secure_write {
			ret.push("secure-write".to_string());
		}
		if self.secure_read {
			ret.push("secure-read".to_string());
		}
		if self.authorize {
			ret.push("authorize".to_string());
		}
		ret
	}
}
#[derive(Clone, Copy, Default)]
pub struct DescFlags {
	pub read: bool,
	pub write: bool,
	pub encrypt_read: bool,
	pub encrypt_write: bool,
	pub encrypt_auth_read: bool,
	pub encrypt_auth_write: bool,
	pub secure_read: bool,
	pub secure_write: bool,
	pub authorize: bool
}
impl DescFlags {
	fn to_strings(&self) -> Vec<String> {
		let mut ret = Vec::new();
		if self.read {
			ret.push("read".to_string());
		}
		if self.write {
			ret.push("write".to_string())
		}
		if self.encrypt_read {
			ret.push("encrypt-read".to_string());
		}
		if self.encrypt_write {
			ret.push("encrypt-write".to_string());
		}
		if self.encrypt_auth_read {
			ret.push("encrypt-authenticated-read".to_string());
		}
		if self.encrypt_auth_write {
			ret.push("encrypt-authenticated-write".to_string());
		}
		if self.secure_write {
			ret.push("secure-write".to_string());
		}
		if self.secure_read {
			ret.push("secure-read".to_string());
		}
		if self.authorize {
			ret.push("authorize".to_string());
		}
		ret
	}
}
pub struct Charactersitic {
    vf: ValOrFn,
    index: u16,
    uuid: String,
	path: String,
	write_fd: Option<()>,
	notify_fd: Option<()>,
    descs: Vec<Descriptor>,
	flags: CharFlags
}
impl Charactersitic {
    pub fn new(uuid: String, flags: CharFlags) -> Self {
        Charactersitic {
            vf: ValOrFn::Value([0; 255], 0),
            index: 0,
			write_fd: None,
			notify_fd: None,
            uuid,
			flags,
			path: String::new(),
            descs: Vec::new(),
        }
    }
	fn update_path(&mut self, base: &str) {
		self.path.clear();
		self.path.push_str(base);
		self.path.push_str("/char");
		let index_str = u16_to_ascii(self.index);
		for i in &index_str {
			self.path.push(*i);
		}
		for desc in &mut self.descs {
			desc.update_path(&self.path);
		}
	}
    fn match_descs(&mut self, msg_path: &str, msg: &Message) -> Option<GattObject> {
        unimplemented!()
    }
}
pub struct Service {
    index: u8,
    uuid: String,
	path: String,
    chars: Vec<Charactersitic>,
	primary: bool
}
impl Service {
    pub fn new(uuid: String, primary: bool) -> Self {
        Service {
            index: 0,
            uuid,
			path: String::new(),
            chars: Vec::new(),
			primary
        }
    }
    pub fn add_char(&mut self, mut character: Charactersitic) {
        // TODO: add check for duplicate UUIDs
        assert!(self.chars.len() < 65535);
        character.index = self.chars.len() as u16;
        self.chars.push(character);
    }
    fn match_chars(&mut self, msg_path: &str, msg: &Message) -> Option<GattObject> {
        if msg_path.len() < 19 || &msg_path[0..1] != "/" {
            return None;
        }
        let msg_path = &msg_path[1..];
        if !msg_path.starts_with("char") {
            return None;
        }
        let msg_path = &msg_path[14..];
        let characteristics = &mut self.chars;
        for (i, character) in characteristics.iter_mut().enumerate() {
            if !msg_path.starts_with(&u16_to_ascii(character.index)[..]) {
                if msg_path.len() == 4 {
                    return Some(GattObject::Char(character));
                }
                character.match_descs(&msg_path[4..], &msg);
            }
        }
        None
    }
	fn update_path(&mut self, mut base: String) {
		base.push_str("/service");
		let index_str = u8_to_ascii(self.index);
		base.push(index_str[0]);
		base.push(index_str[1]);
		self.path = base;
		for character in &mut self.chars {
			character.update_path(&self.path);
		}
	}
}
pub enum GattObject<'a> {
    Char(&'a mut Charactersitic),
    Serv(&'a mut Service),
    Desc(&'a mut Descriptor),
    Appl,
}
pub struct Bluetooth<'a, 'b> {
    rpc_con: RpcConn<'a, 'b>,
    blue_path: String,
    name: String,
    verbose: u8,
    services: Vec<Service>,
    registered: bool,
    msg_idx: u32,
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
        let services = Vec::new();
        let mut ret = Bluetooth {
            rpc_con,
            name,
            verbose: 0,
            services,
            registered: false,
            blue_path,
            msg_idx: 0,
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
    pub fn add_service(&mut self, mut service: Service) -> Result<usize, Error> {
        if self.services.len() >= 255 {
            panic!("Cannot add more than 255 services");
        }
		let mut path = String::new();
		path.push('/');
		path.push_str(&self.name.replace(".", "/"));
		let index = self.services.len();
		service.index = index as u8;
		service.update_path(path);
        self.services.push(service);
        Ok(index)
    }
    pub fn read_characteristic(
        &mut self,
        service_index: usize,
        char_index: usize,
    ) -> ([u8; 255], usize) {
        match &mut self.services[service_index].chars[char_index].vf {
            ValOrFn::Value(buf, len) => (*buf, *len),
            ValOrFn::Function(f) => f(),
        }
    }
    pub fn write_characteristic(
        &mut self,
        service_index: usize,
        char_index: usize,
        val: ValOrFn,
    ) -> Result<(), Error> {
        let character = &mut self.services[service_index].chars[char_index];
        if let ValOrFn::Value(v, l) = val {
            if let ValOrFn::Value(ev, el) = character.vf {
                if l == el && &v[..] == &ev[..] {
                    return Ok(());
                }
            }
        }
        character.vf = val;
        let (buf, len) = match &mut character.vf {
            ValOrFn::Value(v, l) => (*v, *l),
            ValOrFn::Function(f) => f(),
        };
        self.signal_change(&buf[..len], service_index, char_index, None)
    }
    fn signal_change(
        &mut self,
        value: &[u8],
        service_index: usize,
        char_index: usize,
        desc: Option<usize>,
    ) -> Result<(), Error> {
        eprintln!("{:?}", value);
        let mut object_path = String::with_capacity(43);
        object_path.push('/');
        object_path.push_str(&self.name.replace(".", "/"));

        object_path.push('/');
        object_path.push_str("service");
        let ascii = u8_to_ascii(service_index as u8);
        object_path.push(ascii[0]);
        object_path.push(ascii[1]);

        object_path.push_str("/char");
        let ascii = u16_to_ascii(char_index as u16);
        object_path.push(ascii[0]);
        object_path.push(ascii[1]);
        object_path.push(ascii[2]);
        object_path.push(ascii[3]);

        let mut params = Vec::with_capacity(3);
        if let Some(val) = desc {
            params.push(Param::Base(Base::String(DESC_IF_STR.to_string())));
            // TODO: Add descriptor to path
            unimplemented!()
        } else {
            params.push(Param::Base(Base::String(CHAR_IF_STR.to_string())));
        }

        let changed_vec: Vec<Param> = value
            .into_iter()
            .map(|&b| Param::Base(Base::Byte(b)))
            .collect();
        let changed_arr = params::Array {
            element_sig: signature::Type::Base(signature::Base::Byte),
            values: changed_vec,
        };
        let changed_param = Param::Container(Container::Array(changed_arr));
        let mut changed_map = HashMap::with_capacity(1);
        changed_map.insert(Base::String("Value".to_string()), changed_param);
        let changed_dict = params::Dict {
            key_sig: signature::Base::String,
            value_sig: signature::Type::Container(signature::Container::Array(Box::new(
                signature::Type::Base(signature::Base::Byte),
            ))),
            map: changed_map,
        };
        params.push(Param::Container(Container::Dict(changed_dict)));

        let empty = params::Array {
            element_sig: signature::Type::Base(signature::Base::String),
            values: Vec::new(),
        };
        let empty = Param::Container(Container::Array(empty));
        params.push(empty);
        let mut msg = MessageBuilder::new()
            .signal(
                PROP_IF.0.to_string(),
                "PropertiesChanged".to_string(),
                object_path,
            )
            .with_params(params)
            .build();
        eprintln!("msg to be send: {:#?}", msg);
        self.rpc_con.send_message(&mut msg, None)?;
        Ok(())
    }
    pub fn remove_service(&mut self, service_index: usize) -> Result<Service, Error> {
        assert!(!self.registered);
        Ok(self.services.remove(service_index))
    }
    pub fn register_application(&mut self) -> Result<(), Error> {
        let mut path = String::with_capacity(self.name.len() + 1);
        path.push('/');
        path.push_str(&self.name.replace(".", "/"));
        let empty_dict = HashMap::new();
        let dict = params::Dict {
            key_sig: signature::Base::String,
            value_sig: signature::Type::Container(signature::Container::Variant),
            map: empty_dict,
        };
        let mut call_builder = MessageBuilder::new().call(REGISTER_CALL.to_string());
        call_builder.add_param2(
            Param::Base(Base::ObjectPath(path)),
            Param::Container(Container::Dict(dict)),
        );
        let mut msg = call_builder
            .with_interface(MANAGER_IF_STR.to_string())
            .on(self.blue_path.to_string())
            .at(BLUEZ_DEST.to_string())
            .build();

        let msg_idx = self.msg_idx;
        self.msg_idx += 1;
        /*msg.serial = Some(msg_idx);
        msg.sender = Some(self.name.clone());
        */
        eprintln!("registration msg: {:#?}", msg);
        let msg_idx = self.rpc_con.send_message(&mut msg, None)?;
        // we expect there to be no response
        match self
            .rpc_con
            .wait_response(msg_idx, Some(Duration::from_millis(500)))
        {
            Ok(res) => Err(Error::Bluez(format!(
                "Failed to register application with bluez: {:?}",
                res
            ))),
            Err(e) => {
                if let client_conn::Error::TimedOut = e {
                    self.registered = true;
                    Ok(())
                } else {
                    Err(Error::DbusClient(e))
                }
            }
        }
    }
    pub fn unregister_application(&mut self) -> Result<(), Error> {
        unimplemented!();
        self.registered = false;
        Ok(())
    }
    pub fn process_requests(&mut self) -> Result<(), Error> {
        let path = self.name.replace(".", "/");
        while let Some(call) = self.rpc_con.try_get_call() {
            let interface = (&call.interface).as_ref().unwrap();
            let mut reply = match self.match_root(&call, &path) {
                Some(v) => match v {
                    GattObject::Appl => match interface.as_ref() {
                        "org.freedesktop.DBus.Properties" => self.properties_call(&call),
                        "org.freedesktop.DBus.ObjectManager" => self.objectmanager_call(&call),
                        _ => unimplemented!(), // TODO: Added interface not found
                    },
                    GattObject::Serv(v) => match interface.as_ref() {
                        "org.freedesktop.DBus.Properties" => v.properties_call(&call),
                        "org.bluez.GattService1" => unimplemented!(),
                        _ => unimplemented!(), // TODO: Added interface not found
                    },
                    GattObject::Char(v) => match interface.as_ref() {
                        "org.freedesktop.DBus.Properties" => v.properties_call(&call),
                        "org.bluez.GattCharacteristic1" => unimplemented!(),
                        _ => unimplemented!(), // TODO: Added interface not found
                    },
                    GattObject::Desc(v) => match interface.as_ref() {
                        "org.freedesktop.DBus.Properties" => v.properties_call(&call),
                        "org.bluez.GattDescriptor1" => unimplemented!(),
                        _ => unimplemented!(), // TODO: Added interface not found
                    },
                },
                None => standard_messages::unknown_method(&call),
            };
			eprintln!("replying to: {:#?}\nreply: {:#?}", call, reply);
            self.rpc_con.send_message(&mut reply, None)?;
        }
        Ok(())
    }
    fn match_root(&mut self, msg: &Message, path: &str) -> Option<GattObject> {
        eprintln!("For path: {}, Checking msg for match: {:#?}", path, msg);
        if let None = &msg.interface {
            return None;
        }
        if let None = &msg.member {
            return None;
        }
        if let Some(object) = &msg.object {
            if object.len() >= 1 && object[1..].starts_with(&path) {
                if object.len() - 1 == path.len() {
                    return Some(GattObject::Appl);
                }
                return self.match_service(&object[path.len()..], msg);
            }
        }
        None
    }
    fn match_service(&mut self, msg_path: &str, msg: &Message) -> Option<GattObject> {
        eprintln!("Checking for service for match: {:#?}", msg);
        if msg_path.len() < 10 || &msg_path[0..1] != "/" {
            return None;
        }
        let msg_path = &msg_path[1..];
        if !msg_path.starts_with("service") {
            return None;
        }
        let msg_path = &msg_path[7..];
        for (i, service) in self.services.iter_mut().enumerate() {
            if msg_path.starts_with(&u8_to_ascii(service.index)[..]) {
                if msg_path.len() == 2 {
                    return Some(GattObject::Serv(service));
                }
                return service.match_chars(&msg_path[2..], &msg);
            }
        }
        None
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
            "GetManagedObjects" => self.get_managed_object(msg),
            _ => standard_messages::unknown_method(&msg),
        }
    }
	fn object_manager_type() -> signature::Type {
		signature::Type::Container(signature::Container::Dict(signature::Base::String, Box::new(Service::get_all_type())))
	}

    fn get_managed_object(&mut self, msg: &Message<'a, 'b>) -> Message<'a, 'b>;
}

impl<'a, 'b> ObjectManager<'a, 'b> for Bluetooth<'a, 'b> {
    fn get_managed_object(&mut self, msg: &Message<'a, 'b>) -> Message<'a, 'b> {
        let mut reply = msg.make_response();
        let mut outer_dict: HashMap<Base, Param> = HashMap::new();
        let mut path = String::new();
        path.push('/');
        path.push_str(&self.name.replace(".", "/"));
        path.push_str("/service");
        for service in &mut self.services {
            let mut service_path = path.clone();
            let index = u8_to_ascii(service.index);
            service_path.push(index[0]);
            service_path.push(index[1]);
            for characteristic in &mut service.chars {
                let mut char_path = service_path.clone();
                char_path.push('/');
                char_path.push_str("char");
                let index = u16_to_ascii(characteristic.index);
                for &i in &index {
                    char_path.push(i);
                }
                for desc in &mut characteristic.descs {
                    let mut desc_path = char_path.clone();
                    char_path.push('/');
                    let index = u16_to_ascii(desc.index);
                    for &i in &index[1..] {
                        desc_path.push(i);
                    }
					let mut middle_map = HashMap::new();
					for interface in Descriptor::INTERFACES {
						let props = desc.get_all_inner(interface.0).unwrap();
						middle_map.insert(interface.0.to_string().into(), props);
					}
					let middle_cont: Container = (signature::Base::String, Descriptor::get_all_type(), middle_map).try_into().unwrap();
					outer_dict.insert(Base::ObjectPath(desc_path), middle_cont.into());
                }
				let mut middle_map = HashMap::new();
				for interface in Charactersitic::INTERFACES {
					let props = characteristic.get_all_inner(interface.0).unwrap();
					middle_map.insert(interface.0.to_string().into(), props);
				}
				let middle_cont: Container = (signature::Base::String, Charactersitic::get_all_type(), middle_map).try_into().unwrap();
                outer_dict.insert(Base::ObjectPath(char_path), middle_cont.into());
            }
			let mut middle_map = HashMap::new();
			
			for interface in Service::INTERFACES {
					let props = service.get_all_inner(interface.0).unwrap();
					middle_map.insert(interface.0.to_string().into(), props);
			}
			let middle_cont: Container = (signature::Base::String, Service::get_all_type(), middle_map).try_into().unwrap();
            outer_dict.insert(Base::ObjectPath(service_path), middle_cont.into());
        }
        //let outer_param: Result<Param, std::convert::Infallible> = outer_dict.try_into();
        let outer_cont: Container = (signature::Base::ObjectPath, Self::object_manager_type(), outer_dict).try_into().unwrap();
        reply.push_param(outer_cont);
        reply
    }
}
trait Properties<'a, 'b> {
	const GET_ALL_ITEM: signature::Type = signature::Type::Container(signature::Container::Variant);
	fn get_all_type() -> signature::Type {
		signature::Type::Container(signature::Container::Dict(signature::Base::String, Box::new(Self::GET_ALL_ITEM)))
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
		let props = Self::INTERFACES.iter().find(|i| interface == i.0).map(|i| i.1)?;
        let mut prop_map = HashMap::new();
        for prop in props {
			eprintln!("{}: {}", interface, prop);
            let val = self.get_inner(interface, prop).unwrap();
            prop_map.insert(prop.to_string().into(), val);
        }
        let prop_cont: Container = (
            signature::Base::String,
            Self::GET_ALL_ITEM,
            prop_map,
        )
            .try_into()
            .unwrap();
        Some(prop_cont.into())
    }
    fn get_all(&mut self, msg: &Message<'a, 'b>) -> Message<'a, 'b> {
		unimplemented!()
	}

    fn get(&mut self, msg: &Message<'a, 'b>) -> Message<'a, 'b> {
		unimplemented!()
	}
    fn get_inner(&mut self, interface: &str, prop: &str) -> Option<Param<'a, 'b>>;
    fn set(&mut self, msg: &Message<'a, 'b>) -> Message<'a, 'b>;
    fn prop_change(&self, name: &Vec<String>) -> Message;
}

impl<'a, 'b> Properties<'a, 'b> for Bluetooth<'_, '_> {
	const INTERFACES: &'static [(&'static str, &'static [&'static str])] = &[];
    fn get_inner(&mut self, interface: &str, prop: &str) -> Option<Param<'a, 'b>> {

        unimplemented!()
    }
    fn set(&mut self, msg: &Message<'a, 'b>) -> Message<'a, 'b> {
        unimplemented!()
    }
    fn prop_change(&self, name: &Vec<String>) -> Message {
        unimplemented!()
    }
}

fn base_param_to_variant(b: Base) -> Param {
	let var = match b {
		Base::String(s) => params::Variant { sig: signature::Type::Base(signature::Base::String), value: Param::Base(s.into()) },
		Base::Boolean(b) => params::Variant { sig: signature::Type::Base(signature::Base::Boolean), value: Param::Base(b.into()) },
		Base::Uint16(u) => params::Variant { sig: signature::Type::Base(signature::Base::Uint16), value: Param::Base(u.into()) },
		Base::ObjectPath(p) => params::Variant { sig: signature::Type::Base(signature::Base::ObjectPath), value: Param::Base(Base::ObjectPath(p)) },
		Base::Byte(b) => params::Variant { sig: signature::Type::Base(signature::Base::Byte), value: Param::Base(b.into()) },
		_ => unimplemented!()
	};
	Param::Container(Container::Variant(Box::new(var)))
}
impl<'a, 'b> Properties<'a, 'b> for Charactersitic {
    const INTERFACES: &'static [(&'static str, &'static [&'static str])] = &[CHAR_IF, PROP_IF];
    fn get_inner(&mut self, interface: &str, prop: &str) -> Option<Param<'a, 'b>> {
	    match interface {
            CHAR_IF_STR => match prop {
                UUID_PROP => Some(base_param_to_variant(self.uuid.clone().into())),
                SERVICE_PROP => {
					let pnt = self.path.len() - 9;
					Some(base_param_to_variant(Base::ObjectPath(self.path.split_at(pnt).0.to_string())))
				},
                VALUE_PROP => {
					let (v, l) = self.vf.to_value();
					let vec: Vec<Param> = v[..l].into_iter().map(|i| Base::Byte(*i).into()).collect();
					let val = Param::Container(Container::Array(params::Array { element_sig: signature::Type::Base(signature::Base::Byte), values: vec } ));
					let var = Box::new(params::Variant { sig: signature::Type::Container(signature::Container::Array(Box::new(signature::Type::Base(signature::Base::Byte)))), value: val });
					Some(Param::Container(Container::Variant(var)))
				},
                WRITE_ACQUIRED_PROP => Some(base_param_to_variant(Base::Boolean(self.write_fd.is_some()))),
                NOTIFY_ACQUIRED_PROP => Some(base_param_to_variant(Base::Boolean(self.notify_fd.is_some()))),
                NOTIFYING_PROP => Some(base_param_to_variant(Base::Boolean(true))),
                FLAGS_PROP => {
					let flags = self.flags.to_strings();
					let vec = flags.into_iter().map(|s| Base::String(s).into()).collect();
					let val = Param::Container(Container::Array(params::Array { element_sig: signature::Type::Base(signature::Base::String), values: vec} ));
					let var = Box::new(params::Variant { sig: signature::Type::Container(signature::Container::Array(Box::new(signature::Type::Base(signature::Base::String)))), value: val });
					Some(Param::Container(Container::Variant(var)))
				},
                HANDLE_PROP => Some(base_param_to_variant(Base::Uint16(self.index))),
                _ => None,
            },
            PROP_IF_STR => match prop {
                _ => None,
            },
			_ => None
        }
    }
    fn set(&mut self, msg: &Message<'a, 'b>) -> Message<'a, 'b> {
        unimplemented!()
    }
    fn get_all(&mut self, msg: &Message<'a, 'b>) -> Message<'a, 'b> {
        unimplemented!()
    }
    fn prop_change(&self, name: &Vec<String>) -> Message {
        unimplemented!()
    }
}
impl<'a, 'b> Properties<'a, 'b> for Descriptor {
	const INTERFACES: &'static [(&'static str, &'static [&'static str])] = &[DESC_IF, PROP_IF];
    fn get_inner(&mut self, interface: &str, prop: &str) -> Option<Param<'a, 'b>> {
		match interface {
			DESC_IF_STR => match prop {
				UUID_PROP => Some(base_param_to_variant(self.uuid.clone().into())),
				CHAR_PROP => {
					let pnt = self.path.len() - 14;
					Some(base_param_to_variant(Base::ObjectPath(self.path.split_at(pnt).0.to_string())))
				},
				VALUE_PROP => {
					unimplemented!()
				},
				FLAGS_PROP => {
					unimplemented!()
				},
				HANDLE_PROP => Some(base_param_to_variant(self.index.into())),
				_ => None
			},
			PROP_IF_STR => None,
			_ => None
		}
    }
    fn set(&mut self, msg: &Message<'a, 'b>) -> Message<'a, 'b> {
        unimplemented!()
    }
    fn get_all(&mut self, msg: &Message<'a, 'b>) -> Message<'a, 'b> {
        unimplemented!()
    }
    fn prop_change(&self, name: &Vec<String>) -> Message {
        unimplemented!()
    }
}
impl<'a, 'b> Properties<'a, 'b> for Service {
	const INTERFACES: &'static [(&'static str, &'static [&'static str])] = &[SERV_IF, PROP_IF];
    fn get_inner(&mut self, interface: &str, prop: &str) -> Option<Param<'a, 'b>> {
		match interface {
				SERV_IF_STR => match prop {
					UUID_PROP => Some(base_param_to_variant(self.uuid.clone().into())),
					PRIMARY_PROP => Some(base_param_to_variant(self.primary.into())),
					DEVICE_PROP => {
						let pnt = self.path.len() - 10;
						Some(base_param_to_variant(Base::ObjectPath(self.path.split_at(pnt).0.to_string())))
					}
					HANDLE_PROP => Some(base_param_to_variant(self.index.into())),
					_ => None
				},
				PROP_IF_STR => None,
				_ => None,
		}
    }
    fn set(&mut self, msg: &Message<'a, 'b>) -> Message<'a, 'b> {
        unimplemented!()
    }
    fn prop_change(&self, name: &Vec<String>) -> Message {
        unimplemented!()
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn it_works() {
        assert_eq!(2 + 2, 4);
    }
}

fn half_byte(val: u8) -> char {
    if val <= 10 {
        (48 + val) as char
    } else {
        (val + 65 - 11) as char
    }
}
fn u8_to_ascii(val: u8) -> [char; 2] {
    [half_byte((val >> 4) & 0xF), half_byte(val & 0xF)]
}
fn u16_to_ascii(val: u16) -> [char; 4] {
    [
        half_byte((val >> 12) as u8),
        half_byte(((val >> 8) & 0xF) as u8),
        half_byte(((val >> 4) & 0xF) as u8),
        half_byte((val & 0xF) as u8),
    ]
}
