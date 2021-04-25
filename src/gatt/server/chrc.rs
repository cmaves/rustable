use async_std::channel::bounded;
use async_std::os::unix::net::UnixDatagram;
use async_std::task::spawn;
use futures::future::{select, Either};
use futures::pin_mut;
use std::collections::HashMap;
use std::io::ErrorKind;
use std::num::NonZeroU16;
use std::os::unix::io::{AsRawFd, FromRawFd};
use std::pin::Pin;

use super::*;
use crate::drop_select;
use crate::properties::{PropError, Properties};
use async_rustbus::rustbus_core;
use async_rustbus::RpcConn;
use log::{info, warn};
use rustbus_core::message_builder::MessageBuilder;
use rustbus_core::wire::UnixFd;

enum Notify {
    Socket(UnixDatagram, usize),
    Signal,
    None,
}
pub enum ShouldNotify {
    Yes,
    WithSpecialValue(AttValue),
    No,
}
pub struct Characteristic {
    descs: Vec<Descriptor>,
    value: ValOrFn,
    uuid: UUID,
    flags: CharFlags,
    write_callback: Box<dyn FnMut(AttValue) -> (Option<ValOrFn>, bool) + Send + Sync + 'static>,
    notify_cb: Box<
        dyn FnMut() -> Pin<Box<dyn Future<Output = ShouldNotify> + Send>> + Send + Sync + 'static,
    >,
    handle: u16,
}
impl Characteristic {
    pub fn new(uuid: UUID, flags: CharFlags) -> Self {
        Self {
            uuid,
            flags,
            descs: Vec::new(),
            handle: 0,
            value: ValOrFn::default(),
            write_callback: Box::new(|val| (Some(ValOrFn::Value(val)), false)),
            notify_cb: Box::new(|| futures::future::pending().boxed()),
        }
    }
    pub fn set_handle(&mut self, handle: Option<NonZeroU16>) {
        self.handle = handle.map_or(0, |u| u.into());
    }
    pub fn set_value(&mut self, value: ValOrFn) {
        self.value = value;
    }
    pub fn uuid(&self) -> UUID {
        self.uuid
    }
    pub fn add_desc(&mut self, mut desc: Descriptor) {
        match self.find_desc_unsorted(desc.uuid()) {
            Some(d) => std::mem::swap(d, &mut desc),
            None => self.descs.push(desc),
        }
    }
    pub fn remove_desc(&mut self, uuid: UUID) -> Option<Descriptor> {
        let idx = self.descs.iter().position(|d| d.uuid() == uuid)?;
        Some(self.descs.remove(idx))
    }
    pub fn drain_descs(&mut self) -> std::vec::Drain<Descriptor> {
        self.descs.drain(..)
    }
    pub fn desc_cnt(&self) -> usize {
        self.descs.len()
    }
    pub fn set_write_cb<C: FnMut(AttValue) -> (Option<ValOrFn>, bool) + Send + Sync + 'static>(
        &mut self,
        cb: C,
    ) {
        self.write_callback = Box::new(cb);
    }
    pub fn set_notify_cb<C>(&mut self, cb: C)
    where
        C: FnMut() -> Pin<Box<dyn Future<Output = ShouldNotify> + Send>> + Send + Sync + 'static,
    {
        self.notify_cb = Box::new(cb);
    }
    fn find_desc_unsorted(&mut self, uuid: UUID) -> Option<&mut Descriptor> {
        self.descs.iter_mut().find(|d| d.uuid() == uuid)
    }
    pub(super) fn sort_descs(&mut self) {
        self.descs.sort_by_key(|d| d.uuid());
    }
    pub(super) fn start_worker(
        self,
        conn: &Arc<RpcConn>,
        path: &ObjectPath,
        children: usize,
        filter: Option<Arc<str>>,
    ) -> Worker {
        let path = path.to_owned();
        let (sender, recv) = bounded::<WorkerMsg>(8);
        let conn = conn.clone();
        let mut chrc_data = ChrcData::new(self, children);
        let handle = spawn(async move {
            let recv_fut = recv.recv();
            let not_fut = (chrc_data.notify_cb)();
            let mut recv_not_fut = select(recv_fut, not_fut);
            let call_recv = conn.get_call_recv(path.as_str()).await.unwrap();
            loop {
                // This two-staged matches are done to satisfying,
                // the borrow-checker.
                let either = {
                    let call_fut = call_recv.recv();
                    let write_fut = chrc_data.check_for_write();
                    let call_write = drop_select(write_fut, call_fut);
                    pin_mut!(call_write);
                    match select(recv_not_fut, call_write).await {
                        Either::Left((call_not, _)) => Either::Left(call_not),
                        Either::Right(res) => Either::Right(res),
                    }
                };
                match either {
                    Either::Left(Either::Left((msg, not_fut))) => {
                        let msg = msg?;
                        match msg {
                            WorkerMsg::Unregister => break,
                            WorkerMsg::Update(vf, notify) => {
                                chrc_data.value = vf;
                                if notify {
                                    chrc_data.notify(&path, &conn, None).await?;
                                }
                            }
                            WorkerMsg::ObjMgr(sender) => {
                                let map = chrc_data.get_all_interfaces(&path);
                                sender.send((path.clone(), map)).ok();
                            }
                            WorkerMsg::Get(sender) => {
                                sender.send(chrc_data.value.to_value()).ok();
                            }
                            WorkerMsg::GetHandle(sender) => {
                                sender.send(NonZeroU16::new(chrc_data.handle).unwrap()).ok();
                            }
                            WorkerMsg::Notify(opt_att) => {
                                chrc_data.notify(&path, &conn, opt_att).await?;
                            }
                            WorkerMsg::Notifying(sender) => {
                                let is_notifying = !matches!(chrc_data.notify, Notify::None);
                                sender.send(is_notifying).ok();
                            }
                            WorkerMsg::NotifyAcquired(sender) => {
                                let acquired = matches!(chrc_data.notify, Notify::Socket(_, _));
                                sender.send(acquired).ok();
                            }
                            WorkerMsg::NotifyingSignal(sender) => {
                                let signaling = matches!(chrc_data.notify, Notify::Signal);
                                sender.send(signaling).ok();
                            }
                            _ => unreachable!(),
                        }
                        recv_not_fut = select(recv.recv(), not_fut);
                    }
                    Either::Left(Either::Right((should_not, recv_f))) => {
                        match should_not {
                            ShouldNotify::Yes => {
                                chrc_data.notify(&path, &conn, None).await?;
                            }
                            ShouldNotify::WithSpecialValue(val) => {
                                chrc_data.notify(&path, &conn, Some(val)).await?;
                            }
                            ShouldNotify::No => {}
                        }
                        recv_not_fut = select(recv_f, (chrc_data.notify_cb)());
                    }
                    Either::Right((Either::Left(res), recv_not_f)) => {
                        match res {
                            Ok(res) => chrc_data.handle_write(&conn, &path, res).await?,
                            Err(e) if e.kind() == ErrorKind::NotConnected => {
                                chrc_data.write_fd = None;
                            }
                            Err(e) => return Err(e.into()),
                        }
                        recv_not_fut = recv_not_f;
                    }
                    Either::Right((Either::Right(call), recv_not_f)) => {
                        let call = call?;
                        let res = if is_msg_bluez(&call, filter.as_deref()) {
                            chrc_data.handle_call(&call, &conn).await?
                        } else {
                            call.dynheader.make_error_response("PermissionDenied", None)
                        };
                        conn.send_msg_no_reply(&res).await?;
                        recv_not_fut = recv_not_f;
                    } /*
                       */
                }
            }
            Ok(WorkerJoin::Chrc(chrc_data.into_chrc()))
        });
        Worker { sender, handle }
    }
}
struct ChrcData {
    children: usize,
    value: ValOrFn,
    uuid: UUID,
    notify: Notify,
    flags: CharFlags,
    write_callback: Box<dyn FnMut(AttValue) -> (Option<ValOrFn>, bool) + Send + Sync + 'static>,
    notify_cb: Box<
        dyn FnMut() -> Pin<Box<dyn Future<Output = ShouldNotify> + Send>> + Send + Sync + 'static,
    >,
    write_fd: Option<(UnixDatagram, usize)>,
    handle: u16,
}
impl ChrcData {
    fn new(chrc: Characteristic, children: usize) -> Self {
        Self {
            children,
            handle: chrc.handle,
            value: chrc.value,
            uuid: chrc.uuid,
            flags: chrc.flags,
            write_callback: chrc.write_callback,
            notify_cb: chrc.notify_cb,
            notify: Notify::None,
            write_fd: None,
        }
    }
    fn into_chrc(self) -> Characteristic {
        Characteristic {
            uuid: self.uuid,
            handle: self.handle,
            value: self.value,
            flags: self.flags,
            write_callback: self.write_callback,
            notify_cb: self.notify_cb,
            descs: Vec::new(),
        }
    }
    async fn notify(
        &mut self,
        path: &ObjectPath,
        conn: &RpcConn,
        opt_att: Option<AttValue>,
    ) -> std::io::Result<bool> {
        let att_val = opt_att.unwrap_or_else(|| self.value.to_value());
        match &self.notify {
            Notify::None => Ok(false),
            Notify::Socket(socket, _) => match socket.send(&att_val).await {
                Ok(_) => Ok(true),
                Err(e) => {
                    info!("Write to notify fd failed: {:?}, closing socket.", e);
                    self.notify = Notify::None;
                    Ok(false)
                }
            },
            Notify::Signal => {
                let mut sig = MessageBuilder::new()
                    .signal(PROPS_IF, "PropertiesChanged", path.to_string())
                    .to("org.bluez")
                    .build();
                sig.body.push_param(BLUEZ_CHR_IF).unwrap();
                let mut map = HashMap::new();
                map.insert("Value", BluezOptions::Buf(&att_val));
                sig.body.push_param(BLUEZ_CHR_IF).unwrap();
                sig.body.push_param(&map).unwrap();
                conn.send_message(&sig).await?;
                Ok(true)
            }
        }
    }
    async fn handle_call(
        &mut self,
        call: &MarshalledMessage,
        conn: &RpcConn,
    ) -> std::io::Result<MarshalledMessage> {
        let interface = call.dynheader.interface.as_ref().unwrap();
        match &**interface {
            PROPS_IF => Ok(self.properties_call(call)),
            INTRO_IF => Ok(self.handle_intro(call)),
            BLUEZ_CHR_IF => {
                let member = call.dynheader.member.as_ref().unwrap();
                match &**member {
                    "ReadValue" => {
                        if !(self.flags.read
                            || self.flags.secure_read
                            || self.flags.encrypt_read
                            || self.flags.encrypt_auth_read)
                        {
                            return Ok(call
                                .dynheader
                                .make_error_response("PermissionDenied", None));
                        }
                        let options: HashMap<&str, BluezOptions> = match call.body.parser().get() {
                            Ok(o) => o,
                            Err(_) => {
                                return Ok(call.dynheader.make_error_response("UnknownType", None))
                            }
                        };
                        let mut offset = 0;
                        if let Some(BluezOptions::U16(off)) = options.get("offset") {
                            offset = *off as usize;
                        }
                        let att_val = self.value.to_value();
                        let val = att_val.get(offset..).unwrap_or(&[]);
                        let mut reply = call.dynheader.make_response();
                        reply.body.push_param(val).unwrap();
                        Ok(reply)
                    }
                    "WriteValue" => {
                        if !(self.flags.write
                            || self.flags.write_wo_response
                            || self.flags.encrypt_write
                            || self.flags.encrypt_auth_write)
                        {
                            return Ok(call
                                .dynheader
                                .make_error_response("PermissionDenied", None));
                        }
                        let (mut att_val, options): (AttValue, HashMap<&str, BluezOptions>) =
                            match call.body.parser().get() {
                                Ok(o) => o,
                                Err(_) => {
                                    return Ok(call
                                        .dynheader
                                        .make_error_response("UnknownType", None))
                                }
                            };
                        let mut offset = 0;
                        if let Some(BluezOptions::U16(off)) = options.get("offset") {
                            offset = *off as usize;
                        }
                        if offset != 0 {
                            let mut old = self.value.to_value();
                            old.update(&att_val, offset);
                            att_val = old;
                        }
                        let path =
                            ObjectPath::new(call.dynheader.object.as_ref().unwrap()).unwrap();
                        self.handle_write(conn, path, att_val).await?;
                        Ok(call.dynheader.make_response())
                    }
                    "AcquireNotify" => self.acquire_notify(call),
                    "AcquireWrite" => self.acquire_write(call),
                    "StartNotify" => {
                        if !(self.flags.notify || self.flags.indicate) {
                            return Ok(call
                                .dynheader
                                .make_error_response("PermissionDenied", None));
                        }
                        match &self.notify {
                            Notify::Socket(_, _) => {
                                return Ok(call.dynheader.make_error_response("FdAcquired", None))
                            }
                            Notify::Signal => {
                                return Ok(call
                                    .dynheader
                                    .make_error_response("org.Bluez.Error.InProgress", None))
                            }
                            Notify::None => {}
                        }
                        self.notify = Notify::Signal;
                        Ok(call.dynheader.make_response())
                    }
                    "StopNotify" => {
                        if !matches!(self.notify, Notify::Signal) {
                            Ok(call.dynheader.make_error_response("NotNotifying", None))
                        } else {
                            self.notify = Notify::None;
                            Ok(call.dynheader.make_response())
                        }
                    }
                    _ => Ok(call.dynheader.make_error_response("UnknownMethod", None)),
                }
            }
            _ => unreachable!(),
        }
    }
    async fn check_for_write(&self) -> std::io::Result<AttValue> {
        if let Some((socket, mtu)) = &self.write_fd {
            let mut att_val = AttValue::new(512.min(*mtu));
            // this should be the last await in the block
            let read = socket.recv(&mut att_val).await?;
            if read == 0 && is_hung_up(socket).unwrap_or(false) {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::NotConnected,
                    "Notify socket has hung up.",
                ));
            }
            att_val.resize(read, 0);
            return Ok(att_val);
        }
        futures::future::pending().await
    }
    async fn handle_write(
        &mut self,
        conn: &RpcConn,
        path: &ObjectPath,
        att_val: AttValue,
    ) -> std::io::Result<()> {
        let (new_val, notify) = (self.write_callback)(att_val);
        if let Some(val) = new_val {
            self.value = val;
        }
        if notify {
            self.notify(path, conn, None).await?;
        }
        Ok(())
    }
    fn handle_intro(&self, call: &MarshalledMessage) -> MarshalledMessage {
        let mut s = String::from(introspect::INTROSPECT_FMT_P1);
        s.push_str(introspect::PROP_STR);
        s.push_str(introspect::CHAR_STR);
        let children = (0..self.children).map(|u| format!("desc{:04x}", u));
        introspect::child_nodes(children, &mut s);
        s.push_str(introspect::INTROSPECT_FMT_P3);
        let mut reply = call.dynheader.make_response();
        reply.body.push_param(s).unwrap();
        reply
    }
    fn acquire_notify(&mut self, call: &MarshalledMessage) -> std::io::Result<MarshalledMessage> {
        if !self.flags.notify {
            // TODO: check if indication can trigger AcquireNotify call.
            warn!("An attempt was made to AcquireNotify when notify flags isn't set.");
            return Ok(call.dynheader.make_error_response("PermissionDenied", None));
        }
        // Check if notify fd has hung-up and it trying to get a new fd.
        match &self.notify {
            Notify::None => {}
            Notify::Socket(sock, _) if is_hung_up(sock)? => self.notify = Notify::None,
            Notify::Signal | Notify::Socket(_, _) => {
                warn!("An attempt was made to AcquireNotify when session was already in progress.");
                return Ok(call.dynheader.make_error_response("AlreadyAcquired", None));
            }
        }
        let options: HashMap<&str, BluezOptions> = match call.body.parser().get() {
            Ok(o) => o,
            Err(_) => return Ok(call.dynheader.make_error_response("UnknownType", None)),
        };
        let mut mtu = 517;
        if let Some(BluezOptions::U16(off)) = options.get("mtu") {
            mtu = mtu.min(*off as usize);
        }
        let (ours, theirs) = get_sock_seqpacket()?;
        info!(
            "Notify acquired: mtu: {}, our fd: {}, their fd: {}",
            mtu,
            ours.as_raw_fd(),
            theirs.as_raw_fd(),
        );
        self.notify = Notify::Socket(ours, mtu);
        let mut reply = call.dynheader.make_response();
        let fd = UnixFd::new(theirs.as_raw_fd());
        reply.body.push_param(fd).unwrap();
        reply.body.push_param(mtu as u16).unwrap();
        Ok(reply)
    }
    fn acquire_write(&mut self, call: &MarshalledMessage) -> std::io::Result<MarshalledMessage> {
        if !self.flags.write_wo_response {
            return Ok(call.dynheader.make_error_response("PermissionDenied", None));
        }
        if !matches!(self.write_fd, None) {
            return Ok(call.dynheader.make_error_response("AlreadyAcquired", None));
        }
        let options: HashMap<&str, BluezOptions> = match call.body.parser().get() {
            Ok(o) => o,
            Err(_) => return Ok(call.dynheader.make_error_response("UnknownType", None)),
        };
        let mut mtu = 517;
        if let Some(BluezOptions::U16(off)) = options.get("mtu") {
            mtu = mtu.min(*off as usize);
        }
        let (ours, theirs) = get_sock_seqpacket()?;
        self.write_fd = Some((ours, mtu));
        let mut reply = call.dynheader.make_response();
        let fd = UnixFd::new(theirs.as_raw_fd());
        reply.body.push_param(fd).unwrap();
        reply.body.push_param(mtu as u16).unwrap();
        Ok(reply)
    }
}

