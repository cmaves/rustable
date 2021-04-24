use std::collections::HashMap;
use std::os::unix::io::FromRawFd;
use std::sync::Arc;

use super::{is_hung_up, AttValue, CharFlags};
use crate::interfaces::get_prop_call;
use crate::introspect::get_children;
use crate::*;

use futures::future::try_join_all;
use rustbus_core::path::ObjectPathBuf;
use rustbus_core::wire::unixfd::UnixFd;

use async_std::os::unix::net::UnixDatagram;

pub struct Service {
    conn: Arc<RpcConn>,
    path: ObjectPathBuf,
    uuid: UUID,
}

impl Service {
    pub fn uuid(&self) -> UUID {
        self.uuid
    }
    pub(crate) async fn get_service(
        conn: Arc<RpcConn>,
        child: ObjectPathBuf,
    ) -> Result<Option<Self>, Error> {
        let name: &str = child.file_name().unwrap();
        if !name.starts_with("service") {
            return Ok(None);
        }
        let path_str: &str = child.as_ref();
        let call = get_prop_call(path_str, BLUEZ_DEST, BLUEZ_SER_IF, "UUID");
        let res = conn.send_msg_with_reply(&call).await?.await?;
        let uuid_str = match is_msg_err::<BluezOptions>(&res) {
            Ok(BluezOptions::Str(s)) => s,
            _ => return Ok(None),
        };
        let uuid = match UUID::from_str(uuid_str) {
            Ok(u) => u,
            Err(_) => return Ok(None),
        };
        Ok(Some(Self {
            conn,
            path: child,
            uuid,
        }))
    }
    pub async fn get_characteristics(&self) -> Result<Vec<Characteristic>, Error> {
        let services = self.get_chars_stream().await?;
        let fut = |s: Option<Characteristic>| async move { Ok(s) };
        let ret = services.try_filter_map(fut).try_collect().await?;
        Ok(ret)
    }
    async fn get_chars_stream(
        &self,
    ) -> Result<
        FuturesUnordered<impl Future<Output = Result<Option<Characteristic>, Error>> + '_>,
        Error,
    >
//-> Result<impl TryStream<Ok=Option<LocalService>, Error=Error> +'_, Error>
    {
        let children: FuturesUnordered<_> = get_children(&self.conn, BLUEZ_DEST, &self.path)
            .await?
            .into_iter()
            .map(|child| Characteristic::get_char(self.conn.clone(), child))
            .collect();

        Ok(children)
    }
    pub async fn get_characteristic(&self, uuid: UUID) -> Result<Characteristic, Error> {
        let mut characters = self.get_chars_stream().await?;
        while let Some(res) = characters.next().await {
            if let Some(character) = res? {
                if character.uuid() == uuid {
                    return Ok(character);
                }
            }
        }
        Err(Error::UnknownChrc(self.uuid, uuid))
    }
    pub async fn get_includes(&self) -> Result<Vec<Self>, Error> {
        let call = get_prop_call(self.path.clone(), BLUEZ_DEST, BLUEZ_SER_IF, "Includes");
        let res = self.conn.send_msg_with_reply(&call).await?.await?;
        let paths: Vec<&ObjectPath> = match is_msg_err(&res) {
            Ok(BluezOptions::Paths(paths)) => paths,
            Ok(_) => return Err(Error::Dbus(format!("Variant was wrong type!"))),
            Err(e) => return Err(e),
        };
        let uuid_futs = paths.into_iter().map(|p: &ObjectPath| async move {
            let path = p.to_owned();
            let call = get_prop_call(path.clone(), BLUEZ_DEST, BLUEZ_SER_IF, "UUID");
            let res = self.conn.send_msg_with_reply(&call).await?.await?;
            let uuid: UUID = is_msg_err(&res)?;
            Ok(Self {
                conn: self.conn.clone(),
                path,
                uuid,
            })
        });
        try_join_all(uuid_futs).await
    }
}

pub struct Characteristic {
    conn: Arc<RpcConn>,
    uuid: UUID,
    path: ObjectPathBuf,
}
impl Characteristic {
    pub fn uuid(&self) -> UUID {
        self.uuid
    }
    async fn get_char(conn: Arc<RpcConn>, child: ObjectPathBuf) -> Result<Option<Self>, Error> {
        let name: &str = child.file_name().unwrap();
        if !name.starts_with("char") {
            return Ok(None);
        }
        let path_str: &str = child.as_ref();
        let call = get_prop_call(path_str, BLUEZ_DEST, BLUEZ_CHR_IF, "UUID");
        let res = conn.send_msg_with_reply(&call).await?.await?;
        let uuid_str = match is_msg_err::<BluezOptions>(&res) {
            Ok(BluezOptions::Str(s)) => s,
            _ => return Ok(None),
        };
        let uuid = match UUID::from_str(uuid_str) {
            Ok(u) => u,
            Err(_) => return Ok(None),
        };
        Ok(Some(Self {
            conn,
            path: child,
            uuid,
        }))
    }

