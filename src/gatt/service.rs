use crate::gatt::*;
use crate::introspect::*;
use crate::*;
use rustbus::params::message::Message;
use std::collections::hash_map::Keys;
use std::collections::HashMap;
use std::ffi::OsStr;
use std::path::{Component, Path, PathBuf};

pub trait Service<'a> {
    type CharType: Characteristic;
    type Value;
    fn uuid(&self) -> &UUID;
    fn primary(&self) -> bool;
    fn device(&self) -> &Path;
    fn includes(&self) -> &[&Path];
    fn handle(&self) -> u16;
    fn char_uuids(&self) -> Keys<UUID, Self::Value>;
    fn get_char<T: ToUUID>(&'a mut self, uuid: T) -> Option<Self::CharType>;
}

pub struct LocalServiceBase {
    pub(crate) index: u8,
    char_index: u16,
    handle: u16,
    pub(crate) uuid: UUID,
    pub(crate) path: PathBuf,
    pub(crate) chars: HashMap<UUID, LocalCharBase>,
    primary: bool,
}
impl LocalServiceBase {
    pub fn add_char(&mut self, mut character: LocalCharBase) {
        // TODO: add check for duplicate UUIDs
        assert!(self.chars.len() < 65535);
        character.index = self.char_index;
        self.char_index += 1;
        self.chars.insert(character.uuid.clone(), character);
    }

    pub fn service_call<'a, 'b>(&mut self, _call: MarshalledMessage) -> MarshalledMessage {
        unimplemented!()
    }

    pub(crate) fn update_path(&mut self, mut base: PathBuf) {
        let mut name = String::with_capacity(11);
        write!(name, "service{:02x}", self.index).unwrap();
        base.push(name);
        self.path = base;
        for character in self.chars.values_mut() {
            character.update_path(&self.path);
        }
    }
    pub(crate) fn match_chars(
        &mut self,
        msg_path: &Path,
        header: &DynamicHeader,
    ) -> Option<DbusObject> {
        eprintln!("Checking for characteristic for match: {:?}", msg_path);
        let mut components = msg_path.components().take(2);
        if let Component::Normal(path) = components.next().unwrap() {
            let path = path.to_str()?;
            if !path.starts_with("char") {
                return None;
            }
            for character in self.chars.values_mut() {
                //write!(&mut char_str, "char{:02}", character.index).unwrap();
                let char_name = character.path.file_name().unwrap();
                if let Ok(path) = msg_path.strip_prefix(char_name) {
                    eprintln!("match_chars() path: {:?}", path);
                    if path == OsStr::new("") {
                        return Some(DbusObject::Char(character));
                    } else {
                        return character.match_descs(path, header);
                    }
                }
            }
            None
        } else {
            None
        }
    }
    pub fn new<T: ToUUID>(uuid: T, primary: bool) -> Self {
        let uuid = uuid.to_uuid();
        LocalServiceBase {
            index: 0,
            char_index: 0,
            handle: 0,
            uuid,
            path: PathBuf::new(),
            chars: HashMap::new(),
            primary,
        }
    }
}
pub struct LocalService<'a> {
    pub(crate) uuid: UUID,
    pub(crate) bt: &'a mut Bluetooth,
    #[cfg(feature = "unsafe-opt")]
    ptr: *mut LocalServiceBase,
}
impl<'a, 'b: 'a, 'c: 'a, 'd: 'a> Service<'a> for LocalService<'b> {
    type CharType = LocalCharactersitic<'a, 'b>;
    type Value = LocalCharBase;
    fn uuid(&self) -> &UUID {
        &self.uuid
    }
    fn char_uuids(&self) -> Keys<UUID, Self::Value> {
        let service = self.get_service();
        service.chars.keys()
    }
    fn primary(&self) -> bool {
        let service = self.get_service();
        service.primary
    }
    fn get_char<T: ToUUID>(&'a mut self, uuid: T) -> Option<Self::CharType> {
        let service = self.get_service_mut();
        let uuid = uuid.to_uuid();
        if service.chars.contains_key(&uuid) {
            drop(service);
            Some(LocalCharactersitic {
                service: self,
                uuid: uuid.clone(),
            })
        } else {
            None
        }
    }
    /*
    fn get_char<'a, V: ToUUID>(&'a mut self, uuid: &V) -> Option<LocalCharactersitic<'a, 'b, 'c, 'd>> {

    }
    */
    fn device(&self) -> &Path {
        unimplemented!()
    }
    fn includes(&self) -> &[&Path] {
        unimplemented!()
    }
    fn handle(&self) -> u16 {
        unimplemented!()
    }
}

