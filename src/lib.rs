use std::convert::{TryFrom, TryInto};
use std::fmt::{Display, Formatter};
use std::num::ParseIntError;
use std::str::FromStr;
use std::sync::Arc;

//use futures::future::{try_join_all, ready, join};
use async_rustbus::rustbus_core;
use async_rustbus::RpcConn;
use async_std::channel::{RecvError, SendError};
use futures::prelude::*;
use futures::stream::FuturesUnordered;

use rustbus_core::message_builder::{CallBuilder, MarshalledMessage, MessageBuilder, MessageType};
use rustbus_core::wire::unmarshal::traits::Unmarshal;

pub mod gatt;

use gatt::client::Service as LocalService;

pub(crate) mod introspect;

mod path;
pub use path::*;

use interfaces::get_prop_call;

#[derive(Debug, PartialEq, Eq, Copy, Clone, PartialOrd, Ord)]
pub struct UUID(u128);
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub struct MAC(u32, u16);

pub const MAX_CHAR_LEN: usize = 512;
pub const MAX_APP_MTU: usize = 511;
const BLUEZ_DEV_IF: &'static str = "org.bluez.Device1";
const BLUEZ_SER_IF: &'static str = "org.bluez.Service1";
const BLUEZ_CHR_IF: &'static str = "org.bluez.Characteristic1";
const BLUEZ_DES_IF: &'static str = "org.bluez.Descriptor1";
const BLUEZ_MGR_IF: &'static str = "org.bluez.GattManager1";
const PROPS_IF: &'static str = "org.freedesktop.DBus.Properties";
const INTRO_IF: &'static str = "org.freedesktop.DBus.Introspectable";
const OBJMGR_IF: &'static str = "org.freedesktop.DBus.ObjectManager";

/// This trait creates a UUID from the implementing Type.
/// This trait can panic if the given type doesn't represent a valid uuid.
/// Only 128-bit uuids are supported at the moment.
/// ## Note
/// This trait exists because UUID and MAC will eventually be converted into
/// their own structs rather than being aliases for `Rc<str>`
pub enum UUIDParseError {
    InvalidLength,
    InvalidDelimiter,
    InvalidRadix
}
impl From<ParseIntError> for UUIDParseError {
    fn from(_: ParseIntError) -> Self {
        UUIDParseError::InvalidRadix
    }
}
impl FromStr for UUID {
    type Err = UUIDParseError;
    fn from_str(s: &str) -> Result<UUID, Self::Err> {
        if s.len() != 36 {
            return Err(UUIDParseError::InvalidLength);
        }
        let mut uuid_chars = s.chars();
        if uuid_chars.nth(8).unwrap() != '-' {
            return Err(UUIDParseError::InvalidDelimiter);
        }
        for _ in 0..3 {
            if uuid_chars.nth(4).unwrap() != '-' {
                return Err(UUIDParseError::InvalidDelimiter);
            }
        }
        let first = u128::from_str_radix(&s[..8], 16)? << 96;
        let second = u128::from_str_radix(&s[9..13], 16)? << 80;
        let third = u128::from_str_radix(&s[13..18], 16)? << 64;
        let fourth = u128::from_str_radix(&s[18..23], 16)? << 48;
        let fifth = u128::from_str_radix(&s[24..36], 16)? << 0;
        Ok(UUID(first | second | third | fourth | fifth))

    }
}
impl From<u16> for UUID {
    fn from(u: u16) -> Self {
        unimplemented!()
    }
}
impl From<u128> for UUID {
    fn from(u: u128) -> Self {
        Self(u)
    }
}
impl UUID {
    fn chars(&self) -> [char; 32] {
        unimplemented!();
    }
    fn short(&self) -> Option<[char; 4]> {
        unimplemented!();
    }
}
impl Display for UUID {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        unimplemented!()
    }
}
impl MAC {
    fn from_dev_str(child: &str) -> Option<Self> {
        if !child.starts_with("dev") {
            return None;
        }
        let addr_str = child.get(3..)?;
        let mut mac = [0; 6];
        for i in 0..6 {
            let s = addr_str.get(i * 3 + 1..i * 3 + 3)?;
            mac[i] = u8::from_str_radix(s, 16).ok()?;
        }
        let mac_0 = [mac[0], mac[1], mac[2], mac[3]];
        let mac_1 = [mac[4], mac[5]];
        Some(MAC(u32::from_be_bytes(mac_0), u16::from_be_bytes(mac_1)))
    }
    fn to_dev_str(&self) -> String {
        let m0 = self.0.to_be_bytes();
        let m1 = self.1.to_be_bytes();
        format!(
            "dev_{:X}_{:X}_{:X}_{:X}_{:X}_{:X}",
            m0[0], m0[1], m0[2], m0[3], m1[0], m1[1]
        )
    }
}
impl FromStr for MAC {
    type Err = ();
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        unimplemented!();
    }
}

