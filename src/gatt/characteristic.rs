use crate::interfaces::*;
use crate::introspect::*;
use crate::*;
use nix::errno::Errno;
use nix::sys::socket;
use nix::sys::time::{TimeVal, TimeValLike};
use nix::sys::uio::IoVec;
use nix::unistd::close;
use rustbus::params::{Base, Container, Param};
use rustbus::wire::marshal::traits::UnixFd;
use std::convert::TryFrom;
use std::os::unix::io::RawFd;

/// Describes the methods avaliable on GATT characteristics.
pub trait Characteristic {
	/// Reads the value of a GATT characteristic.
    fn read(&mut self) -> Result<([u8; 512], usize), Error>;
	/// Generally returns a previous value of the GATT characteristic. Check the individual implementors,
	/// for a more precise definition.
    fn read_cached(&mut self) -> Result<([u8; 512], usize), Error>;
	/// Write a value to a GATT characteristic.
    fn write(&mut self, val: &[u8]) -> Result<(), Error>;
	/// Get the UUID of the service.
	/// TODO: change to UUID (Rc<str>)
    fn uuid(&self) -> &str;
    //    fn service(&self) -> &Path;
	/// Checks if the characteristic's write fd from [`AcquireWrite`] has already been acquired.
	/// Corresponds to reading [`WriteAcquired`] property.
	///
	/// [`AcquireWrite`]: https://git.kernel.org/pub/scm/bluetooth/bluez.git/tree/doc/gatt-api.txt#n115
	/// [`WriteAcquired`]: https://git.kernel.org/pub/scm/bluetooth/bluez.git/tree/doc/gatt-api.txt#n223
    fn write_acquired(&self) -> bool;
	/// Checks if the characteristic's notify fd from [`AcquireNotify`] has already been acquired.
	/// Corresponds to reading [`NotifyAcquired`] property.
	///
	/// [`AcquireNotify`]: https://git.kernel.org/pub/scm/bluetooth/bluez.git/tree/doc/gatt-api.txt#n145
	/// [`NotifyAcquired`]: https://git.kernel.org/pub/scm/bluetooth/bluez.git/tree/doc/gatt-api.txt#n234
    fn notify_acquired(&self) -> bool;
	/// Checks if the [`StartNotify`] command has been called on a characteristic.
	/// Corresponds to reading [`Notifying`] property.
	///
	/// [`StartNotify`]: https://git.kernel.org/pub/scm/bluetooth/bluez.git/tree/doc/gatt-api.txt#n181
	/// [`Notifying`]: https://git.kernel.org/pub/scm/bluetooth/bluez.git/tree/doc/gatt-api.txt#n245
    fn notifying(&self) -> bool;
	/// Reads the flags present on a characteristic.
	/// Corresponds to reading [`Flags`] property.
	///
	/// [`Flags`]: https://git.kernel.org/pub/scm/bluetooth/bluez.git/tree/doc/gatt-api.txt#n250 
    fn flags(&self) -> CharFlags;
}

#[derive(Debug)]
enum Notify {
    Signal,
    Fd(RawFd),
}

/// `LocalCharBase` is used to create GATT characteristics to be added to `LocalServiceBase`.
pub struct LocalCharBase {
    vf: ValOrFn,
    pub(crate) index: u16,
    handle: u16,
    pub(crate) uuid: UUID,
    pub(crate) serv_uuid: UUID,
    pub(crate) path: PathBuf,
    notify: Option<Notify>,
    write: Option<RawFd>,
    pub(crate) descs: HashMap<String, LocalDescriptor>,
    flags: CharFlags,
    allow_write: bool,
	/// Set a callback that can be when writes are issued by remote device.
	/// The callback function can reject a write with an error, with first String being a general a DBus,
	/// error name, and the Optional second string being an extended error message.=
	/// On a successful write, giving a `Some` variant will overwrite the value of the characteristic,
	/// while `None` leaves the value the same as it was before the write. The purpose of this allows,
	/// the user to change the ValOrFn before it is set the characteristic, for others to use.
	/// The `bool` is used to indicate whether an notification/indication should be issued after an update.
    pub write_callback:
        Option<Box<dyn FnMut(&[u8]) -> Result<(Option<ValOrFn>, bool), (String, Option<String>)>>>,
}
impl Debug for LocalCharBase {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), std::fmt::Error> {
        let wc_str = if let Some(_) = self.write_callback {
            "Some(FnMut)"
        } else {
            "None"
        };
        write!(f, "LocalCharBase{{vf: {:?}, index: {:?}, handle: {:?}, uuid: {:?}, path: {:?}, notify: {:?}, write: {:?}, descs: {:?}, flags: {:?}, allow_write: {:?}, write_callback: {}}}", self.vf, self.index, self.handle, self.uuid, self.path, self.notify, self.write, self.descs, self.flags, self.allow_write, wc_str)
    }
}
impl Drop for LocalCharBase {
    fn drop(&mut self) {
        if let Some(Notify::Fd(fd)) = self.notify {
            close(fd).ok(); // ignore error
        }
        if let Some(fd) = self.write {
            close(fd).ok();
        }
    }
}
impl LocalCharBase {
	/// Enables `AcquireWrite` DBus call to be issued by Bluez to the local application. 
	/// [`AcquireWrite`] will allow Bluez to issue writes to the local characteristic, using
	/// packets over a Unix socket. This can have better performance and lower latency by allowing
	/// writes to avoid using DBus. If this is used, then implementors of local characteristic need,
	/// to periodically call [`LocalCharactersitic::check_write_fd()`] to process these messages, received
	/// on the socket, onces added.
    pub fn enable_write_fd(&mut self, on: bool) {
        self.allow_write = on;
        if !on {
            if let Some(write_fd) = self.write {
                close(write_fd).ok();
                self.write = None;
            }
        }
    }
    pub(super) fn update_path(&mut self, base: &Path) {
        self.path = base.to_owned();
        let mut name = String::with_capacity(8);
        write!(&mut name, "char{:04x}", self.index).unwrap();
        self.path.push(name);
        for desc in self.descs.values_mut() {
            desc.update_path(&self.path);
        }
    }

