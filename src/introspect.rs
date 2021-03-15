use crate::*;
use rustbus_core::message_builder::MarshalledMessage;
use std::fmt::Write;
pub(crate) const INTROSPECT_FMT_P1: &'static str = "<!DOCTYPE node PUBLIC \"-//freedesktop//DTD D-BUS Object Introspection 1.0//EN\" \"http://www.freedesktop.org/standards/dbus/1.0/introspect.dtd\">
 <node>
\t<interface name=\"org.freedesktop.DBus.Introspectable\">
\t\t<method name=\"Introspect\">
\t\t\t<arg name=\"xml_data\" type=\"s\" direction=\"out\"/>
\t\t</method>
\t</interface>\n";

pub(crate) const INTROSPECT_FMT_P3: &'static str = " </node>";

pub(crate) const PROP_STR: &'static str = "\t<interface name=\"org.freedesktop.DBus.Properties\">
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
pub(crate) const ADV_STR: &'static str = "\t<interface name=\"org.bluez.LEAdvertisement1\">
\t\t<method name=\"Release\"/>
\t\t<property name=\"Type\" type=\"s\" access=\"readwrite\"/>
\t\t<property name=\"ServiceUUIDs\" type=\"as\" access=\"readwrite\"/>
\t\t<property name=\"SolicitUUIDs\" type=\"as\" access=\"readwrite\"/>
\t\t<property name=\"ServiceData\" type=\"a{sv}\" access=\"readwrite\"/>
\t\t<property name=\"ManufacturerData\" type=\"a{qa{y}}\" access=\"readwrite\"/>
\t\t<property name=\"Data\" type=\"a{sv}\" access=\"readwrite\"/>
\t\t<property name=\"Discoverable\" type=\"b\" access=\"readwrite\"/>
\t\t<property name=\"DiscoverableTimeout\" type=\"q\" access=\"readwrite\"/>
\t\t<property name=\"Includes\" type=\"as\" access=\"readwrite\"/>
\t\t<property name=\"LocalName\" type=\"s\" access=\"readwrite\"/>
\t\t<property name=\"Appearance\" type=\"q\" access=\"readwrite\"/>
\t\t<property name=\"Duration\" type=\"q\" access=\"readwrite\"/>
\t\t<property name=\"Timeout\" type=\"q\" access=\"readwrite\"/>
\t\t<property name=\"SecondaryChannel\" type=\"s\" access=\"readwrite\"/>
\t</interface>\n";
//TODO: implement for ADV_STR: \t\t<property name=\"ManufacturerData\" type=\"a{sv}\" access=\"readwrite\"/>
pub(crate) const SERVICE_STR: &'static str = "\t<interface name=\"org.bluez.GattService1\">
\t\t<property name=\"UUID\" type=\"s\" access=\"read\"/>
\t\t<property name=\"Primary\" type=\"b\" access=\"read\"/>
\t\t<property name=\"Device\" type=\"o\" access=\"read\"/>
\t\t<property name=\"Handle\" type=\"q\" access=\"read\"/>
\t</interface>\n";
pub(crate) const CHAR_STR: &'static str = "\t<interface name=\"org.bluez.GattCharacteristic1\">
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
pub(crate) const DESC_STR: &'static str = "\t<interface name=\"org.bluez.GattDescriptor1\">
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
pub(crate) const MANGAGER_STR: &'static str = "\t<interface name=\"org.freedesktop.DBus.ObjectManager\">
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
        write!(dst, "\t<node name=\"{}\"/>\n", child.as_ref()).unwrap();
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
