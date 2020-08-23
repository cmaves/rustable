use crate::gatt::*;
use crate::introspect::*;
use crate::*;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Describes the methods avaliable on remote and local GATT services
pub trait Service<'a> {
    type CharType: Characteristic<'a>;
    /// Return the `UUID` of the service.
    fn uuid(&self) -> &UUID;
    /// Return in a service is primary service.
    fn primary(&self) -> bool;
    /// Get the path to the parent service.
    fn device(&self) -> &Path;
    fn includes(&self) -> &[&Path];
    /// Get the Bluetooth handle of service
    fn handle(&self) -> u16;
    /// Get an iterator of all the keys on the valu
    fn char_uuids(&self) -> Vec<UUID>;
    /// Get a characteristic of a GATT service by UUID.
    fn get_char<T: ToUUID>(&'a mut self, uuid: T) -> Option<Self::CharType>;
    /// Get the DBus path of the service.
    fn get_path(&self) -> &Path;
}
/// `LocalServiceBase` is used to construct local service to be provided by the local GATT server.
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
    /// Construct a new `LocalServiceBase` to construct as service with.
    ///
    /// It can be added to `Bluetooth` with [`Bluetooth::add_service()`].
    ///
    /// [`Bluetooth::add_service()`]: ../struct.Bluetooth.html#method.add_service
    pub fn new<T: ToUUID>(uuid: T, primary: bool) -> Self {
        let uuid = uuid.to_uuid();
        assert!(validate_uuid(&uuid));
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
    /// Add a characteristic to the service
    pub fn add_char(&mut self, mut character: LocalCharBase) {
        // TODO: add check for duplicate UUIDs
        //assert!(self.chars.len() < 65535);
        character.serv_uuid = self.uuid.clone();
        for desc in character.descs.values_mut() {
            desc.serv_uuid = self.uuid.clone();
        }
        character.index = self.char_index;
        self.char_index += 1;
        // eprintln!("Adding char: {:?}\nto\n{:?}", character, self.uuid);
        self.chars.insert(character.uuid.clone(), character);
    }
    /// Handle method calls on the local servic
    pub(crate) fn service_call<'a, 'b>(&mut self, _call: MarshalledMessage) -> MarshalledMessage {
        unimplemented!()
    }
    /// Update the path of this service as well as the child characteristics
    pub(crate) fn update_path(&mut self, mut base: PathBuf) {
        let mut name = String::with_capacity(11);
        write!(name, "service{:02x}", self.index).unwrap();
        base.push(name);
        self.path = base;
        for character in self.chars.values_mut() {
            character.update_path(&self.path);
        }
    }
}
/*
impl AsRef<LocalServiceBase> for LocalServiceBase {
    fn as_ref(&self) -> &LocalServiceBase {
        &self
    }
}
*/
impl GattDbusObject for LocalServiceBase {
    fn path(&self) -> &Path {
        &self.path
    }
    fn uuid(&self) -> &UUID {
        &self.uuid
    }
}
impl<'a> HasChildren<'a> for LocalServiceBase {
    type Child = &'a mut LocalCharBase;
    fn get_children(&self) -> Vec<UUID> {
        self.chars.keys().map(|x| x.clone()).collect()
    }
    fn get_child<T: ToUUID>(&'a mut self, uuid: T) -> Option<Self::Child> {
        let uuid = uuid.to_uuid();
        self.chars.get_mut(&uuid)
    }
}
/// Struct representing service currently being hosted by the local GATT server
pub struct LocalService<'a> {
    pub(crate) uuid: UUID,
    pub(crate) bt: &'a mut Bluetooth,
    #[cfg(feature = "unsafe-opt")]
    ptr: *mut LocalServiceBase,
}

impl<'a, 'b: 'a, 'c: 'a, 'd: 'a> Service<'a> for LocalService<'b> {
    type CharType = LocalCharactersitic<'a, 'b>;
    fn uuid(&self) -> &UUID {
        &self.uuid
    }
    fn char_uuids(&self) -> Vec<UUID> {
        let service = self.get_service_base();
        service.chars.keys().map(|k| k.clone()).collect()
    }
    fn primary(&self) -> bool {
        let service = self.get_service_base();
        service.primary
    }
    fn get_char<T: ToUUID>(&'a mut self, uuid: T) -> Option<Self::CharType> {
        let service = self.get_service_base_mut();
        let uuid = uuid.to_uuid();
        if service.chars.contains_key(&uuid) {
            drop(service);
            Some(LocalCharactersitic::new(self, uuid))
        } else {
            None
        }
    }
    fn device(&self) -> &Path {
        unimplemented!()
    }
    fn includes(&self) -> &[&Path] {
        unimplemented!()
    }
    fn handle(&self) -> u16 {
        unimplemented!()
    }
    fn get_path(&self) -> &Path {
        &self.get_service_base().path
    }
}

