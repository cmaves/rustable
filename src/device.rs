
use crate::{MAC, Bluetooth, Error, UUID};
use crate::gatt::*;
use rustbus::params;
use std::collections::HashMap;
use std::collections::hash_map;
use std::iter::Map;
use std::path::{PathBuf};
use std::rc::Rc;
use std::ffi::OsString;

pub trait Device<'a>: Sized {
	type ServiceType: Service<'a>;
	fn services(&'a mut self) -> ServIter<'a, Self>;
	fn get_service(&mut self) -> Option<Self::ServiceType>;
}
pub struct ServIter<'a, T: Device<'a>> {
	devptr: *mut T,
	itermut: hash_map::IterMut<'a, UUID, RemoteServiceBase>
}
impl<'a, T: Device<'a>> Iterator for ServIter<'a, T> {
	type Item = <T as Device<'a>>::ServiceType;	
	fn next(&mut self) -> Option<Self::Item> {
		let (uuid, base) = self.itermut.next()?;
		Some(uuid.clone(), serv)
	}
}
pub struct RemoteDeviceBase {
	pub(crate) mac: MAC,
	path: PathBuf,
    pub(crate) services: HashMap<MAC, RemoteServiceBase>,
	comp_map: HashMap<OsString, MAC>
}
impl RemoteDeviceBase {
	fn from_props(value: &HashMap<String, params::Variant>, path: PathBuf) -> Result<Self, Error> {
		unimplemented!()
	}
}

pub struct RemoteDevice<'a, 'b, 'c> {
	pub(crate) mac: MAC,
	pub(crate) blue: &'a mut Bluetooth<'b, 'c>,
	#[cfg(feature = "unsafe-opt")]
	ptr: *mut RemoteDeviceBase
}
impl RemoteDevice<'_, '_, '_> {
	fn get_base(&self) -> &RemoteDeviceBase {
		#[cfg(feature = "unsafe-opt")] {
			return &*self.ptr
		}
		&self.blue.devices[&self.mac]
	}
	fn get_base_mut(&mut self) -> &mut RemoteDeviceBase {
		#[cfg(feature = "unsafe-opt")] {
			return &mut *self.ptr
		}
		self.blue.devices.get_mut(&self.mac).unwrap()
	}
}
impl<'a, 'c: 'a, 'd: 'a, 'e: 'a> Device<'a> for RemoteDevice<'c, 'd, 'e> {
	type ServiceType = RemoteService<'a, 'c, 'd, 'e>;
	fn services(&'a mut self) -> ServIter<'a, Self> {
		let devptr: *mut Self = self;
		let base = self.get_base_mut();
		let itermut = base.services.iter_mut();
		ServIter {
			itermut,
			devptr
		}
	}
	fn get_service(&mut self) -> Option<Self::ServiceType> {
		unimplemented!()
		
	}
	
}
/*
impl<'a, 'b> Device<'a, 'b> for Bluetooth {

}
*/


