use async_rustbus::rustbus_core;
use async_rustbus::RpcConn;
use rustbus_core::message_builder::MessageBuilder;
use rustbus_core::signature::{Base, Type};
use rustbus_core::wire::marshal::traits::{Marshal, Signature};
use rustbus_core::wire::marshal::MarshalContext;
use rustbus_core::wire::unmarshal::traits::Unmarshal;
use rustbus_core::wire::unmarshal::Error as UnmarshalError;
use rustbus_core::wire::unmarshal::{UnmarshalContext, UnmarshalResult};

use std::borrow::{Borrow, ToOwned};
use std::convert::TryFrom;
use std::ffi::{OsStr, OsString};
use std::fmt::{Display, Formatter};
use std::hash::{Hash, Hasher};
use std::ops::Deref;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf, StripPrefixError};
use std::str::FromStr;

#[derive(Debug)]
pub enum InvalidObjectPath {
    NoRoot,
    ContainsInvalidCharacters,
    TrailingSlash,
}

#[derive(PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct ObjectPath {
    inner: Path,
}
impl ObjectPath {
    fn validate<P: AsRef<Path>>(path: P) -> Result<(), InvalidObjectPath> {
        let path = path.as_ref();
        if !path.has_root() {
            return Err(InvalidObjectPath::NoRoot);
        }
        let path_str = path
            .to_str()
            .ok_or(InvalidObjectPath::ContainsInvalidCharacters)?;
        let mut last_was_sep = false;
        for character in path_str.chars() {
            if !(character.is_ascii_alphanumeric() || character == '_') {
                if character == '/' && !last_was_sep {
                    last_was_sep = true;
                } else {
                    return Err(InvalidObjectPath::ContainsInvalidCharacters);
                }
            } else {
                last_was_sep = false;
            }
        }
        if path_str.len() != 1 && path_str.chars().last().unwrap() == '/' {
            return Err(InvalidObjectPath::TrailingSlash);
        }
        Ok(())
    }
    fn debug_assert_validitity(&self) {
        #[cfg(debug_assertions)]
        Self::validate(self).expect("Failed to validate the object path!");
    }
    pub fn new<P: AsRef<Path> + ?Sized>(p: &P) -> Result<&ObjectPath, InvalidObjectPath> {
        let path = p.as_ref();
        Self::validate(path)?;
        Ok(unsafe { Self::new_no_val(path) })
    }
    unsafe fn new_no_val(p: &Path) -> &ObjectPath {
        &*(p as *const Path as *const ObjectPath)
    }
    pub fn as_bytes(&self) -> &[u8] {
        self.inner.as_os_str().as_bytes()
    }
    pub fn as_str(&self) -> &str {
        self.debug_assert_validitity();

        let bytes = self.as_bytes();
        unsafe { std::str::from_utf8_unchecked(bytes) }
    }
    pub fn strip_prefix<P: AsRef<Path> + ?Sized>(
        &self,
        p: &P,
    ) -> Result<&ObjectPath, StripPrefixError> {
        let stripped = self.inner.strip_prefix(p.as_ref())?;
        if stripped == Path::new("") {
            Ok(unsafe { ObjectPath::new_no_val("/".as_ref()) })
        } else {
            // Get a stripped path that includes the leading seperator.
            // This leading seperator is exists because ObjectPath.inner must be absolute;
            let self_bytes = self.as_bytes();
            let self_len = self_bytes.len(); // Unix-only
            let stripped_len = stripped.as_os_str().len();
            let ret_bytes = &self_bytes[self_len - 1 - stripped_len..];

            // convert bytes to ObjectPath
            let ret = OsStr::from_bytes(ret_bytes);
            let ret = unsafe { ObjectPath::new_no_val(ret.as_ref()) };
            ret.debug_assert_validitity();
            Ok(ret)
        }
    }
    pub fn parent(&self) -> Option<&ObjectPath> {
        let pp = self.inner.parent()?;
        let ret = unsafe { Self::new_no_val(pp) };
        ret.debug_assert_validitity();
        Some(ret)
    }
    pub fn file_name(&self) -> Option<&str> {
        let bytes = self.inner.file_name()?.as_bytes();
        self.debug_assert_validitity();
        unsafe { Some(std::str::from_utf8_unchecked(bytes)) }
    }
    pub fn root_path() -> &'static Self {
        unsafe { ObjectPath::new_no_val("/".as_ref()) }
    }
}
impl Display for ObjectPath {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}
impl Display for ObjectPathBuf {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        self.deref().fmt(f)
    }
}
impl Marshal for &ObjectPath {
    fn marshal(&self, ctx: &mut MarshalContext) -> Result<(), async_rustbus::rustbus_core::Error> {
        self.as_str().marshal(ctx)
    }
}
impl Marshal for ObjectPathBuf {
    fn marshal(&self, ctx: &mut MarshalContext) -> Result<(), async_rustbus::rustbus_core::Error> {
        self.deref().marshal(ctx)
    }
}

