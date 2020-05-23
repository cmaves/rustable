use std::fmt::{Debug, Formatter};
mod characteristic;
mod descriptor;
mod service;

pub use characteristic::{Charactersitic, LocalCharBase, LocalCharactersitic};
pub use descriptor::{Descriptor, LocalDescriptor};
pub use service::{LocalService, LocalServiceBase, Service};

/*
pub struct DbusNotifier<'a> {
    bt: Bluetooth,
    bt
}
pub struct SocketNotifier {

}
*/

pub trait GattDevice<T, U>
where
    T: Charactersitic,
    U: Service<T>,
{
}
