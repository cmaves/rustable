use crate::interfaces::*;
use crate::introspect::*;
use crate::*;
use nix::sys::socket;
use nix::sys::time::{TimeVal, TimeValLike};
use nix::sys::uio::IoVec;
use nix::unistd::close;
use rustbus::params::message::Message;
use rustbus::params::{Base, Container, Param};
use std::convert::TryFrom;
use std::os::unix::io::RawFd;

pub trait Characteristic {
    fn read(&mut self) -> Result<([u8; 255], usize), Error>;
    fn read_value(&mut self) -> Result<([u8; 255], usize), Error>;
    fn write(&mut self, val: &[u8]) -> Result<(), Error>;
    fn uuid(&self) -> &str;
    //    fn service(&self) -> &Path;
    fn write_acquired(&self) -> bool;
    fn notify_acquired(&self) -> bool;
    fn notifying(&self) -> bool;
    fn flags(&self) -> CharFlags;
}

#[derive(Debug)]
enum Notify {
    Signal,
    Fd(RawFd),
}
#[derive(Debug)]
pub struct LocalCharBase {
    vf: ValOrFn,
    pub(crate) index: u16,
    handle: u16,
    pub(super) uuid: UUID,
    pub(crate) path: PathBuf,
    notify: Option<Notify>,
    write: Option<RawFd>,
    pub(crate) descs: HashMap<String, LocalDescriptor>,
    flags: CharFlags,
}
impl Drop for LocalCharBase {
    fn drop(&mut self) {
        if let Some(Notify::Fd(fd)) = self.notify {
            close(fd).ok(); // ignore error
        }
        if let Some(fd) = self.write {
            close(fd).ok();
        }
    }
}
impl LocalCharBase {
    pub(super) fn update_path(&mut self, base: &Path) {
        self.path = base.to_owned();
        let mut name = String::with_capacity(8);
        write!(&mut name, "char{:04x}", self.index).unwrap();
        self.path.push(name);
        for desc in self.descs.values_mut() {
            desc.update_path(&self.path);
        }
    }
    pub(crate) fn char_call<'a, 'b>(&mut self, call: MarshalledMessage) -> MarshalledMessage {
        let call = call.unmarshall_all().unwrap();
        if let Some(member) = &call.dynheader.member {
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
                                            return call.dynheader.make_error_response("UnexpectedType".to_string(), Some("Expected a dict of variants as first parameter".to_string()));
                                        }
                                    } else {
                                        return call.dynheader.make_error_response(
                                            "UnexpectedType".to_string(),
                                            Some(
                                                "Expected a dict of variants as first parameter"
                                                    .to_string(),
                                            ),
                                        );
                                    }
                                }
                            } else {
                                return call.dynheader.make_error_response(
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
                        res.body.push_old_param(&val).unwrap();
                        res
                    } else {
                        call.dynheader.make_error_response(
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
                        call.dynheader.make_error_response(
                            BLUEZ_NOT_PERM.to_string(),
                            Some("This is not a writable characteristic.".to_string()),
                        )
                    }
                }
                "AcquireWrite" => {
                    match socket::socketpair(
                        socket::AddressFamily::Unix,
                        socket::SockType::SeqPacket,
                        None,
                        socket::SockFlag::SOCK_CLOEXEC,
                    ) {
                        Ok((sock1, _sock2)) => {
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
                                                return call.dynheader.make_error_response("UnexpectedType".to_string(), Some("Expected a dict of UInt16 as first offset type".to_string()));
                                            }
                                        } else {
                                            return call.dynheader.make_error_response("UnexpectedType".to_string(), Some("Expected a dict of variants as first parameter".to_string()));
                                        }
                                    }
                                } else {
                                    return call.dynheader.make_error_response(
                                        "UnexpectedType".to_string(),
                                        Some("Expected a dict as first parameter".to_string()),
                                    );
                                }
                            }
                            let mut res = call.make_response();
                            res.body
                                .push_old_params(&[
                                    Param::Base(Base::Uint32(sock1 as u32)),
                                    Param::Base(Base::Uint16(ret)),
                                ])
                                .unwrap();
                            unimplemented!();
                            return res;
                        }
                        Err(_) => {
                            return call.dynheader.make_error_response(
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
                        call.dynheader.make_error_response(
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
                        call.dynheader.make_error_response(
                            "org.bluez.Error.InProgress".to_string(),
                            Some(err_str.to_string()),
                        )
                    } else {
                        match socket::socketpair(
                            socket::AddressFamily::Unix,
                            socket::SockType::SeqPacket,
                            None,
                            socket::SockFlag::SOCK_CLOEXEC,
                        ) {
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
                                                    return call.dynheader.make_error_response("UnexpectedType".to_string(), Some("Expected a dict of UInt16 as first offset type".to_string()));
                                                }
                                            } else {
                                                return call.dynheader.make_error_response("UnexpectedType".to_string(), Some("Expected a dict of variants as first parameter".to_string()));
                                            }
                                        }
                                    } else {
                                        return call.dynheader.make_error_response(
                                            "UnexpectedType".to_string(),
                                            Some("Expected a dict as first parameter".to_string()),
                                        );
                                    }
                                }
                                let mut res = call.make_response();
                                res.body
                                    .push_old_params(&[
                                        Param::Base(Base::UnixFd(0)),
                                        Param::Base(Base::Uint16(ret)),
                                    ])
                                    .unwrap();
                                res.dynheader.num_fds = Some(1);
                                res.raw_fds.push(sock1);
                                self.notify = Some(Notify::Fd(sock2));
                                res
                            }
                            Err(_) => call.dynheader.make_error_response(
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
                        call.dynheader.make_error_response(
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
                        call.dynheader.make_error_response(
                            "org.bluez.Error.InProgress".to_string(),
                            Some(err_str.to_string()),
                        )
                    } else {
                        self.notify = Some(Notify::Signal);
                        call.make_response()
                    }
                }
                "StopNotify" => {
                    if let Some(_) = self.notify.as_ref() {
                        self.notify = None;
                        call.make_response()
                    } else {
                        call.dynheader.make_error_response(
                            BLUEZ_FAILED.to_string(),
                            Some("Notify has not been started".to_string()),
                        )
                    }
                }
                "Confirm" => call.make_response(),
                _ => call
                    .dynheader
                    .make_error_response(UNKNOWN_METHOD.to_string(), None),
            }
        } else {
            // TODO: remove this statement if unneeded
            unreachable!();
        }
    }

    pub(super) fn match_descs(
        &mut self,
        _msg_path: &Path,
        _header: &DynamicHeader,
    ) -> Option<DbusObject> {
        unimplemented!()
    }
    pub fn new<T: ToUUID>(uuid: T, flags: CharFlags) -> Self {
        let uuid: UUID = uuid.to_uuid();
        LocalCharBase {
            vf: ValOrFn::Value([0; 255], 0),
            index: 0,
            handle: 0,
            write: None,
            notify: None,
            uuid,
            flags,
            path: PathBuf::new(),
            descs: HashMap::new(),
        }
    }
}

