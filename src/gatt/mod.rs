use crate::UUID;
use std::fmt::{Debug, Formatter};
mod characteristic;
mod descriptor;
mod service;

//pub use characteristic::{Charactersitic, LocalCharBase, LocalCharactersitic, CharFlags};
pub use characteristic::*;
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
