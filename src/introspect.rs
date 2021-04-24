use crate::*;
use rustbus_core::message_builder::MarshalledMessage;
use rustbus_core::path::{ObjectPath, ObjectPathBuf};
use std::fmt::Write;
pub(crate) const INTROSPECT_FMT_P1: &str = "<!DOCTYPE node PUBLIC \"-//freedesktop//DTD D-BUS Object Introspection 1.0//EN\" \"http://www.freedesktop.org/standards/dbus/1.0/introspect.dtd\">
 <node>
\t<interface name=\"org.freedesktop.DBus.Introspectable\">
\t\t<method name=\"Introspect\">
\t\t\t<arg name=\"xml_data\" type=\"s\" direction=\"out\"/>
\t\t</method>
\t</interface>\n";

pub(crate) const INTROSPECT_FMT_P3: &str = " </node>";

pub(crate) const PROP_STR: &str = "\t<interface name=\"org.freedesktop.DBus.Properties\">
\t\t<method name=\"Get\">
\t\t\t<arg name=\"interface_name\" type=\"s\" direction=\"in\"/>
\t\t\t<arg name=\"property_name\" type=\"s\" direction=\"in\"/>
\t\t\t<arg name=\"value\" type=\"v\" direction=\"out\"/>
\t\t</method>
\t\t<method name=\"GetAll\">
\t\t\t<arg name=\"interface_name\" type=\"s\" direction=\"in\"/>
\t\t\t<arg name=\"props\" type=\"a{sv}\" direction=\"out\"/>
\t\t</method>
\t\t<method name=\"Set\">
\t\t\t<arg name=\"interface_name\" type=\"s\" direction=\"in\"/>
\t\t\t<arg name=\"property_name\" type=\"s\" direction=\"in\"/>
\t\t\t<arg name=\"value\" type=\"v\" direction=\"in\"/>
\t\t</method>
\t</interface>\n";
pub(crate) const ADV_STR: &str = "\t<interface name=\"org.bluez.LEAdvertisement1\">
\t\t<method name=\"Release\"/>
\t\t<property name=\"Type\" type=\"s\" access=\"readwrite\"/>
\t\t<property name=\"ServiceUUIDs\" type=\"as\" access=\"readwrite\"/>
\t\t<property name=\"SolicitUUIDs\" type=\"as\" access=\"readwrite\"/>
\t\t<property name=\"ServiceData\" type=\"a{sv}\" access=\"readwrite\"/>
\t\t<property name=\"ManufacturerData\" type=\"a{qa{y}}\" access=\"readwrite\"/>
\t\t<property name=\"Data\" type=\"a{sv}\" access=\"readwrite\"/>
\t\t<property name=\"Discoverable\" type=\"b\" access=\"readwrite\"/>
\t\t<property name=\"DiscoverableTimeout\" type=\"q\" access=\"readwrite\"/>
\t\t<property name=\"Includes\" type=\"ao\" access=\"readwrite\"/>
\t\t<property name=\"LocalName\" type=\"s\" access=\"readwrite\"/>
\t\t<property name=\"Appearance\" type=\"q\" access=\"readwrite\"/>
\t\t<property name=\"Duration\" type=\"q\" access=\"readwrite\"/>
\t\t<property name=\"Timeout\" type=\"q\" access=\"readwrite\"/>
\t\t<property name=\"SecondaryChannel\" type=\"s\" access=\"readwrite\"/>
\t</interface>\n";
//TODO: implement for ADV_STR: \t\t<property name=\"ManufacturerData\" type=\"a{sv}\" access=\"readwrite\"/>
pub(crate) const SERVICE_STR: &str = "\t<interface name=\"org.bluez.GattService1\">
\t\t<property name=\"UUID\" type=\"s\" access=\"read\"/>
\t\t<property name=\"Primary\" type=\"b\" access=\"read\"/>
\t\t<property name=\"Device\" type=\"o\" access=\"read\"/>
\t\t<property name=\"Handle\" type=\"q\" access=\"read\"/>
\t\t<property name=\"Includes\" type=\"as\" access=\"read\"/>
\t</interface>\n";
pub(crate) const CHAR_STR: &str = "\t<interface name=\"org.bluez.GattCharacteristic1\">
\t\t<method name=\"ReadValue\">
\t\t\t<arg name=\"options\" type=\"a{sv}\" direction=\"in\"/>
\t\t\t<arg name=\"value\" type=\"ay\" direction=\"out\"/>
\t\t</method>
\t\t<method name=\"WriteValue\">
\t\t\t<arg name=\"value\" type=\"ay\" direction=\"in\"/>
\t\t\t<arg name=\"options\" type=\"a{sv}\" direction=\"in\"/>
\t\t</method>
\t\t<method name=\"AcquireWrite\">
\t\t\t<arg name=\"options\" type=\"a{sv}\" direction=\"in\"/>
\t\t\t<arg name=\"write_fd\" type=\"h\" direction=\"out\"/>
\t\t\t<arg name=\"mtu\" type=\"q\" direction=\"out\"/>
\t\t</method>
\t\t<method name=\"AcquireNotify\">
\t\t\t<arg name=\"options\" type=\"a{sv}\" direction=\"in\"/>
\t\t\t<arg name=\"write_fd\" type=\"h\" direction=\"out\"/>
\t\t\t<arg name=\"mtu\" type=\"q\" direction=\"out\"/>
\t\t</method>
\t\t<method name=\"StartNotify\"/>
\t\t<method name=\"StopNotify\"/>
\t\t<method name=\"Confirm\"/>
\t\t<property name=\"UUID\" type=\"s\" access=\"read\"/>
\t\t<property name=\"Service\" type=\"s\" access=\"read\"/>
\t\t<property name=\"Value\" type=\"ay\" access=\"read\"/>
\t\t<property name=\"WriteAcquired\" type=\"b\" access=\"read\"/>
\t\t<property name=\"NotifyAcquired\" type=\"b\" access=\"read\"/>
\t\t<property name=\"Notifying\" type=\"b\" access=\"read\"/>
\t\t<property name=\"Flags\" type=\"as\" access=\"read\"/>
\t\t<property name=\"Handle\" type=\"q\" access=\"readwrite\"/>
\t</interface>\n";
pub(crate) const DESC_STR: &str = "\t<interface name=\"org.bluez.GattDescriptor1\">
\t\t<method name=\"ReadValue\">
\t\t\t<arg name=\"options\" type=\"a{sv}\" direction=\"in\"/>
\t\t\t<arg name=\"value\" type=\"ay\" direction=\"out\"/>
\t\t</method>
\t\t<method name=\"WriteValue\">
\t\t\t<arg name=\"value\" type=\"ay\" direction=\"in\"/>
\t\t\t<arg name=\"options\" type=\"a{sv}\" direction=\"in\"/>
\t\t</method>
\t\t<property name=\"UUID\" type=\"s\" access=\"read\"/>
\t\t<property name=\"Characteristic\" type=\"s\" access=\"read\"/>
\t\t<property name=\"Value\" type=\"ay\" access=\"read\"/>
\t\t<property name=\"Flags\" type=\"as\" access=\"read\"/>
\t\t<property name=\"Handle\" type=\"q\" access=\"readwrite\"/>
\t</interface>\n";
pub(crate) const MANGAGER_STR: &str = "\t<interface name=\"org.freedesktop.DBus.ObjectManager\">
\t\t<method name=\"GetManagedObjects\">
\t\t\t<arg type=\"a{oa{sa{sv}}}\" name=\"object_paths_interfaces_and_properties\" direction=\"out\"/>
\t\t</method>
\t\t<signal name=\"InterfacesAdded\">
\t\t\t<arg type=\"o\" name=\"object_path\"/>
\t\t\t<arg type=\"a{sa{sv}}\" name=\"interfaces_and_properties\"/>
\t\t</signal>
\t\t<signal name=\"InterfacesRemoved\">
\t\t\t<arg type=\"o\" name=\"object_path\"/>
\t\t\t<arg type=\"as\" name=\"interfaces\"/>
\t\t</signal>
\t</interface>\n";
pub(crate) fn child_nodes<S: AsRef<str>, T: IntoIterator<Item = S>>(children: T, dst: &mut String) {
    for child in children {
        writeln!(dst, "\t<node name=\"{}\"/>", child.as_ref()).unwrap();
    }
}
pub trait Introspectable {
    fn introspectable(&self, call: MarshalledMessage) -> MarshalledMessage {
        let call = call.unmarshall_all().unwrap();
        let mut reply = call.make_response();
        reply.body.push_param(self.introspectable_str()).unwrap();
        reply
    }
    fn introspectable_str(&self) -> String;
}

