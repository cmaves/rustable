

use std::fmt::{Formatter, Debug};
mod characteristic;
mod service;
mod descriptor;


pub use service::{LocalService, Service, LocalServiceBase};
pub use characteristic::{LocalCharactersitic, Charactersitic, LocalCharBase};
pub use descriptor::{LocalDescriptor, Descriptor};

/*
pub struct DbusNotifier<'a> {
	bt: Bluetooth,
	bt
}
pub struct SocketNotifier {

}
*/

pub trait GattDevice<T, U> 
	where T: Charactersitic,
	      U: Service<T>
{

}



