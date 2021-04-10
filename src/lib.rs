use std::collections::HashMap;
use std::convert::{TryFrom, TryInto};
use std::fmt::{Debug, Display, Formatter};
use std::num::ParseIntError;
use std::str::FromStr;
use std::sync::Arc;

//use futures::future::{try_join_all, ready, join};
use async_rustbus::rustbus_core;
use async_rustbus::RpcConn;
use async_std::channel::{RecvError, SendError};
use futures::prelude::*;
use futures::stream::FuturesUnordered;

use rustbus_core::dbus_variant_var;
use rustbus_core::message_builder::{MarshalledMessage, MessageBuilder, MessageType};
use rustbus_core::path::{ObjectPath, ObjectPathBuf};
use rustbus_core::wire::marshal::traits::{Marshal, Signature};
use rustbus_core::wire::marshal::MarshalContext;
use rustbus_core::wire::unmarshal;
use unmarshal::traits::Unmarshal;
use unmarshal::{UnmarshalContext, UnmarshalResult};

pub mod advertising;
pub mod gatt;

use gatt::client::Service as LocalService;

pub(crate) mod introspect;
pub(crate) mod properties;

use introspect::get_children;

use interfaces::{get_prop_call, set_prop_call};

dbus_variant_var!(BluezOptions, Bool => bool;
                                U16 => u16;
                                Str => &'buf str;
                                OwnedStr => String;
                                Path => &'buf ObjectPath;
                                OwnedPath => ObjectPathBuf;
                                Buf => &'buf [u8];
                                OwnedBuf => Vec<u8>;
                                Flags => Vec<&'buf str>;
                                UUIDs => Vec<UUID>;
                                DataMap => HashMap<String, Vec<u8>>;
                                UUIDMap => HashMap<UUID, Vec<u8>>
);
#[derive(Debug, PartialEq, Eq, Copy, Clone, PartialOrd, Ord, Hash)]
pub struct UUID(pub u128);
#[derive(Debug, PartialEq, Eq, Clone, Copy, Hash)]
pub struct MAC(u32, u16);

pub const MAX_CHAR_LEN: usize = 512;
pub const MAX_APP_MTU: usize = 511;
const BLUEZ_DEV_IF: &'static str = "org.bluez.Device1";
const BLUEZ_SER_IF: &'static str = "org.bluez.GattService1";
const BLUEZ_CHR_IF: &'static str = "org.bluez.GattCharacteristic1";
const BLUEZ_DES_IF: &'static str = "org.bluez.GattDescriptor1";
const BLUEZ_MGR_IF: &'static str = "org.bluez.GattManager1";
const BLUEZ_ADV_IF: &'static str = "org.bluez.LEAdvertisement1";
const BLUEZ_ADP_IF: &'static str = "org.bluez.Adapter1";
const PROPS_IF: &'static str = "org.freedesktop.DBus.Properties";
const INTRO_IF: &'static str = "org.freedesktop.DBus.Introspectable";
const OBJMGR_IF: &'static str = "org.freedesktop.DBus.ObjectManager";
const BLUEZ_DEST: &'static str = "org.bluez";
const BT_BASE_UUID: UUID = UUID(0x0000000000001000800000805F9B34FB);

/// This trait creates a UUID from the implementing Type.
/// This trait can panic if the given type doesn't represent a valid uuid.
/// Only 128-bit uuids are supported at the moment.
/// ## Note
/// This trait exists because UUID and MAC will eventually be converted into
/// their own structs rather than being aliases for `Rc<str>`
#[derive(Debug)]
pub enum IDParseError {
    InvalidLength,
    InvalidDelimiter,
    InvalidRadix,
}
impl From<ParseIntError> for IDParseError {
    fn from(_: ParseIntError) -> Self {
        IDParseError::InvalidRadix
    }
}
impl FromStr for UUID {
    type Err = IDParseError;
    fn from_str(s: &str) -> Result<UUID, Self::Err> {
        if s.len() != 36 {
            return Err(IDParseError::InvalidLength);
        }
        let mut uuid_chars = s.chars();
        if uuid_chars.nth(8).unwrap() != '-' {
            return Err(IDParseError::InvalidDelimiter);
        }
        for _ in 0..3 {
            if uuid_chars.nth(4).unwrap() != '-' {
                return Err(IDParseError::InvalidDelimiter);
            }
        }
        let first = u128::from_str_radix(&s[..8], 16)? << 96;
        let second = u128::from_str_radix(&s[9..13], 16)? << 80;
        let third = u128::from_str_radix(&s[14..18], 16)? << 64;
        let fourth = u128::from_str_radix(&s[19..23], 16)? << 48;
        let fifth = u128::from_str_radix(&s[24..36], 16)? << 0;
        Ok(UUID(first | second | third | fourth | fifth))
    }
}
impl From<u16> for UUID {
    fn from(u: u16) -> Self {
        Self::from(u as u32)
    }
}
impl From<u32> for UUID {
    fn from(u: u32) -> Self {
        UUID(((u as u128) << 96) | BT_BASE_UUID.0)
    }
}
impl From<UUID> for u128 {
    fn from(u: UUID) -> Self {
        u.0
    }
}
impl From<u128> for UUID {
    fn from(u: u128) -> Self {
        Self(u)
    }
}
pub struct OutOfRange;