impl<'a> LocalService<'a> {
    pub(crate) fn new<T: ToUUID>(bt: &'a mut Bluetooth, uuid: T) -> Self {
        let uuid = uuid.to_uuid();
        LocalService { bt, uuid }
    }
    pub(super) fn get_service_base(&self) -> &LocalServiceBase {
        &self.bt.services[&self.uuid]
    }
    pub(super) fn get_service_base_mut(&mut self) -> &mut LocalServiceBase {
        match self.bt.services.get_mut(&self.uuid) {
            Some(ret) => ret,
            None => panic!("Failed to find: {}", self.uuid),
        }
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
/// Internal data structure. Currently no use for users of this crate.
///
/// This structure is public because it is used as a type for `std::collections::hash_map::Keys`,
/// for the [`Device::get_services()`] functions.
pub struct RemoteServiceBase {
    pub(crate) uuid: UUID,
    primary: bool,
    path: PathBuf,
    pub(crate) chars: HashMap<UUID, RemoteCharBase>,
}
impl RemoteServiceBase {
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
    pub(crate) fn update_all(&mut self, props: HashMap<String, Variant>) -> Result<(), Error> {
        unimplemented!()
    }
}
impl GattDbusObject for RemoteServiceBase {
    fn path(&self) -> &Path {
        &self.path
    }
    fn uuid(&self) -> &UUID {
        &self.uuid
    }
}
impl<'a> HasChildren<'a> for RemoteServiceBase {
    type Child = RemoteCharBase;
    fn get_children(&self) -> Vec<UUID> {
        self.chars.keys().map(|x| x.clone()).collect()
    }
    fn get_child<T: ToUUID>(&'a mut self, uuid: T) -> Option<Self::Child> {
        unimplemented!()
    }
}
impl TryFrom<&Message<'_, '_>> for RemoteServiceBase {
    type Error = Error;
    fn try_from(_value: &Message) -> Result<Self, Self::Error> {
        unimplemented!()
    }
}
/// Return type for [`RemoteDevice::get_service()`]. It represents a service on another device.
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
    fn uuid(&self) -> &UUID {
        &self.get_service().uuid
    }
    fn primary(&self) -> bool {
        self.get_service().primary
    }
    fn device(&self) -> &Path {
        self.get_service().path.parent().unwrap()
    }
    /// **unimplemented**
    fn includes(&self) -> &[&Path] {
        unimplemented!()
    }
    /// **unimplemented**
    fn handle(&self) -> u16 {
        unimplemented!()
    }
    fn char_uuids(&self) -> Vec<UUID> {
        self.get_service().chars.keys().map(|k| k.clone()).collect()
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
    fn get_path(&self) -> &Path {
        &self.get_service().path
    }
}
impl GattDbusObject for RemoteService<'_, '_> {
    fn path(&self) -> &Path {
        self.get_service().path()
    }
    fn uuid(&self) -> &UUID {
        self.get_service().uuid()
    }
}

/*
pub(crate) fn match_service<'a, T, U, V, W>(device: &'a mut W, msg_path: &Path, msg: &DynamicHeader) -> Option<DbusObject<'a>>
        where T: GattDbusObject + 'a , // descriptor
              U: GattDbusObject + for<'b>HasChildren<'b, Child=T> + 'a, // character
              V: GattDbusObject + for<'b>HasChildren<'b, Child=U> + 'a, // service
              W: HasChildren<'a, Child=V> // device

{
        // eprintln!("Checking for service for match");
        let mut components = msg_path.components().take(2);
        if let Component::Normal(path) = components.next().unwrap() {
            let path = path.to_str()?;
            if !path.starts_with("service") {
                return None;
            }
            for uuid in device.get_children() {
                let mut service = device.get_child(&uuid).unwrap();
                let service_str = service.path().file_name().unwrap();
                if let Ok(path) = msg_path.strip_prefix(&service_str) {
                    if let Some(_) = path.components().next() { // check to see if there is more to this
                        return match_chars(&mut service, path, msg);
                    } else {
                        return Some(DbusObject::Serv(service.uuid().clone()));
                    }
                }
            }
            None
        } else {
            None
        }
}
*/
