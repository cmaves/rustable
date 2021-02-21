//! Module containing structures and traits for interacting with remote
//! GATT services/characteristics/descriptors and creating local GATT services.
use crate::{Error, Pending, ToUUID, UUID};
use rustbus::wire::marshal::traits::{Marshal, Signature};
use rustbus::wire::marshal::MarshalContext;
use rustbus::wire::unmarshal;
use rustbus::wire::unmarshal::traits::Unmarshal;
use rustbus::wire::unmarshal::{UnmarshalContext, UnmarshalResult};
use rustbus::{dbus_variant_var, ByteOrder};
use std::borrow::Borrow;
use std::cell::Cell;
use std::fmt::{Debug, Formatter};
use std::mem::MaybeUninit;
use std::ops::{Deref, DerefMut};
use std::path::Path;
use std::rc::Rc;
use std::time::Duration;

mod characteristic;
mod descriptor;
mod service;

//pub use characteristic::{Charactersitic, LocalCharBase, LocalCharactersitic, CharFlags};
pub use characteristic::*;
pub use descriptor::*;
pub use service::*;

dbus_variant_var!(CharVar, U16 => u16; String => String);

/// Types implementing this trait represent Bluetooth ATT (services/characteristics,descriptors),
/// and their associated DBus path.
pub trait AttObject {
    /// Get the DBus object path.
    fn path(&self) -> &Path;
    /// Get the 128-bit UUID of the object.
    fn uuid(&self) -> &UUID;
}
impl<T: AttObject> AttObject for &T {
    fn path(&self) -> &Path {
        T::path(self)
    }
    fn uuid(&self) -> &UUID {
        T::uuid(self)
    }
}
impl<T: AttObject> AttObject for &mut T {
    fn path(&self) -> &Path {
        T::path(self)
    }
    fn uuid(&self) -> &UUID {
        T::uuid(self)
    }
}
pub trait FlaggedAtt: AttObject {
    type Flags;
    fn flags(&self) -> Self::Flags;
}
impl<T: FlaggedAtt> FlaggedAtt for &T {
    type Flags = T::Flags;
    fn flags(&self) -> Self::Flags {
        T::flags(self)
    }
}
impl<T: FlaggedAtt> FlaggedAtt for &mut T {
    type Flags = T::Flags;
    fn flags(&self) -> Self::Flags {
        T::flags(self)
    }
}

/// `AttObject`s implementing this type have 16-bit UUID assigned by the Bluetooth SIG.
pub trait AssignedAtt: AttObject {
    /// Get the 16-bit UUID of the object.
    fn uuid_16(&self) -> u16;
}

/// `AttObject`s implementing this trait are readable, *if* flags allow for it.
pub trait ReadableAtt: AttObject + FlaggedAtt {
    /// Starts a read request of the attribute.
    ///
    /// The returned `Pending` can be resolved with [`Bluetooth::resolve()`] to get the result
    /// when it is finished.
    fn read(&mut self) -> Result<Pending<Result<AttValue, Error>, Rc<Cell<AttValue>>>, Error>;
    /// Generally returns a previous value of the GATT characteristic. Check the individual implementors,
    /// for a more precise definition.
    fn read_cached(&mut self) -> AttValue;