impl Deref for ObjectPath {
    type Target = std::path::Path;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}
impl ToOwned for ObjectPath {
    type Owned = ObjectPathBuf;
    fn to_owned(&self) -> Self::Owned {
        ObjectPathBuf::from(self)
    }
}

impl Borrow<Path> for ObjectPath {
    fn borrow(&self) -> &Path {
        self.deref()
    }
}
impl AsRef<Path> for ObjectPath {
    fn as_ref(&self) -> &Path {
        self.deref()
    }
}
impl AsRef<ObjectPath> for ObjectPath {
    fn as_ref(&self) -> &ObjectPath {
        self
    }
}
impl AsRef<str> for ObjectPath {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}
impl AsRef<OsStr> for ObjectPath {
    fn as_ref(&self) -> &OsStr {
        self.inner.as_ref()
    }
}

impl<'a> From<&'a ObjectPath> for &'a str {
    fn from(path: &'a ObjectPath) -> Self {
        path.as_str()
    }
}

impl Signature for &ObjectPath {
    fn signature() -> Type {
        Type::Base(Base::ObjectPath)
    }
    fn alignment() -> usize {
        Self::signature().get_alignment()
    }
}

impl<'buf, 'fds> Unmarshal<'buf, 'fds> for &'buf ObjectPath {
    fn unmarshal(ctx: &mut UnmarshalContext<'fds, 'buf>) -> UnmarshalResult<Self> {
        let (bytes, val) = <&str>::unmarshal(ctx)?;
        let path = ObjectPath::new(val).map_err(|_| UnmarshalError::InvalidType)?;
        Ok((bytes, path))
    }
}
impl Signature for ObjectPathBuf {
    fn signature() -> Type {
        <&ObjectPath>::signature()
    }
    fn alignment() -> usize {
        <&ObjectPath>::alignment()
    }
}
impl<'buf, 'fds> Unmarshal<'buf, 'fds> for ObjectPathBuf {
    fn unmarshal(ctx: &mut UnmarshalContext<'fds, 'buf>) -> UnmarshalResult<Self> {
        <&ObjectPath>::unmarshal(ctx).map(|(size, op)| (size, op.to_owned()))
    }
}