impl TryFrom<UUID> for u16 {
    type Error = OutOfRange;
    fn try_from(uuid: UUID) -> Result<Self, Self::Error> {
        uuid.short().ok_or(OutOfRange)
    }
}
impl TryFrom<UUID> for u32 {
    type Error = OutOfRange;
    fn try_from(uuid: UUID) -> Result<Self, Self::Error> {
        uuid.uint().ok_or(OutOfRange)
    }
}
impl UUID {
    pub const fn short(&self) -> Option<u16> {
        const U16_MAX_UUID: u128 = BT_BASE_UUID.0 | ((std::u16::MAX as u128) << 112);
        if (self.0 & !U16_MAX_UUID) == 0 {
            Some((self.0 >> 96) as u16)
        } else {
            None
        }
    }
    pub const fn uint(&self) -> Option<u32> {
        const U32_MAX_UUID: u128 = BT_BASE_UUID.0 | ((std::u32::MAX as u128) << 96);
        if (self.0 & !U32_MAX_UUID) == 0 {
            Some((self.0 >> 96) as u32)
        } else {
            None
        }
    }
}
impl Display for UUID {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let first = (self.0 >> 96) as u32;
        let second = (self.0 >> 80) as u16;
        let third = (self.0 >> 64) as u16;
        let fourth = (self.0 >> 48) as u16;
        let fifth = self.0 as u64 & 0xFFFFFFFFFFFF;
        write!(
            f,
            "{:08x}-{:04x}-{:04x}-{:04x}-{:012x}",
            first, second, third, fourth, fifth
        )
    }
}
impl FromStr for MAC {
    type Err = IDParseError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.len() != 17 {
            return Err(IDParseError::InvalidLength);
        }
        for c in s.chars().skip(2).step_by(3) {
            if c != ':' {
                return Err(IDParseError::InvalidDelimiter);
            }
        }
        let zero = [
            u8::from_str_radix(&s[0..2], 16)?,
            u8::from_str_radix(&s[3..5], 16)?,
            u8::from_str_radix(&s[6..8], 16)?,
            u8::from_str_radix(&s[9..11], 16)?,
        ];

        let one = [
            u8::from_str_radix(&s[12..14], 16)?,
            u8::from_str_radix(&s[15..17], 16)?,
        ];
        Ok(MAC(u32::from_be_bytes(zero), u16::from_be_bytes(one)))
    }
}
impl Signature for UUID {
    fn signature() -> rustbus_core::signature::Type {
        String::signature()
    }
    fn alignment() -> usize {
        String::alignment()
    }
}
impl Marshal for UUID {
    fn marshal(&self, ctx: &mut MarshalContext) -> Result<(), rustbus_core::Error> {
        let s = self.to_string();
        String::marshal(&s, ctx)
    }
}
impl<'buf> Unmarshal<'buf, '_> for UUID {
    fn unmarshal(ctx: &mut UnmarshalContext<'_, 'buf>) -> UnmarshalResult<Self> {
        let (used, s): (usize, &str) = Unmarshal::unmarshal(ctx)?;
        let uuid = UUID::from_str(s).map_err(|_| unmarshal::Error::InvalidType)?;
        Ok((used, uuid))
    }
}
impl MAC {
    pub const fn new(mac: u64) -> Self {
        //assert!(mac < 0xFFFF000000000000);
        let a = [0];
        // hacky way to panic if the value is out of range in const_fn
        a[((0xFFFF000000000000 & mac) >> 32) as usize];
        Self((mac >> 16) as u32, mac as u16)
    }
    pub const fn from_parts(first_four: u32, last_two: u16) -> Self {
        MAC(first_four, last_two)
    }
    fn from_dev_str(child: &str) -> Option<Self> {
        if !child.starts_with("dev_") {
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
            "dev_{:02X}_{:02X}_{:02X}_{:02X}_{:02X}_{:02X}",
            m0[0], m0[1], m0[2], m0[3], m1[0], m1[1]
        )
    }
}

