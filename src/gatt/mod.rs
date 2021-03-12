use std::borrow::Borrow;
use std::fmt::{Debug, Formatter};
use std::mem::MaybeUninit;
use std::ops::{Deref, DerefMut};

use async_rustbus::rustbus_core;
use marshal::traits::{Marshal, Signature};
use marshal::MarshalContext;
use rustbus_core::signature;
use rustbus_core::wire::{marshal, unmarshal};
use unmarshal::traits::Unmarshal;
use unmarshal::{UnmarshalContext, UnmarshalResult};

pub mod client;
pub mod server;

/// Represents the value of a characteristic or descriptor.
pub struct AttValue {
    //buf: [u8; 512],
    buf: [MaybeUninit<u8>; 512],
    len: usize,
}
impl Debug for AttValue {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        // TODO: use formmater helper functions
        let slice = &self.buf[..self.len];
        write!(f, "AttValue {{")?;
        slice.fmt(f)?;
        write!(f, "}}")
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
    fn signature() -> signature::Type {
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
    fn marshal(&self, ctx: &mut MarshalContext) -> Result<(), rustbus_core::Error> {
        self.as_slice().marshal(ctx)
    }
}

/// Flags for GATT characteristics.
///
/// What each flags does is detailed on
/// page 1552 (Table 3.5) and page 1554 (Table 3.8) of the [Core Specification (5.2)]
///
/// [Core Specification (5.2)]: https://www.bluetooth.com/specifications/bluetooth-core-specification/
#[derive(Clone, Copy, Default, Debug)]
pub struct CharFlags {
    pub broadcast: bool,
    pub read: bool,
    pub write_wo_response: bool,
    pub write: bool,
    pub notify: bool,
    pub indicate: bool,
    pub auth_signed_writes: bool,
    pub extended_properties: bool,
    pub reliable_write: bool,
    pub writable_auxiliaries: bool,
    pub encrypt_read: bool,
    pub encrypt_write: bool,
    pub encrypt_auth_read: bool,
    pub encrypt_auth_write: bool,
    pub secure_read: bool,
    pub secure_write: bool,
    pub authorize: bool,
}
impl CharFlags {
    fn to_strings(&self) -> Vec<&'static str> {
        let mut ret = Vec::new();
        if self.broadcast {
            ret.push("broadcast");
        }
        if self.read {
            ret.push("read");
        }
        if self.write {
            ret.push("write")
        }
        if self.write_wo_response {
            ret.push("write-without-response");
        }
        if self.notify {
            ret.push("notify");
        }
        if self.indicate {
            ret.push("indicate");
        }
        if self.auth_signed_writes {
            unimplemented!();
            ret.push("authenticated-signed-writes");
        }
        if self.extended_properties {
            ret.push("extended-properties");
        }
        if self.reliable_write {
            ret.push("reliable-write");
        }
        if self.writable_auxiliaries {
            unimplemented!();
            ret.push("writable-auxiliaries");
        }
        if self.encrypt_read {
            ret.push("encrypt-read");
        }
        if self.encrypt_write {
            ret.push("encrypt-write");
        }
        if self.encrypt_auth_read {
            ret.push("encrypt-authenticated-read");
        }
        if self.encrypt_auth_write {
            ret.push("encrypt-authenticated-write");
        }
        if self.secure_write {
            ret.push("secure-write");
        }
        if self.secure_read {
            ret.push("secure-read");
        }
        if self.authorize {
            unimplemented!();
            ret.push("authorize");
        }
        ret
    }
    fn from_strings<'a, I>(flags: I) -> CharFlags
    where
        I: IntoIterator<Item = &'a str>,
    {
        let mut ret = CharFlags::default();
        for flag in flags {
            match flag {
                "broadcast" => ret.broadcast = true,
                "read" => ret.read = true,
                "write" => ret.write = true,
                "write-without-response" => ret.write_wo_response = true,
                "notify" => ret.notify = true,
                "indicate" => ret.indicate = true,
                "authenticated-signed-writes" => ret.auth_signed_writes = true,
                "extended-properties" => ret.extended_properties = true,
                "reliable-write" => ret.reliable_write = true,
                "writable-auxiliaries" => ret.writable_auxiliaries = true,
                "encrypt-read" => ret.encrypt_read = true,
                "encrypt-write" => ret.encrypt_write = true,
                "encrypt-authenticated-read" => ret.encrypt_auth_read = true,
                "encrypt-authenticated-write" => ret.encrypt_auth_write = true,
                "secure-write" => ret.secure_write = true,
                "secure-read" => ret.secure_read = true,
                "authorize" => ret.authorize = true,
                _ => unreachable!(),
            }
        }
        ret
    }
}
/// Use to set the value of a local characteristic or descriptor.
/// The value can be an actual value or it can be callback that returns value.
pub enum ValOrFn {
    Value(AttValue),
    Function(Box<dyn FnMut() -> AttValue + Send + 'static>),
}
impl Default for ValOrFn {
    fn default() -> Self {
        ValOrFn::Value(AttValue::default())
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
            ValOrFn::Value(cv) => cv.clone(),
            ValOrFn::Function(f) => f(),
        }
    }
    pub fn from_slice(slice: &[u8]) -> Self {
        ValOrFn::Value(slice.into())
    }
}