    /// Reads the value of a GATT characteristic, and waits for a response.
    fn read_wait(&mut self) -> Result<AttValue, Error>;
}
impl<T: ReadableAtt> ReadableAtt for &mut T {
    fn read(&mut self) -> Result<Pending<Result<AttValue, Error>, Rc<Cell<AttValue>>>, Error> {
        T::read(self)
    }
    fn read_cached(&mut self) -> AttValue {
        T::read_cached(self)
    }
    fn read_wait(&mut self) -> Result<AttValue, Error> {
        T::read_wait(self)
    }
}
/// `AttObject`s implementing this trait are writeable, *if* flags allow for it.
pub trait WritableAtt: AttObject + FlaggedAtt {
    /// Begin a write operation to a GATT characteristic.
    fn write(
        &mut self,
        val: AttValue,
        write_type: WriteType,
    ) -> Result<Pending<Result<(), Error>, ()>, Error>;
    /// Write a value to a GATT characteristic and wait for completion.
    fn write_wait(&mut self, val: AttValue, write_type: WriteType) -> Result<(), Error>;
    /// Checks if the characteristic's write fd from [`AcquireWrite`] has already been acquired.
    /// Corresponds to reading [`WriteAcquired`] property.
    ///
    /// [`AcquireWrite`]: https://git.kernel.org/pub/scm/bluetooth/bluez.git/tree/doc/gatt-api.txt#n115
    /// [`WriteAcquired`]: https://git.kernel.org/pub/scm/bluetooth/bluez.git/tree/doc/gatt-api.txt#n223
    fn write_acquired(&self) -> bool;
}
impl<T: WritableAtt> WritableAtt for &mut T {
    fn write(
        &mut self,
        val: AttValue,
        write_type: WriteType,
    ) -> Result<Pending<Result<(), Error>, ()>, Error> {
        T::write(self, val, write_type)
    }
    fn write_wait(&mut self, val: AttValue, write_type: WriteType) -> Result<(), Error> {
        T::write_wait(self, val, write_type)
    }
    fn write_acquired(&self) -> bool {
        T::write_acquired(self)
    }
}
fn match_char(gdo: &mut LocalCharBase, path: &Path) -> Option<Option<UUID>> {
    match path.strip_prefix(gdo.path().file_name().unwrap()) {
        Ok(remaining) => {
            if remaining == Path::new("") {
                Some(None)
            } else {
                let r_str = remaining.to_str().unwrap();
                if r_str.len() != 8 || &r_str[..4] != "desc" {
                    return None;
                }
                for uuid in gdo.get_children() {
                    if match_object(&gdo.get_child(&uuid).unwrap(), remaining) {
                        return Some(Some(uuid));
                    }
                }
                None
            }
        }
        Err(_) => None,
    }
}
pub(crate) fn match_serv(
    gdo: &mut LocalServiceBase,
    path: &Path,
) -> Option<Option<(UUID, Option<UUID>)>> {
    match path.strip_prefix(gdo.path().file_name().unwrap()) {
        Ok(remaining) => {
            if remaining == Path::new("") {
                Some(None)
            } else {
                let r_str = remaining.to_str().unwrap();
                if (r_str.len() != 8 && r_str.len() != 17) || &r_str[..4] != "char" {
                    return None;
                }
                for uuid in gdo.get_children() {
                    if let Some(matc) = match_char(&mut gdo.get_child(&uuid).unwrap(), remaining) {
                        return Some(Some((uuid, matc)));
                    }
                }
                None
            }
        }
        Err(_) => None,
    }
}
fn match_remote_char(gdo: &mut RemoteCharBase, path: &Path) -> Option<Option<UUID>> {
    match path.strip_prefix(gdo.path().file_name().unwrap()) {
        Ok(remaining) => {
            if remaining == Path::new("") {
                Some(None)
            } else {
                for uuid in gdo.get_children() {
                    if match_object(&gdo.get_child(&uuid).unwrap(), remaining) {
                        return Some(Some(uuid));
                    }
                }
                None
            }
        }
        Err(_) => None,
    }
}
pub(crate) fn match_remote_serv(
    gdo: &mut RemoteServiceBase,
    path: &Path,
) -> Option<Option<(UUID, Option<UUID>)>>
//U: HasChildren<'b> + AttObject
{
    match path.strip_prefix(gdo.path().file_name().unwrap()) {
        Ok(remaining) => {
            if remaining == Path::new("") {
                Some(None)
            } else {
                for uuid in gdo.get_children() {
                    if let Some(matc) =
                        match_remote_char(&mut gdo.get_child(&uuid).unwrap(), remaining)
                    {
                        return Some(Some((uuid, matc)));
                    }
                }
                None
            }
        }
        Err(_) => None,
    }
}

fn match_object<T: AttObject>(gdo: &T, path: &Path) -> bool {
    gdo.path().file_name().unwrap() == path
}

/// Objects implementing this trait have child attributes.
pub trait HasChildren<'a> {
    type Child: AttObject;
    fn get_children(&self) -> Vec<UUID>;
    fn get_child<T: ToUUID>(&'a mut self, uuid: T) -> Option<Self::Child>;
}
/*
pub trait HasChildrenMut<'a>: HasChildren<'a> {
    fn get_child<T: ToUUID>(&'a mut self, uuid: T) -> Option<Self::Child>;

}
*/
impl<'a, U: HasChildren<'a>> HasChildren<'a> for &mut U {
    type Child = U::Child;
    fn get_children(&self) -> Vec<UUID> {
        U::get_children(self)
    }
    fn get_child<T: ToUUID>(&'a mut self, uuid: T) -> Option<Self::Child> {
        U::get_child(self, uuid)
    }
}

