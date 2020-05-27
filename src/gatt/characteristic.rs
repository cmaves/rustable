use crate::interfaces::*;
use crate::introspect::*;
use crate::*;
use rustbus::message::Message;
use rustbus::{Base, Container, Param};
use std::os::unix::net::UnixDatagram;

pub trait Charactersitic {
    fn read(&mut self) -> Result<([u8; 255], usize), Error>;
    fn read_value(&mut self) -> Result<([u8; 255], usize), Error>;
    fn write(&mut self, val: &[u8]) -> Result<(), Error>;
    fn uuid(&self) -> &str;
    fn service(&self) -> &Path;
    fn write_acquired(&self) -> bool;
    fn notify_acquired(&self) -> bool;
    fn notifying(&self) -> bool;
    fn flags(&self) -> CharFlags;
    /*pub fn start_notify(&self) -> ();
    pub fn acquire_notify(&self) -> (); */
}

#[derive(Debug)]
enum Notify {
    Signal,
    Fd(UnixDatagram),
}

pub struct LocalCharBase {
    vf: ValOrFn,
    pub(crate) index: u16,
    handle: u16,
    pub(super) uuid: UUID,
    pub(crate) path: PathBuf,
    notify: Option<Notify>,
    write: Option<UnixDatagram>,
    pub(crate) descs: HashMap<String, LocalDescriptor>,
    flags: CharFlags,
}
impl LocalCharBase {
    pub(super) fn update_path(&mut self, base: &Path) {
        self.path = base.to_owned();
        let mut name = String::with_capacity(8);
        write!(&mut name, "char{:04x}", self.index);
        self.path.push(name);
        for desc in self.descs.values_mut() {
            desc.update_path(&self.path);
        }
    }
    pub(crate) fn char_call<'a, 'b>(&mut self, call: &Message<'a, 'b>) -> Message<'a, 'b> {
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

    pub(super) fn match_descs(&mut self, msg_path: &Path, msg: &Message) -> Option<DbusObject> {
        unimplemented!()
    }
    pub fn new(uuid: String, flags: CharFlags) -> Self {
        let uuid: Rc<str> = uuid.into();
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

pub struct LocalCharactersitic<'a, 'b: 'a, 'c: 'a, 'd: 'a> {
    pub(super) uuid: UUID,
    pub(super) service: &'a mut LocalService<'b, 'c, 'd>,
    #[cfg(feature = "unsafe-opt")]
    base: *mut LocalCharBase,
}
impl LocalCharactersitic<'_, '_, '_, '_> {
    pub fn write_val_or_fn(&mut self, val: &mut ValOrFn) {
        let base = self.get_char_base_mut();
        std::mem::swap(&mut base.vf, val);
    }
    fn signal_change(&mut self) -> Result<(), Error> {
        let base = self.get_char_base_mut();
        //let (v, l) = self.get_char_base_mut().vf.to_value();
        let (v, l) = base.vf.to_value();
        let value = &v[..];
        let mut params = Vec::with_capacity(3);
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
            .with_params(params)
            .build();
        // eprintln!("msg to be send: {:#?}", msg);
        self.service.bt.rpc_con.send_message(&mut msg, None)?;
        Ok(())
    }
    pub fn notify(&mut self) -> Result<(), Error> {
        let base = self.get_char_base_mut();
        let (buf, len) = base.vf.to_value();
        if let Some(notify) = &mut base.notify {
            match notify {
                Notify::Signal => self.signal_change()?,
                Notify::Fd(sock) => {
                    if let Err(_) = sock.send(&buf[..len]) {
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
}

impl Charactersitic for LocalCharactersitic<'_, '_, '_, '_> {
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
        buf[..val.len()].copy_from_slice(val);
        let mut val = ValOrFn::Value(buf, val.len());
        self.write_val_or_fn(&mut val);
        Ok(())
    }
    fn service(&self) -> &Path {
        let base = self.get_char_base();
        base.path.parent().unwrap()
    }
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

impl<'a, 'b> Properties<'a, 'b> for LocalCharBase {
    const INTERFACES: &'static [(&'static str, &'static [&'static str])] = &[CHAR_IF, PROP_IF];
    fn get_inner(&mut self, interface: &str, prop: &str) -> Option<Param<'a, 'b>> {
        // eprintln!("org.freedesktop.DBus.Charactersitic interface:\n{}, prop {}", interface, prop);
        match interface {
            CHAR_IF_STR => match prop {
                UUID_PROP => Some(base_param_to_variant(self.uuid.to_string().into())),
                SERVICE_PROP => Some(base_param_to_variant(Base::ObjectPath(
                    self.path.parent().unwrap().to_str().unwrap().to_string(),
                ))),
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