impl Display for MAC {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let m0 = self.0.to_be_bytes();
        let m1 = self.1.to_be_bytes();
        write!(
            f,
            "{:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}",
            m0[0], m0[1], m0[2], m0[3], m1[0], m1[1]
        )
    }
}
impl From<(u32, u16)> for MAC {
    fn from(pair: (u32, u16)) -> Self {
        MAC(pair.0, pair.1)
    }
}

#[derive(Debug)]
pub enum Error {
    Bluez(String),
    Dbus(String),
    ThreadClosed,
    SocketHungUp,
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
            | Error::SocketHungUp
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
fn is_msg_err<'buf, T>(msg: &'buf MarshalledMessage) -> Result<T, Error>
where
    T: Unmarshal<'buf, 'buf>,
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

fn is_msg_err2<'buf, T, U>(msg: &'buf MarshalledMessage) -> Result<(T, U), Error>
where
    T: Unmarshal<'buf, 'buf>,
    U: Unmarshal<'buf, 'buf>,
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
        <Self as Debug>::fmt(self, f)
    }
}
impl From<std::io::Error> for Error {
    fn from(err: std::io::Error) -> Self {
        Error::Io(err)
    }
}

/// `Bluetooth` is created to interact with Bluez over DBus and file descriptors.
pub struct BluetoothService {
    conn: Arc<RpcConn>,
}

impl BluetoothService {
    pub async fn from_conn(conn: Arc<RpcConn>) -> Result<Self, Error> {
        let ret = Self { conn };
        let path: &ObjectPath = ObjectPath::new("/org/bluez").unwrap();
        ret.get_children(path).await?; // validate that function is working
        Ok(ret)
    }
    /// Creates a new `Bluetooth` and setup a DBus client to interact with Bluez.
    pub async fn new() -> Result<Self, Error> {
        let conn = RpcConn::system_conn(true).await?;
        Self::from_conn(Arc::new(conn)).await
    }
    async fn get_children<P: AsRef<ObjectPath>>(&self, path: P) -> Result<Vec<u8>, Error> {
        let ret = get_children(&self.conn, BLUEZ_DEST, path)
            .await?
            .into_iter()
            .filter_map(|child| {
                let name = child.file_name()?;
                if !name.starts_with("hci") {
                    return None;
                }
                u8::from_str(name.get(3..)?).ok()
            })
            .collect();
        Ok(ret)
    }
    pub async fn get_adapter(&self, idx: u8) -> Result<Adapter, Error> {
        Adapter::from_conn(self.conn.clone(), idx).await
    }
    pub fn get_conn(&self) -> &Arc<RpcConn> {
        &self.conn
    }
}
pub struct Adapter {
    conn: Arc<RpcConn>,
    path: ObjectPathBuf,
}
impl Adapter {
    pub async fn from_conn(conn: Arc<RpcConn>, idx: u8) -> Result<Adapter, Error> {
        let path = format!("/org/bluez/hci{}", idx);
        let ret = Adapter {
            path: path.try_into().unwrap(),
            conn,
        };
        ret.addr().await?;
        Ok(ret)
    }
    pub async fn new(idx: u8) -> Result<Adapter, Error> {
        let conn = RpcConn::system_conn(true).await?;
        Self::from_conn(Arc::new(conn), idx).await
    }
    /// Get the `MAC` of the local adapter.
    pub async fn addr(&self) -> Result<MAC, Error> {
        let call = get_prop_call(self.path.clone(), BLUEZ_DEST, BLUEZ_ADP_IF, "Address");
        let res = self.conn.send_msg_with_reply(&call).await?.await?;
        let var: BluezOptions = is_msg_err(&res)?;
        let addr_str = match var {
            BluezOptions::Str(s) => s,
            _ => {
                return Err(Error::Bluez(
                    "Received invalid type for address!".to_string(),
                ))
            }
        };
        MAC::from_str(addr_str)
            .map_err(|e| Error::Bluez(format!("Invalid address received: {} ({:?})", addr_str, e)))
    }
    pub async fn set_powered(&self, powered: bool) -> Result<(), Error> {
        let call = set_prop_call(
            self.path.clone(),
            BLUEZ_DEST,
            BLUEZ_ADP_IF,
            "Powered",
            BluezOptions::Bool(powered),
        );
        let res = self.conn.send_msg_with_reply(&call).await?.await?;
        is_msg_err_empty(&res)
    }
    pub fn path(&self) -> &ObjectPath {
        &self.path
    }
    pub async fn get_devices(&self) -> Result<Vec<MAC>, Error> {
        let ret = get_children(&self.conn, BLUEZ_DEST, &self.path)
            .await?
            .into_iter()
            .filter_map(|child| MAC::from_dev_str(child.file_name()?))
            .collect();
        Ok(ret)
    }
    pub async fn get_device(&self, mac: MAC) -> Result<Device, Error> {
        let dev_str = mac.to_dev_str();
        let path = format!("{}/{}", self.path, dev_str);
        let call = get_prop_call(&path, "org.bluez", BLUEZ_DEV_IF, "Address");
        let msg = self.conn.send_msg_with_reply(&call).await?.await?;
        let res_var: BluezOptions = is_msg_err(&msg)?;
        let res_mac = match res_var {
            BluezOptions::Str(mac) => MAC::from_str(mac).map_err(|_| 
                Error::Bluez(format!("Invalid MAC received back: {}", mac)))?,
            _ => return Err(Error::Bluez(format!("Invalid type received for MAC.")))
        };
        if res_mac != mac {
            return Err(Error::Bluez(format!(
                "Address returned {} did not match given ({})!",
                res_mac, mac
            )));
        }
        Ok(Device {
            conn: self.conn.clone(),
            path: ObjectPathBuf::try_from(path).unwrap(),
        })
    }
    pub fn conn(&self) -> &Arc<RpcConn> {
        &self.conn
    }
}