impl Properties for ChrcData {
    const INTERFACES: &'static [(&'static str, &'static [&'static str])] = &[(
        BLUEZ_CHR_IF,
        &[
            UUID_STR, HANDLE_STR, SERV_STR, VAL_STR, NA_STR, NO_STR, FLAG_STR, WA_STR,
        ],
    )];

    fn get_inner(
        &mut self,
        path: &ObjectPath,
        interface: &str,
        prop: &str,
    ) -> Result<BluezOptions<'static, 'static>, PropError> {
        if !matches!(interface, BLUEZ_CHR_IF) {
            return Err(PropError::InterfaceNotFound);
        }
        match prop {
            UUID_STR => Ok(BluezOptions::OwnedStr(self.uuid.to_string())),
            HANDLE_STR => Ok(BluezOptions::U16(self.handle)),
            SERV_STR => Ok(BluezOptions::OwnedPath(path.parent().unwrap().into())),
            VAL_STR => Ok(BluezOptions::OwnedBuf((&*self.value.to_value()).into())),
            NA_STR => Ok(BluezOptions::Bool(!matches!(self.notify, Notify::None))),
            NO_STR => Ok(BluezOptions::Bool(matches!(self.notify, Notify::Signal))),
            FLAG_STR => Ok(BluezOptions::Flags(self.flags.to_strings())),
            WA_STR => Ok(BluezOptions::Bool(matches!(self.write_fd, Some(_)))),
            _ => Err(PropError::PropertyNotFound),
        }
    }
    fn set_inner(
        &mut self,
        _path: &ObjectPath,
        interface: &str,
        prop: &str,
        val: BluezOptions,
    ) -> Result<(), PropError> {
        if !matches!(interface, BLUEZ_CHR_IF) {
            return Err(PropError::InterfaceNotFound);
        }
        match prop {
            HANDLE_STR => match val {
                BluezOptions::U16(h) => {
                    self.handle = h;
                    Ok(())
                }
                _ => Err(PropError::InvalidValue),
            },
            UUID_STR | SERV_STR | VAL_STR | NA_STR | NO_STR | FLAG_STR => {
                Err(PropError::PermissionDenied)
            }
            _ => Err(PropError::PropertyNotFound),
        }
    }
}

fn get_sock_seqpacket() -> std::io::Result<(UnixDatagram, UnixDatagram)> {
    unsafe {
        let mut fds = [0; 2];
        if libc::socketpair(libc::AF_UNIX, libc::SOCK_SEQPACKET, 0, fds.as_mut_ptr()) != 0 {
            Err(std::io::Error::last_os_error())
        } else {
            Ok((
                UnixDatagram::from_raw_fd(fds[0]),
                UnixDatagram::from_raw_fd(fds[1]),
            ))
        }
    }
}