/// Use to set the value of a local characteristic or descriptor.
/// The value can be an actual value or it can be callback that returns value.
pub enum ValOrFn {
    Value(AttValue),
    Function(Box<dyn FnMut() -> AttValue>),
}
impl Default for ValOrFn {
    fn default() -> Self {
        ValOrFn::Value(AttValue::default())
    }
}
impl AsRef<AttValue> for AttValue {
    fn as_ref(&self) -> &AttValue {
        self
    }
}
impl<T: AsRef<AttValue>> From<T> for ValOrFn {
    fn from(cv: T) -> Self {
        ValOrFn::Value(*cv.as_ref())
    }
}

impl Debug for ValOrFn {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        if let ValOrFn::Value(cv) = self {
            write!(f, "ValOrFn {{ Value: {:?} }}", cv)
        } else {
            write!(f, "ValOrFn {{ Fn  }}")
        }
    }
}

impl ValOrFn {
    #[inline]
    pub fn to_value(&mut self) -> AttValue {
        match self {
            ValOrFn::Value(cv) => (*cv),
            ValOrFn::Function(f) => f(),
        }
    }
    pub fn from_slice(slice: &[u8]) -> Self {
        ValOrFn::Value(slice.into())
    }
}

/// Represents the value of a characteristic or descriptor.
#[derive(Copy)]
pub struct AttValue {
    //buf: [u8; 512],
    buf: [MaybeUninit<u8>; 512],
    len: usize,
}
impl Debug for AttValue {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), std::fmt::Error> {
        // TODO: use formmater helper functions
        f.debug_struct("AttValue")
            .field("len", &self.len)
            .field("buf", &self.as_slice())
            .finish()
    }
}
impl AttValue {
    pub fn new(len: usize) -> Self {
        assert!(len <= 512);
        let mut ret = AttValue::default();
        ret.resize(len, 0);
        ret
    }
    pub fn resize(&mut self, new_len: usize, value: u8) {
        if self.len < new_len {
            for i in &mut self.buf[self.len..new_len] {
                *i = MaybeUninit::new(value);
            }
        }
        self.len = new_len;
    }
    pub fn resize_with<F: FnMut() -> u8>(&mut self, new_len: usize, mut f: F) {
        if self.len < new_len {
            for i in &mut self.buf[self.len..new_len] {
                *i = MaybeUninit::new((f)());
            }
        }
        self.len = new_len;
    }
    pub fn as_slice(&self) -> &[u8] {
        // SAFETY: MaybeUninit<u8> has same layout as u8
        unsafe { std::mem::transmute(&self.buf[..self.len]) }
    }
    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        // SAFETY: MaybeUninit<u8> has same layout as u8
        unsafe { std::mem::transmute(&mut self.buf[..self.len]) }
    }
    pub fn update(&mut self, slice: &[u8], offset: usize) {
        assert!(offset <= self.len);
        let end = offset + slice.len();
        for (tar, src) in self.buf[offset..end].iter_mut().zip(slice) {
            *tar = MaybeUninit::new(*src);
        }
        self.len = end;
    }
    pub fn extend_from_slice(&mut self, slice: &[u8]) {
        let mut iter = slice.iter().map(|x| *x);
        self.resize_with(self.len + slice.len(), || iter.next().unwrap());
    }
}
impl Default for AttValue {
    fn default() -> Self {
        // SAFETY: assume_init() is safe for arrays if the underlying type is MaybeUninit
        let buf: [MaybeUninit<u8>; 512] = unsafe { MaybeUninit::uninit().assume_init() };
        AttValue { buf, len: 0 }
    }
}
impl Clone for AttValue {
    fn clone(&self) -> Self {
        let mut ret = AttValue::default();
        ret.extend_from_slice(self.as_slice());
        ret
    }
}
impl Borrow<[u8]> for AttValue {
    #[inline]
    fn borrow(&self) -> &[u8] {
        self.as_slice()
    }
}