#[derive(PartialEq, Eq, PartialOrd, Ord, Clone, Debug)]
pub struct ObjectPathBuf {
    inner: Option<PathBuf>,
}
impl Hash for ObjectPathBuf {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.deref().hash(state);
    }
}
impl ObjectPathBuf {
    pub fn new() -> ObjectPathBuf {
        ObjectPathBuf { inner: None }
    }
    pub fn with_capacity(capacity: usize) -> ObjectPathBuf {
        let inner = if capacity == 0 {
            None
        } else {
            let mut pb = PathBuf::with_capacity(capacity);
            pb.push("/");
            Some(pb)
        };
        ObjectPathBuf { inner }
    }
    pub fn as_object_path(&self) -> &ObjectPath {
        self.deref()
    }
    unsafe fn from_path_buf(pb: PathBuf) -> Self {
        Self { inner: Some(pb) }
    }
    pub fn clear(&mut self) {
        if let Some(buf) = &mut self.inner {
            buf.clear();
        }
    }
    pub fn push(&mut self, path: &ObjectPath) {
        let path: &Path = path.as_ref();
        let stripped = path.strip_prefix("/").unwrap();
        let len = stripped.as_os_str().len();
        if len == 0 {
            return;
        }
        if let None = self.inner {
            *self = Self::with_capacity(len + 1);
        }
        let buf = self.inner.as_mut().unwrap();
        buf.push(stripped);
    }
    pub fn pop(&mut self) -> bool {
        match &mut self.inner {
            Some(pb) => pb.pop(),
            None => false,
        }
    }
    pub fn reserve(&mut self, additional: usize) {
        if additional == 0 {
            return;
        }
        match &mut self.inner {
            Some(buf) => buf.reserve(additional),
            None => *self = Self::with_capacity(additional + 1),
        }
    }
    pub fn reserve_exact(&mut self, additional: usize) {
        if additional == 0 {
            return;
        }
        match &mut self.inner {
            Some(buf) => buf.reserve_exact(additional),
            None => {
                let mut buf = PathBuf::new();
                buf.reserve_exact(additional + 1);
                self.inner = Some(buf);
            }
        }
    }
}
impl TryFrom<OsString> for ObjectPathBuf {
    type Error = InvalidObjectPath;
    fn try_from(value: OsString) -> Result<Self, Self::Error> {
        ObjectPath::validate(&value)?;
        Ok(unsafe { ObjectPathBuf::from_path_buf(value.into()) })
    }
}
impl TryFrom<String> for ObjectPathBuf {
    type Error = InvalidObjectPath;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::try_from(OsString::from(value))
    }
}
impl Deref for ObjectPathBuf {
    type Target = ObjectPath;
    fn deref(&self) -> &Self::Target {
        match &self.inner {
            Some(buf) => unsafe { ObjectPath::new_no_val(&buf) },
            None => ObjectPath::root_path(),
        }
    }
}
impl Borrow<ObjectPath> for ObjectPathBuf {
    fn borrow(&self) -> &ObjectPath {
        self.deref()
    }
}
impl AsRef<ObjectPath> for ObjectPathBuf {
    fn as_ref(&self) -> &ObjectPath {
        self.deref()
    }
}
impl AsRef<str> for ObjectPathBuf {
    fn as_ref(&self) -> &str {
        self.deref().as_ref()
    }
}
impl FromStr for ObjectPathBuf {
    type Err = InvalidObjectPath;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let path: &Path = s.as_ref();
        let obj_path = ObjectPath::new(path)?;
        Ok(obj_path.to_owned())
    }
}
impl From<ObjectPathBuf> for PathBuf {
    fn from(buf: ObjectPathBuf) -> Self {
        match buf.inner {
            Some(buf) => buf,
            None => PathBuf::from("/"),
        }
    }
}
impl From<ObjectPathBuf> for String {
    fn from(path: ObjectPathBuf) -> Self {
        path.debug_assert_validitity();
        let bytes = match path.inner {
            Some(buf) => buf.into_os_string().into_vec(),
            None => Vec::from(&b"/"[..]),
        };

        unsafe { std::string::String::from_utf8_unchecked(bytes) }
    }
}
impl From<&ObjectPath> for ObjectPathBuf {
    fn from(path: &ObjectPath) -> Self {
        let ret = if path == ObjectPath::root_path() {
            ObjectPathBuf::new()
        } else {
            unsafe {
                ObjectPathBuf::from_path_buf(path.into())
            }
        };
        ret.debug_assert_validitity();
        ret
    }
}