    pub async fn get_descriptors(&self) -> Result<Vec<Descriptor>, Error> {
        let services = self.get_descs_stream().await?;
        let fut = |s: Option<Descriptor>| async move { Ok(s) };
        let ret = services.try_filter_map(fut).try_collect().await?;
        Ok(ret)
    }
    async fn get_descs_stream(
        &self,
    ) -> Result<FuturesUnordered<impl Future<Output = Result<Option<Descriptor>, Error>> + '_>, Error>
//-> Result<impl TryStream<Ok=Option<LocalService>, Error=Error> +'_, Error>
    {
        let children: FuturesUnordered<_> = get_children(&self.conn, BLUEZ_DEST, &self.path)
            .await?
            .into_iter()
            .map(|child| Descriptor::get_desc(self.conn.clone(), child))
            .collect();

        Ok(children)
    }
    pub async fn get_descriptor(&self, uuid: UUID) -> Result<Option<Descriptor>, Error> {
        let mut descriptors = self.get_descs_stream().await?;
        while let Some(res) = descriptors.next().await {
            if let Some(descriptor) = res? {
                if descriptor.uuid() == uuid {
                    return Ok(Some(descriptor));
                }
            }
        }
        Ok(None)
    }
    fn build_call(&self) -> MarshalledMessage {
        MessageBuilder::new()
            .call("")
            .with_interface(BLUEZ_CHR_IF)
            .at(BLUEZ_DEST)
            .on(self.path.clone())
            .build()
    }
    pub async fn read_value(
        &self,
        offset: u16,
    ) -> Result<impl Future<Output = Result<AttValue, Error>> + '_, Error> {
        let mut call = self.build_call();
        call.dynheader.member = Some(String::from("ReadValue"));
        let mut options = HashMap::new();
        options.insert("offset", BluezOptions::U16(offset));
        call.body.push_param(options).unwrap();
        let res_fut = self.conn.send_msg_with_reply(&call).await?;
        Ok(async {
            let res = res_fut.await?;
            let value: &[u8] = is_msg_err(&res)?;
            if value.len() > 512 {
                return Err(Error::Bluez(String::from(
                    "AttValue received was too long!",
                )));
            }
            Ok(AttValue::from(value))
        })
    }
    async fn write_value_base(
        &self,
        value: &AttValue,
        options: HashMap<&str, BluezOptions<'_, '_>>,
    ) -> Result<impl Future<Output = Result<(), Error>> + '_, Error> {
        let mut call = self.build_call();
        call.dynheader.member = Some(String::from("WriteValue"));
        call.body.push_param(value).unwrap();
        call.body.push_param(&options).unwrap();
        let res_fut = self.conn.send_msg_with_reply(&call).await?;
        Ok(async {
            let res = res_fut.await?;
            is_msg_err_empty(&res)
        })
    }
    pub async fn write_value(
        &self,
        value: &AttValue,
        offset: u16,
    ) -> Result<impl Future<Output = Result<(), Error>> + '_, Error> {
        let mut options = HashMap::new();
        options.insert("offset", BluezOptions::U16(offset));
        options.insert("type", BluezOptions::Str("request"));
        self.write_value_base(value, options).await
    }
    pub async fn write_value_wo_response(
        &self,
        value: &AttValue,
        offset: u16,
    ) -> Result<impl Future<Output = Result<(), Error>> + '_, Error> {
        let mut options = HashMap::new();
        options.insert("offset", BluezOptions::U16(offset));
        options.insert("type", BluezOptions::Str("command"));
        self.write_value_base(value, options).await
    }
    pub async fn acquire_notify(
        &self,
    ) -> Result<impl Future<Output = Result<NotifySocket, Error>> + '_, Error> {
        let mut call = self.build_call();
        call.dynheader.member = Some(String::from("AcquireNotify"));
        let options: HashMap<&str, BluezOptions> = HashMap::new();
        call.body.push_param(&options).unwrap();
        let conn = &self.conn;
        //let not_mut = &mut self.notify;
        let res_fut = conn.send_msg_with_reply(&call).await?;
        Ok(async move {
            let res = res_fut.await?;
            let (fd, mtu): (UnixFd, u16) = is_msg_err2(&res)?;
            let sock = unsafe { UnixDatagram::from_raw_fd(fd.take_raw_fd().unwrap()) };
            Ok(NotifySocket { sock, mtu })
            /*
            *not_mut = Some(NotifySocket {
                sock,
                buf: vec![0; mtu as usize],
                mtu,
            });
            Ok(())*/
        })
    }
    pub async fn acquire_write(
        &self,
    ) -> Result<impl Future<Output = Result<WriteSocket, Error>> + '_, Error> {
        let mut call = self.build_call();
        call.dynheader.member = Some(String::from("AcquireWrite"));
        let options: HashMap<&str, BluezOptions> = HashMap::new();
        call.body.push_param(&options).unwrap();
        let conn = &self.conn;
        //let write_mut = &mut self.write_wo;
        let res_fut = conn.send_msg_with_reply(&call).await?;
        Ok(async move {
            let res = res_fut.await?;
            let (fd, mtu): (UnixFd, u16) = is_msg_err2(&res)?;
            let sock = unsafe { UnixDatagram::from_raw_fd(fd.take_raw_fd().unwrap()) };
            Ok(WriteSocket { mtu, sock })
        })
    }
    pub async fn flags(
        &self,
    ) -> Result<impl Future<Output = Result<CharFlags, Error>> + '_, Error> {
        let call = get_prop_call(self.path.clone(), BLUEZ_DEST, BLUEZ_CHR_IF, "Flags");
        let res_fut = self.conn.send_msg_with_reply(&call).await?;
        Ok(async {
            let res = res_fut.await?;
            let props: Vec<&str> = is_msg_err(&res)?;
            Ok(CharFlags::from_strings(props))
        })
    }
    //fn conn_notify_write_borrow(&mut self) -> (&mut Arc<RpcConn>, &mut
}