pub struct Device {
    conn: Arc<RpcConn>,
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
        let children: FuturesUnordered<_> = get_children(&self.conn, BLUEZ_DEST, &self.path)
            .await?
            .into_iter()
            .map(|child| LocalService::get_service(self.conn.clone(), child))
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
    fn prop_if_call(path: String, dest: String, interface: &str, prop: &str) -> MarshalledMessage {
        let mut call = MessageBuilder::new()
            .call(String::new())
            .with_interface(PROPS_IF)
            .on(path)
            .at(dest)
            .build();
        call.body.push_param(interface).unwrap();
        call.body.push_param(prop).unwrap();
        call
    }
    pub fn set_prop_call<P, D>(
        path: P,
        dest: D,
        interface: &str,
        prop: &str,
        val: BluezOptions<'_, '_>,
    ) -> MarshalledMessage
    where
        P: Into<String>,
        D: Into<String>,
    {
        let mut msg = prop_if_call(path.into(), dest.into(), interface, prop);
        msg.dynheader.member = Some("Set".to_string());
        msg.body.push_param(val).unwrap();
        msg
    }
    pub fn get_prop_call<P, D>(path: P, dest: D, interface: &str, prop: &str) -> MarshalledMessage
    where
        P: Into<String>,
        D: Into<String>,
    {
        let mut msg = prop_if_call(path.into(), dest.into(), interface, prop);
        msg.dynheader.member = Some("Get".to_string());
        msg
    }
}

#[cfg(test)]
mod tests {
    use super::{MAC, UUID};
    use rand::Rng;
    use std::str::FromStr;
    #[test]
    fn mac_to_string() {
        let mut rng = rand::thread_rng();
        for _ in 0..128 {
            let mac = MAC::from_parts(rng.gen(), rng.gen());
            let mac_str = mac.to_string();
            assert_eq!(mac, MAC::from_str(&mac_str).expect(&mac_str));
        }
    }
    #[test]
    fn mac_dev_string() {
        let mut rng = rand::thread_rng();
        for _ in 0..128 {
            let mac = MAC::from_parts(rng.gen(), rng.gen());
            let dev_str = mac.to_dev_str();
            assert_eq!(mac, MAC::from_dev_str(&dev_str).expect(&dev_str));
        }
    }
    #[test]
    fn uuid_to_string() {
        let mut rng = rand::thread_rng();
        for _ in 0..128 {
            let uuid = UUID(rng.gen());
            let uuid_str = uuid.to_string();
            assert_eq!(uuid, UUID::from_str(&uuid_str).expect(&uuid_str));
        }
    }
}
/*
*/