use xml::reader::{Error as XmlError, EventReader, XmlEvent};
fn ignore_xml_tree(reader: &mut EventReader<&[u8]>, name: &str) -> Result<(), XmlError> {
    loop {
        match reader.next()? {
            XmlEvent::StartElement { name, .. } => ignore_xml_tree(reader, &name.local_name)?,
            XmlEvent::EndElement { name: e_name, .. } if e_name.local_name == name => return Ok(()),
            XmlEvent::EndElement { .. }
            | XmlEvent::StartDocument { .. }
            | XmlEvent::EndDocument { .. } => unreachable!(),
            _ => {}
        }
    }
}
fn invalid_introspect<T>(_: T) -> Error {
    Error::Bluez("Bluez didn't return valid introspect XML string.".into())
}
pub async fn get_children<S: AsRef<str>, P: AsRef<ObjectPath>>(
    conn: &RpcConn,
    dest: S,
    path: P,
) -> Result<Vec<ObjectPathBuf>, Error> {
    let path: &str = path.as_ref().as_ref();
    let call = MessageBuilder::new()
        .call(String::from("Introspect"))
        .with_interface(String::from("org.freedesktop.DBus.Introspectable"))
        .on(path.to_string())
        .at(dest.as_ref().to_string())
        .build();
    let res = conn.send_msg_with_reply(&call).await?.await?;
    let s: &str = res.body.parser().get().map_err(invalid_introspect)?;
    let mut reader = EventReader::from_str(s);
    if !matches!(reader.next(), Ok(XmlEvent::StartDocument { .. })) {
        unimplemented!();
    }
    loop {
        let next = reader.next();
        match &next {
            Ok(XmlEvent::StartElement { name, .. }) if name.local_name == "node" => break,
            Ok(XmlEvent::StartElement { name, .. }) => {
                ignore_xml_tree(&mut reader, &name.local_name).map_err(invalid_introspect)?
            }
            Ok(XmlEvent::EndDocument { .. }) => return Err(invalid_introspect(())),
            _ => {}
        }
    }
    let mut children = Vec::new();
    loop {
        let next = reader.next();
        match &next {
            Ok(XmlEvent::StartElement {
                name, attributes, ..
            }) if name.local_name == "node" => {
                let child_name = match attributes
                    .iter()
                    .find(|attr| attr.name.local_name == "name")
                    .map(|attr| &attr.value)
                {
                    Some(n) => n,
                    None => continue,
                };
                let mut child_path = ObjectPathBuf::with_capacity(path.len() + child_name.len());
                child_path.push_path(&path);
                child_path
                    .push_path_checked(child_name)
                    .map_err(invalid_introspect)?;
                children.push(child_path);
                ignore_xml_tree(&mut reader, "node").map_err(invalid_introspect)?;
            }
            Ok(XmlEvent::StartElement { name, .. }) => {
                ignore_xml_tree(&mut reader, &name.local_name).map_err(invalid_introspect)?
            }
            Ok(XmlEvent::EndElement { .. }) => break,
            _ => {}
        }
    }
    Ok(children)
}