impl Display for MAC {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let m0 = self.0.to_be_bytes();
        let m1 = self.1.to_be_bytes();
        write!(
            f,
            "{:X}:{:X}:{:X}:{:X}:{:X}:{:X}",
            m0[0], m0[1], m0[2], m0[3], m1[0], m1[1]
        )
    }
}
/// Checks if a string is valid MAC.
/// Currently MAC address must be uppercase and use ':' as the seperator.
/// These former requirement will be removed in the future, and '_' will also be accepted as a seperator.
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

#[derive(Debug)]
pub enum Error {
    Bluez(String),
    Dbus(String),
    ThreadClosed,
    UnknownServ(UUID),
    UnknownChar(UUID, UUID),
    UnknownDesc(UUID, UUID, UUID),
    Io(std::io::Error),
}
impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Bluez(_)
            | Error::Dbus(_)
            | Error::ThreadClosed
            | Error::UnknownServ(_)
            | Error::UnknownChar(_, _)
            | Error::UnknownDesc(_, _, _) => None,
            Error::Io(e) => Some(e),
        }
    }
}
impl<T> From<SendError<T>> for Error {
    fn from(_err: SendError<T>) -> Self {
        Error::ThreadClosed
    }
}
impl From<RecvError> for Error {
    fn from(_err: RecvError) -> Self {
        Error::ThreadClosed
    }
}
fn is_msg_err<'r, 'buf: 'r, T>(msg: &'buf MarshalledMessage) -> Result<T, Error>
where
    T: Unmarshal<'r, 'buf, 'buf>,
{
    match msg.typ {
        MessageType::Reply => msg
            .body
            .parser()
            .get()
            .map_err(|e| Error::Dbus(format!("Failed to unmarshal: {:?}", e))),
        MessageType::Error => {
            let err_msg: String = msg
                .body
                .parser()
                .get()
                .map_err(|e| Error::Dbus(format!("Failed to unmarshal msg_err: {:?}", e)))?;
            if err_msg.starts_with("org.bluez") {
                return Err(Error::Bluez(err_msg));
            }
            Err(Error::Dbus(err_msg))
        }
        _ => unreachable!(),
    }
}

fn is_msg_err2<'r, 'buf: 'r, T, U>(msg: &'buf MarshalledMessage) -> Result<(T, U), Error>
where
    T: Unmarshal<'r, 'buf, 'buf>,
    U: Unmarshal<'r, 'buf, 'buf>,
{
    match msg.typ {
        MessageType::Reply => msg
            .body
            .parser()
            .get2()
            .map_err(|e| Error::Dbus(format!("Failed to unmarshal: {:?}", e))),
        MessageType::Error => {
            let err_msg: String = msg
                .body
                .parser()
                .get()
                .map_err(|e| Error::Dbus(format!("Failed to unmarshal msg_err: {:?}", e)))?;
            if err_msg.starts_with("org.bluez") {
                return Err(Error::Bluez(err_msg));
            }
            Err(Error::Dbus(err_msg))
        }
        _ => unreachable!(),
    }
}

fn is_msg_err_empty(msg: &MarshalledMessage) -> Result<(), Error> {
    match msg.typ {
        MessageType::Reply => Ok(()),
        MessageType::Error => {
            let err_msg: String = msg
                .body
                .parser()
                .get()
                .map_err(|e| Error::Dbus(format!("Failed to unmarshal msg_err: {:?}", e)))?;
            if err_msg.starts_with("org.bluez") {
                return Err(Error::Bluez(err_msg));
            }
            Err(Error::Dbus(err_msg))
        }
        _ => unreachable!(),
    }
}
impl Display for Error {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        unimplemented!()
    }
}
impl From<std::io::Error> for Error {
    fn from(err: std::io::Error) -> Self {
        Error::Io(err)
    }
}

struct ServiceData {
    conn: RpcConn,
    service_dest: String,
}
impl ServiceData {
    fn build_service_call(&self) -> CallBuilder {
        MessageBuilder::new()
            .call(String::new())
            .at(self.service_dest.clone())
            .on(String::from("/org/bluez"))
    }
    fn path_str(&self) -> &str {
        self.service_dest.as_ref()
    }
}
/// `Bluetooth` is created to interact with Bluez over DBus and file descriptors.
pub struct BluetoothService {
    inner: Arc<ServiceData>,
}

