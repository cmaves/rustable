
use crate::*;
use crate::gatt::*;
use crate::introspect::*;
use rustbus::Message;
use std::path::{Component, Path, PathBuf};
use std::collections::{HashMap};
use std::collections::hash_map::Keys;


pub trait Service<T: Charactersitic> {
	fn uuid(&self) -> &str;
	fn primary(&self) -> bool;
	fn device(&self) -> &Path;
	fn includes(&self) -> &[&Path];
	fn handle(&self) -> u16;
	fn char_uuids(&self) -> Keys<String, T>;
	fn get_char(&mut self, uuid: &str) -> &mut T;
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

    pub fn service_call<'a, 'b>(&mut self, call: &Message<'a, 'b>) -> Message<'a, 'b> {
        unimplemented!()
    }

    pub(crate) fn update_path(&mut self, mut base: PathBuf) {
		let mut name = String::with_capacity(11);
        write!(name, "{:04x}", self.index).unwrap();
		base.push(name);
		self.path = base;
        for character in self.chars.values_mut() {
            character.update_path(&self.path);
        }
    }
    pub(crate) fn match_chars(&mut self, msg_path: &Path, msg: &Message) -> Option<DbusObject> {
        eprintln!("Checking for characteristic for match");
        let mut components = msg_path.components().take(2);
        if let Component::Normal(path) = components.next().unwrap() {
            let path = path.to_str()?;
            if !path.starts_with("char") {
                return None;
            }
            let mut char_str = String::new();
            for character in self.chars.values_mut() {
                char_str.clear();
                write!(&mut char_str, "char{:02}", character.index).unwrap();
                if let Ok(path) = msg_path.strip_prefix(char_str) {
                    return character.match_descs(path, msg);
                } else {
                    return Some(DbusObject::Char(character));
                }
            }
            None
        } else {
            None
        }
    }
    pub fn new(uuid: String, primary: bool) -> Self {
		let uuid: Rc<str> = uuid.into();
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
pub struct LocalService<'a, 'b, 'c> {
	pub(crate) uuid: UUID,
	pub(crate) bt: &'a mut Bluetooth<'b, 'c>
}
impl LocalService<'_, '_, '_> {

}

impl<'a, 'b> Properties<'a, 'b> for LocalServiceBase {
    const INTERFACES: &'static [(&'static str, &'static [&'static str])] = &[SERV_IF, PROP_IF];
    fn get_inner(&mut self, interface: &str, prop: &str) -> Option<Param<'a, 'b>> {
        match interface {
            SERV_IF_STR => match prop {
                UUID_PROP => Some(base_param_to_variant(self.uuid.to_string().into())),
                PRIMARY_PROP => Some(base_param_to_variant(self.primary.into())),
                DEVICE_PROP => {
                    Some(base_param_to_variant(Base::ObjectPath(
                        self.path.parent().unwrap().to_str().unwrap().to_string())
                    ))
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
