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

mod introspect;
use introspect::Introspectable;

#[cfg(test)]
mod tests;

const PROP_IF_STR: &'static str = "org.freedesktop.Properties";
const SERV_IF_STR: &'static str = "org.bluez.GattService1";
const CHAR_IF_STR: &'static str = "org.bluez.GattCharacteristic1";
const DESC_IF_STR: &'static str = "org.bluez.GattDescriptor1";
const MANAGER_IF_STR: &'static str = "org.bluez.GattManager1";
const LEAD_IF_STR: &'static str = "org.bluez.LEAdvertisement1";

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
const TYPE_PROP: &'static str = "Type";
const SERV_UUIDS_PROP: &'static str = "ServiceUUIDs";
const SOLICIT_UUIDS_PROP: &'static str = "SolicitUUIDs";
const SERV_DATA_PROP: &'static str = "ServiceData";
const DATA_PROP: &'static str = "Data";
const MANU_DATA_PROP: &'static str = "ManufacturererData";
const DISCOVERABLE_PROP: &'static str = "Discoverable";
const DISCOVERABLE_TO_PROP: &'static str = "DiscoverableTimeout";
const LOCAL_NAME_PROP: &'static str = "LocalName";
const APPEARANCE_PROP: &'static str = "Appearance";
const DURATION_PROP: &'static str = "Duration";
const TO_PROP: &'static str = "Timeout";
const SND_CHANNEL_PROP: &'static str = "SecondaryChannel";

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

const LEAD_IF_PROPS: &[&'static str] = &[
    TYPE_PROP,
    SERV_UUIDS_PROP,
    MANU_DATA_PROP,
    SERV_DATA_PROP,
    DATA_PROP,
    DISCOVERABLE_PROP,
    DISCOVERABLE_TO_PROP,
    INCLUDES_PROP,
    LOCAL_NAME_PROP,
    APPEARANCE_PROP,
    DURATION_PROP,
    TO_PROP,
    SND_CHANNEL_PROP,
];

const PROP_IF: (&'static str, &[&'static str]) = (PROP_IF_STR, &[]);
const SERV_IF: (&'static str, &[&'static str]) = (SERV_IF_STR, SERV_IF_PROPS);
const CHAR_IF: (&'static str, &[&'static str]) = (CHAR_IF_STR, CHAR_IF_PROPS);
const DESC_IF: (&'static str, &[&'static str]) = (DESC_IF_STR, DESC_IF_PROPS);
const LEAD_IF: (&'static str, &[&'static str]) = (LEAD_IF_STR, LEAD_IF_PROPS);

const BLUEZ_DEST: &'static str = "org.bluez";
const REGISTER_CALL: &'static str = "RegisterApplication";

// Bluez Errors
const BLUEZ_NOT_PERM: &'static str = "org.bluez.Error.NotPermitted";
const BLUEZ_FAILED: &'static str = "org.bluez.Error.Failed";

// Standard DBus Errors
const UNKNOWN_METHOD: &'static str = "org.dbus.freedesktop.UnknownMethod";

pub struct UUID {
    uuid: u128,
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

#[derive(Debug)]
pub struct Descriptor {
    path: String,
    index: u16,
    handle: u16,
    uuid: String,
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
#[derive(Clone, Copy, Default, Debug)]
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
    pub authorize: bool,
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
        if self.write_wo_response {
            ret.push("write-without-response".to_string());
        }
        if self.notify {
            ret.push("notify".to_string());
        }
        if self.indicate {
            ret.push("indicate".to_string());
        }
        if self.auth_signed_writes {
            unimplemented!();
            ret.push("authenticated-signed-writes".to_string());
        }
        if self.extended_properties {
            ret.push("extended-properties".to_string());
        }
        if self.reliable_write {
            ret.push("reliable-write".to_string());
        }
        if self.writable_auxiliaries {
            unimplemented!();
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
            unimplemented!();
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
    pub authorize: bool,
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
            unimplemented!();
            ret.push("authorize".to_string());
        }
        ret
    }
}

