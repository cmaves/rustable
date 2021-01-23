use crate::interfaces::*;
use crate::introspect::*;
use crate::*;
use rustbus::wire::unmarshal::Error as UnmarshalError;
use std::cell::Cell;
use std::fmt::Write;
use std::path::{Path, PathBuf};

/// `LocalDescBase` is used to create GATT descriptors to be added to `LocalServiceBase`
pub struct LocalDescBase {
    pub(crate) path: PathBuf,
    pub(crate) index: u16,
    pub(crate) uuid: UUID,
    pub(crate) serv_uuid: UUID,
    pub(crate) char_uuid: UUID,
    handle: u16,
    pub vf: ValOrFn,
    pub flags: DescFlags,
    pub write_callback:
        Option<Box<dyn FnMut(&[u8]) -> Result<Option<ValOrFn>, (String, Option<String>)>>>,
}
impl LocalDescBase {
    pub fn new<T: ToUUID>(uuid: T, flags: DescFlags) -> Self {
        let uuid = uuid.to_uuid();
        LocalDescBase {
            uuid,
            flags,
            vf: ValOrFn::default(),
            path: PathBuf::new(),
            serv_uuid: Rc::from(""),
            char_uuid: Rc::from(""),
            write_callback: None,
            index: 0,
            handle: 0,
        }
    }
    pub(super) fn update_path(&mut self, base: &Path) {
        self.path = base.to_owned();
        let mut name = String::with_capacity(8);
        write!(&mut name, "desc{:04x}", self.index).unwrap();
        self.path.push(name);
    }
}
impl Debug for LocalDescBase {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), std::fmt::Error> {
        let wc_str = if let Some(_) = self.write_callback {
            "Some(FnMut)"
        } else {
            "None"
        };
        // TODO: change to use the formatter helper functions
        write!(f, "LocalDescBase{{vf: {:?}, index: {:?}, handle: {:?}, uuid: {:?}, char_uuid: {:?}, serv_uuid: {:?}, path: {:?}, flags: {:?}, write_callback: {}}}", self.vf, self.index, self.handle, self.uuid, self.char_uuid, self.serv_uuid, self.path, self.flags, wc_str)
    }
}
impl AttObject for LocalDescBase {
    fn path(&self) -> &Path {
        &self.path
    }
    fn uuid(&self) -> &UUID {
        &self.uuid
    }
}
/// Represents a descriptor hosted by local instance of `Bluetooth`.
pub struct LocalDesc<'a, 'b, 'c> {
    uuid: UUID,
    character: &'a mut LocalChar<'b, 'c>,
}
impl<'a, 'b, 'c> LocalDesc<'a, 'b, 'c> {
    pub(crate) fn new<T: ToUUID>(character: &'a mut LocalChar<'b, 'c>, uuid: T) -> Self {
        let uuid = uuid.to_uuid();
        LocalDesc { character, uuid }
    }
    fn get_desc_base(&self) -> &LocalDescBase {
        self.character
            .get_char_base()
            .descs
            .get(&self.uuid)
            .unwrap()
    }
    fn get_desc_base_mut(&mut self) -> &mut LocalDescBase {
        self.character
            .get_char_base_mut()
            .descs
            .get_mut(&self.uuid)
            .unwrap()
    }
    pub(crate) fn desc_call(&mut self, call: MarshalledMessage) -> MarshalledMessage {
        let base = self.get_desc_base_mut();
        match &call.dynheader.member.as_ref().unwrap()[..] {
            "ReadValue" => {
                if base.flags.read
                    || base.flags.encrypt_read
                    || base.flags.encrypt_auth_read
                    || base.flags.secure_read
                {
                    let dict: HashMap<String, Variant> = match call.body.parser().get() {
                        Ok(d) => d,
                        Err(e) => match e {
                            UnmarshalError::EndOfMessage => HashMap::new(),
                            _ => {
                                return call.dynheader.make_error_response(
                                    BLUEZ_FAILED.to_string(),
                                    Some("Unexpected type for uint 16.".to_string()),
                                )
                            }
                        },
                    };
                    let offset = match dict.get("offset") {
                        Some(v) => match v.get::<u16>() {
                            Ok(offset) => offset,
                            Err(_) => {
                                return call.dynheader.make_error_response(
                                    BLUEZ_FAILED.to_string(),
                                    Some("Expected type for 'offset' to be  uint16.".to_string()),
                                )
                            }
                        },
                        None => 0,
                    } as usize;
                    let mut reply = call.dynheader.make_response();
                    let val = base.vf.to_value();
                    if offset >= val.len() {
                        // TODO: should this return an error instead of an empty array
                        reply.body.push_param::<&[u8]>(&[]).unwrap();
                    } else {
                        reply.body.push_param(&val[offset..]).unwrap();
                    }
                    reply
                } else {
                    call.dynheader.make_error_response(
                        BLUEZ_NOT_PERM.to_string(),
                        Some("This is not a readable descriptor.".to_string()),
                    )
                }
            }
            "WriteValue" => {
                if base.flags.write
                    || base.flags.encrypt_write
                    || base.flags.encrypt_auth_write
                    || base.flags.secure_write
                {
                    let mut parser = call.body.parser();
                    let bytes = match parser.get() {
                        Ok(bytes) => bytes,
                        Err(_) => {
                            return call.dynheader.make_error_response(
                                BLUEZ_FAILED.to_string(),
                                Some("Expected byte array as first parameter.".to_string()),
                            )
                        }
                    };
                    let dict: HashMap<String, Variant> = match parser.get() {
                        Ok(d) => d,
                        Err(e) => match e {
                            UnmarshalError::EndOfMessage => HashMap::new(),
                            _ => {
                                return call.dynheader.make_error_response(
                                    BLUEZ_FAILED.to_string(),
                                    Some("Expected dict as second parameter.".to_string()),
                                )
                            }
                        },
                    };
                    let offset = match dict.get("offset") {
                        Some(var) => match var.get::<u16>() {
                            Ok(val) => val,
                            Err(_) => {
                                return call.dynheader.make_error_response(
                                    BLUEZ_FAILED.to_string(),
                                    Some("Expected type for 'offset' to be  uint16.".to_string()),
                                )
                            }
                        },
                        None => 0,
                    } as usize;
                    let mut cur_val = base.vf.to_value();
                    let l = cur_val.len() + offset;
                    if l > 512 {
                        return call
                            .dynheader
                            .make_error_response(BLUEZ_INVALID_LEN.to_string(), None);
                    }
                    cur_val.update(bytes, offset);
                    match &mut base.write_callback {
                        Some(cb) => match cb(&cur_val[..]) {
                            Ok(vf) => {
                                if let Some(vf) = vf {
                                    base.vf = vf;
                                }
                            }
                            Err((s1, s2)) => return call.dynheader.make_error_response(s1, s2),
                        },
                        None => base.vf = ValOrFn::Value(cur_val),
                    }
                    call.dynheader.make_response()
                } else {
                    call.dynheader.make_error_response(
                        BLUEZ_NOT_PERM.to_string(),
                        Some("This is not a writable descriptor.".to_string()),
                    )
                }
            }
            _ => call
                .dynheader
                .make_error_response(UNKNOWN_METHOD.to_string(), None),
        }
    }
}