    pub(super) fn match_descs(
        &mut self,
        _msg_path: &Path,
        _header: &DynamicHeader,
    ) -> Option<DbusObject> {
        unimplemented!()
    }
	/// Creates a new `LocalCharBase` with `uuid` and `flags`.
	///
	/// It can be added a local service with [`LocalServiceBase::add_char()`].
	///
	/// [`LocalServiceBase::add_char()`]: ./struct.LocalServiceBase.html#method.new
    pub fn new<T: ToUUID>(uuid: T, flags: CharFlags) -> Self {
        let uuid: UUID = uuid.to_uuid();
        LocalCharBase {
            vf: ValOrFn::Value([0; 512], 0),
            index: 0,
            handle: 0,
            write: None,
            notify: None,
            uuid,
            flags,
            path: PathBuf::new(),
            descs: HashMap::new(),
            allow_write: false,
            write_callback: None,
            serv_uuid: Rc::from(""),
        }
    }
}

pub struct LocalCharactersitic<'a, 'b: 'a> {
    pub(crate) uuid: UUID,
    pub(super) service: &'a mut LocalService<'b>,
    #[cfg(feature = "unsafe-opt")]
    base: *mut LocalCharBase,
}
impl<'c, 'd> LocalCharactersitic<'c, 'd> {
    pub(crate) fn new(service: &'c mut LocalService<'d>, uuid: UUID) -> Self {
        // TODO: implement base for cfg unsafe-opt
        LocalCharactersitic { uuid, service }
    }
    pub(crate) fn char_call<'a, 'b>(&mut self, call: MarshalledMessage) -> MarshalledMessage {
        let base = self.get_char_base_mut();
        match &call.dynheader.member.as_ref().unwrap()[..] {
            "ReadValue" => {
                if base.flags.read
                    || base.flags.secure_read
                    || base.flags.secure_read
                    || base.flags.encrypt_read
                {
                    self.check_write_fd();
                    let base = self.get_char_base_mut();
                    let call = call.unmarshall_all().unwrap();
                    let (v, l) = base.vf.to_value();
                    let mut start = 0;
                    if let Some(dict) = call.params.get(0) {
                        if let Param::Container(Container::Dict(dict)) = dict {
                            if let Some(offset) = dict.map.get(&Base::String("offset".to_string()))
                            {
                                if let Param::Container(Container::Variant(offset)) = offset {
                                    if let Param::Base(Base::Uint16(offset)) = offset.value {
                                        start = l.min(offset as usize);
                                    } else {
                                        return call.dynheader.make_error_response(
                                            "UnexpectedType".to_string(),
                                            Some(
                                                "Expected a dict of variants as first parameter"
                                                    .to_string(),
                                            ),
                                        );
                                    }
                                } else {
                                    return call.dynheader.make_error_response(
                                        "UnexpectedType".to_string(),
                                        Some(
                                            "Expected a dict of variants as first parameter"
                                                .to_string(),
                                        ),
                                    );
                                }
                            }
                        } else {
                            return call.dynheader.make_error_response(
                                "UnexpectedType".to_string(),
                                Some("Expected a dict as first parameter".to_string()),
                            );
                        }
                    }
                    // eprintln!("vf: {:?}\nValue: {:?}", base.vf, &v[..l]);
                    let vec: Vec<Param> = v[start..l]
                        .into_iter()
                        .map(|i| Base::Byte(*i).into())
                        .collect();
                    let val = Param::Container(Container::Array(params::Array {
                        element_sig: signature::Type::Base(signature::Base::Byte),
                        values: vec,
                    }));
                    let mut res = call.make_response();
                    res.body.push_old_param(&val).unwrap();
                    res
                } else {
                    call.dynheader.make_error_response(
                        BLUEZ_NOT_PERM.to_string(),
                        Some("This is not a readable characteristic.".to_string()),
                    )
                }
            }
            "WriteValue" => {
                if base.flags.write
                    || base.flags.write_wo_response
                    || base.flags.secure_write
                    || base.flags.encrypt_write
                    || base.flags.encrypt_auth_write
                {
                    self.check_write_fd();
                    let base = self.get_char_base_mut();
                    let call = call.unmarshall_all().unwrap();
                    if let Some(Param::Container(Container::Array(array))) = call.params.get(0) {
                        let offset = if let Some(dict) = call.params.get(1) {
                            let mut offset = 0;
                            if let Param::Container(Container::Dict(dict)) = dict {
                                for (key, val) in &dict.map {
                                    let var = if let Param::Container(Container::Variant(var)) = val
                                    {
                                        var
                                    } else {
                                        return call.dynheader.make_error_response("org.bluez.Error.Failed".to_string(), Some("Second parameter was wrong type, expected variant values".to_string()));
                                    };
                                    if let Base::String(key) = key {
                                        match &key[..] {
                                            "offset" => {
                                                if let Param::Base(Base::Uint16(off)) = var.value {
                                                    offset = off;
                                                } else {
                                                    return call.dynheader.make_error_response(
                                                        "org.bluez.Error.Failed".to_string(),
                                                        Some(
                                                            "Expected offset key to be u16 value"
                                                                .to_string(),
                                                        ),
                                                    );
                                                }
                                            }
                                            _ => {}
                                        }
                                    } else {
                                        return call.dynheader.make_error_response("org.bluez.Error.Failed".to_string(), Some("Second parameter was wrong type, expected string keys".to_string()));
                                    }
                                }
                                offset as usize
                            } else {
                                return call.dynheader.make_error_response(
                                    "org.bluez.Error.Failed".to_string(),
                                    Some("Second parameter was wrong type".to_string()),
                                );
                            }
                        } else {
                            0
                        };
                        let l = array.values.len() + offset;
                        if l > 512 {
                            return call.dynheader.make_error_response(
                                "org.bluez.Error.InvalidValueLength".to_string(),
                                None,
                            );
                        }
                        let mut bytes = Vec::with_capacity(array.values.len());
                        for val in &array.values {
                            if let Param::Base(Base::Byte(b)) = val {
                                bytes.push(*b);
                            } else {
                                return call.dynheader.make_error_response(
                                    "org.bluez.Error.Failed".to_string(),
                                    Some("First parameter was wrong type".to_string()),
                                );
                            }
                        }
                        let mut v = [0; 512];
                        let (cur_v, cur_l) = base.vf.to_value();
                        v[..cur_l].copy_from_slice(&cur_v[..cur_l]);
                        v[offset..l].copy_from_slice(&bytes[..]);
                        if let Some(cb) = &mut base.write_callback {
                            match cb(&v[..l]) {
                                Ok((vf, notify)) => {
                                    if let Some(vf) = vf {
                                        base.vf = vf;
                                    }
                                    if notify {
                                        // TODO: is there a better way to handle this error?
                                        if let Err(e) = self.notify() {
                                            eprintln!(
                                                "Failed to notify characteristic on change: {:?}",
                                                e
                                            );
                                        }
                                    }
                                }
                                Err((s1, s2)) => {
                                    return call.dynheader.make_error_response(s1, s2);
                                }
                            }
                        }
                        call.dynheader.make_response()
                    } else {
                        call.dynheader.make_error_response(
                            "org.bluez.Error.Failed".to_string(),
                            Some("First parameter was wrong type".to_string()),
                        )
                    }
                } else {
                    call.dynheader.make_error_response(
                        BLUEZ_NOT_PERM.to_string(),
                        Some("This is not a writable characteristic.".to_string()),
                    )
                }
            }
            "AcquireWrite" => {
                if !base.allow_write {
                    return call
                        .dynheader
                        .make_error_response("org.bluez.Error.NotSupported".to_string(), None);
                }
                if base.flags.write {
                    if let Some(_) = base.write {
                        return call.dynheader.make_error_response(
                            "org.bluez.Error.InProgress".to_string(),
                            Some(
                                "This characteristic write fd has already been acquired."
                                    .to_string(),
                            ),
                        );
                    }
                    match socket::socketpair(
                        socket::AddressFamily::Unix,
                        socket::SockType::SeqPacket,
                        None,
                        socket::SockFlag::SOCK_CLOEXEC,
                    ) {
                        Ok((sock1, sock2)) => {
                            let call = call.unmarshall_all().unwrap();
                            let mut ret = 512;
                            if let Some(dict) = call.params.get(0) {
                                if let Param::Container(Container::Dict(dict)) = dict {
                                    if let Some(mtu) =
                                        dict.map.get(&Base::String("mtu".to_string()))
                                    {
                                        if let Param::Container(Container::Variant(mtu)) = mtu {
                                            if let Param::Base(Base::Uint16(mtu)) = mtu.value {
                                                ret = ret.min(mtu);
                                            } else {
                                                return call.dynheader.make_error_response("UnexpectedType".to_string(), Some("Expected a UInt16 as variant type for offset key.".to_string()));
                                            }
                                        } else {
                                            return call.dynheader.make_error_response("UnexpectedType".to_string(), Some("Expected a dict of variants as first parameter".to_string()));
                                        }
                                    }
                                } else {
                                    return call.dynheader.make_error_response(
                                        "UnexpectedType".to_string(),
                                        Some("Expected a dict as first parameter".to_string()),
                                    );
                                }
                            }
                            let mut res = call.make_response();
                            res.raw_fds.push(sock1);
                            res.dynheader.num_fds = Some(1);
                            res.body.push_param2(UnixFd(0), ret).unwrap();
                            base.write = Some(sock2);
                            return res;
                        }
                        Err(_) => {
                            return call.dynheader.make_error_response(
                                BLUEZ_FAILED.to_string(),
                                Some(
                                    "An IO Error occured when creating the unix datagram socket."
                                        .to_string(),
                                ),
                            )
                        }
                    }
                } else {
                    call.dynheader.make_error_response(
                        BLUEZ_NOT_PERM.to_string(),
                        Some("This is not a writable characteristic.".to_string()),
                    )
                }
            }
            "AcquireNotify" => {
                if !(base.flags.notify || base.flags.indicate) {
                    call.dynheader.make_error_response(
                        BLUEZ_NOT_PERM.to_string(),
                        Some("This characteristic doesn't not permit notifying.".to_string()),
                    )
                } else if let Some(notify) = &base.notify {
                    let err_str = match notify {
                        Notify::Signal => "This characteristic is already notifying via signals.",
                        Notify::Fd(_) => "This characteristic is already notifying via a socket.",
                    };
                    call.dynheader.make_error_response(
                        "org.bluez.Error.InProgress".to_string(),
                        Some(err_str.to_string()),
                    )
                } else {
                    match socket::socketpair(
                        socket::AddressFamily::Unix,
                        socket::SockType::SeqPacket,
                        None,
                        socket::SockFlag::SOCK_CLOEXEC,
                    ) {
                        Ok((sock1, sock2)) => {
                            let call = call.unmarshall_all().unwrap();
                            let mut ret = 255;
                            if let Some(dict) = call.params.get(0) {
                                if let Param::Container(Container::Dict(dict)) = dict {
                                    if let Some(mtu) =
                                        dict.map.get(&Base::String("mtu".to_string()))
                                    {
                                        if let Param::Container(Container::Variant(mtu)) = mtu {
                                            if let Param::Base(Base::Uint16(mtu)) = mtu.value {
                                                ret = ret.min(mtu);
                                            } else {
                                                return call.dynheader.make_error_response("UnexpectedType".to_string(), Some("Expected a dict of UInt16 as first offset type".to_string()));
                                            }
                                        } else {
                                            return call.dynheader.make_error_response("UnexpectedType".to_string(), Some("Expected a dict of variants as first parameter".to_string()));
                                        }
                                    }
                                } else {
                                    return call.dynheader.make_error_response(
                                        "UnexpectedType".to_string(),
                                        Some("Expected a dict as first parameter".to_string()),
                                    );
                                }
                            }
                            let mut res = call.make_response();
                            res.body
                                .push_old_params(&[
                                    Param::Base(Base::UnixFd(0)),
                                    Param::Base(Base::Uint16(ret)),
                                ])
                                .unwrap();
                            res.dynheader.num_fds = Some(1);
                            res.raw_fds.push(sock1);
                            base.notify = Some(Notify::Fd(sock2));
                            res
                        }
                        Err(_) => call.dynheader.make_error_response(
                            BLUEZ_FAILED.to_string(),
                            Some(
                                "An IO Error occured when creating the unix datagram socket."
                                    .to_string(),
                            ),
                        ),
                    }
                }
            }
            "StartNotify" => {
                if !base.flags.notify {
                    call.dynheader.make_error_response(
                        BLUEZ_NOT_PERM.to_string(),
                        Some("This characteristic doesn't not permit notifying.".to_string()),
                    )
                } else if let Some(notify) = &base.notify {
                    let err_str = match notify {
                        Notify::Signal => "This characteristic is already notifying via signals.",
                        Notify::Fd(_) => "This characteristic is already notifying via a socket.",
                    };
                    call.dynheader.make_error_response(
                        "org.bluez.Error.InProgress".to_string(),
                        Some(err_str.to_string()),
                    )
                } else {
                    base.notify = Some(Notify::Signal);
                    call.dynheader.make_response()
                }
            }
            "StopNotify" => {
                if let Some(_) = base.notify.as_ref() {
                    base.notify = None;
                    call.dynheader.make_response()
                } else {
                    call.dynheader.make_error_response(
                        BLUEZ_FAILED.to_string(),
                        Some("Notify has not been started".to_string()),
                    )
                }
            }
            "Confirm" => {
                self.check_write_fd();
                call.dynheader.make_response()
            }
            _ => call
                .dynheader
                .make_error_response(UNKNOWN_METHOD.to_string(), None),
        }
    }
    pub fn write_val_or_fn(&mut self, val: &mut ValOrFn) {
        let base = self.get_char_base_mut();
        std::mem::swap(&mut base.vf, val);
    }
    pub fn check_write_fd(&mut self) {
        let mut base = self.get_char_base_mut();
        if let Some(write_fd) = base.write {
            let mut msg_buf = [0; 512];
            loop {
                match socket::recvmsg(
                    write_fd,
                    &[IoVec::from_mut_slice(&mut msg_buf)],
                    None,
                    socket::MsgFlags::MSG_DONTWAIT,
                ) {
                    Ok(recvmsg) => {
                        let l = recvmsg.bytes;
                        if let Some(cb) = &mut base.write_callback {
                            match cb(&msg_buf[..l]) {
                                Ok((vf, notify)) => {
                                    if let Some(vf) = vf {
                                        base.vf = vf;
                                    }
                                    if notify {
                                        drop(base);
                                        if let Err(e) = self.notify() {
                                            // TODO: better way to handle this?
                                            eprintln!("Warning: notify failed: {:?}", e);
                                        }
                                        base = self.get_char_base_mut();
                                    }
                                }
                                Err(_) => continue,
                            }
                        }
                    }
                    Err(e) => match e {
                        nix::Error::Sys(errno) => match errno {
                            Errno::EAGAIN => break,
                            _ => {
                                close(write_fd).ok();
                                base.write = None;
                                break;
                            }
                        },
                        _ => unreachable!(),
                    },
                }
            }
        }
    }
    fn signal_change(&mut self) -> Result<(), Error> {
        let base = self.get_char_base_mut();
        //let (v, l) = self.get_char_base_mut().vf.to_value();
        let (v, l) = base.vf.to_value();
        let value = &v[..l];
        let mut params = Vec::with_capacity(3); // TODO: eliminate this allocations
        params.push(Param::Base(Base::String(CHAR_IF_STR.to_string())));
        let changed_vec: Vec<Param> = value
            .into_iter()
            .map(|&b| Param::Base(Base::Byte(b)))
            .collect();
        let changed_arr = params::Array {
            element_sig: signature::Type::Base(signature::Base::Byte),
            values: changed_vec,
        };
        let changed_param = Param::Container(Container::Array(changed_arr));
        let mut changed_map = HashMap::with_capacity(1);
        changed_map.insert(Base::String(VALUE_PROP.to_string()), changed_param);
        let changed_dict = params::Dict {
            key_sig: signature::Base::String,
            value_sig: signature::Type::Container(signature::Container::Array(Box::new(
                signature::Type::Base(signature::Base::Byte),
            ))),
            map: changed_map,
        };
        params.push(Param::Container(Container::Dict(changed_dict)));

        let empty = params::Array {
            element_sig: signature::Type::Base(signature::Base::String),
            values: Vec::new(),
        };
        let empty = Param::Container(Container::Array(empty));
        params.push(empty);
        let base = self.get_char_base_mut();
        let mut msg = MessageBuilder::new()
            .signal(
                PROP_IF.0.to_string(),
                PROP_CHANGED_SIG.to_string(),
                base.path.to_str().unwrap().to_string(),
            )
            .build();
        msg.body.push_old_params(&params).unwrap();
        // eprintln!("msg to be send: {:#?}", msg);
        self.service
            .bt
            .rpc_con
            .send_message(&mut msg, Timeout::Infinite)?;
        Ok(())
    }
    pub fn notify(&mut self) -> Result<(), Error> {
        let base = self.get_char_base_mut();
        if let Some(notify) = &mut base.notify {
            let (buf, len) = base.vf.to_value();
            match notify {
                Notify::Signal => self.signal_change()?,
                Notify::Fd(sock) => {
                    if let Err(_) = socket::send(*sock, &buf[..len], socket::MsgFlags::MSG_EOR) {
                        base.notify = None;
                    }
                }
            }
        }
        Ok(())
    }
    fn get_char_base_mut(&mut self) -> &mut LocalCharBase {
        self.service
            .get_service_base_mut()
            .chars
            .get_mut(&self.uuid)
            .unwrap()
    }
    fn get_char_base(&self) -> &LocalCharBase {
        &self.service.get_service_base().chars[&self.uuid]
    }
}