impl PartialEq<ObjectPath> for ObjectPathBuf {
    fn eq(&self, other: &ObjectPath) -> bool {
        self.deref().eq(other)
    }
}
#[cfg(test)]
mod tests {
    use super::{ObjectPath, ObjectPathBuf};
    use std::borrow::Borrow;
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    use std::path::Path;
    use std::str::FromStr;
    fn test_objpaths() -> Vec<&'static ObjectPath> {
        vec![
            ObjectPath::new("/org/freedesktop/NetworkManager").unwrap(),
            ObjectPath::new("/org/freedesktop/NetworkManager/ActiveConnection").unwrap(),
        ]
    }
    fn test_objpathbufs() -> Vec<ObjectPathBuf> {
        test_objpaths()
            .into_iter()
            .map(|op| op.to_owned())
            .collect()
    }
    // Borrow requires that the Ord trait prodces equivelent values before and after
    fn compare_ord_borrow<A, T, U>(pre: &[A]) -> Option<(usize, usize)>
    where
        T: Ord + Borrow<U> + ?Sized,
        U: Ord + ?Sized,
        A: Borrow<T>,
    {
        let pre_iter = pre.iter().map(|p| p.borrow());
        for (i, pre_i) in pre_iter.clone().enumerate() {
            for (j, pre_j) in pre_iter.clone().enumerate().skip(i) {
                let pre_ord = pre_i.cmp(pre_j);
                let post_i = pre_i.borrow();
                let post_j = pre_j.borrow();
                let post_ord = post_i.cmp(post_j);
                if pre_ord != post_ord {
                    return Some((i, j));
                }
            }
        }
        None
    }
    // Borrow requires that Hash trait produces equivelent values for before and after borrow()
    // This tests that invariant
    fn compare_hasher_borrow<A, T, U>(pre: &[A]) -> Option<usize>
    where
        T: Hash + Borrow<U> + ?Sized,
        U: Hash + ?Sized,
        A: Borrow<T>,
    {
        let pre_iter = pre.iter().map(|p| p.borrow());
        for (i, (pre, post)) in pre_iter
            .clone()
            .zip(pre_iter.map(|p| p.borrow()))
            .enumerate()
        {
            let mut pre_borrow_hasher = DefaultHasher::new();
            let mut post_borrow_hasher = DefaultHasher::new();
            pre.hash(&mut pre_borrow_hasher);
            post.hash(&mut post_borrow_hasher);
            if pre_borrow_hasher.finish() != post_borrow_hasher.finish() {
                return Some(i);
            }
        }
        None
    }
    #[test]
    fn test_objectpathbuf_borrow_objectpath() {
        let objpathbufs = test_objpathbufs();
        if let Some(i) =
            compare_hasher_borrow::<ObjectPathBuf, ObjectPathBuf, ObjectPath>(&objpathbufs[..])
        {
            panic!("Hash didn't match: {}", i);
        }
        if let Some((i, j)) =
            compare_ord_borrow::<ObjectPathBuf, ObjectPathBuf, ObjectPath>(&objpathbufs[..])
        {
            panic!("Ord didn't match for: {} {}", i, j);
        }
    }
    #[test]
    fn test_objectpath_borrow_path() {
        let objpaths = test_objpaths();
        if let Some(i) = compare_hasher_borrow::<&ObjectPath, ObjectPath, Path>(&objpaths[..]) {
            panic!("Hash didn't match: {}", i);
        }
        if let Some((i, j)) = compare_ord_borrow::<&ObjectPath, ObjectPath, Path>(&objpaths[..]) {
            panic!("Ord didn't match for: {} {}", i, j);
        }
    }
    #[test]
    fn test_push() {
        let objpath = ObjectPath::new("/dbus/test").unwrap();
        let objpath2 = ObjectPath::new("/freedesktop/more").unwrap();
        let mut objpathbuf = ObjectPathBuf::new();
        objpathbuf.push(objpath);
        assert_eq!(objpathbuf, *objpath);
        objpathbuf.push(objpath2);
        assert_eq!(
            objpathbuf,
            *ObjectPath::new("/dbus/test/freedesktop/more").unwrap()
        );
        assert!(objpathbuf.starts_with(objpath));
        assert!(!objpathbuf.starts_with(objpath2));
        assert_eq!(objpathbuf.strip_prefix(objpath).unwrap(), objpath2);
    }
}

pub struct Child {
    path: ObjectPathBuf,
    interface: Vec<String>,
}
impl Child {
    pub fn path(&self) -> &ObjectPath {
        &self.path
    }
    pub fn interfaces(&self) -> &[String] {
        &self.interface[..]
    }
}
impl From<Child> for ObjectPathBuf {
    fn from(child: Child) -> Self {
        child.path
    }
}

pub async fn get_children<S: AsRef<str>, P: AsRef<ObjectPath>>(
    conn: &RpcConn,
    dest: S,
    path: P,
) -> std::io::Result<Vec<Child>> {
    use xml::reader::EventReader;
    let path: &str = path.as_ref().as_ref();
    let call = MessageBuilder::new()
        .call(String::from("Introspect"))
        .with_interface(String::from("org.freedesktop.DBus.Introspectable"))
        .on(path.to_string())
        .at(dest.as_ref().to_string())
        .build();
    let res = conn.send_msg_with_reply(&call).await?.await?;
    let s: &str = res.body.parser().get().unwrap();
    let mut reader = EventReader::from_str(s);
    eprintln!("get_children: {:?}", reader.next());
    unimplemented!()
}