/// Flags for GATT descriptors.
///
/// What each flags does is detailed on
/// page 1552 (Table 3.5) and page 1554 (Table 3.8) of the [Core Specification (5.2)]
///
/// [Core Specification (5.2)]: https://www.bluetooth.com/specifications/bluetooth-core-specification/
#[derive(Clone, Copy, Default, Debug)]
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
    pub fn to_strings(&self) -> Vec<String> {
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

impl Properties for LocalDescBase {
    const INTERFACES: &'static [(&'static str, &'static [&'static str])] = &[DESC_IF, PROP_IF];
    fn get_inner<'a, 'b>(&mut self, interface: &str, prop: &str) -> Option<Param<'a, 'b>> {
        match interface {
            DESC_IF_STR => match prop {
                UUID_PROP => Some(base_param_to_variant(self.uuid.to_string().into())),
                CHAR_PROP => Some(base_param_to_variant(Base::ObjectPath(
                    self.path.parent().unwrap().to_str().unwrap().to_string(),
                ))),
                VALUE_PROP => {
                    let bytes: Vec<Param<'a, 'b>> = self
                        .vf
                        .to_value()
                        .into_iter()
                        .map(|b| Param::Base(Base::Byte(*b)))
                        .collect();
                    Some(container_param_to_variant(Container::Array(
                        params::Array {
                            element_sig: signature::Type::Base(signature::Base::Byte),
                            values: bytes,
                        },
                    )))
                }
                FLAGS_PROP => {
                    let flags: Vec<Param<'a, 'b>> = self
                        .flags
                        .to_strings()
                        .into_iter()
                        .map(|s| Param::Base(Base::String(s)))
                        .collect();
                    Some(container_param_to_variant(Container::Array(
                        params::Array {
                            element_sig: signature::Type::Base(signature::Base::String),
                            values: flags,
                        },
                    )))
                }
                HANDLE_PROP => Some(base_param_to_variant(self.handle.into())),
                _ => None,
            },
            PROP_IF_STR => None,
            _ => None,
        }
    }
    fn set_inner(&mut self, interface: &str, prop: &str, val: Variant) -> Option<String> {
        match interface {
            DESC_IF_STR => match prop {
                HANDLE_PROP => match val.get() {
                    Ok(handle) => {
                        self.handle = handle;
                        None
                    }
                    Err(_) => Some("UnexpectedType".to_string()),
                },
                _ => unimplemented!(),
            },
            PROP_IF_STR => Some("UnknownProperty".to_string()),
            _ => Some("UnknownInterface".to_string()),
        }
    }
}