/// ### Panics
/// Panics if the slice is longer than 512.
impl From<&[u8]> for AttValue {
    fn from(slice: &[u8]) -> Self {
        let mut ret = AttValue::default();
        ret.extend_from_slice(slice);
        ret
    }
}
impl Deref for AttValue {
    type Target = [u8];
    #[inline]
    fn deref(&self) -> &Self::Target {
        self.as_slice()
    }
}
impl DerefMut for AttValue {
    #[inline]
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.as_mut_slice()
    }
}
impl Signature for AttValue {
    fn signature() -> crate::signature::Type {
        <[u8]>::signature()
    }
    fn alignment() -> usize {
        <[u8]>::alignment()
    }
}
impl<'r, 'buf: 'r, 'fds> Unmarshal<'r, 'buf, 'fds> for AttValue {
    fn unmarshal(ctx: &mut UnmarshalContext<'fds, 'buf>) -> UnmarshalResult<Self> {
        let (used, buf): (usize, &'r [u8]) = <&'r [u8]>::unmarshal(ctx)?;
        if buf.len() > 512 {
            Err(unmarshal::Error::InvalidType)
        } else {
            Ok((used, buf.into()))
        }
    }
}
impl Marshal for AttValue {
    fn marshal(&self, ctx: &mut MarshalContext) -> Result<(), rustbus::Error> {
        self.as_slice().marshal(ctx)
    }
}

#[cfg(test)]
mod tests {
    use crate::gatt::{match_char, match_serv};
    use crate::gatt::{CharFlags, DescFlags, LocalCharBase, LocalDescBase, LocalServiceBase};
    use crate::ToUUID;
    use std::path::{Path, PathBuf};
    #[test]
    fn test_match_char() {
        let desc1_uuid = "00000000-0000-0000-0000-000000000001".to_uuid();
        let mut desc1 = LocalDescBase::new(&desc1_uuid, DescFlags::default());
        desc1.path = PathBuf::from("char0001/desc0001");

        let char1_uuid = "00000000-0000-0000-0001-000000000000".to_uuid();
        let mut char1 = LocalCharBase::new(&char1_uuid, CharFlags::default());
        char1.path = PathBuf::from("char0001");
        char1.add_desc(desc1);

        assert_eq!(match_char(&mut char1, Path::new("char0001")), Some(None));
        assert_eq!(
            match_char(&mut char1, Path::new("char0001/desc0001")),
            Some(Some(desc1_uuid))
        );
        assert_eq!(match_char(&mut char1, Path::new("char0002")), None);
        assert_eq!(match_char(&mut char1, Path::new("char0001/desc0002")), None);
    }
    #[test]
    fn test_match_serv() {
        let desc1_uuid = "00000000-0000-0000-0000-000000000001".to_uuid();
        let mut desc1 = LocalDescBase::new(&desc1_uuid, DescFlags::default());
        desc1.path = PathBuf::from("serv0001/char0001/desc0001");

        let char1_uuid = "00000000-0000-0000-0001-000000000000".to_uuid();
        let mut char1 = LocalCharBase::new(&char1_uuid, CharFlags::default());
        char1.path = PathBuf::from("serv0001/char0001");
        char1.add_desc(desc1);

        let serv1_uuid = "00000000-0000-0001-0000-000000000000".to_uuid();
        let mut serv1 = LocalServiceBase::new(&serv1_uuid, true);
        serv1.path = PathBuf::from("serv0001");
        serv1.add_char(char1);
        assert_eq!(match_serv(&mut serv1, Path::new("serv0001")), Some(None));
        assert_eq!(match_serv(&mut serv1, Path::new("char0001")), None);
        assert_eq!(
            match_serv(&mut serv1, Path::new("serv0001/char0001")),
            Some(Some((char1_uuid.clone(), None)))
        );
        assert_eq!(
            match_serv(&mut serv1, Path::new("serv000001/char0002")),
            None
        );
        assert_eq!(
            match_serv(&mut serv1, Path::new("serv0001/char0001/desc0001")),
            Some(Some((char1_uuid.clone(), Some(desc1_uuid))))
        );
        assert_eq!(
            match_serv(&mut serv1, Path::new("serv0001/char0001/desc0002")),
            None
        );
    }
}