pub struct LocalCharactersitic<'a, 'b: 'a> {
    pub(super) uuid: UUID,
    pub(super) service: &'a mut LocalService<'b>,
    #[cfg(feature = "unsafe-opt")]
    base: *mut LocalCharBase,
}
impl LocalCharactersitic<'_, '_> {
    pub fn write_val_or_fn(&mut self, val: &mut ValOrFn) {
        let base = self.get_char_base_mut();
        std::mem::swap(&mut base.vf, val);
    }
    fn signal_change(&mut self) -> Result<(), Error> {
        let base = self.get_char_base_mut();
        //let (v, l) = self.get_char_base_mut().vf.to_value();
        let (v, l) = base.vf.to_value();
        let value = &v[..l];
        let mut params = Vec::with_capacity(3); // TODO: eliminate this allocations
        params.push(Param::Base(Base::String(CHAR_IF_STR.to_string())));
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
        changed_map.insert(Base::String(VALUE_PROP.to_string()), changed_param);
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
        let base = self.get_char_base_mut();
        let mut msg = MessageBuilder::new()
            .signal(
                PROP_IF.0.to_string(),
                PROP_CHANGED_SIG.to_string(),
                base.path.to_str().unwrap().to_string(),
            )
            .build();
        msg.body.push_old_params(&params).unwrap();
        // eprintln!("msg to be send: {:#?}", msg);
        self.service
            .bt
            .rpc_con
            .send_message(&mut msg, Timeout::Infinite)?;
        Ok(())
    }
    pub fn notify(&mut self) -> Result<(), Error> {
        let base = self.get_char_base_mut();
        let (buf, len) = base.vf.to_value();
        if let Some(notify) = &mut base.notify {
            match notify {
                Notify::Signal => self.signal_change()?,
                Notify::Fd(sock) => {
                    if let Err(_) = socket::send(*sock, &buf[..len], socket::MsgFlags::MSG_EOR) {
                        base.notify = None;
                    }
                }
            }
        }
        Ok(())
    }
    fn get_char_base_mut(&mut self) -> &mut LocalCharBase {
        self.service
            .get_service_mut()
            .chars
            .get_mut(&self.uuid)
            .unwrap()
    }
    fn get_char_base(&self) -> &LocalCharBase {
        &self.service.get_service().chars[&self.uuid]
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
    fn from_strings(flags: &[String]) -> CharFlags {
        let mut ret = CharFlags::default();
        for flag in flags {
            match flag.as_str() {
                "broadcast" => ret.broadcast = true,
                "read" => ret.read = true,
                "write" => ret.write = true,
                "write-without-response" => ret.write_wo_response = true,
                "notify" => ret.notify = true,
                "indicate" => ret.indicate = true,
                "authenticated-signed-writes" => ret.auth_signed_writes = true,
                "extended-properties" => ret.extended_properties = true,
                "reliable-write" => ret.reliable_write = true,
                "writable-auxiliaries" => ret.writable_auxiliaries = true,
                "encrypt-read" => ret.encrypt_read = true,
                "encrypt-write" => ret.encrypt_write = true,
                "encrypt-authenticated-read" => ret.encrypt_auth_read = true,
                "encrypt-authenticated-write" => ret.encrypt_auth_write = true,
                "secure-write" => ret.secure_write = true,
                "secure-read" => ret.secure_read = true,
                "authorize" => ret.authorize = true,
                _ => unimplemented!(),
            }
        }
        ret
    }
}

impl Characteristic for LocalCharactersitic<'_, '_> {
    fn read(&mut self) -> Result<([u8; 255], usize), Error> {
        let base = self.get_char_base_mut();
        match &mut base.vf {
            ValOrFn::Value(buf, len) => Ok((*buf, *len)),
            ValOrFn::Function(f) => Ok(f()),
        }
    }
    fn read_value(&mut self) -> Result<([u8; 255], usize), Error> {
        self.read()
    }
    fn write(&mut self, val: &[u8]) -> Result<(), Error> {
        let mut buf = [0; 255];
        //eprintln!("writing to char: {:?}", val);
        buf[..val.len()].copy_from_slice(val);
        let mut val = ValOrFn::Value(buf, val.len());
        self.write_val_or_fn(&mut val);
        Ok(())
    }
    /*fn service(&self) -> &Path {
        let base = self.get_char_base();
        base.path.parent().unwrap()
    }*/
    fn write_acquired(&self) -> bool {
        let base = self.get_char_base();
        if let Some(_) = base.write {
            true
        } else {
            false
        }
    }
    fn notify_acquired(&self) -> bool {
        let base = self.get_char_base();
        if let Some(Notify::Fd(_)) = base.notify {
            true
        } else {
            false
        }
    }
    fn notifying(&self) -> bool {
        let base = self.get_char_base();
        base.notify.is_some()
    }
    fn uuid(&self) -> &str {
        let base = self.get_char_base();
        &base.uuid
    }
    fn flags(&self) -> CharFlags {
        let base = self.get_char_base();
        base.flags
    }
}
impl Introspectable for LocalCharBase {
    fn introspectable_str(&self) -> String {
        let mut ret = String::new();
        ret.push_str(INTROSPECT_FMT_P1);
        ret.push_str(self.path.to_str().unwrap());
        ret.push_str(INTROSPECT_FMT_P2);
        ret.push_str(PROP_STR);
        ret.push_str(CHAR_STR);
        let children: Vec<&str> = self
            .descs
            .values()
            .map(|s| s.path.file_name().unwrap().to_str().unwrap())
            .collect();
        child_nodes(&children, &mut ret);
        ret.push_str(INTROSPECT_FMT_P3);
        ret
    }
}

impl Properties for LocalCharBase {
    const INTERFACES: &'static [(&'static str, &'static [&'static str])] = &[CHAR_IF, PROP_IF];
    fn get_inner<'a, 'b>(&mut self, interface: &str, prop: &str) -> Option<Param<'a, 'b>> {
        eprintln!(
            "org.freedesktop.DBus.Charactersitic interface:\n{}, prop {}",
            interface, prop
        );
        match interface {
            CHAR_IF_STR => match prop {
                UUID_PROP => Some(base_param_to_variant(self.uuid.to_string().into())),
                SERVICE_PROP => Some(base_param_to_variant(Base::ObjectPath(
                    self.path.parent().unwrap().to_str().unwrap().to_string(),
                ))),
                VALUE_PROP => {
                    let (v, l) = self.vf.to_value();
                    eprintln!("vf: {:?}\nValue: {:?}", self.vf, &v[..l]);
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
    fn set_inner(&mut self, interface: &str, prop: &str, val: Variant) -> Option<String> {
        match interface {
            SERV_IF_STR => match prop {
                HANDLE_PROP => {
                    if let Variant::Uint16(handle) = val {
                        eprintln!("setting Handle prop: {:?}", handle); // TODO remove
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
pub struct RemoteCharBase {
    pub(crate) uuid: UUID,
    chars: HashMap<UUID, RemoteDescBase>,
    notify_fd: Option<RawFd>,
    path: PathBuf,
}
impl RemoteCharBase {
    pub(crate) fn from_props(
        value: &mut HashMap<String, params::Variant>,
        path: PathBuf,
    ) -> Result<Self, Error> {
        let uuid = match value.remove("UUID") {
            Some(addr) => {
                if let Param::Base(Base::String(addr)) = addr.value {
                    addr.into()
                } else {
                    return Err(Error::DbusReqErr(
                        "Invalid device returned; UUID field is invalid type".to_string(),
                    ));
                }
            }
            None => {
                return Err(Error::DbusReqErr(
                    "Invalid device returned; missing UUID field".to_string(),
                ))
            }
        };
        Ok(RemoteCharBase {
            uuid,
            chars: HashMap::new(),
            notify_fd: None,
            path,
        })
    }
}
impl Drop for RemoteCharBase {
    fn drop(&mut self) {
        if let Some(fd) = self.notify_fd {
            close(fd).ok();
        }
    }
}
pub struct RemoteChar<'a, 'b, 'c> {
    pub(super) uuid: UUID,
    pub(super) service: &'a mut RemoteService<'b, 'c>,
    #[cfg(feature = "unsafe-opt")]
    ptr: *mut RemoteCharBase,
}

impl<'a, 'b, 'c> RemoteChar<'a, 'b, 'c> {
    pub fn acquire_notify<'sel>(&'sel mut self) -> Result<RawFd, Error> {
        let base = self.get_char_mut();
        let mut msg = MessageBuilder::new()
            .call("AcquireNotify".to_string())
            .on(base.path.to_str().unwrap().to_string())
            .at(BLUEZ_DEST.to_string())
            .with_interface(CHAR_IF_STR.to_string())
            .build();
        let options = Param::Container(Container::Dict(params::Dict {
            key_sig: signature::Base::String,
            value_sig: signature::Type::Container(signature::Container::Variant),
            map: HashMap::new(),
        }));
        msg.body.push_old_param(&options).unwrap();
        let blue = self.get_blue_mut();
        let res_idx = blue.rpc_con.send_message(&mut msg, Timeout::Infinite)?;
        loop {
            blue.process_requests()?;
            if let Some(res) = blue.rpc_con.try_get_response(res_idx) {
                let res = res.unmarshall_all().unwrap();
                return match res.typ {
                    MessageType::Reply => {
                        let fd = if let Some(Param::Base(Base::UnixFd(fd))) = res.params.get(0) {
                            *fd
                        } else {
                            return Err(Error::DbusReqErr(
                                "Response returned unexpected of parameter".to_string(),
                            ));
                        };
                        let fd = res.raw_fds[fd as usize];
                        let base = self.get_char_mut();
                        base.notify_fd = Some(fd);
                        Ok(fd)
                    }
                    MessageType::Error => Err(Error::try_from(&res).unwrap()),
                    _ => unreachable!(),
                };
            }
        }
    }
    pub fn try_get_notify(&self) -> Result<Option<([u8; 255], usize)>, Error> {
        let base = self.get_char();
        let fd = match base.notify_fd {
            Some(fd) => fd,
            None => return Ok(None),
        };
        let mut ret = [0; 255];
        let msg = socket::recvmsg(
            fd,
            &[IoVec::from_mut_slice(&mut ret)],
            None,
            socket::MsgFlags::MSG_DONTWAIT,
        )?;
        Ok(Some((ret, msg.bytes)))
    }
    pub fn wait_get_notify(
        &self,
        timeout: Option<Duration>,
    ) -> Result<Option<([u8; 255], usize)>, Error> {
        let base = self.get_char();
        let fd = match base.notify_fd {
            Some(fd) => fd,
            None => return Ok(None),
        };
        let timeout = match timeout {
            Some(dur) => dur.as_micros(),
            None => 0,
        };
        let mut ret = [0; 255];
        let msg = if timeout == 0 {
            socket::recvmsg(
                fd,
                &[IoVec::from_mut_slice(&mut ret)],
                None,
                socket::MsgFlags::MSG_DONTWAIT,
            )?
        } else {
            let tv = TimeVal::microseconds(timeout.try_into().unwrap());
            socket::setsockopt(fd, socket::sockopt::ReceiveTimeout, &tv)?;
            socket::recvmsg(
                fd,
                &[IoVec::from_mut_slice(&mut ret)],
                None,
                socket::MsgFlags::empty(),
            )?
        };
        Ok(Some((ret, msg.bytes)))
    }
    pub fn get_notify_fd(&self) -> Option<RawFd> {
        let base = self.get_char();
        base.notify_fd
    }
    fn get_blue_mut(&mut self) -> &mut Bluetooth {
        self.service.dev.blue
    }
    fn get_char(&self) -> &RemoteCharBase {
        #[cfg(feature = "unsafe-opt")]
        unsafe {
            return &*self.ptr;
        }
        let service = &self.service;
        let dev = &service.dev;
        let blue = &dev.blue;
        &blue.devices[&dev.mac].services[&service.uuid].chars[&self.uuid]
    }
    fn get_char_mut(&mut self) -> &mut RemoteCharBase {
        #[cfg(feature = "unsafe-opt")]
        unsafe {
            return &mut *self.ptr;
        }
        let service = &mut self.service;
        let dev = &mut service.dev;
        let blue = &mut dev.blue;
        blue.devices
            .get_mut(&dev.mac)
            .unwrap()
            .services
            .get_mut(&service.uuid)
            .unwrap()
            .chars
            .get_mut(&self.uuid)
            .unwrap()
    }
    /*pub fn start_notify(&self) -> ();*/
}

impl Characteristic for RemoteChar<'_, '_, '_> {
    fn read(&mut self) -> Result<([u8; 255], usize), Error> {
        unimplemented!()
    }
    fn read_value(&mut self) -> Result<([u8; 255], usize), Error> {
        unimplemented!()
    }
    fn write(&mut self, _val: &[u8]) -> Result<(), Error> {
        unimplemented!()
    }
    fn uuid(&self) -> &str {
        &self.uuid
    }
    /*fn service(&self) -> &Path {
        unimplemented!()
    }*/
    fn write_acquired(&self) -> bool {
        unimplemented!()
    }
    fn notify_acquired(&self) -> bool {
        unimplemented!()
    }
    fn notifying(&self) -> bool {
        unimplemented!()
    }
    fn flags(&self) -> CharFlags {
        unimplemented!()
    }
}