impl Introspectable for LocalDescBase {
    fn introspectable_str(&self) -> String {
        let mut ret = String::new();
        ret.push_str(INTROSPECT_FMT_P1);
        ret.push_str(self.path.to_str().unwrap());
        ret.push_str(INTROSPECT_FMT_P2);
        ret.push_str(PROP_STR);
        ret.push_str(DESC_STR);
        ret.push_str(INTROSPECT_FMT_P3);
        ret
    }
}

impl AttObject for LocalDesc<'_, '_, '_> {
    fn path(&self) -> &Path {
        self.get_desc_base().path()
    }
    fn uuid(&self) -> &UUID {
        self.get_desc_base().uuid()
    }
}

impl FlaggedAtt for LocalDesc<'_, '_, '_> {
    type Flags = DescFlags;
    fn flags(&self) -> Self::Flags {
        self.get_desc_base().flags
    }
}
pub(crate) struct RemoteDescBase {
    uuid: UUID,
    value: Rc<Cell<AttValue>>,
    path: PathBuf,
}
impl RemoteDescBase {
    pub(crate) fn from_props(
        mut props: HashMap<String, Variant>,
        path: PathBuf,
    ) -> Result<Self, Error> {
        let uuid = match props.remove("UUID") {
            Some(addr) => match addr.get::<String>() {
                Ok(addr) => addr.to_uuid(),
                Err(_) => {
                    return Err(Error::DbusReqErr(
                        "Invalid descriptor returned; UUID field is invalid type".to_string(),
                    ))
                }
            },
            None => {
                return Err(Error::DbusReqErr(
                    "Invalid descriptor returned; missing UUID field".to_string(),
                ))
            }
        };
        let value = match props.remove("Value") {
            Some(var) => match var.get() {
                Ok(cv) => Rc::new(Cell::new(cv)),
                Err(_) => {
                    return Err(Error::DbusReqErr(
                        "Invalid descriptor returned; Value field is invalid type".to_string(),
                    ))
                }
            },
            None => {
                return Err(Error::DbusReqErr(
                    "Invalid descriptor returned; missing Value field".to_string(),
                ))
            }
        };
        Ok(RemoteDescBase { uuid, value, path })
    }
}

impl AttObject for RemoteDescBase {
    fn path(&self) -> &Path {
        &self.path
    }
    fn uuid(&self) -> &UUID {
        &self.uuid
    }
}

/// Represents a descriptor present on a remote device.
pub struct RemoteDesc<'a, 'b, 'c, 'd> {
    pub(super) character: &'a mut RemoteChar<'b, 'c, 'd>,
    pub(super) uuid: UUID,
}
impl RemoteDesc<'_, '_, '_, '_> {
    fn get_desc_base(&self) -> &RemoteDescBase {
        self.character
            .get_char_base()
            .descs
            .get(&self.uuid)
            .unwrap()
    }
}
impl AttObject for RemoteDesc<'_, '_, '_, '_> {
    fn path(&self) -> &Path {
        let base = self.get_desc_base();
        base.path()
    }
    fn uuid(&self) -> &UUID {
        &self.uuid
    }
}