pub struct NotifySocket {
    sock: UnixDatagram,
    mtu: u16,
}
impl NotifySocket {
    pub async fn recv_notification(&self) -> std::io::Result<AttValue> {
        let mut buf = [0; 512];
        let len = self.sock.recv(&mut buf).await?;
        if len == 0 && is_hung_up(&self.sock).unwrap_or(false) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotConnected,
                "Notify socket has hung up.",
            ));
        }
        Ok(AttValue::from(&buf[..len]))
    }
    pub fn mtu(&self) -> u16 {
        self.mtu
    }
}
pub struct WriteSocket {
    sock: UnixDatagram,
    mtu: u16,
}
impl WriteSocket {
    pub async fn write(&self, val: &AttValue) -> std::io::Result<()> {
        self.sock
            .send(&val[..val.len().min(self.mtu as usize)])
            .await?;
        Ok(())
    }
    pub fn mtu(&self) -> u16 {
        self.mtu
    }
}
pub struct Descriptor {
    conn: Arc<RpcConn>,
    path: ObjectPathBuf,
    uuid: UUID,
}

impl Descriptor {
    pub fn uuid(&self) -> UUID {
        self.uuid
    }
    pub async fn read_value(
        &self,
        offset: u16,
    ) -> Result<impl Future<Output = Result<AttValue, Error>> + '_, Error> {
        let mut call = self.build_call();
        call.dynheader.member = Some(String::from("ReadValue"));
        let mut options = HashMap::new();
        options.insert("offset", BluezOptions::U16(offset));
        call.body.push_param(options).unwrap();
        let res_fut = self.conn.send_msg_with_reply(&call).await?;
        Ok(async {
            let res = res_fut.await?;
            let value: &[u8] = is_msg_err(&res)?;
            if value.len() > 512 {
                return Err(Error::Bluez(String::from(
                    "AttValue received was too long!",
                )));
            }
            Ok(AttValue::from(value))
        })
    }
    fn build_call(&self) -> MarshalledMessage {
        MessageBuilder::new()
            .call("")
            .with_interface(BLUEZ_DES_IF)
            .at(BLUEZ_DEST)
            .on(self.path.clone())
            .build()
    }
    async fn get_desc(conn: Arc<RpcConn>, child: ObjectPathBuf) -> Result<Option<Self>, Error> {
        let name: &str = child.file_name().unwrap();
        if !name.starts_with("desc") {
            return Ok(None);
        }
        let path_str: &str = child.as_ref();
        let call = get_prop_call(path_str, BLUEZ_DEST, BLUEZ_DES_IF, "UUID");
        let res = conn.send_msg_with_reply(&call).await?.await?;
        let uuid_str = match is_msg_err::<BluezOptions>(&res) {
            Ok(BluezOptions::Str(s)) => s,
            _ => return Ok(None),
        };
        let uuid = match UUID::from_str(uuid_str) {
            Ok(u) => u,
            Err(_) => return Ok(None),
        };
        Ok(Some(Self {
            conn,
            path: child,
            uuid,
        }))
    }
}