#[derive(Debug)]
enum Notify {
    Signal,
    Fd(UnixDatagram),
}
#[derive(Debug)]
pub struct Charactersitic {
    vf: ValOrFn,
    index: u16,
    handle: u16,
    uuid: String,
    path: String,
    notify: Option<Notify>,
    write: Option<UnixDatagram>,
    descs: Vec<Descriptor>,
    flags: CharFlags,
}
impl Charactersitic {
    pub fn new(uuid: String, flags: CharFlags) -> Self {
        Charactersitic {
            vf: ValOrFn::Value([0; 255], 0),
            index: 0,
            handle: 0,
            write: None,
            notify: None,
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
    fn match_descs(&mut self, msg_path: &Path, msg: &Message) -> Option<GattObject> {
        unimplemented!()
    }
    fn char_call<'a, 'b>(&mut self, call: &Message<'a, 'b>) -> Message<'a, 'b> {
        if let Some(member) = &call.member {
            match &member[..] {
                "ReadValue" => {
                    if self.flags.read
                        || self.flags.secure_read
                        || self.flags.secure_read
                        || self.flags.encrypt_read
                    {
                        let (v, l) = self.vf.to_value();
                        let mut start = 0;
                        if let Some(dict) = call.params.get(0) {
                            if let Param::Container(Container::Dict(dict)) = dict {
                                if let Some(offset) =
                                    dict.map.get(&Base::String("offset".to_string()))
                                {
                                    if let Param::Container(Container::Variant(offset)) = offset {
                                        if let Param::Base(Base::Uint16(offset)) = offset.value {
                                            start = l.min(offset as usize);
                                        } else {
                                            return call.make_error_response("UnexpectedType".to_string(), Some("Expected a dict of variants as first parameter".to_string()));
                                        }
                                    } else {
                                        return call.make_error_response(
                                            "UnexpectedType".to_string(),
                                            Some(
                                                "Expected a dict of variants as first parameter"
                                                    .to_string(),
                                            ),
                                        );
                                    }
                                }
                            } else {
                                return call.make_error_response(
                                    "UnexpectedType".to_string(),
                                    Some("Expected a dict as first parameter".to_string()),
                                );
                            }
                        }
                        // eprintln!("vf: {:?}\nValue: {:?}", self.vf, &v[..l]);
                        let vec: Vec<Param> = v[start..l]
                            .into_iter()
                            .map(|i| Base::Byte(*i).into())
                            .collect();
                        let val = Param::Container(Container::Array(params::Array {
                            element_sig: signature::Type::Base(signature::Base::Byte),
                            values: vec,
                        }));
                        let mut res = call.make_response();
                        res.add_param(val);
                        res
                    } else {
                        call.make_error_response(
                            BLUEZ_NOT_PERM.to_string(),
                            Some("This is not a readable characteristic.".to_string()),
                        )
                    }
                }
                "WriteValue" => {
                    if self.flags.write
                        || self.flags.write_wo_response
                        || self.flags.secure_write
                        || self.flags.encrypt_write
                        || self.flags.encrypt_auth_write
                    {
                        unimplemented!();
                    } else {
                        call.make_error_response(
                            BLUEZ_NOT_PERM.to_string(),
                            Some("This is not a writable characteristic.".to_string()),
                        )
                    }
                }
                "AcquireWrite" => {
                    match UnixDatagram::pair() {
                        Ok((sock1, sock2)) => {
                            unimplemented!();
                            let mut ret = 255;
                            if let Some(dict) = call.params.get(0) {
                                if let Param::Container(Container::Dict(dict)) = dict {
                                    if let Some(mtu) =
                                        dict.map.get(&Base::String("mtu".to_string()))
                                    {
                                        if let Param::Container(Container::Variant(mtu)) = mtu {
                                            if let Param::Base(Base::Uint16(mtu)) = mtu.value {
                                                ret = ret.min(mtu);
                                            } else {
                                                return call.make_error_response("UnexpectedType".to_string(), Some("Expected a dict of UInt16 as first offset type".to_string()));
                                            }
                                        } else {
                                            return call.make_error_response("UnexpectedType".to_string(), Some("Expected a dict of variants as first parameter".to_string()));
                                        }
                                    }
                                } else {
                                    return call.make_error_response(
                                        "UnexpectedType".to_string(),
                                        Some("Expected a dict as first parameter".to_string()),
                                    );
                                }
                            }
                            let mut res = call.make_response();
                            res.add_param2(
                                Param::Base(Base::Uint32(sock1.as_raw_fd() as u32)),
                                Param::Base(Base::Uint16(ret)),
                            );
                            return res;
                        }
                        Err(_) => {
                            return call.make_error_response(
                                BLUEZ_FAILED.to_string(),
                                Some(
                                    "An IO Error occured when creating the unix datagram socket."
                                        .to_string(),
                                ),
                            )
                        }
                    }
                }
                "AcquireNotify" => {
                    if !self.flags.notify {
                        call.make_error_response(
                            BLUEZ_NOT_PERM.to_string(),
                            Some("This characteristic doesn't not permit notifying.".to_string()),
                        )
                    } else if let Some(notify) = &self.notify {
                        let err_str = match notify {
                            Notify::Signal => {
                                "This characteristic is already notifying via signals."
                            }
                            Notify::Fd(_) => {
                                "This characteristic is already notifying via a socket."
                            }
                        };
                        call.make_error_response(
                            "org.bluez.Error.InProgress".to_string(),
                            Some(err_str.to_string()),
                        )
                    } else {
                        match UnixDatagram::pair() {
                            Ok((sock1, sock2)) => {
                                let mut ret = 255;
                                if let Some(dict) = call.params.get(0) {
                                    if let Param::Container(Container::Dict(dict)) = dict {
                                        if let Some(mtu) =
                                            dict.map.get(&Base::String("mtu".to_string()))
                                        {
                                            if let Param::Container(Container::Variant(mtu)) = mtu {
                                                if let Param::Base(Base::Uint16(mtu)) = mtu.value {
                                                    ret = ret.min(mtu);
                                                } else {
                                                    return call.make_error_response("UnexpectedType".to_string(), Some("Expected a dict of UInt16 as first offset type".to_string()));
                                                }
                                            } else {
                                                return call.make_error_response("UnexpectedType".to_string(), Some("Expected a dict of variants as first parameter".to_string()));
                                            }
                                        }
                                    } else {
                                        return call.make_error_response(
                                            "UnexpectedType".to_string(),
                                            Some("Expected a dict as first parameter".to_string()),
                                        );
                                    }
                                }
                                let mut res = call.make_response();
                                res.add_param2(
                                    Param::Base(Base::Uint32(sock1.as_raw_fd() as u32)),
                                    Param::Base(Base::Uint16(ret)),
                                );
                                self.notify = Some(Notify::Fd(sock2));
                                res
                            }
                            Err(_) => call.make_error_response(
                                BLUEZ_FAILED.to_string(),
                                Some(
                                    "An IO Error occured when creating the unix datagram socket."
                                        .to_string(),
                                ),
                            ),
                        }
                    }
                }
                "StartNotify" => {
                    if !self.flags.notify {
                        call.make_error_response(
                            BLUEZ_NOT_PERM.to_string(),
                            Some("This characteristic doesn't not permit notifying.".to_string()),
                        )
                    } else if let Some(notify) = &self.notify {
                        let err_str = match notify {
                            Notify::Signal => {
                                "This characteristic is already notifying via signals."
                            }
                            Notify::Fd(_) => {
                                "This characteristic is already notifying via a socket."
                            }
                        };
                        call.make_error_response(
                            "org.bluez.Error.InProgress".to_string(),
                            Some(err_str.to_string()),
                        )
                    } else {
                        self.notify = Some(Notify::Signal);
                        call.make_response()
                    }
                }
                "StopNotify" => {
                    if let Some(notify) = self.notify.as_ref() {
                        self.notify = None;
                        call.make_response()
                    } else {
                        call.make_error_response(
                            BLUEZ_FAILED.to_string(),
                            Some("Notify has not been started".to_string()),
                        )
                    }
                }
                "Confirm" => call.make_response(),
                _ => call.make_error_response(UNKNOWN_METHOD.to_string(), None),
            }
        } else {
            // TODO: remove this statement if unneeded
            unreachable!();
        }
    }
}
pub struct Service {
    index: u8,
    handle: u16,
    uuid: String,
    path: String,
    chars: Vec<Charactersitic>,
    primary: bool,
}
impl Service {
    pub fn new(uuid: String, primary: bool) -> Self {
        Service {
            index: 0,
            handle: 0,
            uuid,
            path: String::new(),
            chars: Vec::new(),
            primary,
        }
    }
    pub fn add_char(&mut self, mut character: Charactersitic) {
        // TODO: add check for duplicate UUIDs
        assert!(self.chars.len() < 65535);
        character.index = self.chars.len() as u16;
        self.chars.push(character);
    }
    fn match_chars(&mut self, msg_path: &Path, msg: &Message) -> Option<GattObject> {
        eprintln!("Checking for characteristic for match");
        let mut components = msg_path.components().take(2);
        if let Component::Normal(path) = components.next().unwrap() {
            let path = path.to_str()?;
            if !path.starts_with("char") {
                return None;
            }
            let mut char_str = String::new();
            for character in self.chars.iter_mut() {
                char_str.clear();
                write!(&mut char_str, "char{:02}", character.index).unwrap();
                if let Ok(path) = msg_path.strip_prefix(char_str) {
                    return character.match_descs(path, msg);
                } else {
                    return Some(GattObject::Char(character));
                }
            }
            None
        } else {
            None
        }
    }
    fn update_path(&mut self, mut base: String) {
        base.push_str("/service");
        //let index_str = u8_to_ascii(self.index);
        write!(base, "{:02}", self.index).unwrap();
        //base.push(index_str[0]);
        //base.push(index_str[1]);
        self.path = base;
        for character in &mut self.chars {
            character.update_path(&self.path);
        }
    }
    pub fn service_call<'a, 'b>(&mut self, call: &Message<'a, 'b>) -> Message<'a, 'b> {
        unimplemented!()
    }
}
pub enum GattObject<'a> {
    Char(&'a mut Charactersitic),
    Serv(&'a mut Service),
    Desc(&'a mut Descriptor),
    Ad(&'a Advertisement),
    Appl,
}
pub struct Bluetooth<'a, 'b> {
    rpc_con: RpcConn<'a, 'b>,
    blue_path: String,
    name: String,
    pub verbose: u8,
    services: Vec<Service>,
    registered: bool,
    pub filter_dest: Option<String>,
    pub ads: VecDeque<Advertisement>,
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
            filter_dest: Some(BLUEZ_DEST.to_string()),
            ads: VecDeque::new(),
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
    pub fn get_path(&self) -> PathBuf {
        let mut ret = String::with_capacity(self.name.len() + 1);
        ret.push('/');
        ret.push_str(&self.name.replace(".", "/"));
        PathBuf::from(ret)
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
        if let Some(notify) = &mut character.notify {
            match notify {
                Notify::Signal => self.signal_change(&buf[..len], service_index, char_index, None),
                Notify::Fd(sock) => {
                    if let Err(_) = sock.send(&buf[..len]) {
                        character.notify = None;
                    }
                    Ok(())
                }
            }
        } else {
            Ok(())
        }
    }
    pub fn start_advertise(&mut self, adv: Advertisement) -> Result<(), Error> {
        unimplemented!()
    }
    fn signal_change(
        &mut self,
        value: &[u8],
        service_index: usize,
        char_index: usize,
        desc: Option<usize>,
    ) -> Result<(), Error> {
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
        // eprintln!("msg to be send: {:#?}", msg);
        self.rpc_con.send_message(&mut msg, None)?;
        Ok(())
    }
    pub fn remove_service(&mut self, service_index: usize) -> Result<Service, Error> {
        assert!(!self.registered);
        Ok(self.services.remove(service_index))
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
                            GattObject::Appl => match interface.as_ref() {
                                "org.freedesktop.DBus.Properties" => self.properties_call(&call),
                                "org.freedesktop.DBus.ObjectManager" => {
                                    self.objectmanager_call(&call)
                                }
                                "org.freedesktop.DBus.Introspectable" => self.introspectable(&call),
                                _ => unimplemented!(), // TODO: Added interface not found
                            },
                            GattObject::Serv(v) => match interface.as_ref() {
                                "org.freedesktop.DBus.Properties" => v.properties_call(&call),
                                "org.bluez.GattService1" => v.service_call(&call),
                                "org.freedesktop.DBus.Introspectable" => v.introspectable(&call),
                                _ => unimplemented!(), // TODO: Added interface not found
                            },
                            GattObject::Char(v) => match interface.as_ref() {
                                "org.freedesktop.DBus.Properties" => v.properties_call(&call),
                                "org.bluez.GattCharacteristic1" => v.char_call(&call),
                                "org.freedesktop.DBus.Introspectable" => v.introspectable(&call),
                                _ => unimplemented!(), // TODO: Added interface not found
                            },
                            GattObject::Desc(v) => match interface.as_ref() {
                                "org.freedesktop.DBus.Properties" => v.properties_call(&call),
                                "org.bluez.GattDescriptor1" => unimplemented!(),
                                "org.freedesktop.DBus.Introspectable" => v.introspectable(&call),
                                _ => unimplemented!(), // TODO: Added interface not found
                            },
                            GattObject::Ad(ad) => unimplemented!(),
                        },
                        None => standard_messages::unknown_method(&call),
                    };
                    // eprintln!("replying to: {:#?}\nreply: {:#?}", call, reply);
                    self.rpc_con.send_message(&mut reply, None)?;
                }
                Err(e) => match e {
                    client_conn::Error::TimedOut => return Ok(()),
                    _ => return Err(e.into()),
                },
            }
        }
    }
    fn match_root(&mut self, msg: &Message) -> Option<GattObject> {
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
            Some(GattObject::Appl)
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
    fn match_advertisement(&self, msg_path: &Path, msg: &Message) -> Option<GattObject> {
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
                Some(GattObject::Ad(ad))
            } else {
                None
            }
        } else {
            None
        }
    }
    fn match_service(&mut self, msg_path: &Path, msg: &Message) -> Option<GattObject> {
        eprintln!("Checking for service for match");
        let mut components = msg_path.components().take(2);
        if let Component::Normal(path) = components.next().unwrap() {
            let path = path.to_str()?;
            if !path.starts_with("service") {
                return None;
            }
            let mut service_str = String::new();
            for service in self.services.iter_mut() {
                service_str.clear();
                write!(&mut service_str, "service{:02X}/", service.index).unwrap();
                if let Ok(path) = msg_path.strip_prefix(&service_str) {
                    if let Some(_) = path.components().next() {
                        return service.match_chars(path, msg);
                    } else {
                        return Some(GattObject::Serv(service));
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
            "GetManagedObjects" => self.get_managed_object(msg),
            _ => standard_messages::unknown_method(&msg),
        }
    }
    fn object_manager_type() -> signature::Type {
        signature::Type::Container(signature::Container::Dict(
            signature::Base::String,
            Box::new(Service::get_all_type()),
        ))
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
                    let middle_cont: Container = (
                        signature::Base::String,
                        Descriptor::get_all_type(),
                        middle_map,
                    )
                        .try_into()
                        .unwrap();
                    outer_dict.insert(Base::ObjectPath(desc_path), middle_cont.into());
                }
                let mut middle_map = HashMap::new();
                for interface in Charactersitic::INTERFACES {
                    let props = characteristic.get_all_inner(interface.0).unwrap();
                    middle_map.insert(interface.0.to_string().into(), props);
                }
                let middle_cont: Container = (
                    signature::Base::String,
                    Charactersitic::get_all_type(),
                    middle_map,
                )
                    .try_into()
                    .unwrap();
                outer_dict.insert(Base::ObjectPath(char_path), middle_cont.into());
            }
            let mut middle_map = HashMap::new();

            for interface in Service::INTERFACES {
                let props = service.get_all_inner(interface.0).unwrap();
                middle_map.insert(interface.0.to_string().into(), props);
            }
            let middle_cont: Container =
                (signature::Base::String, Service::get_all_type(), middle_map)
                    .try_into()
                    .unwrap();
            outer_dict.insert(Base::ObjectPath(service_path), middle_cont.into());
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
impl<'a, 'b> Properties<'a, 'b> for Charactersitic {
    const INTERFACES: &'static [(&'static str, &'static [&'static str])] = &[CHAR_IF, PROP_IF];
    fn get_inner(&mut self, interface: &str, prop: &str) -> Option<Param<'a, 'b>> {
        // eprintln!("org.freedesktop.DBus.Charactersitic interface:\n{}, prop {}", interface, prop);
        match interface {
            CHAR_IF_STR => match prop {
                UUID_PROP => Some(base_param_to_variant(self.uuid.clone().into())),
                SERVICE_PROP => {
                    let pnt = self.path.len() - 9;
                    Some(base_param_to_variant(Base::ObjectPath(
                        self.path.split_at(pnt).0.to_string(),
                    )))
                }
                VALUE_PROP => {
                    let (v, l) = self.vf.to_value();
                    // eprintln!("vf: {:?}\nValue: {:?}", self.vf, &v[..l]);
                    let vec: Vec<Param> =
                        v[..l].into_iter().map(|i| Base::Byte(*i).into()).collect();
                    let val = Param::Container(Container::Array(params::Array {
                        element_sig: signature::Type::Base(signature::Base::Byte),
                        values: vec,
                    }));
                    let var = Box::new(params::Variant {
                        sig: signature::Type::Container(signature::Container::Array(Box::new(
                            signature::Type::Base(signature::Base::Byte),
                        ))),
                        value: val,
                    });
                    Some(Param::Container(Container::Variant(var)))
                }
                WRITE_ACQUIRED_PROP => {
                    Some(base_param_to_variant(Base::Boolean(self.write.is_some())))
                }
                NOTIFY_ACQUIRED_PROP => {
                    Some(base_param_to_variant(Base::Boolean(self.notify.is_some())))
                }
                NOTIFYING_PROP => Some(base_param_to_variant(Base::Boolean(self.notify.is_some()))),
                FLAGS_PROP => {
                    let flags = self.flags.to_strings();
                    let vec = flags.into_iter().map(|s| Base::String(s).into()).collect();
                    let val = Param::Container(Container::Array(params::Array {
                        element_sig: signature::Type::Base(signature::Base::String),
                        values: vec,
                    }));
                    let var = Box::new(params::Variant {
                        sig: signature::Type::Container(signature::Container::Array(Box::new(
                            signature::Type::Base(signature::Base::String),
                        ))),
                        value: val,
                    });
                    Some(Param::Container(Container::Variant(var)))
                }
                HANDLE_PROP => Some(base_param_to_variant(Base::Uint16(self.handle))),
                INCLUDES_PROP => None, // TODO: implement
                _ => None,
            },
            PROP_IF_STR => match prop {
                _ => None,
            },
            _ => None,
        }
    }
    fn set_inner(&mut self, interface: &str, prop: &str, val: &params::Variant) -> Option<String> {
        match interface {
            SERV_IF_STR => match prop {
                HANDLE_PROP => {
                    if let Param::Base(Base::Uint16(handle)) = val.value {
                        eprintln!("setting Handle prop: {:?}", val.value); // TODO remove
                        self.handle = handle;
                        None
                    } else {
                        Some("UnexpectedType".to_string())
                    }
                }
                _ => unimplemented!(),
            },
            PROP_IF_STR => Some("UnknownProperty".to_string()),
            _ => Some("UnknownInterface".to_string()),
        }
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
                    Some(base_param_to_variant(Base::ObjectPath(
                        self.path.split_at(pnt).0.to_string(),
                    )))
                }
                VALUE_PROP => unimplemented!(),
                FLAGS_PROP => unimplemented!(),
                HANDLE_PROP => Some(base_param_to_variant(self.index.into())),
                _ => None,
            },
            PROP_IF_STR => None,
            _ => None,
        }
    }
    fn set_inner(&mut self, interface: &str, prop: &str, val: &params::Variant) -> Option<String> {
        unimplemented!()
    }
    fn get_all(&mut self, msg: &Message<'a, 'b>) -> Message<'a, 'b> {
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
                    Some(base_param_to_variant(Base::ObjectPath(
                        self.path.split_at(pnt).0.to_string(),
                    )))
                }
                HANDLE_PROP => {
                    // eprintln!("Getting handle: {}", self.index);
                    Some(base_param_to_variant(Base::Uint16(self.handle)))
                }
                _ => None,
            },
            PROP_IF_STR => None,
            _ => None,
        }
    }
    fn set_inner(&mut self, interface: &str, prop: &str, val: &params::Variant) -> Option<String> {
        match interface {
            SERV_IF_STR => match prop {
                HANDLE_PROP => {
                    if let Param::Base(Base::Uint16(handle)) = val.value {
                        eprintln!("setting Handle prop: {:?}", val.value); // TODO remove
                        self.handle = handle;
                        None
                    } else {
                        Some("UnexpectedType".to_string())
                    }
                }
                _ => unimplemented!(),
            },
            PROP_IF_STR => Some("UnknownProperty".to_string()),
            _ => Some("UnknownInterface".to_string()),
        }
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