impl LocalService<'_> {
    pub(super) fn get_service(&self) -> &LocalServiceBase {
        &self.bt.services[&self.uuid]
    }
    pub(super) fn get_service_mut(&mut self) -> &mut LocalServiceBase {
        self.bt.services.get_mut(&self.uuid).unwrap()
    }
}

impl Properties for LocalServiceBase {
    const INTERFACES: &'static [(&'static str, &'static [&'static str])] = &[SERV_IF, PROP_IF];
    fn get_inner<'a, 'b>(&mut self, interface: &str, prop: &str) -> Option<Param<'a, 'b>> {
        match interface {
            SERV_IF_STR => match prop {
                UUID_PROP => Some(base_param_to_variant(self.uuid.to_string().into())),
                PRIMARY_PROP => Some(base_param_to_variant(self.primary.into())),
                DEVICE_PROP => Some(base_param_to_variant(Base::ObjectPath(
                    self.path.parent().unwrap().to_str().unwrap().to_string(),
                ))),
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

impl Introspectable for LocalServiceBase {
    fn introspectable_str(&self) -> String {
        let mut ret = String::new();
        ret.push_str(INTROSPECT_FMT_P1);
        ret.push_str(self.path.to_str().unwrap());
        ret.push_str(INTROSPECT_FMT_P2);
        ret.push_str(PROP_STR);
        ret.push_str(SERVICE_STR);
        let children: Vec<&str> = self
            .chars
            .values()
            .map(|s| s.path.file_name().unwrap().to_str().unwrap())
            .collect();
        child_nodes(&children, &mut ret);
        ret.push_str(INTROSPECT_FMT_P3);
        ret
    }
}

pub struct RemoteServiceBase {
    pub(crate) uuid: UUID,
    primary: bool,
    path: PathBuf,
    pub(crate) chars: HashMap<UUID, RemoteCharBase>,
}
impl RemoteServiceBase {
    pub fn from_props(
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
        let primary = match value.remove("Primary") {
            Some(p) => {
                if let Param::Base(Base::Boolean(p)) = p.value {
                    p.into()
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
        Ok(RemoteServiceBase {
            uuid,
            primary,
            path,
            chars: HashMap::new(),
        })
    }
}
impl TryFrom<&Message<'_, '_>> for RemoteServiceBase {
    type Error = Error;
    fn try_from(_value: &Message) -> Result<Self, Self::Error> {
        unimplemented!()
    }
}

pub struct RemoteService<'a, 'b> {
    pub(crate) uuid: UUID,
    pub(crate) dev: &'a mut RemoteDevice<'b>,
    #[cfg(feature = "unsafe-opt")]
    pub(crate) ptr: *mut RemoteServiceBase,
}
impl RemoteService<'_, '_> {
    fn get_service(&self) -> &RemoteServiceBase {
        #[cfg(feature = "unsafe-opt")]
        unsafe {
            return &*self.ptr;
        }
        &self.dev.blue.devices[&self.dev.mac].services[&self.uuid]
    }
    fn get_service_mut(&mut self) -> &mut RemoteServiceBase {
        #[cfg(feature = "unsafe-opt")]
        unsafe {
            return &mut *self.ptr;
        }
        self.dev
            .blue
            .devices
            .get_mut(&self.dev.mac)
            .unwrap()
            .services
            .get_mut(&self.uuid)
            .unwrap()
    }
}

impl<'a, 'b: 'a, 'c: 'a> Service<'a> for RemoteService<'b, 'c> {
    type CharType = RemoteChar<'a, 'b, 'c>;
    type Value = RemoteCharBase;
    fn uuid(&self) -> &UUID {
        &self.get_service().uuid
    }
    fn primary(&self) -> bool {
        self.get_service().primary
    }
    fn device(&self) -> &Path {
        self.get_service().path.parent().unwrap()
    }
    fn includes(&self) -> &[&Path] {
        unimplemented!()
    }
    fn handle(&self) -> u16 {
        unimplemented!()
    }
    fn char_uuids(&self) -> Keys<UUID, Self::Value> {
        self.get_service().chars.keys()
    }
    fn get_char<T: ToUUID>(&'a mut self, uuid: T) -> Option<Self::CharType> {
        let uuid = uuid.to_uuid();
        if let Some(_character) = self.get_service_mut().chars.get_mut(&uuid) {
            Some(RemoteChar {
                service: self,
                uuid,
                #[cfg(feature = "unsafe-opt")]
                ptr: _character,
            })
        } else {
            None
        }
    }
}