/// Flags for GATT characteristics. What each flags does it detailed on 
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
    fn to_strings(&self) -> Vec<String> {
        let mut ret = Vec::new();
        if self.broadcast {
            ret.push("broadcast".to_string());
        }
        if self.read {
            ret.push("read".to_string());
        }
        if self.write {
            ret.push("write".to_string())
        }
        if self.write_wo_response {
            ret.push("write-without-response".to_string());
        }
        if self.notify {
            ret.push("notify".to_string());
        }
        if self.indicate {
            ret.push("indicate".to_string());
        }
        if self.auth_signed_writes {
            unimplemented!();
            ret.push("authenticated-signed-writes".to_string());
        }
        if self.extended_properties {
            ret.push("extended-properties".to_string());
        }
        if self.reliable_write {
            ret.push("reliable-write".to_string());
        }
        if self.writable_auxiliaries {
            unimplemented!();
            ret.push("writable-auxiliaries".to_string());
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
    fn from_strings(flags: &[String]) -> CharFlags {
        let mut ret = CharFlags::default();
        for flag in flags {
            match flag.as_str() {
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
                _ => unimplemented!(),
            }
        }
        ret
    }
}

impl Characteristic for LocalCharactersitic<'_, '_> {
    /// Reads the local value of the characteristic. If the value
    /// of the characteristic is given by a function, it will be executed.
    fn read(&mut self) -> Result<([u8; 512], usize), Error> {
        self.check_write_fd();
        let base = self.get_char_base_mut();
        Ok(base.vf.to_value())
        /*match &mut base.vf {
            ValOrFn::Value(buf, len) => Ok((*buf, *len)),
            ValOrFn::Function(f) => Ok(f()),
        }*/
    }
    /// For `LocalCharactersitic` this function is identical to `read()`
    /// (This does not not hold true for other implementors of this trait).
    ///
    /// # Notes
    // TODO: implement this note
    /// In the future, if the LocalCharactersitic value is given by a function,
    /// a function, then this function may read a cached version of it.
    #[inline]
    fn read_cached(&mut self) -> Result<([u8; 512], usize), Error> {
        self.read()
    }
    fn write(&mut self, val: &[u8]) -> Result<(), Error> {
        self.check_write_fd();
        let base = self.get_char_base_mut();
        let mut buf = [0; 512];
        //eprintln!("writing to char: {:?}", val);
        buf[..val.len()].copy_from_slice(val);
        let val = ValOrFn::Value(buf, val.len());
        base.vf = val;
        Ok(())
    }
    /*fn service(&self) -> &Path {
        let base = self.get_char_base();
        base.path.parent().unwrap()
    }*/
    fn write_acquired(&self) -> bool {
        let base = self.get_char_base();
        if let Some(_) = base.write {
            true
        } else {
            false
        }
    }
    fn notify_acquired(&self) -> bool {
        let base = self.get_char_base();
        if let Some(Notify::Fd(_)) = base.notify {
            true
        } else {
            false
        }
    }
    fn notifying(&self) -> bool {
        let base = self.get_char_base();
        base.notify.is_some()
    }
    fn uuid(&self) -> &str {
        let base = self.get_char_base();
        &base.uuid
    }
    fn flags(&self) -> CharFlags {
        let base = self.get_char_base();
        base.flags
    }
}
impl Introspectable for LocalCharBase {
    fn introspectable_str(&self) -> String {
        let mut ret = String::new();
        ret.push_str(INTROSPECT_FMT_P1);
        ret.push_str(self.path.to_str().unwrap());
        ret.push_str(INTROSPECT_FMT_P2);
        ret.push_str(PROP_STR);
        ret.push_str(CHAR_STR);
        let children: Vec<&str> = self
            .descs
            .values()
            .map(|s| s.path.file_name().unwrap().to_str().unwrap())
            .collect();
        child_nodes(&children, &mut ret);
        ret.push_str(INTROSPECT_FMT_P3);
        ret
    }
}

impl Properties for LocalCharBase {
    const INTERFACES: &'static [(&'static str, &'static [&'static str])] = &[CHAR_IF, PROP_IF];
    fn get_inner<'a, 'b>(&mut self, interface: &str, prop: &str) -> Option<Param<'a, 'b>> {
        eprintln!(
            "org.freedesktop.DBus.Charactersitic interface:\n{}, prop {}",
            interface, prop
        );
        match interface {
            CHAR_IF_STR => match prop {
                UUID_PROP => Some(base_param_to_variant(self.uuid.to_string().into())),
                SERVICE_PROP => Some(base_param_to_variant(Base::ObjectPath(
                    self.path.parent().unwrap().to_str().unwrap().to_string(),
                ))),
                VALUE_PROP => {
                    let (v, l) = self.vf.to_value();
                    eprintln!("vf: {:?}\nValue: {:?}", self.vf, &v[..l]);
                    let vec: Vec<Param> =
                        v[..l].into_iter().map(|i| Base::Byte(*i).into()).collect();
                    let val = Param::Container(Container::Array(params::Array {
                        element_sig: signature::Type::Base(signature::Base::Byte),
                        values: vec,
                    }));
                    let var = Box::new(params::Variant {
                        sig: signature::Type::Container(signature::Container::Array(Box::new(
                            signature::Type::Base(signature::Base::Byte),
                        ))),
                        value: val,
                    });
                    Some(Param::Container(Container::Variant(var)))
                }
                WRITE_ACQUIRED_PROP => {
                    Some(base_param_to_variant(Base::Boolean(self.write.is_some())))
                }
                NOTIFY_ACQUIRED_PROP => {
                    Some(base_param_to_variant(Base::Boolean(self.notify.is_some())))
                }
                NOTIFYING_PROP => Some(base_param_to_variant(Base::Boolean(self.notify.is_some()))),
                FLAGS_PROP => {
                    let flags = self.flags.to_strings();
                    let vec = flags.into_iter().map(|s| Base::String(s).into()).collect();
                    let val = Param::Container(Container::Array(params::Array {
                        element_sig: signature::Type::Base(signature::Base::String),
                        values: vec,
                    }));
                    let var = Box::new(params::Variant {
                        sig: signature::Type::Container(signature::Container::Array(Box::new(
                            signature::Type::Base(signature::Base::String),
                        ))),
                        value: val,
                    });
                    Some(Param::Container(Container::Variant(var)))
                }
                HANDLE_PROP => Some(base_param_to_variant(Base::Uint16(self.handle))),
                INCLUDES_PROP => None, // TODO: implement
                _ => None,
            },
            PROP_IF_STR => match prop {
                _ => None,
            },
            _ => None,
        }
    }
    fn set_inner(&mut self, interface: &str, prop: &str, val: Variant) -> Option<String> {
        match interface {
            SERV_IF_STR => match prop {
                HANDLE_PROP => {
                    if let Variant::Uint16(handle) = val {
                        eprintln!("setting Handle prop: {:?}", handle); // TODO remove
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
pub struct RemoteCharBase {
    pub(crate) uuid: UUID,
    chars: HashMap<UUID, RemoteDescBase>,
    notify_fd: Option<RawFd>,
    path: PathBuf,
}
impl RemoteCharBase {
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
        Ok(RemoteCharBase {
            uuid,
            chars: HashMap::new(),
            notify_fd: None,
            path,
        })
    }
}
impl Drop for RemoteCharBase {
    fn drop(&mut self) {
        if let Some(fd) = self.notify_fd {
            close(fd).ok();
        }
    }
}
pub struct RemoteChar<'a, 'b, 'c> {
    pub(super) uuid: UUID,
    pub(super) service: &'a mut RemoteService<'b, 'c>,
    #[cfg(feature = "unsafe-opt")]
    ptr: *mut RemoteCharBase,
}

impl<'a, 'b, 'c> RemoteChar<'a, 'b, 'c> {
    pub fn acquire_notify<'sel>(&'sel mut self) -> Result<RawFd, Error> {
        let base = self.get_char_mut();
        let mut msg = MessageBuilder::new()
            .call("AcquireNotify".to_string())
            .on(base.path.to_str().unwrap().to_string())
            .at(BLUEZ_DEST.to_string())
            .with_interface(CHAR_IF_STR.to_string())
            .build();
        let options = Param::Container(Container::Dict(params::Dict {
            key_sig: signature::Base::String,
            value_sig: signature::Type::Container(signature::Container::Variant),
            map: HashMap::new(),
        }));
        msg.body.push_old_param(&options).unwrap();
        let blue = self.get_blue_mut();
        let res_idx = blue.rpc_con.send_message(&mut msg, Timeout::Infinite)?;
        loop {
            blue.process_requests()?;
            if let Some(res) = blue.rpc_con.try_get_response(res_idx) {
                let res = res.unmarshall_all().unwrap();
                return match res.typ {
                    MessageType::Reply => {
                        let fd = if let Some(Param::Base(Base::UnixFd(fd))) = res.params.get(0) {
                            *fd
                        } else {
                            return Err(Error::DbusReqErr(
                                "Response returned unexpected of parameter".to_string(),
                            ));
                        };
                        let fd = res.raw_fds[fd as usize];
                        let base = self.get_char_mut();
                        base.notify_fd = Some(fd);
                        Ok(fd)
                    }
                    MessageType::Error => Err(Error::try_from(&res).unwrap()),
                    _ => unreachable!(),
                };
            }
        }
    }
    pub fn try_get_notify(&self) -> Result<Option<([u8; 255], usize)>, Error> {
        let base = self.get_char();
        let fd = match base.notify_fd {
            Some(fd) => fd,
            None => return Ok(None),
        };
        let mut ret = [0; 255];
        let msg = socket::recvmsg(
            fd,
            &[IoVec::from_mut_slice(&mut ret)],
            None,
            socket::MsgFlags::MSG_DONTWAIT,
        )?;
        Ok(Some((ret, msg.bytes)))
    }
    pub fn wait_get_notify(
        &self,
        timeout: Option<Duration>,
    ) -> Result<Option<([u8; 255], usize)>, Error> {
        let base = self.get_char();
        let fd = match base.notify_fd {
            Some(fd) => fd,
            None => return Ok(None),
        };
        let timeout = match timeout {
            Some(dur) => dur.as_micros(),
            None => 0,
        };
        let mut ret = [0; 255];
        let msg = if timeout == 0 {
            socket::recvmsg(
                fd,
                &[IoVec::from_mut_slice(&mut ret)],
                None,
                socket::MsgFlags::MSG_DONTWAIT,
            )?
        } else {
            let tv = TimeVal::microseconds(timeout.try_into().unwrap());
            socket::setsockopt(fd, socket::sockopt::ReceiveTimeout, &tv)?;
            socket::recvmsg(
                fd,
                &[IoVec::from_mut_slice(&mut ret)],
                None,
                socket::MsgFlags::empty(),
            )?
        };
        Ok(Some((ret, msg.bytes)))
    }
    pub fn get_notify_fd(&self) -> Option<RawFd> {
        let base = self.get_char();
        base.notify_fd
    }
    fn get_blue_mut(&mut self) -> &mut Bluetooth {
        self.service.dev.blue
    }
    fn get_char(&self) -> &RemoteCharBase {
        #[cfg(feature = "unsafe-opt")]
        unsafe {
            return &*self.ptr;
        }
        let service = &self.service;
        let dev = &service.dev;
        let blue = &dev.blue;
        &blue.devices[&dev.mac].services[&service.uuid].chars[&self.uuid]
    }
    fn get_char_mut(&mut self) -> &mut RemoteCharBase {
        #[cfg(feature = "unsafe-opt")]
        unsafe {
            return &mut *self.ptr;
        }
        let service = &mut self.service;
        let dev = &mut service.dev;
        let blue = &mut dev.blue;
        blue.devices
            .get_mut(&dev.mac)
            .unwrap()
            .services
            .get_mut(&service.uuid)
            .unwrap()
            .chars
            .get_mut(&self.uuid)
            .unwrap()
    }
    /*pub fn start_notify(&self) -> ();*/
}

impl Characteristic for RemoteChar<'_, '_, '_> {
    /// Reads a value from the remote device's characteristic.
    fn read(&mut self) -> Result<([u8; 512], usize), Error> {
        let base = self.get_char_mut();
        let path = base.path.to_str().unwrap().to_string();
        let mut msg = MessageBuilder::new()
            .call("ReadValue".to_string())
            .on(path)
            .at(BLUEZ_DEST.to_string())
            .with_interface(CHAR_IF_STR.to_string())
            .build();
        let cont: Container = (
            signature::Base::String,
            signature::Type::Container(signature::Container::Variant),
            HashMap::new(),
        )
            .try_into()
            .unwrap();
        msg.body.push_old_param(&mut cont.into()).unwrap();
        let blue = &mut self.service.dev.blue;
        let res_idx = blue.rpc_con.send_message(&mut msg, Timeout::Infinite)?;
        loop {
            blue.process_requests()?;
            if let Some(res) = blue.rpc_con.try_get_response(res_idx) {
                match res.typ {
                    MessageType::Reply => {
                        let mut v = [0; 512];
                        let buf: &[u8] = res.body.parser().get()?;
                        let l = buf.len();
                        v[..l].copy_from_slice(buf);
                        return Ok((v, l));
                    }
                    MessageType::Error => {
                        return Err(Error::DbusReqErr(format!("Read call failed: {:?}", res)))
                    }
                    _ => unreachable!(),
                }
            }
        }
    }
    fn read_cached(&mut self) -> Result<([u8; 512], usize), Error> {
        unimplemented!()
    }
    fn write(&mut self, _val: &[u8]) -> Result<(), Error> {
        unimplemented!()
    }
    fn uuid(&self) -> &str {
        &self.uuid
    }
    /*fn service(&self) -> &Path {
        unimplemented!()
    }*/
    fn write_acquired(&self) -> bool {
        unimplemented!()
    }
    fn notify_acquired(&self) -> bool {
        unimplemented!()
    }
    fn notifying(&self) -> bool {
        unimplemented!()
    }
    fn flags(&self) -> CharFlags {
        unimplemented!()
    }
}
