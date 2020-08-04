use std::fmt::Write;
use std::path::{Path, PathBuf};

use crate::interfaces::*;
use crate::*;

/// Describes the methods avaliable on local and remote GATT descriptors.
pub trait Descriptor {
	/// Reads the value of the descriptor.
	fn read(&mut self) -> Result<([u8; 512], usize), Error>;
	/// Returns a previous value of the GATT descriptor. Check the individual
	/// implementors, for a more precise definition.
	fn read_cached(&mut self) -> Result<([u8; 512], usize), Error>;
	/// Writes a value to a GATT descriptors.
	fn write(&mut self, val: &[u8]) -> Result<(), Error>;
	/// Get the UUID of the descriptor.
	fn uuid(&mut self) -> UUID;
	/// Get the flags present on the descritptors
	fn flags(&mut self) -> DescFlags;
}

#[derive(Debug)]
pub struct LocalDescriptor {
	pub(crate) vf: ValOrFn,
    pub(crate) path: PathBuf,
    pub(crate) index: u16,
    handle: u16,
    uuid: Rc<String>,
}
impl LocalDescriptor {
    pub fn new(_uuid: String) -> Self {
        unimplemented!()
    }
    pub(super) fn update_path(&mut self, base: &Path) {
        self.path = base.to_owned();
        let mut name = String::with_capacity(7);
        write!(&mut name, "desc{:03x}", self.index).unwrap();
        self.path.push(name);
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

impl Properties for LocalDescriptor {
    const INTERFACES: &'static [(&'static str, &'static [&'static str])] = &[DESC_IF, PROP_IF];
    fn get_inner<'a, 'b>(&mut self, interface: &str, prop: &str) -> Option<Param<'a, 'b>> {
        match interface {
            DESC_IF_STR => match prop {
                UUID_PROP => Some(base_param_to_variant(self.uuid.to_string().into())),
                CHAR_PROP => Some(base_param_to_variant(Base::ObjectPath(
                    self.path.parent().unwrap().to_str().unwrap().to_string(),
                ))),
                VALUE_PROP => unimplemented!(),
                FLAGS_PROP => unimplemented!(),
                HANDLE_PROP => Some(base_param_to_variant(self.index.into())),
                _ => None,
            },
            PROP_IF_STR => None,
            _ => None,
        }
    }
    fn set_inner(&mut self, _interface: &str, _prop: &str, _val: Variant) -> Option<String> {
        unimplemented!()
    }
}

impl Introspectable for LocalDescriptor {
    fn introspectable_str(&self) -> String {
        unimplemented!()
    }
}

pub struct RemoteDescBase {}