impl BluetoothService {
    /// Creates a new `Bluetooth` and setup a DBus client to interact with Bluez.
    pub async fn new() -> Result<Self, Error> {
        let conn = RpcConn::system_conn(true).await?;
        let ret = Self {
            inner: Arc::new(ServiceData {
                conn,
                service_dest: String::from("org.bluez"),
            }),
        };
        let path: &ObjectPath = ObjectPath::new("/org/bluez").unwrap();
        ret.get_children(path).await?;
        Ok(ret)
    }
    async fn get_children<P: AsRef<ObjectPath>>(&self, path: P) -> Result<Vec<u8>, Error> {
        let ret = get_children(&self.inner.conn, &self.inner.service_dest, path)
            .await?
            .into_iter()
            .filter_map(|child| {
                let name = child.path().file_name()?;
                if !name.starts_with("hci") {
                    return None;
                }
                u8::from_str(name.get(3..)?).ok()
            })
            .collect();
        Ok(ret)
    }
    async fn get_adapter(&self, idx: u8) -> Result<Adapter, Error> {
        let path = format!("/org/bluez/hci{}", idx);
        let ret = Adapter {
            inner: self.inner.clone(),
            path: path.try_into().unwrap(),
        };
        ret.addr().await?;
        Ok(ret)
    }
}
pub struct Adapter {
    inner: Arc<ServiceData>,
    path: ObjectPathBuf,
}
impl Adapter {
    /// Get the `MAC` of the local adapter.
    pub async fn addr(&self) -> Result<MAC, Error> {
        unimplemented!()
    }
    /// Gets the path of the adapter
    fn path(&self) -> &ObjectPath {
        &self.path
    }
    pub async fn get_devices(&self) -> Result<Vec<MAC>, Error> {
        let ret = get_children(&self.inner.conn, &self.inner.service_dest[..], &self.path)
            .await?
            .into_iter()
            .filter_map(|child| MAC::from_dev_str(child.path().as_ref()))
            .collect();
        Ok(ret)
    }
    pub async fn get_device(&self, mac: MAC) -> Result<Device, Error> {
        let dev_str = mac.to_dev_str();
        let path = format!("{}/{}", self.inner.path_str(), dev_str);
        let call = get_prop_call(&path, "org.bluez", BLUEZ_DEV_IF, "Address");
        let msg = self.inner.conn.send_message(&call).await?.await?.unwrap();
        let res_addr: &str = is_msg_err(&msg)?;
        let res_mac = MAC::from_str(res_addr)
            .map_err(|_| Error::Bluez(format!("Invalid MAC received back: {}", res_addr)))?;
        if res_mac != mac {
            return Err(Error::Bluez(format!(
                "Address returned {} did not match given ({})!",
                res_mac, mac
            )));
        }
        Ok(Device {
            inner: self.inner.clone(),
            path: ObjectPathBuf::try_from(path).unwrap(),
        })
    }
}

pub struct Device {
    inner: Arc<ServiceData>,
    path: ObjectPathBuf,
}
impl Device {
    pub async fn get_services(&self) -> Result<Vec<LocalService>, Error> {
        let services = self.get_services_stream().await?;
        let fut = |s: Option<LocalService>| async move { Ok(s) };
        let ret = services.try_filter_map(fut).try_collect().await?;
        Ok(ret)
    }
    async fn get_services_stream(
        &self,
    ) -> Result<
        FuturesUnordered<impl Future<Output = Result<Option<LocalService>, Error>> + '_>,
        Error,
    >
//-> Result<impl TryStream<Ok=Option<LocalService>, Error=Error> +'_, Error>
    {
        let children: FuturesUnordered<_> =
            get_children(&self.inner.conn, &self.inner.service_dest[..], &self.path)
                .await?
                .into_iter()
                .map(|child| LocalService::get_service(&self.inner, child))
                .collect();

        Ok(children)
    }
    pub async fn get_service(&self, uuid: UUID) -> Result<Option<gatt::client::Service>, Error> {
        let mut services = self.get_services_stream().await?;
        while let Some(res) = services.next().await {
            if let Some(service) = res? {
                if service.uuid() == uuid {
                    return Ok(Some(service));
                }
            }
        }
        Ok(None)
    }
}

/*
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
*/
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

mod interfaces {
    use super::*;
    pub fn get_prop_call<P, D>(path: P, dest: D, interface: &str, prop: &str) -> MarshalledMessage
    where
        P: Into<String>,
        D: Into<String>,
    {
        let mut call = MessageBuilder::new()
            .call("Get".to_string())
            .with_interface(PROPS_IF.to_string())
            .on(path.into())
            .at(dest.into())
            .build();
        call.body.push_param(interface).unwrap();
        call.body.push_param(prop).unwrap();
        call
    }
}
/*
*/
